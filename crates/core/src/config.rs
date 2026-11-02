//! Конфигурация harness: слои TOML плюс оверрайды флагов.
//!
//! Слои складываются **по возрастанию приоритета**:
//!
//! ```text
//! ~/.config/mark2/config.toml   пользовательские дефолты
//!            ▼
//! ./.mark2.toml                 настройки проекта (ищется от cwd вверх, ближайший выигрывает)
//!            ▼
//! флаги CLI                     разовые оверрайды
//! ```
//!
//! Каждый слой — тот же [`Config`], у которого **все поля опциональны**:
//! `None` означает «слой ничего не сказал», а не «выключено». Слияние
//! ([`Config::merge`]) поэтому тривиально и ассоциативно: скалярные поля
//! перекрываются старшим слоем, таблица `[models]` объединяется поключево.
//! Флаги CLI не отличаются от файла — они собираются в такой же `Config` и
//! мёржатся последними.
//!
//! Два свойства формата зафиксированы намеренно:
//!
//! * **незнакомые ключи не валят загрузку** — конфиг переживает и опечатку, и
//!   новую версию harness, которая написала поле из будущего;
//! * **битый TOML — понятная ошибка с путём файла**: пользователь должен
//!   узнать, какой из трёх файлов чинить.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, de};

use crate::permission::PermissionPolicy;
use crate::tools::EditStrategy;

/// Базовый URL по умолчанию — llama-server на локальной машине.
pub const DEFAULT_BASE_URL: &str = "http://localhost:8080/v1";

/// Путь пользовательского конфига относительно `$HOME`.
pub const USER_CONFIG_RELATIVE: &str = ".config/mark2/config.toml";

/// Имя проектного конфига; ищется от cwd вверх.
pub const PROJECT_CONFIG_FILE: &str = ".mark2.toml";

/// Ошибка загрузки конфига. В сообщении — путь файла, причина — в `source`
/// (её печатает и `anyhow` в альтернативном формате, и [`ConfigError::chain`]).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Файл существует, но не читается.
    #[error("не удалось прочитать конфиг {}", path.display())]
    Read {
        /// Путь файла.
        path: PathBuf,
        /// Исходная ошибка ввода-вывода.
        source: io::Error,
    },
    /// Файл прочитан, но это не валидный TOML (или значение не того типа).
    #[error("не удалось разобрать конфиг {}", path.display())]
    Parse {
        /// Путь файла.
        path: PathBuf,
        /// Ошибка разбора TOML.
        source: toml::de::Error,
    },
}

impl ConfigError {
    /// Сообщение вместе с причиной — «что и почему» одной строкой.
    pub fn chain(&self) -> String {
        let mut text = self.to_string();
        let mut source: Option<&(dyn std::error::Error + 'static)> =
            std::error::Error::source(self);
        while let Some(error) = source {
            text.push_str(&format!(": {error}"));
            source = error.source();
        }
        text
    }

    /// Путь файла, из-за которого ошибка.
    pub fn path(&self) -> &Path {
        match self {
            Self::Read { path, .. } | Self::Parse { path, .. } => path,
        }
    }
}

/// Результат операций модуля.
pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

/// Один слой настроек.
///
/// Пустой конфиг (`Config::default()`) — нейтральный элемент слияния: он
/// ничего не переопределяет.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Секция `[provider]`.
    pub provider: ProviderConfig,
    /// Секция `[agent]`.
    pub agent: AgentSettings,
    /// Таблица `[models."<id>"]` — настройки под конкретную модель.
    pub models: BTreeMap<String, ModelConfig>,
}

/// Секция `[provider]`: куда и как ходить за моделью.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// База OpenAI-совместимого API (`http://localhost:8080/v1`).
    pub base_url: Option<String>,
    /// Имя переменной окружения с API-ключом. Сам ключ в конфиге не хранится:
    /// конфиг лежит в репозитории проекта, ключ — нет.
    pub api_key_env: Option<String>,
    /// llama-server: просить сервер переиспользовать KV-кэш промпта.
    pub cache_prompt: Option<bool>,
    /// llama-server: пин на конкретный слот, чтобы кэш сессии не вытеснялся.
    pub slot_id: Option<i64>,
}

