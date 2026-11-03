//! Приведение аргументов тула к типам его схемы — шаг перед валидацией.
//!
//! Локальные модели (flint:40b и qwen3.6 в прогонах бенча 2026-11-03/04)
//! стабильно шлют числа строками — `"timeout_seconds": "60.0"`, — и каждый
//! такой вызов отбивается валидацией, съедая ход. Смысл аргумента при этом
//! однозначен, поэтому реестр до валидации приводит его к типу схемы, но только
//! когда это без потерь:
//!
//! * схема ждёт `integer`: строка с JSON-литералом целого (`"60"`, `"-3"`), в
//!   том числе с нулевой дробью (`"60.0"`), и число с нулевой дробью (`60.0`:
//!   валидация его пропускает как целое, а десериализация в `u64` — нет);
//! * схема ждёт `number`: строка с JSON-литералом числа (`"1.5"`, `"1e3"`);
//! * схема ждёт `boolean`: строки `"true"` и `"false"`.
//!
//! Всё остальное остаётся как пришло и отбивается валидацией с прежним текстом
//! ошибки: строки с мусором (`"60s"`, `" 60"`, `""`), дробь для `integer`
//! (`"60.5"`, `60.5`), любые значения там, где схема принимает строку (числа в
//! строки тоже не превращаются), `null`, вложенные объекты и массивы.
//! Приводятся только свойства верхнего уровня, у которых схема прямо объявляет
//! `type` — строкой или массивом, как его пишет schemars (`["integer", "null"]`
//! у `Option<u64>`); `anyOf`, `$ref` и прочие композиции не разбираются.
//!
//! Это не `json_repair` провайдера: тот спасает синтаксически битый JSON, здесь
//! JSON уже целый — не совпали только типы.

use serde_json::{Number, Value};

/// 2⁶⁴: первое неотрицательное `f64`, которое уже не помещается в `u64`.
const U64_END: f64 = 18_446_744_073_709_551_616.0;
/// −2⁶³ = `i64::MIN`, представимо в `f64` точно.
const I64_MIN: f64 = -9_223_372_036_854_775_808.0;

/// Парсер строкового литерала под тип схемы.
type Literal = fn(&str) -> Option<Value>;

/// Порядок важен: у схемы `["integer", "number"]` целое выигрывает.
const STRING_LITERALS: [(&str, Literal); 3] = [
    ("integer", integer_literal),
    ("number", number_literal),
    ("boolean", boolean_literal),
];

/// Привести свойства верхнего уровня `input` к типам из `schema`.
///
/// Возвращает имена приведённых свойств в порядке аргументов (пусто — ничего
/// не поменялось). Не объект на входе или схема без `properties` — ничего не
/// делается.
pub(crate) fn coerce_arguments(schema: &Value, input: &mut Value) -> Vec<String> {
    let (Some(properties), Some(arguments)) = (
        schema.get("properties").and_then(Value::as_object),
        input.as_object_mut(),
    ) else {
        return Vec::new();
    };

    let mut changed = Vec::new();
    for (key, value) in arguments.iter_mut() {
        if let Some(property) = properties.get(key)
            && let Some(coerced) = coerce_value(property, value)
        {
            *value = coerced;
            changed.push(key.clone());
        }
    }
    changed
}

/// Значение, приведённое к типу `property`, или `None` — оставить как есть.
fn coerce_value(property: &Value, value: &Value) -> Option<Value> {
    let types = declared_types(property)?;
    let accepts = |name: &str| types.contains(&name);
    match value {
        // Строка, которую схема и так принимает, — уже валидна.
        Value::String(_) if accepts("string") => None,
        Value::String(text) => STRING_LITERALS
            .iter()
            .filter(|(name, _)| accepts(name))
            .find_map(|(_, parse)| parse(text)),
        // Дробное число там, где годится и дробное, — уже валидно.
        Value::Number(number) if accepts("integer") && !accepts("number") => whole_float(number),
        _ => None,
    }
}

/// Имена типов из `type` свойства: строка или массив строк.
fn declared_types(property: &Value) -> Option<Vec<&str>> {
    match property.get("type")? {
        Value::String(name) => Some(vec![name.as_str()]),
        Value::Array(names) => Some(names.iter().filter_map(Value::as_str).collect()),
        _ => None,
    }
}

