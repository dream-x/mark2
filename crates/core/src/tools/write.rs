//! Тул `write`: создать файл или перезаписать его целиком.
//!
//! Родительские директории создаются молча — модель, которой пришлось бы
//! сначала звать `bash mkdir -p`, тратит лишний ход на каждый новый файл.
//!
//! Пишется только внутри cwd: цель проходит замок мутирующих тулов
//! (`tools::confine`, общий с `edit`) — абсолютный путь наружу, `..` и
//! симлинк наружу отбиваются явным «outside the working directory», висячий
//! симлинк — своим текстом, и пишется ровно резолвленный путь. Иначе подтверждение «write
//! notes.md» в `ask_writes` одобряло бы запись в `~/.zshrc`, если `notes.md` —
//! симлинк туда.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::confine::{self, Target};
use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Аргументы тула `write`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteInput {
    /// Path to the file inside the working directory, relative or absolute.
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

        // В текстах — путь, как его видит модель; пишется — резолвленный,
        // прошедший замок.
        let display_path = ctx.resolve(&input.path);
        let display = display_path.display();

        let Target { path, metadata } =
            match confine::resolve_target("write", ctx, &input.path).await {
                Ok(target) => target,
                Err(message) => return ToolOutput::err(message),
            };

        if metadata.as_ref().is_some_and(std::fs::Metadata::is_dir) {
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

        let existed = metadata.is_some();
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

    /// Абсолютный путь внутри cwd — законен. Раньше тест назывался
    /// `absolute_paths_are_used_as_is` и писал в чужую временную директорию —
    /// ровно ту дыру, которую закрыл замок; запись наружу — в
    /// `absolute_path_outside_is_refused`.
    #[tokio::test]
    async fn absolute_paths_inside_the_cwd_are_allowed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("abs.txt");
        let out = run(
            &dir,
            json!({"path": path.to_str().unwrap(), "content": "x"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "x");
    }

    /// `parent/work` — cwd; `parent/` и всё в нём, кроме `work`, — снаружи.
    fn sandbox() -> (TempDir, ToolCtx) {
        let parent = TempDir::new().unwrap();
        std::fs::create_dir(parent.path().join("work")).unwrap();
        std::fs::write(parent.path().join("secret.txt"), "untouched").unwrap();
        std::fs::create_dir(parent.path().join("outside_dir")).unwrap();
        let ctx = ToolCtx::new(parent.path().join("work"));
        (parent, ctx)
    }

    fn assert_outside(out: &ToolOutput, requested: &str) {
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.starts_with(&format!(
                "Path {requested} is outside the working directory"
            )),
            "{}",
            out.content
        );
        assert!(
            out.content
                .contains("`write` only changes files inside the working directory"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn parent_dir_escape_is_refused() {
        let (parent, ctx) = sandbox();
        let tool = WriteTool::new();
        for requested in ["../escape.txt", "sub/../../escape.txt", "../secret.txt"] {
            let out = tool
                .execute(json!({"path": requested, "content": "pwned"}), &ctx)
                .await;
            assert_outside(&out, requested);
        }
        assert!(!parent.path().join("escape.txt").exists());
        assert!(!parent.path().join("work/sub").exists(), "лишний каталог");
        assert_eq!(
            std::fs::read_to_string(parent.path().join("secret.txt")).unwrap(),
            "untouched"
        );
    }

    #[tokio::test]
    async fn absolute_path_outside_is_refused() {
        let (parent, ctx) = sandbox();
        for target in [
            parent.path().join("escape.txt"),
            parent.path().join("secret.txt"),
            parent.path().join("new_dir/escape.txt"),
        ] {
            let requested = target.to_str().unwrap();
            let out = WriteTool::new()
                .execute(json!({"path": requested, "content": "pwned"}), &ctx)
                .await;
            assert_outside(&out, requested);
        }
        assert!(!parent.path().join("escape.txt").exists());
        assert!(!parent.path().join("new_dir").exists());
        assert_eq!(
            std::fs::read_to_string(parent.path().join("secret.txt")).unwrap(),
            "untouched"
        );
    }

    /// Сценарий аудита: `notes.md -> ~/.zshrc` в репозитории. Подтверждение
    /// «write notes.md» не должно писать в `.zshrc` — ни в существующий, ни
    /// в ещё не созданный (висячий симлинк).
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_file_pointing_outside_is_refused() {
        use std::os::unix::fs::symlink;
        let (parent, ctx) = sandbox();
        symlink(parent.path().join("secret.txt"), ctx.cwd.join("notes.md")).unwrap();
        symlink(parent.path().join("zshrc"), ctx.cwd.join("dangling.md")).unwrap();

        let out = WriteTool::new()
            .execute(json!({"path": "notes.md", "content": "pwned"}), &ctx)
            .await;
        assert_outside(&out, "notes.md");
        assert_eq!(
            std::fs::read_to_string(parent.path().join("secret.txt")).unwrap(),
            "untouched"
        );

        let out = WriteTool::new()
            .execute(json!({"path": "dangling.md", "content": "pwned"}), &ctx)
            .await;
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("symlink whose target does not exist"),
            "{}",
            out.content
        );
        assert!(!parent.path().join("zshrc").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_directory_pointing_outside_is_refused() {
        let (parent, ctx) = sandbox();
        std::os::unix::fs::symlink(parent.path().join("outside_dir"), ctx.cwd.join("docs"))
            .unwrap();
        for requested in ["docs/new.txt", "docs/deeper/new.txt"] {
            let out = WriteTool::new()
                .execute(json!({"path": requested, "content": "pwned"}), &ctx)
                .await;
            assert_outside(&out, requested);
        }
        let leaked: Vec<_> = std::fs::read_dir(parent.path().join("outside_dir"))
            .unwrap()
            .collect();
        assert!(leaked.is_empty(), "{leaked:?}");
    }

    /// Симлинк внутри cwd, ведущий внутрь, — законен: пишется его цель.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inside_the_cwd_writes_its_target() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("real.txt"), "old").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("link.txt"))
            .unwrap();
        let out = run(&dir, json!({"path": "link.txt", "content": "new"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("Overwrote "), "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("real.txt")).unwrap(),
            "new"
        );
        assert!(
            std::fs::symlink_metadata(dir.path().join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "симлинк остался симлинком"
        );
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
        // Граница видна модели заранее, а не только из отказа.
        assert!(
            schema["properties"]["path"]["description"]
                .as_str()
                .unwrap()
                .contains("inside the working directory"),
            "{schema}"
        );
    }
}
