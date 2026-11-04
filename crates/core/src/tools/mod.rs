//! Тулы агента: контракт [`Tool`], реестр [`ToolRegistry`] и пять базовых
//! инструментов — [`BashTool`], [`EditTool`], [`GrepTool`], [`ReadTool`],
//! [`WriteTool`].
//!
//! Четыре свойства этого модуля важны за пределами «просто вызвать функцию»:
//!
//! * **Схемы генерируются из типов** ([`input_schema_for`] поверх `schemars`),
//!   поэтому описание, валидация и десериализация не расходятся.
//! * **Типы аргументов приводятся к схеме до валидации** — без потерь и только
//!   на верхнем уровне ([`ToolRegistry::coerce_input`]): `"60"` и `60.0` там,
//!   где ждут целое, становятся `60`, а не отбитым вызовом. Что приводится и
//!   что нет — в `coerce`.
//! * **Ошибки — это данные для модели.** Всё, что может пойти не так (неизвестный
//!   тул, мусорные аргументы, отсутствующий файл, неуникальный `old_string`),
//!   возвращается как [`ToolOutput`] с `is_error = true` и текстом «что не так и
//!   как надо» — агент отдаёт его модели как tool result, и та исправляется
//!   сама (retry-with-error). Паника или `Result::Err` наружу здесь не нужны.
//! * **Порядок тулов детерминирован.** [`ToolRegistry::defs`] отдаёт тулы в
//!   порядке регистрации, а не хеш-таблицей: префикс промпта обязан быть
//!   байт-в-байт стабильным, иначе KV-кэш локального сервера инвалидируется.
//!
//! Читающие тулы (`read`, `grep`) заперты внутри cwd общим замком (модуль
//! `confine`): путь канонизируется, наружу — ни абсолютным путём, ни `..`,
//! ни симлинком.

pub mod bash;
mod coerce;
mod confine;
pub mod edit;
pub mod grep;
pub mod read;
pub mod write;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use jsonschema::Validator;
use schemars::JsonSchema;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::provider::ToolDef;

pub use bash::BashTool;
pub use edit::{EditStrategy, EditTool, UnknownEditStrategy};
pub use grep::GrepTool;
pub use read::ReadTool;
pub use write::WriteTool;

/// Результат выполнения тула — ровно то, что уйдёт модели как tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// Текст результата (при `is_error` — текст ошибки).
    pub content: String,
    /// Завершился ли тул ошибкой.
    pub is_error: bool,
}

impl ToolOutput {
    /// Успешный результат.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Ошибка: текст уходит модели как обычный результат, чтобы она могла
    /// исправить вызов.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Контекст выполнения тула.
#[derive(Debug, Clone)]
pub struct ToolCtx {
    /// Рабочая директория: относительные пути тулов резолвятся от неё.
    pub cwd: PathBuf,
    /// Отмена хода — единственный примитив прерывания в ядре.
    pub cancel: CancellationToken,
}

impl ToolCtx {
    /// Контекст со свежим токеном отмены.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            cancel: CancellationToken::new(),
        }
    }

    /// Контекст с внешним токеном отмены (общим на весь ход).
    pub fn with_cancel(cwd: impl Into<PathBuf>, cancel: CancellationToken) -> Self {
        Self {
            cwd: cwd.into(),
            cancel,
        }
    }

    /// Абсолютный путь: абсолютный вход — как есть, относительный — от [`Self::cwd`].
    ///
    /// Только склейка: границу cwd метод не проверяет (`..`, абсолютный путь и
    /// симлинк уводят наружу). Тулы, которым наружу нельзя (`read`, `grep`),
    /// пропускают путь через замок модуля `confine`.
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    /// Отменён ли ход.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// Инструмент агента.
///
/// Реализации не возвращают `Result`: любая ошибка — это [`ToolOutput::err`] с
/// объяснением для модели.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Имя, которым модель зовёт тул (стабильное, snake_case).
    fn name(&self) -> &str;

    /// Описание для модели — короткое, оно попадает в каждый промпт.
    fn description(&self) -> &str;

    /// JSON Schema аргументов (обычно [`input_schema_for`]).
    fn input_schema(&self) -> Value;

    /// Выполнить тул. `input` уже провалидирован реестром, но реализация всё
    /// равно обязана пережить мусор (её могут вызвать напрямую).
    async fn execute(&self, input: Value, ctx: &ToolCtx) -> ToolOutput;
}