/// Секция `[agent]`: поведение цикла.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    /// Идентификатор модели в терминах сервера.
    pub model: Option<String>,
    /// Предохранитель: максимум итераций сэмплинга на ход.
    pub max_turns: Option<usize>,
    /// Политика доступа к тулам (`yolo` | `ask_writes` | `read_only`).
    #[serde(default, deserialize_with = "de_policy")]
    pub policy: Option<PermissionPolicy>,
    /// Верхняя граница генерации.
    pub max_tokens: Option<u32>,
    /// Температура сэмплинга.
    pub temperature: Option<f32>,
    /// Детектор зацикливания: с какого подряд идентичного тул-колла его
    /// блокировать (`0` — выключить).
    pub loop_detection: Option<usize>,
}

/// Настройки одной модели: `[models."qwen3-coder"]`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Формат правки файлов (`search_replace` | `whole_file`): слабые локалки
    /// не воспроизводят фрагмент точно, им нужен файл целиком.
    #[serde(default, deserialize_with = "de_edit_strategy")]
    pub edit_strategy: Option<EditStrategy>,
    /// Размер контекстного окна в токенах — от него считается порог компакции.
    pub context_limit: Option<u64>,
}

impl ModelConfig {
    /// Слить с более приоритетным слоем: непустые поля `other` побеждают.
    #[must_use]
    pub fn merge(self, other: ModelConfig) -> ModelConfig {
        ModelConfig {
            edit_strategy: other.edit_strategy.or(self.edit_strategy),
            context_limit: other.context_limit.or(self.context_limit),
        }
    }
}

/// Настройки модели после разрешения слоёв и дефолтов.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelSettings {
    /// Формат правки файлов; по умолчанию — точное search/replace.
    pub edit_strategy: EditStrategy,
    /// Контекстное окно, если известно.
    pub context_limit: Option<u64>,
}

impl Config {
    /// Слить с более приоритетным слоем.
    ///
    /// Скалярные поля: непустое значение `other` перекрывает `self`.
    /// Таблица `models` объединяется **поключево** — модель, описанная только
    /// в пользовательском конфиге, не исчезает из-за того, что проектный
    /// конфиг описал другую.
    #[must_use]
    pub fn merge(self, other: Config) -> Config {
        let Config {
            provider,
            agent,
            mut models,
        } = self;

        for (id, model) in other.models {
            let merged = match models.remove(&id) {
                Some(existing) => existing.merge(model),
                None => model,
            };
            models.insert(id, merged);
        }

        Config {
            provider: ProviderConfig {
                base_url: other.provider.base_url.or(provider.base_url),
                api_key_env: other.provider.api_key_env.or(provider.api_key_env),
                cache_prompt: other.provider.cache_prompt.or(provider.cache_prompt),
                slot_id: other.provider.slot_id.or(provider.slot_id),
            },
            agent: AgentSettings {
                model: other.agent.model.or(agent.model),
                max_turns: other.agent.max_turns.or(agent.max_turns),
                policy: other.agent.policy.or(agent.policy),
                max_tokens: other.agent.max_tokens.or(agent.max_tokens),
                temperature: other.agent.temperature.or(agent.temperature),
                loop_detection: other.agent.loop_detection.or(agent.loop_detection),
            },
            models,
        }
    }

    /// Разобрать конфиг из строки. `path` нужен только для текста ошибки.
    pub fn parse(text: &str, path: impl Into<PathBuf>) -> ConfigResult<Config> {
        toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.into(),
            source,
        })
    }

    /// Прочитать конфиг из файла. Отсутствующий файл — не ошибка (`Ok(None)`):
    /// ни один из слоёв не обязан существовать.
    pub fn load_file(path: impl AsRef<Path>) -> ConfigResult<Option<Config>> {
        let path = path.as_ref();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        Config::parse(&text, path).map(Some)
    }

    /// Собрать файловые слои для рабочей директории: пользовательский конфиг,
    /// затем ближайший `.mark2.toml`. Оверрайды CLI мёржатся вызывающим — они
    /// такой же [`Config`].
    pub fn load(cwd: impl AsRef<Path>) -> ConfigResult<Config> {
        let mut config = Config::default();
        if let Some(path) = user_config_path()
            && let Some(layer) = Config::load_file(&path)?
        {
            tracing::debug!(path = %path.display(), "загружен пользовательский конфиг");
            config = config.merge(layer);
        }
        if let Some(path) = find_project_config(cwd)
            && let Some(layer) = Config::load_file(&path)?
        {
            tracing::debug!(path = %path.display(), "загружен проектный конфиг");
            config = config.merge(layer);
        }
        Ok(config)
    }

    /// База API с учётом дефолта.
    pub fn base_url(&self) -> &str {
        self.provider
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
    }

    /// Настройки конкретной модели, если она описана в `[models]`.
    ///
    /// Сперва точное совпадение ключа, затем — самый длинный ключ, который
    /// входит в идентификатор модели без учёта регистра. Так `[models.qwen3]`
    /// накрывает и `Qwen3-Coder-30B-A3B-Q4_K_M.gguf`, и
    /// `openrouter/qwen3-coder`, а точный ключ всегда важнее приблизительного.
    pub fn model_config(&self, model_id: &str) -> Option<&ModelConfig> {
        if let Some(exact) = self.models.get(model_id) {
            return Some(exact);
        }
        let needle = model_id.to_ascii_lowercase();
        self.models
            .iter()
            .filter(|(key, _)| {
                let key = key.to_ascii_lowercase();
                !key.is_empty() && needle.contains(&key)
            })
            .max_by_key(|(key, _)| key.len())
            .map(|(_, value)| value)
    }

    /// Настройки модели с подставленными дефолтами.
    pub fn resolve(&self, model_id: &str) -> ModelSettings {
        let config = self.model_config(model_id);
        ModelSettings {
            edit_strategy: config
                .and_then(|config| config.edit_strategy)
                .unwrap_or_default(),
            context_limit: config.and_then(|config| config.context_limit),
        }
    }
}

