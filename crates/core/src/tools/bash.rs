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
//!
//! Запуск с этими тремя свойствами — [`run_shell`] — общий с post-edit
//! сенсором agent loop ([`crate::post_edit`]): тот же `sh -c`, та же группа и
//! тот же kill, только кап другой — голова и хвост вывода
//! ([`OutputCap::head_and_tail`]).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

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
        "Run a shell command with `sh -c`. It already runs in the agent's working directory: \
         use relative paths, do not `cd` to absolute paths. \
         Returns stdout and stderr merged, truncated at 30000 bytes; \
         a non-zero exit code is reported and counts as an error. \
         Use it for builds, tests, git, and for finding files (ls, find); \
         search file contents with the `grep` tool."
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

        let ShellRun {
            outcome,
            mut output,
        } = match run_shell(
            &input.command,
            &ctx.cwd,
            timeout,
            &ctx.cancel,
            OutputCap::head(MAX_OUTPUT_BYTES),
        )
        .await
        {
            Ok(run) => run,
            Err(message) => return ToolOutput::err(message),
        };

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Процесс завершился сам; внутри — код возврата (`None` при сигнале).
    Finished(Option<i32>),
    /// Истёк таймаут.
    TimedOut,
    /// Отменён ход.
    Cancelled,
    /// Не удалось дождаться процесса.
    Failed(String),
}

/// Итог [`run_shell`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellRun {
    /// Чем закончился процесс.
    pub(crate) outcome: Outcome,
    /// stdout и stderr одной строкой, без хвостовых пробелов; если вывод не
    /// влез в кап — с маркером обрезки (`[output truncated] …`) на месте
    /// разреза.
    pub(crate) output: String,
}

/// Какую часть вывода сохранить.
///
/// Труба дренируется до конца в любом случае; кап решает только, какие байты
/// попадут в результат: первые `head` и последние `tail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputCap {
    head: usize,
    tail: usize,
}

impl OutputCap {
    /// Только начало вывода — так режет тул `bash`.
    pub(crate) fn head(bytes: usize) -> Self {
        Self {
            head: bytes,
            tail: 0,
        }
    }

    /// Начало и конец: у диагностики важное бывает с обеих сторон (первая
    /// ошибка компилятора — вверху, итог тест-раннера и последняя строка
    /// трейсбэка — внизу).
    pub(crate) fn head_and_tail(head: usize, tail: usize) -> Self {
        Self { head, tail }
    }
}

