//! Тул `grep`: read-only поиск по содержимому файлов регулярным выражением.
//!
//! Нужен прежде всего read-only сессиям (ревьювер `scripts/review.sh`): без
//! него модель может только `read` по точному пути и тянется к `bash`, который
//! политика отбивает. Поэтому тул намеренно маленький — три параметра, вывод
//! `путь:строка: текст`, — и весь его вес в ограничителях:
//!
//! * **Только внутри cwd.** И cwd, и `path` канонизируются (`..` и симлинки
//!   раскрыты), результат обязан лежать внутри cwd — замок общий с `read`
//!   (модуль `tools::confine`). Обход симлинки не разыменовывает вовсе — ни
//!   на файлы, ни на директории: так нельзя ни выйти наружу, ни зациклиться.
//! * **Шум не ищется.** Скрытые файлы и директории (имя с точки, включая
//!   `.git`), `node_modules` и директории-кэши с `CACHEDIR.TAG` (так помечен
//!   `target/` у cargo) пропускаются; бинарные файлы (NUL в первом блоке) —
//!   тоже. Корень поиска, названный явно, не фильтруется: `path: ".github"`
//!   работает.
//! * **Капы ради контекста модели**: число совпадений ([`MAX_MATCHES`]),
//!   суммарный вывод ([`MAX_OUTPUT_BYTES`], как у `bash`), длина показанной
//!   строки ([`MAX_LINE_BYTES`], окно вокруг совпадения) и сколько байт файла
//!   вообще читаем ([`MAX_FILE_BYTES`], как у `read`). Любая обрезка помечается
//!   маркером — молча укоротить результат хуже, чем сказать «дальше есть ещё».
//!
//! Обход синхронный (`std::fs`) и уходит в `spawn_blocking`, чтобы не
//! занимать поток рантайма; отмена хода проверяется перед каждым файлом.
//! Порядок вывода детерминирован: записи директории сортируются по имени.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use regex::{Regex, RegexBuilder};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::confine::{CanonicalCwd, ConfineError};
use super::{Tool, ToolCtx, ToolOutput, input_schema_for};

/// Сколько совпадений максимум отдаём модели.
pub const MAX_MATCHES: usize = 200;
/// Сколько байт вывода максимум уходит модели (как у `bash`).
pub const MAX_OUTPUT_BYTES: usize = 30_000;
/// Сколько байт строки показываем; длиннее — окно вокруг совпадения.
pub const MAX_LINE_BYTES: usize = 300;
/// Сколько байт файла максимум читаем (как у `read`); хвост больших файлов не
/// ищется, и это помечается.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Первый блок файла, в котором NUL означает «бинарный».
const BINARY_PROBE_BYTES: u64 = 8 * 1024;
/// Сколько байт до совпадения оставляем, когда строка режется окном.
const LINE_CONTEXT_BYTES: usize = 100;
/// Директории, которые не обходятся никогда (кроме явно названного корня).
const SKIPPED_DIRS: [&str; 1] = ["node_modules"];
/// Маркер директории-кэша (<https://bford.info/cachedir/>): им помечен
/// `target/` у cargo.
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

/// Аргументы тула `grep`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GrepInput {
    /// Regex (Rust syntax) matched against each line.
    pub pattern: String,
    /// File or directory to search, relative to the working directory (the default).
    #[serde(default)]
    pub path: Option<String>,
    /// Ignore case.
    #[serde(default)]
    pub case_insensitive: Option<bool>,
}

/// Тул поиска по файлам.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrepTool;

