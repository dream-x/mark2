//! Контракт провайдера моделей: [`Provider`], [`ProviderRequest`],
//! [`MessageStream`], [`StreamChunk`].
//!
//! Trait намеренно минимальный (урок goose): единственный обязательный метод —
//! `stream()`. Всё, что можно сделать чистой функцией на стороне реализации
//! (wire-диалект, сборка дельт, ретраи), в контракт не выносится.
//!
//! Провайдер «глупый»: он не знает про компакцию, видимость сообщений и
//! политики доступа. Ему приносят готовый [`ProviderRequest`] — уже
//! отфильтрованную историю, системный промпт и список тулов, — а он отдаёт
//! поток [`StreamChunk`]. Фильтрация по `agent_visible` — забота агента
//! ([`crate::agent`]), не провайдера.
//!
//! `ToolDef` определён заранее как общий контракт между провайдером (task 3)
//! и реестром тулов (task 4) — не менять сигнатуру без синхронизации обоих.

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::message::{Message, Usage};

/// Определение тула, передаваемое провайдеру: имя, описание и JSON Schema входа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    /// Имя тула — по нему матчится [`crate::message::ContentBlock::ToolUse`].
    pub name: String,
    /// Описание для модели (входит в бюджет системного промпта).
    pub description: String,
    /// JSON Schema аргументов (генерируется `schemars` в реестре тулов).
    pub input_schema: serde_json::Value,
}

/// Ошибка обращения к провайдеру.
///
/// [`ProviderError::ContextLengthExceeded`] выделен отдельным вариантом
/// намеренно: на него завязана компакция (task 7) — агент ловит его, сжимает
/// историю и повторяет запрос один раз. Все прочие 4xx попадают в
/// [`ProviderError::Http`].
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Промпт не влезает в контекст модели — сигнал к компакции и повтору.
    #[error("контекст модели переполнен")]
    ContextLengthExceeded,
    /// Ответ с не-2xx статусом; тело сохранено для диагностики.
    #[error("HTTP {status}: {body}")]
    Http {
        /// HTTP-статус ответа.
        status: u16,
        /// Тело ответа (может быть обрезано реализацией).
        body: String,
    },
    /// Транспорт: не удалось соединиться, таймаут, обрыв потока.
    #[error("сетевая ошибка: {0}")]
    Network(String),
    /// Ответ получен, но не разобран (битый JSON/SSE, неожиданная форма).
    #[error("ошибка разбора ответа: {0}")]
    Parse(String),
    /// Запрос отменён снаружи (`CancellationToken`, `Op::Interrupt`).
    #[error("запрос отменён")]
    Cancelled,
}

/// Запрос к модели в нейтральных типах.
///
/// Порядок полей фиксирован объявлением — реализации сериализуют его
/// детерминированно, чтобы префикс промпта не «плыл» между ходами и KV-кэш
/// локального сервера оставался валидным.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// Идентификатор модели в терминах сервера.
    pub model: String,
    /// Системный промпт целиком (провайдер сам кладёт его в свой диалект).
    pub system: String,
    /// История в нейтральном формате, уже отфильтрованная агентом.
    pub messages: Vec<Message>,
    /// Доступные тулы в фиксированном порядке.
    pub tools: Vec<ToolDef>,
    /// Верхняя граница генерации, если задана.
    pub max_tokens: Option<u32>,
    /// Температура сэмплинга, если задана.
    pub temperature: Option<f32>,
}

impl ProviderRequest {
    /// Запрос с моделью и системным промптом; остальное — дефолты.
    pub fn new(model: impl Into<String>, system: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: system.into(),
            ..Self::default()
        }
    }

    /// Задать историю.
    #[must_use]
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Задать список тулов.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    /// Задать лимит генерации.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Задать температуру.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// Элемент потока ответа модели.
///
/// Дельты идут в порядке генерации; [`StreamChunk::Complete`] приходит ровно
/// один раз и последним — он несёт уже собранное assistant-сообщение (включая
/// [`crate::message::ContentBlock::ToolUse`] с распарсенными аргументами), так
/// что потребителю не нужно самому склеивать дельты, чтобы положить ответ в
/// историю.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamChunk {
    /// Кусок текста ответа.
    TextDelta(String),
    /// Кусок рассуждений (reasoning/thinking-каналы).
    ThinkingDelta(String),
    /// Кусок тул-колла: `id` и `name` обычно приходят в первом фрагменте,
    /// аргументы — кусками строки JSON в последующих.
    ToolCallDelta {
        /// Позиция тул-колла в ответе (нумерация провайдера).
        index: usize,
        /// Идентификатор вызова, если пришёл в этом фрагменте.
        id: Option<String>,
        /// Имя тула, если пришло в этом фрагменте.
        name: Option<String>,
        /// Кусок JSON-аргументов (может быть пустым).
        args_delta: String,
    },
    /// Ответ собран целиком.
    Complete {
        /// Итоговое assistant-сообщение в нейтральном формате.
        message: Message,
        /// Расход токенов за запрос.
        usage: Usage,
    },
}

