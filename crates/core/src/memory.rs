//! Персистентная память агента: файл, который харнесс кладёт в системный
//! промпт, а модель правит обычными `write`/`edit`. Новых тулов нет — память
//! это просто файл проекта, который читает и пишет каждый, кто умеет в файлы.
//!
//! # Где файл
//!
//! Явный `[agent] memory_file` (относительный — от директории конфиг-файла,
//! см. [`crate::config::AgentSettings::memory_file`]) или ближайший
//! [`MEMORY_FILE`] от cwd вверх — тем же обходом, что `.mark2.toml`
//! ([`crate::config::find_upwards`]). Конфиг и память ищутся независимо,
//! каждый по своему ближайшему. Явный путь, которого нет на диске, — не
//! ошибка: памяти просто нет, файл появится при первой записи; к поиску
//! ближайшего такой путь не откатывается.
//!
//! # Включение = существование непустого файла
//!
//! * Файла нет, он пуст или из одних пробелов — в промпт не уходит **ни
//!   байта**, даже строки о конвенции: кто памятью не пользуется, не платит за
//!   неё токенами, и промпт байт-в-байт тот же, что был до памяти.
//! * Файл непуст — в конец промпта дописывается секция `## Memory`:
//!   однострочная конвенция (какой файл, что он инъектируется при старте
//!   сессии, как его обновлять) и содержимое
//!   ([`crate::prompt::with_memory`]).
//!
//! Следствие: о механизме модель узнаёт только из существующего файла.
//! Пользователь «включает» память, создав файл с заголовком
//! (`echo '# Memory' > .mark2.memory.md`); голый `touch` создаёт пустой файл,
//! а пустой файл — это «памяти нет». Так же устроена задача бенча T19: в её
//! setup лежит память из одной строки-заголовка.
//!
//! # Кап
//!
//! В промпт уходит не больше [`MEMORY_CAP_BYTES`] байт содержимого: длиннее —
//! обрезка по границе строки с пометкой `[memory truncated: N of M bytes
//! shown]` в промпте и [`crate::Event::Warning`] наружу. Файл не правится —
//! обрезается только инъекция.
//!
//! # Когда читается
//!
//! Один раз, при сборке агента ([`crate::Agent::new`]): системный промпт
//! обязан быть стабильным между запросами (KV-кэш), поэтому правки памяти по
//! ходу сессии модель видит в истории своих же тул-коллов, а в промпт они
//! попадут в следующей сессии. Компакция системный промпт не трогает — память
//! её переживает.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::config::find_upwards;

/// Имя файла памяти; ищется от cwd вверх.
pub const MEMORY_FILE: &str = ".mark2.memory.md";

/// Сколько байт содержимого памяти уходит в промпт: 4 КиБ ≈ 1000 токенов —
/// столько же, сколько весь остальной системный промпт со схемами тулов.
pub const MEMORY_CAP_BYTES: usize = 4096;

/// Память, прочитанная для инъекции в системный промпт.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    /// Файл памяти; относительный путь уже развёрнут от cwd, `.`/`..`
    /// свёрнуты лексически.
    pub path: PathBuf,
    /// То, что уходит в промпт: целые строки в пределах капа, без хвостовых
    /// пробелов. Никогда не пустое.
    pub content: String,
    /// Размер файла в байтах.
    pub file_bytes: usize,
    /// Содержимое обрезано по капу.
    pub truncated: bool,
}

impl Memory {
    /// Память из текста файла. Пустой файл (или из одних пробелов) — `None`:
    /// памяти нет. Длиннее [`MEMORY_CAP_BYTES`] — обрезка.
    pub fn from_text(path: impl Into<PathBuf>, text: &str) -> Option<Self> {
        Self::with_cap(path.into(), text, MEMORY_CAP_BYTES)
    }

    fn with_cap(path: PathBuf, text: &str, cap: usize) -> Option<Self> {
        let body = text.trim_end();
        if body.trim_start().is_empty() {
            return None;
        }
        let (content, truncated) = if body.len() <= cap {
            (body, false)
        } else {
            (truncate_at_line(body, cap), true)
        };
        Some(Self {
            path,
            content: content.to_string(),
            file_bytes: text.len(),
            truncated,
        })
    }

    /// Сколько байт памяти ушло в промпт.
    pub fn injected_bytes(&self) -> usize {
        self.content.len()
    }

    /// Путь так, как его увидит модель: относительно `cwd`, если файл внутри
    /// него (тулы резолвят пути от `cwd`, а в бенче `cwd` — длинный временный
    /// путь), иначе абсолютный.
    pub fn display_path(&self, cwd: &Path) -> String {
        self.path
            .strip_prefix(cwd)
            .unwrap_or(&self.path)
            .display()
            .to_string()
    }

