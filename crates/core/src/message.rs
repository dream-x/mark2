//! Нейтральный формат сообщений mark2.
//!
//! Ядро не хранит историю в формате конкретного провайдера: диалекты (OpenAI
//! Chat Completions, Anthropic, …) — это чистые функции `to_wire` /
//! `parse_stream_chunk` на модуле провайдера, а сессия сериализуется в этих
//! нейтральных типах. Смена провайдера не теряет историю.
//!
//! Два свойства формата важны за пределами «просто структур»:
//!
//! * **Детерминированность.** Порядок полей фиксирован объявлением, а
//!   `serde_json` собран с фичей `preserve_order`, поэтому порядок ключей
//!   внутри [`ContentBlock::ToolUse::input`] переживает round-trip. Один и тот
//!   же вход даёт байт-в-байт один и тот же JSON — префикс промпта стабилен,
//!   KV-кэш локального сервера не инвалидируется.
//! * **Флаги видимости.** [`MessageMetadata`] — фундамент компакции: старые
//!   сообщения не удаляются, а помечаются `agent_visible = false`; саммари
//!   помечается `user_visible = false`. Сборка промпта фильтрует по
//!   `agent_visible`, UI — по `user_visible`, сессия хранит всё.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Роль автора сообщения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Системный промпт.
    System,
    /// Ввод пользователя.
    User,
    /// Ответ модели.
    Assistant,
    /// Результаты тул-коллов, возвращаемые модели.
    Tool,
}

impl Role {
    /// Имя роли в том виде, в каком оно уходит в JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// Блок содержимого сообщения.
///
/// Serde-тег `type` в snake_case: `text`, `thinking`, `tool_use`,
/// `tool_result`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Обычный текст.
    Text {
        /// Текст блока.
        text: String,
    },
    /// Рассуждения модели (reasoning / thinking-каналы).
    Thinking {
        /// Текст рассуждения.
        text: String,
    },
    /// Запрос модели на вызов тула.
    ToolUse {
        /// Идентификатор вызова, по нему матчится [`ContentBlock::ToolResult`].
        id: String,
        /// Имя тула.
        name: String,
        /// Аргументы тула как есть (валидируются схемой на стороне реестра).
        input: Value,
    },
    /// Результат выполнения тула, возвращаемый модели.
    ToolResult {
        /// Идентификатор соответствующего [`ContentBlock::ToolUse`].
        tool_use_id: String,
        /// Текстовый результат (в т.ч. текст ошибки при `is_error`).
        content: String,
        /// `true`, если тул завершился ошибкой (retry-with-error).
        #[serde(default)]
        is_error: bool,
    },
}

impl ContentBlock {
    /// Текстовый блок.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Блок рассуждений.
    pub fn thinking(text: impl Into<String>) -> Self {
        Self::Thinking { text: text.into() }
    }

    /// Запрос на вызов тула.
    pub fn tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    /// Успешный результат тула.
    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    /// Ошибка тула: текст ошибки уходит модели как обычный результат.
    pub fn tool_error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}

/// Флаги видимости сообщения — основа компакции без удаления истории.
///
/// Обе по умолчанию `true`; в JSON дефолтные значения не пишутся, поэтому
/// обычные сообщения сериализуются без лишнего шума и старые сессии читаются
/// без миграции.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMetadata {
    /// Попадает ли сообщение в промпт, отправляемый модели.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub agent_visible: bool,
    /// Показывается ли сообщение пользователю (UI).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub user_visible: bool,
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl Default for MessageMetadata {
    fn default() -> Self {
        Self {
            agent_visible: true,
            user_visible: true,
        }
    }
}

impl MessageMetadata {
    /// Видно и агенту, и пользователю (значение по умолчанию).
    pub fn visible() -> Self {
        Self::default()
    }

    /// Сообщение скрыто от модели (вытеснено компакцией), но остаётся в UI.
    pub fn hidden_from_agent() -> Self {
        Self {
            agent_visible: false,
            user_visible: true,
        }
    }

    /// Служебное сообщение (например саммари компакции): модель его видит,
    /// пользователь — нет.
    pub fn hidden_from_user() -> Self {
        Self {
            agent_visible: true,
            user_visible: false,
        }
    }

    /// Дефолтные ли флаги — используется `skip_serializing_if`.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Сообщение диалога в нейтральном формате.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Роль автора.
    pub role: Role,
    /// Блоки содержимого.
    pub content: Vec<ContentBlock>,
    /// Флаги видимости; дефолт не сериализуется.
    #[serde(default, skip_serializing_if = "MessageMetadata::is_default")]
    pub metadata: MessageMetadata,
}