impl GrepTool {
    /// Новый тул.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents for a regex, line by line; directories are searched recursively, \
         skipping hidden files, binaries and build caches. Returns `path:line: text`, \
         at most 200 matches."
    }

    fn input_schema(&self) -> Value {
        input_schema_for::<GrepInput>()
    }

    async fn execute(&self, input: Value, ctx: &ToolCtx) -> ToolOutput {
        let input: GrepInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid arguments for tool `grep`: {error}. Expected {{\"pattern\": \"...\"}}."
                ));
            }
        };

        if input.pattern.is_empty() {
            return ToolOutput::err(
                "`pattern` is empty: pass the regular expression to search for.",
            );
        }

        let regex = match RegexBuilder::new(&input.pattern)
            .case_insensitive(input.case_insensitive.unwrap_or(false))
            .build()
        {
            Ok(regex) => regex,
            Err(error) => {
                return ToolOutput::err(format!(
                    "Invalid regex `{}`: {error}\nEscape special characters such as `(`, `[`, \
                     `{{`, `.`, `*` with a backslash to match them literally, then call `grep` again.",
                    input.pattern
                ));
            }
        };

        if ctx.is_cancelled() {
            return ToolOutput::err("Search was cancelled before it started.");
        }

        // Канонизация и обход — блокирующие `std::fs`, им место вне потока
        // рантайма.
        let ctx = ctx.clone();
        let task = move || {
            let root = match resolve_inside_cwd(&ctx, input.path.as_deref()) {
                Ok(root) => root,
                Err(message) => return ToolOutput::err(message),
            };
            search(&regex, root, ctx.cancel).into_output(&input.pattern)
        };
        match tokio::task::spawn_blocking(task).await {
            Ok(output) => output,
            Err(error) => ToolOutput::err(format!("Search failed: {error}")),
        }
    }
}

/// Корень поиска: канонизированные cwd и `path`, причём `path` прошёл замок
/// [`CanonicalCwd::confine`] — внутри cwd, обычный файл или директория.
/// `Err` — готовый текст ошибки для модели.
fn resolve_inside_cwd(ctx: &ToolCtx, path: Option<&str>) -> Result<Root, String> {
    let cwd = CanonicalCwd::new(&ctx.cwd).map_err(|error| {
        format!(
            "Cannot search: the working directory {} is not accessible: {error}",
            ctx.cwd.display()
        )
    })?;

    let requested = match path.map(str::trim) {
        None | Some("") => {
            let cwd = cwd.into_path();
            return Ok(Root {
                path: cwd.clone(),
                cwd,
                is_dir: true,
            });
        }
        Some(path) => path,
    };

    let resolved = ctx.resolve(requested);
    let confined = cwd.confine(&resolved).map_err(|error| match error {
        ConfineError::NotFound => format!(
            "Path not found: {}. Pass a file or directory inside the working directory {} \
             (relative paths resolve against it), or omit `path` to search all of it.",
            resolved.display(),
            cwd.path().display()
        ),
        ConfineError::Unresolvable(error) => {
            format!("Cannot search {}: {error}", resolved.display())
        }
        ConfineError::Outside => format!(
            "Path {requested} is outside the working directory {}: `grep` only searches inside it. \
             Pass a path within the working directory or omit `path`.",
            cwd.path().display()
        ),
        ConfineError::NotRegular => {
            format!("{requested} is not a regular file or directory: `grep` cannot search it.")
        }
        ConfineError::Metadata(error) => format!("Cannot search {requested}: {error}"),
    })?;

    Ok(Root {
        is_dir: confined.metadata.is_dir(),
        cwd: cwd.into_path(),
        path: confined.path,
    })
}

/// Канонизированные cwd и корень поиска внутри него.
struct Root {
    cwd: PathBuf,
    path: PathBuf,
    is_dir: bool,
}

/// Итог поиска: найденные строки и всё, что надо сказать о пропусках.
#[derive(Debug, Default)]
struct Report {
    /// Отображаемый корень (относительно cwd), для текста «ничего не найдено».
    root_display: String,
    output: String,
    matches: usize,
    files_searched: usize,
    binary_skipped: usize,
    unreadable: usize,
    /// Файлы, у которых искали только первые [`MAX_FILE_BYTES`].
    clipped_files: usize,
    /// Упёрлись в [`MAX_MATCHES`].
    match_cap_hit: bool,
    /// Упёрлись в [`MAX_OUTPUT_BYTES`].
    output_cap_hit: bool,
    cancelled: bool,
}

impl Report {
    /// Пора ли прекращать обход.
    fn stopped(&self) -> bool {
        self.match_cap_hit || self.output_cap_hit || self.cancelled
    }

