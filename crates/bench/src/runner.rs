//! Протокол прогона из `bench/STRATEGY.md`.
//!
//! Порядок и изоляция здесь — не украшение, а условие сравнимости чисел:
//!
//! * **последовательно и model-major**: все задачи одной модели идут подряд,
//!   чтобы ollama не выгружала её из памяти между прогонами, а параллель не
//!   врала в таймингах (модели делят GPU и RAM);
//! * **прогрев** — один нетаймируемый вызов на модель: загрузка весов в память
//!   не должна попасть в wall time первой задачи;
//! * **свежая tmp-cwd** с копией `setup/` и **свежий tmp-`$HOME`** на каждый
//!   прогон: `$HOME` изолирует пользовательский конфиг mark2 и заодно даёт
//!   директорию сессий, из которой сессия и забирается;
//! * **температура 0** через `<cwd>/.mark2.toml`: сравниваем модели, а не
//!   сэмплинг;
//! * **настройки задачи** — тоже снаружи, без особых режимов в mark2:
//!   `policy = "read-only"` превращается в `--policy read-only` вместо
//!   `--yolo`, `max_turns` — в строку секции `[agent]`, `context_limit` — в
//!   секцию `[models."<id>"]` того же `.mark2.toml` (см. [`project_config`]);
//! * **таймаут** = `timeout_seconds` задачи × множитель модели, по истечении —
//!   `kill` и пометка `timed_out` (прогон засчитывается как непройденный, но не
//!   вешает бенч); у `verify.sh` свой бюджет — [`verify_timeout`].
//!
//! Ранер не интерпретирует ответ модели: приговор выносит `verify.sh`.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use jiff::{Timestamp, Zoned};
use tokio::process::Command;

use crate::metrics::{find_session_file, parse_tokens, read_session_metrics};
use crate::models::ModelSpec;
use crate::results::{Results, RunRecord, VerifyOutcome};
use crate::task::{Task, TaskPolicy};

/// Промпт прогрева: тривиальный ответ без тулов, только чтобы поднять модель.
pub const WARMUP_PROMPT: &str = "Reply with exactly: ok";

/// Бюджет прогрева. Он не таймируется, но и висеть вечно не должен.
pub const WARMUP_TIMEOUT: Duration = Duration::from_secs(300);

/// Бюджет `verify.sh` по умолчанию: проверка локальная и должна быть
/// мгновенной. Задача, которой этого мало (сервер + браузер), задаёт свой
/// `verify_timeout_seconds`.
pub const VERIFY_TIMEOUT_SECONDS: u64 = 30;

/// Проектный конфиг, который кладётся в cwd каждого прогона (база; задача с
/// `max_turns` или `context_limit` дописывает к нему — см. [`project_config`]).
pub const PROJECT_CONFIG: &str = "[agent]\ntemperature = 0.0\n";

/// Сколько символов вывода `verify.sh` сохраняем в `results.json`.
const VERIFY_OUTPUT_LIMIT: usize = 4000;

/// Ранер: знает, чем запускать и куда складывать артефакты.
#[derive(Debug, Clone)]
pub struct Runner {
    /// Путь к бинарю mark2.
    pub mark2: PathBuf,
    /// Абсолютный путь директории результатов прогона.
    pub run_dir: PathBuf,
    /// `--repeats`, если задан.
    pub repeats_override: Option<usize>,
    /// Делать ли прогрев модели (выключается только в тестах).
    pub warmup: bool,
}

impl Runner {
    /// Собрать ранер.
    ///
    /// Оба пути разворачиваются в абсолютные: mark2 запускается с
    /// `current_dir` = tmp-cwd прогона (относительный путь к бинарю резолвился
    /// бы уже от неё), а из `run_dir` растут пути логов и `MARK2_STDOUT`,
    /// который читает `verify.sh` — тоже из чужой cwd.
    pub fn new(mark2: impl AsRef<Path>, run_dir: impl AsRef<Path>) -> Self {
        Self {
            mark2: crate::resolve_path(mark2),
            run_dir: crate::resolve_path(run_dir),
            repeats_override: None,
            warmup: true,
        }
    }

