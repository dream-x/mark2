//! Системный промпт агента и подмешивание `AGENTS.md` проекта.
//!
//! Бюджет — жёсткий: **системный промпт вместе с описаниями локальных тулов
//! ≤ 1000 токенов**, сетевые (`web_fetch`, `web_search`) — отдельная надбавка
//! ≤ 175 токенов поверх. На локалках с контекстом в 8–32k каждая тысяча
//! токенов преамбулы отъедается у задачи, а длинные инструкции слабые модели
//! всё равно игнорируют. Отсюда два правила:
//!
//! * описания тулов живут в их схемах ([`crate::tools`]) и **не дублируются**
//!   в промпте — здесь только то, чего из схем не видно: кто мы, где мы и как
//!   себя вести;
//! * никаких динамических данных (время, список файлов, состояние git) — префикс
//!   промпта обязан быть байт-в-байт одинаковым между ходами, иначе KV-кэш
//!   локального сервера инвалидируется на каждом запросе.
//!
//! `AGENTS.md` — сложившийся стандарт проектных инструкций; [`find_agents_md`]
//! ищет его от рабочей директории вверх (не выше `$HOME`; вне `$HOME` —
//! только в самой рабочей директории), ближайший выигрывает, и
//! содержимое уходит отдельной секцией **в конец** промпта — после стабильной
//! части, чтобы её префикс не менялся от проекта к проекту.
//!
//! Память агента ([`crate::memory`]) — ещё одна секция, `## Memory`, самая
//! последняя: её правит сама модель, так что меняется она чаще всего. Нет
//! файла памяти (или он пуст) — нет и секции, промпт байт-в-байт тот же.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::config::{search_boundary, upward_dirs};
use crate::memory::Memory;

/// Имя файла проектных инструкций.
pub const AGENTS_MD: &str = "AGENTS.md";

/// Заголовок секции с проектными инструкциями.
const PROJECT_SECTION: &str = "# Project instructions";

/// Заголовок секции памяти.
pub const MEMORY_SECTION: &str = "## Memory";

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

/// [`system_prompt`] плюс секция памяти, если память есть. `None` — ровно
/// [`system_prompt`], без единого лишнего байта.
pub fn system_prompt_with_memory(cwd: &Path, memory: Option<&Memory>) -> String {
    let prompt = system_prompt(cwd);
    match memory {
        Some(memory) => with_memory(&prompt, memory, cwd),
        None => prompt,
    }
}

