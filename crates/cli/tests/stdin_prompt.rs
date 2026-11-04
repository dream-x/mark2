//! `mark2 -p -`: промпт из stdin — через настоящий бинарь, без живой модели.
//!
//! Юнит-тесты в `main.rs` проверяют разбор и чтение по отдельности; здесь —
//! что `main` их действительно связывает: ошибка аргументов уходит кодом 2,
//! пустой stdin — кодом 1 до любого запроса, а промпт больше лимита argv
//! Linux (128 КиБ на аргумент) доходит до сервера модели целиком. Сервер —
//! заглушка на один запрос, отвечающая SSE-стримом из одного текста. Здесь же —
//! что `--temperature` из такого вызова (так зовёт mark2 `scripts/review.sh`)
//! перекрывает проектный `.mark2.toml` в самом запросе к модели.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Сколько ждём бинарь: всё локально, хватает с запасом.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Бинарь с изолированным окружением: `$HOME` — пустая временная директория
/// (ни пользовательского конфига, ни сессий), рабочая директория — она же.
fn mark2(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mark2"));
    command
        .args(args)
        .arg("--no-session")
        .arg("-C")
        .arg(home)
        .env("HOME", home)
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Запустить, подать `input` на stdin и дождаться выхода (не дольше
/// [`TIMEOUT`]). stdin пишется из отдельного потока: промпт больше буфера
/// пайпа.
fn run(mut command: Command, input: &str) -> Output {
    let mut child: Child = command.spawn().expect("бинарь mark2 запускается");
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_string();
    let writer = thread::spawn(move || {
        // Бинарь может выйти, не дочитав (ошибка аргументов), — это не сбой.
        let _ = stdin.write_all(input.as_bytes());
    });
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            panic!("mark2 не завершился за {TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    writer.join().unwrap();
    child.wait_with_output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Сервер модели на один запрос: дочитывает запрос (заголовки +
/// `content-length`), отвечает SSE-стримом с `answer` и возвращает тело
/// запроса через `JoinHandle`.
fn serve_once(answer: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let event = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": answer}, "finish_reason": "stop"}]
    });
    let body = format!("data: {event}\n\ndata: [DONE]\n\n");
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        let mut expected = None;
        loop {
            if let Some((head_end, length)) = expected
                && request.len() >= head_end + length
            {
                break;
            }
            let n = socket.read(&mut buf).unwrap();
            assert!(n > 0, "запрос оборвался");
            request.extend_from_slice(&buf[..n]);
            if expected.is_none()
                && let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|value| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                expected = Some((end + 4, length));
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).unwrap();
        let (head_end, _) = expected.unwrap();
        String::from_utf8(request[head_end..].to_vec()).unwrap()
    });
    (base_url, handle)
}

#[test]
fn dash_without_print_mode_is_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let output = run(mark2(home.path(), &["--model", "m", "-"]), "задача");

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = text(&output.stderr);
    assert!(stderr.contains("stdin"), "{stderr}");
    assert!(stderr.contains("-p"), "{stderr}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn empty_stdin_fails_before_any_request() {
    let home = tempfile::tempdir().unwrap();
    // Порт 9 никто не слушает: дойди дело до запроса, ошибка была бы сетевой.
    let args = [
        "-p",
        "--model",
        "m",
        "--base-url",
        "http://127.0.0.1:9/v1",
        "-",
    ];
    for input in ["", "\n  \n"] {
        let output = run(mark2(home.path(), &args), input);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.contains("empty prompt"), "{stderr}");
        assert!(output.stdout.is_empty(), "{output:?}");
    }
}

#[test]
fn stdin_prompt_beyond_the_argv_limit_reaches_the_model_whole() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, server) = serve_once("ok");
    // 200 КиБ — больше, чем Linux пропускает одним аргументом.
    let prompt = format!("{} END-OF-PROMPT", "x".repeat(200 * 1024));

    let output = run(
        mark2(
            home.path(),
            &["-p", "--model", "m", "--base-url", &base_url, "-"],
        ),
        &prompt,
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(text(&output.stdout), "ok\n");
    let request = server.join().unwrap();
    assert!(request.contains(&prompt), "промпт дошёл не целиком");
}

/// Температура из тела запроса к модели.
fn sent_temperature(request: &str) -> Option<f64> {
    let body: serde_json::Value = serde_json::from_str(request).expect("тело запроса — JSON");
    body.get("temperature").map(|t| t.as_f64().expect("число"))
}

/// Слои по-настоящему: `.mark2.toml` в `-C` говорит 0.9, флаг — 0, и в запрос
/// уходит 0; без флага уходит значение из файла.
#[test]
fn temperature_flag_beats_the_project_config_in_the_request() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".mark2.toml"),
        "[agent]\ntemperature = 0.9\n",
    )
    .unwrap();

    let (base_url, server) = serve_once("ok");
    let flag = [
        "-p",
        "--policy",
        "read-only",
        "--model",
        "m",
        "--base-url",
        &base_url,
        "--temperature",
        "0",
        "-",
    ];
    let output = run(mark2(home.path(), &flag), "задача");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(sent_temperature(&server.join().unwrap()), Some(0.0));

    let (base_url, server) = serve_once("ok");
    let silent = ["-p", "--model", "m", "--base-url", &base_url, "-"];
    let output = run(mark2(home.path(), &silent), "задача");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let from_file = sent_temperature(&server.join().unwrap()).expect("температура из файла");
    assert!((from_file - 0.9).abs() < 1e-6, "{from_file}");
}