    /// Прогнать матрицу: модели снаружи, задачи внутри, повторы подряд.
    pub async fn run_all(
        &self,
        models: &[ModelSpec],
        tasks: &[Task],
        progress: &mut dyn Write,
    ) -> Vec<RunRecord> {
        let mut records = Vec::new();

        for model in models {
            if self.warmup {
                let _ = writeln!(progress, "[{}] прогрев…", model.alias);
                if let Err(error) = self.warm_up(model).await {
                    let _ = writeln!(progress, "[{}] прогрев не удался: {error:#}", model.alias);
                }
            }

            for task in tasks {
                let repeats = task.repeats_with_override(self.repeats_override);
                for repeat in 1..=repeats {
                    let record = self.run_once(model, task, repeat).await;
                    let _ = writeln!(
                        progress,
                        "[{}] {} run {repeat}/{repeats} … {}",
                        model.alias,
                        task.id,
                        outcome_label(&record)
                    );
                    let _ = progress.flush();
                    records.push(record);
                }
            }
        }

        records
    }

    /// Прогрев: один вызов mark2 в пустой tmp-cwd, результат не сохраняется.
    async fn warm_up(&self, model: &ModelSpec) -> Result<()> {
        let sandbox = Sandbox::new()?;
        let stdout = sandbox.path().join("warmup.stdout");
        let stderr = sandbox.path().join("warmup.stderr");
        // Прогрев не зовёт тулы, политика ему безразлична — берём обычную.
        self.execute_mark2(
            model,
            WARMUP_PROMPT,
            TaskPolicy::default(),
            sandbox.cwd(),
            sandbox.home(),
            &stdout,
            &stderr,
            WARMUP_TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// Один прогон пары модель×задача.
    async fn run_once(&self, model: &ModelSpec, task: &Task, repeat: usize) -> RunRecord {
        let timeout = effective_timeout(task, model);
        let mut record = RunRecord {
            model: model.alias.clone(),
            model_id: model.model.clone(),
            task: task.id.clone(),
            repeat,
            started_at: Timestamp::now().to_string(),
            pass: false,
            exit_code: None,
            timed_out: false,
            wall_time_seconds: 0.0,
            timeout_seconds: timeout.as_secs_f64(),
            tokens: None,
            session: None,
            verify: None,
            answer_chars: 0,
            stdout_path: None,
            stderr_path: None,
            session_path: None,
            error: None,
        };

        if let Err(error) = self
            .fill_record(model, task, repeat, timeout, &mut record)
            .await
        {
            record.error = Some(format!("{error:#}"));
        }
        record
    }

    /// Тело прогона: подготовка, запуск, сбор метрик, проверка.
    async fn fill_record(
        &self,
        model: &ModelSpec,
        task: &Task,
        repeat: usize,
        timeout: Duration,
        record: &mut RunRecord,
    ) -> Result<()> {
        let sandbox = Sandbox::new()?;
        if let Some(setup) = task.setup_dir() {
            copy_dir_all(&setup, sandbox.cwd()).with_context(|| {
                format!(
                    "не удалось скопировать {} в {}",
                    setup.display(),
                    sandbox.cwd().display()
                )
            })?;
        }
        let project_config_path = sandbox.cwd().join(".mark2.toml");
        std::fs::write(
            &project_config_path,
            project_config(&model.model, task.context_limit, task.max_turns),
        )
        .with_context(|| format!("не удалось записать {}", project_config_path.display()))?;

        let logs_dir = self.run_dir.join("logs").join(&model.alias).join(&task.id);
        std::fs::create_dir_all(&logs_dir)
            .with_context(|| format!("не удалось создать {}", logs_dir.display()))?;
        let stdout_path = logs_dir.join(format!("{repeat}.stdout.txt"));
        let stderr_path = logs_dir.join(format!("{repeat}.stderr.txt"));

        let outcome = self
            .execute_mark2(
                model,
                &task.prompt,
                task.policy,
                sandbox.cwd(),
                sandbox.home(),
                &stdout_path,
                &stderr_path,
                timeout,
            )
            .await?;
        record.exit_code = outcome.exit_code;
        record.timed_out = outcome.timed_out;
        record.wall_time_seconds = outcome.wall.as_secs_f64();
        record.stdout_path = self.relative(&stdout_path);
        record.stderr_path = self.relative(&stderr_path);

        let stdout = read_lossy(&stdout_path);
        record.answer_chars = stdout.trim().chars().count();
        record.tokens = parse_tokens(&read_lossy(&stderr_path));

        if let Some(source) = find_session_file(sandbox.home()) {
            let dir = self
                .run_dir
                .join("sessions")
                .join(&model.alias)
                .join(&task.id);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("не удалось создать {}", dir.display()))?;
            let destination = dir.join(format!("{repeat}.jsonl"));
            std::fs::copy(&source, &destination).with_context(|| {
                format!(
                    "не удалось сохранить сессию {} → {}",
                    source.display(),
                    destination.display()
                )
            })?;
            record.session_path = self.relative(&destination);
            match read_session_metrics(&destination).await {
                Ok(metrics) => record.session = Some(metrics),
                Err(error) => tracing::warn!(%error, "не удалось разобрать сессию прогона"),
            }
        }

        let verify = run_verify(
            &task.verify_script(),
            sandbox.cwd(),
            &stdout_path,
            verify_timeout(task),
        )
        .await?;
        record.pass = verify.passed();
        record.verify = Some(verify);

        Ok(())
    }

    /// Запустить mark2 и дождаться его — или убить по таймауту.
    #[allow(clippy::too_many_arguments)]
    async fn execute_mark2(
        &self,
        model: &ModelSpec,
        prompt: &str,
        policy: TaskPolicy,
        cwd: &Path,
        home: &Path,
        stdout_path: &Path,
        stderr_path: &Path,
        timeout: Duration,
    ) -> Result<MarkOutcome> {
        let stdout = std::fs::File::create(stdout_path)
            .with_context(|| format!("не удалось создать {}", stdout_path.display()))?;
        let stderr = std::fs::File::create(stderr_path)
            .with_context(|| format!("не удалось создать {}", stderr_path.display()))?;

        let mut command = Command::new(&self.mark2);
        command
            .args(mark2_args(model, policy, cwd, prompt))
            .current_dir(cwd)
            // Изолируем и пользовательский конфиг (~/.config/mark2), и
            // директорию сессий (~/.mark2/sessions) одним махом.
            .env("HOME", home)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);

        let started = Instant::now();
        let mut child = command
            .spawn()
            .with_context(|| format!("не удалось запустить {}", self.mark2.display()))?;

        let outcome = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => {
                let status = status.context("не удалось дождаться mark2")?;
                MarkOutcome {
                    exit_code: status.code(),
                    timed_out: false,
                    wall: started.elapsed(),
                }
            }
            Err(_) => {
                // Убиваем сам mark2; его тул-подпроцессы (bash) живут в своих
                // группах и умирают вместе с ним по закрытию каналов.
                let _ = child.kill().await;
                MarkOutcome {
                    exit_code: None,
                    timed_out: true,
                    wall: started.elapsed(),
                }
            }
        };

