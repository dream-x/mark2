//! Сырьё прогона: то, что уезжает в `results.json`.
//!
//! Принцип один: **в JSON лежит всё, из чего можно пересобрать любой отчёт**.
//! `report.md` — производная от этих структур и ничего не знает сверх них,
//! поэтому старый прогон можно перечитать новым кодом отчёта.
//!
//! Пути артефактов (stdout/stderr/сессия) хранятся относительно директории
//! результатов: каталог с прогоном должен переживать переезд и архивацию.

use serde::{Deserialize, Serialize};

use crate::metrics::{SessionMetrics, Tokens};
use crate::models::ModelSpec;
use crate::task::TaskPolicy;

/// Версия схемы `results.json`. Растёт при несовместимых изменениях.
pub const SCHEMA_VERSION: u32 = 1;

/// Прогон целиком.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Results {
    /// Версия схемы файла.
    pub schema_version: u32,
    /// Метка прогона из `--label`.
    pub label: String,
    /// Имя директории результатов (`20260824-120000-baseline`).
    pub run_id: String,
    /// Начало прогона, RFC 3339.
    pub started_at: String,
    /// Конец прогона, RFC 3339.
    pub finished_at: String,
    /// С чем гоняли.
    pub config: RunConfig,
    /// Все повторы всех пар модель×задача в порядке выполнения.
    pub runs: Vec<RunRecord>,
}

/// Конфигурация прогона — чтобы результат читался без исходного окружения.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunConfig {
    /// Версия ранера.
    pub bench_version: String,
    /// Путь к бинарю mark2.
    pub mark2: String,
    /// Директория задач.
    pub tasks_dir: String,
    /// Файл матрицы моделей.
    pub models_file: String,
    /// Значение `--repeats`, если задавали.
    pub repeats_override: Option<usize>,
    /// Таймаут `verify.sh` по умолчанию, секунды (задача может задать свой —
    /// [`TaskRecord::verify_timeout_seconds`]).
    pub verify_timeout_seconds: u64,
    /// Промпт прогрева (не таймируется, в результаты не попадает).
    pub warmup_prompt: String,
    /// Что писалось в `<cwd>/.mark2.toml` каждого прогона. Задача с
    /// `max_turns` (см. [`TaskRecord::max_turns`]) дописывает строку в
    /// `[agent]`, с `context_limit` (см. [`TaskRecord::context_limit`]) — секцию
    /// `[models."<id модели>"]`.
    pub project_config: String,
    /// Матрица моделей после фильтра `--models`.
    pub models: Vec<ModelSpec>,
    /// Задачи после фильтра `--tasks`.
    pub tasks: Vec<TaskRecord>,
}

/// Задача в том виде, в каком её увидел ранер.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Идентификатор (`T04`).
    pub id: String,
    /// Slug (`fix-bug-test`).
    pub slug: String,
    /// Директория задачи.
    pub dir: String,
    /// Базовый таймаут задачи, секунды.
    pub timeout_seconds: u64,
    /// Эффективное число повторов (с учётом `--repeats`).
    pub repeats: usize,
    /// Теги задачи.
    pub tags: Vec<String>,
    /// Политика доступа к тулам. В старых `results.json` поля нет — тогда
    /// гоняли только `--yolo`.
    #[serde(default)]
    pub policy: TaskPolicy,
    /// Окно модели в токенах, если задача его задавала.
    #[serde(default)]
    pub context_limit: Option<u64>,
    /// Предохранитель `max_turns` агента, если задача его задавала (иначе —
    /// дефолт mark2). В старых `results.json` поля нет.
    #[serde(default)]
    pub max_turns: Option<usize>,
    /// Свой бюджет `verify.sh`, секунды, если задача его задавала (иначе —
    /// [`RunConfig::verify_timeout_seconds`]).
    #[serde(default)]
    pub verify_timeout_seconds: Option<u64>,
    /// Запускался ли mark2 с `python3` интерпретатора `uitest/.venv` первым в
    /// `PATH`. В старых `results.json` поля нет — тогда PATH не трогали никому.
    #[serde(default)]
    pub uitest_env: bool,
    /// Промпт целиком — прогон должен быть воспроизводим по одному файлу.
    pub prompt: String,
}