    fn into_output(self, pattern: &str) -> ToolOutput {
        if self.cancelled {
            return ToolOutput::err("Search was cancelled.");
        }

        let mut markers: Vec<String> = Vec::new();
        if self.match_cap_hit {
            markers.push(format!(
                "[results truncated at {MAX_MATCHES} matches; narrow the pattern or the path]"
            ));
        }
        if self.output_cap_hit {
            markers.push(format!(
                "[output truncated at {MAX_OUTPUT_BYTES} bytes; narrow the pattern or the path]"
            ));
        }
        if self.clipped_files > 0 {
            markers.push(format!(
                "[only the first {MAX_FILE_BYTES} bytes of {} large file(s) were searched]",
                self.clipped_files
            ));
        }
        if self.unreadable > 0 {
            markers.push(format!("[{} file(s) could not be read]", self.unreadable));
        }

        if self.matches == 0 {
            let mut text = format!(
                "No matches for `{pattern}` in {} ({} file(s) searched",
                self.root_display, self.files_searched
            );
            if self.binary_skipped > 0 {
                text.push_str(&format!(", {} binary skipped", self.binary_skipped));
            }
            text.push_str(").");
            for marker in markers {
                text.push('\n');
                text.push_str(&marker);
            }
            return ToolOutput::ok(text);
        }

        // Каждая найденная строка заканчивается `\n`: маркеры встают следом,
        // а без маркеров снимаем только последний перевод строки — пробелы в
        // конце самой строки остаются как в файле.
        let mut output = self.output;
        if markers.is_empty() {
            output.pop();
        } else {
            output.push_str(&markers.join("\n"));
        }
        ToolOutput::ok(output)
    }
}

/// Обойти корень и собрать совпадения.
fn search(regex: &Regex, root: Root, cancel: CancellationToken) -> Report {
    let root_display = if root.path == root.cwd {
        "the working directory".to_string()
    } else {
        display_path(&root.cwd, &root.path)
    };
    let mut search = Search {
        regex,
        cwd: &root.cwd,
        cancel,
        report: Report {
            root_display,
            ..Report::default()
        },
    };
    if root.is_dir {
        search.walk(&root.path);
    } else {
        // Явно названный файл ищется как есть: фильтры обхода — для того, что
        // нашлось в директориях, а не для того, о чём модель попросила прямо.
        search.file(&root.path);
    }
    search.report
}

struct Search<'a> {
    regex: &'a Regex,
    cwd: &'a Path,
    cancel: CancellationToken,
    report: Report,
}

impl Search<'_> {
    /// Рекурсивный обход директории: записи по имени, скрытое и кэши мимо,
    /// симлинки не разыменовываются.
    fn walk(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            self.report.unreadable += 1;
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            if self.report.stopped() {
                return;
            }
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            // `DirEntry::file_type` не идёт по симлинкам: симлинк — ни файл,
            // ни директория, и пропускается.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                if is_skipped_dir(&path, &name.to_string_lossy()) {
                    continue;
                }
                self.walk(&path);
            } else if file_type.is_file() {
                self.file(&path);
            }
        }
    }

    /// Поиск в одном файле.
    fn file(&mut self, path: &Path) {
        if self.cancel.is_cancelled() {
            self.report.cancelled = true;
            return;
        }
        let Some((bytes, clipped)) = self.read_text(path) else {
            return;
        };
        self.report.files_searched += 1;
        if clipped {
            self.report.clipped_files += 1;
        }

        let text = String::from_utf8_lossy(&bytes);
        let display = display_path(self.cwd, path);
        for (index, line) in text.lines().enumerate() {
            let Some(found) = self.regex.find(line) else {
                continue;
            };
            if self.report.matches == MAX_MATCHES {
                self.report.match_cap_hit = true;
                return;
            }
            let entry = format!(
                "{display}:{}: {}\n",
                index + 1,
                clip_line(line, found.start())
            );
            if self.report.output.len() + entry.len() > MAX_OUTPUT_BYTES {
                self.report.output_cap_hit = true;
                return;
            }
            self.report.output.push_str(&entry);
            self.report.matches += 1;
        }
    }

    /// Прочитать файл не длиннее [`MAX_FILE_BYTES`]. `None` — бинарный или
    /// нечитаемый (учтён в отчёте); `Some((байты, обрезан ли))`.
    fn read_text(&mut self, path: &Path) -> Option<(Vec<u8>, bool)> {
        let Ok(mut file) = File::open(path) else {
            self.report.unreadable += 1;
            return None;
        };
        let mut bytes = Vec::new();
        // Сначала первый блок: бинарный файл отсеивается, не читаясь целиком.
        if (&mut file)
            .take(BINARY_PROBE_BYTES)
            .read_to_end(&mut bytes)
            .is_err()
        {
            self.report.unreadable += 1;
            return None;
        }
        if bytes.contains(&0) {
            self.report.binary_skipped += 1;
            return None;
        }
        // Читаем на байт больше капа: так видно, что файл длиннее, без
        // отдельного `metadata` (размер мог и поменяться между вызовами).
        let rest = MAX_FILE_BYTES + 1 - bytes.len() as u64;
        if (&mut file).take(rest).read_to_end(&mut bytes).is_err() {
            self.report.unreadable += 1;
            return None;
        }
        let clipped = bytes.len() as u64 > MAX_FILE_BYTES;
        if clipped {
            // Не обрезаем по границе строки: последняя неполная строка всё
            // равно ищется, а файл помечен как прочитанный не целиком.
            bytes.truncate(MAX_FILE_BYTES as usize);
        }
        Some((bytes, clipped))
    }
}