        Ok(outcome)
    }

    /// Путь артефакта относительно директории результатов.
    fn relative(&self, path: &Path) -> Option<String> {
        path.strip_prefix(&self.run_dir)
            .ok()
            .map(|rest| rest.to_string_lossy().replace('\\', "/"))
    }
}

/// Итог запуска mark2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MarkOutcome {
    exit_code: Option<i32>,
    timed_out: bool,
    wall: Duration,
}

/// Как прогон выглядит в строке прогресса.
fn outcome_label(record: &RunRecord) -> String {
    if let Some(error) = &record.error {
        return format!("error: {error}");
    }
    let verdict = if record.timed_out {
        "timeout"
    } else if record.pass {
        "pass"
    } else {
        "fail"
    };
    format!("{verdict} {:.1}s", record.wall_time_seconds)
}

/// Бюджет времени прогона: таймаут задачи × множитель модели, не меньше секунды.
pub fn effective_timeout(task: &Task, model: &ModelSpec) -> Duration {
    let seconds = task.timeout_seconds as f64 * model.timeout_multiplier;
    Duration::from_secs_f64(seconds.max(1.0))
}

/// Бюджет `verify.sh` прогона: свой у задачи или общий [`VERIFY_TIMEOUT_SECONDS`].
pub fn verify_timeout(task: &Task) -> Duration {
    Duration::from_secs(
        task.verify_timeout_seconds
            .unwrap_or(VERIFY_TIMEOUT_SECONDS),
    )
}

