//! Матрица моделей бенча: разбор `bench/models.toml`.
//!
//! Файл описывает, *куда* и *под каким именем* ходить, плюс поправку на
//! скорость железа: `timeout_multiplier` умножает `timeout_seconds` задачи.
//! Так задача остаётся одна на всех, а 9b на localhost и 27b по сети получают
//! честные бюджеты времени.
//!
//! Ключи не угадываются: `deny_unknown_fields` превращает опечатку в ошибку с
//! путём файла, а не в молчаливо потерянную настройку.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Путь к матрице моделей по умолчанию (относительно корня репозитория).
pub const DEFAULT_MODELS_FILE: &str = "bench/models.toml";

/// Одна модель матрицы.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// Короткое имя для CLI, колонок отчёта и путей артефактов.
    pub alias: String,
    /// Идентификатор модели в терминах сервера (уходит в `--model`).
    pub model: String,
    /// База OpenAI-совместимого API (уходит в `--base-url`).
    pub base_url: String,
    /// Множитель таймаута задачи под скорость модели/сети.
    #[serde(default = "default_timeout_multiplier")]
    pub timeout_multiplier: f64,
    /// Переменная окружения с API-ключом (уходит в `--api-key-env`), если
    /// сервер его требует. Сам ключ в файле не хранится никогда.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

fn default_timeout_multiplier() -> f64 {
    1.0
}

/// Файл целиком: `[[models]]`-массив.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelsFile {
    #[serde(default)]
    models: Vec<ModelSpec>,
}

/// Разобрать содержимое `models.toml`. `origin` попадает в текст ошибок.
pub fn parse_models(raw: &str, origin: &str) -> Result<Vec<ModelSpec>> {
    let file: ModelsFile =
        toml::from_str(raw).with_context(|| format!("не удалось разобрать {origin}"))?;

    if file.models.is_empty() {
        bail!("{origin}: не описано ни одной модели (нужен хотя бы один блок [[models]])");
    }

    let mut seen = BTreeSet::new();
    for model in &file.models {
        if model.alias.trim().is_empty() {
            bail!("{origin}: пустой alias у модели {:?}", model.model);
        }
        if model.model.trim().is_empty() {
            bail!("{origin}: пустой model у алиаса {:?}", model.alias);
        }
        if model.base_url.trim().is_empty() {
            bail!("{origin}: пустой base_url у алиаса {:?}", model.alias);
        }
        if !(model.timeout_multiplier.is_finite() && model.timeout_multiplier > 0.0) {
            bail!(
                "{origin}: timeout_multiplier у {:?} должен быть положительным числом, а не {}",
                model.alias,
                model.timeout_multiplier
            );
        }
        if !seen.insert(model.alias.clone()) {
            bail!("{origin}: алиас {:?} встречается дважды", model.alias);
        }
    }

    Ok(file.models)
}

/// Прочитать и разобрать `models.toml`.
pub fn load_models(path: &Path) -> Result<Vec<ModelSpec>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("не удалось прочитать матрицу моделей {}", path.display()))?;
    parse_models(&raw, &path.display().to_string())
}