/// Путь пользовательского конфига: `~/.config/mark2/config.toml`.
///
/// Именно `~/.config` и на macOS тоже: конфиг правят руками, а
/// `~/Library/Application Support` для этого неудобен.
pub fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(USER_CONFIG_RELATIVE))
}

/// Найти ближайший `.mark2.toml`, поднимаясь от `from` к корню.
pub fn find_project_config(from: impl AsRef<Path>) -> Option<PathBuf> {
    from.as_ref()
        .ancestors()
        .map(|dir| dir.join(PROJECT_CONFIG_FILE))
        .find(|candidate| candidate.is_file())
}

/// Политика доступа из строки: терпит и `ask_writes`, и `ask-writes`.
fn de_policy<'de, D: Deserializer<'de>>(de: D) -> Result<Option<PermissionPolicy>, D::Error> {
    let Some(raw) = Option::<String>::deserialize(de)? else {
        return Ok(None);
    };
    match normalize(&raw).as_str() {
        "yolo" => Ok(Some(PermissionPolicy::Yolo)),
        "ask_writes" => Ok(Some(PermissionPolicy::AskWrites)),
        "read_only" => Ok(Some(PermissionPolicy::ReadOnly)),
        _ => Err(de::Error::custom(format!(
            "неизвестная policy `{raw}`, ожидается yolo, ask_writes или read_only"
        ))),
    }
}

/// Формат правки из строки: терпит и `whole_file`, и `whole-file`.
fn de_edit_strategy<'de, D: Deserializer<'de>>(de: D) -> Result<Option<EditStrategy>, D::Error> {
    let Some(raw) = Option::<String>::deserialize(de)? else {
        return Ok(None);
    };
    match normalize(&raw).as_str() {
        "search_replace" => Ok(Some(EditStrategy::SearchReplace)),
        "whole_file" => Ok(Some(EditStrategy::WholeFile)),
        _ => Err(de::Error::custom(format!(
            "неизвестная edit_strategy `{raw}`, ожидается search_replace или whole_file"
        ))),
    }
}

