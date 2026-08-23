//! Тул `edit` в двух форматах правки — [`EditStrategy`].
//!
//! Формат выбирается под модель (урок aider): сильные модели точно
//! воспроизводят фрагмент файла и правят его точечным search/replace, слабые
//! локалки на этом сыплются, зато уверенно выдают файл целиком. Имя тула,
//! `edit`, в обоих случаях одно — меняются только схема и описание, поэтому
//! остальное ядро (и промпт) о стратегии не знает.
//!
//! Оба формата работают только с существующим файлом: создание — это `write`, и
//! ошибка прямо на это указывает.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Формат правки файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditStrategy {
    /// Точное search/replace: модель присылает фрагмент до и после.
    #[default]
    SearchReplace,
    /// Файл целиком: для моделей, которые не воспроизводят фрагмент точно.
    WholeFile,
}

/// Аргументы `edit` в режиме [`EditStrategy::SearchReplace`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchReplaceInput {
    /// Path to an existing file, absolute or relative to the working directory.
    pub path: String,
    /// Exact text to find, copied verbatim from the file including indentation.
    /// It must match exactly once unless `replace_all` is true.
    pub old_string: String,
    /// Text to put in its place. Use an empty string to delete.
    pub new_string: String,
    /// Replace every occurrence instead of requiring a unique match.
    #[serde(default)]
    pub replace_all: Option<bool>,
}

/// Аргументы `edit` в режиме [`EditStrategy::WholeFile`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WholeFileInput {
    /// Path to an existing file, absolute or relative to the working directory.
    pub path: String,
    /// Complete new contents of the file, not a fragment or a diff.
    pub content: String,
}

/// Тул правки файлов.
#[derive(Debug, Clone, Copy, Default)]
pub struct EditTool {
    strategy: EditStrategy,
}

impl EditTool {
    /// Тул с заданным форматом правки.
    pub fn new(strategy: EditStrategy) -> Self {
        Self { strategy }
    }

    /// Выбранный формат правки.
    pub fn strategy(&self) -> EditStrategy {
        self.strategy
    }

    async fn search_replace(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        let input: SearchReplaceInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid arguments for tool `edit`: {error}. Expected \
                     {{\"path\": \"...\", \"old_string\": \"...\", \"new_string\": \"...\"}}."
                ));
            }
        };

        let path = ctx.resolve(&input.path);
        let display = path.display();

        if input.old_string.is_empty() {
            return ToolOutput::err(format!(
                "`old_string` is empty. It must be the exact text to replace in {display}; \
                 to create or fully replace a file use the `write` tool."
            ));
        }
        if input.old_string == input.new_string {
            return ToolOutput::err(
                "`old_string` and `new_string` are identical, so the edit would change nothing."
                    .to_string(),
            );
        }

        let contents = match read_existing(&path, ctx).await {
            Ok(contents) => contents,
            Err(output) => return output,
        };

        let matches = contents.matches(&input.old_string).count();
        let replace_all = input.replace_all.unwrap_or(false);

        if matches == 0 {
            return ToolOutput::err(format!(
                "`old_string` was not found in {display}. The text must match the file exactly, \
                 including whitespace and indentation — read the file again and copy the snippet verbatim."
            ));
        }
        if matches > 1 && !replace_all {
            return ToolOutput::err(format!(
                "Found {matches} matches for `old_string` in {display}, expected exactly 1. \
                 Include more surrounding lines to make the snippet unique, \
                 or set \"replace_all\": true to change all {matches} occurrences."
            ));
        }

        let updated = if replace_all {
            contents.replace(&input.old_string, &input.new_string)
        } else {
            contents.replacen(&input.old_string, &input.new_string, 1)
        };

        match tokio::fs::write(&path, updated.as_bytes()).await {
            Ok(()) => {
                let replaced = if replace_all { matches } else { 1 };
                let plural = if replaced == 1 { "" } else { "s" };
                ToolOutput::ok(format!(
                    "Replaced {replaced} occurrence{plural} in {display}"
                ))
            }
            Err(error) => ToolOutput::err(format!("Cannot write {display}: {error}")),
        }
    }

    async fn whole_file(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        let input: WholeFileInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid arguments for tool `edit`: {error}. Expected \
                     {{\"path\": \"...\", \"content\": \"...\"}}."
                ));
            }
        };

        let path = ctx.resolve(&input.path);
        let display = path.display();

        let contents = match read_existing(&path, ctx).await {
            Ok(contents) => contents,
            Err(output) => return output,
        };

        if contents == input.content {
            return ToolOutput::err(format!(
                "`content` is identical to the current contents of {display}, \
                 so the edit would change nothing."
            ));
        }

        let before = contents.len();
        let after = input.content.len();

        match tokio::fs::write(&path, input.content.as_bytes()).await {
            Ok(()) => ToolOutput::ok(format!("Rewrote {display} ({after} bytes, was {before})")),
            Err(error) => ToolOutput::err(format!("Cannot write {display}: {error}")),
        }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        match self.strategy {
            EditStrategy::SearchReplace => {
                "Replace an exact snippet in an existing file. `old_string` must be copied \
                 verbatim from the file and match exactly once unless `replace_all` is set. \
                 Use `write` to create a new file."
            }
            EditStrategy::WholeFile => {
                "Rewrite an existing file with its complete new contents. \
                 Send the whole file, never a fragment or a diff. \
                 Use `write` to create a new file."
            }
        }
    }

    fn input_schema(&self) -> Value {
        match self.strategy {
            EditStrategy::SearchReplace => input_schema_for::<SearchReplaceInput>(),
            EditStrategy::WholeFile => input_schema_for::<WholeFileInput>(),
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        match self.strategy {
            EditStrategy::SearchReplace => self.search_replace(input, ctx).await,
            EditStrategy::WholeFile => self.whole_file(input, ctx).await,
        }
    }
}

