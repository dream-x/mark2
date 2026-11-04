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
//!   туда же — `edit_strategy` модели из матрицы (свойство модели, а не
//!   задачи);
//! * **абляция** — строки `--agent-override 'key = value'` ([`Runner::agent_overrides`])
//!   дописываются в `[agent]` каждого `.mark2.toml` после базы и `max_turns`;
//!   каждая строка и каждый итоговый конфиг проверяются парсером ядра до
//!   первого прогона ([`parse_agent_override`], [`check_project_configs`]);
//! * **окружение uitest** — задаче с `uitest_env = true` ранер до первой
//!   модели собирает `uitest/.venv` (`sh uitest/bootstrap.sh`, см.
//!   [`prepare_envs`]) и запускает mark2 с директорией песочницы, где лежит
//!   один `python3` (обёртка над интерпретатором venv, [`python_shim_script`]),
//!   первой в `PATH` ([`path_with_first`]): `python3` модели — тот же
//!   интерпретатор с aiohttp, которым `verify.sh` потом гоняет её сервер, а
//!   `pip` и прочие бинари venv модели не видны. Прогрев и остальные задачи
//!   получают `PATH` ранера как есть;
//! * **таймаут** = `timeout_seconds` задачи × множитель модели, по истечении —
//!   `kill` и пометка `timed_out` (прогон засчитывается как непройденный, но не
//!   вешает бенч); у `verify.sh` свой бюджет — [`verify_timeout`].
//!
//! Ранер не интерпретирует ответ модели: приговор выносит `verify.sh`.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use jiff::{Timestamp, Zoned};
use tokio::process::Command;

use mark2_core::config::{AgentSettings, Config};
use mark2_core::tools::EditStrategy;

use crate::metrics::{find_session_file, parse_tokens, read_session_metrics};
use crate::models::ModelSpec;
use crate::results::{Results, RunRecord, VerifyOutcome};
use crate::task::{Task, TaskPolicy, UITEST_BOOTSTRAP};
use crate::telemetry;

/// Промпт прогрева: тривиальный ответ без тулов, только чтобы поднять модель.
pub const WARMUP_PROMPT: &str = "Reply with exactly: ok";

/// Бюджет прогрева. Он не таймируется, но и висеть вечно не должен.
pub const WARMUP_TIMEOUT: Duration = Duration::from_secs(300);

/// Бюджет `verify.sh` по умолчанию: проверка локальная и должна быть
/// мгновенной. Задача, которой этого мало (сервер + браузер), задаёт свой
/// `verify_timeout_seconds`.
pub const VERIFY_TIMEOUT_SECONDS: u64 = 30;

/// Проектный конфиг, который кладётся в cwd каждого прогона (база; задача с
/// `max_turns` или `context_limit` и модель с `edit_strategy` дописывают к
/// нему — см. [`project_config`]).
pub const PROJECT_CONFIG: &str = "[agent]\ntemperature = 0.0\n";

/// Сколько символов вывода `verify.sh` сохраняем в `results.json`.
const VERIFY_OUTPUT_LIMIT: usize = 4000;

/// Бюджет `uitest/bootstrap.sh`. С готовым venv он мгновенный, полная сборка
/// (PyPI, возможно — скачивание Chromium) укладывается в минуты. Дольше —
/// значит, висит сеть, и узнать об этом надо до прогона, а не вместо него.
pub const UITEST_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(900);

/// Сколько последних строк вывода `bootstrap.sh` показываем в ошибке.
const BOOTSTRAP_TAIL_LINES: usize = 15;

