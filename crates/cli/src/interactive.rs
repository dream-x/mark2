//! Интерактивный режим: стрим хода в терминал, y/n-аппрувы и Ctrl+C.
//!
//! Отличий от print mode ровно три, и все они про присутствие человека:
//!
//! * в stdout идёт не только ответ, но и служебные строки (`→ tool`, `✓`/`✗`)
//!   и, по флагу `--show-thinking`, приглушённые рассуждения;
//! * [`Event::ApprovalRequired`] превращается в вопрос `allow? [y/N]` и
//!   ответ [`Op::Approve`] — ждём stdin, а не отказываем;
//! * Ctrl+C прерывает ход ([`Op::Interrupt`]), второй Ctrl+C выходит из CLI,
//!   не дожидаясь, пока ядро свернёт работу.
//!
//! Сам ход в MVP один: prompt обязателен, после `TurnFinished` CLI
//! завершается. REPL появится вместе со steering-очередью (фаза 3) — здесь
//! для него уже всё готово, ядро принимает [`Op`] в любой момент.

use std::io::{IsTerminal, Write};

use mark2_core::message::Usage;
use mark2_core::{AgentHandle, Event, Op, StopReason};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::print::accumulate;
use crate::render::{self, RenderOptions};

/// Ответ на запрос аппрува.
enum Answer {
    /// Пользователь разрешил.
    Yes,
    /// Пользователь отказал (в том числе просто нажав Enter или закрыв stdin).
    No,
    /// Пользователь нажал Ctrl+C вместо ответа.
    Interrupted,
}

/// Прогнать ход в терминале и вернуть причину завершения.
pub async fn run(
    handle: &AgentHandle,
    events: &mut UnboundedReceiver<Event>,
    opts: RenderOptions,
) -> StopReason {
    let mut usage = Usage::default();
    let mut interrupt_pending = false;
    let mut ends_with_newline = true;
    let mut reason = StopReason::Cancelled;

    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            signal = tokio::signal::ctrl_c() => {
                if let Err(error) = signal {
                    tracing::warn!(%error, "не удалось подписаться на Ctrl+C");
                }
                if interrupt_pending {
                    // Второй Ctrl+C: ядро не успело свернуть ход — уходим сами.
                    note(&opts, "^C выхожу");
                    return StopReason::Cancelled;
                }
                interrupt_pending = true;
                handle.interrupt();
                note(&opts, "^C прерываю ход (ещё раз — выход)");
                continue;
            }
        };

        // Канал закрылся без `TurnFinished` — ход всё равно окончен.
        let Some(event) = event else { break };

        match &event {
            Event::ApprovalRequired { id, name, input } => match ask(name, input, opts).await {
                Answer::Yes => handle.submit(Op::allow(id)),
                Answer::No => handle.submit(Op::deny(id)),
                Answer::Interrupted => {
                    interrupt_pending = true;
                    handle.interrupt();
                    note(&opts, "^C прерываю ход (ещё раз — выход)");
                }
            },
            Event::Error { message } => {
                let mut stderr = std::io::stderr().lock();
                let _ = writeln!(stderr, "{}", render::error_line(message, opts));
            }
            Event::Usage { usage: chunk } => accumulate(&mut usage, chunk),
            Event::TurnFinished { reason: finished } => reason = *finished,
            _ => {}
        }

        if let Some(text) = render::render(&event, opts)
            && !text.is_empty()
        {
            ends_with_newline = text.ends_with('\n');
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
        }

        if matches!(event, Event::TurnFinished { .. }) {
            break;
        }
    }

    let mut stdout = std::io::stdout().lock();
    if !ends_with_newline {
        let _ = stdout.write_all(b"\n");
    }
    let _ = stdout.flush();
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{}", render::usage_summary(&usage, opts));

    reason
}

/// Спросить про тул и дождаться ответа — либо Ctrl+C.
async fn ask(name: &str, input: &serde_json::Value, opts: RenderOptions) -> Answer {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(render::approval_prompt(name, input, opts).as_bytes());
    let _ = stdout.flush();
    drop(stdout);

    // Без терминала спрашивать некого: отказываем, как в print mode.
    if !std::io::stdin().is_terminal() {
        let _ = writeln!(std::io::stdout(), "n (stdin не терминал)");
        return Answer::No;
    }

    tokio::select! {
        line = read_line() => match line {
            Some(line) => if is_yes(&line) { Answer::Yes } else { Answer::No },
            // EOF (Ctrl+D) — тот же отказ.
            None => Answer::No,
        },
        _ = tokio::signal::ctrl_c() => Answer::Interrupted,
    }
}

/// Прочитать строку stdin, не блокируя runtime.
async fn read_line() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let mut buffer = String::new();
        match std::io::stdin().read_line(&mut buffer) {
            Ok(0) => None,
            Ok(_) => Some(buffer),
            Err(error) => {
                tracing::warn!(%error, "не удалось прочитать ответ пользователя");
                None
            }
        }
    })
    .await
    .unwrap_or(None)
}

/// Считается ли ответ согласием. Умолчание (пустая строка) — отказ: `[y/N]`.
fn is_yes(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "д" | "да"
    )
}

/// Служебная строка для человека — в stderr, чтобы не мешаться в ответе.
fn note(opts: &RenderOptions, text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "\n{}", render::note_line(text, *opts));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_yes_allows() {
        for yes in ["y", "Y", "yes", "YES\n", " да \n", "д"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "нет", "yep", "yolo"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }
}
