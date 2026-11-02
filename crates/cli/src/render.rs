//! Рендер [`Event`]-стрима в текст терминала.
//!
//! Это «первый фронтенд» harness, и логики в нём нет по определению:
//! [`render`] — **чистая функция** `Event -> Option<String>`, без ввода-вывода,
//! без состояния и без решений о ходе. `None` значит «этому событию нечего
//! показать в потоке вывода» — так тихо проходят служебные события
//! (`TurnStarted`, `MessageComplete`, `Usage`) и всё, что появится в `Event`
//! позже: ветка `_ => None` существует ровно затем, чтобы новый вариант ядра
//! не ронял CLI.
//!
//! Что рендерится **не** здесь и почему:
//!
//! * [`Event::Error`] — уходит в stderr, чтобы не смешиваться с ответом
//!   модели в пайпе;
//! * [`Event::ApprovalRequired`] — вопрос пользователю живёт в интерактивном
//!   цикле (для него есть чистая заготовка [`approval_prompt`]);
//! * итог [`Event::Usage`] — печатается один раз в конце хода в stderr
//!   ([`usage_summary`]);
//! * ответ, который модель положила целиком в thinking-канал
//!   ([`thinking_only_answer`]) — решение «показать рассуждения вместо
//!   ответа» принимает фронтенд, здесь только распознавание и текст пометки;
//! * сводка тул-коллов print mode ([`tool_summary_line`]) — она в stderr.

use mark2_core::Event;
use mark2_core::message::{ContentBlock, Message, Role, Usage};
use serde_json::Value;

/// Приглушённый текст (рассуждения, служебные строки).
const DIM: &str = "\x1b[2m";
/// Ошибка.
const RED: &str = "\x1b[31m";
/// Предупреждение.
const YELLOW: &str = "\x1b[33m";

/// Пометка для ответа, собранного из рассуждений.
pub const THINKING_ONLY_NOTE: &str = "модель не дала текстового ответа; показаны рассуждения";
/// Сброс оформления.
const RESET: &str = "\x1b[0m";

/// Сколько символов аргументов тула показываем в строке вызова.
const ARGS_LIMIT: usize = 96;

/// Сколько символов текста ошибки тула показываем рядом с `✗`.
const ERROR_LIMIT: usize = 160;

/// Как рендерить поток.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderOptions {
    /// Показывать `ThinkingDelta` (приглушённо). По умолчанию рассуждения
    /// скрыты: на локалках их больше, чем ответа.
    pub show_thinking: bool,
    /// Раскрашивать вывод ANSI-кодами.
    pub color: bool,
    /// Print mode: в поток идёт только текст ответа, без служебных строк.
    pub text_only: bool,
}

impl RenderOptions {
    /// Интерактивный режим.
    pub fn interactive(show_thinking: bool, color: bool) -> Self {
        Self {
            show_thinking,
            color,
            text_only: false,
        }
    }

    /// Print mode (`-p`): только ответ модели, пригодный для пайпа.
    pub fn print() -> Self {
        Self {
            show_thinking: false,
            color: false,
            text_only: true,
        }
    }

    /// Обернуть текст в ANSI-код, если раскраска включена.
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }
}

/// Что показать пользователю по поводу события.
///
/// Чистая функция: ни ввода-вывода, ни состояния. `None` — событию нечего
/// сказать в этом режиме.
pub fn render(event: &Event, opts: RenderOptions) -> Option<String> {
    match event {
        Event::TextDelta { text } => Some(text.clone()),

        Event::ThinkingDelta { text } => {
            if opts.text_only || !opts.show_thinking {
                None
            } else {
                Some(opts.paint(DIM, text))
            }
        }

        Event::ToolCallStarted { name, input, .. } => {
            if opts.text_only {
                None
            } else {
                let line = format!("→ {name} {}", summarize_args(input));
                Some(format!("\n{}\n", opts.paint(DIM, &line)))
            }
        }

        Event::ToolCallOutput { chunk, .. } => {
            if opts.text_only {
                None
            } else {
                Some(opts.paint(DIM, chunk))
            }
        }

        Event::ToolCallFinished {
            content, is_error, ..
        } => {
            if opts.text_only {
                None
            } else if *is_error {
                let line = format!("✗ {}", brief(content, ERROR_LIMIT));
                Some(format!("{}\n", opts.paint(RED, &line)))
            } else {
                Some(format!("{}\n", opts.paint(DIM, "✓")))
            }
        }

        // Всё остальное — служебное (`TurnStarted`, `MessageComplete`,
        // `Usage`, `TurnFinished`), рендерится отдельно (`Error`,
        // `ApprovalRequired`) или ещё не существует: новые варианты `Event`
        // обязаны проходить здесь молча, а не ронять CLI.
        _ => None,
    }
}

