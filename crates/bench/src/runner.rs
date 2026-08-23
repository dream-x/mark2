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
//! * **таймаут** = `timeout_seconds` задачи × множитель модели, по истечении —
//!   `kill` и пометка `timed_out` (прогон засчитывается как непройденный, но не
//!   вешает бенч).
//!
//! Ранер не интерпретирует ответ модели: приговор выносит `verify.sh`.

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
use crate::task::Task;

/// Промпт прогрева: тривиальный ответ без тулов, только чтобы поднять модель.
pub const WARMUP_PROMPT: &str = "Reply with exactly: ok";

/// Бюджет прогрева. Он не таймируется, но и висеть вечно не должен.
pub const WARMUP_TIMEOUT: Duration = Duration::from_secs(300);

/// Бюджет `verify.sh`: проверка локальная и должна быть мгновенной.
pub const VERIFY_TIMEOUT_SECONDS: u64 = 30;

/// Проектный конфиг, который кладётся в cwd каждого прогона.
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
    /// Собрать ранер. `run_dir` приводится к абсолютному пути: `verify.sh`
    /// запускается в чужой cwd и получает путь к stdout через окружение.
    pub fn new(mark2: impl AsRef<Path>, run_dir: impl AsRef<Path>) -> Result<Self> {
        let mark2 = std::path::absolute(mark2.as_ref())
            .with_context(|| format!("не удалось развернуть путь {}", mark2.as_ref().display()))?;
        let run_dir = std::path::absolute(run_dir.as_ref()).with_context(|| {
            format!("не удалось развернуть путь {}", run_dir.as_ref().display())
        })?;
        Ok(Self {
            mark2,
            run_dir,
            repeats_override: None,
            warmup: true,
        })
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
        self.execute_mark2(
            model,
            WARMUP_PROMPT,
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
        let project_config = sandbox.cwd().join(".mark2.toml");
        std::fs::write(&project_config, PROJECT_CONFIG)
            .with_context(|| format!("не удалось записать {}", project_config.display()))?;

        let logs_dir = self.run_dir.join("logs").join(&model.alias).join(&task.id);
        std::fs::create_dir_all(&logs_dir)
            .with_context(|| format!("не удалось создать {}", logs_dir.display()))?;
        let stdout_path = logs_dir.join(format!("{repeat}.stdout.txt"));
        let stderr_path = logs_dir.join(format!("{repeat}.stderr.txt"));

        let outcome = self
            .execute_mark2(
                model,
                &task.prompt,
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
            Duration::from_secs(VERIFY_TIMEOUT_SECONDS),
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
            .arg("-p")
            .arg("--yolo")
            .arg("--base-url")
            .arg(&model.base_url)
            .arg("--model")
            .arg(&model.model)
            .arg("-C")
            .arg(cwd);
        if let Some(variable) = &model.api_key_env {
            command.arg("--api-key-env").arg(variable);
        }
        command
            // Промпт — последним: это позиционный аргумент mark2.
            .arg(prompt)
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
