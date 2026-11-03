//! Сборка `report.md` из [`Results`].
//!
//! Отчёт читает человек, который решает «какую модель брать» и «стало лучше или
//! хуже после правки харнеса», поэтому порядок разделов такой: сначала таблица
//! модель×задача (где именно ломается), потом сводка (чем платим), потом
//! аномалии (чему в цифрах верить нельзя).
//!
//! Функция чистая: `Results` на входе, строка на выходе. Никакого чтения диска
//! — старый `results.json` пересобирается в новый отчёт без перепрогона.

use std::collections::BTreeSet;

use crate::results::{Results, RunRecord, median};

/// Собрать текст `report.md`.
pub fn render_report(results: &Results) -> String {
    let mut out = String::new();

    out.push_str(&format!("# mark2 bench — {}\n\n", results.run_id));
    out.push_str(&format!(
        "- прогон: {} → {}\n",
        results.started_at, results.finished_at
    ));
    out.push_str(&format!("- mark2: `{}`\n", results.config.mark2));
    out.push_str(&format!("- задачи: `{}`\n", results.config.tasks_dir));
    out.push_str(&format!("- модели: `{}`\n", results.config.models_file));
    out.push_str(&format!("- повторов: {}\n", repeats_summary(results)));
    out.push('\n');

    out.push_str(&render_matrix(results));
    out.push('\n');
    out.push_str(&render_summary(results));
    out.push('\n');
    out.push_str(&render_anomalies(results));

    out
}

/// Строка про повторы: откуда взялось число.
fn repeats_summary(results: &Results) -> String {
    match results.config.repeats_override {
        Some(n) => format!("{n} (--repeats)"),
        None => {
            let mut counts: BTreeSet<usize> =
                results.config.tasks.iter().map(|t| t.repeats).collect();
            if counts.len() == 1 {
                let single = counts.pop_first().unwrap_or_default();
                format!("{single} (из task.toml)")
            } else {
                let list: Vec<String> = counts.iter().map(usize::to_string).collect();
                format!("{} (из task.toml)", list.join("/"))
            }
        }
    }
}

/// Таблица модель×задача: пройдено повторов и медиана времени.
fn render_matrix(results: &Results) -> String {
    let mut out = String::from("## Модель × задача\n\n");
    out.push_str("В ячейке: пройдено/прогонов, медиана времени.\n\n");

    let header: Vec<String> = results
        .config
        .tasks
        .iter()
        .map(|task| format!("{} {}", task.id, task.slug))
        .collect();
    out.push_str(&format!("| модель | {} |\n", header.join(" | ")));
    out.push_str(&format!(
        "|---|{}\n",
        "---|".repeat(results.config.tasks.len())
    ));

    for model in &results.config.models {
        let mut cells = Vec::new();
        for task in &results.config.tasks {
            let runs: Vec<&RunRecord> = results
                .runs
                .iter()
                .filter(|run| run.model == model.alias && run.task == task.id)
                .collect();
            cells.push(matrix_cell(&runs));
        }
        out.push_str(&format!("| {} | {} |\n", model.alias, cells.join(" | ")));
    }

    out
}

/// Содержимое одной ячейки таблицы.
fn matrix_cell(runs: &[&RunRecord]) -> String {
    if runs.is_empty() {
        return "—".to_string();
    }
    let passed = runs.iter().filter(|run| run.pass).count();
    let times: Vec<f64> = runs.iter().map(|run| run.wall_time_seconds).collect();
    let time = median(&times).unwrap_or_default();
    format!("{passed}/{} · {time:.1}s", runs.len())
}

/// Сводка по моделям: цена и качество в одной таблице.
fn render_summary(results: &Results) -> String {
    let mut out = String::from("## Сводка\n\n");
    out.push_str(
        "| модель | pass rate | медиана времени | медиана токенов in/out | tool error rate | \
         таймауты | компакции |\n|---|---|---|---|---|---|---|\n",
    );

    for model in &results.config.models {
        let runs: Vec<&RunRecord> = results
            .runs
            .iter()
            .filter(|run| run.model == model.alias)
            .collect();
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            model.alias,
            pass_rate(&runs),
            median_time(&runs),
            median_tokens(&runs),
            tool_error_rate(&runs),
            runs.iter().filter(|run| run.timed_out).count(),
            compactions(&runs),
        ));
    }

    let all: Vec<&RunRecord> = results.runs.iter().collect();
    out.push_str(&format!("\nВсего: {}.\n", pass_rate(&all)));

    out
}

