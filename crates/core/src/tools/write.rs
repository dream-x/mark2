//! Тул `write`: создать файл или перезаписать его целиком.
//!
//! Родительские директории создаются молча — модель, которой пришлось бы
//! сначала звать `bash mkdir -p`, тратит лишний ход на каждый новый файл.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Аргументы тула `write`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteInput {
    /// Path to the file, absolute or relative to the working directory.
    pub path: String,
    /// Full contents of the file. Any existing contents are replaced.
    pub content: String,
}

/// Тул записи файлов.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteTool;

impl WriteTool {
    /// Новый тул.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write a file with the given contents, creating parent directories and \
         overwriting any existing file. To change part of an existing file use `edit` instead."
    }

    fn input_schema(&self) -> Value {
        input_schema_for::<WriteInput>()
    }

    async fn execute(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        let input: WriteInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid arguments for tool `write`: {error}. \
                     Expected {{\"path\": \"...\", \"content\": \"...\"}}."
                ));
            }
        };

        let path = ctx.resolve(&input.path);
        let display = path.display();

        if path.is_dir() {
            return ToolOutput::err(format!("{display} is a directory, not a file."));
        }

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            return ToolOutput::err(format!(
                "Cannot create directory {}: {error}",
                parent.display()
            ));
        }

        let existed = tokio::fs::try_exists(&path).await.unwrap_or(false);
        let bytes = input.content.len();

        match tokio::fs::write(&path, input.content.as_bytes()).await {
            Ok(()) => {
                let verb = if existed { "Overwrote" } else { "Wrote" };
                ToolOutput::ok(format!("{verb} {display} ({bytes} bytes)"))
            }
            Err(error) => ToolOutput::err(format!("Cannot write {display}: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn run(dir: &TempDir, input: Value) -> ToolOutput {
        WriteTool::new()
            .execute(input, &ToolCtx::new(dir.path()))
            .await
    }

    #[tokio::test]
    async fn writes_a_file_and_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let out = run(
            &dir,
            json!({"path": "deep/nested/a.txt", "content": "hello\n"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("Wrote "), "{}", out.content);
        assert!(out.content.contains("(6 bytes)"), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("deep/nested/a.txt")).unwrap(),
            "hello\n"
        );
    }

    #[tokio::test]
    async fn overwrites_existing_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old contents").unwrap();
        let out = run(&dir, json!({"path": "a.txt", "content": "new"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("Overwrote "), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "new"
        );
    }

    #[tokio::test]
    async fn byte_count_is_in_bytes_not_chars() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"path": "a.txt", "content": "привет"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(12 bytes)"), "{}", out.content);
    }

    #[tokio::test]
    async fn empty_content_is_allowed() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"path": "a.txt", "content": ""})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(dir.path().join("a.txt").exists());
    }

    #[tokio::test]
    async fn absolute_paths_are_used_as_is() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("abs.txt");
        let other = TempDir::new().unwrap();
        let out = WriteTool::new()
            .execute(
                json!({"path": path.to_str().unwrap(), "content": "x"}),
                &ToolCtx::new(other.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn directory_target_is_rejected() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let out = run(&dir, json!({"path": "sub", "content": "x"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("is a directory"), "{}", out.content);
    }

    #[tokio::test]
    async fn unwritable_parent_is_reported() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file"), "x").unwrap();
        // `file` — не директория, значит `file/child` создать нельзя.
        let out = run(&dir, json!({"path": "file/child.txt", "content": "x"})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Cannot create directory"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn garbage_input_is_reported_without_panicking() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"path": "a.txt"})).await;
        assert!(out.is_error);
        assert!(
            out.content
                .starts_with("Invalid arguments for tool `write`"),
            "{}",
            out.content
        );
    }

    #[test]
    fn schema_requires_path_and_content() {
        let schema = WriteTool::new().input_schema();
        assert_eq!(schema["required"], json!(["path", "content"]));
    }
}
