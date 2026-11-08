//! Post-edit сенсор: харнес сам проверяет правку, не дожидаясь, пока модель
//! догадается запустить тест.
//!
//! После каждого **успешного** `write`/`edit` agent loop запускает настроенную
//! команду диагностики ([`PostEditCheck`], `[agent] post_edit_command`) через
//! `sh -c` в cwd агента — тем же запуском, что и тул `bash`
//! ([`crate::tools::bash`]: своя process group, `kill -9` всей группе по
//! таймауту и отмене хода, окружение без переменных с ключами и токенами —
//! вывод проверки уходит модели, и `printenv` в ней не должен унести секрет).
//! Сенсор нужен слабым локальным моделям: они правят
//! файл и не перепроверяют, а когда перепроверяют — тратят на поиск правильной
//! команды целые итерации (e2e-findings P1 «нет контура верификации»).
//!
//! Что видит модель — [`Report::note`], дописанный к результату той же
//! записи; что видит человек — [`Report::marker`] (agent loop отдаёт его
//! [`crate::Event::Nudge`] с [`crate::Guard::PostEdit`], CLI печатает
//! `[agent] post_edit: exit N`). Правила:
//!
//! * **exit 0 — тишина**, даже если команда что-то напечатала: сигнал только
//!   об ошибке. «5 passed» или «All checks passed!» после каждой правки —
//!   чистый налог на контекст локальной модели, а отсутствие секции и есть
//!   норма. Нужны предупреждения — команда должна на них падать
//!   (`-D warnings`, `--strict`);
//! * **ненулевой exit, сигнал, таймаут** — секция с заголовком и выводом
//!   (голова и хвост, [`HEAD_BYTES`] + [`TAIL_BYTES`]: первая ошибка
//!   компилятора вверху, итог тест-раннера и последняя строка трейсбэка
//!   внизу). Результат записи при этом остаётся успешным: запись-то удалась,
//!   и `is_error` заставил бы модель переписать файл ещё раз;
//! * **отмена хода** — тоже тишина: команда убита, это не результат проверки.
//!
//! Конфига нет — сенсора нет: ни процесса, ни байта в промпте, ни события.

use std::path::Path;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::tools::bash::{self, Outcome, OutputCap, ShellRun};

/// Таймаут команды по умолчанию, секунды (`post_edit_timeout_seconds`).
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

/// Потолок таймаута — тот же, что у тула `bash`; больше молча зажимается.
pub const MAX_TIMEOUT_SECONDS: u64 = bash::MAX_TIMEOUT_SECONDS;

/// Сколько первых байт вывода проверки уходит модели.
pub const HEAD_BYTES: usize = 2_000;

/// Сколько последних байт вывода проверки уходит модели.
pub const TAIL_BYTES: usize = 2_000;

/// Тулы, после успешного вызова которых запускается проверка. Имена — из
/// реестра по умолчанию (`tools/`), как у сенсора write-churn.
pub fn applies_to(tool: &str) -> bool {
    matches!(tool, "write" | "edit")
}

/// Настроенная проверка: что запускать и сколько ждать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostEditCheck {
    /// Shell-команда для `sh -c` в cwd агента.
    pub command: String,
    /// Сколько ждать; дольше — группа процессов убивается.
    pub timeout: Duration,
}

impl PostEditCheck {
    /// Проверка с таймаутом по умолчанию.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        }
    }

    /// Задать таймаут; ноль поднимается до секунды, больше
    /// [`MAX_TIMEOUT_SECONDS`] — зажимается.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout.clamp(
            Duration::from_secs(1),
            Duration::from_secs(MAX_TIMEOUT_SECONDS),
        );
        self
    }
}

/// Проверка не прошла: что сказать модели и человеку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Модели: секция, которая дописывается к результату записи через пустую
    /// строку. По-английски — как и все тексты для модели.
    pub note: String,
    /// Человеку: текст [`crate::Event::Nudge`] — `exit N`, `таймаут 30s`, …
    pub marker: String,
}