/// Аргументы запуска mark2 для одного прогона.
///
/// Чистая функция: протокол запуска (print mode, политика задачи, куда ходить,
/// где работать) проверяется юнитом, без процесса.
pub fn mark2_args(
    model: &ModelSpec,
    policy: TaskPolicy,
    cwd: &Path,
    prompt: &str,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-p".into()];
    match policy {
        TaskPolicy::Yolo => args.push("--yolo".into()),
        // `--yolo` и `--policy` у mark2 взаимоисключающие — только одно из двух.
        TaskPolicy::ReadOnly => args.extend(["--policy".into(), "read-only".into()]),
    }
    args.extend([
        "--base-url".into(),
        model.base_url.clone().into(),
        "--model".into(),
        model.model.clone().into(),
        "-C".into(),
        cwd.as_os_str().to_owned(),
    ]);
    if let Some(variable) = &model.api_key_env {
        args.extend(["--api-key-env".into(), variable.into()]);
    }
    // Промпт — последним: это позиционный аргумент mark2.
    args.push(prompt.into());
    args
}

/// Содержимое `<cwd>/.mark2.toml` для прогона модели `model_id`.
///
/// Без настроек задачи — ровно [`PROJECT_CONFIG`], байт в байт как раньше.
/// `max_turns` дописывается строкой сразу за базой: база — это секция
/// `[agent]` целиком, так что строка попадает в неё же. `context_limit` —
/// отдельной секцией `[models."<model_id>"]`: ключ — точный id модели, а
/// точное совпадение в `mark2_core::config` важнее любого приблизительного,
/// так что лимит гарантированно достаётся этой модели.
pub fn project_config(
    model_id: &str,
    context_limit: Option<u64>,
    max_turns: Option<usize>,
) -> String {
    let mut text = PROJECT_CONFIG.to_string();
    if let Some(turns) = max_turns {
        text.push_str(&format!("max_turns = {turns}\n"));
    }
    if let Some(limit) = context_limit {
        text.push_str(&format!(
            "\n[models.{}]\ncontext_limit = {limit}\n",
            toml_basic_string(model_id)
        ));
    }
    text
}

/// Строка в кавычках по правилам TOML basic string: id моделей бывают с
/// двоеточиями и слешами, а в теории — и с кавычками.
fn toml_basic_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Запустить `verify.sh` в cwd прогона.
///
/// Контракт задачи: `sh verify.sh`, cwd — директория прогона (там лежат
/// артефакты), `MARK2_STDOUT` — путь к захваченному stdout (проверять текст
/// ответа или нет, решает сама задача).
async fn run_verify(
    script: &Path,
    cwd: &Path,
    stdout_path: &Path,
    timeout: Duration,
) -> Result<VerifyOutcome> {
    let mut command = Command::new("sh");
    command
        .arg(script)
        .current_dir(cwd)
        .env("MARK2_STDOUT", stdout_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = command
        .spawn()
        .with_context(|| format!("не удалось запустить sh {}", script.display()))?;

    // На таймауте future дропается вместе с `Child`, а `kill_on_drop` добивает
    // процесс: зависший verify не должен вешать весь бенч.
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => {
            let output = output.with_context(|| format!("сбой verify {}", script.display()))?;
            Ok(VerifyOutcome {
                exit_code: output.status.code(),
                timed_out: false,
                stdout: truncate(&String::from_utf8_lossy(&output.stdout)),
                stderr: truncate(&String::from_utf8_lossy(&output.stderr)),
            })
        }
        Err(_) => Ok(VerifyOutcome {
            exit_code: None,
            timed_out: true,
            stdout: String::new(),
            stderr: String::new(),
        }),
    }
}