fn pass_rate(runs: &[&RunRecord]) -> String {
    if runs.is_empty() {
        return "—".to_string();
    }
    let passed = runs.iter().filter(|run| run.pass).count();
    format!("{passed}/{} ({})", runs.len(), percent(passed, runs.len()))
}

fn median_time(runs: &[&RunRecord]) -> String {
    let times: Vec<f64> = runs.iter().map(|run| run.wall_time_seconds).collect();
    median(&times).map_or_else(|| "—".to_string(), |value| format!("{value:.1}s"))
}

fn median_tokens(runs: &[&RunRecord]) -> String {
    let input: Vec<f64> = runs
        .iter()
        .filter_map(|run| run.tokens.map(|t| t.input as f64))
        .collect();
    let output: Vec<f64> = runs
        .iter()
        .filter_map(|run| run.tokens.map(|t| t.output as f64))
        .collect();
    match (median(&input), median(&output)) {
        (Some(input), Some(output)) => format!("{} / {}", input.round(), output.round()),
        _ => "—".to_string(),
    }
}

fn tool_error_rate(runs: &[&RunRecord]) -> String {
    let uses: usize = runs
        .iter()
        .filter_map(|run| run.session.as_ref())
        .map(|session| session.tool_uses)
        .sum();
    let errors: usize = runs
        .iter()
        .filter_map(|run| run.session.as_ref())
        .map(|session| session.tool_errors)
        .sum();
    if uses == 0 {
        return "—".to_string();
    }
    format!("{} ({errors}/{uses})", percent(errors, uses))
}

/// Сколько раз за все прогоны модели сработала компакция. Прогон без сессии
/// даёт ноль: колонка печатается всегда, даже если везде нули, — формат
/// таблицы не должен зависеть от данных.
fn compactions(runs: &[&RunRecord]) -> usize {
    runs.iter()
        .filter_map(|run| run.session.as_ref())
        .map(|session| session.compactions)
        .sum()
}

fn percent(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "—".to_string();
    }
    let value = (part as f64 / whole as f64) * 100.0;
    format!("{}%", value.round())
}