    /// Пометка об обрезке для промпта; `None` — содержимое целиком.
    pub fn truncation_marker(&self) -> Option<String> {
        self.truncated.then(|| {
            format!(
                "[memory truncated: {} of {} bytes shown]",
                self.injected_bytes(),
                self.file_bytes
            )
        })
    }

    /// Предупреждение для человека, если память обрезана.
    pub fn truncation_warning(&self, cwd: &Path) -> Option<String> {
        self.truncated.then(|| {
            format!(
                "память {} — {} байт, в промпт ушло {} (кап {MEMORY_CAP_BYTES}); файл не тронут, \
                 сократи его",
                self.display_path(cwd),
                self.file_bytes,
                self.injected_bytes(),
            )
        })
    }
}

/// Самый длинный префикс из целых строк, не длиннее `cap` байт. Строка,
/// которая кончается ровно на капе, — тоже целая. Если целых строк нет (первая
/// строка длиннее капа), — обрезка по границе символа.
fn truncate_at_line(body: &str, cap: usize) -> &str {
    debug_assert!(body.len() > cap);
    let window = &body.as_bytes()[..=cap];
    let by_line = window
        .iter()
        .rposition(|&byte| byte == b'\n')
        .filter(|&newline| !body[..newline].trim().is_empty());
    let end = by_line.unwrap_or_else(|| {
        let mut end = cap;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        end
    });
    body[..end].trim_end()
}

/// Где лежит память: явный путь (относительный — от `cwd`) или ближайший
/// [`MEMORY_FILE`] от `cwd` вверх. `None` — искать нечего.
pub fn memory_path(cwd: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    match explicit {
        Some(path) => Some(lexical(&cwd.join(path))),
        None => find_upwards(cwd, MEMORY_FILE),
    }
}

/// Прочитать память для `cwd`. `None` — памяти нет: файла нет, он пуст или
/// не читается (последнее — `tracing::warn`: ход это не ломает).
pub fn load_memory(cwd: &Path, explicit: Option<&Path>) -> Option<Memory> {
    let path = memory_path(cwd, explicit)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Memory::from_text(path, &text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "не удалось прочитать память, работаю без неё");
            None
        }
    }
}

