//! End-to-end ранера с **фейковым mark2**.
//!
//! Настоящий бенч ходит в модель, поэтому его нельзя гонять в CI. Но всё, что
//! между промптом и вердиктом, — наше: копирование `setup/`, изоляция `$HOME`,
//! аргументы запуска, `.mark2.toml` с температурой 0, таймаут с убийством
//! процесса, сбор сессии, `verify.sh`, `results.json` и `report.md`. Это и
//! проверяется — на шелл-скрипте, который притворяется агентом: пишет ответ в
//! stdout, строку токенов в stderr, создаёт файл в cwd и кладёт JSONL-сессию в
//! `$HOME/.mark2/sessions/**`.
//!
//! Ни сети, ни настоящих моделей.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Фейковый mark2: разбирает те же флаги, что и настоящий, и оставляет после
/// себя ровно те следы, по которым ранер считает метрики.
const FAKE_MARK2: &str = r#"#!/bin/sh
set -e

# Улика для теста: с чем нас позвали и что видно из cwd прогона.
if [ -n "$FAKE_MARK2_LOG" ]; then
    {
        echo "--- run"
        echo "argv: $*"
        echo "home: $HOME"
        echo "config: $(tr '\n' ' ' < .mark2.toml 2>/dev/null)"
    } >> "$FAKE_MARK2_LOG"
fi

cwd="."
prompt=""
while [ $# -gt 0 ]; do
    case "$1" in
        -C) cwd="$2"; shift 2 ;;
        --base-url|--model|--api-key-env) shift 2 ;;
        -p|--yolo) shift ;;
        *) prompt="$1"; shift ;;
    esac
done

# Задача-долгожитель: ранер обязан убить нас по таймауту.
case "$prompt" in
    *SLEEP*) sleep 30 ;;
esac

printf 'mark2 works\n' > "$cwd/hello.txt"
printf 'Готово: файл создан.\n'
printf 'tokens: in 1200 / out 340\n' >&2

sessions="$HOME/.mark2/sessions/2026/08/24"
mkdir -p "$sessions"
cat > "$sessions/1756000000-11111111-2222-3333-4444-555555555555.jsonl" <<'JSONL'
{"ts":"2026-08-24T09:00:00Z","type":"meta","id":"11111111-2222-3333-4444-555555555555","cwd":"/w","model":"fake","provider":"openai_compat"}
{"ts":"2026-08-24T09:00:01Z","type":"turn_started","turn_id":"t1"}
{"ts":"2026-08-24T09:00:02Z","type":"message","message":{"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"write","input":{"path":"hello.txt","content":"mark2 works"}}]}}
{"ts":"2026-08-24T09:00:03Z","type":"message","message":{"role":"tool","content":[{"type":"tool_result","tool_use_id":"c1","content":"ok"}]}}
{"ts":"2026-08-24T09:00:04Z","type":"message","message":{"role":"assistant","content":[{"type":"tool_use","id":"c2","name":"edit","input":{"path":"hello.txt"}}]}}
{"ts":"2026-08-24T09:00:05Z","type":"message","message":{"role":"tool","content":[{"type":"tool_result","tool_use_id":"c2","content":"missing field `old_string`","is_error":true}]}}
{"ts":"2026-08-24T09:00:06Z","type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Готово: файл создан."}]}}
{"ts":"2026-08-24T09:00:07Z","type":"turn_finished","turn_id":"t1","reason":"completed"}
JSONL
"#;

/// Матрица из двух фейковых моделей: проверяем и порядок model-major, и
/// множитель таймаута.
const FAKE_MODELS: &str = r#"
[[models]]
alias = "fake1"
model = "fake-model-1"
base_url = "http://127.0.0.1:1/v1"
timeout_multiplier = 1.0

[[models]]
alias = "fake2"
model = "fake-model-2"
base_url = "http://127.0.0.1:2/v1"
timeout_multiplier = 2.0
"#;

struct Fixture {
    root: TempDir,
    mark2: PathBuf,
    tasks: PathBuf,
    models: PathBuf,
    out: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let mark2 = root.path().join("fake-mark2");
        write_executable(&mark2, FAKE_MARK2);

        let models = root.path().join("models.toml");
        std::fs::write(&models, FAKE_MODELS).unwrap();