impl Message {
    /// Сообщение из готовых блоков.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            metadata: MessageMetadata::default(),
        }
    }

    /// Системное сообщение из текста.
    pub fn system(text: impl Into<String>) -> Self {
        Self::new(Role::System, vec![ContentBlock::text(text)])
    }

    /// Сообщение пользователя из текста.
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::text(text)])
    }

    /// Ответ модели из текста.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentBlock::text(text)])
    }

    /// Сообщение с результатами тулов.
    pub fn tool(content: Vec<ContentBlock>) -> Self {
        Self::new(Role::Tool, content)
    }

    /// Сообщение с единственным успешным результатом тула.
    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::tool(vec![ContentBlock::tool_result(tool_use_id, content)])
    }

    /// Сообщение с единственной ошибкой тула.
    pub fn tool_error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::tool(vec![ContentBlock::tool_error(tool_use_id, content)])
    }

    /// Заменить флаги видимости целиком.
    #[must_use]
    pub fn with_metadata(mut self, metadata: MessageMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Выставить `agent_visible`.
    #[must_use]
    pub fn with_agent_visible(mut self, visible: bool) -> Self {
        self.metadata.agent_visible = visible;
        self
    }

    /// Выставить `user_visible`.
    #[must_use]
    pub fn with_user_visible(mut self, visible: bool) -> Self {
        self.metadata.user_visible = visible;
        self
    }

    /// Виден ли модели (фильтр сборки промпта).
    pub fn is_agent_visible(&self) -> bool {
        self.metadata.agent_visible
    }

    /// Виден ли пользователю (фильтр UI).
    pub fn is_user_visible(&self) -> bool {
        self.metadata.user_visible
    }

    /// Склеенный текст всех текстовых блоков (thinking не включается).
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if let ContentBlock::Text { text } = block {
                out.push_str(text);
            }
        }
        out
    }

    /// Запросы на вызов тулов в порядке появления: `(id, name, input)`.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }

    /// Есть ли в сообщении хоть один тул-колл.
    pub fn has_tool_uses(&self) -> bool {
        self.tool_uses().next().is_some()
    }
}

/// Расход токенов за запрос к провайдеру.
///
/// Используется и как событие наружу ([`crate::event::Event::Usage`]), и как
/// вход оценки занятости контекста для компакции.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Токены промпта.
    #[serde(default)]
    pub input_tokens: u64,
    /// Сгенерированные токены.
    #[serde(default)]
    pub output_tokens: u64,
    /// Сколько токенов промпта пришло из кэша (llama-server / OpenRouter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Сколько токенов промпта записано в кэш.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

