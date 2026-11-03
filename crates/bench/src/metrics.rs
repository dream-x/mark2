//! Метрики одного прогона: что осталось от mark2 после выхода.
//!
//! Два источника, оба уже существуют в харнесе и ничего специально для бенча
//! не пишут:
//!
//! * **stderr print mode** — итоговая строка `tokens: in X / out Y` (плюс
//!   `(cached Z)`, если сервер отдал кэш промпта);
//! * **JSONL-сессия** в `$HOME/.mark2/sessions/**` — оттуда берутся глубокие
//!   метрики: сколько раз модель ходила к провайдеру, сколько тул-коллов
//!   сделала, сколько результатов вернулось ошибкой (главный индикатор
//!   качества tool calling у малых моделей) и была ли компакция.
//!
//! Сессия разбирается типами `mark2_core` — формат один на харнес и на бенч,
//! своего парсера JSONL здесь нет и быть не должно.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mark2_core::event::StopReason;
use mark2_core::message::{ContentBlock, Role};
use mark2_core::session::{LoadedSession, SessionItem, SessionLine, load_session};
use serde::{Deserialize, Serialize};

/// Расход токенов за прогон.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// Токены промпта.
    pub input: u64,
    /// Сгенерированные токены.
    pub output: u64,
    /// Сколько токенов промпта пришло из кэша, если сервер сообщил.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<u64>,
}

/// Найти в stderr итоговую строку расхода токенов.
///
/// Берётся **последняя** подходящая строка: перед ней в stderr может быть что
/// угодно (варнинги, авто-deny, логи `RUST_LOG`), а итог печатается один раз в
/// конце хода. ANSI-коды снимаются на всякий случай — в print mode раскраска
/// выключена, но stderr могли записать и из интерактивного прогона.
pub fn parse_tokens(stderr: &str) -> Option<Tokens> {
    stderr
        .lines()
        .rev()
        .find_map(|line| parse_tokens_line(&strip_ansi(line)))
}

/// Разбор одной строки вида `tokens: in 1200 / out 340 (cached 1000)`.
fn parse_tokens_line(line: &str) -> Option<Tokens> {
    let rest = line.split_once("tokens: in ")?.1;
    let (input, rest) = rest.split_once('/')?;
    let input: u64 = input.trim().parse().ok()?;

    let rest = rest.trim_start().strip_prefix("out ")?;
    let output: u64 = leading_number(rest)?;

    let cached = rest
        .split_once("(cached ")
        .and_then(|(_, tail)| leading_number(tail));

    Some(Tokens {
        input,
        output,
        cached,
    })
}

/// Число в начале строки (дальше может идти что угодно).
fn leading_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Убрать ANSI-escape-последовательности.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        // CSI-последовательность: \x1b[ … <финальный байт 0x40..0x7e>.
        for next in chars.by_ref() {
            if next.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

/// Глубокие метрики хода из JSONL-сессии.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetrics {
    /// Ответов модели — то есть итераций сэмплинга: лишние круги значат слабое
    /// планирование даже при пройденной задаче.
    pub assistant_messages: usize,
    /// Сколько тул-коллов модель запросила всего.
    pub tool_uses: usize,
    /// Сколько результатов тулов вернулись с `is_error`.
    pub tool_errors: usize,
    /// Сколько раз сработала компакция контекста.
    pub compactions: usize,
    /// Чем закончился последний ход, если сессия дописана до конца.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Битых строк в сессии (обрыв записи при kill по таймауту — нормально).
    pub skipped_lines: usize,
}

impl SessionMetrics {
    /// Доля ошибочных результатов среди тул-коллов. `None`, если тулов не было.
    pub fn tool_error_rate(&self) -> Option<f64> {
        (self.tool_uses > 0).then(|| self.tool_errors as f64 / self.tool_uses as f64)
    }
}