/// Привести написание значения к канону: нижний регистр, дефисы как
/// подчёркивания.
fn normalize(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
# Пример конфига пользователя.
[provider]
base_url = "http://localhost:11434/v1"
api_key_env = "OPENROUTER_API_KEY"
cache_prompt = true
slot_id = 3

[agent]
model = "qwen3-coder-30b"
max_turns = 12
policy = "yolo"
max_tokens = 4096
temperature = 0.2
loop_detection = 4

[models."qwen3-coder-30b"]
edit_strategy = "search_replace"
context_limit = 32768

[models."gemma-2b"]
edit_strategy = "whole_file"
context_limit = 8192
"#;

    fn layer(text: &str) -> Config {
        Config::parse(text, "test.toml").expect("слой должен разбираться")
    }

    #[test]
    fn parses_a_realistic_config() {
        let config = layer(EXAMPLE);

        assert_eq!(config.base_url(), "http://localhost:11434/v1");
        assert_eq!(
            config.provider.api_key_env.as_deref(),
            Some("OPENROUTER_API_KEY")
        );
        assert_eq!(config.provider.cache_prompt, Some(true));
        assert_eq!(config.provider.slot_id, Some(3));

        assert_eq!(config.agent.model.as_deref(), Some("qwen3-coder-30b"));
        assert_eq!(config.agent.max_turns, Some(12));
        assert_eq!(config.agent.policy, Some(PermissionPolicy::Yolo));
        assert_eq!(config.agent.max_tokens, Some(4096));
        assert_eq!(config.agent.temperature, Some(0.2));
        assert_eq!(config.agent.loop_detection, Some(4));

        assert_eq!(
            config.resolve("qwen3-coder-30b"),
            ModelSettings {
                edit_strategy: EditStrategy::SearchReplace,
                context_limit: Some(32_768),
            }
        );
        assert_eq!(
            config.resolve("gemma-2b"),
            ModelSettings {
                edit_strategy: EditStrategy::WholeFile,
                context_limit: Some(8_192),
            }
        );
    }

    #[test]
    fn empty_config_is_all_defaults() {
        let config = layer("");
        assert_eq!(config, Config::default());
        assert_eq!(config.base_url(), DEFAULT_BASE_URL);
        assert_eq!(config.agent.model, None);
        assert_eq!(
            config.resolve("whatever"),
            ModelSettings {
                edit_strategy: EditStrategy::SearchReplace,
                context_limit: None,
            }
        );
    }

    #[test]
    fn unknown_keys_do_not_break_loading() {
        let config = layer(
            r#"
            future_section = { anything = 1 }

            [provider]
            base_url = "http://x/v1"
            retries_from_the_future = 7

            [agent]
            model = "m"
            reasoning_effort = "high"

            [models."m"]
            context_limit = 4096
            sampler = "mirostat"
            "#,
        );
        assert_eq!(config.base_url(), "http://x/v1");
        assert_eq!(config.agent.model.as_deref(), Some("m"));
        assert_eq!(config.resolve("m").context_limit, Some(4096));
    }

    #[test]
    fn broken_toml_reports_the_file_path() {
        let error = Config::parse("[provider", "/etc/mark2/config.toml").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("/etc/mark2/config.toml"), "{text}");
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert_eq!(error.path(), Path::new("/etc/mark2/config.toml"));
        // Причина — в source, чтобы вызывающий не печатал её дважды.
        assert!(
            error.chain().contains("unclosed table"),
            "{}",
            error.chain()
        );
    }

    #[test]
    fn bad_enum_values_report_the_file_and_the_alternatives() {
        let error = Config::parse("[agent]\npolicy = \"whatever\"\n", "cfg.toml").unwrap_err();
        let text = error.chain();
        assert!(text.contains("cfg.toml"), "{text}");
        assert!(text.contains("ask_writes"), "{text}");

        let error =
            Config::parse("[models.m]\nedit_strategy = \"diff\"\n", "cfg.toml").unwrap_err();
        assert!(error.chain().contains("whole_file"), "{}", error.chain());
    }

    #[test]
    fn enum_values_tolerate_hyphens_and_case() {
        let config = layer(
            r#"
            [agent]
            policy = "Ask-Writes"

            [models.m]
            edit_strategy = "Whole-File"
            "#,
        );
        assert_eq!(config.agent.policy, Some(PermissionPolicy::AskWrites));
        assert_eq!(config.resolve("m").edit_strategy, EditStrategy::WholeFile);
    }

    /// Три слоя: пользовательский → проектный → флаги CLI. Старший слой
    /// перекрывает то, что назвал, и только это.
    #[test]
    fn three_layers_merge_by_priority() {
        let user = layer(
            r#"
            [provider]
            base_url = "http://user/v1"
            api_key_env = "USER_KEY"
            cache_prompt = true

            [agent]
            model = "user-model"
            max_turns = 5
            policy = "read_only"
            temperature = 0.1
            loop_detection = 5
            "#,
        );
        let project = layer(
            r#"
            [provider]
            base_url = "http://project/v1"

            [agent]
            model = "project-model"
            policy = "ask_writes"
            loop_detection = 0
            "#,
        );
        let flags = layer(
            r#"
            [agent]
            model = "flag-model"
            "#,
        );

        let merged = user.merge(project).merge(flags);

        // Старший слой победил.
        assert_eq!(merged.agent.model.as_deref(), Some("flag-model"));
        // Средний слой победил пользовательский там, где высказался.
        assert_eq!(merged.base_url(), "http://project/v1");
        assert_eq!(merged.agent.policy, Some(PermissionPolicy::AskWrites));
        // Явный 0 («выключить») — тоже высказывание, а не пропуск.
        assert_eq!(merged.agent.loop_detection, Some(0));
        // Про что старшие слои молчали — досталось от пользовательского.
        assert_eq!(merged.provider.api_key_env.as_deref(), Some("USER_KEY"));
        assert_eq!(merged.provider.cache_prompt, Some(true));
        assert_eq!(merged.agent.max_turns, Some(5));
        assert_eq!(merged.agent.temperature, Some(0.1));
    }

    #[test]
    fn model_tables_merge_per_key_and_per_field() {
        let user = layer(
            r#"
            [models."qwen"]
            edit_strategy = "search_replace"
            context_limit = 32768

            [models."gemma"]
            edit_strategy = "whole_file"
            "#,
        );
        let project = layer(
            r#"
            [models."qwen"]
            edit_strategy = "whole_file"

            [models."llama"]
            context_limit = 4096
            "#,
        );

        let merged = user.merge(project);

        // Ключи из обоих слоёв на месте.
        assert_eq!(merged.models.len(), 3);
        // Поле перекрыто, соседнее поле того же ключа — уцелело.
        assert_eq!(
            merged.resolve("qwen"),
            ModelSettings {
                edit_strategy: EditStrategy::WholeFile,
                context_limit: Some(32_768),
            }
        );
        // Модель, о которой сказал только младший слой, не исчезла.
        assert_eq!(
            merged.resolve("gemma").edit_strategy,
            EditStrategy::WholeFile
        );
        assert_eq!(merged.resolve("llama").context_limit, Some(4096));
    }

    #[test]
    fn empty_layer_changes_nothing() {
        let user = layer(EXAMPLE);
        assert_eq!(user.clone().merge(Config::default()), user);
        assert_eq!(Config::default().merge(user.clone()), user);
    }

    #[test]
    fn model_lookup_prefers_exact_then_longest_substring() {
        let config = layer(
            r#"
            [models."qwen"]
            context_limit = 1

            [models."qwen3-coder"]
            context_limit = 2

            [models."Qwen3-Coder-30B-A3B-Q4_K_M.gguf"]
            context_limit = 3
            "#,
        );

        // Точное совпадение важнее приблизительного.
        assert_eq!(
            config
                .resolve("Qwen3-Coder-30B-A3B-Q4_K_M.gguf")
                .context_limit,
            Some(3)
        );
        // Из подходящих подстрок выигрывает самая длинная.
        assert_eq!(config.resolve("qwen3-coder-30b").context_limit, Some(2));
        assert_eq!(config.resolve("qwen2.5").context_limit, Some(1));
        // Ничего не подошло — дефолты.
        assert_eq!(config.resolve("gpt-oss-20b").context_limit, None);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert_eq!(Config::load_file(&missing).unwrap(), None);
    }

    #[test]
    fn load_file_reports_the_path_of_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECT_CONFIG_FILE);
        std::fs::write(&path, "[agent]\nmax_turns = \"нет\"\n").unwrap();

        let error = Config::load_file(&path).unwrap_err();
        let text = error.to_string();
        assert!(text.contains(&path.display().to_string()), "{text}");
    }

    #[test]
    fn project_config_is_found_upwards_nearest_wins() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("crates").join("core").join("src");
        std::fs::create_dir_all(&nested).unwrap();

        // Ничего нет — ничего и не нашли.
        assert_eq!(find_project_config(&nested), None);

        let top = root.path().join(PROJECT_CONFIG_FILE);
        std::fs::write(&top, "[agent]\nmodel = \"top\"\n").unwrap();
        assert_eq!(find_project_config(&nested), Some(top));

        // Ближайший к cwd выигрывает у дальнего.
        let near = root.path().join("crates").join(PROJECT_CONFIG_FILE);
        std::fs::write(&near, "[agent]\nmodel = \"near\"\n").unwrap();
        assert_eq!(find_project_config(&nested), Some(near.clone()));

        let found = Config::load_file(&near).unwrap().unwrap();
        assert_eq!(found.agent.model.as_deref(), Some("near"));
    }

    #[test]
    fn directories_are_not_mistaken_for_config_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(PROJECT_CONFIG_FILE)).unwrap();
        assert_eq!(find_project_config(root.path()), None);
    }

    #[test]
    fn user_config_path_is_dot_config_mark2() {
        let path = user_config_path().expect("в тестовой среде есть $HOME");
        assert!(path.ends_with(USER_CONFIG_RELATIVE), "{}", path.display());
    }
}
