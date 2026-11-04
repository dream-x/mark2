//! `mark2-bench` — ранер бенчмарка из [`bench/STRATEGY.md`](../../../bench/STRATEGY.md).
//!
//! Бенч отвечает на два вопроса: какая локальная модель даёт рабочего агента и
//! не сломало ли изменение харнеса то, что работало. Отсюда устройство ранера:
//!
//! * он **ничего не знает про модели** — только запускает бинарь `mark2` как
//!   чёрный ящик и читает то, что тот оставил после себя (exit code, stderr,
//!   JSONL-сессию, артефакты в рабочей директории);
//! * оценка **программная**: `verify.sh` задачи, exit 0 = pass. Никаких
//!   LLM-судей и сравнения текста ответа;
//! * прогоны **герметичны**: свежая tmp-cwd с копией `setup/` и отдельный
//!   tmp-`$HOME` на каждый прогон — изолируется и пользовательский конфиг, и
//!   директория сессий (сессию потом забираем именно оттуда).
//!
//! Модули: [`models`] — матрица моделей (`bench/models.toml`), [`task`] —
//! формат задачи на диске, [`metrics`] — извлечение метрик из stderr и сессии,
//! [`telemetry`] — срабатывания контролей харнеса по маркерам stderr,
//! [`runner`] — протокол прогона, [`results`] — сырьё для `results.json`,
//! [`report`] — сборка `report.md`.

pub mod metrics;
pub mod models;
pub mod report;
pub mod results;
pub mod runner;
pub mod task;
pub mod telemetry;

use std::path::{Path, PathBuf};

/// Версия ранера.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Развернуть путь в абсолютный.
///
/// **Инвариант ранера: всё, что переживает смену cwd, хранится абсолютным.**
/// Прогон живёт в чужой рабочей директории — mark2 запускается с
/// `current_dir` = tmp-cwd, `sh verify.sh` тоже, — поэтому относительный путь
/// (`bench/tasks/T01-create-file/verify.sh`) из них не резолвится: получаем
/// `sh: … No such file or directory` и exit 127 вместо честного вердикта.
/// Разворачиваем один раз при загрузке, а не в точке использования.
///
/// Функция намеренно не падает: несуществующий путь просто не канонизируется —
/// ошибку о нём выдаст тот, кто попробует его прочитать, и она будет понятнее.
pub fn resolve_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_makes_relative_paths_absolute_and_normalized() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        // `..` схлопывается: путь уезжает в аргументы чужих процессов.
        let weird = nested.join("..").join("b");
        let resolved = resolve_path(&weird);
        assert!(resolved.is_absolute(), "{}", resolved.display());
        assert!(
            !resolved.components().any(|c| c.as_os_str() == ".."),
            "{}",
            resolved.display()
        );
        assert_eq!(resolved, std::fs::canonicalize(&nested).unwrap());
    }

    #[test]
    fn resolve_does_not_fail_on_missing_paths() {
        let resolved = resolve_path("нет-такого-файла");
        assert!(resolved.is_absolute(), "{}", resolved.display());
        assert!(resolved.ends_with("нет-такого-файла"));
    }
}
