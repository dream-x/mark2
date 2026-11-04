//! Телеметрия обвязки: сколько раз за прогон сработали контроли харнеса.
//!
//! Источник — stderr print mode: каждый контроль оставляет там машинно-
//! греппабельную строку (контракт с `crates/cli/src/render.rs`):
//!
//! | счётчик | маркер в начале строки | кто печатает |
//! |---|---|---|
//! | `nudges_write_churn` | `[agent] write_churn: …` | `render::nudge_line` |
//! | `nudges_loop_detection` | `[agent] loop_detection: …` | `render::nudge_line` |
//! | `compactions` | `[agent] compaction: …` | `render::compaction_line` |
//! | `tool_errors` | `✗ <тул>: …` | `render::tool_summary_line` |
//! | `coerced` | хвост `· coerced N` строки `tokens: in …` | `render::usage_summary` |
//!
//! `✗` — ровно одна строка на тул-колл с `is_error`: кривые аргументы,
//! отказ политики, заблокированный повтор, ненулевой exit `bash`.
//!
//! Маркер засчитывается **только с начала строки** (ANSI снимается): тот же
//! текст внутри строки вызова (`→ bash {"command":"echo '[agent] …'"}`) — это
//! аргументы модели, а не срабатывание.
//!
//! Почему stderr, а не сессия: нуджей и приведений аргументов в JSONL нет, а
//! stderr остаётся и у прогона, убитого по таймауту раньше, чем сессия
//! дописалась. Токены из той же итоговой строки здесь не дублируются — они уже
//! лежат в [`RunRecord::tokens`](crate::results::RunRecord::tokens), разобранные
//! тем же [`parse_tokens_line`].

use serde::{Deserialize, Serialize};

use crate::metrics::{leading_number, parse_tokens_line, strip_ansi};

/// Префикс маркеров agent loop: `[agent] <имя>: <текст>`.
const AGENT_MARKER: &str = "[agent] ";

/// Префикс строки упавшего тул-колла: `✗ <тул>: <начало ошибки>`.
const TOOL_ERROR_MARKER: &str = "✗ ";

/// Начало итоговой строки хода.
const USAGE_PREFIX: &str = "tokens: in ";

/// Счётчик приведений в хвосте итоговой строки.
const COERCED_MARKER: &str = " · coerced ";

/// Срабатывания контролей харнеса за один прогон.
///
/// `#[serde(default)]` на всей структуре: счётчик, добавленный позже, в старом
/// `results.json` читается нулём.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Telemetry {
    /// Подсказки сенсора write-churn (`[agent] write_churn`).
    pub nudges_write_churn: usize,
    /// Заблокированные повторы детектора зацикливания (`[agent] loop_detection`).
    pub nudges_loop_detection: usize,
    /// Тул-коллы, спасённые приведением типов аргументов (`· coerced N`).
    /// `None` — итоговой строки нет (убит по таймауту, упал) или бинарь mark2
    /// старше счётчика.
    pub coerced: Option<usize>,
    /// Компакции контекста (`[agent] compaction`).
    pub compactions: usize,
    /// Тул-коллы, вернувшиеся ошибкой (`✗ <тул>:`).
    pub tool_errors: usize,
}

impl Telemetry {
    /// Нуджи всех предохранителей вместе.
    pub fn nudges(&self) -> usize {
        self.nudges_write_churn + self.nudges_loop_detection
    }
}

/// Разобрать stderr одного прогона.
///
/// Чистая функция: пустой или чужой stderr — нули, а не ошибка. Итоговых строк
/// в `-p` одна; если их почему-то несколько, `coerced` берётся из последней —
/// как и токены в [`crate::metrics::parse_tokens`].
pub fn parse(stderr: &str) -> Telemetry {
    let mut telemetry = Telemetry::default();
    for raw in stderr.lines() {
        let line = strip_ansi(raw);
        if let Some(rest) = line.strip_prefix(AGENT_MARKER) {
            match rest.split_once(':').map(|(name, _)| name) {
                Some("write_churn") => telemetry.nudges_write_churn += 1,
                Some("loop_detection") => telemetry.nudges_loop_detection += 1,
                Some("compaction") => telemetry.compactions += 1,
                _ => {}
            }
        } else if let Some(rest) = line.strip_prefix(TOOL_ERROR_MARKER) {
            if is_tool_name(rest) {
                telemetry.tool_errors += 1;
            }
        } else if line.starts_with(USAGE_PREFIX) && parse_tokens_line(&line).is_some() {
            telemetry.coerced = line
                .split_once(COERCED_MARKER)
                .and_then(|(_, tail)| leading_number(tail))
                .and_then(|count| usize::try_from(count).ok());
        }
    }
    telemetry
}

