//! Компакция контекста: суммаризация истории через провайдера, когда промпт
//! подбирается к контекстному лимиту модели.
//!
//! На локальных моделях контекст — самый дефицитный ресурс: 8–32k токенов
//! кончаются за десяток тул-коллов, а сервер на переполнение отвечает не
//! деградацией качества, а отказом ([`ProviderError::ContextLengthExceeded`]).
//! Поэтому компакция здесь не «оптимизация», а часть контракта цикла.
//!
//! # Как оценивается занятость
//!
//! Основной сигнал — [`Usage`] последнего ответа провайдера: сервер сам
//! посчитал токены своим токенизатором, точнее не будет. Пока ни одного ответа
//! не пришло (или сервер не вернул счётчики), работает грубый фолбэк
//! [`estimate_bytes`]: байты сериализованной истории, делённые на
//! [`BYTES_PER_TOKEN`]. Обе оценки — про историю; системный промпт в них не
//! входит, потому что компакция всё равно не может его сжать.
//!
//! # Что делает компакция
//!
//! [`compact`] собирает запрос из **agent-visible** истории, просит модель
//! написать структурное саммари ([`SUMMARY_INSTRUCTIONS`]) под отдельным
//! коротким системным промптом ([`SUMMARY_SYSTEM_PROMPT`]) и затем:
//!
//! * помечает все видимые модели сообщения `agent_visible = false` — **ничего
//!   не удаляя**: пользователь по-прежнему видит свою переписку, а сессия
//!   хранит её целиком;
//! * дописывает саммари сообщением с префиксом [`SUMMARY_PREFIX`] и флагами
//!   `user_visible = false` (модели нужно, пользователю — нет: он и так видел
//!   оригинал).
//!
//! Саммари кладётся сообщением от роли `user`: строгие OpenAI-совместимые
//! сервера и jinja-шаблоны локальных серверов ждут, что разговор заканчивается
//! пользовательским ходом, а после компакции в истории может не остаться
//! ничего другого.
//!
//! Скрывается **вся** видимая история целиком, а не «всё, кроме хвоста»: так у
//! каждого `tool_use` гарантированно остаётся его `tool_result` (оба уходят в
//! скрытые), и следующий запрос заведомо не развалит строгий сервер висящим
//! тул-коллом.

use futures::StreamExt;

use crate::message::{Message, MessageMetadata, Usage};
use crate::provider::{Provider, ProviderError, ProviderRequest, StreamChunk};

/// Доля контекста, после которой запускается компакция, если не задано иное.
pub const DEFAULT_THRESHOLD: f32 = 0.8;

/// Сколько байт нейтрального JSON считать одним токеном в фолбэк-оценке.
///
/// Грубо, но в нужную сторону: на коде и английском тексте реальное отношение
/// ближе к 3.5–4 байт на токен, так что оценка скорее занизит расход, чем
/// заставит сжимать историю раньше времени.
pub const BYTES_PER_TOKEN: u64 = 4;

/// Префикс, с которого начинается саммари в истории.
pub const SUMMARY_PREFIX: &str = "[Summary of earlier conversation]";

/// Системный промпт суммаризатора — отдельный и короткий: агентские правила и
/// описания тулов суммаризации только мешают.
pub const SUMMARY_SYSTEM_PROMPT: &str = "You compress a coding agent's conversation into a handoff \
summary for the same agent. Report only what the transcript actually shows. Plain prose, no \
preamble, no questions, no offers to help.";

/// Инструкция суммаризации, уходящая последним сообщением запроса.
pub const SUMMARY_INSTRUCTIONS: &str = "Summarize the conversation above so that the work can \
continue from this summary alone. Use these sections:

1. Task: what the user asked for, in their own terms.
2. Progress: what has already been done and what the results were.
3. Files: paths created, edited or read, with a note on what changed in each.
4. Decisions: choices made and constraints to respect (APIs, conventions, user preferences).
5. Next: the immediate next step, plus anything still unverified or broken.