/// Изолированная песочница прогона: `cwd` для агента и `home` под конфиг и
/// сессии. Удаляется вместе со структурой.
struct Sandbox {
    dir: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
}

impl Sandbox {
    fn new() -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("mark2-bench-")
            .tempdir()
            .context("не удалось создать временную директорию прогона")?;
        let cwd = dir.path().join("cwd");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&cwd)
            .with_context(|| format!("не удалось создать {}", cwd.display()))?;
        std::fs::create_dir_all(&home)
            .with_context(|| format!("не удалось создать {}", home.display()))?;
        Ok(Self { dir, cwd, home })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn home(&self) -> &Path {
        &self.home
    }
}

/// Рекурсивно скопировать `setup/` в cwd прогона (симлинки разыменовываются).
pub fn copy_dir_all(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)
        .with_context(|| format!("не удалось создать {}", destination.display()))?;
    for entry in std::fs::read_dir(source)
        .with_context(|| format!("не удалось прочитать {}", source.display()))?
    {
        let entry = entry.with_context(|| format!("не удалось обойти {}", source.display()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("не удалось скопировать {}", from.display()))?;
        }
    }
    Ok(())
}

/// Прочитать файл, не падая на не-UTF-8 (модели пишут в stdout что угодно).
fn read_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// Обрезать вывод verify до разумного размера.
fn truncate(text: &str) -> String {
    if text.chars().count() <= VERIFY_OUTPUT_LIMIT {
        return text.to_string();
    }
    let head: String = text.chars().take(VERIFY_OUTPUT_LIMIT).collect();
    format!("{head}…[обрезано]")
}

/// Записать `results.json` и `report.md` в директорию прогона.
///
/// Сырьё пишется первым: даже если сборка отчёта споткнётся, данные прогона —
/// а он мог занять час — уже на диске.
pub fn write_outputs(run_dir: &Path, results: &Results) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(run_dir)
        .with_context(|| format!("не удалось создать {}", run_dir.display()))?;

    let json_path = run_dir.join("results.json");
    let json =
        serde_json::to_string_pretty(results).context("не удалось сериализовать результаты")?;
    std::fs::write(&json_path, format!("{json}\n"))
        .with_context(|| format!("не удалось записать {}", json_path.display()))?;

    let report_path = run_dir.join("report.md");
    std::fs::write(&report_path, crate::report::render_report(results))
        .with_context(|| format!("не удалось записать {}", report_path.display()))?;

    Ok((json_path, report_path))
}

/// Имя директории результатов: `<YYYYMMDD-HHMMSS>-<label>`.
pub fn run_id(label: &str, now: &Zoned) -> String {
    format!(
        "{}-{}",
        now.strftime("%Y%m%d-%H%M%S"),
        sanitize_label(label)
    )
}