/// Директория в песочнице прогона с единственным `python3` — её ранер ставит
/// первой в `PATH` mark2 задачи с `uitest_env`. Сосед `cwd` и `home`, а не
/// их часть: в артефакты модели и в её `$HOME` она не попадает.
const PYTHON_SHIM_DIR: &str = "python-bin";

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
    /// Строки `--agent-override` (уже проверенные [`parse_agent_override`]):
    /// дописываются в `[agent]` `.mark2.toml` каждого прогона.
    pub agent_overrides: Vec<String>,
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
            agent_overrides: Vec::new(),
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
        // Прогрев не зовёт тулы, политика ему безразлична — берём обычную;
        // PATH — ранера как есть.
        self.execute_mark2(
            model,
            WARMUP_PROMPT,
            TaskPolicy::default(),
            None,
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
            harness: None,
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
            project_config(
                &model.model,
                task.context_limit,
                task.max_turns,
                model.edit_strategy,
                &self.agent_overrides,
            ),
        )
        .with_context(|| format!("не удалось записать {}", project_config_path.display()))?;

        let logs_dir = self.run_dir.join("logs").join(&model.alias).join(&task.id);
        std::fs::create_dir_all(&logs_dir)
            .with_context(|| format!("не удалось создать {}", logs_dir.display()))?;
        let stdout_path = logs_dir.join(format!("{repeat}.stdout.txt"));
        let stderr_path = logs_dir.join(format!("{repeat}.stderr.txt"));

        let path_env = match task.uitest_dir() {
            Some(dir) => Some(uitest_path(
                &dir,
                &sandbox.path().join(PYTHON_SHIM_DIR),
                std::env::var_os("PATH").as_deref(),
            )?),
            None => None,
        };

        let outcome = self
            .execute_mark2(
                model,
                &task.prompt,
                task.policy,
                path_env.as_deref(),
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
        let stderr = read_lossy(&stderr_path);
        record.tokens = parse_tokens(&stderr);
        record.harness = Some(telemetry::parse(&stderr));

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
    ///
    /// `path_env` — свой `PATH` прогона (задача с `uitest_env`); `None` —
    /// mark2 наследует `PATH` ранера.
    #[allow(clippy::too_many_arguments)]
    async fn execute_mark2(
        &self,
        model: &ModelSpec,
        prompt: &str,
        policy: TaskPolicy,
        path_env: Option<&OsStr>,
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
        if let Some(path) = path_env {
            command.env("PATH", path);
        }

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

/// Подготовить окружения, которых требуют задачи прогона, — до первой модели.
///
/// Сейчас это uitest: для каждой задачи с `uitest_env` — [`ensure_uitest_env`]
/// её `uitest/`, по одному разу на директорию, сколько бы задач её ни
/// требовали. Сломанное окружение — ошибка всего бенча до прогрева: иначе
/// модель потратила бы весь таймаут задачи на `pip install`, а verify всё
/// равно упал бы на том же bootstrap.
pub async fn prepare_envs(tasks: &[Task], progress: &mut dyn Write) -> Result<()> {
    let mut ready: Vec<PathBuf> = Vec::new();
    for task in tasks {
        let Some(dir) = task.uitest_dir() else {
            continue;
        };
        if ready.contains(&dir) {
            continue;
        }
        let _ = writeln!(
            progress,
            "uitest: готовлю окружение {} (нужно {})…",
            dir.display(),
            task.id
        );
        let _ = progress.flush();
        ensure_uitest_env(&dir, UITEST_BOOTSTRAP_TIMEOUT)
            .await
            .with_context(|| format!("задача {} требует uitest_env", task.name()))?;
        ready.push(dir);
    }
    Ok(())
}

/// Интерпретатор venv окружения uitest: им verify гоняет сервер модели, его
/// же (через обёртку) модель видит как `python3`.
pub fn uitest_python(uitest_dir: &Path) -> PathBuf {
    uitest_dir.join(".venv").join("bin").join("python3")
}

/// Гарантировать готовый `uitest/.venv`: `sh <uitest>/bootstrap.sh`
/// (идемпотентный: с актуальным штампом — мгновенный выход) и проверка, что
/// `python3` на месте. Возвращает путь интерпретатора venv.
///
/// Провал — внятная ошибка с хвостом вывода bootstrap и командой для ручного
/// запуска, как у `FAIL [env]` в verify T17.
pub async fn ensure_uitest_env(uitest_dir: &Path, timeout: Duration) -> Result<PathBuf> {
    let script = uitest_dir.join(UITEST_BOOTSTRAP);
    let manual = format!("sh {} --force", script.display());

    let child = Command::new("sh")
        .arg(&script)
        .current_dir(uitest_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("не удалось запустить sh {}", script.display()))?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => output.with_context(|| format!("сбой sh {}", script.display()))?,
        Err(_) => bail!(
            "окружение uitest не готово: sh {} не уложился в {} с (висит сеть до PyPI \
             или CDN Playwright?); запусти руками: {manual}",
            script.display(),
            timeout.as_secs()
        ),
    };

    if !output.status.success() {
        let mut log = String::from_utf8_lossy(&output.stdout).into_owned();
        log.push_str(&String::from_utf8_lossy(&output.stderr));
        let code = output
            .status
            .code()
            .map_or_else(|| "убит сигналом".to_string(), |code| format!("код {code}"));
        bail!(
            "окружение uitest не готово: sh {} завершился с ошибкой ({code}; нужна сеть до \
             PyPI и, возможно, CDN Playwright); запусти руками, чтобы увидеть причину: \
             {manual}\n{}",
            script.display(),
            tail_lines(&log, BOOTSTRAP_TAIL_LINES)
        );
    }

    let python = uitest_python(uitest_dir);
    if !python.is_file() {
        bail!(
            "окружение uitest не готово: {} прошёл, но {} нет; пересобери: {manual}",
            script.display(),
            python.display()
        );
    }
    Ok(python)
}

/// `PATH` прогона задачи с `uitest_env`: `shim_dir` с единственным
/// `python3` (обёртка над интерпретатором venv) первым, дальше `current`.
///
/// В `PATH` модели попадает только интерпретатор — ни `pip`, ни прочие
/// бинари venv: окружение, которым verify потом гоняет её сервер, не должно
/// меняться от `pip install` модели. `shim_dir` — в песочнице прогона, своя у
/// каждого прогона: сломать её для следующих модель не может.
///
/// Venv к этому моменту собран [`prepare_envs`]; если его нет — ошибка до
/// запуска mark2, а не прогон, в котором `python3` модели без aiohttp.
fn uitest_path(uitest_dir: &Path, shim_dir: &Path, current: Option<&OsStr>) -> Result<OsString> {
    let python = uitest_python(uitest_dir);
    if !python.is_file() {
        bail!(
            "окружение uitest не собрано: нет {}; ранер собирает его до первой модели \
             (sh {})",
            python.display(),
            uitest_dir.join(UITEST_BOOTSTRAP).display()
        );
    }
    write_python_shim(shim_dir, &python)?;
    path_with_first(shim_dir, current)
}

/// Текст обёртки `python3`, которая запускает интерпретатор `python`.
///
/// Чистая функция. Именно `exec` по абсолютному пути, а не симлинк: venv
/// узнаёт себя по `pyvenv.cfg` рядом с *запущенным* путём, и симлинк из
/// чужой директории поднимает базовый интерпретатор без site-packages venv
/// (на бенч-машине — homebrew python3.14 с `sys.executable` мимо venv).
/// `exec` сохраняет и venv, и `sys.executable` (через него тест модели
/// поднимает `server.py`), и аргументы, stdin, код выхода.
pub fn python_shim_script(python: &Path) -> Result<String> {
    let Some(raw) = python.to_str() else {
        bail!(
            "путь интерпретатора uitest не UTF-8, обёртку не написать: {}",
            python.display()
        );
    };
    // Одинарные кавычки sh: внутри ничего не раскрывается, сама кавычка — '\''.
    let quoted = format!("'{}'", raw.replace('\'', "'\\''"));
    Ok(format!("#!/bin/sh\nexec {quoted} \"$@\"\n"))
}

/// Положить в `dir` исполняемую обёртку `python3` над `python`.
fn write_python_shim(dir: &Path, python: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("не удалось создать {}", dir.display()))?;
    let shim = dir.join("python3");
    std::fs::write(&shim, python_shim_script(python)?)
        .with_context(|| format!("не удалось записать {}", shim.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("не удалось сделать исполняемым {}", shim.display()))?;
    }
    Ok(())
}