Keep every detail needed to continue: exact paths, identifiers, commands, error text. Drop \
small talk and tool output that no longer matters. Be concise and invent nothing.";

/// Настройки компакции.
///
/// `context_limit` — окно модели в токенах. Пока он не задан, порог не
/// срабатывает: гадать про размер контекста незнакомой локальной модели опаснее,
/// чем не сжимать вовсе. Реактивная компакция по
/// [`ProviderError::ContextLengthExceeded`] работает независимо от этого поля.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionConfig {
    /// Доля контекста, после которой история сжимается (`0.8` — по умолчанию).
    pub threshold: f32,
    /// Контекстное окно модели в токенах; `None` — порог выключен.
    pub context_limit: Option<u64>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            context_limit: None,
        }
    }
}

impl CompactionConfig {
    /// Настройки по умолчанию: порог `0.8`, лимит не задан.
    pub fn new() -> Self {
        Self::default()
    }

    /// Задать долю контекста, после которой история сжимается.
    #[must_use]
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    /// Задать контекстное окно модели в токенах.
    #[must_use]
    pub fn with_context_limit(mut self, context_limit: u64) -> Self {
        self.context_limit = Some(context_limit);
        self
    }

    /// Порог в токенах, если контекстное окно известно.
    pub fn threshold_tokens(&self) -> Option<u64> {
        let limit = self.context_limit?;
        let share = f64::from(self.threshold).clamp(0.0, 1.0);
        Some((limit as f64 * share) as u64)
    }

    /// Пора ли сжимать при такой оценке занятости.
    pub fn should_compact(&self, tokens: u64) -> bool {
        self.threshold_tokens()
            .is_some_and(|threshold| tokens > threshold)
    }
}

/// Итог компакции.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionOutcome {
    /// Саммари, дописанное в конец истории.
    pub summary: Message,
    /// Сообщения, только что скрытые от модели (уже с новыми флагами) — их
    /// дописывает в сессию вызывающий.
    pub hidden: Vec<Message>,
    /// Оценка занятости контекста до компакции.
    pub tokens_before: u64,
    /// Оценка занятости контекста после компакции.
    pub tokens_after: u64,
}

/// Ошибки компакции.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    /// Провайдер не смог выполнить запрос суммаризации.
    #[error("суммаризация не удалась: {0}")]
    Provider(#[from] ProviderError),
    /// Поток закрылся, не отдав собранное сообщение.
    #[error("суммаризатор закрыл поток, не отдав готовое сообщение")]
    NoSummary,
    /// Модель вернула пустое саммари — заменять историю нечем.
    #[error("суммаризатор вернул пустое саммари")]
    EmptySummary,
    /// В истории нет ни одного видимого модели сообщения.
    #[error("сжимать нечего: в истории нет видимых модели сообщений")]
    NothingToCompact,
}

/// Оценка занятости контекста историей.
///
/// Основной сигнал — `last_usage` последнего ответа провайдера; если его нет
/// (или сервер не вернул счётчики), считается [`estimate_bytes`].
pub fn estimate_tokens(history: &[Message], last_usage: Option<Usage>) -> u64 {
    match last_usage {
        Some(usage) if usage.total_tokens() > 0 => usage.total_tokens(),
        _ => estimate_bytes(history),
    }
}

/// Фолбэк-оценка: байты сериализованных agent-visible сообщений, делённые на
/// [`BYTES_PER_TOKEN`].
pub fn estimate_bytes(history: &[Message]) -> u64 {
    let bytes: u64 = history
        .iter()
        .filter(|message| message.is_agent_visible())
        .map(|message| serde_json::to_string(message).map_or(0, |json| json.len() as u64))
        .sum();
    bytes.div_ceil(BYTES_PER_TOKEN)
}

/// Саммари как сообщение истории: префикс [`SUMMARY_PREFIX`], роль `user`,
/// флаги «модели видно, пользователю нет».
pub fn summary_message(summary: &str) -> Message {
    Message::user(format!("{SUMMARY_PREFIX}\n{}", summary.trim()))
        .with_metadata(MessageMetadata::hidden_from_user())
}

