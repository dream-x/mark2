//! Матрица моделей бенча: разбор `bench/models.toml`.
//!
//! Файл описывает, *куда* и *под каким именем* ходить, плюс поправку на
//! скорость железа: `timeout_multiplier` умножает `timeout_seconds` задачи.
//! Так задача остаётся одна на всех, а 9b на localhost и 27b по сети получают
//! честные бюджеты времени. Свойства самой модели, которые mark2 настраивает
//! per-model (`edit_strategy`), тоже живут здесь — ранер передаёт их через
//! `[models."<id>"]` своего конфига в `$HOME` песочницы.
//!
//! Ключи не угадываются: `deny_unknown_fields` превращает опечатку в ошибку с
//! путём файла, а не в молчаливо потерянную настройку.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use mark2_core::tools::EditStrategy;
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
    /// Формат правки файлов mark2 (`search_replace` | `whole_file`), если
    /// модели нужен не дефолтный; разбор — тот же, что у конфига mark2,
    /// неизвестное имя — ошибка загрузки матрицы. Ранер кладёт его в
    /// `[models."<model>"]` своего конфига в `$HOME` песочницы. Без поля — дефолт mark2
    /// (`search_replace`); в старых `results.json` его нет, и в новых оно
    /// пишется только когда задано.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "edit_strategy_name"
    )]
    pub edit_strategy: Option<EditStrategy>,
}

/// `Option<EditStrategy>` как каноническое имя строкой — и в `models.toml`, и
/// в `results.json`.
mod edit_strategy_name {
    use mark2_core::tools::EditStrategy;
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub fn serialize<S: Serializer>(
        value: &Option<EditStrategy>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(strategy) => serializer.serialize_some(strategy.as_str()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<EditStrategy>, D::Error> {
        let Some(raw) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        raw.parse().map(Some).map_err(de::Error::custom)
    }
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
        edit_strategy = "whole_file"
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
        assert_eq!(models[0].edit_strategy, None);
        assert_eq!(models[1].alias, "oss120");
        assert_eq!(models[1].timeout_multiplier, 2.5);
        assert_eq!(models[1].api_key_env.as_deref(), Some("OLLAMA_API_KEY"));
        assert_eq!(models[1].edit_strategy, Some(EditStrategy::WholeFile));
    }

    #[test]
    fn edit_strategy_is_parsed_like_mark2_does() {
        let parse = |value: &str| {
            parse_models(
                &format!(
                    "[[models]]\nalias = \"a\"\nmodel = \"m\"\nbase_url = \"http://x/v1\"\nedit_strategy = \"{value}\"\n"
                ),
                "bench/models.toml",
            )
        };
        assert_eq!(
            parse("search_replace").unwrap()[0].edit_strategy,
            Some(EditStrategy::SearchReplace)
        );
        // Терпимость к написанию — как у конфига mark2.
        assert_eq!(
            parse("Whole-File").unwrap()[0].edit_strategy,
            Some(EditStrategy::WholeFile)
        );

        let text = format!("{:#}", parse("diff").unwrap_err());
        assert!(text.contains("bench/models.toml"), "{text}");
        assert!(text.contains("diff"), "{text}");
        for known in EditStrategy::ALL {
            assert!(text.contains(known.as_str()), "{text}");
        }
    }

    /// `ModelSpec` уезжает в `results.json`: без стратегии поле не пишется
    /// (файл как раньше), со стратегией — каноническим именем; старый файл без
    /// поля читается как «дефолт mark2».
    #[test]
    fn edit_strategy_round_trips_through_results_json() {
        let models = parse_models(SAMPLE, "test.toml").unwrap();
        let plain = serde_json::to_value(&models[0]).unwrap();
        assert!(plain.get("edit_strategy").is_none(), "{plain}");
        let whole = serde_json::to_value(&models[1]).unwrap();
        assert_eq!(whole["edit_strategy"], "whole_file");
        assert_eq!(
            serde_json::from_value::<ModelSpec>(whole).unwrap(),
            models[1]
        );

        let old: ModelSpec = serde_json::from_str(
            r#"{"alias": "qwen9", "model": "qwen3.5:9b", "base_url": "http://x/v1", "timeout_multiplier": 1.0}"#,
        )
        .unwrap();
        assert_eq!(old.edit_strategy, None);
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

    /// Реальный файл репозитория обязан разбираться текущим контрактом —
    /// иначе бенч не запустится.
    ///
    /// Точный состав матрицы намеренно не прибит: она растёт от версии к
    /// версии (v3 — квант-лесенка `qwen36c-*`, дуэль хостов, новые модели), и
    /// тест не должен ломаться от каждой новой строки. Прибиты только якоря —
    /// алиасы, через которые сравниваются прогоны v1/v2/v3: они обязаны
    /// остаться в матрице и значить ту же модель, иначе старые `results.json`
    /// перестают быть сравнимы с новыми.
    #[test]
    fn repository_models_file_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DEFAULT_MODELS_FILE);
        let models = load_models(&path).unwrap();

        let aliases: Vec<&str> = models.iter().map(|m| m.alias.as_str()).collect();
        let unique: BTreeSet<&str> = aliases.iter().copied().collect();
        assert_eq!(
            unique.len(),
            aliases.len(),
            "алиасы повторяются: {aliases:?}"
        );

        for (anchor, model_id) in [("qwen9", "qwen3.5:9b"), ("flint40", "flint:40b")] {
            let spec = models
                .iter()
                .find(|m| m.alias == anchor)
                .unwrap_or_else(|| panic!("нет якоря {anchor} в матрице: {aliases:?}"));
            assert_eq!(spec.model, model_id, "якорь {anchor} сменил модель");
        }

        // flint роняет поля search/replace на длинных сессиях — ему файл
        // целиком (прогоны 2026-11-03/04).
        let flint = models.iter().find(|m| m.alias == "flint40").unwrap();
        assert_eq!(flint.edit_strategy, Some(EditStrategy::WholeFile));
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