/// Аномалии: всё, из-за чего цифрам выше нельзя верить как есть.
fn render_anomalies(results: &Results) -> String {
    let mut out = String::from("## Аномалии\n\n");
    let mut lines = Vec::new();

    for run in &results.runs {
        let who = format!("[{}] {} повтор {}", run.model, run.task, run.repeat);
        if let Some(error) = &run.error {
            lines.push(format!("- {who}: ошибка ранера: {error}"));
            continue;
        }
        if run.timed_out {
            lines.push(format!(
                "- {who}: таймаут mark2 ({:.0}s)",
                run.timeout_seconds
            ));
        } else if run.exit_code != Some(0) {
            lines.push(format!(
                "- {who}: mark2 вышел с кодом {}",
                run.exit_code
                    .map_or_else(|| "—".to_string(), |code| code.to_string())
            ));
        }
        if run.empty_answer() {
            lines.push(format!("- {who}: пустой ответ в stdout"));
        }
        if run.verify.as_ref().is_some_and(|verify| verify.timed_out) {
            lines.push(format!(
                "- {who}: verify.sh не уложился в {}s",
                results.config.verify_timeout_seconds
            ));
        }
        if run.session.is_none() {
            lines.push(format!("- {who}: сессия не записана"));
        }
    }

    if lines.is_empty() {
        out.push_str("Нет.\n");
    } else {
        out.push_str(&lines.join("\n"));
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{SessionMetrics, Tokens};
    use crate::models::ModelSpec;
    use crate::results::{RunConfig, SCHEMA_VERSION, TaskRecord, VerifyOutcome};
    use crate::task::TaskPolicy;
    use mark2_core::event::StopReason;

    fn model(alias: &str, id: &str, multiplier: f64) -> ModelSpec {
        ModelSpec {
            alias: alias.into(),
            model: id.into(),
            base_url: "http://localhost:11434/v1".into(),
            timeout_multiplier: multiplier,
            api_key_env: None,
        }
    }

    fn task(id: &str, slug: &str) -> TaskRecord {
        TaskRecord {
            id: id.into(),
            slug: slug.into(),
            dir: format!("bench/tasks/{id}-{slug}"),
            timeout_seconds: 120,
            repeats: 2,
            tags: vec![],
            policy: TaskPolicy::Yolo,
            context_limit: None,
            prompt: "промпт".into(),
        }
    }

    /// Прогон, в сессии которого компакция сработала `count` раз.
    fn with_compactions(mut run: RunRecord, count: usize) -> RunRecord {
        if let Some(session) = run.session.as_mut() {
            session.compactions = count;
        }
        run
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        model: &str,
        task: &str,
        repeat: usize,
        pass: bool,
        wall: f64,
        tokens: Option<(u64, u64)>,
        session: Option<(usize, usize, usize)>,
        timed_out: bool,
    ) -> RunRecord {
        RunRecord {
            model: model.into(),
            model_id: format!("{model}-id"),
            task: task.into(),
            repeat,
            started_at: "2026-08-24T12:00:00Z".into(),
            pass,
            exit_code: if timed_out { None } else { Some(0) },
            timed_out,
            wall_time_seconds: wall,
            timeout_seconds: 120.0,
            tokens: tokens.map(|(input, output)| Tokens {
                input,
                output,
                cached: None,
            }),
            session: session.map(|(assistant, uses, errors)| SessionMetrics {
                assistant_messages: assistant,
                tool_uses: uses,
                tool_errors: errors,
                compactions: 0,
                stop_reason: Some(StopReason::Completed),
                skipped_lines: 0,
            }),
            verify: Some(VerifyOutcome {
                exit_code: Some(i32::from(!pass)),
                timed_out: false,
                stdout: String::new(),
                stderr: String::new(),
            }),
            answer_chars: 12,
            stdout_path: Some(format!("logs/{model}/{task}/{repeat}.stdout.txt")),
            stderr_path: Some(format!("logs/{model}/{task}/{repeat}.stderr.txt")),
            session_path: Some(format!("sessions/{model}/{task}/{repeat}.jsonl")),
            error: None,
        }
    }

    fn fixture() -> Results {
        let mut timed_out = run("qwen9", "T02", 2, false, 120.0, None, None, true);
        timed_out.answer_chars = 0;
        timed_out.session_path = None;

        let mut empty = run(
            "oss120",
            "T01",
            2,
            true,
            3.0,
            Some((900, 40)),
            Some((1, 1, 0)),
            false,
        );
        empty.answer_chars = 0;

        Results {
            schema_version: SCHEMA_VERSION,
            label: "baseline".into(),
            run_id: "20260824-120000-baseline".into(),
            started_at: "2026-08-24T12:00:00Z".into(),
            finished_at: "2026-08-24T12:31:00Z".into(),
            config: RunConfig {
                bench_version: "0.1.0".into(),
                mark2: "/repo/target/release/mark2".into(),
                tasks_dir: "bench/tasks".into(),
                models_file: "bench/models.toml".into(),
                repeats_override: None,
                verify_timeout_seconds: 30,
                warmup_prompt: "Reply with exactly: ok".into(),
                project_config: "[agent]\ntemperature = 0.0\n".into(),
                models: vec![
                    model("qwen9", "qwen3.5:9b", 1.0),
                    model("oss120", "gpt-oss:120b-cloud", 2.5),
                ],
                tasks: vec![task("T01", "create-file"), task("T02", "read-answer")],
            },
            runs: vec![
                with_compactions(
                    run(
                        "qwen9",
                        "T01",
                        1,
                        true,
                        10.0,
                        Some((1200, 340)),
                        Some((2, 3, 1)),
                        false,
                    ),
                    2,
                ),
                run(
                    "qwen9",
                    "T01",
                    2,
                    true,
                    14.0,
                    Some((1300, 300)),
                    Some((2, 3, 0)),
                    false,
                ),
                with_compactions(
                    run(
                        "qwen9",
                        "T02",
                        1,
                        false,
                        30.0,
                        Some((800, 20)),
                        Some((1, 2, 2)),
                        false,
                    ),
                    1,
                ),
                timed_out,
                run(
                    "oss120",
                    "T01",
                    1,
                    true,
                    4.0,
                    Some((1000, 50)),
                    Some((1, 1, 0)),
                    false,
                ),
                empty,
                run(
                    "oss120",
                    "T02",
                    1,
                    true,
                    5.0,
                    Some((900, 60)),
                    Some((1, 1, 0)),
                    false,
                ),
                run(
                    "oss120",
                    "T02",
                    2,
                    true,
                    7.0,
                    Some((950, 55)),
                    Some((1, 1, 0)),
                    false,
                ),
            ],
        }
    }

    /// Снапшот: отчёт читает человек, его формат меняется осознанно.
    /// Обновить эталон: `UPDATE_SNAPSHOTS=1 cargo test -p mark2-bench`.
    #[test]
    fn report_snapshot() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/report.md");
        let actual = render_report(&fixture());
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, &actual).unwrap();
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("нет снапшота {}: {error}", path.display()));
        assert_eq!(
            actual, expected,
            "отчёт разошёлся с эталоном; если так и задумано — UPDATE_SNAPSHOTS=1 cargo test"
        );
    }

    #[test]
    fn cells_and_rates_handle_missing_data() {
        assert_eq!(matrix_cell(&[]), "—");
        assert_eq!(pass_rate(&[]), "—");
        assert_eq!(median_time(&[]), "—");
        assert_eq!(median_tokens(&[]), "—");
        assert_eq!(tool_error_rate(&[]), "—");
        assert_eq!(compactions(&[]), 0);
        assert_eq!(percent(0, 0), "—");
        assert_eq!(percent(1, 3), "33%");
    }

    /// Строка сводки модели (не путать со строкой таблицы модель×задача).
    fn summary_row(report: &str, alias: &str) -> String {
        let summary = report.split("## Сводка").nth(1).unwrap();
        summary
            .lines()
            .find(|line| line.starts_with(&format!("| {alias} |")))
            .unwrap_or_else(|| panic!("нет строки {alias}:\n{summary}"))
            .to_string()
    }

    /// Компакции суммируются по всем прогонам модели; прогон без сессии
    /// (таймаут) просто ничего не добавляет.
    #[test]
    fn summary_sums_compactions_per_model() {
        let report = render_report(&fixture());
        assert!(
            report.contains("| таймауты | компакции |\n|---|---|---|---|---|---|---|\n"),
            "{report}"
        );
        // qwen9: 2 + 1 + 0 + (таймаут без сессии) = 3; таймаутов — 1.
        assert!(
            summary_row(&report, "qwen9").ends_with("| 1 | 3 |"),
            "{report}"
        );
        assert!(
            summary_row(&report, "oss120").ends_with("| 0 | 0 |"),
            "{report}"
        );
    }

    /// Нули — тоже данные: колонка не пропадает, если компакций не было вовсе.
    #[test]
    fn compactions_column_stays_when_everything_is_zero() {
        let mut results = fixture();
        for run in &mut results.runs {
            if let Some(session) = run.session.as_mut() {
                session.compactions = 0;
            }
        }
        let report = render_report(&results);
        assert!(report.contains("| компакции |"), "{report}");
        for alias in ["qwen9", "oss120"] {
            let row = summary_row(&report, alias);
            assert!(row.ends_with(" | 0 |"), "{row}");
            assert_eq!(row.matches('|').count(), 8, "семь колонок: {row}");
        }
    }

    #[test]
    fn no_anomalies_says_so_explicitly() {
        let mut results = fixture();
        results
            .runs
            .retain(|run| run.model == "qwen9" && run.task == "T01");
        let report = render_report(&results);
        assert!(report.contains("## Аномалии\n\nНет.\n"), "{report}");
    }

    #[test]
    fn repeats_line_reports_the_override() {
        let mut results = fixture();
        results.config.repeats_override = Some(1);
        assert!(render_report(&results).contains("- повторов: 1 (--repeats)"));

        results.config.repeats_override = None;
        results.config.tasks[1].repeats = 5;
        assert!(render_report(&results).contains("- повторов: 2/5 (из task.toml)"));
    }
}