/// Дописать секцию памяти в конец промпта: заголовок, однострочная конвенция
/// (какой файл, когда инъектируется, как обновлять) и содержимое — с пометкой,
/// если оно обрезано по капу.
///
/// Путь — относительно `cwd`, если файл внутри него: тем же путём модель
/// потом зовёт `write`/`edit`.
pub fn with_memory(base: &str, memory: &Memory, cwd: &Path) -> String {
    let mut prompt = format!(
        "{base}\n\n{MEMORY_SECTION}\n\n\
         Your persistent memory file `{path}`, injected at session start. When asked to \
         remember something durable, update that file with write/edit; keep it short, facts \
         only.\n\n{content}",
        path = memory.display_path(cwd),
        content = memory.content,
    );
    if let Some(marker) = memory.truncation_marker() {
        prompt.push('\n');
        prompt.push_str(&marker);
    }
    prompt
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

/// Найти ближайший `AGENTS.md`: от `cwd` вверх, но не выше `$HOME`, а вне
/// `$HOME` — только в самом `cwd` ([`crate::config::upward_dirs`]): чужой
/// `/tmp/AGENTS.md` не должен становиться правилами каждого запуска в `/tmp`.
///
/// Возвращает путь и содержимое. Ближайший к рабочей директории выигрывает —
/// файл монорепы не перебивает файл конкретного пакета. Нечитаемый кандидат
/// (нет прав, это директория) не останавливает поиск: он логируется и обход
/// продолжается вверх.
pub fn find_agents_md(cwd: &Path) -> Option<(PathBuf, String)> {
    find_agents_md_within(cwd, search_boundary().as_deref())
}

/// [`find_agents_md`] с явной границей поиска `home` вместо настоящего `$HOME`.
pub fn find_agents_md_within(cwd: &Path, home: Option<&Path>) -> Option<(PathBuf, String)> {
    for dir in upward_dirs(cwd, home) {
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

    /// Надбавка на сетевые тулы (`web_fetch` всегда, `web_search` — с
    /// бэкендом): ≈175 токенов. Отдельно от основного бюджета, чтобы рост
    /// локальной части по-прежнему упирался в 4000.
    const WEB_TOOLS_BUDGET_CHARS: usize = 700;

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
        // схемы тулов — в самом полном составе, с `web_search`.
        let defs = ToolRegistry::with_defaults(EditStrategy::SearchReplace)
            .with_web_search(Some("http://localhost:8888"))
            .unwrap()
            .defs();
        let size = |network: bool| -> usize {
            defs.iter()
                .filter(|def| crate::permission::is_network(&def.name) == network)
                .map(|def| {
                    def.name.len() + def.description.len() + def.input_schema.to_string().len()
                })
                .sum()
        };
        let (local, web) = (size(false), size(true));
        assert!(
            prompt.len() + local <= PROMPT_BUDGET_CHARS,
            "промпт {} + локальные тулы {local} символов не влезают в {PROMPT_BUDGET_CHARS}",
            prompt.len(),
        );
        assert!(web > 0, "сетевые тулы должны попасть в замер");
        assert!(
            web <= WEB_TOOLS_BUDGET_CHARS,
            "сетевые тулы {web} символов не влезают в надбавку {WEB_TOOLS_BUDGET_CHARS}"
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

        // tempdir — вне настоящего `$HOME`; граница поиска — его корень.
        let (path, content) = find_agents_md_within(&nested, Some(root)).unwrap();
        assert_eq!(path, nested.join(AGENTS_MD));
        assert_eq!(content, "правила пакета\n");

        // На уровень выше — уже файл монорепы.
        let (path, content) = find_agents_md_within(&root.join("crates"), Some(root)).unwrap();
        assert_eq!(path, root.join(AGENTS_MD));
        assert_eq!(content, "правила монорепы\n");
    }

    #[test]
    fn agents_md_is_found_by_walking_up() {
        let dir = tempdir();
        let nested = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join(AGENTS_MD), "корневые правила").unwrap();

        let (path, _) = find_agents_md_within(&nested, Some(dir.path())).unwrap();
        assert_eq!(path, dir.path().join(AGENTS_MD));
    }

    /// Выше границы `AGENTS.md` не ищется: вне `$HOME` (здесь — вне `home`)
    /// — только в самом cwd, внутри — не выше `home`.
    #[test]
    fn agents_md_above_the_boundary_is_not_found() {
        let dir = tempdir();
        let home = dir.path().join("home");
        let nested = home.join("proj");
        let outside = dir.path().join("tmp").join("scratch");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(dir.path().join(AGENTS_MD), "чужие правила").unwrap();

        assert!(find_agents_md_within(&nested, Some(&home)).is_none());
        assert!(find_agents_md_within(&outside, Some(&home)).is_none());
        assert!(find_agents_md_within(&outside, None).is_none());

        std::fs::write(outside.join(AGENTS_MD), "свои правила").unwrap();
        let (path, _) = find_agents_md_within(&outside, Some(&home)).unwrap();
        assert_eq!(path, outside.join(AGENTS_MD));
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

    /// Нет файла памяти — промпт байт-в-байт прежний: ни секции, ни строки о
    /// конвенции. Пустой файл и файл из пробелов — то же самое.
    #[test]
    fn no_memory_file_leaves_the_prompt_byte_identical() {
        use crate::memory::{MEMORY_FILE, load_memory};

        let dir = tempdir();
        std::fs::write(dir.path().join(AGENTS_MD), "правила").unwrap();
        let before = system_prompt(dir.path());

        let memory = load_memory(dir.path(), None);
        assert_eq!(memory, None);
        assert_eq!(system_prompt_with_memory(dir.path(), None), before);

        for empty in ["", "\n\n", " \t\n"] {
            std::fs::write(dir.path().join(MEMORY_FILE), empty).unwrap();
            let memory = load_memory(dir.path(), None);
            assert_eq!(memory, None, "{empty:?}");
            let prompt = system_prompt_with_memory(dir.path(), memory.as_ref());
            assert_eq!(prompt, before, "{empty:?}");
            assert!(!prompt.contains(MEMORY_SECTION));
            assert!(!prompt.contains("memory"), "ни слова о памяти: {prompt}");
        }
    }

    /// Память — последняя секция: после базы и AGENTS.md, с путём от cwd,
    /// конвенцией одной строкой и содержимым.
    #[test]
    fn memory_goes_into_the_last_section_with_the_convention() {
        use crate::memory::{MEMORY_FILE, load_memory};

        let dir = tempdir();
        std::fs::write(dir.path().join(AGENTS_MD), "правила проекта").unwrap();
        std::fs::write(
            dir.path().join(MEMORY_FILE),
            "# Memory\n\n- The staging deploy port is 7342.\n\n",
        )
        .unwrap();

        let memory = load_memory(dir.path(), None).unwrap();
        let prompt = system_prompt_with_memory(dir.path(), Some(&memory));
        let without = system_prompt(dir.path());
        assert!(prompt.starts_with(&without), "память — в конец");

        let section = &prompt[without.len()..];
        assert_eq!(
            section,
            "\n\n## Memory\n\n\
             Your persistent memory file `.mark2.memory.md`, injected at session start. When \
             asked to remember something durable, update that file with write/edit; keep it \
             short, facts only.\n\n\
             # Memory\n\n- The staging deploy port is 7342."
        );
        // Конвенция — ровно одна строка.
        let convention = section.lines().nth(4).unwrap();
        assert!(convention.starts_with("Your persistent memory file"));
        assert!(convention.ends_with("facts only."));
    }

    /// Файл памяти выше cwd — путь абсолютный: относительного «от cwd» нет.
    #[test]
    fn memory_outside_the_cwd_is_shown_by_its_absolute_path() {
        use crate::memory::{MEMORY_FILE, load_memory_within};

        let dir = tempdir();
        let nested = dir.path().join("pkg");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join(MEMORY_FILE), "- факт\n").unwrap();

        let memory = load_memory_within(&nested, None, Some(dir.path())).unwrap();
        let prompt = system_prompt_with_memory(&nested, Some(&memory));
        let absolute = dir.path().join(MEMORY_FILE).display().to_string();
        assert!(prompt.contains(&format!("`{absolute}`")), "{prompt}");
        assert!(prompt.ends_with("- факт"), "{prompt}");
    }

    #[test]
    fn truncated_memory_carries_the_marker_in_the_prompt() {
        use crate::memory::{MEMORY_CAP_BYTES, Memory};

        let text = "- one more fact to keep\n".repeat(400);
        let memory = Memory::from_text("/w/.mark2.memory.md", &text).unwrap();
        let prompt = with_memory("base", &memory, Path::new("/w"));
        let marker = format!(
            "\n[memory truncated: {} of {} bytes shown]",
            memory.injected_bytes(),
            text.len()
        );
        assert!(prompt.ends_with(&marker), "{prompt}");
        assert!(memory.injected_bytes() <= MEMORY_CAP_BYTES);
        assert!(prompt.contains(&memory.content));
    }

    /// Директория с именем `AGENTS.md` не должна прерывать поиск.
    #[test]
    fn unreadable_candidate_does_not_stop_the_search() {
        let dir = tempdir();
        let nested = dir.path().join("nested");
        std::fs::create_dir_all(nested.join(AGENTS_MD)).unwrap();
        std::fs::write(dir.path().join(AGENTS_MD), "выше").unwrap();

        let (path, content) = find_agents_md_within(&nested, Some(dir.path())).unwrap();
        assert_eq!(path, dir.path().join(AGENTS_MD));
        assert_eq!(content, "выше");
    }
}