/// Прочитать файл, который обязан существовать; иначе — готовая ошибка модели.
async fn read_existing(path: &std::path::Path, ctx: &ToolCtx) -> Result<String, ToolOutput> {
    let display = path.display();
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(ToolOutput::err(format!(
                "File not found: {display}. `edit` only changes existing files — \
             create it with the `write` tool, or check the path \
             (relative paths resolve against {}).",
                ctx.cwd.display()
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::IsADirectory => Err(ToolOutput::err(
            format!("{display} is a directory, not a file."),
        )),
        Err(error) => Err(ToolOutput::err(format!("Cannot read {display}: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn tool() -> EditTool {
        EditTool::new(EditStrategy::SearchReplace)
    }

    async fn run(tool: EditTool, dir: &TempDir, input: Value) -> ToolOutput {
        tool.execute(input, &ToolCtx::new(dir.path())).await
    }

    fn seed(dir: &TempDir, body: &str) -> std::path::PathBuf {
        let path = dir.path().join("a.rs");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn strategy_is_reported_and_defaults_to_search_replace() {
        assert_eq!(tool().strategy(), EditStrategy::SearchReplace);
        assert_eq!(EditStrategy::default(), EditStrategy::SearchReplace);
        assert_eq!(EditTool::default().strategy(), EditStrategy::SearchReplace);
    }

    #[test]
    fn both_strategies_share_the_name_but_not_the_schema() {
        let search = tool();
        let whole = EditTool::new(EditStrategy::WholeFile);
        assert_eq!(search.name(), whole.name());
        assert_ne!(search.description(), whole.description());
        assert_eq!(
            search.input_schema()["required"],
            json!(["path", "old_string", "new_string"])
        );
        assert_eq!(whole.input_schema()["required"], json!(["path", "content"]));
    }

    #[tokio::test]
    async fn replaces_a_unique_occurrence() {
        let dir = TempDir::new().unwrap();
        let path = seed(&dir, "fn a() {}\nfn b() {}\n");
        let out = run(
            tool(),
            &dir,
            json!({"path": "a.rs", "old_string": "fn b() {}", "new_string": "fn c() {}"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.starts_with("Replaced 1 occurrence in"),
            "{}",
            out.content
        );
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "fn a() {}\nfn c() {}\n"
        );
    }

    #[tokio::test]
    async fn deletes_when_new_string_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = seed(&dir, "keep\ndrop\n");
        let out = run(
            tool(),
            &dir,
            json!({"path": "a.rs", "old_string": "drop\n", "new_string": ""}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep\n");
    }

    #[tokio::test]
    async fn missing_old_string_explains_how_to_fix_it() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "fn a() {}\n");
        let out = run(
            tool(),
            &dir,
            json!({"path": "a.rs", "old_string": "fn zzz() {}", "new_string": "x"}),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("`old_string` was not found"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("copy the snippet verbatim"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn ambiguous_old_string_reports_the_match_count() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "todo\ntodo\ntodo\n");
        let out = run(
            tool(),
            &dir,
            json!({"path": "a.rs", "old_string": "todo", "new_string": "done"}),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Found 3 matches for `old_string`"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("expected exactly 1"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("\"replace_all\": true"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn replace_all_changes_every_occurrence() {
        let dir = TempDir::new().unwrap();
        let path = seed(&dir, "todo\ntodo\ntodo\n");
        let out = run(
            tool(),
            &dir,
            json!({
                "path": "a.rs",
                "old_string": "todo",
                "new_string": "done",
                "replace_all": true,
            }),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.starts_with("Replaced 3 occurrences in"),
            "{}",
            out.content
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "done\ndone\ndone\n");
    }

    #[tokio::test]
    async fn missing_file_points_at_write() {
        let dir = TempDir::new().unwrap();
        let out = run(
            tool(),
            &dir,
            json!({"path": "nope.rs", "old_string": "a", "new_string": "b"}),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("File not found:"),
            "{}",
            out.content
        );
        assert!(out.content.contains("`write` tool"), "{}", out.content);
    }

    #[tokio::test]
    async fn empty_old_string_is_rejected() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "body");
        let out = run(
            tool(),
            &dir,
            json!({"path": "a.rs", "old_string": "", "new_string": "x"}),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("`old_string` is empty"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn identical_strings_are_rejected() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "body");
        let out = run(
            tool(),
            &dir,
            json!({"path": "a.rs", "old_string": "body", "new_string": "body"}),
        )
        .await;
        assert!(out.is_error);
        assert!(out.content.contains("identical"), "{}", out.content);
    }

    #[tokio::test]
    async fn directory_target_is_rejected() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let out = run(
            tool(),
            &dir,
            json!({"path": "sub", "old_string": "a", "new_string": "b"}),
        )
        .await;
        assert!(out.is_error);
        assert!(!out.content.is_empty());
    }

    #[tokio::test]
    async fn search_replace_rejects_garbage_input() {
        let dir = TempDir::new().unwrap();
        let out = run(tool(), &dir, json!({"path": "a.rs"})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Invalid arguments for tool `edit`"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn whole_file_rewrites_an_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = seed(&dir, "old body\n");
        let out = run(
            EditTool::new(EditStrategy::WholeFile),
            &dir,
            json!({"path": "a.rs", "content": "brand new body\n"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("(15 bytes, was 9)"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "brand new body\n");
    }

    #[tokio::test]
    async fn whole_file_refuses_to_create_files() {
        let dir = TempDir::new().unwrap();
        let out = run(
            EditTool::new(EditStrategy::WholeFile),
            &dir,
            json!({"path": "nope.rs", "content": "x"}),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("File not found:"),
            "{}",
            out.content
        );
        assert!(out.content.contains("`write` tool"), "{}", out.content);
    }

    #[tokio::test]
    async fn whole_file_rejects_a_no_op_rewrite() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "same\n");
        let out = run(
            EditTool::new(EditStrategy::WholeFile),
            &dir,
            json!({"path": "a.rs", "content": "same\n"}),
        )
        .await;
        assert!(out.is_error);
        assert!(out.content.contains("identical"), "{}", out.content);
    }

    #[tokio::test]
    async fn whole_file_rejects_garbage_input() {
        let dir = TempDir::new().unwrap();
        let out = run(
            EditTool::new(EditStrategy::WholeFile),
            &dir,
            json!({"path": "a.rs", "old_string": "a"}),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Invalid arguments for tool `edit`"),
            "{}",
            out.content
        );
    }
}