/// Целое из строки без потерь: JSON-литерал целого, допускается нулевая дробь
/// (`"60"`, `"-3"`, `"60.0"`, `"60.00"`).
///
/// Не принимается всё, где «целое» пришлось бы угадывать или округлять:
/// ненулевая дробь, экспонента (`"6e1"`), `+`, ведущие нули (`"060"`), пустая
/// дробь (`"60."`), пробелы, выход за `i64`/`u64`.
fn integer_literal(text: &str) -> Option<Value> {
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    if let Some(fraction) = fraction
        && (fraction.is_empty() || fraction.bytes().any(|byte| byte != b'0'))
    {
        return None;
    }

    let negative = whole.starts_with('-');
    let digits = whole.strip_prefix('-').unwrap_or(whole);
    let plain = !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !plain {
        return None;
    }

    if negative {
        whole.parse::<i64>().ok().map(Value::from)
    } else {
        whole.parse::<u64>().ok().map(Value::from)
    }
}

/// Число из строки: ровно JSON-литерал числа (`"1.5"`, `"-2"`, `"1e3"`).
///
/// `NaN`, `Infinity`, `+1`, `0x10`, `.5`, пробелы вокруг и числа вне `f64` —
/// не числа.
fn number_literal(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    // JSON-число начинается с минуса или цифры и кончается цифрой; заодно это
    // отсекает пробелы, которые `serde_json` вокруг значения бы стерпел.
    let starts = matches!(bytes.first(), Some(b'-' | b'0'..=b'9'));
    let ends = matches!(bytes.last(), Some(b'0'..=b'9'));
    if !(starts && ends) {
        return None;
    }
    serde_json::from_str::<Number>(text).ok().map(Value::Number)
}

/// Булево из строки: только JSON-литералы `"true"` и `"false"`.
fn boolean_literal(text: &str) -> Option<Value> {
    match text {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => None,
    }
}

