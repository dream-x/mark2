//! Задача бенча на диске.
//!
//! Контракт директории `bench/tasks/<ID>-<slug>/` (зафиксирован — ранер и
//! автор задач держатся за него независимо):
//!
//! ```text
//! T04-fix-bug-test/
//! ├── prompt.md   весь файл целиком уходит в mark2 позиционным аргументом
//! ├── setup/      копируется в свежую tmp-cwd прогона (может отсутствовать)
//! ├── verify.sh   `sh verify.sh` с cwd = cwd прогона; exit 0 = pass
//! └── task.toml   [task] timeout_seconds, опц. repeats, tags
//! ```
//!
//! Всё, что можно проверить до запуска модели, проверяется до запуска модели:
//! отсутствующий `verify.sh` или пустой `prompt.md` — ошибка загрузки, а не
//! сюрприз через пять минут прогона.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Директория задач по умолчанию (относительно корня репозитория).
pub const DEFAULT_TASKS_DIR: &str = "bench/tasks";

/// Сколько раз гонять пару модель×задача, если задача не сказала иначе.
pub const DEFAULT_REPEATS: usize = 3;

/// Имя файла с промптом.
pub const PROMPT_FILE: &str = "prompt.md";
/// Имя директории с исходными файлами задачи.
pub const SETUP_DIR: &str = "setup";
/// Имя скрипта верификации.
pub const VERIFY_SCRIPT: &str = "verify.sh";
/// Имя файла настроек задачи.
pub const TASK_FILE: &str = "task.toml";

/// Загруженная задача.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    /// Идентификатор: часть имени директории до первого дефиса (`T04`).
    pub id: String,
    /// Человекочитаемая часть имени директории (`fix-bug-test`).
    pub slug: String,
    /// Директория задачи.
    pub dir: PathBuf,
    /// Текст промпта — файл `prompt.md` целиком.
    pub prompt: String,
    /// Базовый таймаут прогона в секундах (умножается на множитель модели).
    pub timeout_seconds: u64,
    /// Сколько повторов просит задача.
    pub repeats: usize,
    /// Теги для группировки в отчёте.
    pub tags: Vec<String>,
}

impl Task {
    /// Имя директории задачи (`T04-fix-bug-test`).
    pub fn name(&self) -> String {
        if self.slug.is_empty() {
            self.id.clone()
        } else {
            format!("{}-{}", self.id, self.slug)
        }
    }

    /// Путь к `setup/`, если он есть.
    pub fn setup_dir(&self) -> Option<PathBuf> {
        let path = self.dir.join(SETUP_DIR);
        path.is_dir().then_some(path)
    }

    /// Путь к `verify.sh`.
    pub fn verify_script(&self) -> PathBuf {
        self.dir.join(VERIFY_SCRIPT)
    }

    /// Эффективное число повторов: `--repeats` старше `task.toml`.
    pub fn repeats_with_override(&self, override_repeats: Option<usize>) -> usize {
        override_repeats.unwrap_or(self.repeats)
    }
}

/// Содержимое `task.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskFile {
    task: TaskSection,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskSection {
    timeout_seconds: u64,
    #[serde(default)]
    repeats: Option<usize>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Разобрать `task.toml`. `origin` попадает в текст ошибок.
fn parse_task_file(raw: &str, origin: &str) -> Result<TaskSection> {
    let file: TaskFile =
        toml::from_str(raw).with_context(|| format!("не удалось разобрать {origin}"))?;
    let section = file.task;
    if section.timeout_seconds == 0 {
        bail!("{origin}: timeout_seconds должен быть больше нуля");
    }
    if section.repeats == Some(0) {
        bail!("{origin}: repeats должен быть больше нуля");
    }
    Ok(section)
}

/// Загрузить одну задачу из её директории.
///
/// Путь директории разворачивается в абсолютный сразу: `verify.sh` и `setup/`
/// нужны из tmp-cwd прогона, где относительный `bench/tasks/…` не резолвится.
pub fn load_task(dir: &Path) -> Result<Task> {
    let dir = &crate::resolve_path(dir);
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("нечитаемое имя директории задачи {}", dir.display()))?
        .to_string();
    let (id, slug) = match name.split_once('-') {
        Some((id, slug)) => (id.to_string(), slug.to_string()),
        None => (name.clone(), String::new()),
    };

    let prompt_path = dir.join(PROMPT_FILE);
    let prompt = std::fs::read_to_string(&prompt_path)
        .with_context(|| format!("не удалось прочитать промпт {}", prompt_path.display()))?;
    if prompt.trim().is_empty() {
        bail!("{}: промпт пуст", prompt_path.display());
    }

    let verify = dir.join(VERIFY_SCRIPT);
    if !verify.is_file() {
        bail!(
            "{}: нет {VERIFY_SCRIPT} — задача без программной проверки не бенчится",
            dir.display()
        );
    }

    let task_path = dir.join(TASK_FILE);
    let raw = std::fs::read_to_string(&task_path)
        .with_context(|| format!("не удалось прочитать {}", task_path.display()))?;
    let section = parse_task_file(&raw, &task_path.display().to_string())?;

    Ok(Task {
        id,
        slug,
        dir: dir.to_path_buf(),
        prompt,
        timeout_seconds: section.timeout_seconds,
        repeats: section.repeats.unwrap_or(DEFAULT_REPEATS),
        tags: section.tags,
    })
}