/// Метка в имени директории: только то, что безопасно в пути.
fn sanitize_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "bench".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::VerifyOutcome;
    use crate::task::tests::write_task;
    use crate::task::{SETUP_DIR, load_task};
    use jiff::civil::date;
    use tempfile::TempDir;

    fn model(multiplier: f64) -> ModelSpec {
        ModelSpec {
            alias: "qwen9".into(),
            model: "qwen3.5:9b".into(),
            base_url: "http://localhost:11434/v1".into(),
            timeout_multiplier: multiplier,
            api_key_env: None,
        }
    }

    #[test]
    fn timeout_is_task_budget_times_model_multiplier() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 120\n");
        let task = load_task(&dir).unwrap();

        assert_eq!(
            effective_timeout(&task, &model(1.0)),
            Duration::from_secs(120)
        );
        assert_eq!(
            effective_timeout(&task, &model(2.5)),
            Duration::from_secs(300)
        );
        assert!(effective_timeout(&task, &model(0.001)) >= Duration::from_secs(1));
    }

    fn args_as_strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn yolo_task_keeps_the_historical_argv() {
        let args = mark2_args(
            &model(1.0),
            TaskPolicy::Yolo,
            Path::new("/tmp/run/cwd"),
            "Создай файл",
        );
        assert_eq!(
            args_as_strings(&args),
            [
                "-p",
                "--yolo",
                "--base-url",
                "http://localhost:11434/v1",
                "--model",
                "qwen3.5:9b",
                "-C",
                "/tmp/run/cwd",
                "Создай файл",
            ]
        );
    }

    #[test]
    fn read_only_task_gets_policy_flag_and_no_yolo() {
        let mut spec = model(1.0);
        spec.api_key_env = Some("OLLAMA_API_KEY".into());
        let args = args_as_strings(&mark2_args(
            &spec,
            TaskPolicy::ReadOnly,
            Path::new("/tmp/run/cwd"),
            "Только посмотри",
        ));

        assert!(!args.iter().any(|arg| arg == "--yolo"), "{args:?}");
        let at = args.iter().position(|arg| arg == "--policy").unwrap();
        assert_eq!(args[at + 1], "read-only");
        assert_eq!(
            args,
            [
                "-p",
                "--policy",
                "read-only",
                "--base-url",
                "http://localhost:11434/v1",
                "--model",
                "qwen3.5:9b",
                "-C",
                "/tmp/run/cwd",
                "--api-key-env",
                "OLLAMA_API_KEY",
                "Только посмотри",
            ]
        );
    }

    #[test]
    fn project_config_without_task_settings_is_unchanged() {
        assert_eq!(project_config("qwen3.5:9b", None, None), PROJECT_CONFIG);
        assert_eq!(
            project_config("qwen3.5:9b", None, None),
            "[agent]\ntemperature = 0.0\n"
        );
    }

    /// `max_turns` обязан доехать до `[agent]` именно так, как его читает
    /// mark2 (CLI берёт `config.agent.max_turns` из проектного конфига), и не
    /// сломать соседние настройки.
    #[test]
    fn project_config_with_max_turns_reaches_the_agent_settings_of_mark2() {
        let text = project_config("flint:40b", None, Some(80));
        assert_eq!(text, "[agent]\ntemperature = 0.0\nmax_turns = 80\n");
        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.max_turns, Some(80));
        assert_eq!(config.agent.temperature, Some(0.0));

        // Вместе с context_limit: строка остаётся в [agent], лимит — у модели.
        let text = project_config("flint:40b", Some(8192), Some(80));
        assert_eq!(
            text,
            "[agent]\ntemperature = 0.0\nmax_turns = 80\n\n[models.\"flint:40b\"]\ncontext_limit = 8192\n"
        );
        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.max_turns, Some(80));
        assert_eq!(config.agent.temperature, Some(0.0));
        assert_eq!(config.resolve("flint:40b").context_limit, Some(8192));
    }

    #[test]
    fn verify_timeout_is_the_task_budget_or_the_default() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 60\n");
        let task = load_task(&dir).unwrap();
        assert_eq!(
            verify_timeout(&task),
            Duration::from_secs(VERIFY_TIMEOUT_SECONDS)
        );

        let dir = write_task(
            root.path(),
            "T17-x",
            "[task]\ntimeout_seconds = 60\nverify_timeout_seconds = 300\n",
        );
        let task = load_task(&dir).unwrap();
        assert_eq!(verify_timeout(&task), Duration::from_secs(300));
    }

    /// Лимит обязан доехать до настроек модели именно так, как их читает
    /// mark2: разбираем сгенерированный файл парсером ядра, а не глазами.
    #[test]
    fn project_config_with_a_limit_reaches_the_model_settings_of_mark2() {
        let text = project_config("qwen3.5:9b", Some(8192), None);
        assert_eq!(
            text,
            "[agent]\ntemperature = 0.0\n\n[models.\"qwen3.5:9b\"]\ncontext_limit = 8192\n"
        );
        assert!(text.starts_with(PROJECT_CONFIG));

        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.temperature, Some(0.0));
        assert_eq!(config.resolve("qwen3.5:9b").context_limit, Some(8192));
        // Другие модели лимит не задевает.
        assert_eq!(config.resolve("gpt-oss:120b-cloud").context_limit, None);

        // Экзотический id: кавычки, слеши, обратный слеш — ключ экранирован.
        let odd = r#"hf.co/org/"model"\q4:latest"#;
        let config =
            mark2_core::config::Config::parse(&project_config(odd, Some(4096), None), "x.toml")
                .unwrap();
        assert_eq!(config.resolve(odd).context_limit, Some(4096));
    }

    #[test]
    fn run_id_is_timestamp_plus_sanitized_label() {
        let now = date(2026, 8, 24).at(12, 0, 0, 0).in_tz("UTC").unwrap();
        assert_eq!(run_id("baseline", &now), "20260824-120000-baseline");
        assert_eq!(
            run_id("edit strategy/2", &now),
            "20260824-120000-edit-strategy-2"
        );
        assert_eq!(run_id("", &now), "20260824-120000-bench");
    }

    #[test]
    fn setup_is_copied_recursively() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 60\n");
        let setup = dir.join(SETUP_DIR);
        std::fs::create_dir_all(setup.join("src")).unwrap();
        std::fs::write(setup.join("src/main.py"), "print(1)\n").unwrap();
        std::fs::write(setup.join("README"), "hi").unwrap();

        let target = TempDir::new().unwrap();
        copy_dir_all(&setup, target.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.path().join("src/main.py")).unwrap(),
            "print(1)\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("README")).unwrap(),
            "hi"
        );
    }

    #[tokio::test]
    async fn verify_gets_the_run_cwd_and_the_captured_stdout() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("verify.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\npwd > pwd.txt\ngrep -q 'ответ модели' \"$MARK2_STDOUT\"\n",
        )
        .unwrap();
        let cwd = dir.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let stdout = dir.path().join("run.stdout");
        std::fs::write(&stdout, "ответ модели\n").unwrap();

        let outcome = run_verify(&script, &cwd, &stdout, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(outcome.passed(), "{outcome:?}");
        let seen = std::fs::read_to_string(cwd.join("pwd.txt")).unwrap();
        assert_eq!(
            std::fs::canonicalize(seen.trim()).unwrap(),
            std::fs::canonicalize(&cwd).unwrap()
        );

        std::fs::write(&stdout, "молчание\n").unwrap();
        let outcome = run_verify(&script, &cwd, &stdout, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(!outcome.passed());
        assert_eq!(outcome.exit_code, Some(1));
    }

    #[tokio::test]
    async fn hanging_verify_is_killed_by_its_own_timeout() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("verify.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        let stdout = dir.path().join("run.stdout");
        std::fs::write(&stdout, "").unwrap();

        let outcome = run_verify(&script, dir.path(), &stdout, Duration::from_millis(200))
            .await
            .unwrap();
        assert!(outcome.timed_out);
        assert!(!outcome.passed());
    }

    #[test]
    fn progress_label_names_the_verdict() {
        let mut record = RunRecord {
            model: "qwen9".into(),
            model_id: "qwen3.5:9b".into(),
            task: "T01".into(),
            repeat: 1,
            started_at: "2026-08-24T12:00:00Z".into(),
            pass: true,
            exit_code: Some(0),
            timed_out: false,
            wall_time_seconds: 12.34,
            timeout_seconds: 120.0,
            tokens: None,
            session: None,
            verify: Some(VerifyOutcome {
                exit_code: Some(0),
                timed_out: false,
                stdout: String::new(),
                stderr: String::new(),
            }),
            answer_chars: 10,
            stdout_path: None,
            stderr_path: None,
            session_path: None,
            error: None,
        };
        assert_eq!(outcome_label(&record), "pass 12.3s");

        record.pass = false;
        assert_eq!(outcome_label(&record), "fail 12.3s");

        record.timed_out = true;
        assert_eq!(outcome_label(&record), "timeout 12.3s");

        record.error = Some("не удалось запустить mark2".into());
        assert_eq!(outcome_label(&record), "error: не удалось запустить mark2");
    }

    #[test]
    fn truncation_marks_what_it_cut() {
        assert_eq!(truncate("коротко"), "коротко");
        let long = "я".repeat(VERIFY_OUTPUT_LIMIT + 10);
        let cut = truncate(&long);
        assert!(cut.ends_with("…[обрезано]"), "{}", &cut[cut.len() - 40..]);
    }
}
