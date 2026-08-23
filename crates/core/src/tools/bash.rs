//! Тул `bash`: выполнение shell-команды через `sh -c`.
//!
//! Три вещи здесь неочевидны и важны:
//!
//! * **Один поток вывода.** Скрипт запускается с `exec 2>&1`, поэтому stdout и
//!   stderr приходят одной трубой и в реальном порядке — склеивать два пайпа
//!   постфактум означало бы перемешать порядок строк.
//! * **Убиваем группу, а не процесс.** Ребёнок стартует в своей process group
//!   (`process_group(0)`), и по таймауту/отмене группа получает `kill -9`
//!   целиком — иначе `cmd &`-потомки переживают смерть шелла и держат пайп.
//! * **Вывод копится с капом.** Труба всё равно дренируется до конца (иначе
//!   процесс заблокируется на записи), но в буфер попадают только первые
//!   [`MAX_OUTPUT_BYTES`] байт.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Таймаут по умолчанию.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
/// Потолок таймаута: большее значение молча зажимается.
pub const MAX_TIMEOUT_SECONDS: u64 = 600;
/// Сколько байт вывода уходит модели.
pub const MAX_OUTPUT_BYTES: usize = 30_000;

/// Маркер обрезки вывода.
const TRUNCATION_MARKER: &str = "[output truncated]";

/// Аргументы тула `bash`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BashInput {
    /// Shell command to run with `sh -c` in the working directory.
    pub command: String,
    /// Timeout in seconds. Defaults to 120, values above 600 are clamped.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Тул выполнения shell-команд.
#[derive(Debug, Clone, Copy, Default)]
pub struct BashTool;

impl BashTool {
    /// Новый тул.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command with `sh -c` in the working directory. \
         Returns stdout and stderr merged, truncated at 30000 bytes; \
         a non-zero exit code is reported and counts as an error. \
         Use it for builds, tests, git, and for finding files (ls, find, grep)."
    }

    fn input_schema(&self) -> Value {
        input_schema_for::<BashInput>()
    }

    async fn execute(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        let input: BashInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid arguments for tool `bash`: {error}. Expected {{\"command\": \"...\"}}."
                ));
            }
        };

        if input.command.trim().is_empty() {
            return ToolOutput::err("`command` is empty: pass the shell command to run.");
        }

        if ctx.is_cancelled() {
            return ToolOutput::err("Command was cancelled before it started.");
        }

        let timeout = Duration::from_secs(
            input
                .timeout_seconds
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
                .clamp(1, MAX_TIMEOUT_SECONDS),
        );

        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            // stderr в ту же трубу, что и stdout — порядок строк сохраняется.
            .arg(format!("exec 2>&1\n{}", input.command))
            .current_dir(&ctx.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Своя process group, чтобы убить всё дерево одним kill.
            .process_group(0)
            .kill_on_drop(true);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Failed to run the command in {}: {error}",
                    ctx.cwd.display()
                ));
            }
        };

        let pid = child.id();
        let Some(mut stdout) = child.stdout.take() else {
            return ToolOutput::err("Failed to capture command output.");
        };

        let mut buffer: Vec<u8> = Vec::new();
        let mut total_bytes: usize = 0;

        let outcome = {
            let run = async {
                drain_capped(&mut stdout, &mut buffer, &mut total_bytes, MAX_OUTPUT_BYTES).await;
                child.wait().await
            };

            tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => Outcome::Cancelled,
                result = tokio::time::timeout(timeout, run) => match result {
                    Ok(Ok(status)) => Outcome::Finished(status.code()),
                    Ok(Err(error)) => Outcome::Failed(error.to_string()),
                    Err(_) => Outcome::TimedOut,
                },
            }
        };

        if !matches!(outcome, Outcome::Finished(_)) {
            kill_process_group(pid).await;
            let _ = child.kill().await;
        }

        let mut output = String::from_utf8_lossy(&buffer).trim_end().to_string();
        let truncated = total_bytes > buffer.len();

        if truncated {
            push_line(
                &mut output,
                &format!(
                    "{TRUNCATION_MARKER} kept the first {} of {total_bytes} bytes",
                    buffer.len()
                ),
            );
        }

        match outcome {
            Outcome::Finished(Some(0)) => {
                if output.is_empty() {
                    ToolOutput::ok("[no output]")
                } else {
                    ToolOutput::ok(output)
                }
            }
            Outcome::Finished(Some(code)) => {
                push_line(&mut output, &format!("[exit code: {code}]"));
                ToolOutput::err(output)
            }
            Outcome::Finished(None) => {
                push_line(&mut output, "[command was terminated by a signal]");
                ToolOutput::err(output)
            }
            Outcome::TimedOut => {
                push_line(
                    &mut output,
                    &format!(
                        "[command timed out after {}s and was killed; \
                         re-run a faster command or raise `timeout_seconds` (max {MAX_TIMEOUT_SECONDS})]",
                        timeout.as_secs()
                    ),
                );
                ToolOutput::err(output)
            }
            Outcome::Cancelled => {
                push_line(&mut output, "[command was cancelled and killed]");
                ToolOutput::err(output)
            }
            Outcome::Failed(error) => {
                push_line(&mut output, &format!("[command failed: {error}]"));
                ToolOutput::err(output)
            }
        }
    }
}

/// Чем закончился запуск.
enum Outcome {
    /// Процесс завершился сам; внутри — код возврата (`None` при сигнале).
    Finished(Option<i32>),
    /// Истёк таймаут.
    TimedOut,
    /// Отменён ход.
    Cancelled,
    /// Не удалось дождаться процесса.
    Failed(String),
}