/// Отобрать модели по алиасам, сохраняя порядок файла.
///
/// Пустой фильтр — взять все. Неизвестный алиас — ошибка со списком доступных:
/// опечатка в `--models` не должна тихо превращаться в прогон не того, что
/// просили.
pub fn select_models(all: Vec<ModelSpec>, wanted: &[String]) -> Result<Vec<ModelSpec>> {
    if wanted.is_empty() {
        return Ok(all);
    }

    let known: Vec<&str> = all.iter().map(|m| m.alias.as_str()).collect();
    for alias in wanted {
        if !known.contains(&alias.as_str()) {
            bail!(
                "неизвестная модель {alias:?}; в матрице есть: {}",
                known.join(", ")
            );
        }
    }

    Ok(all
        .into_iter()
        .filter(|model| wanted.iter().any(|alias| alias == &model.alias))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [[models]]
        alias = "qwen9"
        model = "qwen3.5:9b"
        base_url = "http://localhost:11434/v1"
        timeout_multiplier = 1.0

        [[models]]
        alias = "oss120"
        model = "gpt-oss:120b-cloud"
        base_url = "http://localhost:11434/v1"
        timeout_multiplier = 2.5
        api_key_env = "OLLAMA_API_KEY"
    "#;

    #[test]
    fn parses_matrix_in_file_order() {
        let models = parse_models(SAMPLE, "test.toml").unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].alias, "qwen9");
        assert_eq!(models[0].model, "qwen3.5:9b");
        assert_eq!(models[0].base_url, "http://localhost:11434/v1");
        assert_eq!(models[0].timeout_multiplier, 1.0);
        assert_eq!(models[0].api_key_env, None);
        assert_eq!(models[1].alias, "oss120");
        assert_eq!(models[1].timeout_multiplier, 2.5);
        assert_eq!(models[1].api_key_env.as_deref(), Some("OLLAMA_API_KEY"));
    }

    #[test]
    fn timeout_multiplier_defaults_to_one() {
        let models = parse_models(
            r#"
            [[models]]
            alias = "a"
            model = "m"
            base_url = "http://x/v1"
            "#,
            "test.toml",
        )
        .unwrap();
        assert_eq!(models[0].timeout_multiplier, 1.0);
    }

    /// Реальный файл репозитория обязан разбираться — иначе бенч не запустится.
    /// Состав матрицы зафиксирован: меняется осознанно, вместе с этим тестом.
    #[test]
    fn repository_models_file_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DEFAULT_MODELS_FILE);
        let models = load_models(&path).unwrap();
        let aliases: Vec<&str> = models.iter().map(|m| m.alias.as_str()).collect();
        assert_eq!(aliases, ["qwen9", "bolt4", "flint40", "qwen27", "gemma31c"]);
    }

    #[test]
    fn unknown_field_is_an_error_with_the_file_name() {
        let error = parse_models(
            r#"
            [[models]]
            alias = "a"
            model = "m"
            base_url = "http://x/v1"
            timout_multiplier = 2.0
            "#,
            "bench/models.toml",
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("bench/models.toml"), "{text}");
        assert!(text.contains("timout_multiplier"), "{text}");
    }

    #[test]
    fn empty_matrix_and_bad_values_are_rejected() {
        for (raw, expected) in [
            ("", "ни одной модели"),
            (
                r#"
                [[models]]
                alias = ""
                model = "m"
                base_url = "http://x/v1"
                "#,
                "пустой alias",
            ),
            (
                r#"
                [[models]]
                alias = "a"
                model = "m"
                base_url = "http://x/v1"
                timeout_multiplier = 0.0
                "#,
                "timeout_multiplier",
            ),
            (
                r#"
                [[models]]
                alias = "a"
                model = "m"
                base_url = "http://x/v1"

                [[models]]
                alias = "a"
                model = "m2"
                base_url = "http://x/v1"
                "#,
                "дважды",
            ),
        ] {
            let error = parse_models(raw, "bench/models.toml").unwrap_err();
            let text = format!("{error:#}");
            assert!(
                text.contains(expected),
                "ждали {expected:?}, получили {text}"
            );
            assert!(text.contains("bench/models.toml"), "{text}");
        }
    }

    #[test]
    fn missing_file_error_names_the_path() {
        let error = load_models(Path::new("/nope/models.toml")).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("/nope/models.toml"), "{text}");
    }

    #[test]
    fn selection_keeps_file_order_and_rejects_typos() {
        let all = parse_models(SAMPLE, "test.toml").unwrap();
        assert_eq!(select_models(all.clone(), &[]).unwrap().len(), 2);

        let picked = select_models(all.clone(), &["oss120".into(), "qwen9".into()]).unwrap();
        let aliases: Vec<&str> = picked.iter().map(|m| m.alias.as_str()).collect();
        assert_eq!(aliases, ["qwen9", "oss120"], "порядок файла, а не флага");

        let error = select_models(all, &["qwen99".into()]).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("qwen99"), "{text}");
        assert!(text.contains("qwen9, oss120"), "{text}");
    }
}