        Self {
            mark2,
            tasks: root.path().join("tasks"),
            models,
            out: root.path().join("results"),
            log: root.path().join("fake.log"),
            root,
        }
    }

    /// Задача бенча по контракту: prompt.md, setup/, verify.sh, task.toml.
    fn task(&self, name: &str, prompt: &str, task_toml: &str, verify: &str) {
        let dir = self.tasks.join(name);
        std::fs::create_dir_all(dir.join("setup")).unwrap();
        std::fs::write(dir.join("prompt.md"), prompt).unwrap();
        std::fs::write(dir.join("task.toml"), task_toml).unwrap();
        std::fs::write(dir.join("verify.sh"), verify).unwrap();
        std::fs::write(dir.join("setup").join("given.txt"), "исходные данные\n").unwrap();
    }

    fn bench(&self, args: &[&str]) -> std::process::Output {
        self.bench_with_paths(
            &[
                "--mark2",
                self.mark2.to_str().unwrap(),
                "--tasks-dir",
                self.tasks.to_str().unwrap(),
                "--models-file",
                self.models.to_str().unwrap(),
                "--out",
                self.out.to_str().unwrap(),
            ],
            args,
        )
    }

    /// Прогон с произвольным набором путей — в том числе относительных.
    /// cwd процесса всегда корень фикстуры, как у смоука cwd = корень репо.
    fn bench_with_paths(&self, paths: &[&str], args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_mark2-bench"))
            .args(paths)
            .args(args)
            .current_dir(self.root.path())
            .env("FAKE_MARK2_LOG", &self.log)
            .output()
            .expect("ранер должен запускаться")
    }

    /// Единственная директория прогона внутри `--out`.
    fn run_dir(&self) -> PathBuf {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&self.out)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        dirs.sort();
        assert_eq!(dirs.len(), 1, "ожидали одну директорию прогона: {dirs:?}");
        dirs.pop().unwrap()
    }

    fn results(&self) -> serde_json::Value {
        let raw = std::fs::read_to_string(self.run_dir().join("results.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn report(&self) -> String {
        std::fs::read_to_string(self.run_dir().join("report.md")).unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn write_executable(path: &Path, content: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, content).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Полный прогон: две модели × две задачи, одна из которых заведомо проваливается.
#[test]
fn full_run_writes_results_report_and_sessions() {
    let fixture = Fixture::new();
    fixture.task(
        "T01-create-file",
        "Создай hello.txt с содержимым ровно: mark2 works\n",
        "[task]\ntimeout_seconds = 60\nrepeats = 2\ntags = [\"write\"]\n",
        // Проверяем и артефакт, и что setup доехал, и что stdout захвачен.
        "#!/bin/sh\ntest \"$(cat hello.txt)\" = 'mark2 works' || exit 1\ntest -f given.txt || exit 1\ngrep -q 'Готово' \"$MARK2_STDOUT\" || exit 1\n",
    );
    fixture.task(
        "T02-impossible",
        "Сделай невозможное.\n",
        "[task]\ntimeout_seconds = 60\nrepeats = 1\n",
        "#!/bin/sh\ntest -f missing.txt\n",
    );

    let output = fixture.bench(&["--label", "e2e"]);
    assert!(
        output.status.success(),
        "ранер упал: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let results = fixture.results();
    assert_eq!(results["schema_version"], 1);
    assert_eq!(results["label"], "e2e");
    assert!(
        results["run_id"].as_str().unwrap().ends_with("-e2e"),
        "{}",
        results["run_id"]
    );

    let runs = results["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 6, "2 модели × (2 + 1) повторов");

    // Model-major: сперва все задачи первой модели, потом второй.
    let order: Vec<String> = runs
        .iter()
        .map(|run| {
            format!(
                "{}/{}/{}",
                run["model"].as_str().unwrap(),
                run["task"].as_str().unwrap(),
                run["repeat"]
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            "fake1/T01/1",
            "fake1/T01/2",
            "fake1/T02/1",
            "fake2/T01/1",
            "fake2/T01/2",
            "fake2/T02/1",
        ]
    );

    let first = &runs[0];
    assert_eq!(first["pass"], true);
    assert_eq!(first["exit_code"], 0);
    assert_eq!(first["timed_out"], false);
    assert_eq!(first["error"], serde_json::Value::Null);
    assert_eq!(first["tokens"]["input"], 1200);
    assert_eq!(first["tokens"]["output"], 340);
    assert_eq!(first["session"]["assistant_messages"], 3);
    assert_eq!(first["session"]["tool_uses"], 2);
    assert_eq!(first["session"]["tool_errors"], 1);
    assert_eq!(first["session"]["compactions"], 0);
    assert_eq!(first["session"]["stop_reason"], "completed");
    assert_eq!(first["session"]["skipped_lines"], 0);
    assert!(first["answer_chars"].as_u64().unwrap() > 0);
    assert_eq!(first["verify"]["exit_code"], 0);
    // Таймаут задачи умножается на множитель модели.
    assert_eq!(first["timeout_seconds"], 60.0);
    assert_eq!(runs[3]["timeout_seconds"], 120.0);

    // Провалившаяся задача — это данные, а не сбой бенча.
    let failed = &runs[2];
    assert_eq!(failed["task"], "T02");
    assert_eq!(failed["pass"], false);
    assert_eq!(failed["verify"]["exit_code"], 1);

    // Артефакты на диске: сессия, stdout, stderr — по путям из results.json.
    let run_dir = fixture.run_dir();
    let session_path = first["session_path"].as_str().unwrap();
    assert_eq!(session_path, "sessions/fake1/T01/1.jsonl");
    let session = std::fs::read_to_string(run_dir.join(session_path)).unwrap();
    assert!(session.contains("\"type\":\"turn_finished\""), "{session}");
    let stdout =
        std::fs::read_to_string(run_dir.join(first["stdout_path"].as_str().unwrap())).unwrap();
    assert_eq!(stdout, "Готово: файл создан.\n");
    let stderr =
        std::fs::read_to_string(run_dir.join(first["stderr_path"].as_str().unwrap())).unwrap();
    assert!(stderr.contains("tokens: in 1200 / out 340"), "{stderr}");

    // Конфиг прогона воспроизводим по одному файлу.
    let config = &results["config"];
    assert_eq!(config["repeats_override"], serde_json::Value::Null);
    assert_eq!(config["verify_timeout_seconds"], 30);
    assert_eq!(config["warmup_prompt"], "Reply with exactly: ok");
    assert_eq!(config["tasks"][0]["repeats"], 2);
    assert_eq!(config["tasks"][0]["tags"][0], "write");
    assert!(
        config["tasks"][0]["prompt"]
            .as_str()
            .unwrap()
            .contains("mark2 works")
    );

    // Отчёт: таблица, сводка, аномалии.
    let report = fixture.report();
    assert!(report.contains("| fake1 | 2/2 · "), "{report}");
    assert!(report.contains("0/1 · "), "{report}");
    assert!(report.contains("| fake1 | 2/3 (67%)"), "{report}");
    assert!(report.contains("1200 / 340"), "{report}");
    assert!(report.contains("50% (3/6)"), "tool error rate:\n{report}");
    assert!(report.contains("## Аномалии"), "{report}");

    // Прогон mark2: аргументы, изоляция HOME, температура 0, прогрев на модель.
    let log = fixture.log();
    assert!(
        log.contains("argv: -p --yolo --base-url http://127.0.0.1:1/v1 --model fake-model-1 -C "),
        "{log}"
    );
    assert!(log.contains("--model fake-model-2"), "{log}");
    assert!(log.contains("Создай hello.txt"), "{log}");
    assert!(log.contains("config: [agent] temperature = 0.0"), "{log}");
    assert!(
        !log.contains(&format!(
            "home: {}",
            std::env::var("HOME").unwrap_or_default()
        )),
        "$HOME обязан быть подменён:\n{log}"
    );
    assert_eq!(
        log.matches("--- run").count(),
        8,
        "6 прогонов + по прогреву на модель:\n{log}"
    );
    assert!(log.contains("Reply with exactly: ok"), "прогрев:\n{log}");
}

/// Фильтры и `--repeats`: гоняем ровно то, что попросили.
#[test]
fn filters_and_repeats_override_narrow_the_matrix() {
    let fixture = Fixture::new();
    fixture.task(
        "T01-create-file",
        "Создай hello.txt\n",
        "[task]\ntimeout_seconds = 60\nrepeats = 3\n",
        "#!/bin/sh\ntest -f hello.txt\n",
    );
    fixture.task(
        "T02-other",
        "Что-нибудь ещё.\n",
        "[task]\ntimeout_seconds = 60\n",
        "#!/bin/sh\nexit 0\n",
    );

    let output = fixture.bench(&[
        "--models",
        "fake2",
        "--tasks",
        "T01",
        "--repeats",
        "1",
        "--no-warmup",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let results = fixture.results();
    let runs = results["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["model"], "fake2");
    assert_eq!(runs[0]["task"], "T01");
    assert_eq!(results["config"]["repeats_override"], 1);
    assert_eq!(results["config"]["models"].as_array().unwrap().len(), 1);
    assert_eq!(results["config"]["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(fixture.log().matches("--- run").count(), 1, "прогрева нет");

    // Прогресс — в stderr по ходу дела.
    let progress = String::from_utf8_lossy(&output.stderr);
    assert!(
        progress.contains("[fake2] T01 run 1/1 … pass"),
        "{progress}"
    );
}

/// Регрессия: относительные пути в флагах (как при `mark2-bench --tasks T01`
/// из корня репозитория с дефолтным `--tasks-dir bench/tasks`).
///
/// `verify.sh` и mark2 запускаются с cwd = tmp-директория прогона, поэтому
/// относительный путь оттуда не резолвится: до фикса verify отдавал exit 127 и
/// `sh: tasks/T01-create-file/verify.sh: No such file or directory`, а решённая
/// задача засчитывалась как провал.
#[test]
fn relative_paths_still_resolve_from_the_run_cwd() {
    let fixture = Fixture::new();
    fixture.task(
        "T01-create-file",
        "Создай hello.txt с содержимым ровно: mark2 works\n",
        "[task]\ntimeout_seconds = 60\nrepeats = 1\n",
        "#!/bin/sh\ntest \"$(cat hello.txt)\" = 'mark2 works' || exit 1\ntest -f given.txt || exit 1\ngrep -q 'Готово' \"$MARK2_STDOUT\" || exit 1\n",
    );

    let output = fixture.bench_with_paths(
        &[
            "--mark2",
            "./fake-mark2",
            "--tasks-dir",
            "tasks",
            "--models-file",
            "models.toml",
            "--out",
            "results",
        ],
        &[
            "--models",
            "fake1",
            "--tasks",
            "T01",
            "--repeats",
            "1",
            "--no-warmup",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let results = fixture.results();
    let run = &results["runs"][0];
    assert_eq!(run["exit_code"], 0);
    assert_eq!(
        run["verify"]["exit_code"], 0,
        "verify не нашёл свой скрипт: {}",
        run["verify"]
    );
    assert_eq!(run["verify"]["stderr"], "");
    assert_eq!(
        run["pass"], true,
        "решённая задача обязана быть pass: {run}"
    );

    // Пути в сырье развёрнуты — results.json читается не только из cwd прогона.
    let config = &results["config"];
    for key in ["mark2", "tasks_dir", "models_file"] {
        let path = config[key].as_str().unwrap();
        assert!(Path::new(path).is_absolute(), "{key} = {path}");
    }
    assert!(
        Path::new(config["tasks"][0]["dir"].as_str().unwrap()).is_absolute(),
        "{}",
        config["tasks"][0]["dir"]
    );

    // Артефакты нашлись по путям из сырья (они относительны run_dir).
    let run_dir = fixture.run_dir();
    for key in ["stdout_path", "stderr_path", "session_path"] {
        let relative = run[key].as_str().unwrap_or_else(|| panic!("{key} пуст"));
        assert!(
            run_dir.join(relative).is_file(),
            "{key} = {relative} не найден в {}",
            run_dir.display()
        );
    }
}

/// Зависший прогон убивается по таймауту и помечается в результатах.
#[test]
fn hanging_run_is_killed_and_reported_as_timeout() {
    let fixture = Fixture::new();
    fixture.task(
        "T09-hang",
        "SLEEP и не отвечай\n",
        "[task]\ntimeout_seconds = 1\nrepeats = 1\n",
        "#!/bin/sh\ntest -f hello.txt\n",
    );

    let output = fixture.bench(&["--models", "fake1", "--no-warmup", "--label", "timeout"]);
    assert!(
        output.status.success(),
        "таймаут задачи не должен ронять бенч: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let results = fixture.results();
    let run = &results["runs"][0];
    assert_eq!(run["timed_out"], true);
    assert_eq!(run["exit_code"], serde_json::Value::Null);
    assert_eq!(run["pass"], false);
    assert_eq!(run["answer_chars"], 0);
    assert_eq!(
        run["session"],
        serde_json::Value::Null,
        "сессии не случилось"
    );
    assert_eq!(run["tokens"], serde_json::Value::Null);
    let wall = run["wall_time_seconds"].as_f64().unwrap();
    assert!(
        (1.0..10.0).contains(&wall),
        "убить должны были почти сразу после бюджета, а не досидеть 30с: {wall}"
    );

    let report = fixture.report();
    assert!(report.contains("таймаут mark2"), "{report}");
    assert!(report.contains("сессия не записана"), "{report}");
    // Пустой ответ убитого прогона — следствие таймаута, отдельной строкой не
    // дублируется: аномалии должны показывать причину, а не её симптомы.
    assert!(!report.contains("пустой ответ"), "{report}");
}

/// Понятные ошибки до старта: нет бинаря, нет задач, опечатка в алиасе.
#[test]
fn setup_errors_are_explicit() {
    let fixture = Fixture::new();
    fixture.task(
        "T01-x",
        "Задача\n",
        "[task]\ntimeout_seconds = 60\n",
        "#!/bin/sh\nexit 0\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_mark2-bench"))
        .args(["--mark2", "/nope/mark2"])
        .current_dir(fixture.root.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cargo build --release"), "{stderr}");

    let output = fixture.bench(&["--models", "fake9"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fake9"), "{stderr}");
    assert!(stderr.contains("fake1, fake2"), "{stderr}");

    let output = fixture.bench(&["--tasks", "T77"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("T77"), "{stderr}");
    assert!(stderr.contains("T01-x"), "{stderr}");

    // Директория результатов не создаётся, пока не прошли проверки.
    assert!(!fixture.out.exists(), "лишняя директория прогона");
}