/// Дренирует поток до конца, копя в буфер не больше `limit` байт.
///
/// Читать нужно всегда: если перестать забирать из трубы, процесс заснёт на
/// записи и таймаут превратится в «зависание».
async fn drain_capped<R: AsyncRead + Unpin>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
    total: &mut usize,
    limit: usize,
) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                *total += read;
                if buffer.len() < limit {
                    let take = (limit - buffer.len()).min(read);
                    buffer.extend_from_slice(&chunk[..take]);
                }
            }
        }
    }
}

/// `kill -9` по всей группе процессов.
///
/// Шлём через `sh` со встроенным `kill`: libc в зависимостях ядра не нужен, а
/// отрицательный pid адресует именно группу.
async fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    let _ = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("kill -9 -{pid} 2>/dev/null"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Дописать служебную строку, не склеивая её с последней строкой вывода.
fn push_line(output: &mut String, line: &str) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Instant;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    async fn run(dir: &TempDir, input: Value) -> ToolOutput {
        BashTool::new()
            .execute(input, &ToolCtx::new(dir.path()))
            .await
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

    #[tokio::test]
    async fn runs_command_and_returns_stdout() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"command": "echo hello"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "hello");
    }

    #[tokio::test]
    async fn empty_output_is_marked() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"command": "true"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "[no output]");
    }

    #[tokio::test]
    async fn stderr_is_merged_in_order() {
        let dir = TempDir::new().unwrap();
        let out = run(
            &dir,
            json!({"command": "echo first; echo second >&2; echo third"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "first\nsecond\nthird");
    }

    #[tokio::test]
    async fn non_zero_exit_is_an_error_with_code() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"command": "echo nope >&2; exit 3"})).await;
        assert!(out.is_error);
        assert_eq!(out.content, "nope\n[exit code: 3]");
    }

    #[tokio::test]
    async fn runs_in_the_context_cwd() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let out = run(&dir, json!({"command": "ls"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "marker.txt");
    }

    #[tokio::test]
    async fn output_is_truncated_with_marker() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"command": "yes 0123456789 | head -c 90000"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains(TRUNCATION_MARKER), "{}", out.content);
        assert!(out.content.contains("of 90000 bytes"), "{}", out.content);
        // Полезная нагрузка + короткий маркер: заметно меньше исходных 90000.
        assert!(out.content.len() < MAX_OUTPUT_BYTES + 200);
        assert!(out.content.len() > MAX_OUTPUT_BYTES - 200);
    }

    /// Таймаут обязан именно убивать дерево процессов, а не отпускать его в фон.
    #[tokio::test]
    async fn timeout_kills_the_whole_process_group() {
        let dir = TempDir::new().unwrap();
        let started = Instant::now();
        let out = run(
            &dir,
            json!({
                "command": "sleep 300 & echo $! > child.pid; echo $$ > shell.pid; sleep 300",
                "timeout_seconds": 1,
            }),
        )
        .await;

        assert!(out.is_error);
        assert!(
            out.content.contains("timed out after 1s"),
            "{}",
            out.content
        );
        assert!(started.elapsed() < Duration::from_secs(20));

        let shell_pid = std::fs::read_to_string(dir.path().join("shell.pid")).unwrap();
        let child_pid = std::fs::read_to_string(dir.path().join("child.pid")).unwrap();
        assert!(
            wait_until_dead(shell_pid.trim()).await,
            "shell {shell_pid} survived the timeout"
        );
        assert!(
            wait_until_dead(child_pid.trim()).await,
            "background child {child_pid} survived the timeout"
        );
    }

    #[tokio::test]
    async fn partial_output_survives_a_timeout() {
        let dir = TempDir::new().unwrap();
        let out = run(
            &dir,
            json!({"command": "echo partial; sleep 300", "timeout_seconds": 1}),
        )
        .await;
        assert!(out.is_error);
        assert!(out.content.starts_with("partial\n"), "{}", out.content);
    }

    #[tokio::test]
    async fn timeout_is_clamped_to_the_maximum() {
        let dir = TempDir::new().unwrap();
        let out = run(
            &dir,
            json!({"command": "echo ok", "timeout_seconds": 999_999}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "ok");
    }

    #[tokio::test]
    async fn cancellation_before_start_is_reported() {
        let dir = TempDir::new().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx = ToolCtx::with_cancel(dir.path(), cancel);
        let out = BashTool::new()
            .execute(json!({"command": "echo hi"}), &ctx)
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("cancelled"), "{}", out.content);
    }

    #[tokio::test]
    async fn cancellation_mid_run_kills_the_command() {
        let dir = TempDir::new().unwrap();
        let cancel = CancellationToken::new();
        let ctx = ToolCtx::with_cancel(dir.path(), cancel.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        });

        let started = Instant::now();
        let out = BashTool::new()
            .execute(
                json!({"command": "echo $$ > shell.pid; sleep 300", "timeout_seconds": 600}),
                &ctx,
            )
            .await;

        assert!(out.is_error);
        assert!(out.content.contains("cancelled"), "{}", out.content);
        assert!(started.elapsed() < Duration::from_secs(20));

        let shell_pid = std::fs::read_to_string(dir.path().join("shell.pid")).unwrap();
        assert!(wait_until_dead(shell_pid.trim()).await);
    }

    #[tokio::test]
    async fn garbage_input_is_reported_without_panicking() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"command": 5})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Invalid arguments for tool `bash`"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn empty_command_is_reported() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"command": "   "})).await;
        assert!(out.is_error);
        assert!(
            out.content.contains("`command` is empty"),
            "{}",
            out.content
        );
    }

    #[test]
    fn schema_documents_command_and_timeout() {
        let schema = BashTool::new().input_schema();
        assert_eq!(schema["required"], json!(["command"]));
        assert!(schema["properties"]["command"]["description"].is_string());
        assert!(schema["properties"]["timeout_seconds"]["description"].is_string());
    }
}
