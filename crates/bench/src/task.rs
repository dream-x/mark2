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
//! └── task.toml   [task] timeout_seconds, опц. repeats, tags, policy,
//!                 context_limit, max_turns, verify_timeout_seconds, uitest_env,
//!                 post_edit_command, post_edit_timeout_seconds
//! ```
//!
//! Поля `[task]`:
//!
//! * `timeout_seconds` — базовый бюджет прогона (> 0), умножается на
//!   `timeout_multiplier` модели;
//! * `repeats` — сколько повторов (> 0), по умолчанию [`DEFAULT_REPEATS`];
//! * `tags` — группировка в отчёте;
//! * `policy` — политика доступа к тулам: `"yolo"` (по умолчанию, mark2
//!   запускается с `--yolo`) или `"read-only"` (`--policy read-only`: `bash`,
//!   `edit`, `write` отклоняются ядром без вопросов);
//! * `context_limit` — окно модели в токенах (> 0) для этого прогона: ранер
//!   дописывает `[models."<id>"] context_limit = N` в `<cwd>/.mark2.toml`, и
//!   mark2 считает от него порог компакции;
//! * `max_turns` — предохранитель агента для этого прогона (> 0): ранер
//!   дописывает `max_turns = N` в секцию `[agent]` того же `.mark2.toml`
//!   (у mark2 по умолчанию 50 — проектным задачам этого мало);
//! * `verify_timeout_seconds` — бюджет `verify.sh` (> 0) вместо общего
//!   [`crate::runner::VERIFY_TIMEOUT_SECONDS`]: проверка, которая поднимает
//!   сервер и headless-браузер, в 30 секунд не укладывается;
//! * `uitest_env` — задаче нужно окружение проектных verify, `uitest/` рядом с
//!   директорией задач (`bench/uitest`, см. [`Task::uitest_dir`]): ранер до
//!   первой модели собирает `.venv` его `bootstrap.sh` и запускает mark2 с
//!   директорией, где лежит один `python3` (обёртка над интерпретатором
//!   venv), первой в `PATH` — `python3` модели тот же, которым verify потом
//!   гоняет её сервер (с aiohttp), сеть до PyPI модели не нужна, а `pip`
//!   venv ей не виден;
//! * `post_edit_command` — команда post-edit сенсора mark2 (непустая): ранер
//!   дописывает `post_edit_command = "…"` в секцию `[agent]` `.mark2.toml`, и
//!   mark2 запускает её из корня песочницы после каждой успешной записи
//!   модели; `post_edit_timeout_seconds` (> 0, только вместе с командой) — её
//!   таймаут. `--agent-override` с тем же ключом перекрывает задачу (пустая
//!   команда — выключить сенсор): так меряется абляция «с сенсором / без».
//!
//! Всё, что можно проверить до запуска модели, проверяется до запуска модели:
//! отсутствующий `verify.sh`, пустой `prompt.md`, опечатка в ключе или
//! значении `task.toml` — ошибка загрузки, а не сюрприз через пять минут
//! прогона.

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
/// Окружение проектных verify: сосед директории задач (`bench/tasks` →
/// `bench/uitest`).
pub const UITEST_DIR: &str = "uitest";
/// Скрипт, который собирает и проверяет `uitest/.venv` (идемпотентный).
pub const UITEST_BOOTSTRAP: &str = "bootstrap.sh";

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
    /// Политика доступа к тулам, с которой запускается mark2.
    pub policy: TaskPolicy,
    /// Окно модели в токенах для этого прогона, если задача его задаёт.
    pub context_limit: Option<u64>,
    /// Предохранитель `max_turns` агента для этого прогона, если задача его задаёт.
    pub max_turns: Option<usize>,
    /// Бюджет `verify.sh` в секундах, если задача его задаёт.
    pub verify_timeout_seconds: Option<u64>,
    /// Нужно ли задаче окружение uitest (см. [`Task::uitest_dir`]).
    pub uitest_env: bool,
    /// Post-edit сенсор mark2 для этой задачи, если она его задаёт.
    pub post_edit: Option<PostEdit>,
}

/// Post-edit сенсор задачи — что ранер пишет в `[agent]` песочницы.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PostEdit {
    /// `post_edit_command`: команда проверки, из корня песочницы.
    pub command: String,
    /// `post_edit_timeout_seconds`, если задача задала свой (иначе — дефолт
    /// mark2).
    pub timeout_seconds: Option<u64>,
}

