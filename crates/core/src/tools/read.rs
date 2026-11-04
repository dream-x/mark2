//! Тул `read`: чтение текстового файла с постраничным доступом.
//!
//! Два капа, оба ради контекста модели, а не ради безопасности:
//! [`MAX_FILE_BYTES`] ограничивает, сколько файла вообще берём в память, а
//! [`MAX_OUTPUT_BYTES`] — сколько уходит в промпт. Обрезка всегда помечается
//! маркером: молча укоротить файл хуже, чем сказать модели «дальше есть ещё».
//!
//! Строки режутся по `\n` с сохранением разделителей (`split_inclusive`), чтобы
//! содержимое возвращалось байт-в-байт: `edit` ищет в файле точное вхождение,
//! и «нормализованный» CRLF ломал бы поиск.
//!
//! Читается только внутри cwd: путь проходит общий с `grep` замок (модуль
//! `tools::confine`) — канонизируется, и абсолютный путь наружу, `..` или
//! симлинк наружу отбиваются текстом для модели. FIFO и устройства — тоже:
//! `open` на них повис бы, а отмену хода он не слышит.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::confine::{CanonicalCwd, ConfineError, Confined};
use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Сколько байт файла максимум читаем в память.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Сколько байт максимум отдаём модели.
pub const MAX_OUTPUT_BYTES: usize = 100_000;

/// Аргументы тула `read`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadInput {
    /// Path to a file inside the working directory, relative to it or absolute.
    pub path: String,
    /// 1-based line number to start from. Defaults to the first line.
    #[serde(default)]
    pub offset: Option<u64>,
    /// Maximum number of lines to return. Defaults to the whole file.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Тул чтения файлов.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadTool;

impl ReadTool {
    /// Новый тул.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a text file and return its contents verbatim. \
         Use `offset` and `limit` to page through large files; \
         output is capped at 100000 bytes and truncation is marked."
    }

    fn input_schema(&self) -> Value {
        input_schema_for::<ReadInput>()
    }

    async fn execute(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        let input: ReadInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid arguments for tool `read`: {error}. Expected {{\"path\": \"...\"}}."
                ));
            }
        };

        // В текстах — путь, как его видит модель: склеенный с cwd, но не
        // канонизированный. Открывается — канонический, прошедший замок.
        let path = ctx.resolve(&input.path);
        let display = path.display();

        // Канонизация и `metadata` — блокирующие `std::fs`, им место вне
        // потока рантайма.
        let confined = {
            let cwd = ctx.cwd.clone();
            let requested = input.path.clone();
            let path = path.clone();
            tokio::task::spawn_blocking(move || resolve_inside_cwd(&cwd, &requested, &path)).await
        };
        let Confined {
            path: canonical,
            metadata,
        } = match confined {
            Ok(Ok(confined)) => confined,
            Ok(Err(message)) => return ToolOutput::err(message),
            Err(error) => return ToolOutput::err(format!("Cannot read {display}: {error}")),
        };

        if metadata.is_dir() {
            return ToolOutput::err(format!(
                "{display} is a directory, not a file. List it with `bash` (`ls {display}`)."
            ));
        }

        let mut file = match tokio::fs::File::open(&canonical).await {
            Ok(file) => file,
            Err(error) => return ToolOutput::err(format!("Cannot read {display}: {error}")),
        };

        let mut bytes = Vec::new();
        if let Err(error) = (&mut file)
            .take(MAX_FILE_BYTES)
            .read_to_end(&mut bytes)
            .await
        {
            return ToolOutput::err(format!("Cannot read {display}: {error}"));
        }

        let file_clipped = metadata.len() > MAX_FILE_BYTES;
        let text = String::from_utf8_lossy(&bytes);

        if text.is_empty() {
            return ToolOutput::ok(format!("[{display} is empty]"));
        }

        let start = input.offset.unwrap_or(1).max(1);
        let limit = input.limit.unwrap_or(u64::MAX).max(1);

        let mut output = String::new();
        let mut line_number: u64 = 0;
        let mut first_line: u64 = 0;
        let mut last_line: u64 = 0;
        let mut taken: u64 = 0;
        let mut more_lines = false;
        let mut clipped_bytes = false;

        for line in text.split_inclusive('\n') {
            line_number += 1;
            if line_number < start {
                continue;
            }
            if taken >= limit {
                more_lines = true;
                break;
            }
            if output.len() + line.len() > MAX_OUTPUT_BYTES {
                clipped_bytes = true;
                more_lines = true;
                // Одна строка длиннее всего лимита: отдаём её начало, иначе
                // модель получила бы пустой результат.
                if output.is_empty() {
                    output.push_str(clip_to_char_boundary(line, MAX_OUTPUT_BYTES));
                    first_line = line_number;
                    last_line = line_number;
                }
                break;
            }
            if first_line == 0 {
                first_line = line_number;
            }
            last_line = line_number;
            taken += 1;
            output.push_str(line);
        }

        if first_line == 0 {
            return ToolOutput::ok(format!(
                "[no lines at offset {start}: {display} has {line_number} lines]"
            ));
        }

        let mut markers: Vec<String> = Vec::new();
        if clipped_bytes {
            markers.push(format!("[output truncated at {MAX_OUTPUT_BYTES} bytes]"));
        }
        if more_lines {
            markers.push(format!(
                "[showing lines {first_line}-{last_line}; read on with offset={}]",
                last_line + 1
            ));
        }
        if file_clipped {
            markers.push(format!(
                "[only the first {MAX_FILE_BYTES} bytes of {} were read]",
                metadata.len()
            ));
        }

        if !markers.is_empty() {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&markers.join("\n"));
        }

        ToolOutput::ok(output)
    }
}