/// Сжать историю: суммаризировать видимую часть через провайдера, скрыть её от
/// модели и дописать саммари.
///
/// `system` — системный промпт суммаризатора (обычно
/// [`SUMMARY_SYSTEM_PROMPT`]), а не системный промпт агента. `last_usage`
/// нужен только для `tokens_before`.
///
/// История меняется **на месте и только дополняется смыслом**: сообщения не
/// удаляются и не переписываются по содержимому — у них меняется единственный
/// флаг видимости, а саммари добавляется в конец.
pub async fn compact(
    provider: &dyn Provider,
    model: &str,
    system: &str,
    history: &mut Vec<Message>,
    last_usage: Option<Usage>,
) -> Result<CompactionOutcome, CompactionError> {
    let visible: Vec<Message> = history
        .iter()
        .filter(|message| message.is_agent_visible())
        .cloned()
        .collect();
    if visible.is_empty() {
        return Err(CompactionError::NothingToCompact);
    }

    let tokens_before = estimate_tokens(history, last_usage);
    let summary = summary_message(&summarize(provider, model, system, visible).await?);

    let mut hidden = Vec::new();
    for message in history
        .iter_mut()
        .filter(|message| message.is_agent_visible())
    {
        message.metadata.agent_visible = false;
        hidden.push(message.clone());
    }
    history.push(summary.clone());

    // После компакции старый usage уже ничего не описывает, поэтому «после»
    // всегда считается по байтам — по той же истории, что уйдёт в промпт.
    let tokens_after = estimate_bytes(history);

    Ok(CompactionOutcome {
        summary,
        hidden,
        tokens_before,
        tokens_after,
    })
}

