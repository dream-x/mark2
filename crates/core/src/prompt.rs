//! Системный промпт агента и подмешивание `AGENTS.md` проекта.
//!
//! Бюджет — жёсткий: **системный промпт вместе с описаниями тулов ≤ 1000
//! токенов**. На локалках с контекстом в 8–32k каждая тысяча токенов преамбулы
//! отъедается у задачи, а длинные инструкции слабые модели всё равно
//! игнорируют. Отсюда два правила:
//!
//! * описания тулов живут в их схемах ([`crate::tools`]) и **не дублируются**
//!   в промпте — здесь только то, чего из схем не видно: кто мы, где мы и как
//!   себя вести;
//! * никаких динамических данных (время, список файлов, состояние git) — префикс
//!   промпта обязан быть байт-в-байт одинаковым между ходами, иначе KV-кэш
//!   локального сервера инвалидируется на каждом запросе.
//!
//! `AGENTS.md` — сложившийся стандарт проектных инструкций; [`find_agents_md`]
//! ищет его от рабочей директории вверх до корня, ближайший выигрывает, и
//! содержимое уходит отдельной секцией **в конец** промпта — после стабильной
//! части, чтобы её префикс не менялся от проекта к проекту.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Имя файла проектных инструкций.
pub const AGENTS_MD: &str = "AGENTS.md";

/// Заголовок секции с проектными инструкциями.
const PROJECT_SECTION: &str = "# Project instructions";

/// Личность и правила агента без проектной секции.
///
/// `{cwd}` — единственная подстановка: она стабильна в пределах сессии, так что
/// префикс промпта остаётся неизменным.
pub fn base_prompt(cwd: &Path) -> String {
    format!(
        "You are mark2, a coding agent working in a terminal on the user's machine.\n\
         \n\
         Working directory: {cwd}\n\
         \n\
         - Use the tools to inspect and change the project. Never claim an edit you did not \
         actually make with a tool.\n\
         - Read a file before editing it. `edit` needs text copied verbatim from the file; \
         use `write` only for a new file or a full rewrite.\n\
         - Run builds, tests and git through `bash`, always non-interactively.\n\
         - Work in small steps: call a tool, look at the result, then decide the next one. \
         Stop as soon as the task is done.\n\
         - If a tool returns an error, fix the call or the code and try again; do not ask the \
         user to run things you can run yourself.\n\
         - Answer briefly, in plain sentences, in the language the user writes in. No preamble, \
         no restating output the user already saw.",
        cwd = cwd.display(),
    )
}

/// Полный системный промпт: [`base_prompt`] плюс `AGENTS.md`, если он найден.
pub fn system_prompt(cwd: &Path) -> String {
    let base = base_prompt(cwd);
    match find_agents_md(cwd) {
        Some((path, content)) => with_project_instructions(&base, &path, &content),
        None => base,
    }
}

/// Дописать секцию проектных инструкций в конец промпта.
///
/// Путь указывается явно: модель должна понимать, откуда взялись правила и что
/// они принадлежат проекту, а не ядру.
pub fn with_project_instructions(base: &str, path: &Path, content: &str) -> String {
    format!(
        "{base}\n\n{PROJECT_SECTION}\n\nFrom {path} — project rules, follow them.\n\n{content}",
        path = path.display(),
        content = content.trim_end(),
    )
}