/// Файл для чтения: `path` (уже склеенный с `cwd`), пропущенный через замок
/// [`CanonicalCwd::confine`]. `Err` — готовый текст ошибки для модели;
/// `requested` — путь ровно как его прислала модель.
fn resolve_inside_cwd(cwd: &Path, requested: &str, path: &Path) -> Result<Confined, String> {
    let display = path.display();
    let canonical_cwd = CanonicalCwd::new(cwd).map_err(|error| {
        format!(
            "Cannot read {display}: the working directory {} is not accessible: {error}",
            cwd.display()
        )
    })?;
    canonical_cwd.confine(path).map_err(|error| match error {
        // «Файла нет» — тем же текстом, что и до замка: это не отказ в доступе.
        ConfineError::NotFound => format!(
            "File not found: {display}. Check the path (relative paths resolve against {}) \
             or list the directory with `bash`.",
            cwd.display()
        ),
        ConfineError::Outside => format!(
            "Path {requested} is outside the working directory {} (after resolving `..` and \
             symlinks): `read` only reads files inside the working directory, and retrying this \
             path will fail the same way. Pass a path within the working directory; relative \
             paths resolve against it.",
            cwd.display()
        ),
        ConfineError::NotRegular => format!(
            "{display} is not a regular file (a FIFO, socket or device): `read` cannot read it."
        ),
        ConfineError::Unresolvable(error) | ConfineError::Metadata(error) => {
            format!("Cannot read {display}: {error}")
        }
    })
}

