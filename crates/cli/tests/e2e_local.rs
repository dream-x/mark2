//! E2E против живого локального сервера моделей.
//!
//! Тесты помечены `#[ignore]`: они требуют поднятого OpenAI-совместимого
//! сервера (ollama / llama-server / LM Studio) с моделью, умеющей tool calls.
//! Запуск:
//!
//! ```text
//! cargo test -p mark2-cli --test e2e_local -- --ignored
//! ```
//!
//! Куда ходить и какой моделью — из окружения:
//!
//! - `MARK2_E2E_BASE_URL` (по умолчанию `http://localhost:11434/v1`)
//! - `MARK2_E2E_MODEL` (по умолчанию `qwen3.5:9b`)
//! - `MARK2_E2E_TIMEOUT_SECS` (по умолчанию 300 — первая загрузка модели в
//!   память бывает долгой)
//!
//! Проверяем **наблюдаемый результат работы агента**, а не текст ответа:
//! локальная модель каждый раз формулирует по-своему, поэтому утверждения —
//! про созданный файл и про наличие подстроки-маркера в выводе.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Дефолты подобраны под ollama: самый частый локальный сервер.
const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
const DEFAULT_MODEL: &str = "qwen3.5:9b";
const DEFAULT_TIMEOUT_SECS: u64 = 300;

fn env_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

fn timeout() -> Duration {
    let secs = std::env::var("MARK2_E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Что вышло из одного запуска бинаря.
struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Run {
    /// Единый текст для сообщений об ошибке: без него падение теста на живой
    /// модели невозможно разобрать.
    fn report(&self, what: &str) -> String {
        format!(
            "{what}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout, self.stderr
        )
    }
}

/// Запустить `mark2` в `cwd` и дождаться завершения (или убить по таймауту).
///
/// Потоки уводим в файлы, а не в пайпы: пайп на несколько килобайт вывода
/// заблокировал бы дочерний процесс, пока тест ждёт его завершения.
fn run_mark2(cwd: &Path, args: &[&str], prompt: &str) -> Run {
    let base_url = env_or("MARK2_E2E_BASE_URL", DEFAULT_BASE_URL);
    let model = env_or("MARK2_E2E_MODEL", DEFAULT_MODEL);

    let logs = tempfile::tempdir().expect("временная директория для логов");
    let out_path = logs.path().join("stdout");
    let err_path = logs.path().join("stderr");
    let out_file = std::fs::File::create(&out_path).expect("файл stdout");
    let err_file = std::fs::File::create(&err_path).expect("файл stderr");

    let mut child = Command::new(env!("CARGO_BIN_EXE_mark2"))
        .arg("-p")
        // Сессии пишутся в ~/.mark2: тестам там делать нечего.
        .arg("--no-session")
        .args(["--base-url", &base_url])
        .args(["--model", &model])
        .args(["-C", cwd.to_str().expect("путь в utf-8")])
        .args(args)
        .arg(prompt)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("бинарь mark2 должен запускаться");

    let deadline = Instant::now() + timeout();
    let status = loop {
        match child.try_wait().expect("статус дочернего процесса") {
            Some(status) => break Some(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };

    let read = |path: &Path| {
        let mut buf = String::new();
        if let Ok(mut file) = std::fs::File::open(path) {
            let _ = file.read_to_string(&mut buf);
        }
        buf
    };
    let run = Run {
        ok: status.map(|status| status.success()).unwrap_or(false),
        stdout: read(&out_path),
        stderr: read(&err_path),
    };

    assert!(
        status.is_some(),
        "{}",
        run.report(&format!(
            "mark2 не завершился за {:?} (модель {model} на {base_url})",
            timeout()
        ))
    );
    run
}

/// Модель может ответить как угодно; сравниваем по-человечески.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Базовый сценарий MVP: «создай файл с таким содержимым» → файл на диске.
#[test]
#[ignore = "requires local model server"]
fn creates_a_file_with_requested_content() {
    let dir = tempfile::tempdir().expect("рабочая директория");
    let run = run_mark2(
        dir.path(),
        &["--yolo"],
        "Create a file hello.txt with content exactly: mark2 works",
    );
    assert!(run.ok, "{}", run.report("ход завершился неуспешно"));

    let path = dir.path().join("hello.txt");
    assert!(
        path.exists(),
        "{}",
        run.report(&format!("агент не создал {}", path.display()))
    );
    let content = std::fs::read_to_string(&path).expect("файл читается");
    assert!(
        contains_ignore_case(&content, "mark2 works"),
        "{}",
        run.report(&format!("в файле не то содержимое: {content:?}"))
    );
}

/// Задача на чтение: агент должен сходить в файл и назвать значение из него.
/// Политика `read-only` заодно проверяет, что без прав на запись ход не рушится.
#[test]
#[ignore = "requires local model server"]
fn reads_a_file_and_answers_from_it() {
    let dir = tempfile::tempdir().expect("рабочая директория");
    // Значение нарочно «нечеловеческое»: угадать его или знать заранее нельзя,
    // так что попадание в ответ означает реальный вызов тула read.
    std::fs::write(
        dir.path().join("service.conf"),
        "name = billing\nport = 48213\nworkers = 4\n",
    )
    .expect("подготовить файл");

    let run = run_mark2(
        dir.path(),
        &["--policy", "read-only"],
        "Read the file service.conf in the current directory and reply with the port number only.",
    );
    assert!(run.ok, "{}", run.report("ход завершился неуспешно"));
    assert!(
        run.stdout.contains("48213"),
        "{}",
        run.report("в ответе нет значения из файла")
    );
}