/// Запустить проверку в `cwd`. `None` — сказать нечего: exit 0 или ход
/// отменён (тогда команда уже убита).
pub async fn run(check: &PostEditCheck, cwd: &Path, cancel: &CancellationToken) -> Option<Report> {
    if cancel.is_cancelled() {
        return None;
    }
    let cap = OutputCap::head_and_tail(HEAD_BYTES, TAIL_BYTES);
    let run = bash::run_shell(&check.command, cwd, check.timeout, cancel, cap).await;
    report(check, run)
}

/// Отчёт по итогу запуска — чистая функция, чтобы формат проверялся без
/// процессов.
fn report(check: &PostEditCheck, run: Result<ShellRun, String>) -> Option<Report> {
    let command = &check.command;
    let (headline, marker, output) = match run {
        Err(error) => (
            format!("post-edit check could not start: `{command}`"),
            "не запустилась".to_string(),
            error,
        ),
        Ok(ShellRun { outcome, output }) => match outcome {
            Outcome::Finished(Some(0)) | Outcome::Cancelled => return None,
            Outcome::Finished(Some(code)) => (
                format!("post-edit check failed (exit {code}): `{command}`"),
                format!("exit {code}"),
                output,
            ),
            Outcome::Finished(None) => (
                format!("post-edit check was killed by a signal: `{command}`"),
                "убита сигналом".to_string(),
                output,
            ),
            Outcome::TimedOut => {
                let seconds = check.timeout.as_secs();
                (
                    format!(
                        "post-edit check timed out after {seconds}s and was killed: `{command}`"
                    ),
                    format!("таймаут {seconds}s"),
                    output,
                )
            }
            Outcome::Failed(error) => (
                format!("post-edit check failed to finish ({error}): `{command}`"),
                "сбой запуска".to_string(),
                output,
            ),
        },
    };
    let output = if output.trim().is_empty() {
        "[no output]".to_string()
    } else {
        output
    };
    Some(Report {
        note: format!("{headline}\n{output}"),
        marker,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn check(command: &str) -> PostEditCheck {
        PostEditCheck::new(command)
    }

    async fn run_in(dir: &tempfile::TempDir, check: &PostEditCheck) -> Option<Report> {
        run(check, dir.path(), &CancellationToken::new()).await
    }

    /// Жив ли процесс: `kill -0` не шлёт сигнал, только проверяет.
    fn is_alive(pid: &str) -> bool {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    async fn wait_until_dead(pid: &str) -> bool {
        for _ in 0..40 {
            if !is_alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// exit 0 — тишина, даже с выводом: сигнал только об ошибке.
    #[tokio::test]
    async fn a_passing_check_says_nothing_even_with_output() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(run_in(&dir, &check("true")).await, None);
        assert_eq!(run_in(&dir, &check("echo '5 passed'")).await, None);
    }

    /// Ненулевой exit — заголовок с кодом и командой, затем вывод (stderr
    /// вместе со stdout, в порядке строк); маркер для человека — `exit N`.
    #[tokio::test]
    async fn a_failing_check_reports_the_exit_code_and_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let report = run_in(
            &dir,
            &check("echo first; echo 'AssertionError: got 3' >&2; exit 3"),
        )
        .await
        .unwrap();
        assert_eq!(
            report.note,
            "post-edit check failed (exit 3): `echo first; echo 'AssertionError: got 3' >&2; \
             exit 3`\nfirst\nAssertionError: got 3"
        );
        assert_eq!(report.marker, "exit 3");

        // Упала молча — так и сказано, а не пустая секция.
        let report = run_in(&dir, &check("exit 1")).await.unwrap();
        assert_eq!(
            report.note,
            "post-edit check failed (exit 1): `exit 1`\n[no output]"
        );
    }

    /// Команда идёт в cwd агента — относительные пути проекта работают.
    #[tokio::test]
    async fn the_check_runs_in_the_agent_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let report = run_in(&dir, &check("ls; exit 2")).await.unwrap();
        assert!(report.note.ends_with("\nmarker.txt"), "{}", report.note);
    }

    /// Таймаут: вся группа убита (и фоновый ребёнок тоже), частичный вывод
    /// сохранён, в заголовке — сколько ждали.
    #[tokio::test]
    async fn a_hanging_check_is_killed_by_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let report = run_in(
            &dir,
            &check("echo started; sleep 300 & echo $! > child.pid; echo $$ > shell.pid; sleep 300")
                .with_timeout(Duration::from_secs(1)),
        )
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(
            report
                .note
                .starts_with("post-edit check timed out after 1s and was killed: `echo started;"),
            "{}",
            report.note
        );
        assert!(report.note.ends_with("\nstarted"), "{}", report.note);
        assert_eq!(report.marker, "таймаут 1s");

        for file in ["shell.pid", "child.pid"] {
            let pid = std::fs::read_to_string(dir.path().join(file)).unwrap();
            assert!(
                wait_until_dead(pid.trim()).await,
                "{file} {pid} пережил таймаут"
            );
        }
    }

    /// Длинный вывод режется посередине: голова и хвост на месте, маркер
    /// говорит, сколько оставлено из скольких.
    #[tokio::test]
    async fn long_output_keeps_the_head_and_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let report = run_in(
            &dir,
            &check(
                "echo FIRST-LINE; i=0; while [ $i -lt 2000 ]; do echo \"noise line $i\"; \
                 i=$((i+1)); done; echo 'LAST: AssertionError'; exit 1",
            ),
        )
        .await
        .unwrap();
        let note = &report.note;
        assert!(note.contains("\nFIRST-LINE\n"), "{note}");
        assert!(note.ends_with("\nLAST: AssertionError"), "{note}");
        assert!(
            note.contains("[output truncated] kept the first 2000 and the last 2000 of "),
            "{note}"
        );
        assert!(!note.contains("noise line 1000\n"), "середина вырезана");
        // Голова + хвост + заголовок и маркер — не больше.
        assert!(note.len() < HEAD_BYTES + TAIL_BYTES + 300, "{}", note.len());
    }

    /// Отмена хода гасит и команду: процесс убит, сказать модели нечего.
    #[tokio::test]
    async fn cancellation_kills_the_check_and_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            trigger.cancel();
        });
        let started = Instant::now();
        let report = run(
            &check("echo $$ > shell.pid; sleep 300; exit 1"),
            dir.path(),
            &cancel,
        )
        .await;
        assert_eq!(report, None);
        assert!(started.elapsed() < Duration::from_secs(20));
        let pid = std::fs::read_to_string(dir.path().join("shell.pid")).unwrap();
        assert!(wait_until_dead(pid.trim()).await);

        // Отменённый заранее ход команду даже не запускает.
        let report = run(&check("touch ran.txt; exit 1"), dir.path(), &cancel).await;
        assert_eq!(report, None);
        assert!(!dir.path().join("ran.txt").exists());
    }

    #[tokio::test]
    async fn a_check_killed_by_a_signal_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let report = run_in(&dir, &check("echo bye; kill -9 $$")).await.unwrap();
        assert!(
            report
                .note
                .starts_with("post-edit check was killed by a signal: `"),
            "{}",
            report.note
        );
        assert_eq!(report.marker, "убита сигналом");
    }

    #[test]
    fn a_check_that_could_not_start_is_reported() {
        let report = report(
            &check("x"),
            Err("Failed to run the command in /nope: No such file".to_string()),
        )
        .unwrap();
        assert_eq!(
            report.note,
            "post-edit check could not start: `x`\nFailed to run the command in /nope: No such file"
        );
        assert_eq!(report.marker, "не запустилась");
    }

    #[test]
    fn timeout_is_clamped_and_defaults_to_thirty_seconds() {
        assert_eq!(check("x").timeout, Duration::from_secs(30));
        assert_eq!(
            check("x").with_timeout(Duration::ZERO).timeout,
            Duration::from_secs(1)
        );
        assert_eq!(
            check("x").with_timeout(Duration::from_secs(99_999)).timeout,
            Duration::from_secs(MAX_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn only_successful_writes_and_edits_are_checked() {
        assert!(applies_to("write"));
        assert!(applies_to("edit"));
        for other in ["bash", "read", "grep", "Write", ""] {
            assert!(!applies_to(other), "{other}");
        }
    }
}
