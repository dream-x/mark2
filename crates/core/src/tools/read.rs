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

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Сколько байт файла максимум читаем в память.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Сколько байт максимум отдаём модели.
pub const MAX_OUTPUT_BYTES: usize = 100_000;

/// Аргументы тула `read`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadInput {
    /// Path to the file, absolute or relative to the working directory.
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

        let path = ctx.resolve(&input.path);
        let display = path.display();

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ToolOutput::err(format!(
                    "File not found: {display}. Check the path (relative paths resolve against {}) \
                     or list the directory with `bash`.",
                    ctx.cwd.display()
                ));
            }
            Err(error) => {
                return ToolOutput::err(format!("Cannot read {display}: {error}"));
            }
        };

        if metadata.is_dir() {
            return ToolOutput::err(format!(
                "{display} is a directory, not a file. List it with `bash` (`ls {display}`)."
            ));
        }

        let mut file = match tokio::fs::File::open(&path).await {
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

    #[tokio::test]
    async fn resolves_absolute_paths() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "body").unwrap();
        let other = TempDir::new().unwrap();
        let out = ReadTool::new()
            .execute(
                json!({"path": path.to_str().unwrap()}),
                &ToolCtx::new(other.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "body");
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