/// Запустить `command` через `sh -c` в `cwd`: stderr в той же трубе, что
/// stdout, своя process group, по таймауту и отмене — `kill -9` всей группе.
///
/// `Err` — процесс не стартовал (текст для модели); всё остальное, включая
/// таймаут и отмену, — [`ShellRun`] с тем выводом, что успел накопиться.
pub(crate) async fn run_shell(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
    cap: OutputCap,
) -> Result<ShellRun, String> {
    let mut shell = Command::new("/bin/sh");
    shell
        .arg("-c")
        // stderr в ту же трубу, что и stdout — порядок строк сохраняется.
        .arg(format!("exec 2>&1\n{command}"))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Своя process group, чтобы убить всё дерево одним kill.
        .process_group(0)
        .kill_on_drop(true);

    let mut child = shell
        .spawn()
        .map_err(|error| format!("Failed to run the command in {}: {error}", cwd.display()))?;

    let pid = child.id();
    let Some(mut stdout) = child.stdout.take() else {
        return Err("Failed to capture command output.".to_string());
    };

    let mut captured = Captured::default();

    let outcome = {
        let run = async {
            drain_capped(&mut stdout, &mut captured, cap).await;
            child.wait().await
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => Outcome::Cancelled,
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

    Ok(ShellRun {
        outcome,
        output: captured.render(),
    })
}

/// Сохранённые куски вывода и сколько его было всего.
#[derive(Debug, Default)]
struct Captured {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl Captured {
    /// Текст результата: целиком, если влез, иначе голова, маркер обрезки и
    /// хвост. С одной головой маркер — последняя строка, как и раньше у
    /// `bash`.
    fn render(&self) -> String {
        let kept = self.head.len() + self.tail.len();
        if self.total <= kept {
            let mut whole = self.head.clone();
            whole.extend_from_slice(&self.tail);
            return String::from_utf8_lossy(&whole).trim_end().to_string();
        }

        let mut output = String::from_utf8_lossy(&self.head).trim_end().to_string();
        if self.tail.is_empty() {
            push_line(
                &mut output,
                &format!(
                    "{TRUNCATION_MARKER} kept the first {} of {} bytes",
                    self.head.len(),
                    self.total
                ),
            );
        } else {
            push_line(
                &mut output,
                &format!(
                    "{TRUNCATION_MARKER} kept the first {} and the last {} of {} bytes",
                    self.head.len(),
                    self.tail.len(),
                    self.total
                ),
            );
            push_line(&mut output, String::from_utf8_lossy(&self.tail).trim_end());
        }
        output
    }
}

/// Дренирует поток до конца, копя первые `cap.head` и последние `cap.tail`
/// байт.
///
/// Читать нужно всегда: если перестать забирать из трубы, процесс заснёт на
/// записи и таймаут превратится в «зависание». Хвост держится буфером не
/// больше удвоенного капа: лишнее срезается с начала пачкой, а не на каждом
/// чанке.
async fn drain_capped<R: AsyncRead + Unpin>(
    reader: &mut R,
    captured: &mut Captured,
    cap: OutputCap,
) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                captured.total += read;
                let mut rest = &chunk[..read];
                if captured.head.len() < cap.head {
                    let take = (cap.head - captured.head.len()).min(rest.len());
                    captured.head.extend_from_slice(&rest[..take]);
                    rest = &rest[take..];
                }
                if cap.tail > 0 && !rest.is_empty() {
                    captured.tail.extend_from_slice(rest);
                    if captured.tail.len() > 2 * cap.tail {
                        let excess = captured.tail.len() - cap.tail;
                        captured.tail.drain(..excess);
                    }
                }
            }
        }
    }
    if captured.tail.len() > cap.tail {
        let excess = captured.tail.len() - cap.tail;
        captured.tail.drain(..excess);
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

    /// Кап общего дренажа: с одной головой — формат `bash` как был (маркер
    /// последней строкой); с хвостом — маркер на месте разреза; влезло
    /// целиком — без маркера, сколько бы ни было кусков.
    #[tokio::test]
    async fn capture_keeps_the_head_and_optionally_the_tail() {
        async fn capture(input: &[u8], cap: OutputCap) -> String {
            let mut reader = input;
            let mut captured = Captured::default();
            drain_capped(&mut reader, &mut captured, cap).await;
            captured.render()
        }

        assert_eq!(
            capture(b"0123456789", OutputCap::head(4)).await,
            "0123\n[output truncated] kept the first 4 of 10 bytes"
        );
        assert_eq!(
            capture(b"0123456789", OutputCap::head_and_tail(3, 2)).await,
            "012\n[output truncated] kept the first 3 and the last 2 of 10 bytes\n89"
        );
        // Ровно влезло в голову и хвост — склеено обратно, без маркера.
        assert_eq!(
            capture(b"0123456789\n", OutputCap::head_and_tail(6, 5)).await,
            "0123456789"
        );
        assert_eq!(capture(b"ok\n", OutputCap::head(100)).await, "ok");

        // Хвост длиннее чанка чтения: срезается до капа, последние байты целы.
        let long: Vec<u8> = (0..50_000u32)
            .flat_map(|i| format!("{i}\n").into_bytes())
            .collect();
        let text = capture(&long, OutputCap::head_and_tail(10, 12)).await;
        assert!(text.starts_with("0\n1\n2\n3\n4\n"), "{text}");
        assert!(
            text.contains(&format!(
                "kept the first 10 and the last 12 of {} bytes",
                long.len()
            )),
            "{text}"
        );
        assert!(text.ends_with("\n49998\n49999"), "{text}");
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

    /// Живые прогоны: модели не верят, что команда уже идёт в cwd, и
    /// подставляют `cd <абсолютный путь> && …`. Описание говорит об этом прямо.
    #[test]
    fn description_says_commands_already_run_in_the_cwd() {
        let description = BashTool::new().description().to_string();
        assert!(
            description.contains("already runs in the agent's working directory"),
            "{description}"
        );
        assert!(description.contains("relative paths"), "{description}");
        assert!(
            description.contains("do not `cd` to absolute paths"),
            "{description}"
        );
    }

    /// Поиск по содержимому — тулом `grep`, а не шеллом: в read-only сессии
    /// `bash` отбивается политикой, а модель тянулась к нему именно ради grep.
    #[test]
    fn description_sends_content_search_to_the_grep_tool() {
        let description = BashTool::new().description().to_string();
        assert!(
            description.contains("search file contents with the `grep` tool"),
            "{description}"
        );
        assert!(!description.contains("find, grep"), "{description}");
    }
}