/// Схема входа тула из Rust-типа.
///
/// Схема едет в каждом запросе, поэтому из неё вычищается всё, что не несёт
/// смысла для модели: корневые `$schema`, `title` и `description` (имя и
/// описание тула — отдельные поля [`ToolDef`]) и `"default": null` у
/// опциональных полей, который иначе провоцирует модель слать явные `null`.
/// Бюджет «системный промпт + тулы ≤ 1000 токенов» тратится на описания полей.
pub fn input_schema_for<T: JsonSchema>() -> Value {
    let mut schema = schemars::schema_for!(T).to_value();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
        object.remove("description");
    }
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            if let Some(property) = property.as_object_mut()
                && property.get("default") == Some(&Value::Null)
            {
                property.remove("default");
            }
        }
    }
    schema
}

/// Сколько ошибок валидации максимум показываем модели.
const MAX_VALIDATION_ERRORS: usize = 8;

struct Registered {
    tool: Box<dyn Tool>,
    schema: Value,
    /// `None` — схема не скомпилировалась (баг в схеме тула); тогда валидация
    /// пропускается, а не роняет ход.
    validator: Option<Validator>,
}

/// Реестр тулов: владеет инструментами, кэширует их схемы и скомпилированные
/// валидаторы, отдаёт [`ToolDef`] провайдеру в детерминированном порядке.
pub struct ToolRegistry {
    tools: Vec<Registered>,
    /// Приводить ли типы аргументов к схеме до валидации (`[agent]
    /// arg_coercion`, по умолчанию да).
    arg_coercion: bool,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            arg_coercion: true,
        }
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names())
            .field("arg_coercion", &self.arg_coercion)
            .finish()
    }
}