/// Посчитать метрики по строкам сессии.
pub fn session_metrics(session: &LoadedSession) -> SessionMetrics {
    let mut metrics = SessionMetrics {
        skipped_lines: session.skipped,
        ..SessionMetrics::default()
    };

    for SessionLine { item, .. } in &session.lines {
        match item {
            SessionItem::Message { message } => {
                if message.role == Role::Assistant {
                    metrics.assistant_messages += 1;
                }
                for block in &message.content {
                    match block {
                        ContentBlock::ToolUse { .. } => metrics.tool_uses += 1,
                        ContentBlock::ToolResult { is_error: true, .. } => {
                            metrics.tool_errors += 1;
                        }
                        _ => {}
                    }
                }
            }
            SessionItem::Compacted { .. } => metrics.compactions += 1,
            SessionItem::TurnFinished { reason, .. } => metrics.stop_reason = Some(*reason),
            // Новые варианты SessionItem не должны ронять бенч.
            _ => {}
        }
    }

    metrics
}

/// Прочитать сессию и посчитать метрики.
pub async fn read_session_metrics(path: &Path) -> Result<SessionMetrics> {
    let session = load_session(path)
        .await
        .with_context(|| format!("не удалось прочитать сессию {}", path.display()))?;
    Ok(session_metrics(&session))
}

/// Найти файл сессии в изолированной домашней директории прогона.
///
/// В свежем `$HOME` сессия ровно одна, но mark2 мог не записать её вовсе
/// (упал до создания, `kill` по таймауту раньше первой строки) — тогда `None`.
/// Если файлов почему-то несколько, берётся самый свежий.
pub fn find_session_file(home: &Path) -> Option<PathBuf> {
    let root = home.join(".mark2").join("sessions");
    let mut found = Vec::new();
    collect_jsonl(&root, &mut found);
    found.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    });
    found.pop()
}