/// Обрезать строку по границе символа, чтобы не разрубить UTF-8.
fn clip_to_char_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn run(dir: &TempDir, input: Value) -> ToolOutput {
        ReadTool::new()
            .execute(input, &ToolCtx::new(dir.path()))
            .await
    }

    #[tokio::test]
    async fn reads_a_file_verbatim() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\r\ntwo\nthree").unwrap();
        let out = run(&dir, json!({"path": "a.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "one\r\ntwo\nthree");
    }

    /// Отказ «снаружи»: ошибка текстом для модели, содержимое не утекает.
    fn assert_outside(out: &ToolOutput, path: &str) {
        assert!(out.is_error, "{path}: {}", out.content);
        assert!(
            out.content.contains("is outside the working directory"),
            "{path}: {}",
            out.content
        );
        assert!(
            out.content
                .contains("`read` only reads files inside the working directory"),
            "{path}: {}",
            out.content
        );
        assert!(!out.content.contains("secret"), "{path}: {}", out.content);
    }

    #[tokio::test]
    async fn absolute_paths_are_read_only_inside_the_cwd() {
        let dir = TempDir::new().unwrap();
        let inside = dir.path().join("a.txt");
        std::fs::write(&inside, "body").unwrap();
        let out = run(&dir, json!({"path": inside.to_str().unwrap()})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "body");

        let other = TempDir::new().unwrap();
        let outside = other.path().join("b.txt");
        std::fs::write(&outside, "secret").unwrap();
        let path = outside.to_str().unwrap();
        let out = run(&dir, json!({"path": path})).await;
        assert_outside(&out, path);
        assert!(out.content.contains(path), "{}", out.content);
    }

    #[tokio::test]
    async fn parent_traversal_out_of_the_cwd_is_rejected() {
        let parent = TempDir::new().unwrap();
        std::fs::write(parent.path().join("x.txt"), "secret").unwrap();
        let work = parent.path().join("work");
        std::fs::create_dir_all(work.join("sub")).unwrap();
        std::fs::write(work.join("a.txt"), "body").unwrap();
        let ctx = ToolCtx::new(&work);

        for path in ["../x.txt", "sub/../../x.txt", "/etc/hosts"] {
            let out = ReadTool::new().execute(json!({"path": path}), &ctx).await;
            assert_outside(&out, path);
        }

        // `..`, не покидающий cwd, — законно.
        let out = ReadTool::new()
            .execute(json!({"path": "sub/../a.txt"}), &ctx)
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "body");
    }

    /// Суть дыры: симлинк внутри cwd, ведущий наружу, раньше читался.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_leading_outside_is_rejected() {
        use std::os::unix::fs::symlink;
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("x.txt"), "secret").unwrap();
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "body").unwrap();
        symlink(outside.path().join("x.txt"), dir.path().join("link.txt")).unwrap();
        symlink(outside.path(), dir.path().join("link_dir")).unwrap();
        symlink(dir.path().join("a.txt"), dir.path().join("alias.txt")).unwrap();

        for path in ["link.txt", "link_dir/x.txt"] {
            let out = run(&dir, json!({"path": path})).await;
            assert_outside(&out, path);
        }

        // Симлинк, ведущий внутрь cwd, читается как обычно.
        let out = run(&dir, json!({"path": "alias.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "body");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fifo_is_rejected_instead_of_hanging() {
        let dir = TempDir::new().unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(dir.path().join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run(&dir, json!({"path": "pipe"})),
        )
        .await
        .expect("read повис на FIFO");
        assert!(out.is_error);
        assert!(
            out.content.contains("is not a regular file"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn missing_file_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"path": "nope.txt"})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("File not found:"),
            "{}",
            out.content
        );
        assert!(out.content.contains("nope.txt"), "{}", out.content);
        // «Нет файла» — не «доступ запрещён».
        assert!(!out.content.contains("outside"), "{}", out.content);
    }

    #[tokio::test]
    async fn directory_is_rejected() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let out = run(&dir, json!({"path": "sub"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("is a directory"), "{}", out.content);
    }

    #[tokio::test]
    async fn empty_file_is_marked() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let out = run(&dir, json!({"path": "empty.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("is empty"), "{}", out.content);
    }

    #[tokio::test]
    async fn offset_and_limit_select_a_window() {
        let dir = TempDir::new().unwrap();
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.path().join("a.txt"), body).unwrap();

        let out = run(&dir, json!({"path": "a.txt", "offset": 3, "limit": 2})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("line3\nline4\n"), "{}", out.content);
        assert!(
            out.content
                .contains("[showing lines 3-4; read on with offset=5]"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn limit_covering_the_file_adds_no_marker() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\nb\n").unwrap();
        let out = run(&dir, json!({"path": "a.txt", "offset": 1, "limit": 100})).await;
        assert_eq!(out.content, "a\nb\n");
    }

    #[tokio::test]
    async fn offset_past_the_end_explains_itself() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\nb\n").unwrap();
        let out = run(&dir, json!({"path": "a.txt", "offset": 50})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("[no lines at offset 50"),
            "{}",
            out.content
        );
        assert!(out.content.contains("has 2 lines"), "{}", out.content);
    }

    #[tokio::test]
    async fn large_file_is_truncated_with_a_marker() {
        let dir = TempDir::new().unwrap();
        let body: String = std::iter::repeat_n("0123456789012345678\n", 20_000).collect();
        assert!(body.len() > MAX_OUTPUT_BYTES);
        std::fs::write(dir.path().join("big.txt"), &body).unwrap();

        let out = run(&dir, json!({"path": "big.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains(&format!("[output truncated at {MAX_OUTPUT_BYTES} bytes]")),
            "tail: {}",
            &out.content[out.content.len() - 200..]
        );
        assert!(
            out.content.contains("read on with offset="),
            "no offset hint"
        );
        assert!(out.content.len() < MAX_OUTPUT_BYTES + 300);
    }

    #[tokio::test]
    async fn garbage_input_is_reported_without_panicking() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"path": 7})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Invalid arguments for tool `read`"),
            "{}",
            out.content
        );
    }

    #[test]
    fn schema_requires_only_the_path() {
        let schema = ReadTool::new().input_schema();
        assert_eq!(schema["required"], json!(["path"]));
        assert!(schema["properties"]["offset"]["description"].is_string());
        assert!(schema["properties"]["limit"]["description"].is_string());
    }
}