/// Поток ответа модели.
pub type MessageStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, ProviderError>> + Send>>;

/// Провайдер модели.
///
/// Единственный обязательный метод — [`Provider::stream`]. Реализация обязана
/// быть `Send + Sync`: агент держит её за `Arc` и зовёт из разных задач.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Имя провайдера для логов и событий (`openai_compat`, …).
    fn name(&self) -> &str;

    /// Отправить запрос и получить поток ответа.
    ///
    /// Ретраи (если реализация их делает) допустимы только до начала стрима:
    /// повтор мид-стрима дублировал бы уже отданные дельты.
    async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Role};
    use futures::StreamExt;
    use serde_json::json;

    fn tool_def() -> ToolDef {
        ToolDef {
            name: "read".into(),
            description: "читает файл".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }
    }

    #[test]
    fn provider_request_builders_compose() {
        let req = ProviderRequest::new("qwen3-coder", "ты агент")
            .with_messages(vec![Message::user("привет")])
            .with_tools(vec![tool_def()])
            .with_max_tokens(1024)
            .with_temperature(0.2);

        assert_eq!(req.model, "qwen3-coder");
        assert_eq!(req.system, "ты агент");
        assert_eq!(req.messages, vec![Message::user("привет")]);
        assert_eq!(req.tools, vec![tool_def()]);
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.temperature, Some(0.2));
    }

    #[test]
    fn provider_request_round_trips_and_is_byte_stable() {
        let req = ProviderRequest::new("m", "s")
            .with_messages(vec![Message::user("hi")])
            .with_tools(vec![tool_def()]);
        let json = serde_json::to_string(&req).unwrap();
        let back: ProviderRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
        for _ in 0..8 {
            assert_eq!(serde_json::to_string(&req).unwrap(), json);
        }
    }

    #[test]
    fn tool_def_round_trips_with_key_order_preserved() {
        let raw =
            r#"{"name":"write","description":"пишет файл","input_schema":{"zebra":1,"alpha":2}}"#;
        let def: ToolDef = serde_json::from_str(raw).unwrap();
        assert_eq!(serde_json::to_string(&def).unwrap(), raw);
    }

    #[test]
    fn context_length_exceeded_is_matchable_for_compaction() {
        // Task 7 ловит ровно этот вариант, чтобы сжать историю и повторить.
        let err = ProviderError::ContextLengthExceeded;
        assert!(matches!(err, ProviderError::ContextLengthExceeded));
        assert!(!matches!(
            ProviderError::Http {
                status: 400,
                body: "bad".into()
            },
            ProviderError::ContextLengthExceeded
        ));
    }

    #[test]
    fn provider_errors_display_with_context() {
        assert_eq!(
            ProviderError::Http {
                status: 502,
                body: "upstream down".into(),
            }
            .to_string(),
            "HTTP 502: upstream down"
        );
        assert_eq!(
            ProviderError::Network("connection refused".into()).to_string(),
            "сетевая ошибка: connection refused"
        );
        assert_eq!(
            ProviderError::Parse("bad json".into()).to_string(),
            "ошибка разбора ответа: bad json"
        );
        assert_eq!(ProviderError::Cancelled.to_string(), "запрос отменён");
    }

    struct EchoProvider;

    #[async_trait]
    impl Provider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }

        async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError> {
            let text = req.messages.last().map(Message::text).unwrap_or_default();
            let message = Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::text(text.clone()),
                    ContentBlock::tool_use("call_1", "read", json!({"path": "a"})),
                ],
            );
            let chunks = vec![
                Ok(StreamChunk::TextDelta(text)),
                Ok(StreamChunk::ToolCallDelta {
                    index: 0,
                    id: Some("call_1".into()),
                    name: Some("read".into()),
                    args_delta: r#"{"path":"a"}"#.into(),
                }),
                Ok(StreamChunk::Complete {
                    message,
                    usage: Usage::new(10, 2),
                }),
            ];
            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    /// Trait должен быть object-safe и пригодным для `Arc<dyn Provider>` —
    /// агент держит провайдера именно так.
    #[tokio::test]
    async fn provider_is_object_safe_and_streams() {
        let provider: std::sync::Arc<dyn Provider> = std::sync::Arc::new(EchoProvider);
        assert_eq!(provider.name(), "echo");

        let stream = provider
            .stream(ProviderRequest::new("m", "s").with_messages(vec![Message::user("эхо")]))
            .await
            .unwrap();
        let chunks: Vec<_> = stream.map(Result::unwrap).collect().await;

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], StreamChunk::TextDelta("эхо".into()));
        match &chunks[2] {
            StreamChunk::Complete { message, usage } => {
                assert_eq!(message.role, Role::Assistant);
                assert!(message.has_tool_uses());
                assert_eq!(usage.total_tokens(), 12);
            }
            other => panic!("ожидался Complete, получен {other:?}"),
        }
    }
}
