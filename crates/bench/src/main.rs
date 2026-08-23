//! `mark2-bench` — прогон бенчмарка: матрица моделей × агентные задачи.
//!
//! Тут только сборка прогона: разобрать флаги, загрузить матрицу и задачи,
//! создать директорию результатов, отдать всё [`Runner`] и записать
//! `results.json` + `report.md`. Логика протокола — в
//! [`mark2_bench::runner`], оценка — в `verify.sh` самих задач.
//!
//! Прогресс идёт в stderr по ходу дела (бенч живёт десятки минут — молчащий
//! процесс неотличим от зависшего), итоговые пути — в stdout.
//!
//! Коды возврата: `0` — прогон состоялся (провалы задач это данные, а не сбой
//! бенча), `1` — не удалось даже начать или записать результаты.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use jiff::{Timestamp, Zoned};
use mark2_bench::models::{DEFAULT_MODELS_FILE, load_models, select_models};
use mark2_bench::results::{Results, RunConfig, SCHEMA_VERSION, TaskRecord};
use mark2_bench::runner::{
    PROJECT_CONFIG, Runner, VERIFY_TIMEOUT_SECONDS, WARMUP_PROMPT, run_id, write_outputs,
};
use mark2_bench::task::{DEFAULT_TASKS_DIR, discover_tasks, select_tasks};

/// Путь к бинарю mark2 по умолчанию.
const DEFAULT_MARK2: &str = "target/release/mark2";

/// Директория результатов по умолчанию.
const DEFAULT_RESULTS_DIR: &str = "bench/results";

/// Метка прогона по умолчанию.
const DEFAULT_LABEL: &str = "bench";

/// Ранер бенчмарка mark2.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "mark2-bench",
    version,
    about = "Прогон бенчмарка mark2: модели × агентные задачи, программная верификация",
    after_help = "Протокол и матрица задач — bench/STRATEGY.md.\nРезультаты: \
                  bench/results/<YYYYMMDD-HHMMSS>-<label>/{results.json,report.md,sessions/}."
)]
struct Args {
    /// Только эти модели (алиасы из models.toml через запятую).
    #[arg(long, value_name = "ALIASES", value_delimiter = ',')]
    models: Vec<String>,

    /// Только эти задачи (ID через запятую, например T01,T03).
    #[arg(long, value_name = "IDS", value_delimiter = ',')]
    tasks: Vec<String>,

    /// Повторов на пару модель×задача (перекрывает task.toml).
    #[arg(long, value_name = "N")]
    repeats: Option<usize>,

    /// Метка прогона в имени директории результатов.
    #[arg(long, value_name = "STR", default_value = DEFAULT_LABEL)]
    label: String,

    /// Путь к бинарю mark2.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_MARK2)]
    mark2: PathBuf,

    /// Директория с задачами.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_TASKS_DIR)]
    tasks_dir: PathBuf,

    /// Файл матрицы моделей.
    #[arg(long, value_name = "FILE", default_value = DEFAULT_MODELS_FILE)]
    models_file: PathBuf,

    /// Директория, в которой создаётся папка прогона.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_RESULTS_DIR)]
    out: PathBuf,

    /// Не прогревать модель перед задачами (для отладки самого ранера).
    #[arg(long)]
    no_warmup: bool,
}

fn main() -> ExitCode {
    init_tracing();
    let args = Args::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: не удалось поднять tokio: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Логи — в stderr и только по запросу: `RUST_LOG=mark2_bench=debug`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .try_init();
}