/// Рекурсивно собрать `*.jsonl` (директория `YYYY/MM/DD` — деталь ядра, здесь
/// её не воспроизводим).
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mark2_core::message::{ContentBlock, Message, MessageMetadata, Role};
    use mark2_core::session::{SessionMeta, SessionRecorder};
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn parses_the_usage_line_of_print_mode() {
        let tokens = parse_tokens("tokens: in 1200 / out 340").unwrap();
        assert_eq!(
            tokens,
            Tokens {
                input: 1200,
                output: 340,
                cached: None
            }
        );
    }

    #[test]
    fn parses_cached_variant_and_ansi_dimmed_line() {
        let tokens =
            parse_tokens("\u{1b}[2mtokens: in 1200 / out 340 (cached 1000)\u{1b}[0m").unwrap();
        assert_eq!(
            tokens,
            Tokens {
                input: 1200,
                output: 340,
                cached: Some(1000)
            }
        );
    }

    /// Хвост итоговой строки (счётчик приведений аргументов `· coerced N`)
    /// парсер токенов пропускает — и с кэшем, и без.
    #[test]
    fn coercion_counter_suffix_does_not_break_the_usage_line() {
        assert_eq!(
            parse_tokens("tokens: in 1200 / out 340 · coerced 3").unwrap(),
            Tokens {
                input: 1200,
                output: 340,
                cached: None
            }
        );
        assert_eq!(
            parse_tokens("\u{1b}[2mtokens: in 1200 / out 340 (cached 1000) · coerced 0\u{1b}[0m")
                .unwrap(),
            Tokens {
                input: 1200,
                output: 340,
                cached: Some(1000)
            }
        );
    }

    #[test]
    fn takes_the_last_line_and_ignores_the_noise_around_it() {
        let stderr = concat!(
            "warn: сессия не пишется\n",
            "tokens: in 1 / out 2\n",
            "auto-denied `write` {\"path\":\"a\"}\n",
            "tokens: in 30 / out 4\n",
        );
        assert_eq!(
            parse_tokens(stderr).unwrap(),
            Tokens {
                input: 30,
                output: 4,
                cached: None
            }
        );
    }

    #[test]
    fn missing_or_broken_line_gives_none() {
        for stderr in [
            "",
            "error: connection refused",
            "tokens: in / out 4",
            "tokens: in 30 out 4",
            "tokens: in x / out 4",
        ] {
            assert_eq!(parse_tokens(stderr), None, "stderr = {stderr:?}");
        }
    }

    /// Фикстура сессии собирается настоящим `SessionRecorder` из `mark2-core`:
    /// метрики обязаны читать тот формат, который харнес пишет на самом деле, а
    /// не строку, придуманную в тесте.
    async fn fixture_session(dir: &Path) -> PathBuf {
        let recorder = SessionRecorder::create(
            dir,
            SessionMeta::new("/work", "qwen3.5:9b", "openai_compat"),
        )
        .await
        .unwrap();
        let path = recorder.path().to_path_buf();

        recorder.record(SessionItem::message(Message::user("создай файл")));
        recorder.record(SessionItem::turn_started("t1"));
        // Первый круг сэмплинга: два тул-колла, один из них с кривыми аргументами.
        recorder.record(SessionItem::message(Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("сейчас"),
                ContentBlock::tool_use("c1", "write", json!({"path": "a.txt", "content": "x"})),
                ContentBlock::tool_use("c2", "edit", json!({"path": "a.txt"})),
            ],
        )));
        recorder.record(SessionItem::message(Message::tool(vec![
            ContentBlock::tool_result("c1", "ok"),
            ContentBlock::tool_error("c2", "missing field `old_string`"),
        ])));
        // Второй круг: ещё один тул-колл, уже удачный.
        recorder.record(SessionItem::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "c3",
                "read",
                json!({"path": "a.txt"}),
            )],
        )));
        recorder.record(SessionItem::message(Message::tool_result("c3", "x")));
        recorder.record(SessionItem::Compacted {
            tokens_before: 12_000,
            tokens_after: 3_000,
        });
        // Саммари компакции — сообщение ассистента, скрытое от пользователя.
        recorder.record(SessionItem::message(
            Message::assistant("готово").with_metadata(MessageMetadata::hidden_from_user()),
        ));
        recorder.record(SessionItem::turn_finished("t1", StopReason::Completed));
        recorder.close().await;

        path
    }

    #[tokio::test]
    async fn metrics_come_from_a_real_session_file() {
        let dir = TempDir::new().unwrap();
        let path = fixture_session(dir.path()).await;

        let metrics = read_session_metrics(&path).await.unwrap();
        assert_eq!(
            metrics,
            SessionMetrics {
                assistant_messages: 3,
                tool_uses: 3,
                tool_errors: 1,
                compactions: 1,
                stop_reason: Some(StopReason::Completed),
                skipped_lines: 0,
            }
        );
        assert!((metrics.tool_error_rate().unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(SessionMetrics::default().tool_error_rate(), None);
    }

    #[tokio::test]
    async fn truncated_session_is_counted_not_fatal() {
        let dir = TempDir::new().unwrap();
        let path = fixture_session(dir.path()).await;
        let mut raw = std::fs::read_to_string(&path).unwrap();
        raw.push_str("{\"ts\":\"2026-08-24T09:00:0");
        std::fs::write(&path, raw).unwrap();

        let metrics = read_session_metrics(&path).await.unwrap();
        assert_eq!(metrics.skipped_lines, 1);
        assert_eq!(metrics.tool_uses, 3);
    }

    #[tokio::test]
    async fn session_is_found_inside_an_isolated_home() {
        let home = TempDir::new().unwrap();
        assert_eq!(find_session_file(home.path()), None);

        let base = home.path().join(".mark2").join("sessions");
        let path = fixture_session(&base).await;
        assert_eq!(find_session_file(home.path()), Some(path));
    }

    #[test]
    fn ansi_stripping_keeps_the_text() {
        assert_eq!(strip_ansi("\u{1b}[2mтихо\u{1b}[0m"), "тихо");
        assert_eq!(strip_ansi("без кодов"), "без кодов");
    }
}
