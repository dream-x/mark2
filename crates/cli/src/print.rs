//! Print mode (`-p`): один ход, ответ в stdout, вопросов не задаём.
//!
//! Режим существует для пайпов и скриптов, поэтому дисциплина потоков жёсткая:
//! в **stdout** уходит только текст ответа модели (никаких `→ tool`, спиннеров
//! и рассуждений), всё служебное — в **stderr**: по строке на каждый тул-колл
//! (`→ bash …` / `✗ edit: …`), срабатывания предохранителей и сенсоров agent
//! loop (`[agent] write_churn: …`, `[agent] post_edit: exit N`) и компакции
//! (`[agent] compaction: …`), ошибки, предупреждения и итог токенов.
//!
//! Текст каждого assistant-сообщения хода начинается с новой строки: модель
//! часто пишет фразу перед тул-коллом без завершающего `\n`, и без разделителя
//! она склеилась бы со следующим сообщением («…test module.1. [blocker] …»).
//! Внутри одного сообщения дельты печатаются как пришли, байт-в-байт.
//!
//! Одно исключение из «рассуждений в stdout нет»: если ход закончился
//! сообщением, в котором есть только thinking (ни текста, ни тул-коллов), —
//! так qwen3.5 через ollama отвечает в половине прогонов — рассуждения
//! печатаются в stdout **вместо** ответа, а в stderr уходит предупреждение.
//! Пустой stdout с кодом 0 для скрипта хуже, чем ответ с пометкой.
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

use std::collections::HashMap;
use std::io::Write;