/// Найти ближайший `AGENTS.md`: от `cwd` вверх до корня файловой системы.
///
/// Возвращает путь и содержимое. Ближайший к рабочей директории выигрывает —
/// файл монорепы не перебивает файл конкретного пакета. Нечитаемый кандидат
/// (нет прав, это директория) не останавливает поиск: он логируется и обход
/// продолжается вверх.
pub fn find_agents_md(cwd: &Path) -> Option<(PathBuf, String)> {
    for dir in cwd.ancestors() {
        let candidate = dir.join(AGENTS_MD);
        match std::fs::read_to_string(&candidate) {
            Ok(content) => return Some((candidate, content)),
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(
                    path = %candidate.display(),
                    %error,
                    "не удалось прочитать AGENTS.md, ищем выше"
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{EditStrategy, ToolRegistry};

    /// Грубая оценка бюджета «≤ 1000 токенов»: на английском тексте и JSON
    /// одна тысяча токенов — это примерно четыре тысячи символов.
    const PROMPT_BUDGET_CHARS: usize = 4000;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn base_prompt_names_the_agent_and_the_cwd() {
        let prompt = base_prompt(Path::new("/work/mark2"));
        assert!(prompt.starts_with("You are mark2"), "{prompt}");
        assert!(prompt.contains("/work/mark2"), "{prompt}");
        // Описания тулов живут в схемах — в промпте их быть не должно.
        assert!(!prompt.contains("JSON Schema"), "{prompt}");
    }

    /// Префикс промпта обязан быть стабильным между ходами.
    #[test]
    fn base_prompt_is_deterministic() {
        let first = base_prompt(Path::new("/work"));
        for _ in 0..8 {
            assert_eq!(base_prompt(Path::new("/work")), first);
        }
    }

    #[test]
    fn prompt_with_tool_descriptions_fits_the_budget() {
        let prompt = base_prompt(Path::new("/Users/someone/projects/mark2"));
        assert!(
            prompt.len() <= PROMPT_BUDGET_CHARS,
            "системный промпт {} символов",
            prompt.len()
        );

        // То, что реально уходит модели преамбулой: промпт + имена, описания и
        // схемы тулов.
        let defs = ToolRegistry::with_defaults(EditStrategy::SearchReplace).defs();
        let tools_size: usize = defs
            .iter()
            .map(|def| def.name.len() + def.description.len() + def.input_schema.to_string().len())
            .sum();
        assert!(
            prompt.len() + tools_size <= PROMPT_BUDGET_CHARS,
            "промпт {} + тулы {tools_size} символов не влезают в {PROMPT_BUDGET_CHARS}",
            prompt.len(),
        );
    }

    #[test]
    fn agents_md_is_not_required() {
        let dir = tempdir();
        assert!(find_agents_md(dir.path()).is_none());
        assert_eq!(system_prompt(dir.path()), base_prompt(dir.path()));
    }

    #[test]
    fn nearest_agents_md_wins() {
        let dir = tempdir();
        let root = dir.path();
        let nested = root.join("crates").join("core");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join(AGENTS_MD), "правила монорепы\n").unwrap();
        std::fs::write(nested.join(AGENTS_MD), "правила пакета\n").unwrap();

        let (path, content) = find_agents_md(&nested).unwrap();
        assert_eq!(path, nested.join(AGENTS_MD));
        assert_eq!(content, "правила пакета\n");

        // На уровень выше — уже файл монорепы.
        let (path, content) = find_agents_md(&root.join("crates")).unwrap();
        assert_eq!(path, root.join(AGENTS_MD));
        assert_eq!(content, "правила монорепы\n");
    }

    #[test]
    fn agents_md_is_found_by_walking_up() {
        let dir = tempdir();
        let nested = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join(AGENTS_MD), "корневые правила").unwrap();

        let (path, _) = find_agents_md(&nested).unwrap();
        assert_eq!(path, dir.path().join(AGENTS_MD));
    }

    #[test]
    fn agents_md_goes_into_a_trailing_section() {
        let dir = tempdir();
        std::fs::write(
            dir.path().join(AGENTS_MD),
            "Build with `cargo build`.\n\n\n",
        )
        .unwrap();

        let prompt = system_prompt(dir.path());
        let base = base_prompt(dir.path());
        assert!(prompt.starts_with(&base), "проектная секция идёт в конец");
        assert!(prompt.contains(PROJECT_SECTION), "{prompt}");
        assert!(prompt.contains("Build with `cargo build`."), "{prompt}");
        assert!(
            prompt.ends_with("Build with `cargo build`."),
            "хвост обрезан"
        );
        assert!(
            prompt.contains(&dir.path().join(AGENTS_MD).display().to_string()),
            "{prompt}"
        );
    }

    /// Директория с именем `AGENTS.md` не должна прерывать поиск.
    #[test]
    fn unreadable_candidate_does_not_stop_the_search() {
        let dir = tempdir();
        let nested = dir.path().join("nested");
        std::fs::create_dir_all(nested.join(AGENTS_MD)).unwrap();
        std::fs::write(dir.path().join(AGENTS_MD), "выше").unwrap();

        let (path, content) = find_agents_md(&nested).unwrap();
        assert_eq!(path, dir.path().join(AGENTS_MD));
        assert_eq!(content, "выше");
    }
}