/// Политика доступа к тулам в `task.toml`.
///
/// Намеренно уже, чем `--policy` у mark2: `ask-writes` в print mode без
/// человека за терминалом означает «отказать во всём мутирующем» — то же
/// самое, что `read-only`, только с менее честным именем.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskPolicy {
    /// Ничего не спрашивать — обычный прогон бенча.
    #[default]
    Yolo,
    /// Только чтение: мутирующие тулы отклоняются ядром.
    ReadOnly,
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

    /// `uitest/` бенча, если задаче нужно это окружение (`uitest_env = true`).
    ///
    /// Директория задачи — `bench/tasks/<ID>-<slug>`, так что `uitest/` лежит
    /// двумя уровнями выше: та же, которую находит `verify.sh` через
    /// `$here/../../uitest` — модель и проверка видят один интерпретатор.
    pub fn uitest_dir(&self) -> Option<PathBuf> {
        self.uitest_env.then(|| uitest_dir_of(&self.dir))
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
    #[serde(default)]
    policy: TaskPolicy,
    #[serde(default)]
    context_limit: Option<u64>,
    #[serde(default)]
    max_turns: Option<usize>,
    #[serde(default)]
    verify_timeout_seconds: Option<u64>,
    #[serde(default)]
    uitest_env: bool,
    #[serde(default)]
    post_edit_command: Option<String>,
    #[serde(default)]
    post_edit_timeout_seconds: Option<u64>,
}

impl TaskSection {
    /// Post-edit сенсор задачи из пары полей (уже проверенных
    /// [`parse_task_file`]).
    fn post_edit(&self) -> Option<PostEdit> {
        self.post_edit_command.as_ref().map(|command| PostEdit {
            command: command.clone(),
            timeout_seconds: self.post_edit_timeout_seconds,
        })
    }
}