impl ToolRegistry {
    /// Пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Реестр из пяти базовых тулов в фиксированном порядке:
    /// `bash`, `edit`, `grep`, `read`, `write`.
    pub fn with_defaults(edit_strategy: EditStrategy) -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(BashTool::new()));
        registry.register(Box::new(EditTool::new(edit_strategy)));
        registry.register(Box::new(GrepTool::new()));
        registry.register(Box::new(ReadTool::new()));
        registry.register(Box::new(WriteTool::new()));
        registry
    }

    /// Зарегистрировать тул. Схема компилируется здесь один раз — на горячем
    /// пути валидации остаётся только проверка.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let schema = tool.input_schema();
        let validator = match jsonschema::validator_for(&schema) {
            Ok(validator) => Some(validator),
            Err(error) => {
                tracing::warn!(
                    tool = tool.name(),
                    %error,
                    "не удалось скомпилировать схему тула, аргументы не валидируются"
                );
                None
            }
        };
        self.tools.push(Registered {
            tool,
            schema,
            validator,
        });
    }

    /// Builder-форма [`Self::register`].
    #[must_use]
    pub fn with(mut self, tool: Box<dyn Tool>) -> Self {
        self.register(tool);
        self
    }

    /// Включить или выключить приведение типов аргументов к схеме
    /// ([`Self::coerce_input`]); по умолчанию включено. Выключенное — для
    /// абляции: аргументы валидируются ровно такими, какими их прислала модель.
    pub fn set_arg_coercion(&mut self, enabled: bool) {
        self.arg_coercion = enabled;
    }

    /// Builder-форма [`Self::set_arg_coercion`].
    #[must_use]
    pub fn with_arg_coercion(mut self, enabled: bool) -> Self {
        self.set_arg_coercion(enabled);
        self
    }

    /// Включено ли приведение типов аргументов.
    pub fn arg_coercion(&self) -> bool {
        self.arg_coercion
    }

    /// Имена тулов в порядке регистрации.
    pub fn names(&self) -> Vec<&str> {
        self.tools.iter().map(|entry| entry.tool.name()).collect()
    }

    /// Сколько тулов зарегистрировано.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Пуст ли реестр.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Тул по имени.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.entry(name).map(|entry| entry.tool.as_ref())
    }

    fn entry(&self, name: &str) -> Option<&Registered> {
        self.tools.iter().find(|entry| entry.tool.name() == name)
    }

    /// Определения тулов для провайдера — в порядке регистрации.
    ///
    /// Порядок обязан быть стабильным между запросами: тулы стоят в начале
    /// промпта, их перестановка убивает prompt cache.
    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|entry| ToolDef {
                name: entry.tool.name().to_string(),
                description: entry.tool.description().to_string(),
                input_schema: entry.schema.clone(),
            })
            .collect()
    }

    /// Привести аргументы тула `name` к типам его схемы: числовые и булевы
    /// строки (`"60"`, `"60.0"`, `"1.5"`, `"true"`) и дробные числа с нулевой
    /// дробью (`60.0` для целого) — там, где схема ждёт число или булево и
    /// приведение без потерь. Только свойства верхнего уровня; мусор, строковые
    /// поля, `null` и вложенные значения не трогаются. Возвращает имена
    /// приведённых свойств в порядке аргументов (пусто — ничего не менялось).
    ///
    /// [`Self::execute`] приводит аргументы сам. Агент зовёт этот метод раньше —
    /// до истории, событий и сессии, чтобы везде лежали ровно те аргументы,
    /// которые получит тул. Выключенное приведение ([`Self::set_arg_coercion`]),
    /// неизвестный тул и тул с несобравшейся схемой — ничего не трогается:
    /// схеме, которой не верит валидация, не верит и приведение.
    pub fn coerce_input(&self, name: &str, input: &mut Value) -> Vec<String> {
        if !self.arg_coercion {
            return Vec::new();
        }
        self.entry(name)
            .filter(|entry| entry.validator.is_some())
            .map(|entry| coerce::coerce_arguments(&entry.schema, input))
            .unwrap_or_default()
    }

    /// Выполнить тул по имени: неизвестное имя и невалидные аргументы
    /// возвращаются модели как ошибка, а не как паника. Перед валидацией
    /// аргументы приводятся к схеме ([`Self::coerce_input`], если приведение
    /// не выключено) — тул получает уже приведённые.
    pub async fn execute(&self, name: &str, mut input: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(entry) = self.entry(name) else {
            return ToolOutput::err(format!(
                "Unknown tool `{name}`. Available tools: {}.",
                self.names().join(", ")
            ));
        };

        if let Some(validator) = &entry.validator {
            if self.arg_coercion {
                coerce::coerce_arguments(&entry.schema, &mut input);
            }
            if let Some(message) = validation_message(validator, &entry.schema, name, &input) {
                return ToolOutput::err(message);
            }
        }

        entry.tool.execute(input, ctx).await
    }
}

