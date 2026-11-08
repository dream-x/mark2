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
//!
//! Правится только внутри cwd: путь проходит тот же замок мутирующих тулов,
//! что у `write` (`tools::confine`), — наружу ни абсолютным путём, ни `..`, ни
//! симлинком; симлинк внутри, ведущий внутрь, правит свою цель. Путь снаружи
//! отбивается явным «outside the working directory» ещё до проверки
//! существования: «File not found … create it with `write`» для него было бы
//! ложной подсказкой.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::confine::{self, Target};
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

impl EditStrategy {
    /// Все форматы — в порядке, в котором их перечисляют ошибки.
    pub const ALL: [EditStrategy; 2] = [EditStrategy::SearchReplace, EditStrategy::WholeFile];

    /// Каноническое имя в конфиге (`search_replace` | `whole_file`).
    pub fn as_str(self) -> &'static str {
        match self {
            EditStrategy::SearchReplace => "search_replace",
            EditStrategy::WholeFile => "whole_file",
        }
    }
}

impl std::fmt::Display for EditStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Неизвестное имя формата правки; текст перечисляет допустимые.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEditStrategy(pub String);

impl std::fmt::Display for UnknownEditStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known: Vec<&str> = EditStrategy::ALL.iter().map(|s| s.as_str()).collect();
        write!(
            f,
            "неизвестная edit_strategy `{}`, ожидается {}",
            self.0,
            known.join(" или ")
        )
    }
}

impl std::error::Error for UnknownEditStrategy {}

/// Разбор имени формата — единый для конфига mark2 и матрицы бенча. Терпит
/// регистр, пробелы по краям и дефисы вместо подчёркиваний (`Whole-File`).
impl std::str::FromStr for EditStrategy {
    type Err = UnknownEditStrategy;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let canonical = raw.trim().to_ascii_lowercase().replace('-', "_");
        EditStrategy::ALL
            .into_iter()
            .find(|strategy| strategy.as_str() == canonical)
            .ok_or_else(|| UnknownEditStrategy(raw.to_string()))
    }
}