impl TaskRecord {
    /// Имя директории задачи (`T04-fix-bug-test`).
    pub fn name(&self) -> String {
        if self.slug.is_empty() {
            self.id.clone()
        } else {
            format!("{}-{}", self.id, self.slug)
        }
    }
}

/// Один прогон: одна пара модель×задача, один повтор.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Алиас модели.
    pub model: String,
    /// Идентификатор модели у сервера.
    pub model_id: String,
    /// Идентификатор задачи.
    pub task: String,
    /// Номер повтора, с единицы.
    pub repeat: usize,
    /// Момент старта, RFC 3339.
    pub started_at: String,
    /// Прошла ли проверка: `verify.sh` вернул 0.
    pub pass: bool,
    /// Код возврата mark2 (`None` — убит по таймауту или сигналом).
    pub exit_code: Option<i32>,
    /// Убит ли прогон по таймауту.
    pub timed_out: bool,
    /// Время работы mark2 (только вызов, без копирования setup и verify).
    pub wall_time_seconds: f64,
    /// Эффективный бюджет времени: `timeout_seconds` задачи × множитель модели.
    pub timeout_seconds: f64,
    /// Расход токенов из stderr, если строка нашлась.
    pub tokens: Option<Tokens>,
    /// Метрики из JSONL-сессии, если она была записана.
    pub session: Option<SessionMetrics>,
    /// Результат `verify.sh`.
    pub verify: Option<VerifyOutcome>,
    /// Сколько непробельных символов модель написала в stdout.
    pub answer_chars: usize,
    /// Путь к захваченному stdout относительно директории результатов.
    pub stdout_path: Option<String>,
    /// Путь к захваченному stderr относительно директории результатов.
    pub stderr_path: Option<String>,
    /// Путь к копии сессии относительно директории результатов.
    pub session_path: Option<String>,
    /// Ошибка самого ранера (не смог запустить, не смог скопировать setup).
    pub error: Option<String>,
}

impl RunRecord {
    /// Пустой ответ при завершившемся прогоне — подозрительно даже при pass.
    pub fn empty_answer(&self) -> bool {
        self.error.is_none() && !self.timed_out && self.answer_chars == 0
    }
}

/// Итог `verify.sh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyOutcome {
    /// Код возврата (`None` — убит по таймауту).
    pub exit_code: Option<i32>,
    /// Упёрся ли verify в свой таймаут.
    pub timed_out: bool,
    /// stdout проверки (обрезанный).
    pub stdout: String,
    /// stderr проверки (обрезанный).
    pub stderr: String,
}

impl VerifyOutcome {
    /// Прошла ли проверка.
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

/// Медиана выборки. `None` — пустая выборка.
///
/// Медиана, а не среднее: агентные прогоны недетерминированы, один зависший
/// повтор не должен тянуть за собой всю ячейку таблицы.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_handles_odd_even_and_empty() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[5.0]), Some(5.0));
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn verify_outcome_passes_only_on_clean_zero() {
        let ok = VerifyOutcome {
            exit_code: Some(0),
            timed_out: false,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(ok.passed());
        assert!(
            !VerifyOutcome {
                exit_code: Some(1),
                ..ok.clone()
            }
            .passed()
        );
        assert!(
            !VerifyOutcome {
                exit_code: None,
                timed_out: true,
                ..ok.clone()
            }
            .passed()
        );
    }

    #[test]
    fn task_record_name_survives_a_missing_slug() {
        let mut record = TaskRecord {
            id: "T01".into(),
            slug: "create-file".into(),
            dir: "bench/tasks/T01-create-file".into(),
            timeout_seconds: 120,
            repeats: 3,
            tags: vec![],
            policy: TaskPolicy::Yolo,
            context_limit: None,
            max_turns: None,
            verify_timeout_seconds: None,
            uitest_env: false,
            prompt: "…".into(),
        };
        assert_eq!(record.name(), "T01-create-file");
        record.slug = String::new();
        assert_eq!(record.name(), "T01");
    }
}
