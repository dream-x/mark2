//! Print mode (`-p`): один ход, ответ в stdout, вопросов не задаём.
//!
//! Режим существует для пайпов и скриптов, поэтому дисциплина потоков жёсткая:
//! в **stdout** уходит только текст ответа модели (никаких `→ tool`, спиннеров
//! и рассуждений), всё служебное — в **stderr**.
//!
//! # Что делать с аппрувами
//!
//! Спросить в неинтерактивном режиме некого, а вариантов ровно два, и оба
//! плохи по-своему: молча выполнить (тогда `-p` тихо стал бы `--yolo` и
//! записал бы файлы там, где пользователь этого не разрешал) или отказать.
//!
//! Выбран **авто-deny**: политика остаётся той, которую задал пользователь, и
//! если она требует подтверждения, вызов отклоняется — модель получает обычный
//! tool result «denied by user» и продолжает ход без этого тула, а пользователь
//! видит на stderr, что именно было отклонено. Политика `Yolo` применяется
//! только когда её задали явно (`--yolo` или `policy = "yolo"` в конфиге) —
//! режим вывода никогда не расширяет права сам.

use std::io::Write;

use mark2_core::message::Usage;
use mark2_core::{AgentHandle, Event, Op, StopReason};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::render::{self, RenderOptions};