/// Свернуть `.` и `..` лексически, не трогая файловую систему.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn memory_is_found_in_the_cwd_and_in_a_parent() {
        let dir = tempdir();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(load_memory(&nested, None), None, "файла нет — памяти нет");

        std::fs::write(dir.path().join(MEMORY_FILE), "# Memory\n- корень\n").unwrap();
        let memory = load_memory(&nested, None).unwrap();
        assert_eq!(memory.path, dir.path().join(MEMORY_FILE));
        assert_eq!(memory.content, "# Memory\n- корень");
        assert_eq!(memory.file_bytes, "# Memory\n- корень\n".len());
        assert!(!memory.truncated);

        // Ближайший выигрывает.
        std::fs::write(nested.join(MEMORY_FILE), "# Memory\n- пакет\n").unwrap();
        let memory = load_memory(&nested, None).unwrap();
        assert_eq!(memory.path, nested.join(MEMORY_FILE));
        assert_eq!(memory.content, "# Memory\n- пакет");
        assert_eq!(memory.display_path(&nested), MEMORY_FILE);
        // Из родителя файл пакета не виден.
        assert_eq!(
            load_memory(&dir.path().join("a"), None).unwrap().path,
            dir.path().join(MEMORY_FILE)
        );
    }

    #[test]
    fn empty_or_blank_file_means_no_memory() {
        let dir = tempdir();
        for text in ["", "\n", "  \n\t\n"] {
            std::fs::write(dir.path().join(MEMORY_FILE), text).unwrap();
            assert_eq!(load_memory(dir.path(), None), None, "{text:?}");
        }
    }

    /// Директория с именем файла памяти — не кандидат, поиск идёт выше.
    #[test]
    fn a_directory_named_like_memory_is_skipped() {
        let dir = tempdir();
        let nested = dir.path().join("nested");
        std::fs::create_dir_all(nested.join(MEMORY_FILE)).unwrap();
        std::fs::write(dir.path().join(MEMORY_FILE), "выше").unwrap();
        assert_eq!(
            load_memory(&nested, None).unwrap().path,
            dir.path().join(MEMORY_FILE)
        );
    }

    /// Явный путь главнее поиска: даже если его нет на диске, а рядом лежит
    /// `.mark2.memory.md`, — памяти нет (файл появится при первой записи).
    #[test]
    fn explicit_path_wins_and_a_missing_one_is_not_an_error() {
        let dir = tempdir();
        std::fs::write(dir.path().join(MEMORY_FILE), "рядом").unwrap();
        let notes = dir.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();

        let missing = notes.join("memory.md");
        assert_eq!(load_memory(dir.path(), Some(&missing)), None);

        std::fs::write(&missing, "# Notes\n- явная\n").unwrap();
        let memory = load_memory(dir.path(), Some(&missing)).unwrap();
        assert_eq!(memory.content, "# Notes\n- явная");

        // Относительный (если путь пришёл не из конфига) — от cwd, `..` свёрнуты.
        let relative = load_memory(&notes, Some(Path::new("../notes/./memory.md"))).unwrap();
        assert_eq!(relative.path, missing);
        assert_eq!(relative.display_path(&notes), "memory.md");
        // Вне cwd модель видит абсолютный путь.
        let outside = dir.path().join("elsewhere");
        assert_eq!(
            relative.display_path(&outside),
            missing.display().to_string()
        );
    }

    /// Относительный `memory_file` конфига резолвится от директории конфига,
    /// а не от cwd, из которой запущен агент.
    #[test]
    fn config_memory_file_is_relative_to_the_config_file() {
        let root = tempdir();
        let cwd = root.path().join("crates").join("core");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(
            root.path().join(crate::config::PROJECT_CONFIG_FILE),
            "[agent]\nmemory_file = \"docs/memory.md\"\n",
        )
        .unwrap();
        std::fs::write(root.path().join("docs").join("memory.md"), "- из docs\n").unwrap();
        // Ближайший `.mark2.memory.md` есть, но конфиг сказал явно.
        std::fs::write(cwd.join(MEMORY_FILE), "- ближайший\n").unwrap();

        let config_path = crate::config::find_project_config(&cwd).unwrap();
        let config = crate::Config::load_file(&config_path).unwrap().unwrap();
        let memory = load_memory(&cwd, config.agent.memory_file.as_deref()).unwrap();
        assert_eq!(memory.path, root.path().join("docs").join("memory.md"));
        assert_eq!(memory.content, "- из docs");
    }

    #[test]
    fn long_memory_is_cut_at_a_line_boundary_with_a_marker() {
        let line = "- fact number xx: something worth remembering\n";
        let text: String = (0..200).map(|_| line).collect();
        assert!(text.len() > MEMORY_CAP_BYTES);

        let memory = Memory::from_text("/w/.mark2.memory.md", &text).unwrap();
        assert!(memory.truncated);
        assert!(memory.injected_bytes() <= MEMORY_CAP_BYTES);
        let whole_lines = MEMORY_CAP_BYTES / line.len();
        // Целые строки, без хвостового перевода строки.
        assert_eq!(memory.content, line.repeat(whole_lines).trim_end());
        assert!(text.starts_with(&memory.content));
        assert_eq!(memory.file_bytes, text.len());
        assert_eq!(
            memory.truncation_marker().unwrap(),
            format!(
                "[memory truncated: {} of {} bytes shown]",
                memory.injected_bytes(),
                text.len()
            )
        );
        let warning = memory.truncation_warning(Path::new("/w")).unwrap();
        assert!(warning.contains(MEMORY_FILE), "{warning}");
        assert!(warning.contains(&text.len().to_string()), "{warning}");
    }

    #[test]
    fn memory_at_the_cap_is_whole() {
        let text = "x".repeat(MEMORY_CAP_BYTES);
        let memory = Memory::from_text("m", &text).unwrap();
        assert!(!memory.truncated);
        assert_eq!(memory.truncation_marker(), None);
        assert_eq!(memory.truncation_warning(Path::new("/")), None);
        assert_eq!(memory.injected_bytes(), MEMORY_CAP_BYTES);

        // Строка, которая кончается ровно на капе, — целая.
        let exact = format!("{}\n{}", "a".repeat(10), "b".repeat(30));
        let memory = Memory::with_cap(PathBuf::from("m"), &exact, 10).unwrap();
        assert_eq!(memory.content, "a".repeat(10));
    }

    /// Первая строка длиннее капа — режем по границе символа, не байта.
    #[test]
    fn a_single_huge_line_is_cut_at_a_char_boundary() {
        let text = "я".repeat(10); // по 2 байта на символ
        let memory = Memory::with_cap(PathBuf::from("m"), &text, 5).unwrap();
        assert!(memory.truncated);
        assert_eq!(memory.content, "яя");

        // Пустые строки в начале не считаются «целыми строками»: вместо
        // пустой инъекции — обрезка по символу.
        let text = format!("\n\n{}", "z".repeat(20));
        let memory = Memory::with_cap(PathBuf::from("m"), &text, 8).unwrap();
        assert_eq!(memory.content, "\n\nzzzzzz");
    }

    /// Обрезается только инъекция — файл на диске остаётся как был.
    #[test]
    fn truncation_does_not_touch_the_file() {
        let dir = tempdir();
        let path = dir.path().join(MEMORY_FILE);
        let text = "- long line of memory\n".repeat(400);
        std::fs::write(&path, &text).unwrap();
        let memory = load_memory(dir.path(), None).unwrap();
        assert!(memory.truncated);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