/// `uitest/` для директории задачи: два уровня вверх (`bench/tasks/T17-…` →
/// `bench/`) плюс [`UITEST_DIR`]. `task_dir` уже абсолютный и нормализованный
/// ([`load_task`]), так что предки — настоящие директории, а не `..`.
fn uitest_dir_of(task_dir: &Path) -> PathBuf {
    task_dir
        .ancestors()
        .nth(2)
        .unwrap_or(task_dir)
        .join(UITEST_DIR)
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
    if section.context_limit == Some(0) {
        bail!("{origin}: context_limit должен быть больше нуля");
    }
    if section.max_turns == Some(0) {
        bail!("{origin}: max_turns должен быть больше нуля");
    }
    if section.verify_timeout_seconds == Some(0) {
        bail!("{origin}: verify_timeout_seconds должен быть больше нуля");
    }
    // Пустая команда в mark2 значит «выключить» — в задаче это бессмыслица:
    // сенсор без команды задают отсутствием ключа.
    if section
        .post_edit_command
        .as_deref()
        .is_some_and(|command| command.trim().is_empty())
    {
        bail!("{origin}: post_edit_command пуст; убери ключ, чтобы гонять задачу без сенсора");
    }
    if section.post_edit_timeout_seconds == Some(0) {
        bail!("{origin}: post_edit_timeout_seconds должен быть больше нуля");
    }
    if section.post_edit_timeout_seconds.is_some() && section.post_edit_command.is_none() {
        bail!("{origin}: post_edit_timeout_seconds без post_edit_command — таймаут не для чего");
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

    // Окружение, которое не из чего собрать, — ошибка загрузки, а не прогон,
    // в котором модель час ищет aiohttp.
    if section.uitest_env {
        let bootstrap = uitest_dir_of(dir).join(UITEST_BOOTSTRAP);
        if !bootstrap.is_file() {
            bail!(
                "{}: uitest_env = true, но нет {} — окружение uitest не из чего собрать",
                task_path.display(),
                bootstrap.display()
            );
        }
    }

    Ok(Task {
        id,
        slug,
        dir: dir.to_path_buf(),
        prompt,
        timeout_seconds: section.timeout_seconds,
        repeats: section.repeats.unwrap_or(DEFAULT_REPEATS),
        post_edit: section.post_edit(),
        tags: section.tags,
        policy: section.policy,
        context_limit: section.context_limit,
        max_turns: section.max_turns,
        verify_timeout_seconds: section.verify_timeout_seconds,
        uitest_env: section.uitest_env,
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
        // Новые поля необязательны: старый task.toml значит то же, что раньше.
        assert_eq!(task.policy, TaskPolicy::Yolo);
        assert_eq!(task.context_limit, None);
        assert_eq!(task.max_turns, None);
        assert_eq!(task.verify_timeout_seconds, None);
        assert!(!task.uitest_env);
        assert_eq!(task.uitest_dir(), None);
        assert_eq!(task.post_edit, None);
    }

    #[test]
    fn policy_and_context_limit_are_loaded() {
        let root = TempDir::new().unwrap();
        let dir = write_task(
            root.path(),
            "T15-x",
            "[task]\ntimeout_seconds = 300\npolicy = \"read-only\"\ncontext_limit = 8192\n",
        );
        let task = load_task(&dir).unwrap();
        assert_eq!(task.policy, TaskPolicy::ReadOnly);
        assert_eq!(task.context_limit, Some(8192));

        std::fs::write(
            dir.join(TASK_FILE),
            "[task]\ntimeout_seconds = 300\npolicy = \"yolo\"\n",
        )
        .unwrap();
        let task = load_task(&dir).unwrap();
        assert_eq!(task.policy, TaskPolicy::Yolo);
        assert_eq!(task.context_limit, None);
    }

    /// Допустимы ровно два написания: `ask-writes` в `-p` выродился бы в тот же
    /// read-only, а `read_only` — опечатка, которую лучше поймать сразу.
    #[test]
    fn unknown_policy_is_a_load_error_naming_the_alternatives() {
        let root = TempDir::new().unwrap();
        for bad in ["whatever", "ask-writes", "read_only", "Read-Only"] {
            let dir = write_task(
                root.path(),
                "T16-x",
                &format!("[task]\ntimeout_seconds = 60\npolicy = \"{bad}\"\n"),
            );
            let text = format!("{:#}", load_task(&dir).unwrap_err());
            assert!(text.contains(&dir.display().to_string()), "{text}");
            assert!(text.contains(bad), "{bad}: {text}");
            assert!(
                text.contains("yolo") && text.contains("read-only"),
                "{bad}: {text}"
            );
        }
    }

    #[test]
    fn zero_or_mistyped_context_limit_is_a_load_error() {
        let root = TempDir::new().unwrap();
        let dir = write_task(
            root.path(),
            "T15-x",
            "[task]\ntimeout_seconds = 60\ncontext_limit = 0\n",
        );
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("context_limit"), "{text}");
        assert!(text.contains("больше нуля"), "{text}");
        assert!(text.contains(&dir.display().to_string()), "{text}");

        for bad in ["context_limit = -1", "context_limit = \"8k\""] {
            std::fs::write(
                dir.join(TASK_FILE),
                format!("[task]\ntimeout_seconds = 60\n{bad}\n"),
            )
            .unwrap();
            let text = format!("{:#}", load_task(&dir).unwrap_err());
            assert!(text.contains("task.toml"), "{bad}: {text}");
        }
    }

    #[test]
    fn max_turns_and_verify_timeout_are_loaded() {
        let root = TempDir::new().unwrap();
        let dir = write_task(
            root.path(),
            "T17-x",
            "[task]\ntimeout_seconds = 1800\nmax_turns = 80\nverify_timeout_seconds = 300\n",
        );
        let task = load_task(&dir).unwrap();
        assert_eq!(task.max_turns, Some(80));
        assert_eq!(task.verify_timeout_seconds, Some(300));
        // Соседние поля не задеты.
        assert_eq!(task.policy, TaskPolicy::Yolo);
        assert_eq!(task.context_limit, None);
    }

    /// Ноль, отрицательное число и строка — ошибка загрузки с путём и именем
    /// поля, а не прогон, который упадёт на первом же ходу или на verify.
    #[test]
    fn zero_or_mistyped_max_turns_and_verify_timeout_are_load_errors() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T17-x", "[task]\ntimeout_seconds = 60\n");
        for field in ["max_turns", "verify_timeout_seconds"] {
            std::fs::write(
                dir.join(TASK_FILE),
                format!("[task]\ntimeout_seconds = 60\n{field} = 0\n"),
            )
            .unwrap();
            let text = format!("{:#}", load_task(&dir).unwrap_err());
            assert!(text.contains(field), "{field}: {text}");
            assert!(text.contains("больше нуля"), "{field}: {text}");
            assert!(text.contains(&dir.display().to_string()), "{field}: {text}");

            for bad in ["-1", "\"80\""] {
                std::fs::write(
                    dir.join(TASK_FILE),
                    format!("[task]\ntimeout_seconds = 60\n{field} = {bad}\n"),
                )
                .unwrap();
                let text = format!("{:#}", load_task(&dir).unwrap_err());
                assert!(text.contains("task.toml"), "{field} = {bad}: {text}");
            }
        }

        // Опечатка в имени поля ловится deny_unknown_fields.
        std::fs::write(
            dir.join(TASK_FILE),
            "[task]\ntimeout_seconds = 60\nmax_turn = 80\n",
        )
        .unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("max_turn"), "{text}");
    }

    #[test]
    fn post_edit_command_and_timeout_are_loaded() {
        let root = TempDir::new().unwrap();
        let dir = write_task(
            root.path(),
            "T04-x",
            "[task]\ntimeout_seconds = 240\npost_edit_command = \"python3 -B test.py\"\n",
        );
        let task = load_task(&dir).unwrap();
        assert_eq!(
            task.post_edit,
            Some(PostEdit {
                command: "python3 -B test.py".into(),
                timeout_seconds: None,
            })
        );

        std::fs::write(
            dir.join(TASK_FILE),
            "[task]\ntimeout_seconds = 240\npost_edit_command = \"make check\"\n\
             post_edit_timeout_seconds = 90\n",
        )
        .unwrap();
        let task = load_task(&dir).unwrap();
        assert_eq!(
            task.post_edit,
            Some(PostEdit {
                command: "make check".into(),
                timeout_seconds: Some(90),
            })
        );
        // Соседние поля не задеты.
        assert_eq!(task.max_turns, None);
        assert_eq!(task.policy, TaskPolicy::Yolo);

        // Нет ключа — нет сенсора.
        std::fs::write(dir.join(TASK_FILE), "[task]\ntimeout_seconds = 240\n").unwrap();
        assert_eq!(load_task(&dir).unwrap().post_edit, None);
    }

    /// Пустая команда, нулевой таймаут, таймаут без команды и кривой тип —
    /// ошибка загрузки с путём и именем поля, а не прогон с выключенным или
    /// сломанным сенсором.
    #[test]
    fn bad_post_edit_settings_are_load_errors() {
        let root = TempDir::new().unwrap();
        let dir = write_task(root.path(), "T04-x", "[task]\ntimeout_seconds = 60\n");
        for (lines, expected) in [
            ("post_edit_command = \"\"", "post_edit_command пуст"),
            ("post_edit_command = \"   \"", "post_edit_command пуст"),
            (
                "post_edit_command = \"x\"\npost_edit_timeout_seconds = 0",
                "больше нуля",
            ),
            ("post_edit_timeout_seconds = 30", "без post_edit_command"),
            ("post_edit_command = 1", "task.toml"),
            (
                "post_edit_command = \"x\"\npost_edit_timeout_seconds = \"30\"",
                "task.toml",
            ),
            ("post_edit = \"x\"", "post_edit"),
        ] {
            std::fs::write(
                dir.join(TASK_FILE),
                format!("[task]\ntimeout_seconds = 60\n{lines}\n"),
            )
            .unwrap();
            let text = format!("{:#}", load_task(&dir).unwrap_err());
            assert!(text.contains(expected), "{lines}: {text}");
            assert!(text.contains(&dir.display().to_string()), "{lines}: {text}");
        }
    }

    /// Раскладка как в репозитории: `<root>/tasks/<задача>` и `<root>/uitest/`.
    fn write_uitest(root: &Path) -> PathBuf {
        let uitest = root.join(UITEST_DIR);
        std::fs::create_dir_all(&uitest).unwrap();
        std::fs::write(uitest.join(UITEST_BOOTSTRAP), "#!/bin/sh\nexit 0\n").unwrap();
        uitest
    }

    #[test]
    fn uitest_env_points_at_the_uitest_dir_next_to_the_tasks_dir() {
        let root = TempDir::new().unwrap();
        let uitest = write_uitest(root.path());
        let dir = write_task(
            &root.path().join("tasks"),
            "T17-x",
            "[task]\ntimeout_seconds = 60\nuitest_env = true\n",
        );
        let task = load_task(&dir).unwrap();
        assert!(task.uitest_env);
        // Тот же путь, что у verify.sh (`$here/../../uitest`), развёрнутый.
        assert_eq!(
            task.uitest_dir(),
            Some(std::fs::canonicalize(&uitest).unwrap())
        );
        assert!(task.uitest_dir().unwrap().is_absolute());
        // Соседние поля не задеты.
        assert_eq!(task.policy, TaskPolicy::Yolo);
        assert_eq!(task.max_turns, None);

        // Явный false — то же, что отсутствие поля: окружения не нужно.
        std::fs::write(
            dir.join(TASK_FILE),
            "[task]\ntimeout_seconds = 60\nuitest_env = false\n",
        )
        .unwrap();
        let task = load_task(&dir).unwrap();
        assert!(!task.uitest_env);
        assert_eq!(task.uitest_dir(), None);
    }

    /// Окружение, которое не из чего собрать, и кривое значение — ошибка
    /// загрузки с путём, а не прогон, в котором у модели нет aiohttp.
    #[test]
    fn uitest_env_without_bootstrap_or_mistyped_is_a_load_error() {
        let root = TempDir::new().unwrap();
        let dir = write_task(
            &root.path().join("tasks"),
            "T17-x",
            "[task]\ntimeout_seconds = 60\nuitest_env = true\n",
        );
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("uitest_env"), "{text}");
        assert!(text.contains(UITEST_BOOTSTRAP), "{text}");
        assert!(text.contains(&dir.display().to_string()), "{text}");

        // Без поля bootstrap.sh не нужен вовсе.
        std::fs::write(dir.join(TASK_FILE), "[task]\ntimeout_seconds = 60\n").unwrap();
        assert!(load_task(&dir).is_ok());

        write_uitest(root.path());
        for bad in ["\"yes\"", "1", "\"true\""] {
            std::fs::write(
                dir.join(TASK_FILE),
                format!("[task]\ntimeout_seconds = 60\nuitest_env = {bad}\n"),
            )
            .unwrap();
            let text = format!("{:#}", load_task(&dir).unwrap_err());
            assert!(text.contains("task.toml"), "uitest_env = {bad}: {text}");
        }

        // Опечатка в имени поля ловится deny_unknown_fields.
        std::fs::write(
            dir.join(TASK_FILE),
            "[task]\ntimeout_seconds = 60\nuitest = true\n",
        )
        .unwrap();
        let text = format!("{:#}", load_task(&dir).unwrap_err());
        assert!(text.contains("uitest"), "{text}");
    }

    /// Реальные задачи репозитория обязаны грузиться текущим контрактом —
    /// иначе бенч упадёт на старте. Старые task.toml без новых полей значат
    /// ровно то же, что и раньше.
    #[test]
    fn repository_tasks_load_with_the_current_contract() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DEFAULT_TASKS_DIR);
        let tasks = discover_tasks(&dir).unwrap();
        let ids: Vec<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
        for expected in (1..=19).map(|n| format!("T{n:02}")) {
            assert!(ids.contains(&expected.as_str()), "нет {expected}: {ids:?}");
        }
        let first = tasks.iter().find(|task| task.id == "T01").unwrap();
        assert_eq!(first.policy, TaskPolicy::Yolo);
        assert_eq!(first.context_limit, None);
        assert_eq!(first.max_turns, None);
        assert_eq!(first.verify_timeout_seconds, None);
        assert!(!first.uitest_env, "базовым задачам PATH не трогаем");

        // Проектная задача: больше ходов и свой бюджет на трёхслойный verify.
        let project = tasks.iter().find(|task| task.id == "T17").unwrap();
        assert_eq!(project.slug, "ws-chat");
        assert_eq!(project.max_turns, Some(80));
        assert_eq!(project.timeout_seconds, 1800);
        assert!(project.verify_timeout_seconds.is_some_and(|s| s > 30));
        assert!(project.tags.iter().any(|tag| tag == "project"));
        assert!(project.setup_dir().is_none(), "T17 стартует с пустой cwd");
        // python3 модели — тот же, что у verify: bench/uitest/.venv.
        let uitest = std::fs::canonicalize(dir.join("..").join(UITEST_DIR)).unwrap();
        assert_eq!(project.uitest_dir(), Some(uitest.clone()));
        assert!(uitest.join(UITEST_BOOTSTRAP).is_file());

        // Задачи памяти: файл памяти — dotfile в setup/, ранер копирует его
        // в cwd прогона, и mark2 находит его там как ближайший.
        for (id, slug, tag) in [
            ("T18", "memory-recall", "read"),
            ("T19", "memory-store", "write"),
        ] {
            let task = tasks.iter().find(|task| task.id == id).unwrap();
            assert_eq!(task.slug, slug);
            assert_eq!(task.timeout_seconds, 120);
            assert_eq!(task.policy, TaskPolicy::Yolo);
            assert!(
                task.tags.iter().any(|t| t == "memory"),
                "{id}: {:?}",
                task.tags
            );
            assert!(task.tags.iter().any(|t| t == tag), "{id}: {:?}", task.tags);
            let memory = task
                .setup_dir()
                .expect("у задачи памяти есть setup/")
                .join(mark2_core::memory::MEMORY_FILE);
            let text = std::fs::read_to_string(&memory).unwrap();
            assert!(
                mark2_core::Memory::from_text(&memory, &text).is_some(),
                "{id}: память в setup/ должна быть непустой — иначе она выключена"
            );
            assert!(text.starts_with("# Memory\n"), "{id}: {text:?}");
        }
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