/// Начинается ли хвост строки `✗ …` с имени тула и двоеточия: `bash: …`.
///
/// Имя — одно слово (у неизвестного вызова на его месте id): строка с
/// пробелом до двоеточия — не сводка вызова.
fn is_tool_name(rest: &str) -> bool {
    rest.split_once(':')
        .is_some_and(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stderr_gives_zeros_and_no_coerced() {
        assert_eq!(parse(""), Telemetry::default());
        assert_eq!(parse("\n\n"), Telemetry::default());
        assert_eq!(Telemetry::default().coerced, None);
    }

    /// Строки — ровно те, что печатает `-p` (см. тесты `crates/cli/src/print.rs`).
    #[test]
    fn counts_every_marker_of_a_print_mode_run() {
        let stderr = concat!(
            "→ write {\"path\":\"test_chat.py\",\"content\":\"v1\"}\n",
            "→ write {\"path\":\"test_chat.py\",\"content\":\"v2\"}\n",
            "→ write {\"path\":\"test_chat.py\",\"content\":\"v3\"}\n",
            "[agent] write_churn: 3-я запись подряд в test_chat.py без запуска между ними \
             — подсказка запустить\n",
            "→ write {\"path\":\"test_chat.py\",\"content\":\"v4\"}\n",
            "[agent] write_churn: 4-я запись подряд в test_chat.py без запуска между ними \
             — подсказка запустить\n",
            "→ bash {\"command\":\"echo \\\"port = 4821\\\"\"}\n",
            "→ bash {\"command\":\"echo \\\"port = 4821\\\"\"}\n",
            "✗ bash: You have already made this exact tool call 2 times\n",
            "[agent] loop_detection: 3-й подряд одинаковый вызов `bash` не исполнен\n",
            "✗ read: File not found: missing.txt\n",
            "[agent] compaction: контекст сжат ~4096 → ~310 токенов\n",
            "tokens: in 1200 / out 340 · coerced 3\n",
        );
        assert_eq!(
            parse(stderr),
            Telemetry {
                nudges_write_churn: 2,
                nudges_loop_detection: 1,
                coerced: Some(3),
                compactions: 1,
                tool_errors: 2,
            }
        );
        assert_eq!(parse(stderr).nudges(), 3);
    }

    /// Фрагмент реального stderr (flint:40b на T17, `20261104-000902-t17-v1`):
    /// невалидные аргументы, ненулевой exit `bash`, `WARN` трейсинга с ANSI.
    #[test]
    fn real_stderr_fragment_counts_tool_errors() {
        let stderr = concat!(
            "→ bash {\"command\":\"pwd && ls -la\"}\n",
            "✗ bash: Invalid arguments for tool `bash`:   - `/timeout_seconds`: \"180.0\" is \
             not of types \"null\", \"integer\" Expected arguments matching this JSON Schema: \
             {\"type\":\"obje…\n",
            "✗ edit: Invalid arguments for tool `edit`:   - the arguments object: \
             \"new_string\" is a required property E…\n",
            "✗ write: Cannot create directory /private/var/folders/x/cwd: Permission denied \
             (os error 13)\n",
            "\u{1b}[33m WARN\u{1b}[0m \u{1b}[2mmark2_providers::openai_compat\u{1b}[0m\u{1b}[2m:\
             \u{1b}[0m ответ модели обрезан по лимиту токенов (finish_reason=length)\n",
            "warning: модель не дала текстового ответа; показаны рассуждения\n",
            "tokens: in 1581509 / out 53694 (cached 1536368)\n",
        );
        assert_eq!(
            parse(stderr),
            Telemetry {
                tool_errors: 3,
                // Бинарь до счётчика приведений: хвоста `· coerced` нет.
                coerced: None,
                ..Telemetry::default()
            }
        );
    }

    #[test]
    fn usage_line_with_and_without_the_coerced_tail() {
        for (stderr, coerced) in [
            ("tokens: in 100 / out 20 · coerced 0", Some(0)),
            ("tokens: in 100 / out 20 · coerced 12", Some(12)),
            ("tokens: in 100 / out 20 (cached 90) · coerced 4", Some(4)),
            (
                "\u{1b}[2mtokens: in 100 / out 20 (cached 90) · coerced 7\u{1b}[0m",
                Some(7),
            ),
            ("tokens: in 100 / out 20", None),
            ("tokens: in 100 / out 20 (cached 90)", None),
            // Хвост есть, числа нет.
            ("tokens: in 100 / out 20 · coerced x", None),
        ] {
            assert_eq!(parse(stderr).coerced, coerced, "stderr = {stderr:?}");
        }
    }

    /// Итог один на ход; если строк несколько — последняя, как у токенов.
    #[test]
    fn the_last_usage_line_wins() {
        let stderr =
            "tokens: in 1 / out 2 · coerced 5\nwarning: x\ntokens: in 3 / out 4 · coerced 1\n";
        assert_eq!(parse(stderr).coerced, Some(1));
        let stderr = "tokens: in 1 / out 2 · coerced 5\ntokens: in 3 / out 4\n";
        assert_eq!(parse(stderr).coerced, None);
    }

    /// Похожее на маркер, но не маркер: внутри строки вызова, с отступом,
    /// без двоеточия, чужое имя, слитно, сломанная итоговая строка.
    #[test]
    fn lookalike_lines_are_not_counted() {
        let stderr = concat!(
            "→ bash {\"command\":\"echo '[agent] write_churn: x'\"}\n",
            "→ edit {\"new_string\":\"✗ bash: boom\"}\n",
            "→ bash {\"command\":\"grep 'tokens: in 5 / out 3 · coerced 9' log\"}\n",
            "  [agent] write_churn: с отступом\n",
            "[agent] write_churn без двоеточия\n",
            "[agent] write_churned: чужое имя\n",
            "[agent] unknown_guard: будущий предохранитель\n",
            "[agent]loop_detection: слитно\n",
            "[agent]\n",
            "✗bash: слитно\n",
            "✗ \n",
            "✗ File not found: /tmp/nope\n",
            "✗ : пустое имя\n",
            "error: [agent] compaction: в тексте ошибки\n",
            " WARN mark2: ✗ bash: в логе трейсинга\n",
            "tokens: in x / out 3 · coerced 9\n",
            "tokens: in 5 out 3 · coerced 9\n",
        );
        assert_eq!(parse(stderr), Telemetry::default());
    }

    /// Строка ошибки тула может нести в тексте что угодно, включая чужие
    /// маркеры: считается один раз и только как ошибка.
    #[test]
    fn tool_error_text_does_not_leak_into_other_counters() {
        let stderr = "✗ bash: [agent] write_churn: tokens: in 5 / out 3 · coerced 9\n\
                      ✗ call_7: Denied by user\n";
        assert_eq!(
            parse(stderr),
            Telemetry {
                tool_errors: 2,
                ..Telemetry::default()
            }
        );
    }

    /// В `results.json` — плоский объект; старый объект без новых полей
    /// читается нулями, `coerced` без итоговой строки — `null`.
    #[test]
    fn serializes_flat_and_reads_partial_objects() {
        let telemetry = Telemetry {
            nudges_write_churn: 1,
            coerced: Some(2),
            tool_errors: 3,
            ..Telemetry::default()
        };
        assert_eq!(
            serde_json::to_value(telemetry).unwrap(),
            serde_json::json!({
                "nudges_write_churn": 1,
                "nudges_loop_detection": 0,
                "coerced": 2,
                "compactions": 0,
                "tool_errors": 3,
            })
        );
        let partial: Telemetry = serde_json::from_str(r#"{"tool_errors": 4}"#).unwrap();
        assert_eq!(
            partial,
            Telemetry {
                tool_errors: 4,
                ..Telemetry::default()
            }
        );
        assert_eq!(
            serde_json::to_value(Telemetry::default()).unwrap()["coerced"],
            serde_json::Value::Null
        );
    }
}