/// Вопрос об аппруве: тул, аргументы и `allow? [y/N]` одной заготовкой.
///
/// Ответ читает интерактивный цикл — здесь только текст.
pub fn approval_prompt(name: &str, input: &Value, opts: RenderOptions) -> String {
    let header = format!("? {name} {}", summarize_args(input));
    format!("\n{}\nallow? [y/N] ", opts.paint(DIM, &header))
}

/// Итог расхода токенов за ход (идёт в stderr).
pub fn usage_summary(usage: &Usage, opts: RenderOptions) -> String {
    let mut line = format!(
        "tokens: in {} / out {}",
        usage.input_tokens, usage.output_tokens
    );
    if let Some(cached) = usage.cache_read_tokens.filter(|tokens| *tokens > 0) {
        line.push_str(&format!(" (cached {cached})"));
    }
    opts.paint(DIM, &line)
}

/// Строка ошибки для stderr.
pub fn error_line(message: &str, opts: RenderOptions) -> String {
    opts.paint(RED, &format!("error: {message}"))
}

/// Служебная строка для человека (Ctrl+C, предупреждение о сессии) — тоже в
/// stderr, но это не ошибка.
pub fn note_line(text: &str, opts: RenderOptions) -> String {
    opts.paint(DIM, text)
}

/// Предупреждение для stderr: ход не сломан, но что-то не так, как ждали.
pub fn warning_line(text: &str, opts: RenderOptions) -> String {
    opts.paint(YELLOW, &format!("warning: {text}"))
}

/// Ответ модели, если она положила его целиком в thinking-канал.
///
/// `Some` — только для assistant-сообщения без тул-коллов, без непустого
/// текста и с непустыми рассуждениями: такое сообщение завершает ход, и без
/// подстановки ответ был бы пустым (qwen3.5 через ollama — половина прогонов
/// бенча). Рассуждения склеиваются через пустую строку и обрезаются по краям.
pub fn thinking_only_answer(message: &Message) -> Option<String> {
    if message.role != Role::Assistant
        || message.has_tool_uses()
        || !message.text().trim().is_empty()
    {
        return None;
    }
    let thinking: Vec<&str> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Thinking { text } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect();
    (!thinking.is_empty()).then(|| thinking.join("\n\n"))
}

/// Строка-сводка одного тул-колла для stderr print mode.
///
/// Ровно одна строка на вызов: `→ bash {"command":"ls"}` для успешного,
/// `✗ edit: <начало ошибки>` для упавшего, запрещённого или заблокированного.
pub fn tool_summary_line(
    name: &str,
    input: &Value,
    content: &str,
    is_error: bool,
    opts: RenderOptions,
) -> String {
    if is_error {
        opts.paint(RED, &format!("✗ {name}: {}", brief(content, ERROR_LIMIT)))
    } else {
        opts.paint(DIM, &format!("→ {name} {}", summarize_args(input)))
    }
}

/// Аргументы тула одной короткой строкой.
///
/// Компактный JSON: он однострочен по построению (переводы строк внутри
/// значений экранируются) и не врёт про то, что реально уехало в тул.
pub fn summarize_args(input: &Value) -> String {
    let text = serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
    brief(&text, ARGS_LIMIT)
}