/// Прогнать ход до `TurnFinished` и вернуть причину завершения.
///
/// Писатели переданы явно (а не взяты из `std::io`), чтобы поток можно было
/// прогнать в тесте и сравнить с ожидаемым текстом.
pub async fn run(
    handle: &AgentHandle,
    events: &mut UnboundedReceiver<Event>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> StopReason {
    let opts = RenderOptions::print();
    let mut usage = Usage::default();
    let mut ends_with_newline = true;
    // Канал событий, закрывшийся без `TurnFinished`, — тоже конец хода:
    // считаем его прерванным, а не успешным.
    let mut reason = StopReason::Cancelled;

    while let Some(event) = events.recv().await {
        match &event {
            Event::ApprovalRequired { id, name, input } => {
                let _ = writeln!(
                    err,
                    "{}",
                    render::note_line(
                        &format!(
                            "auto-denied `{name}` {args}: -p не может спросить подтверждение; \
                             запусти без -p или разреши заранее (--yolo / policy = \"yolo\")",
                            args = render::summarize_args(input),
                        ),
                        opts,
                    )
                );
                handle.submit(Op::deny(id));
            }
            Event::Error { message } => {
                let _ = writeln!(err, "{}", render::error_line(message, opts));
            }
            Event::Usage { usage: chunk } => accumulate(&mut usage, chunk),
            Event::TurnFinished { reason: finished } => {
                reason = *finished;
            }
            _ => {}
        }

        if let Some(text) = render::render(&event, opts)
            && !text.is_empty()
        {
            ends_with_newline = text.ends_with('\n');
            let _ = out.write_all(text.as_bytes());
        }

        if matches!(event, Event::TurnFinished { .. }) {
            break;
        }
    }

    // Ответ модели может закончиться без перевода строки — в пайпе это склеит
    // его со следующей командой.
    if !ends_with_newline {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
    let _ = writeln!(err, "{}", render::usage_summary(&usage, opts));
    let _ = err.flush();

    reason
}

/// Сложить расход токенов нескольких запросов одного хода.
pub fn accumulate(total: &mut Usage, chunk: &Usage) {
    total.input_tokens = total.input_tokens.saturating_add(chunk.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(chunk.output_tokens);
    if let Some(cached) = chunk.cache_read_tokens {
        total.cache_read_tokens = Some(total.cache_read_tokens.unwrap_or(0) + cached);
    }
    if let Some(written) = chunk.cache_write_tokens {
        total.cache_write_tokens = Some(total.cache_write_tokens.unwrap_or(0) + written);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use mark2_core::message::{ContentBlock, Message, Role};
    use mark2_core::provider::{
        MessageStream, Provider, ProviderError, ProviderRequest, StreamChunk,
    };
    use mark2_core::tools::{EditStrategy, ToolRegistry};
    use mark2_core::{Agent, AgentConfig, PermissionPolicy};
    use serde_json::json;

    /// Провайдер со скриптованными ответами: `stream()` отдаёт по одному
    /// заранее записанному ответу за вызов и запоминает пришедшие запросы.
    struct MockProvider {
        script: Mutex<VecDeque<Vec<StreamChunk>>>,
        seen: Mutex<Vec<ProviderRequest>>,
    }

    impl MockProvider {
        fn new(script: Vec<Vec<StreamChunk>>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn requests(&self) -> Vec<ProviderRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn stream(&self, req: ProviderRequest) -> Result<MessageStream, ProviderError> {
            self.seen.lock().unwrap().push(req);
            let chunks = self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("скрипт провайдера кончился раньше, чем ход");
            Ok(Box::pin(futures::stream::iter(chunks.into_iter().map(Ok))))
        }
    }

    /// Содержимое tool result'ов сообщения (в `Message::text()` их нет).
    fn tool_results(message: &Message) -> String {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Ответ модели: текст плюс опциональный тул-колл.
    fn answer(text: &str, call: Option<(&str, &str, serde_json::Value)>) -> Vec<StreamChunk> {
        let mut chunks = vec![StreamChunk::TextDelta(text.to_string())];
        let mut blocks = vec![ContentBlock::text(text)];
        if let Some((id, name, input)) = call {
            blocks.push(ContentBlock::tool_use(id, name, input));
        }
        chunks.push(StreamChunk::Complete {
            message: Message::new(Role::Assistant, blocks),
            usage: Usage::new(100, 20),
        });
        chunks
    }

    struct Harness {
        provider: Arc<MockProvider>,
        cwd: PathBuf,
        _dir: tempfile::TempDir,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        reason: StopReason,
    }

    impl Harness {
        fn stdout(&self) -> &str {
            std::str::from_utf8(&self.stdout).unwrap()
        }

        fn stderr(&self) -> &str {
            std::str::from_utf8(&self.stderr).unwrap()
        }
    }

    /// Полный прогон print mode: submit → события → собранный вывод.
    async fn run_turn(
        script: Vec<Vec<StreamChunk>>,
        policy: PermissionPolicy,
        prompt: &str,
    ) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let provider = MockProvider::new(script);

        let agent = Agent::new(
            Arc::clone(&provider) as Arc<dyn Provider>,
            ToolRegistry::with_defaults(EditStrategy::SearchReplace),
            AgentConfig::new("mock-model")
                .with_cwd(&cwd)
                .with_policy(policy),
        )
        // Системный промпт фиксируем: иначе тест зависел бы от AGENTS.md,
        // который лежит выше по дереву от временной директории.
        .with_system_prompt("test");

        let (handle, mut events) = agent.spawn();
        handle.submit(Op::user_input(prompt));

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let reason = run(&handle, &mut events, &mut stdout, &mut stderr).await;
        handle.shutdown().await;

        Harness {
            provider,
            cwd,
            _dir: dir,
            stdout,
            stderr,
            reason,
        }
    }

    #[tokio::test]
    async fn plain_answer_goes_to_stdout_and_usage_to_stderr() {
        let harness = run_turn(
            vec![answer("Готово: тестов нет.", None)],
            PermissionPolicy::AskWrites,
            "что там с тестами?",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        // Ровно ответ модели плюс завершающий перевод строки.
        assert_eq!(harness.stdout(), "Готово: тестов нет.\n");
        assert!(
            harness.stderr().contains("tokens: in 100 / out 20"),
            "{}",
            harness.stderr()
        );

        // Запрос дошёл до провайдера ровно один раз и с вводом пользователя.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "mock-model");
        assert_eq!(requests[0].messages.len(), 1);
        assert_eq!(requests[0].messages[0].text(), "что там с тестами?");
        assert_eq!(requests[0].tools.len(), 4);
    }

    #[tokio::test]
    async fn tool_call_runs_under_yolo_and_only_text_reaches_stdout() {
        let path = "note.txt";
        let harness = run_turn(
            vec![
                answer(
                    "Пишу файл.",
                    Some(("c1", "write", json!({"path": path, "content": "привет"}))),
                ),
                answer("Файл создан.", None),
            ],
            PermissionPolicy::Yolo,
            "создай note.txt",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        // Ни `→ write`, ни `✓` в stdout: только текст ответа.
        assert_eq!(harness.stdout(), "Пишу файл.Файл создан.\n");
        assert_eq!(
            std::fs::read_to_string(harness.cwd.join(path)).unwrap(),
            "привет"
        );

        // Второй запрос уже содержит результат тула.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].messages.len(), 3);
        assert_eq!(requests[1].messages[2].role, Role::Tool);
    }

    /// Ключевое решение режима: без интерактива подтверждение не выпрашивается,
    /// а отклоняется — тул не выполняется, модель узнаёт об этом tool result'ом.
    #[tokio::test]
    async fn approval_is_auto_denied_without_running_the_tool() {
        let harness = run_turn(
            vec![
                answer(
                    "Пишу файл.",
                    Some(("c1", "write", json!({"path": "note.txt", "content": "x"}))),
                ),
                answer("Не смог: нет доступа.", None),
            ],
            PermissionPolicy::AskWrites,
            "создай note.txt",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "Пишу файл.Не смог: нет доступа.\n");
        assert!(
            !harness.cwd.join("note.txt").exists(),
            "тул не должен был выполниться"
        );

        let stderr = harness.stderr();
        assert!(stderr.contains("auto-denied `write`"), "{stderr}");
        assert!(stderr.contains("--yolo"), "{stderr}");

        // Модель получила отказ как обычный tool result.
        let requests = harness.provider.requests();
        let tool_result = tool_results(&requests[1].messages[2]);
        assert!(tool_result.contains("Denied by user"), "{tool_result}");
    }

    #[tokio::test]
    async fn read_only_policy_denies_without_any_stderr_prompt() {
        let harness = run_turn(
            vec![
                answer(
                    "Правлю файл.",
                    Some(("c1", "write", json!({"path": "note.txt", "content": "x"}))),
                ),
                answer("Ок, только читаю.", None),
            ],
            PermissionPolicy::ReadOnly,
            "создай note.txt",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert!(!harness.cwd.join("note.txt").exists());
        // Отказала политика, аппрув никто не запрашивал.
        assert!(!harness.stderr().contains("auto-denied"));
        let requests = harness.provider.requests();
        assert!(tool_results(&requests[1].messages[2]).contains("read-only"));
    }

    #[tokio::test]
    async fn provider_error_lands_in_stderr_and_ends_the_turn_with_error() {
        struct Failing;

        #[async_trait]
        impl Provider for Failing {
            fn name(&self) -> &str {
                "failing"
            }
            async fn stream(&self, _req: ProviderRequest) -> Result<MessageStream, ProviderError> {
                Err(ProviderError::Network("connection refused".into()))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let agent = Agent::new(
            Arc::new(Failing) as Arc<dyn Provider>,
            ToolRegistry::with_defaults(EditStrategy::SearchReplace),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        )
        .with_system_prompt("test");
        let (handle, mut events) = agent.spawn();
        handle.submit(Op::user_input("привет"));

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let reason = run(&handle, &mut events, &mut stdout, &mut stderr).await;
        handle.shutdown().await;

        assert_eq!(reason, StopReason::Error);
        assert!(stdout.is_empty(), "{stdout:?}");
        let stderr = std::str::from_utf8(&stderr).unwrap();
        assert!(stderr.contains("connection refused"), "{stderr}");
    }

    #[test]
    fn usage_accumulates_across_requests_of_one_turn() {
        let mut total = Usage::default();
        accumulate(&mut total, &Usage::new(100, 20));
        accumulate(
            &mut total,
            &Usage {
                cache_read_tokens: Some(90),
                ..Usage::new(120, 5)
            },
        );
        assert_eq!(total.input_tokens, 220);
        assert_eq!(total.output_tokens, 25);
        assert_eq!(total.cache_read_tokens, Some(90));
    }
}