/// `PATH`, в котором `first` стоит первым, а остальное — `current` как было.
///
/// Чистая функция. Повтор `first` дальше по списку выкидывается (venv мог
/// быть уже активирован у того, кто запускал бенч); пустые элементы
/// `current` («текущая директория») сохраняются, а вот пустой или
/// отсутствующий `PATH` целиком не превращается в лишний `:` в конце.
pub fn path_with_first(first: &Path, current: Option<&OsStr>) -> Result<OsString> {
    let mut entries = vec![first.to_path_buf()];
    if let Some(current) = current.filter(|current| !current.is_empty()) {
        entries.extend(std::env::split_paths(current).filter(|entry| entry != first));
    }
    std::env::join_paths(entries)
        .with_context(|| format!("{} нельзя поставить в PATH", first.display()))
}

/// Последние `lines` строк текста (вывод bootstrap бывает длинным).
fn tail_lines(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let start = all.len().saturating_sub(lines);
    let tail = all[start..].join("\n");
    if tail.is_empty() {
        "(вывода нет)".to_string()
    } else {
        tail
    }
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
/// Без настроек задачи, модели и overrides — ровно [`PROJECT_CONFIG`], байт в
/// байт как раньше. `max_turns` и затем строки `agent_overrides` (как есть, по
/// одной на строку) дописываются сразу за базой: база — это секция `[agent]`
/// целиком, так что они попадают в неё же. `context_limit` задачи и
/// `edit_strategy` модели — в одну секцию `[models."<model_id>"]` (TOML не
/// терпит повторного заголовка): ключ — точный id модели, а точное совпадение
/// в `mark2_core::config` важнее любого приблизительного, так что настройки
/// гарантированно достаются этой модели.
pub fn project_config(
    model_id: &str,
    context_limit: Option<u64>,
    max_turns: Option<usize>,
    edit_strategy: Option<EditStrategy>,
    agent_overrides: &[String],
) -> String {
    let mut text = PROJECT_CONFIG.to_string();
    if let Some(turns) = max_turns {
        text.push_str(&format!("max_turns = {turns}\n"));
    }
    for line in agent_overrides {
        text.push_str(line);
        text.push('\n');
    }

    let mut model_settings = String::new();
    if let Some(limit) = context_limit {
        model_settings.push_str(&format!("context_limit = {limit}\n"));
    }
    if let Some(strategy) = edit_strategy {
        model_settings.push_str(&format!(
            "edit_strategy = {}\n",
            toml_basic_string(strategy.as_str())
        ));
    }
    if !model_settings.is_empty() {
        text.push_str(&format!(
            "\n[models.{}]\n{model_settings}",
            toml_basic_string(model_id)
        ));
    }
    text
}

/// Разобрать одно значение `--agent-override`: ровно одна пара `key = value`
/// секции `[agent]`, которую понимает mark2. Возвращает строку для
/// `.mark2.toml` (без пробелов по краям).
///
/// Проверок две, обе — до прогонов:
///
/// * это одна TOML-пара, а не заголовок секции, не dotted-ключ и не две строки:
///   иначе override мог бы писать мимо `[agent]`;
/// * `mark2_core::config` её понимает: значение подходит по типу, а ключ —
///   настройка `[agent]`. Неизвестный ключ (опечатка) ядро молча
///   проигнорировало бы — толерантный конфиг, — и абляция не выключила бы
///   ничего; поэтому «разобралось, но ничего не поменяло» — тоже ошибка.
pub fn parse_agent_override(raw: &str) -> Result<String> {
    let line = raw.trim();
    let table: toml::Table = toml::from_str(line).with_context(|| {
        format!("--agent-override {raw:?}: ожидается одна строка TOML `key = value`")
    })?;
    let Some((key, value)) = table.iter().next() else {
        bail!("--agent-override {raw:?}: пусто, ожидается `key = value`");
    };
    if table.len() != 1 || value.is_table() {
        bail!(
            "--agent-override {raw:?}: ожидается ровно одна пара `key = value` секции [agent] \
             (без заголовков секций и dotted-ключей)"
        );
    }

    let config = Config::parse(&format!("[agent]\n{line}\n"), "--agent-override")
        .map_err(|error| anyhow::anyhow!("--agent-override {raw:?}: {}", error.chain()))?;
    if config.agent == AgentSettings::default() {
        bail!(
            "--agent-override {raw:?}: `{key}` — не настройка [agent] mark2, ядро бы её \
             проигнорировало"
        );
    }
    Ok(line.to_string())
}

/// Проверить до первого прогона, что `.mark2.toml` каждой пары модель × задача
/// разбирается парсером ядра. Отдельные строки override уже валидны, но могут
/// столкнуться с тем, что пишет сам ранер: `temperature` базы или `max_turns`
/// задачи — дубликат ключа в TOML. Такое обязано всплыть сразу, с текстом
/// конфига и строкой-виновницей, а не провалом каждого прогона задачи.
pub fn check_project_configs(
    models: &[ModelSpec],
    tasks: &[Task],
    agent_overrides: &[String],
) -> Result<()> {
    for model in models {
        for task in tasks {
            let text = project_config(
                &model.model,
                task.context_limit,
                task.max_turns,
                model.edit_strategy,
                agent_overrides,
            );
            if let Err(error) = Config::parse(&text, ".mark2.toml") {
                bail!(
                    ".mark2.toml для {} × {} не разбирается mark2 (--agent-override: {}): {}\n{text}",
                    model.alias,
                    task.id,
                    agent_overrides.join("; "),
                    error.chain()
                );
            }
        }
    }
    Ok(())
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
    use crate::task::{SETUP_DIR, UITEST_DIR, load_task};
    use jiff::civil::date;
    use tempfile::TempDir;

    fn model(multiplier: f64) -> ModelSpec {
        ModelSpec {
            alias: "qwen9".into(),
            model: "qwen3.5:9b".into(),
            base_url: "http://localhost:11434/v1".into(),
            timeout_multiplier: multiplier,
            api_key_env: None,
            edit_strategy: None,
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
        assert_eq!(
            project_config("qwen3.5:9b", None, None, None, &[]),
            PROJECT_CONFIG
        );
        assert_eq!(
            project_config("qwen3.5:9b", None, None, None, &[]),
            "[agent]\ntemperature = 0.0\n"
        );
    }

    /// `max_turns` обязан доехать до `[agent]` именно так, как его читает
    /// mark2 (CLI берёт `config.agent.max_turns` из проектного конфига), и не
    /// сломать соседние настройки.
    #[test]
    fn project_config_with_max_turns_reaches_the_agent_settings_of_mark2() {
        let text = project_config("flint:40b", None, Some(80), None, &[]);
        assert_eq!(text, "[agent]\ntemperature = 0.0\nmax_turns = 80\n");
        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.max_turns, Some(80));
        assert_eq!(config.agent.temperature, Some(0.0));

        // Вместе с context_limit: строка остаётся в [agent], лимит — у модели.
        let text = project_config("flint:40b", Some(8192), Some(80), None, &[]);
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
    fn project_config_with_an_edit_strategy_reaches_the_model_settings_of_mark2() {
        // Только стратегия модели: секция модели появляется ради неё одной.
        let text = project_config("flint:40b", None, None, Some(EditStrategy::WholeFile), &[]);
        assert_eq!(
            text,
            "[agent]\ntemperature = 0.0\n\n[models.\"flint:40b\"]\nedit_strategy = \"whole_file\"\n"
        );
        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.temperature, Some(0.0));
        let settings = config.resolve("flint:40b");
        assert_eq!(settings.edit_strategy, EditStrategy::WholeFile);
        assert_eq!(settings.context_limit, None);
        // Другие модели стратегию не наследуют.
        assert_eq!(
            config.resolve("qwen3.5:9b").edit_strategy,
            EditStrategy::SearchReplace
        );

        // Всё сразу: заголовок модели один, обе настройки под ним, max_turns —
        // по-прежнему в [agent].
        let text = project_config(
            "flint:40b",
            Some(8192),
            Some(80),
            Some(EditStrategy::WholeFile),
            &[],
        );
        assert_eq!(
            text,
            "[agent]\ntemperature = 0.0\nmax_turns = 80\n\n[models.\"flint:40b\"]\ncontext_limit = 8192\nedit_strategy = \"whole_file\"\n"
        );
        assert_eq!(text.matches("[models.").count(), 1, "{text}");
        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.max_turns, Some(80));
        let settings = config.resolve("flint:40b");
        assert_eq!(settings.edit_strategy, EditStrategy::WholeFile);
        assert_eq!(settings.context_limit, Some(8192));

        // Явный дефолт тоже доезжает как есть.
        let text = project_config("m", None, None, Some(EditStrategy::SearchReplace), &[]);
        let config = mark2_core::config::Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(
            config.model_config("m").unwrap().edit_strategy,
            Some(EditStrategy::SearchReplace)
        );
    }

    /// Абляция: строки `--agent-override` встают в [agent] после базы и
    /// `max_turns`, до секции модели, — и ядро читает их как настройки агента.
    #[test]
    fn agent_overrides_land_in_the_agent_section_and_reach_mark2() {
        let overrides = [
            "arg_coercion = false".to_string(),
            "write_churn = false".to_string(),
        ];
        let text = project_config(
            "flint:40b",
            Some(8192),
            Some(80),
            Some(EditStrategy::WholeFile),
            &overrides,
        );
        assert_eq!(
            text,
            "[agent]\ntemperature = 0.0\nmax_turns = 80\narg_coercion = false\nwrite_churn = false\n\n[models.\"flint:40b\"]\ncontext_limit = 8192\nedit_strategy = \"whole_file\"\n"
        );
        let config = Config::parse(&text, ".mark2.toml").unwrap();
        assert_eq!(config.agent.temperature, Some(0.0));
        assert_eq!(config.agent.max_turns, Some(80));
        assert_eq!(config.agent.arg_coercion, Some(false));
        assert_eq!(config.agent.write_churn, Some(0));
        let settings = config.resolve("flint:40b");
        assert_eq!(settings.context_limit, Some(8192));
        assert_eq!(settings.edit_strategy, EditStrategy::WholeFile);

        // Без настроек задачи и модели — сразу за базой.
        assert_eq!(
            project_config("m", None, None, None, &overrides[..1]),
            "[agent]\ntemperature = 0.0\narg_coercion = false\n"
        );
    }

    #[test]
    fn agent_override_is_one_known_agent_setting() {
        assert_eq!(
            parse_agent_override("  arg_coercion = false ").unwrap(),
            "arg_coercion = false"
        );
        assert_eq!(
            parse_agent_override("write_churn = 5").unwrap(),
            "write_churn = 5"
        );
        assert_eq!(
            parse_agent_override("loop_detection = 0").unwrap(),
            "loop_detection = 0"
        );

        for (raw, expected) in [
            ("arg_coercion", "одна строка TOML"),
            ("", "пусто"),
            (
                "arg_coercion = false\nwrite_churn = false",
                "ровно одна пара",
            ),
            ("[models.x]", "ровно одна пара"),
            ("models.x.context_limit = 1", "ровно одна пара"),
            // Опечатка: ядро её бы молча проигнорировало — абляция в никуда.
            (
                "arg_coerciom = false",
                "`arg_coerciom` — не настройка [agent]",
            ),
            ("arg_coercion = \"no\"", "boolean"),
            ("write_churn = -1", "true, false или порог"),
        ] {
            let text = format!("{:#}", parse_agent_override(raw).unwrap_err());
            assert!(text.contains(expected), "{raw:?}: {text}");
            assert!(
                text.contains(&format!("--agent-override {raw:?}")),
                "строка-виновница: {text}"
            );
        }
    }

    /// Каждая пара модель × задача проверяется до прогонов: override, который
    /// сам по себе валиден, может столкнуться с ключом, который пишет ранер.
    #[test]
    fn project_configs_are_checked_for_every_model_and_task() {
        let root = TempDir::new().unwrap();
        let plain = load_task(&write_task(
            root.path(),
            "T01-x",
            "[task]\ntimeout_seconds = 60\n",
        ))
        .unwrap();
        let capped = load_task(&write_task(
            root.path(),
            "T17-x",
            "[task]\ntimeout_seconds = 60\nmax_turns = 80\n",
        ))
        .unwrap();
        let mut flint = model(1.0);
        flint.alias = "flint40".into();
        flint.model = "flint:40b".into();
        flint.edit_strategy = Some(EditStrategy::WholeFile);
        let models = [model(1.0), flint];
        let tasks = [plain, capped];

        check_project_configs(&models, &tasks, &[]).unwrap();
        check_project_configs(
            &models,
            &tasks,
            &["arg_coercion = false".into(), "write_churn = false".into()],
        )
        .unwrap();

        // Дубликат с max_turns задачи — только у T17, но всплывает сразу.
        let text = format!(
            "{:#}",
            check_project_configs(&models, &tasks, &["max_turns = 10".into()]).unwrap_err()
        );
        assert!(text.contains("qwen9 × T17"), "{text}");
        assert!(text.contains("max_turns = 10"), "{text}");
        assert!(text.contains("duplicate key"), "{text}");

        // Дубликат с базой — на первой же паре.
        let text = format!(
            "{:#}",
            check_project_configs(&models, &tasks, &["temperature = 0.7".into()]).unwrap_err()
        );
        assert!(text.contains("qwen9 × T01"), "{text}");
        assert!(text.contains("temperature = 0.7"), "{text}");
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
        let text = project_config("qwen3.5:9b", Some(8192), None, None, &[]);
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
        let config = mark2_core::config::Config::parse(
            &project_config(odd, Some(4096), None, None, &[]),
            "x.toml",
        )
        .unwrap();
        assert_eq!(config.resolve(odd).context_limit, Some(4096));
    }

    fn path_string(first: &str, current: Option<&str>) -> String {
        path_with_first(Path::new(first), current.map(OsStr::new))
            .unwrap()
            .into_string()
            .unwrap()
    }

    #[test]
    fn path_with_first_puts_the_dir_first_and_keeps_the_rest() {
        assert_eq!(
            path_string("/s/python-bin", Some("/usr/local/bin:/usr/bin:/bin")),
            "/s/python-bin:/usr/local/bin:/usr/bin:/bin"
        );
        // Уже был в PATH — не дублируется, но первый.
        assert_eq!(
            path_string("/s/python-bin", Some("/usr/bin:/s/python-bin/:/bin")),
            "/s/python-bin:/usr/bin:/bin"
        );
        // Пустой элемент посреди PATH — чужая семантика, её не трогаем.
        assert_eq!(
            path_string("/v/bin", Some("/usr/bin::/bin")),
            "/v/bin:/usr/bin::/bin"
        );
        // Нет PATH вовсе или он пуст — без хвостового `:` (он значил бы cwd).
        assert_eq!(path_string("/v/bin", None), "/v/bin");
        assert_eq!(path_string("/v/bin", Some("")), "/v/bin");
        // Двоеточие в пути в PATH не выразить — ошибка с путём, а не мусор.
        let text = format!(
            "{:#}",
            path_with_first(Path::new("/a:b/bin"), Some(OsStr::new("/bin"))).unwrap_err()
        );
        assert!(text.contains("/a:b/bin"), "{text}");
    }

    /// Фейковый `uitest/bootstrap.sh` с телом `body`; `uitest/` — в `root`.
    fn fake_uitest(root: &Path, body: &str) -> PathBuf {
        let uitest = root.join(UITEST_DIR);
        std::fs::create_dir_all(&uitest).unwrap();
        std::fs::write(
            uitest.join(UITEST_BOOTSTRAP),
            format!("#!/bin/sh\nset -eu\n{body}"),
        )
        .unwrap();
        uitest
    }

    /// Bootstrap, который собирает «venv» с `python3` и `pip` и считает свои
    /// вызовы. Фейковый `python3` печатает свои аргументы по одному в строке
    /// и выходит с кодом 7 — видно, что обёртка ничего не теряет.
    const GOOD_BOOTSTRAP: &str = "echo run >> calls.log\nmkdir -p .venv/bin\n\
         printf '#!/bin/sh\\nfor a in \"$@\"; do echo \"[$a]\"; done\\nexit 7\\n' > .venv/bin/python3\n\
         printf '#!/bin/sh\\n' > .venv/bin/pip\n\
         chmod +x .venv/bin/python3 .venv/bin/pip\n";

    #[tokio::test]
    async fn bootstrap_builds_the_venv_and_returns_its_python() {
        let root = TempDir::new().unwrap();
        let uitest = fake_uitest(root.path(), GOOD_BOOTSTRAP);

        let python = ensure_uitest_env(&uitest, Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(python, uitest.join(".venv").join("bin").join("python3"));
        assert!(python.is_file());
    }

    #[test]
    fn python_shim_execs_the_venv_python_by_its_absolute_quoted_path() {
        assert_eq!(
            python_shim_script(Path::new("/b/uitest/.venv/bin/python3")).unwrap(),
            "#!/bin/sh\nexec '/b/uitest/.venv/bin/python3' \"$@\"\n"
        );
        // Пробел, кавычка и `$` в пути: sh не должен их ни делить, ни раскрывать.
        assert_eq!(
            python_shim_script(Path::new("/it's $HOME/python3")).unwrap(),
            "#!/bin/sh\nexec '/it'\\''s $HOME/python3' \"$@\"\n"
        );
    }

    /// В `PATH` модели — директория ровно с одним `python3`, который и правда
    /// запускает интерпретатор venv: аргументы и код выхода доходят как есть,
    /// `pip` venv не виден.
    #[tokio::test]
    async fn uitest_path_puts_a_dir_with_only_python3_first() {
        // Путь с пробелом и кавычкой — обёртка обязана его пережить.
        let root = TempDir::new().unwrap();
        let odd = root.path().join("it's a bench");
        let uitest = fake_uitest(&odd, GOOD_BOOTSTRAP);
        ensure_uitest_env(&uitest, Duration::from_secs(30))
            .await
            .unwrap();

        let shim = root.path().join("sandbox").join(PYTHON_SHIM_DIR);
        let path = uitest_path(&uitest, &shim, Some(OsStr::new("/usr/bin:/bin"))).unwrap();
        assert_eq!(
            path,
            OsString::from(format!("{}:/usr/bin:/bin", shim.display()))
        );
        let listed: Vec<String> = std::fs::read_dir(&shim)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(listed, ["python3"], "ни pip, ни прочих бинарей venv");

        let output = std::process::Command::new("python3")
            .args(["a", "b c", ""])
            .env("PATH", &path)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "[a]\n[b c]\n[]\n");
        assert_eq!(output.status.code(), Some(7));

        // Повторный прогон в той же директории переписывает обёртку, не падает.
        uitest_path(&uitest, &shim, None).unwrap();
    }

    #[tokio::test]
    async fn failing_bootstrap_explains_itself_with_the_tail_and_the_manual_command() {
        let root = TempDir::new().unwrap();
        let uitest = fake_uitest(
            root.path(),
            "for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do echo \"noise $i\"; done\n\
             echo 'ERROR: Could not find a version that satisfies aiohttp' >&2\nexit 3\n",
        );

        let text = format!(
            "{:#}",
            ensure_uitest_env(&uitest, Duration::from_secs(30))
                .await
                .unwrap_err()
        );
        let script = uitest.join(UITEST_BOOTSTRAP).display().to_string();
        assert!(text.contains("окружение uitest не готово"), "{text}");
        assert!(text.contains("код 3"), "{text}");
        assert!(text.contains(&format!("sh {script} --force")), "{text}");
        assert!(text.contains("Could not find a version"), "{text}");
        // Хвост, а не весь вывод.
        assert!(text.contains("noise 20"), "{text}");
        assert!(!text.contains("noise 1\n"), "{text}");
    }

    #[tokio::test]
    async fn bootstrap_that_leaves_no_python_or_hangs_is_an_error() {
        let root = TempDir::new().unwrap();
        let uitest = fake_uitest(root.path(), "exit 0\n");
        let text = format!(
            "{:#}",
            ensure_uitest_env(&uitest, Duration::from_secs(30))
                .await
                .unwrap_err()
        );
        assert!(text.contains(".venv/bin/python3"), "{text}");
        assert!(text.contains("--force"), "{text}");

        // Без venv PATH прогону не собрать: ошибка до запуска mark2.
        let shim = root.path().join(PYTHON_SHIM_DIR);
        let text = format!("{:#}", uitest_path(&uitest, &shim, None).unwrap_err());
        assert!(!shim.exists(), "обёртки над несуществующим python3 нет");
        assert!(text.contains("не собрано"), "{text}");
        assert!(text.contains(".venv/bin/python3"), "{text}");

        let uitest = fake_uitest(root.path(), "sleep 30\n");
        let started = Instant::now();
        let text = format!(
            "{:#}",
            ensure_uitest_env(&uitest, Duration::from_millis(300))
                .await
                .unwrap_err()
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "не дождались бюджета"
        );
        assert!(text.contains("не уложился"), "{text}");
        assert!(text.contains("--force"), "{text}");
    }

    /// Один bootstrap на директорию, сколько бы задач её ни требовали; задачи
    /// без поля окружение не трогают.
    #[tokio::test]
    async fn prepare_bootstraps_each_uitest_dir_once_and_only_when_asked() {
        let root = TempDir::new().unwrap();
        let uitest = fake_uitest(root.path(), GOOD_BOOTSTRAP);
        let tasks_dir = root.path().join("tasks");
        let plain = write_task(&tasks_dir, "T01-plain", "[task]\ntimeout_seconds = 60\n");
        let tasks: Vec<Task> = [plain.clone()]
            .into_iter()
            .chain(["T17-a", "T18-b"].map(|name| {
                write_task(
                    &tasks_dir,
                    name,
                    "[task]\ntimeout_seconds = 60\nuitest_env = true\n",
                )
            }))
            .map(|dir| load_task(&dir).unwrap())
            .collect();

        let mut progress = Vec::new();
        prepare_envs(&tasks[..1], &mut progress).await.unwrap();
        assert!(
            !uitest.join("calls.log").exists(),
            "без uitest_env — ни одного вызова"
        );
        assert!(progress.is_empty());

        prepare_envs(&tasks, &mut progress).await.unwrap();
        let calls = std::fs::read_to_string(uitest.join("calls.log")).unwrap();
        assert_eq!(calls, "run\n");
        let progress = String::from_utf8(progress).unwrap();
        assert!(progress.contains("uitest: готовлю окружение"), "{progress}");
        assert!(progress.contains("T17"), "{progress}");

        // Провал называет задачу и поле: ясно, кому и зачем понадобилась сеть.
        fake_uitest(
            root.path(),
            "echo 'pip: network is unreachable' >&2\nexit 1\n",
        );
        let text = format!(
            "{:#}",
            prepare_envs(&tasks, &mut Vec::new()).await.unwrap_err()
        );
        assert!(text.contains("T17-a"), "{text}");
        assert!(text.contains("uitest_env"), "{text}");
        assert!(text.contains("network is unreachable"), "{text}");
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
        // Dotfile — так в setup/ задач T18/T19 лежит память агента.
        std::fs::write(setup.join(".mark2.memory.md"), "# Memory\n").unwrap();

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
        assert_eq!(
            std::fs::read_to_string(target.path().join(".mark2.memory.md")).unwrap(),
            "# Memory\n"
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
            harness: None,
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