use mark2_core::message::Usage;
use mark2_core::{AgentHandle, Event, Op, StopReason};
use serde_json::Value;
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
    // Сколько тул-коллов спасло приведение типов аргументов — в итоговую строку.
    let mut coerced_calls = 0usize;
    // Закончился ли напечатанный в stdout текст переводом строки (до первого
    // текста — «да»: ответ не начинается с пустой строки).
    let mut ends_with_newline = true;
    // Было ли сообщение закрыто (`MessageComplete`) после последнего
    // напечатанного текста: следующий текст — уже другое сообщение хода.
    let mut message_ended = false;
    // Канал событий, закрывшийся без `TurnFinished`, — тоже конец хода:
    // считаем его прерванным, а не успешным.
    let mut reason = StopReason::Cancelled;
    // Имя и аргументы тул-коллов по id: `ToolCallFinished` их не несёт, а
    // `ToolCallStarted` бывает не у всех (запрет политики, блокировка
    // детектором зацикливания). Источник — `MessageComplete`.
    let mut calls: HashMap<String, (String, Value)> = HashMap::new();

    while let Some(event) = events.recv().await {
        match &event {
            Event::MessageComplete { message } => {
                for (id, name, input) in message.tool_uses() {
                    calls.insert(id.to_string(), (name.to_string(), input.clone()));
                }
                if let Some(thinking) = render::thinking_only_answer(message) {
                    let _ = writeln!(
                        err,
                        "{}",
                        render::warning_line(render::THINKING_ONLY_NOTE, opts)
                    );
                    if !ends_with_newline {
                        let _ = out.write_all(b"\n");
                    }
                    let _ = out.write_all(thinking.as_bytes());
                    ends_with_newline = thinking.ends_with('\n');
                }
                message_ended = true;
            }
            Event::ToolCallFinished {
                id,
                content,
                is_error,
            } => {
                let (name, input) = calls
                    .remove(id)
                    .unwrap_or_else(|| (id.clone(), Value::Null));
                let _ = writeln!(
                    err,
                    "{}",
                    render::tool_summary_line(&name, &input, content, *is_error, opts)
                );
            }
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
            Event::Nudge { guard, message, .. } => {
                let _ = writeln!(err, "{}", render::nudge_line(*guard, message, opts));
            }
            Event::Compacted {
                tokens_before,
                tokens_after,
            } => {
                let _ = writeln!(
                    err,
                    "{}",
                    render::compaction_line(*tokens_before, *tokens_after, opts)
                );
            }
            Event::Warning { message } => {
                let _ = writeln!(err, "{}", render::warning_line(message, opts));
            }
            Event::Usage { usage: chunk } => accumulate(&mut usage, chunk),
            Event::ArgumentsCoerced { .. } => coerced_calls += 1,
            Event::TurnFinished { reason: finished } => {
                reason = *finished;
            }
            _ => {}
        }

        if let Some(text) = render::render(&event, opts)
            && !text.is_empty()
        {
            // Первый текст нового сообщения — с новой строки, если предыдущее
            // её не закрыло (и если модель не начала с перевода строки сама).
            if message_ended && !ends_with_newline && !text.starts_with('\n') {
                let _ = out.write_all(b"\n");
            }
            message_ended = false;
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
    let _ = writeln!(
        err,
        "{}",
        render::usage_summary(&usage, coerced_calls, opts)
    );
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

    /// Ответ модели, пришедший несколькими текстовыми дельтами.
    fn streamed(
        deltas: &[&str],
        call: Option<(&str, &str, serde_json::Value)>,
    ) -> Vec<StreamChunk> {
        let text: String = deltas.concat();
        let mut chunks = answer(&text, call);
        chunks.splice(
            0..1,
            deltas
                .iter()
                .map(|delta| StreamChunk::TextDelta(delta.to_string())),
        );
        chunks
    }

    /// Ответ, который модель положила целиком в thinking-канал.
    fn thinking_only(text: &str) -> Vec<StreamChunk> {
        vec![
            StreamChunk::ThinkingDelta(text.to_string()),
            StreamChunk::Complete {
                message: Message::new(Role::Assistant, vec![ContentBlock::thinking(text)]),
                usage: Usage::new(100, 20),
            },
        ]
    }

    /// Строки stderr, начинающиеся с префикса.
    fn lines_with<'a>(stderr: &'a str, prefix: &str) -> Vec<&'a str> {
        stderr
            .lines()
            .filter(|line| line.starts_with(prefix))
            .collect()
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
        run_configured(
            script,
            AgentConfig::new("mock-model").with_policy(policy),
            prompt,
        )
        .await
    }

    /// То же с произвольным конфигом агента; `cwd` — всегда временная
    /// директория.
    async fn run_configured(
        script: Vec<Vec<StreamChunk>>,
        config: AgentConfig,
        prompt: &str,
    ) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let provider = MockProvider::new(script);

        let agent = Agent::new(
            Arc::clone(&provider) as Arc<dyn Provider>,
            ToolRegistry::with_defaults(EditStrategy::SearchReplace),
            config.with_cwd(&cwd),
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

    /// Память не влезла в кап: предупреждение — в stderr строкой `warning:`,
    /// в stdout только ответ; модель видит память с пометкой об обрезке.
    #[tokio::test]
    async fn truncated_memory_warning_goes_to_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let memory = "- a durable fact worth keeping around\n".repeat(200);
        std::fs::write(dir.path().join(mark2_core::memory::MEMORY_FILE), &memory).unwrap();
        let provider = MockProvider::new(vec![answer("ок", None)]);
        let agent = Agent::new(
            Arc::clone(&provider) as Arc<dyn Provider>,
            ToolRegistry::new(),
            AgentConfig::new("mock-model").with_cwd(dir.path()),
        );
        assert!(agent.memory().is_some_and(|memory| memory.truncated));

        let (handle, mut events) = agent.spawn();
        handle.submit(Op::user_input("привет"));
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let reason = run(&handle, &mut events, &mut stdout, &mut stderr).await;
        handle.shutdown().await;

        assert_eq!(reason, StopReason::Completed);
        assert_eq!(std::str::from_utf8(&stdout).unwrap(), "ок\n");
        let stderr = String::from_utf8(stderr).unwrap();
        let warnings = lines_with(&stderr, "warning: ");
        assert_eq!(warnings.len(), 1, "{stderr}");
        assert!(warnings[0].contains(".mark2.memory.md"), "{stderr}");
        assert!(warnings[0].contains(&memory.len().to_string()), "{stderr}");
        // Файл не тронут, а в промпте — пометка об обрезке.
        assert_eq!(
            std::fs::read_to_string(dir.path().join(mark2_core::memory::MEMORY_FILE)).unwrap(),
            memory
        );
        let system = &provider.requests()[0].system;
        assert!(system.contains("[memory truncated: "), "{system}");
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
        // Обычный ответ — без пометок и без строк о тулах.
        assert!(
            !harness.stderr().contains(render::THINKING_ONLY_NOTE),
            "{}",
            harness.stderr()
        );
        assert!(lines_with(harness.stderr(), "→").is_empty());

        // Запрос дошёл до провайдера ровно один раз и с вводом пользователя.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "mock-model");
        assert_eq!(requests[0].messages.len(), 1);
        assert_eq!(requests[0].messages[0].text(), "что там с тестами?");
        assert_eq!(requests[0].tools.len(), 5);
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
        assert_eq!(harness.stdout(), "Пишу файл.\nФайл создан.\n");
        assert_eq!(
            std::fs::read_to_string(harness.cwd.join(path)).unwrap(),
            "привет"
        );
        // Зато в stderr — строка о вызове.
        assert_eq!(
            lines_with(harness.stderr(), "→"),
            [r#"→ write {"path":"note.txt","content":"привет"}"#]
        );

        // Второй запрос уже содержит результат тула.
        let requests = harness.provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].messages.len(), 3);
        assert_eq!(requests[1].messages[2].role, Role::Tool);
    }

    /// Приведённые аргументы видны в строке вызова, а счётчик приведений — в
    /// итоговой строке рядом с токенами (и нулём, когда приводить было нечего).
    #[tokio::test]
    async fn coerced_calls_are_counted_in_the_final_stderr_line() {
        let harness = run_turn(
            vec![
                answer(
                    "Запускаю.",
                    Some((
                        "c1",
                        "bash",
                        json!({"command": "echo hi", "timeout_seconds": "60.0"}),
                    )),
                ),
                answer("Готово.", None),
            ],
            PermissionPolicy::Yolo,
            "скажи hi",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(
            lines_with(harness.stderr(), "→"),
            [r#"→ bash {"command":"echo hi","timeout_seconds":60}"#]
        );
        assert_eq!(
            harness.stderr().lines().last(),
            Some("tokens: in 200 / out 40 · coerced 1"),
            "{}",
            harness.stderr()
        );

        let plain = run_turn(vec![answer("Ок.", None)], PermissionPolicy::Yolo, "привет").await;
        assert_eq!(
            plain.stderr().lines().last(),
            Some("tokens: in 100 / out 20 · coerced 0"),
            "{}",
            plain.stderr()
        );
    }

    /// Находка живого ревью: фраза перед тул-коллом без `\n` склеивалась со
    /// следующим сообщением («…test module.1. [blocker] …»). Теперь каждое
    /// сообщение — с новой строки; пустое сообщение с одним тул-коллом
    /// разделитель не съедает, а дельты одного сообщения не разрываются.
    #[tokio::test]
    async fn text_of_each_message_starts_on_a_new_line() {
        let read = |id: &'static str, path: &str| Some((id, "read", json!({"path": path})));
        let harness = run_turn(
            vec![
                streamed(&["Смотрю ", "test module."], read("c1", "a.rs")),
                answer("", read("c2", "b.rs")),
                streamed(&["1. [blocker] ", "a.rs:1: x."], None),
            ],
            PermissionPolicy::Yolo,
            "отревьюй",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(
            harness.stdout(),
            "Смотрю test module.\n1. [blocker] a.rs:1: x.\n"
        );
    }

    /// Строку, которую модель закрыла сама (в конце сообщения или в начале
    /// следующего), разделитель не дублирует: пустых строк не появляется.
    #[tokio::test]
    async fn a_line_closed_by_the_model_gets_no_extra_newline() {
        let read = |id: &'static str, path: &str| Some((id, "read", json!({"path": path})));
        let harness = run_turn(
            vec![
                answer("Первое.\n", read("c1", "a.rs")),
                answer("Второе.", read("c2", "b.rs")),
                answer("\nТретье.\n", None),
            ],
            PermissionPolicy::Yolo,
            "три шага",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "Первое.\nВторое.\nТретье.\n");
    }

    /// Одиночное сообщение печатается байт-в-байт, как до разделителя:
    /// дельты подряд, внутренние переводы строк как есть, в конце хода —
    /// ровно один `\n` (свой у модели или добавленный).
    #[tokio::test]
    async fn a_single_message_is_printed_byte_for_byte() {
        for (deltas, expected) in [
            (
                &["Строка 1\n\n", "Стро", "ка 2"][..],
                "Строка 1\n\nСтрока 2\n",
            ),
            (&["ок", "\n"][..], "ок\n"),
            (&["  отступ", " и хвост  "][..], "  отступ и хвост  \n"),
        ] {
            let harness = run_turn(vec![streamed(deltas, None)], PermissionPolicy::Yolo, "x").await;
            assert_eq!(harness.reason, StopReason::Completed);
            assert_eq!(harness.stdout(), expected, "{deltas:?}");
        }
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
        assert_eq!(harness.stdout(), "Пишу файл.\nНе смог: нет доступа.\n");
        assert!(
            !harness.cwd.join("note.txt").exists(),
            "тул не должен был выполниться"
        );

        let stderr = harness.stderr();
        assert!(stderr.contains("auto-denied `write`"), "{stderr}");
        assert!(stderr.contains("--yolo"), "{stderr}");
        let failed = lines_with(stderr, "✗");
        assert_eq!(failed.len(), 1, "{stderr}");
        assert!(failed[0].starts_with("✗ write: Denied by user"), "{stderr}");

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

    /// P1 с бенча: финальное сообщение — один thinking. stdout не пуст, а
    /// stderr честно говорит, что это рассуждения, а не ответ.
    #[tokio::test]
    async fn thinking_only_final_answer_goes_to_stdout_with_a_warning() {
        let harness = run_turn(
            vec![thinking_only("The port value is 4821.")],
            PermissionPolicy::Yolo,
            "какой порт?",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "The port value is 4821.\n");
        let warnings = lines_with(harness.stderr(), "warning:");
        assert_eq!(
            warnings,
            [format!("warning: {}", render::THINKING_ONLY_NOTE)],
            "{}",
            harness.stderr()
        );
    }

    /// Рассуждения промежуточных шагов (рядом с тул-коллами) в stdout не
    /// попадают — только финальные, и с новой строки после уже выведенного.
    #[tokio::test]
    async fn only_the_final_thinking_only_message_is_used_as_the_answer() {
        let mut first = vec![
            StreamChunk::ThinkingDelta("надо записать файл".into()),
            StreamChunk::TextDelta("Пишу файл.".into()),
        ];
        first.push(StreamChunk::Complete {
            message: Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::thinking("надо записать файл"),
                    ContentBlock::text("Пишу файл."),
                    ContentBlock::tool_use(
                        "c1",
                        "write",
                        json!({"path": "note.txt", "content": "x"}),
                    ),
                ],
            ),
            usage: Usage::new(100, 20),
        });

        let harness = run_turn(
            vec![first, thinking_only("Файл создан.")],
            PermissionPolicy::Yolo,
            "создай note.txt",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "Пишу файл.\nФайл создан.\n");
        assert_eq!(lines_with(harness.stderr(), "warning:").len(), 1);
    }

    /// P3 + детектор зацикливания: в stderr по строке на каждый вызов,
    /// включая заблокированный, — видно, что делал агент.
    #[tokio::test]
    async fn every_tool_call_gets_one_stderr_line() {
        // Ровно цикл qwen с бенча: один и тот же echo раз за разом.
        let echo = |id: &str| {
            answer(
                "",
                Some((id, "bash", json!({"command": "echo \"port = 4821\""}))),
            )
        };
        let harness = run_turn(
            vec![
                echo("c1"),
                echo("c2"),
                echo("c3"),
                answer("", Some(("c4", "read", json!({"path": "missing.txt"})))),
                answer("port = 4821", None),
            ],
            PermissionPolicy::Yolo,
            "какой порт?",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "port = 4821\n");

        let stderr = harness.stderr();
        let summary: Vec<&str> = stderr
            .lines()
            .filter(|line| line.starts_with('→') || line.starts_with('✗'))
            .collect();
        assert_eq!(summary.len(), 4, "{stderr}");
        assert_eq!(summary[0], r#"→ bash {"command":"echo \"port = 4821\""}"#);
        assert_eq!(summary[1], summary[0]);
        assert!(
            summary[2].starts_with("✗ bash: You have already made this exact tool call 2 times"),
            "{stderr}"
        );
        assert!(summary[3].starts_with("✗ read: "), "{stderr}");
        assert!(summary[3].contains("missing.txt"), "{stderr}");
        // Блокировка помечена маркером предохранителя — по нему грепают логи.
        assert_eq!(
            lines_with(stderr, "[agent]"),
            ["[agent] loop_detection: 3-й подряд одинаковый вызов `bash` не исполнен"],
            "{stderr}"
        );
    }

    /// Write-churn в `-p`: запись исполнена (строка `→ write`), подсказка
    /// модели — в tool result, а в stderr — строка с маркером
    /// `[agent] write_churn`. stdout не тронут.
    #[tokio::test]
    async fn write_churn_nudge_lands_in_stderr_with_the_agent_marker() {
        let write = |id: &str, content: &str| {
            answer(
                "",
                Some((
                    id,
                    "write",
                    json!({"path": "test_chat.py", "content": content}),
                )),
            )
        };
        let harness = run_turn(
            vec![
                write("c1", "v1"),
                write("c2", "v2"),
                write("c3", "v3"),
                answer("Готово.", None),
            ],
            PermissionPolicy::Yolo,
            "напиши тест",
        )
        .await;

        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "Готово.\n");
        let stderr = harness.stderr();
        assert_eq!(lines_with(stderr, "→ write").len(), 3, "{stderr}");
        assert!(lines_with(stderr, "✗").is_empty(), "{stderr}");
        assert_eq!(
            lines_with(stderr, "[agent]"),
            [
                "[agent] write_churn: 3-я запись подряд в test_chat.py без запуска между ними \
                 — подсказка запустить"
            ],
            "{stderr}"
        );
        // Маркер — сразу за строкой своего вызова.
        let lines: Vec<&str> = stderr.lines().collect();
        let marker = lines
            .iter()
            .position(|line| line.starts_with("[agent]"))
            .unwrap();
        assert!(lines[marker - 1].starts_with("→ write"), "{stderr}");

        let requests = harness.provider.requests();
        let tool_result = tool_results(&requests[3].messages[6]);
        assert!(
            tool_result.contains("write #3 to test_chat.py in a row"),
            "{tool_result}"
        );
    }

    /// Post-edit сенсор в `-p`: проверка упала — в stderr строка
    /// `[agent] post_edit: exit N` сразу за строкой записи (запись успешна —
    /// `→`, не `✗`), вывод проверки — модели в tool result, stdout не тронут.
    /// Прошла — ни строки.
    #[tokio::test]
    async fn failing_post_edit_check_lands_in_stderr_with_the_agent_marker() {
        let script = || {
            vec![
                answer(
                    "",
                    Some(("c1", "write", json!({"path": "stats.py", "content": "x"}))),
                ),
                answer("Готово.", None),
            ]
        };
        let config = |command: &str| {
            AgentConfig::new("mock-model")
                .with_policy(PermissionPolicy::Yolo)
                .with_post_edit(mark2_core::PostEditCheck::new(command))
        };

        let harness = run_configured(
            script(),
            config("echo 'AssertionError: boom' >&2; exit 3"),
            "почини",
        )
        .await;
        assert_eq!(harness.reason, StopReason::Completed);
        assert_eq!(harness.stdout(), "Готово.\n");
        let stderr = harness.stderr();
        assert!(lines_with(stderr, "✗").is_empty(), "{stderr}");
        assert_eq!(
            lines_with(stderr, "[agent]"),
            ["[agent] post_edit: exit 3"],
            "{stderr}"
        );
        let lines: Vec<&str> = stderr.lines().collect();
        assert!(lines[0].starts_with("→ write"), "{stderr}");
        assert_eq!(lines[1], "[agent] post_edit: exit 3", "{stderr}");
        let requests = harness.provider.requests();
        let tool_result = tool_results(&requests[1].messages[2]);
        assert!(
            tool_result.ends_with("\n\npost-edit check failed (exit 3): `echo 'AssertionError: boom' >&2; exit 3`\nAssertionError: boom"),
            "{tool_result}"
        );

        let passing = run_configured(script(), config("echo ok"), "почини").await;
        assert_eq!(passing.reason, StopReason::Completed);
        assert!(
            lines_with(passing.stderr(), "[agent]").is_empty(),
            "{}",
            passing.stderr()
        );
    }

    /// Компакция в `-p`: порог срабатывает между итерациями единственного
    /// хода, в stderr — строка `[agent] compaction` между строкой вызова и
    /// итогом токенов; текст саммари в stdout не попадает.
    #[tokio::test]
    async fn compaction_lands_in_stderr_with_the_agent_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let mut first = answer("", Some(("c1", "read", json!({"path": "a.txt"}))));
        // Расход выше порога окна в 5000 токенов: компакция до второго запроса.
        if let Some(StreamChunk::Complete { usage, .. }) = first.last_mut() {
            *usage = Usage::new(4000, 96);
        }
        let provider = MockProvider::new(vec![
            first,
            answer("Task: прочитать a.txt. Next: ответить", None),
            answer("hello", None),
        ]);
        let agent = Agent::new(
            Arc::clone(&provider) as Arc<dyn Provider>,
            ToolRegistry::with_defaults(EditStrategy::SearchReplace),
            AgentConfig::new("mock-model")
                .with_cwd(dir.path())
                .with_policy(PermissionPolicy::Yolo)
                .with_compaction(mark2_core::CompactionConfig::new().with_context_limit(5000)),
        )
        .with_system_prompt("test");

        let (handle, mut events) = agent.spawn();
        handle.submit(Op::user_input("прочитай a.txt"));
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let reason = run(&handle, &mut events, &mut stdout, &mut stderr).await;
        handle.shutdown().await;

        assert_eq!(reason, StopReason::Completed);
        assert_eq!(std::str::from_utf8(&stdout).unwrap(), "hello\n");
        assert_eq!(
            provider.requests().len(),
            3,
            "итерация, суммаризация, итерация"
        );
        let stderr = String::from_utf8(stderr).unwrap();
        let markers = lines_with(&stderr, "[agent]");
        assert_eq!(markers.len(), 1, "{stderr}");
        assert!(
            markers[0].starts_with("[agent] compaction: контекст сжат ~4096 → ~"),
            "{stderr}"
        );
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(lines.len(), 3, "{stderr}");
        assert!(lines[0].starts_with("→ read"), "{stderr}");
        assert_eq!(lines[1], markers[0]);
        assert!(lines[2].starts_with("tokens: in "), "{stderr}");
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