/// Один запрос к модели: история плюс инструкция, без тулов.
async fn summarize(
    provider: &dyn Provider,
    model: &str,
    system: &str,
    mut messages: Vec<Message>,
) -> Result<String, CompactionError> {
    messages.push(Message::user(SUMMARY_INSTRUCTIONS));
    let request = ProviderRequest::new(model, system).with_messages(messages);

    let mut stream = provider.stream(request).await?;
    let mut summary: Option<String> = None;
    while let Some(chunk) = stream.next().await {
        if let StreamChunk::Complete { message, .. } = chunk? {
            summary = Some(message.text());
        }
    }

    let summary = summary.ok_or(CompactionError::NoSummary)?;
    if summary.trim().is_empty() {
        return Err(CompactionError::EmptySummary);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Role};
    use crate::provider::MessageStream;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    /// Что стаб отвечает на единственный запрос суммаризации.
    enum Reply {
        /// Дельты плюс `Complete` с этим текстом.
        Text(String),
        /// Поток без `Complete`.
        NoComplete,
        /// Ошибка до начала стрима.
        Error(ProviderError),
    }

    /// Провайдер-заглушка: отдаёт заготовленный ответ и запоминает запрос.
    struct StubProvider {
        reply: Mutex<Option<Reply>>,
        requests: Mutex<Vec<ProviderRequest>>,
    }

    impl StubProvider {
        fn new(reply: Reply) -> Self {
            Self {
                reply: Mutex::new(Some(reply)),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn text(summary: &str) -> Self {
            Self::new(Reply::Text(summary.to_string()))
        }

        fn request(&self) -> ProviderRequest {
            self.requests.lock().unwrap().last().cloned().unwrap()
        }
    }

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }

        async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError> {
            self.requests.lock().unwrap().push(req);
            match self.reply.lock().unwrap().take().expect("стаб уже отвечал") {
                Reply::Text(text) => {
                    let chunks = vec![
                        Ok(StreamChunk::TextDelta(text.clone())),
                        Ok(StreamChunk::Complete {
                            message: Message::assistant(text),
                            usage: Usage::new(120, 40),
                        }),
                    ];
                    Ok(Box::pin(futures::stream::iter(chunks)))
                }
                Reply::NoComplete => Ok(Box::pin(futures::stream::iter(vec![Ok(
                    StreamChunk::TextDelta("обрыв".into()),
                )]))),
                Reply::Error(error) => Err(error),
            }
        }
    }

    fn sample_history() -> Vec<Message> {
        vec![
            Message::user("почини тест в src/lib.rs"),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::text("смотрю"),
                    ContentBlock::tool_use("call_1", "read", json!({"path": "src/lib.rs"})),
                ],
            ),
            Message::tool_result("call_1", "fn main() {}"),
        ]
    }

    #[test]
    fn config_defaults_and_builders() {
        let config = CompactionConfig::default();
        assert_eq!(config.threshold, DEFAULT_THRESHOLD);
        assert_eq!(config.context_limit, None);
        assert_eq!(config.threshold_tokens(), None);
        assert!(
            !config.should_compact(u64::MAX),
            "без context_limit порог не срабатывает"
        );

        let config = CompactionConfig::new()
            .with_threshold(0.5)
            .with_context_limit(1000);
        assert_eq!(config.threshold_tokens(), Some(500));
        assert!(!config.should_compact(500));
        assert!(config.should_compact(501));
    }

    /// Мусорный порог не должен превращаться в NaN-границу.
    #[test]
    fn threshold_is_clamped_to_a_sane_share() {
        let config = CompactionConfig::new()
            .with_threshold(5.0)
            .with_context_limit(1000);
        assert_eq!(config.threshold_tokens(), Some(1000));

        let config = CompactionConfig::new()
            .with_threshold(-1.0)
            .with_context_limit(1000);
        assert_eq!(config.threshold_tokens(), Some(0));
        assert!(config.should_compact(1));
    }

    #[test]
    fn usage_is_the_primary_signal() {
        let history = sample_history();
        assert_eq!(
            estimate_tokens(&history, Some(Usage::new(3000, 500))),
            3500,
            "оценка берётся из последнего usage"
        );
    }

    /// Фолбэк до первого ответа провайдера — байты сериализованной истории.
    #[test]
    fn byte_estimate_is_the_fallback() {
        let history = vec![Message::user("x".repeat(400))];
        let bytes = serde_json::to_string(&history[0]).unwrap().len() as u64;
        let expected = bytes.div_ceil(BYTES_PER_TOKEN);

        assert_eq!(estimate_bytes(&history), expected);
        assert_eq!(estimate_tokens(&history, None), expected);
        assert_eq!(
            estimate_tokens(&history, Some(Usage::default())),
            expected,
            "пустой usage — тоже повод считать по байтам"
        );
        assert!(expected >= 100, "{expected}");
    }

    #[test]
    fn byte_estimate_counts_only_agent_visible_messages() {
        let visible = vec![Message::user("x".repeat(400))];
        let mut mixed = visible.clone();
        mixed.push(Message::assistant("y".repeat(4000)).with_agent_visible(false));
        assert_eq!(estimate_bytes(&mixed), estimate_bytes(&visible));
    }

    #[test]
    fn summary_message_is_prefixed_and_hidden_from_the_user() {
        let message = summary_message("  сделали то-то  ");
        assert_eq!(message.role, Role::User);
        assert_eq!(
            message.text(),
            format!("{SUMMARY_PREFIX}\nсделали то-то"),
            "префикс плюс саммари без хвостовых пробелов"
        );
        assert!(message.is_agent_visible());
        assert!(!message.is_user_visible());
    }

    #[tokio::test]
    async fn compact_hides_history_and_appends_the_summary() {
        let provider = StubProvider::text("Task: починить тест. Next: запустить cargo test.");
        let mut history = sample_history();

        let outcome = compact(
            &provider,
            "mock-model",
            SUMMARY_SYSTEM_PROMPT,
            &mut history,
            Some(Usage::new(4000, 96)),
        )
        .await
        .unwrap();

        // Запрос суммаризации: свой system, вся видимая история и инструкция
        // последним сообщением; тулы не нужны.
        let request = provider.request();
        assert_eq!(request.model, "mock-model");
        assert_eq!(request.system, SUMMARY_SYSTEM_PROMPT);
        assert!(request.tools.is_empty());
        assert_eq!(request.messages.len(), 4);
        assert_eq!(request.messages[..3], sample_history()[..3]);
        assert_eq!(request.messages[3], Message::user(SUMMARY_INSTRUCTIONS));

        // Ничего не удалено: старые сообщения на месте, только скрыты от модели.
        assert_eq!(history.len(), 4);
        assert_eq!(outcome.hidden.len(), 3);
        for (before, after) in sample_history().iter().zip(&history[..3]) {
            assert_eq!(before.content, after.content, "содержимое не переписано");
            assert!(!after.is_agent_visible());
            assert!(after.is_user_visible(), "пользователь видит свою переписку");
        }
        assert_eq!(outcome.hidden, history[..3]);

        let summary = history.last().unwrap();
        assert_eq!(summary, &outcome.summary);
        assert!(summary.text().starts_with(SUMMARY_PREFIX));
        assert!(summary.text().contains("запустить cargo test"));
        assert!(summary.is_agent_visible());
        assert!(!summary.is_user_visible());

        // Оценка «до» — из usage, «после» — по байтам оставшегося промпта.
        assert_eq!(outcome.tokens_before, 4096);
        assert_eq!(outcome.tokens_after, estimate_bytes(&history));
        assert!(
            outcome.tokens_before > outcome.tokens_after,
            "{} → {}",
            outcome.tokens_before,
            outcome.tokens_after
        );
    }

    /// Уже скрытые сообщения второй раз в суммаризацию не уходят.
    #[tokio::test]
    async fn compact_summarizes_only_the_visible_part() {
        let provider = StubProvider::text("второе саммари");
        let mut history = vec![
            Message::user("древняя просьба").with_agent_visible(false),
            summary_message("первое саммари"),
            Message::user("свежая просьба"),
        ];

        compact(
            &provider,
            "mock-model",
            SUMMARY_SYSTEM_PROMPT,
            &mut history,
            None,
        )
        .await
        .unwrap();

        let request = provider.request();
        assert_eq!(request.messages.len(), 3, "скрытое сообщение не пошло");
        assert!(request.messages[0].text().starts_with(SUMMARY_PREFIX));
        assert_eq!(request.messages[1], Message::user("свежая просьба"));

        assert_eq!(history.len(), 4);
        assert!(
            history.iter().take(3).all(|m| !m.is_agent_visible()),
            "прошлое саммари тоже уходит из промпта"
        );
        assert!(history.last().unwrap().text().contains("второе саммари"));
    }

    #[tokio::test]
    async fn compact_refuses_an_empty_history() {
        let provider = StubProvider::text("нечего");
        let mut history = vec![Message::user("старое").with_agent_visible(false)];
        let error = compact(&provider, "m", "s", &mut history, None)
            .await
            .unwrap_err();
        assert!(matches!(error, CompactionError::NothingToCompact));
        assert_eq!(history.len(), 1, "история не тронута");
    }

    #[tokio::test]
    async fn compact_reports_a_broken_summarizer() {
        for (reply, expected) in [
            (Reply::NoComplete, "не отдав готовое сообщение"),
            (Reply::Text("   ".into()), "пустое саммари"),
        ] {
            let provider = StubProvider::new(reply);
            let mut history = sample_history();
            let error = compact(&provider, "m", "s", &mut history, None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(history, sample_history(), "история не тронута");
        }
    }

    /// Переполнение контекста самой суммаризацией — обычная ошибка наружу:
    /// решение о том, что делать с ходом, принимает агент.
    #[tokio::test]
    async fn compact_propagates_provider_errors() {
        let provider = StubProvider::new(Reply::Error(ProviderError::ContextLengthExceeded));
        let mut history = sample_history();
        let error = compact(&provider, "m", "s", &mut history, None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            CompactionError::Provider(ProviderError::ContextLengthExceeded)
        ));
        assert!(error.to_string().contains("контекст модели переполнен"));
        assert_eq!(history, sample_history());
    }
}