/// Директория, которую обход пропускает: `node_modules` и кэши с `CACHEDIR.TAG`.
fn is_skipped_dir(path: &Path, name: &str) -> bool {
    SKIPPED_DIRS.contains(&name) || path.join(CACHEDIR_TAG).is_file()
}

/// Путь относительно cwd. Обход симлинки не разыменовывает, так что всё
/// найденное лежит под канонизированным cwd; ветка `Err` — на всякий случай.
fn display_path(cwd: &Path, path: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// Строка для вывода: целиком, если короткая, иначе окно [`MAX_LINE_BYTES`]
/// вокруг совпадения с `…` на обрезанных краях.
fn clip_line(line: &str, match_start: usize) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return line.to_string();
    }
    let mut start = match_start.saturating_sub(LINE_CONTEXT_BYTES);
    // Окно не должно выходить за конец строки: тогда сдвигаем его влево.
    start = start.min(line.len() - MAX_LINE_BYTES);
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + MAX_LINE_BYTES).min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let mut clipped = String::with_capacity(end - start + 8);
    if start > 0 {
        clipped.push('…');
    }
    clipped.push_str(&line[start..end]);
    if end < line.len() {
        clipped.push('…');
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn run(dir: &TempDir, input: Value) -> ToolOutput {
        GrepTool::new()
            .execute(input, &ToolCtx::new(dir.path()))
            .await
    }

    fn write(dir: &TempDir, path: &str, content: impl AsRef<[u8]>) {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn finds_matching_lines_with_path_and_line_number() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "a.rs",
            "fn main() {\n    let needle = 1;\n}\nneedle again\n",
        );
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            "a.rs:2:     let needle = 1;\na.rs:4: needle again"
        );
    }

    #[tokio::test]
    async fn pattern_is_a_regex() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "a.rs",
            "fn alpha() {}\nfn beta() {}\nlet alpha = 1;\n",
        );
        let out = run(&dir, json!({"pattern": r"^fn \w+\("})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "a.rs:1: fn alpha() {}\na.rs:2: fn beta() {}");
    }

    #[tokio::test]
    async fn case_insensitive_is_opt_in() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "Needle\nneedle\nNEEDLE\n");

        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert_eq!(out.content, "a.txt:2: needle");

        let out = run(&dir, json!({"pattern": "needle", "case_insensitive": true})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            "a.txt:1: Needle\na.txt:2: needle\na.txt:3: NEEDLE"
        );
    }

    #[tokio::test]
    async fn directories_are_searched_recursively_in_name_order() {
        let dir = TempDir::new().unwrap();
        write(&dir, "b.txt", "needle b\n");
        write(&dir, "a/deep/er/x.txt", "needle deep\n");
        write(&dir, "a/z.txt", "needle z\n");
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            "a/deep/er/x.txt:1: needle deep\na/z.txt:1: needle z\nb.txt:1: needle b"
        );
    }

    #[tokio::test]
    async fn path_narrows_the_search_to_a_directory_or_a_file() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "needle lib\n");
        write(&dir, "src/main.rs", "needle main\n");
        write(&dir, "tests/t.rs", "needle test\n");

        let out = run(&dir, json!({"pattern": "needle", "path": "src"})).await;
        assert_eq!(
            out.content,
            "src/lib.rs:1: needle lib\nsrc/main.rs:1: needle main"
        );

        let out = run(&dir, json!({"pattern": "needle", "path": "src/main.rs"})).await;
        assert_eq!(out.content, "src/main.rs:1: needle main");

        // Абсолютный путь внутри cwd — тоже можно; вывод всё равно относительный.
        let absolute = dir.path().join("tests").display().to_string();
        let out = run(&dir, json!({"pattern": "needle", "path": absolute})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "tests/t.rs:1: needle test");
    }

    #[tokio::test]
    async fn no_matches_is_an_explicit_answer() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "nothing here\n");
        write(&dir, "b.bin", b"needle\0\x01");
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            "No matches for `needle` in the working directory (1 file(s) searched, 1 binary skipped)."
        );

        let out = run(&dir, json!({"pattern": "absent", "path": "a.txt"})).await;
        assert_eq!(
            out.content,
            "No matches for `absent` in a.txt (1 file(s) searched)."
        );
    }

    #[tokio::test]
    async fn hidden_entries_git_and_build_caches_are_skipped() {
        let dir = TempDir::new().unwrap();
        write(&dir, ".git/config", "needle in git\n");
        write(&dir, ".hidden.txt", "needle in a hidden file\n");
        write(&dir, "src/.cache/x.txt", "needle in a nested hidden dir\n");
        write(&dir, "node_modules/pkg/index.js", "needle in deps\n");
        write(
            &dir,
            "target/CACHEDIR.TAG",
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        );
        write(&dir, "target/debug/out.txt", "needle in target\n");
        write(&dir, "src/visible.txt", "needle visible\n");

        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "src/visible.txt:1: needle visible");

        // Явно названный корень не фильтруется.
        let out = run(&dir, json!({"pattern": "needle", "path": ".git"})).await;
        assert_eq!(out.content, ".git/config:1: needle in git");
        let out = run(&dir, json!({"pattern": "needle", "path": "target"})).await;
        assert_eq!(out.content, "target/debug/out.txt:1: needle in target");
    }

    #[tokio::test]
    async fn binary_files_are_skipped() {
        let dir = TempDir::new().unwrap();
        write(&dir, "blob.bin", b"\x7fELF\0\0needle\0");
        write(&dir, "text.txt", "needle\n");
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert_eq!(out.content, "text.txt:1: needle");

        // NUL далеко за первым блоком — файл считается текстовым.
        let mut late_nul = "needle early\n".to_string().into_bytes();
        late_nul.extend(std::iter::repeat_n(b'x', BINARY_PROBE_BYTES as usize));
        late_nul.extend(b"\0\n");
        write(&dir, "late.txt", late_nul);
        let out = run(&dir, json!({"pattern": "needle early"})).await;
        assert_eq!(out.content, "late.txt:1: needle early");
    }

    /// Конец последней строки — байт-в-байт как в файле.
    #[tokio::test]
    async fn trailing_whitespace_of_the_last_match_is_kept() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "needle  \r\n");
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert_eq!(out.content, "a.txt:1: needle  ");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fifo_root_is_rejected_instead_of_hanging() {
        let dir = TempDir::new().unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(dir.path().join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        write(&dir, "a.txt", "needle\n");

        let out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run(&dir, json!({"pattern": "needle", "path": "pipe"})),
        )
        .await
        .expect("grep повис на FIFO");
        assert!(out.is_error);
        assert!(
            out.content.contains("not a regular file or directory"),
            "{}",
            out.content
        );

        // При обходе FIFO просто пропускается.
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert_eq!(out.content, "a.txt:1: needle");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_are_not_followed() {
        let outside = TempDir::new().unwrap();
        write(&outside, "secret.txt", "needle outside\n");
        let dir = TempDir::new().unwrap();
        write(&dir, "inside.txt", "needle inside\n");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link_dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link_file.txt"),
        )
        .unwrap();

        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert_eq!(out.content, "inside.txt:1: needle inside");

        // Явный путь через симлинк наружу канонизируется и отбивается.
        let out = run(&dir, json!({"pattern": "needle", "path": "link_dir"})).await;
        assert!(out.is_error);
        assert!(
            out.content.contains("outside the working directory"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn paths_outside_the_cwd_are_rejected() {
        let parent = TempDir::new().unwrap();
        write(&parent, "secret.txt", "needle\n");
        write(&parent, "work/a.txt", "needle\n");
        std::fs::create_dir_all(parent.path().join("work/sub")).unwrap();
        let ctx = ToolCtx::new(parent.path().join("work"));

        for path in [
            "..".to_string(),
            "../secret.txt".to_string(),
            "sub/../../secret.txt".to_string(),
            parent.path().join("secret.txt").display().to_string(),
            "/".to_string(),
        ] {
            let out = GrepTool::new()
                .execute(json!({"pattern": "needle", "path": path}), &ctx)
                .await;
            assert!(out.is_error, "{path}: {}", out.content);
            assert!(
                out.content.contains("outside the working directory"),
                "{path}: {}",
                out.content
            );
            assert!(
                !out.content.contains("secret.txt:1"),
                "{path}: {}",
                out.content
            );
        }

        // `..` внутри cwd — законно.
        let out = GrepTool::new()
            .execute(json!({"pattern": "needle", "path": "sub/../a.txt"}), &ctx)
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "a.txt:1: needle");
    }

    #[tokio::test]
    async fn missing_path_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"pattern": "x", "path": "nope"})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Path not found:"),
            "{}",
            out.content
        );
        assert!(out.content.contains("nope"), "{}", out.content);
    }

    #[tokio::test]
    async fn invalid_regex_is_an_error_for_the_model() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "fn call(\n");
        let out = run(&dir, json!({"pattern": "call("})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Invalid regex `call(`:"),
            "{}",
            out.content
        );
        assert!(out.content.contains("backslash"), "{}", out.content);
        assert!(out.content.contains("call `grep` again"), "{}", out.content);

        // Исправленный вызов проходит.
        let out = run(&dir, json!({"pattern": r"call\("})).await;
        assert_eq!(out.content, "a.txt:1: fn call(");
    }

    #[tokio::test]
    async fn empty_pattern_and_garbage_input_are_reported() {
        let dir = TempDir::new().unwrap();
        let out = run(&dir, json!({"pattern": ""})).await;
        assert!(out.is_error);
        assert!(
            out.content.contains("`pattern` is empty"),
            "{}",
            out.content
        );

        let out = run(&dir, json!({"pattern": 7})).await;
        assert!(out.is_error);
        assert!(
            out.content.starts_with("Invalid arguments for tool `grep`"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn match_count_is_capped_with_a_marker() {
        let dir = TempDir::new().unwrap();
        let body: String = (0..MAX_MATCHES + 50)
            .map(|i| format!("hit {i}\n"))
            .collect();
        write(&dir, "a.txt", body);
        let out = run(&dir, json!({"pattern": "hit"})).await;
        assert!(!out.is_error, "{}", out.content);
        let hits = out
            .content
            .lines()
            .filter(|line| line.starts_with("a.txt:"))
            .count();
        assert_eq!(hits, MAX_MATCHES);
        assert!(
            out.content.ends_with(&format!(
                "[results truncated at {MAX_MATCHES} matches; narrow the pattern or the path]"
            )),
            "{}",
            out.content
        );

        // Ровно кап совпадений — без маркера: обрезать было нечего.
        let body: String = (0..MAX_MATCHES).map(|i| format!("hit {i}\n")).collect();
        write(&dir, "a.txt", body);
        let out = run(&dir, json!({"pattern": "hit"})).await;
        assert!(!out.content.contains("truncated"), "{}", out.content);
    }

    #[tokio::test]
    async fn output_size_is_capped_with_a_marker() {
        let dir = TempDir::new().unwrap();
        // 150 совпадений по ~260 байт — заметно больше капа вывода, но меньше
        // капа совпадений.
        let line = format!("hit {}\n", "x".repeat(250));
        write(&dir, "a.txt", line.repeat(150));
        let out = run(&dir, json!({"pattern": "hit"})).await;
        assert!(!out.is_error);
        assert!(
            out.content.ends_with(&format!(
                "[output truncated at {MAX_OUTPUT_BYTES} bytes; narrow the pattern or the path]"
            )),
            "tail: {}",
            &out.content[out.content.len() - 200..]
        );
        assert!(out.content.len() <= MAX_OUTPUT_BYTES + 100);
        assert!(out.content.len() > MAX_OUTPUT_BYTES - 300);
    }

    #[tokio::test]
    async fn long_lines_are_clipped_around_the_match() {
        let dir = TempDir::new().unwrap();
        let long = format!("{}needle{}", "a".repeat(1_000), "b".repeat(1_000));
        write(
            &dir,
            "min.js",
            format!("{long}\nneedle {}\n", "c".repeat(1_000)),
        );
        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert!(!out.is_error, "{}", out.content);

        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 2, "{}", out.content);
        // Совпадение в середине: окно с многоточиями по обоим краям.
        let first = lines[0].strip_prefix("min.js:1: ").unwrap();
        assert!(first.starts_with('…') && first.ends_with('…'), "{first}");
        assert!(first.contains("needle"), "{first}");
        assert!(first.len() <= MAX_LINE_BYTES + 2 * '…'.len_utf8());
        // Совпадение в начале: обрезан только хвост.
        let second = lines[1].strip_prefix("min.js:2: ").unwrap();
        assert!(
            second.starts_with("needle ") && second.ends_with('…'),
            "{second}"
        );
    }

    #[test]
    fn line_clipping_respects_char_boundaries() {
        let line = "ж".repeat(400);
        for start in [0, 2, 300, 798] {
            let clipped = clip_line(&line, start);
            assert!(
                clipped.len() <= MAX_LINE_BYTES + 2 * '…'.len_utf8(),
                "{start}"
            );
            assert!(clipped.chars().all(|c| c == 'ж' || c == '…'), "{start}");
        }
        assert_eq!(clip_line("short", 0), "short");
    }

    #[tokio::test]
    async fn only_the_head_of_a_huge_file_is_searched() {
        let dir = TempDir::new().unwrap();
        let mut body = "needle head\n".to_string();
        body.push_str(&"filler line\n".repeat(MAX_FILE_BYTES as usize / 12 + 10));
        body.push_str("needle tail\n");
        assert!(body.len() as u64 > MAX_FILE_BYTES);
        write(&dir, "big.log", body);

        let out = run(&dir, json!({"pattern": "needle"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            format!(
                "big.log:1: needle head\n\
                 [only the first {MAX_FILE_BYTES} bytes of 1 large file(s) were searched]"
            )
        );
    }

    #[tokio::test]
    async fn cancelled_turn_does_not_search() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "needle\n");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let out = GrepTool::new()
            .execute(
                json!({"pattern": "needle"}),
                &ToolCtx::with_cancel(dir.path(), cancel),
            )
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("cancelled"), "{}", out.content);
    }

    #[test]
    fn schema_is_minimal() {
        let schema = GrepTool::new().input_schema();
        assert_eq!(schema["required"], json!(["pattern"]));
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(
            properties.keys().collect::<Vec<_>>(),
            ["pattern", "path", "case_insensitive"]
        );
        for property in properties.values() {
            assert!(property["description"].is_string(), "{property}");
        }
        // Кап из описания и из кода — одно число.
        assert!(
            GrepTool::new()
                .description()
                .contains(&format!("at most {MAX_MATCHES} matches")),
            "{}",
            GrepTool::new().description()
        );
    }
}