/// Загрузить все задачи директории в порядке имён (то есть по возрастанию ID).
pub fn discover_tasks(tasks_dir: &Path) -> Result<Vec<Task>> {
    let entries = std::fs::read_dir(tasks_dir).with_context(|| {
        format!(
            "не удалось прочитать директорию задач {}",
            tasks_dir.display()
        )
    })?;

    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("не удалось обойти директорию {}", tasks_dir.display()))?;
        let path = entry.path();
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_none_or(|name| name.starts_with('.'));
        if path.is_dir() && !hidden {
            dirs.push(path);
        }
    }
    dirs.sort();

    if dirs.is_empty() {
        bail!("в {} нет ни одной задачи", tasks_dir.display());
    }

    dirs.iter().map(|dir| load_task(dir)).collect()
}

/// Отобрать задачи по ID (или полному имени директории), сохраняя порядок.
///
/// Сравнение по ID регистронезависимое: `t01` и `T01` — одно и то же.
pub fn select_tasks(all: Vec<Task>, wanted: &[String]) -> Result<Vec<Task>> {
    if wanted.is_empty() {
        return Ok(all);
    }

    let matches = |task: &Task, wanted: &str| {
        task.id.eq_ignore_ascii_case(wanted) || task.name().eq_ignore_ascii_case(wanted)
    };

    for id in wanted {
        if !all.iter().any(|task| matches(task, id)) {
            let known: Vec<String> = all.iter().map(Task::name).collect();
            bail!("неизвестная задача {id:?}; доступны: {}", known.join(", "));
        }
    }

    Ok(all
        .into_iter()
        .filter(|task| wanted.iter().any(|id| matches(task, id)))
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Фикстурная задача: ранер тестируется на своих задачах, а не на
    /// `bench/tasks` — иначе тесты падали бы от чужих правок.
    pub(crate) fn write_task(root: &Path, name: &str, task_toml: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join(SETUP_DIR)).unwrap();
        std::fs::write(dir.join(PROMPT_FILE), "Сделай что-нибудь полезное.\n").unwrap();
        std::fs::write(dir.join(VERIFY_SCRIPT), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(dir.join(TASK_FILE), task_toml).unwrap();
        dir
    }

    #[test]
    fn loads_all_fields_and_splits_id_from_slug() {
        let root = TempDir::new().unwrap();
        let dir = write_task(
            root.path(),
            "T04-fix-bug-test",
            "[task]\ntimeout_seconds = 120\nrepeats = 5\ntags = [\"edit\", \"bash\"]\n",
        );

        let task = load_task(&dir).unwrap();
        assert_eq!(task.id, "T04");
        assert_eq!(task.slug, "fix-bug-test");
        assert_eq!(task.name(), "T04-fix-bug-test");
        assert_eq!(task.prompt, "Сделай что-нибудь полезное.\n");
        assert_eq!(task.timeout_seconds, 120);
        assert_eq!(task.repeats, 5);
        assert_eq!(task.tags, ["edit", "bash"]);
        // Пути развёрнуты при загрузке (на macOS /var → /private/var).
        let canonical = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(task.dir, canonical);
        assert_eq!(task.setup_dir(), Some(canonical.join(SETUP_DIR)));
        assert_eq!(task.verify_script(), canonical.join(VERIFY_SCRIPT));
    }

    #[test]
    fn repeats_default_to_three_and_flag_wins() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 60\n");
        let task = load_task(&dir).unwrap();
        assert_eq!(task.repeats, DEFAULT_REPEATS);
        assert_eq!(task.repeats_with_override(None), 3);
        assert_eq!(task.repeats_with_override(Some(1)), 1);
        assert!(task.tags.is_empty());
    }

    /// Пути задачи используются из tmp-cwd прогона (`sh verify.sh`, копия
    /// `setup/`), поэтому загрузка обязана вернуть абсолютные и нормализованные.
    #[test]
    fn paths_are_absolute_after_loading() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 60\n");

        // Путь с `..` — как если бы задачу нашли обходом директории.
        let indirect = dir.join("..").join("T01-x");
        let task = load_task(&indirect).unwrap();

        assert_eq!(task.dir, std::fs::canonicalize(&dir).unwrap());
        assert!(task.verify_script().is_absolute());
        assert!(task.verify_script().is_file());
        assert!(task.setup_dir().is_some_and(|path| path.is_absolute()));
        assert!(
            !task.dir.components().any(|c| c.as_os_str() == ".."),
            "{}",
            task.dir.display()
        );
        // ID и slug берутся из уже развёрнутого пути, а не из «..».
        assert_eq!(task.id, "T01");
        assert_eq!(task.slug, "x");
    }

    /// Тот же контракт при обходе директории заданной относительным путём.
    #[test]
    fn discovery_returns_absolute_paths_for_a_relative_tasks_dir() {
        let root = TempDir::new().unwrap();
        write_task(root.path(), "T01-a", "[task]\ntimeout_seconds = 60\n");

        // Относительный путь строим от временной директории, не трогая cwd
        // процесса: тесты идут параллельно.
        let relative = root
            .path()
            .join("..")
            .join(root.path().file_name().unwrap());
        let tasks = discover_tasks(&relative).unwrap();
        assert!(tasks[0].dir.is_absolute(), "{}", tasks[0].dir.display());
        assert!(tasks[0].verify_script().is_file());
    }

    #[test]
    fn missing_setup_directory_is_fine() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 60\n");
        std::fs::remove_dir(dir.join(SETUP_DIR)).unwrap();
        assert_eq!(load_task(&dir).unwrap().setup_dir(), None);
    }

    #[test]
    fn broken_task_toml_errors_mention_the_path() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_secs = 60\n");
        let error = load_task(&dir).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("task.toml"), "{text}");
        assert!(text.contains("timeout_secs"), "{text}");

        std::fs::write(dir.join(TASK_FILE), "[task]\ntimeout_seconds = 0\n").unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("timeout_seconds"), "{text}");
        assert!(text.contains(&dir.display().to_string()), "{text}");

        std::fs::write(
            dir.join(TASK_FILE),
            "[task]\ntimeout_seconds = 5\nrepeats = 0\n",
        )
        .unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("repeats"), "{text}");
    }

    #[test]
    fn missing_pieces_are_reported_with_paths() {
        let root = TempDir::new().unwrap();

        let dir = write_task(root.path(), "T01-x", "[task]\ntimeout_seconds = 60\n");
        std::fs::remove_file(dir.join(VERIFY_SCRIPT)).unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("verify.sh"), "{text}");

        let dir = write_task(root.path(), "T02-y", "[task]\ntimeout_seconds = 60\n");
        std::fs::write(dir.join(PROMPT_FILE), "   \n").unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("промпт пуст"), "{text}");
        assert!(text.contains("prompt.md"), "{text}");

        let dir = write_task(root.path(), "T03-z", "[task]\ntimeout_seconds = 60\n");
        std::fs::remove_file(dir.join(PROMPT_FILE)).unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("prompt.md"), "{text}");
    }

    #[test]
    fn discovery_is_sorted_and_skips_non_task_entries() {
        let root = TempDir::new().unwrap();
        write_task(root.path(), "T03-c", "[task]\ntimeout_seconds = 60\n");
        write_task(root.path(), "T01-a", "[task]\ntimeout_seconds = 60\n");
        write_task(root.path(), "T02-b", "[task]\ntimeout_seconds = 60\n");
        std::fs::write(root.path().join("README.md"), "не задача").unwrap();
        std::fs::create_dir(root.path().join(".hidden")).unwrap();

        let tasks = discover_tasks(root.path()).unwrap();
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["T01", "T02", "T03"]);
    }

    #[test]
    fn empty_or_missing_tasks_dir_is_an_error_with_the_path() {
        let root = TempDir::new().unwrap();
        let text = format!("{:#}", discover_tasks(root.path()).unwrap_err());
        assert!(text.contains("нет ни одной задачи"), "{text}");

        let missing = root.path().join("нет-такой");
        let text = format!("{:#}", discover_tasks(&missing).unwrap_err());
        assert!(text.contains(&missing.display().to_string()), "{text}");
    }

    #[test]
    fn selection_matches_id_case_insensitively_and_rejects_typos() {
        let root = TempDir::new().unwrap();
        write_task(root.path(), "T01-a", "[task]\ntimeout_seconds = 60\n");
        write_task(root.path(), "T02-b", "[task]\ntimeout_seconds = 60\n");
        let all = discover_tasks(root.path()).unwrap();

        assert_eq!(select_tasks(all.clone(), &[]).unwrap().len(), 2);
        let picked = select_tasks(all.clone(), &["t02".into()]).unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "T02");
        let by_name = select_tasks(all.clone(), &["T01-a".into()]).unwrap();
        assert_eq!(by_name[0].id, "T01");

        let text = format!("{:#}", select_tasks(all, &["T99".into()]).unwrap_err());
        assert!(text.contains("T99"), "{text}");
        assert!(text.contains("T01-a, T02-b"), "{text}");
    }
}
