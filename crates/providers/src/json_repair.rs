//! Tolerant-парс JSON для tool-аргументов слабых моделей.
//!
//! Это третий слой надёжности тул-коллов (после нативного шаблона модели и
//! валидации схемой): строгий `serde_json` уже провалился, но выбрасывать
//! ответ рано. Локалки регулярно отдают почти-JSON:
//!
//! * поток оборвался по `max_tokens` — хвост обрезан прямо посреди строки;
//! * `{'path': 'a.txt'}` — одинарные кавычки вместо двойных;
//! * `{"a": 1,}` / `[1, 2, ]` — висячая запятая;
//! * ```` ```json … ``` ```` — обёртка в markdown-фенс;
//! * `Вот аргументы: {…}` — преамбула перед объектом;
//! * сырой перевод строки внутри строкового литерала.
//!
//! Алгоритм: нормализовать (снять фенс/преамбулу, привести кавычки, экранировать
//! управляющие символы), закрыть незакрытые скобки и строки — и попробовать
//! разобрать. Если не вышло, откатиться к предыдущей структурной границе
//! (запятая, `{`, `[`) и повторить: лучше отдать объект без последнего
//! недописанного поля, чем ничего.

use serde_json::Value;

/// Восстановление не удалось: даже после починки JSON не разбирается.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("не удалось разобрать JSON даже с восстановлением: {0}")]
pub struct RepairError(String);

impl RepairError {
    /// Исходный текст, который не удалось разобрать.
    pub fn input(&self) -> &str {
        &self.0
    }
}

/// Сколько структурных границ откатываем, прежде чем сдаться.
const MAX_CUT_ATTEMPTS: usize = 64;

/// Строгий парс, при провале — восстановление.
///
/// Возвращает `Ok` c разобранным значением (возможно, потерявшим оборванный
/// хвост) либо [`RepairError`], если спасать нечего.
pub fn parse_tolerant(input: &str) -> Result<Value, RepairError> {
    if let Ok(value) = serde_json::from_str::<Value>(input) {
        return Ok(value);
    }
    let candidate = extract_candidate(input);
    for end in cut_points(candidate) {
        let repaired = normalize_and_close(&candidate[..end]);
        if let Ok(value) = serde_json::from_str::<Value>(&repaired) {
            return Ok(value);
        }
    }
    Err(RepairError(input.to_string()))
}

/// Починенный текст, который заведомо разбирается, — для логов и тестов.
///
/// `None`, если восстановить не удалось.
pub fn repair(input: &str) -> Option<String> {
    let candidate = extract_candidate(input);
    cut_points(candidate).into_iter().find_map(|end| {
        let repaired = normalize_and_close(&candidate[..end]);
        serde_json::from_str::<Value>(&repaired)
            .ok()
            .map(|_| repaired)
    })
}

/// Снять markdown-фенс и преамбулу вида «Вот аргументы: {…}».
fn extract_candidate(input: &str) -> &str {
    let mut text = input.trim();

    if let Some(rest) = text.strip_prefix("```") {
        // ```json\n… или ```\n…
        let rest = match rest.find('\n') {
            Some(nl) => &rest[nl + 1..],
            None => rest,
        };
        text = rest.trim_end().trim_end_matches("```").trim();
    }

    match text.find(['{', '[']) {
        Some(0) | None => text,
        Some(start) => &text[start..],
    }
}

/// Байтовые границы, на которых имеет смысл обрезать вход: сначала весь текст,
/// затем — с конца — позиции структурных токенов вне строк.
fn cut_points(text: &str) -> Vec<usize> {
    let mut points = vec![text.len()];
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut structural = Vec::new();

    for (idx, ch) in text.char_indices() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                // Обрезаем ПЕРЕД запятой — недописанный элемент отбрасывается.
                ',' => structural.push(idx),
                // Обрезаем ПОСЛЕ скобки — остаётся пустой объект/массив.
                '{' | '[' => structural.push(idx + ch.len_utf8()),
                _ => {}
            },
        }
    }

    structural.reverse();
    points.extend(structural.into_iter().take(MAX_CUT_ATTEMPTS));
    points
}

/// Нормализовать текст и дописать недостающие закрывающие токены.
fn normalize_and_close(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut stack: Vec<char> = Vec::new();
    let mut in_string: Option<char> = None;
    let mut escaped = false;

    for ch in text.chars() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                    if quote == '\'' && ch == '\'' {
                        // \' валиден в одинарных кавычках, но не в JSON.
                        out.push('\'');
                    } else {
                        out.push('\\');
                        out.push(ch);
                    }
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    in_string = None;
                    out.push('"');
                } else if ch == '"' {
                    // Сырая кавычка внутри строки в одинарных кавычках.
                    out.push_str("\\\"");
                } else if ch.is_control() {
                    push_escaped_control(&mut out, ch);
                } else {
                    out.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => {
                    in_string = Some(ch);
                    out.push('"');
                }
                '{' => {
                    stack.push('}');
                    out.push('{');
                }
                '[' => {
                    stack.push(']');
                    out.push('[');
                }
                '}' | ']' => {
                    trim_trailing_comma(&mut out);
                    if stack.last() == Some(&ch) {
                        stack.pop();
                        out.push(ch);
                    }
                    // Лишнюю закрывающую скобку просто игнорируем.
                }
                _ => out.push(ch),
            },
        }
    }

    // Незавершённый escape в конце — байт-мусор, выбрасываем (он не попал в out).
    if in_string.is_some() {
        out.push('"');
    }

    trim_trailing_comma(&mut out);
    if out.trim_end().ends_with(':') {
        // Ключ есть, значение не успело прийти — сохраняем ключ с null.
        out.push_str("null");
    }
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    out
}