impl Usage {
    /// Usage только с базовыми счётчиками.
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            ..Self::default()
        }
    }

    /// Сколько токенов занято суммарно — оценка позиции относительно
    /// контекстного лимита модели.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_message() -> Message {
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("привет"),
                ContentBlock::thinking("надо позвать тул"),
                ContentBlock::tool_use(
                    "call_1",
                    "read",
                    json!({"path": "/tmp/a.txt", "offset": 0, "limit": 100}),
                ),
                ContentBlock::tool_result("call_1", "содержимое"),
                ContentBlock::tool_error("call_2", "file not found"),
            ],
        )
    }

    #[test]
    fn message_round_trip_with_all_block_kinds() {
        let message = sample_message();
        let json = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(message, back);
    }

    #[test]
    fn content_block_tags_are_snake_case() {
        let blocks = [
            ContentBlock::text("t"),
            ContentBlock::thinking("th"),
            ContentBlock::tool_use("id", "bash", json!({})),
            ContentBlock::tool_result("id", "ok"),
        ];
        let tags: Vec<String> = blocks
            .iter()
            .map(|b| {
                serde_json::to_value(b).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(tags, ["text", "thinking", "tool_use", "tool_result"]);
    }

    #[test]
    fn role_wire_names_are_stable() {
        for (role, expected) in [
            (Role::System, "system"),
            (Role::User, "user"),
            (Role::Assistant, "assistant"),
            (Role::Tool, "tool"),
        ] {
            assert_eq!(serde_json::to_value(role).unwrap(), json!(expected));
            assert_eq!(role.as_str(), expected);
            let back: Role = serde_json::from_value(json!(expected)).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn default_metadata_is_not_serialized() {
        let json = serde_json::to_value(Message::user("привет")).unwrap();
        assert_eq!(
            json,
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "привет"}],
            })
        );
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn only_non_default_visibility_flags_are_serialized() {
        let hidden = Message::user("старое").with_agent_visible(false);
        let json = serde_json::to_value(&hidden).unwrap();
        assert_eq!(json["metadata"], json!({"agent_visible": false}));

        let summary = Message::assistant("саммари").with_user_visible(false);
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["metadata"], json!({"user_visible": false}));
    }

    #[test]
    fn missing_metadata_deserializes_to_visible_defaults() {
        let message: Message = serde_json::from_value(json!({
            "role": "user",
            "content": [{"type": "text", "text": "hi"}],
        }))
        .unwrap();
        assert!(message.is_agent_visible());
        assert!(message.is_user_visible());
        assert_eq!(message.metadata, MessageMetadata::default());
    }

    #[test]
    fn partial_metadata_deserializes_missing_flag_as_true() {
        let message: Message = serde_json::from_value(json!({
            "role": "user",
            "content": [],
            "metadata": {"agent_visible": false},
        }))
        .unwrap();
        assert!(!message.is_agent_visible());
        assert!(message.is_user_visible());
    }

    #[test]
    fn visibility_metadata_round_trips() {
        for metadata in [
            MessageMetadata::visible(),
            MessageMetadata::hidden_from_agent(),
            MessageMetadata::hidden_from_user(),
            MessageMetadata {
                agent_visible: false,
                user_visible: false,
            },
        ] {
            let message = Message::user("x").with_metadata(metadata);
            let json = serde_json::to_string(&message).unwrap();
            let back: Message = serde_json::from_str(&json).unwrap();
            assert_eq!(back.metadata, metadata);
        }
    }

    #[test]
    fn missing_is_error_defaults_to_false() {
        let block: ContentBlock = serde_json::from_value(json!({
            "type": "tool_result",
            "tool_use_id": "call_1",
            "content": "ok",
        }))
        .unwrap();
        assert_eq!(block, ContentBlock::tool_result("call_1", "ok"));
    }

    /// Стабильность сериализации: одинаковый вход → байт-в-байт одинаковый
    /// JSON (иначе рвётся префикс промпта и KV-кэш локального сервера).
    #[test]
    fn serialization_is_byte_stable() {
        let first = serde_json::to_string(&sample_message()).unwrap();
        for _ in 0..16 {
            assert_eq!(serde_json::to_string(&sample_message()).unwrap(), first);
        }
    }

    /// Порядок ключей внутри аргументов тула не должен меняться после
    /// round-trip — за это отвечает фича `preserve_order` у `serde_json`.
    #[test]
    fn tool_input_key_order_survives_round_trip() {
        let raw = r#"{"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"write","input":{"zebra":1,"alpha":2,"middle":{"z":1,"a":2}}}]}"#;
        let message: Message = serde_json::from_str(raw).unwrap();
        assert_eq!(serde_json::to_string(&message).unwrap(), raw);
    }

    #[test]
    fn message_field_order_is_fixed() {
        let json = serde_json::to_string(&Message::user("x").with_agent_visible(false)).unwrap();
        assert_eq!(
            json,
            r#"{"role":"user","content":[{"type":"text","text":"x"}],"metadata":{"agent_visible":false}}"#
        );
    }

    #[test]
    fn constructors_build_expected_shapes() {
        assert_eq!(Message::system("s").role, Role::System);
        assert_eq!(Message::user("u").role, Role::User);
        assert_eq!(Message::assistant("a").role, Role::Assistant);
        assert_eq!(Message::tool_result("id", "out").role, Role::Tool);
        assert_eq!(
            Message::tool_error("id", "boom").content,
            vec![ContentBlock::ToolResult {
                tool_use_id: "id".into(),
                content: "boom".into(),
                is_error: true,
            }]
        );
    }

    #[test]
    fn text_and_tool_use_accessors() {
        let message = sample_message();
        assert_eq!(message.text(), "привет");
        assert!(message.has_tool_uses());
        let uses: Vec<_> = message.tool_uses().collect();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].0, "call_1");
        assert_eq!(uses[0].1, "read");
        assert_eq!(uses[0].2["path"], json!("/tmp/a.txt"));
        assert!(!Message::user("x").has_tool_uses());
    }

    #[test]
    fn usage_round_trip_and_totals() {
        let usage = Usage::new(1200, 300);
        assert_eq!(usage.total_tokens(), 1500);
        assert_eq!(
            serde_json::to_value(usage).unwrap(),
            json!({"input_tokens": 1200, "output_tokens": 300})
        );

        let full = Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: Some(8),
            cache_write_tokens: Some(2),
        };
        let json = serde_json::to_string(&full).unwrap();
        assert_eq!(serde_json::from_str::<Usage>(&json).unwrap(), full);

        let empty: Usage = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, Usage::default());
    }
}