/// Аргументы `edit` в режиме [`EditStrategy::SearchReplace`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchReplaceInput {
    /// Path to an existing file inside the working directory, relative or absolute.
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
    /// Path to an existing file inside the working directory, relative or absolute.
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

        let (target, contents) = match read_existing(&input.path, ctx).await {
            Ok(existing) => existing,
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

        match tokio::fs::write(&target, updated.as_bytes()).await {
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

        let (target, contents) = match read_existing(&input.path, ctx).await {
            Ok(existing) => existing,
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

        match tokio::fs::write(&target, input.content.as_bytes()).await {
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

/// Пропустить `requested` через замок мутирующих тулов и прочитать файл,
/// который обязан существовать; иначе — готовая ошибка модели. Отдаёт
/// резолвленный путь — писать правку нужно по нему, — и содержимое.
async fn read_existing(requested: &str, ctx: &ToolCtx) -> Result<(PathBuf, String), ToolOutput> {
    // В текстах — путь, как его видит модель, а не резолвленный.
    let display_path = ctx.resolve(requested);
    let display = display_path.display();
    let not_found = || {
        ToolOutput::err(format!(
            "File not found: {display}. `edit` only changes existing files — \
             create it with the `write` tool, or check the path \
             (relative paths resolve against {}).",
            ctx.cwd.display()
        ))
    };
    let is_a_directory = || ToolOutput::err(format!("{display} is a directory, not a file."));

    let Target { path, metadata } = confine::resolve_target("edit", ctx, requested)
        .await
        .map_err(ToolOutput::err)?;
    match &metadata {
        None => return Err(not_found()),
        Some(metadata) if metadata.is_dir() => return Err(is_a_directory()),
        Some(_) => {}
    }

    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok((path, String::from_utf8_lossy(&bytes).into_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(not_found()),
        Err(error) if error.kind() == std::io::ErrorKind::IsADirectory => Err(is_a_directory()),
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
    fn strategy_names_round_trip_and_unknown_names_list_the_known_ones() {
        for strategy in EditStrategy::ALL {
            assert_eq!(strategy.as_str().parse::<EditStrategy>(), Ok(strategy));
            assert_eq!(strategy.to_string(), strategy.as_str());
        }
        assert_eq!(
            " Whole-File ".parse::<EditStrategy>(),
            Ok(EditStrategy::WholeFile)
        );
        let error = "diff".parse::<EditStrategy>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "неизвестная edit_strategy `diff`, ожидается search_replace или whole_file"
        );
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

    /// `parent/work` — cwd; `parent/secret.rs` — снаружи.
    fn sandbox() -> (TempDir, ToolCtx) {
        let parent = TempDir::new().unwrap();
        std::fs::create_dir(parent.path().join("work")).unwrap();
        std::fs::write(parent.path().join("secret.rs"), "fn secret() {}\n").unwrap();
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
                .contains("`edit` only changes files inside the working directory"),
            "{}",
            out.content
        );
    }

    /// Оба формата правки: `..`, абсолютный путь наружу и путь снаружи,
    /// которого нет, — явное «outside», а не «File not found … use `write`».
    #[tokio::test]
    async fn paths_outside_the_cwd_are_refused_in_both_strategies() {
        let (parent, ctx) = sandbox();
        let absolute = parent.path().join("secret.rs");
        let absolute = absolute.to_str().unwrap();
        for requested in [
            "../secret.rs",
            absolute,
            "../missing.rs",
            "ghost/../../secret.rs",
        ] {
            let out = tool()
                .execute(
                    json!({"path": requested, "old_string": "secret", "new_string": "pwned"}),
                    &ctx,
                )
                .await;
            assert_outside(&out, requested);

            let out = EditTool::new(EditStrategy::WholeFile)
                .execute(json!({"path": requested, "content": "pwned"}), &ctx)
                .await;
            assert_outside(&out, requested);
        }
        assert_eq!(
            std::fs::read_to_string(parent.path().join("secret.rs")).unwrap(),
            "fn secret() {}\n"
        );
        assert!(!parent.path().join("missing.rs").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_pointing_outside_are_refused() {
        use std::os::unix::fs::symlink;
        let (parent, ctx) = sandbox();
        symlink(parent.path().join("secret.rs"), ctx.cwd.join("notes.rs")).unwrap();
        symlink(parent.path(), ctx.cwd.join("up")).unwrap();
        for requested in ["notes.rs", "up/secret.rs"] {
            let out = tool()
                .execute(
                    json!({"path": requested, "old_string": "secret", "new_string": "pwned"}),
                    &ctx,
                )
                .await;
            assert_outside(&out, requested);
        }
        assert_eq!(
            std::fs::read_to_string(parent.path().join("secret.rs")).unwrap(),
            "fn secret() {}\n"
        );
    }

    /// Симлинк внутри cwd, ведущий внутрь, — законен: правится его цель, а сам
    /// симлинк остаётся симлинком.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inside_the_cwd_edits_its_target() {
        let dir = TempDir::new().unwrap();
        let real = seed(&dir, "fn a() {}\n");
        std::os::unix::fs::symlink(&real, dir.path().join("link.rs")).unwrap();

        let out = run(
            tool(),
            &dir,
            json!({"path": "link.rs", "old_string": "fn a", "new_string": "fn b"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "fn b() {}\n");

        let out = run(
            EditTool::new(EditStrategy::WholeFile),
            &dir,
            json!({"path": "link.rs", "content": "fn c() {}\n"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "fn c() {}\n");
        assert!(
            std::fs::symlink_metadata(dir.path().join("link.rs"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// Абсолютный путь внутри cwd — по-прежнему законен.
    #[tokio::test]
    async fn absolute_path_inside_the_cwd_is_allowed() {
        let dir = TempDir::new().unwrap();
        let path = seed(&dir, "old\n");
        let out = run(
            tool(),
            &dir,
            json!({"path": path.to_str().unwrap(), "old_string": "old", "new_string": "new"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new\n");
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