fn push_escaped_control(out: &mut String, ch: char) {
    match ch {
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        other => out.push_str(&format!("\\u{:04x}", other as u32)),
    }
}

fn trim_trailing_comma(out: &mut String) {
    let trimmed = out.trim_end();
    if trimmed.ends_with(',') {
        out.truncate(trimmed.len() - 1);
    } else {
        out.truncate(trimmed.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_json_passes_through_untouched() {
        let value = parse_tolerant(r#"{"path":"a.txt","limit":10}"#).unwrap();
        assert_eq!(value, json!({"path": "a.txt", "limit": 10}));
    }

    #[test]
    fn truncated_string_value_is_closed() {
        // Поток оборвался по max_tokens посреди строкового литерала.
        let value = parse_tolerant(r#"{"command":"cargo test --workspace"#).unwrap();
        assert_eq!(value, json!({"command": "cargo test --workspace"}));
    }

    #[test]
    fn truncated_nested_object_is_closed() {
        let value = parse_tolerant(r#"{"a":{"b":[1,2,{"c":"x"#).unwrap();
        assert_eq!(value, json!({"a": {"b": [1, 2, {"c": "x"}]}}));
    }

    #[test]
    fn truncated_after_key_keeps_key_as_null() {
        let value = parse_tolerant(r#"{"path":"a.txt","content":"#).unwrap();
        assert_eq!(value, json!({"path": "a.txt", "content": null}));
    }

    #[test]
    fn truncated_literal_drops_incomplete_pair() {
        // `tru` не спасти — откатываемся к предыдущей запятой.
        let value = parse_tolerant(r#"{"path":"a.txt","recursive":tru"#).unwrap();
        assert_eq!(value, json!({"path": "a.txt"}));
    }

    #[test]
    fn trailing_commas_are_dropped() {
        assert_eq!(
            parse_tolerant(r#"{"a":1,"b":2,}"#).unwrap(),
            json!({"a": 1, "b": 2})
        );
        assert_eq!(parse_tolerant("[1, 2, 3, ]").unwrap(), json!([1, 2, 3]));
        assert_eq!(
            parse_tolerant(r#"{"a":[1,2,],"b":{"c":3,},}"#).unwrap(),
            json!({"a": [1, 2], "b": {"c": 3}})
        );
    }

    #[test]
    fn single_quotes_become_double() {
        assert_eq!(
            parse_tolerant("{'path': 'a.txt', 'limit': 10}").unwrap(),
            json!({"path": "a.txt", "limit": 10})
        );
    }

    #[test]
    fn single_quoted_string_with_inner_double_quote() {
        assert_eq!(
            parse_tolerant(r#"{'command': 'echo "hi"'}"#).unwrap(),
            json!({"command": "echo \"hi\""})
        );
    }

    #[test]
    fn single_quoted_string_with_escaped_quote() {
        assert_eq!(
            parse_tolerant(r#"{'a': 'it\'s'}"#).unwrap(),
            json!({"a": "it's"})
        );
    }

    #[test]
    fn double_quoted_escapes_survive() {
        assert_eq!(
            parse_tolerant(r#"{"a": "line\nnext \"q\" \\ end",}"#).unwrap(),
            json!({"a": "line\nnext \"q\" \\ end"})
        );
    }

    #[test]
    fn raw_newline_inside_string_is_escaped() {
        assert_eq!(
            parse_tolerant("{\"content\": \"first\nsecond\"}").unwrap(),
            json!({"content": "first\nsecond"})
        );
    }

    #[test]
    fn markdown_fence_is_stripped() {
        let input = "```json\n{\"path\": \"a.txt\"}\n```";
        assert_eq!(parse_tolerant(input).unwrap(), json!({"path": "a.txt"}));
    }

    #[test]
    fn preamble_before_object_is_dropped() {
        assert_eq!(
            parse_tolerant("Вот аргументы: {\"path\": \"a.txt\"}").unwrap(),
            json!({"path": "a.txt"})
        );
    }

    #[test]
    fn empty_arguments_object_survives_truncation() {
        assert_eq!(parse_tolerant("{").unwrap(), json!({}));
        assert_eq!(parse_tolerant("[").unwrap(), json!([]));
    }

    #[test]
    fn key_order_is_preserved() {
        let repaired = parse_tolerant(r#"{"zebra":1,"alpha":2,"middle":{"z":1,"a":2},}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&repaired).unwrap(),
            r#"{"zebra":1,"alpha":2,"middle":{"z":1,"a":2}}"#
        );
    }

    #[test]
    fn hopeless_input_reports_error() {
        let err = parse_tolerant("совсем не json").unwrap_err();
        assert_eq!(err.input(), "совсем не json");
        assert!(err.to_string().contains("совсем не json"));
        assert!(parse_tolerant("").is_err());
    }

    #[test]
    fn repair_returns_parseable_text() {
        let repaired = repair(r#"{'a': 1,"#).unwrap();
        assert_eq!(repaired, r#"{"a": 1}"#);
        assert!(serde_json::from_str::<Value>(&repaired).is_ok());
        assert!(repair("не json").is_none());
    }

    #[test]
    fn extra_closing_bracket_is_ignored() {
        assert_eq!(parse_tolerant(r#"{"a": 1}}"#).unwrap(), json!({"a": 1}));
    }
}