/// Человекочитаемое объяснение, почему аргументы не подошли под схему.
///
/// Текст читает модель, поэтому в нём есть и «что не так» (список ошибок с
/// путями), и «как надо» (сама схема).
fn validation_message(
    validator: &Validator,
    schema: &Value,
    name: &str,
    input: &Value,
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for error in validator.iter_errors(input) {
        let path = error.instance_path().to_string();
        let location = if path.is_empty() {
            "the arguments object".to_string()
        } else {
            format!("`{path}`")
        };
        let line = format!("  - {location}: {error}");
        if !lines.contains(&line) {
            lines.push(line);
        }
        if lines.len() == MAX_VALIDATION_ERRORS {
            break;
        }
    }

    if lines.is_empty() {
        return None;
    }

    Some(format!(
        "Invalid arguments for tool `{name}`:\n{problems}\nExpected arguments matching this JSON Schema:\n{schema}\nFix the arguments and call `{name}` again.",
        problems = lines.join("\n"),
        schema = serde_json::to_string(schema).unwrap_or_else(|_| "{}".to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registry() -> ToolRegistry {
        ToolRegistry::with_defaults(EditStrategy::SearchReplace)
    }

    fn ctx() -> ToolCtx {
        ToolCtx::new(std::env::temp_dir())
    }

    #[test]
    fn tool_output_helpers() {
        assert_eq!(
            ToolOutput::ok("done"),
            ToolOutput {
                content: "done".into(),
                is_error: false
            }
        );
        assert!(ToolOutput::err("boom").is_error);
    }

    #[test]
    fn ctx_resolves_relative_paths_against_cwd() {
        let ctx = ToolCtx::new("/work");
        assert_eq!(ctx.resolve("src/lib.rs"), PathBuf::from("/work/src/lib.rs"));
        assert_eq!(ctx.resolve("/etc/hosts"), PathBuf::from("/etc/hosts"));
        assert!(!ctx.is_cancelled());
        ctx.cancel.cancel();
        assert!(ctx.is_cancelled());
    }

    #[test]
    fn input_schema_helper_strips_root_noise() {
        /// Doc-комментарий корня не должен попасть в схему.
        #[derive(JsonSchema)]
        struct Sample {
            /// Field description stays.
            _field: String,
        }

        let schema = input_schema_for::<Sample>();
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("title").is_none());
        assert!(schema.get("description").is_none());
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(
            schema["properties"]["_field"]["description"],
            json!("Field description stays.")
        );
    }

    /// `"default": null` у опциональных полей только провоцирует модель слать
    /// явные `null` — в схему он не попадает.
    #[test]
    fn optional_fields_carry_no_null_default() {
        for def in registry().defs() {
            let schema = serde_json::to_string(&def.input_schema).unwrap();
            assert!(
                !schema.contains(r#""default":null"#),
                "{} leaks a null default: {schema}",
                def.name
            );
        }
        // Опциональные поля при этом остаются опциональными.
        let bash = registry().get("bash").unwrap().input_schema();
        assert!(bash["properties"]["timeout_seconds"].is_object());
        assert_eq!(bash["required"], json!(["command"]));
    }

    #[test]
    fn defs_are_in_registration_order() {
        let names: Vec<String> = registry().defs().into_iter().map(|def| def.name).collect();
        assert_eq!(names, ["bash", "edit", "grep", "read", "write"]);
    }

    /// Порядок и содержимое `defs()` обязаны быть байт-в-байт стабильными —
    /// иначе рвётся префикс промпта и KV-кэш.
    #[test]
    fn defs_serialization_is_byte_stable() {
        let first = serde_json::to_string(&registry().defs()).unwrap();
        for _ in 0..8 {
            assert_eq!(serde_json::to_string(&registry().defs()).unwrap(), first);
        }
    }

    #[test]
    fn every_def_has_a_valid_object_schema_and_description() {
        for def in registry().defs() {
            assert!(
                !def.description.is_empty(),
                "{} has no description",
                def.name
            );
            assert_eq!(def.input_schema["type"], json!("object"), "{}", def.name);
            assert!(
                def.input_schema["properties"].is_object(),
                "{} has no properties",
                def.name
            );
            jsonschema::validator_for(&def.input_schema)
                .unwrap_or_else(|e| panic!("schema of {} does not compile: {e}", def.name));
        }
    }

    #[test]
    fn registry_lookup_helpers() {
        let registry = registry();
        assert_eq!(registry.len(), 5);
        assert!(!registry.is_empty());
        assert_eq!(registry.get("bash").map(Tool::name), Some("bash"));
        assert!(registry.get("nope").is_none());
        assert!(ToolRegistry::new().is_empty());
        assert!(format!("{registry:?}").contains("bash"));
    }

    #[tokio::test]
    async fn unknown_tool_lists_available_tools() {
        let out = registry().execute("browse", json!({}), &ctx()).await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Unknown tool `browse`"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("bash, edit, grep, read, write"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn wrong_argument_type_is_rejected_with_schema_hint() {
        let out = registry()
            .execute("bash", json!({"command": 42}), &ctx())
            .await;
        assert!(out.is_error);
        assert!(
            out.content
                .starts_with("Invalid arguments for tool `bash`:"),
            "{}",
            out.content
        );
        assert!(out.content.contains("`/command`"), "{}", out.content);
        assert!(
            out.content
                .contains("Expected arguments matching this JSON Schema:"),
            "{}",
            out.content
        );
        assert!(
            out.content
                .contains("Fix the arguments and call `bash` again."),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn missing_required_field_is_rejected() {
        let out = registry().execute("read", json!({}), &ctx()).await;
        assert!(out.is_error);
        assert!(out.content.contains("path"), "{}", out.content);
    }

    #[tokio::test]
    async fn non_object_input_is_rejected() {
        let out = registry().execute("bash", json!("ls -la"), &ctx()).await;
        assert!(out.is_error);
        assert!(
            out.content.contains("the arguments object"),
            "{}",
            out.content
        );
    }

    /// Лишние поля не отвергаются: слабые локалки регулярно досыпают мусор, и
    /// ронять из-за него ход дороже, чем проигнорировать.
    #[tokio::test]
    async fn unknown_properties_are_tolerated() {
        let out = registry()
            .execute(
                "bash",
                json!({"command": "echo hi", "shell": "zsh"}),
                &ctx(),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "hi");
    }

    #[tokio::test]
    async fn valid_input_reaches_the_tool() {
        let out = registry()
            .execute("bash", json!({"command": "echo hi"}), &ctx())
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "hi");
    }

    /// Тул-зонд: десериализует аргументы в типизированную структуру (как
    /// настоящие тулы) и возвращает их ровно в том виде, в каком получил.
    struct Probe;

    #[derive(serde::Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct ProbeInput {
        count: Option<u64>,
        ratio: Option<f64>,
        flag: Option<bool>,
        label: Option<String>,
        sizes: Option<Vec<u64>>,
    }

    #[async_trait]
    impl Tool for Probe {
        fn name(&self) -> &str {
            "probe"
        }
        fn description(&self) -> &str {
            "echoes its typed arguments"
        }
        fn input_schema(&self) -> Value {
            input_schema_for::<ProbeInput>()
        }
        async fn execute(&self, input: Value, _ctx: &ToolCtx) -> ToolOutput {
            match serde_json::from_value::<ProbeInput>(input.clone()) {
                Ok(_) => ToolOutput::ok(input.to_string()),
                Err(error) => ToolOutput::err(format!("probe: {error}")),
            }
        }
    }

    /// Текст ошибки валидации для аргументов как есть — без приведения.
    fn raw_validation_message(registry: &ToolRegistry, name: &str, input: &Value) -> String {
        let entry = registry.entry(name).unwrap();
        validation_message(
            entry.validator.as_ref().unwrap(),
            &entry.schema,
            name,
            input,
        )
        .expect("аргументы должны быть невалидны и без приведения")
    }

    /// Числа и булевы строками доходят до тула уже приведёнными — и проходят
    /// типизированную десериализацию тула.
    #[tokio::test]
    async fn numeric_and_boolean_strings_reach_the_tool_coerced() {
        let registry = ToolRegistry::new().with(Box::new(Probe));
        let out = registry
            .execute(
                "probe",
                json!({"count": "60.0", "ratio": "1.5", "flag": "true", "label": "60"}),
                &ctx(),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            r#"{"count":60,"ratio":1.5,"flag":true,"label":"60"}"#
        );

        let out = registry
            .execute("probe", json!({"count": 60.0}), &ctx())
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, r#"{"count":60}"#);
    }

    /// Ровно сценарий живых прогонов: `timeout_seconds` строкой и `60.0`
    /// числом. Раньше первое отбивала схема, второе — десериализация `u64`.
    #[tokio::test]
    async fn bash_accepts_timeout_seconds_as_a_numeric_string_or_a_whole_float() {
        for timeout in [json!("60"), json!("60.0"), json!(60.0)] {
            let out = registry()
                .execute(
                    "bash",
                    json!({"command": "echo hi", "timeout_seconds": timeout}),
                    &ctx(),
                )
                .await;
            assert!(!out.is_error, "{timeout}: {}", out.content);
            assert_eq!(out.content, "hi", "{timeout}");
        }
    }

    /// `case_insensitive` строкой (`"true"`) доходит до `grep` уже булевым.
    #[tokio::test]
    async fn grep_accepts_case_insensitive_as_a_boolean_string() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "Needle\n").unwrap();
        let out = registry()
            .execute(
                "grep",
                json!({"pattern": "needle", "case_insensitive": "true"}),
                &ToolCtx::new(dir.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "a.txt:1: Needle");
    }

    /// Неприводимое остаётся невалидным — с тем же текстом ошибки, что и без
    /// приведения.
    #[tokio::test]
    async fn uncoercible_arguments_fail_with_the_unchanged_message() {
        let registry = registry();
        let cases = [
            (
                "bash",
                json!({"command": "echo hi", "timeout_seconds": "60s"}),
            ),
            (
                "bash",
                json!({"command": "echo hi", "timeout_seconds": "abc"}),
            ),
            ("bash", json!({"command": "echo hi", "timeout_seconds": ""})),
            (
                "bash",
                json!({"command": "echo hi", "timeout_seconds": "60.5"}),
            ),
            (
                "bash",
                json!({"command": "echo hi", "timeout_seconds": 60.5}),
            ),
            ("bash", json!({"command": 42})),
            ("read", json!({"path": "a.txt", "offset": [1]})),
            ("read", json!({"path": "a.txt", "limit": {"value": "10"}})),
            (
                "edit",
                json!({"path": "a.txt", "old_string": "a", "new_string": "b", "replace_all": "yes"}),
            ),
            ("write", json!({"path": "a.txt", "content": 60})),
            ("grep", json!({"pattern": "x", "case_insensitive": "yes"})),
        ];
        for (name, input) in cases {
            let expected = raw_validation_message(&registry, name, &input);
            let out = registry.execute(name, input.clone(), &ctx()).await;
            assert!(out.is_error, "{name} {input}");
            assert_eq!(out.content, expected, "{name} {input}");
        }
    }

    #[test]
    fn coerce_input_rewrites_in_place_and_reports_changes() {
        let registry = registry();
        let mut input = json!({"command": "ls", "timeout_seconds": "30"});
        assert_eq!(
            registry.coerce_input("bash", &mut input),
            ["timeout_seconds"]
        );
        assert_eq!(input, json!({"command": "ls", "timeout_seconds": 30}));
        // Повтор — ничего нового.
        assert!(registry.coerce_input("bash", &mut input).is_empty());

        let mut input = json!({"command": "ls", "timeout_seconds": null});
        assert!(registry.coerce_input("bash", &mut input).is_empty());
        assert_eq!(input, json!({"command": "ls", "timeout_seconds": null}));

        // Неизвестный тул — не трогаем.
        let mut input = json!({"timeout_seconds": "30"});
        assert!(registry.coerce_input("browse", &mut input).is_empty());
        assert_eq!(input, json!({"timeout_seconds": "30"}));
    }

    /// Абляция: с выключенным приведением `"60.0"` снова отбивается схемой — с
    /// тем же текстом, что и до появления приведения, — а `coerce_input` ничего
    /// не трогает.
    #[tokio::test]
    async fn disabled_coercion_validates_the_arguments_as_sent() {
        let registry = registry().with_arg_coercion(false);
        assert!(!registry.arg_coercion());
        assert!(ToolRegistry::new().arg_coercion(), "по умолчанию включено");

        let input = json!({"command": "echo hi", "timeout_seconds": "60.0"});
        let mut probe = input.clone();
        assert!(registry.coerce_input("bash", &mut probe).is_empty());
        assert_eq!(probe, input);

        let expected = raw_validation_message(&registry, "bash", &input);
        assert!(
            expected.contains(r#""60.0" is not of types "null", "integer""#),
            "{expected}"
        );
        let out = registry.execute("bash", input, &ctx()).await;
        assert!(out.is_error);
        assert_eq!(out.content, expected);

        // Включили обратно — тот же вызов проходит.
        let mut registry = registry;
        registry.set_arg_coercion(true);
        let out = registry
            .execute(
                "bash",
                json!({"command": "echo hi", "timeout_seconds": "60.0"}),
                &ctx(),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
    }

    #[tokio::test]
    async fn registry_without_validator_still_executes() {
        struct Loose;

        #[async_trait]
        impl Tool for Loose {
            fn name(&self) -> &str {
                "loose"
            }
            fn description(&self) -> &str {
                "no schema at all"
            }
            fn input_schema(&self) -> Value {
                // Невалидная схема: валидатор не скомпилируется.
                json!({"type": "definitely-not-a-type"})
            }
            async fn execute(&self, input: Value, _ctx: &ToolCtx) -> ToolOutput {
                ToolOutput::ok(input.to_string())
            }
        }

        let registry = ToolRegistry::new().with(Box::new(Loose));
        let out = registry.execute("loose", json!({"a": 1}), &ctx()).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, r#"{"a":1}"#);

        // Схеме, которая не собралась, не верит и приведение.
        let mut input = json!({"a": "1"});
        assert!(registry.coerce_input("loose", &mut input).is_empty());
        assert_eq!(input, json!({"a": "1"}));
    }
}