async fn run(args: Args) -> Result<()> {
    if args.repeats == Some(0) {
        bail!("--repeats должен быть больше нуля");
    }
    if !args.mark2.is_file() {
        bail!(
            "не найден бинарь mark2: {}; собери `cargo build --release` или укажи --mark2 <path>",
            args.mark2.display()
        );
    }

    let models = select_models(load_models(&args.models_file)?, &args.models)?;
    let tasks = select_tasks(discover_tasks(&args.tasks_dir)?, &args.tasks)?;

    let run_id = run_id(&args.label, &Zoned::now());
    let run_dir = args.out.join(&run_id);
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("не удалось создать {}", run_dir.display()))?;

    let mut runner = Runner::new(&args.mark2, &run_dir)?;
    runner.repeats_override = args.repeats;
    runner.warmup = !args.no_warmup;

    let config = RunConfig {
        bench_version: mark2_bench::VERSION.to_string(),
        mark2: runner.mark2.display().to_string(),
        tasks_dir: args.tasks_dir.display().to_string(),
        models_file: args.models_file.display().to_string(),
        repeats_override: args.repeats,
        verify_timeout_seconds: VERIFY_TIMEOUT_SECONDS,
        warmup_prompt: WARMUP_PROMPT.to_string(),
        project_config: PROJECT_CONFIG.to_string(),
        models: models.clone(),
        tasks: tasks
            .iter()
            .map(|task| TaskRecord {
                id: task.id.clone(),
                slug: task.slug.clone(),
                dir: task.dir.display().to_string(),
                timeout_seconds: task.timeout_seconds,
                repeats: task.repeats_with_override(args.repeats),
                tags: task.tags.clone(),
                prompt: task.prompt.clone(),
            })
            .collect(),
    };

    let mut progress = std::io::stderr();
    let _ = writeln!(
        &mut progress,
        "bench {run_id}: {} моделей × {} задач → {}",
        models.len(),
        tasks.len(),
        run_dir.display()
    );

    let started_at = Timestamp::now().to_string();
    let runs = runner.run_all(&models, &tasks, &mut progress).await;
    let finished_at = Timestamp::now().to_string();

    let results = Results {
        schema_version: SCHEMA_VERSION,
        label: args.label.clone(),
        run_id,
        started_at,
        finished_at,
        config,
        runs,
    };

    let (json_path, report_path) = write_outputs(&run_dir, &results)?;
    let passed = results.runs.iter().filter(|run| run.pass).count();
    let _ = writeln!(
        &mut progress,
        "готово: {passed}/{} прогонов прошло",
        results.runs.len()
    );
    println!("{}", json_path.display());
    println!("{}", report_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use mark2_bench::report::render_report;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("аргументы должны разбираться")
    }

    #[test]
    fn defaults_point_at_the_repository_layout() {
        let args = parse(&["mark2-bench"]);
        assert!(args.models.is_empty(), "по умолчанию гоняем всю матрицу");
        assert!(args.tasks.is_empty());
        assert_eq!(args.repeats, None);
        assert_eq!(args.label, "bench");
        assert_eq!(args.mark2, PathBuf::from("target/release/mark2"));
        assert_eq!(args.tasks_dir, PathBuf::from("bench/tasks"));
        assert_eq!(args.models_file, PathBuf::from("bench/models.toml"));
        assert_eq!(args.out, PathBuf::from("bench/results"));
        assert!(!args.no_warmup);
    }

    #[test]
    fn lists_are_comma_separated() {
        let args = parse(&[
            "mark2-bench",
            "--models",
            "qwen9,oss120",
            "--tasks",
            "T01,T03",
            "--repeats",
            "1",
            "--label",
            "edit-strategy",
            "--mark2",
            "/tmp/mark2",
            "--tasks-dir",
            "/tmp/tasks",
            "--models-file",
            "/tmp/models.toml",
            "--out",
            "/tmp/out",
            "--no-warmup",
        ]);
        assert_eq!(args.models, ["qwen9", "oss120"]);
        assert_eq!(args.tasks, ["T01", "T03"]);
        assert_eq!(args.repeats, Some(1));
        assert_eq!(args.label, "edit-strategy");
        assert_eq!(args.mark2, PathBuf::from("/tmp/mark2"));
        assert_eq!(args.tasks_dir, PathBuf::from("/tmp/tasks"));
        assert_eq!(args.models_file, PathBuf::from("/tmp/models.toml"));
        assert_eq!(args.out, PathBuf::from("/tmp/out"));
        assert!(args.no_warmup);
    }

    #[tokio::test]
    async fn missing_binary_error_tells_how_to_build_it() {
        let args = parse(&["mark2-bench", "--mark2", "/nope/mark2"]);
        let error = run(args).await.unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("cargo build --release"), "{text}");
        assert!(text.contains("/nope/mark2"), "{text}");
    }

    #[tokio::test]
    async fn zero_repeats_is_rejected_before_anything_runs() {
        let args = parse(&["mark2-bench", "--repeats", "0"]);
        let text = format!("{:#}", run(args).await.unwrap_err());
        assert!(text.contains("--repeats"), "{text}");
    }

    #[test]
    fn help_mentions_the_strategy_and_the_flags() {
        let help = Args::command().render_help().to_string();
        for flag in [
            "--models",
            "--tasks",
            "--repeats",
            "--label",
            "--mark2",
            "--tasks-dir",
            "--models-file",
            "--out",
        ] {
            assert!(help.contains(flag), "{flag} нет в справке:\n{help}");
        }
        assert!(help.contains("bench/STRATEGY.md"), "{help}");
    }

    /// Отчёт собирается из результатов, а не из состояния ранера: старый
    /// `results.json` обязан пересобираться в `report.md` без перепрогона.
    #[test]
    fn report_is_a_pure_function_of_results() {
        let json = r#"{
            "schema_version": 1,
            "label": "baseline",
            "run_id": "20260824-120000-baseline",
            "started_at": "2026-08-24T12:00:00Z",
            "finished_at": "2026-08-24T12:01:00Z",
            "config": {
                "bench_version": "0.1.0",
                "mark2": "/repo/target/release/mark2",
                "tasks_dir": "bench/tasks",
                "models_file": "bench/models.toml",
                "repeats_override": null,
                "verify_timeout_seconds": 30,
                "warmup_prompt": "Reply with exactly: ok",
                "project_config": "[agent]\ntemperature = 0.0\n",
                "models": [
                    {"alias": "qwen9", "model": "qwen3.5:9b", "base_url": "http://x/v1", "timeout_multiplier": 1.0}
                ],
                "tasks": [
                    {"id": "T01", "slug": "create-file", "dir": "bench/tasks/T01-create-file",
                     "timeout_seconds": 120, "repeats": 1, "tags": [], "prompt": "p"}
                ]
            },
            "runs": [
                {"model": "qwen9", "model_id": "qwen3.5:9b", "task": "T01", "repeat": 1,
                 "started_at": "2026-08-24T12:00:00Z", "pass": true, "exit_code": 0,
                 "timed_out": false, "wall_time_seconds": 12.3, "timeout_seconds": 120.0,
                 "tokens": {"input": 1200, "output": 340}, "session": null, "verify": null,
                 "answer_chars": 5, "stdout_path": null, "stderr_path": null,
                 "session_path": null, "error": null}
            ]
        }"#;
        let results: Results = serde_json::from_str(json).unwrap();
        let report = render_report(&results);
        assert!(report.contains("| qwen9 | 1/1 · 12.3s |"), "{report}");
        assert!(report.contains("1200 / 340"), "{report}");
    }
}