/// Обрезать текст до `limit` символов (не байт — строка может быть не ASCII),
/// схлопнув переводы строк.
fn brief(text: &str, limit: usize) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    let trimmed = flat.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(limit).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mark2_core::event::StopReason;
    use mark2_core::message::Message;
    use serde_json::json;

    fn interactive() -> RenderOptions {
        RenderOptions::interactive(false, false)
    }

    /// Скрипт одного хода: текст → тул → результат → конец.
    fn scripted_turn() -> Vec<Event> {
        vec![
            Event::TurnStarted {
                turn_id: "t1".into(),
            },
            Event::TextDelta {
                text: "Смотрю ".into(),
            },
            Event::TextDelta {
                text: "файлы.".into(),
            },
            Event::ThinkingDelta {
                text: "надо бы ls".into(),
            },
            Event::ToolCallStarted {
                id: "c1".into(),
                name: "bash".into(),
                input: json!({"command": "ls"}),
            },
            Event::ToolCallOutput {
                id: "c1".into(),
                chunk: "src\n".into(),
            },
            Event::ToolCallFinished {
                id: "c1".into(),
                content: "src\n".into(),
                is_error: false,
            },
            Event::MessageComplete {
                message: Message::assistant("готово"),
            },
            Event::Usage {
                usage: Usage::new(100, 20),
            },
            Event::TurnFinished {
                reason: StopReason::Completed,
            },
        ]
    }

    fn rendered(events: &[Event], opts: RenderOptions) -> String {
        events
            .iter()
            .filter_map(|event| render(event, opts))
            .collect()
    }

    #[test]
    fn interactive_stream_shows_text_and_tool_lines() {
        assert_eq!(
            rendered(&scripted_turn(), interactive()),
            "Смотрю файлы.\n→ bash {\"command\":\"ls\"}\nsrc\n✓\n"
        );
    }

    #[test]
    fn print_mode_emits_only_the_answer() {
        assert_eq!(
            rendered(&scripted_turn(), RenderOptions::print()),
            "Смотрю файлы."
        );
    }

    #[test]
    fn thinking_is_hidden_by_default_and_dimmed_when_shown() {
        let event = Event::ThinkingDelta {
            text: "хм".into()
        };
        assert_eq!(render(&event, interactive()), None);
        assert_eq!(
            render(&event, RenderOptions::interactive(true, false)),
            Some("хм".to_string())
        );
        assert_eq!(
            render(&event, RenderOptions::interactive(true, true)),
            Some(format!("{DIM}хм{RESET}"))
        );
        // В print mode рассуждения не показываются даже по флагу.
        assert_eq!(
            render(
                &event,
                RenderOptions {
                    show_thinking: true,
                    ..RenderOptions::print()
                }
            ),
            None
        );
    }

    #[test]
    fn failed_tool_shows_a_short_reason() {
        let event = Event::ToolCallFinished {
            id: "c1".into(),
            content: "File not found: /tmp/nope\nCheck the path".into(),
            is_error: true,
        };
        assert_eq!(
            render(&event, interactive()),
            Some("✗ File not found: /tmp/nope Check the path\n".to_string())
        );
        assert_eq!(
            render(&event, RenderOptions::interactive(false, true)),
            Some(format!(
                "{RED}✗ File not found: /tmp/nope Check the path{RESET}\n"
            ))
        );
    }

    #[test]
    fn long_arguments_and_errors_are_truncated_on_char_boundaries() {
        let event = Event::ToolCallStarted {
            id: "c1".into(),
            name: "write".into(),
            input: json!({"content": "я".repeat(500)}),
        };
        let line = render(&event, interactive()).unwrap();
        assert!(line.ends_with("…\n"), "{line}");
        // Одна строка: перевод строки только в конце.
        assert_eq!(line.matches('\n').count(), 2, "{line}");

        let long_error = Event::ToolCallFinished {
            id: "c1".into(),
            content: "ошибка ".repeat(100),
            is_error: true,
        };
        assert!(render(&long_error, interactive()).unwrap().ends_with("…\n"));
    }

    #[test]
    fn service_events_render_to_nothing() {
        for event in [
            Event::TurnStarted {
                turn_id: "t".into(),
            },
            Event::MessageComplete {
                message: Message::assistant("x"),
            },
            Event::Usage {
                usage: Usage::new(1, 1),
            },
            Event::TurnFinished {
                reason: StopReason::Completed,
            },
            Event::Error {
                message: "boom".into(),
            },
            Event::ApprovalRequired {
                id: "c1".into(),
                name: "write".into(),
                input: json!({}),
            },
        ] {
            assert_eq!(render(&event, interactive()), None, "{event:?}");
            assert_eq!(render(&event, RenderOptions::print()), None, "{event:?}");
        }
    }

    /// Ядро дорастает новыми вариантами `Event` (компакция и дальше) — CLI
    /// обязан молча пропускать незнакомое.
    #[test]
    fn future_event_variants_are_ignored() {
        let compacted = Event::Compacted {
            tokens_before: 12_000,
            tokens_after: 3_000,
        };
        assert_eq!(render(&compacted, interactive()), None);
        assert_eq!(render(&compacted, RenderOptions::print()), None);
    }

    #[test]
    fn approval_prompt_shows_tool_and_question() {
        let prompt = approval_prompt("write", &json!({"path": "/tmp/a"}), interactive());
        assert!(prompt.contains("? write {\"path\":\"/tmp/a\"}"), "{prompt}");
        assert!(prompt.ends_with("allow? [y/N] "), "{prompt}");
    }

    #[test]
    fn usage_summary_counts_both_directions() {
        let opts = interactive();
        assert_eq!(
            usage_summary(&Usage::new(1200, 340), opts),
            "tokens: in 1200 / out 340"
        );
        let cached = Usage {
            cache_read_tokens: Some(1000),
            ..Usage::new(1200, 340)
        };
        assert_eq!(
            usage_summary(&cached, opts),
            "tokens: in 1200 / out 340 (cached 1000)"
        );
    }

    #[test]
    fn error_line_is_prefixed_and_colored_on_demand() {
        assert_eq!(error_line("нет связи", interactive()), "error: нет связи");
        assert_eq!(
            error_line("нет связи", RenderOptions::interactive(false, true)),
            format!("{RED}error: нет связи{RESET}")
        );
    }

    #[test]
    fn thinking_only_answer_is_recognized_only_without_text_and_tools() {
        // Ровно то, что qwen3.5 записала в сессию бенча.
        let thinking_only = Message::new(
            Role::Assistant,
            vec![ContentBlock::thinking(
                "\nDone. The count is 17, written to report.txt.\n",
            )],
        );
        assert_eq!(
            thinking_only_answer(&thinking_only).as_deref(),
            Some("Done. The count is 17, written to report.txt.")
        );

        // Несколько блоков склеиваются, пустые пропускаются; пробельный
        // текст текстом не считается.
        let split = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::thinking("первое"),
                ContentBlock::text("  \n"),
                ContentBlock::thinking("   "),
                ContentBlock::thinking("второе"),
            ],
        );
        assert_eq!(
            thinking_only_answer(&split).as_deref(),
            Some("первое\n\nвторое")
        );

        for message in [
            // Есть текст — это нормальный ответ.
            Message::new(
                Role::Assistant,
                vec![ContentBlock::thinking("хм"), ContentBlock::text("ответ")],
            ),
            // Есть тул-колл — ход ещё не кончился.
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::thinking("позову ls"),
                    ContentBlock::tool_use("c1", "bash", json!({"command": "ls"})),
                ],
            ),
            // Совсем пустое сообщение — подставлять нечего.
            Message::new(Role::Assistant, vec![]),
            Message::new(Role::Assistant, vec![ContentBlock::thinking(" \n ")]),
            // Не ответ модели.
            Message::new(Role::User, vec![ContentBlock::thinking("x")]),
        ] {
            assert_eq!(thinking_only_answer(&message), None, "{message:?}");
        }
    }

    #[test]
    fn tool_summary_is_one_compact_line_per_call() {
        let opts = RenderOptions::print();
        assert_eq!(
            tool_summary_line("bash", &json!({"command": "ls -la"}), "src\n", false, opts),
            "→ bash {\"command\":\"ls -la\"}"
        );
        assert_eq!(
            tool_summary_line(
                "edit",
                &json!({"path": "a.rs"}),
                "old_string not found in a.rs\nRead the file first",
                true,
                opts
            ),
            "✗ edit: old_string not found in a.rs Read the file first"
        );
        let long = tool_summary_line("bash", &json!({}), &"ошибка ".repeat(100), true, opts);
        assert!(long.ends_with('…'), "{long}");
        assert!(!long.contains('\n'), "{long}");
        assert_eq!(
            tool_summary_line(
                "bash",
                &json!({}),
                "boom",
                true,
                RenderOptions::interactive(false, true)
            ),
            format!("{RED}✗ bash: boom{RESET}")
        );
    }

    #[test]
    fn warning_line_is_prefixed_and_colored_on_demand() {
        assert_eq!(
            warning_line(THINKING_ONLY_NOTE, interactive()),
            format!("warning: {THINKING_ONLY_NOTE}")
        );
        assert_eq!(
            warning_line("x", RenderOptions::interactive(false, true)),
            format!("{YELLOW}warning: x{RESET}")
        );
    }

    #[test]
    fn note_line_is_not_an_error() {
        assert_eq!(note_line("^C прерываю", interactive()), "^C прерываю");
        assert_eq!(
            note_line("^C прерываю", RenderOptions::interactive(false, true)),
            format!("{DIM}^C прерываю{RESET}")
        );
    }
}