/// Целое из дробного числа с нулевой дробью (`60.0` → `60`). Уже целые числа,
/// ненулевая дробь и значения вне `i64`/`u64` — `None`.
fn whole_float(number: &Number) -> Option<Value> {
    if number.is_u64() || number.is_i64() {
        return None;
    }
    let float = number.as_f64()?;
    if float.fract() != 0.0 {
        return None;
    }
    // Значение целое и в диапазоне — каст точен.
    if (0.0..U64_END).contains(&float) {
        Some(Value::from(float as u64))
    } else if (I64_MIN..0.0).contains(&float) {
        Some(Value::from(float as i64))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Схема на все случаи — в том виде, в каком её пишет schemars для
    /// `Option<_>`-полей, плюс поля с одиночным `type`.
    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "count": {"type": ["integer", "null"], "format": "uint64", "minimum": 0},
                "delta": {"type": "integer", "format": "int64"},
                "ratio": {"type": ["number", "null"], "format": "double"},
                "flag": {"type": ["boolean", "null"]},
                "label": {"type": "string"},
                "either": {"type": ["string", "integer"]},
                "sizes": {"type": ["array", "null"], "items": {"type": "integer"}},
                "nested": {
                    "type": "object",
                    "properties": {"count": {"type": "integer"}}
                },
                "untyped": {"description": "без type"},
                "composed": {"anyOf": [{"type": "integer"}, {"type": "null"}]}
            }
        })
    }

    /// Значение свойства `key` после приведения и признак «поменялось».
    fn coerce(key: &str, value: Value) -> (Value, bool) {
        let mut input = json!({ key: value });
        let changed = coerce_arguments(&schema(), &mut input);
        assert!(changed.is_empty() || changed == [key], "{changed:?}");
        (input[key].clone(), !changed.is_empty())
    }

    fn coerced(key: &str, value: Value, expected: Value) {
        let (got, changed) = coerce(key, value.clone());
        assert_eq!(got, expected, "{key}: {value}");
        assert!(changed, "{key}: {value} должно было привестись");
    }

    fn untouched(key: &str, value: Value) {
        let (got, changed) = coerce(key, value.clone());
        assert_eq!(got, value, "{key}: {value} не должно было меняться");
        assert!(!changed, "{key}: {value}");
    }

    #[test]
    fn integer_strings_become_integers() {
        coerced("count", json!("60"), json!(60));
        coerced("count", json!("60.0"), json!(60));
        coerced("count", json!("60.00"), json!(60));
        coerced("count", json!("0"), json!(0));
        coerced("delta", json!("-3"), json!(-3));
        coerced("delta", json!("-3.0"), json!(-3));
        coerced("count", json!("18446744073709551615"), json!(u64::MAX));
        coerced("delta", json!("-9223372036854775808"), json!(i64::MIN));
        // Знак — забота валидации (`minimum: 0`), а не приведения.
        coerced("count", json!("-5"), json!(-5));

        // Тип результата — именно целое: `u64` его десериализует.
        let (value, _) = coerce("count", json!("60.0"));
        assert!(value.is_u64(), "{value}");
        assert_eq!(serde_json::from_value::<u64>(value).unwrap(), 60);
    }

    #[test]
    fn whole_floats_become_integers() {
        coerced("count", json!(60.0), json!(60));
        coerced("delta", json!(-3.0), json!(-3));
        coerced("count", json!(-0.0), json!(0));
        let (value, _) = coerce("count", json!(60.0));
        assert!(value.is_u64(), "{value}");
    }

    #[test]
    fn number_strings_become_numbers() {
        coerced("ratio", json!("1.5"), json!(1.5));
        coerced("ratio", json!("-0.25"), json!(-0.25));
        coerced("ratio", json!("1e3"), json!(1000.0));
        // Целый литерал для `number` остаётся целым числом.
        coerced("ratio", json!("60"), json!(60));
    }

    #[test]
    fn boolean_strings_become_booleans() {
        coerced("flag", json!("true"), json!(true));
        coerced("flag", json!("false"), json!(false));
        for raw in ["True", "FALSE", "1", "yes", ""] {
            untouched("flag", json!(raw));
        }
    }

    #[test]
    fn garbage_strings_stay_strings() {
        for raw in [
            "60s", "abc", "", " 60", "60 ", "+60", "060", "60.", ".5", "6e1", "0x10", "1_000",
            "60.5", "60.0.0", "--1", "-", "NaN", "Infinity", "null",
        ] {
            untouched("count", json!(raw));
        }
        // Вне диапазона целых — не угадываем.
        untouched("count", json!("18446744073709551616"));
        untouched("delta", json!("-9223372036854775809"));

        for raw in [
            "1.5x", " 1.5", "1.5 ", "+1.5", "NaN", "Infinity", "1e400", ".5", "",
        ] {
            untouched("ratio", json!(raw));
        }
    }

    #[test]
    fn fractional_numbers_are_not_rounded_into_integers() {
        untouched("count", json!(60.5));
        untouched("delta", json!(-0.1));
        // Целое за пределами `i64`/`u64` — тоже нет.
        untouched("count", json!(1e20));
        untouched("delta", json!(-1e19));
        // Для `number` дробное число и так валидно.
        untouched("ratio", json!(60.0));
    }

    #[test]
    fn string_schemas_and_already_valid_values_are_left_alone() {
        // Строка для строкового поля — даже если похожа на число.
        untouched("label", json!("60"));
        untouched("either", json!("60"));
        // Число для строкового поля в строку не превращается.
        untouched("label", json!(60));
        untouched("label", json!(true));
        // Уже правильные типы и `null`.
        untouched("count", json!(60));
        untouched("count", Value::Null);
        untouched("ratio", json!(1.5));
        untouched("flag", json!(true));
        untouched("either", json!(60));
    }

    #[test]
    fn only_top_level_typed_properties_are_coerced() {
        untouched("sizes", json!(["1", "2"]));
        untouched("sizes", json!("3"));
        untouched("nested", json!({"count": "60"}));
        untouched("untyped", json!("60"));
        untouched("composed", json!("60"));

        // Свойства, которых нет в схеме, — тоже как пришли.
        let mut input = json!({"stray": "60", "count": "7"});
        assert_eq!(coerce_arguments(&schema(), &mut input), ["count"]);
        assert_eq!(input, json!({"stray": "60", "count": 7}));
    }

    #[test]
    fn non_object_input_or_schema_without_properties_is_a_no_op() {
        let mut input = json!("60");
        assert!(coerce_arguments(&schema(), &mut input).is_empty());
        assert_eq!(input, json!("60"));

        let mut input = json!({"count": "60"});
        assert!(coerce_arguments(&json!({"type": "object"}), &mut input).is_empty());
        assert_eq!(input, json!({"count": "60"}));
    }

    /// Порядок ключей сохраняется: аргументы уходят в историю и сессию, а
    /// сериализация обязана оставаться байт-стабильной.
    #[test]
    fn key_order_is_preserved() {
        let mut input = json!({"label": "x", "count": "1", "flag": "true", "ratio": "0.5"});
        assert_eq!(
            coerce_arguments(&schema(), &mut input),
            ["count", "flag", "ratio"]
        );
        assert_eq!(
            serde_json::to_string(&input).unwrap(),
            r#"{"label":"x","count":1,"flag":true,"ratio":0.5}"#
        );
    }
}
