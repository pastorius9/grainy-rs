//! The argument checker: a validator for the JSON-Schema subset the tools use, with the oracle's exact texts.

use std::cmp::Ordering;

use serde_json::{Number, Value};

use crate::pyjson::{format_g, py_repr, type_name};

/// Port of `photo_service.ServiceError`: a request the caller can correct; the text is English for the AI client.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ServiceError(pub String);

fn fail<T>(text: String) -> Result<T, ServiceError> {
    Err(ServiceError(text))
}

fn is_integer(value: &Value) -> bool {
    matches!(value, Value::Number(n) if n.is_i64() || n.is_u64())
}

/// Port of `mcp_tools._is`: whether `value` is of JSON-Schema type `kind` (a name or a list of names).
pub fn is_kind(kind: &Value, value: &Value) -> bool {
    match kind {
        Value::Array(kinds) => kinds.iter().any(|k| is_kind(k, value)),
        Value::String(name) => match name.as_str() {
            "null" => value.is_null(),
            "integer" => is_integer(value),
            "number" => matches!(value, Value::Number(n) if n.as_f64().is_some_and(f64::is_finite)),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            _ => false,
        },
        _ => false,
    }
}

/// Port of `mcp_tools._show`: a schema bound as text (`{value:g}` for floats, `str` for integers).
pub fn show(value: &Value) -> String {
    match value {
        Value::Number(n) if !(n.is_i64() || n.is_u64()) => format_g(n.as_f64().unwrap_or(0.0)),
        other => crate::pyjson::py_str(other),
    }
}

fn compare(a: &Number, b: &Number) -> Option<Ordering> {
    let integer = |n: &Number| n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from));
    match (integer(a), integer(b)) {
        (Some(x), Some(y)) => Some(x.cmp(&y)),
        _ => a.as_f64()?.partial_cmp(&b.as_f64()?),
    }
}

/// `re.search(pattern, value)` for the subset the tool schemas use: `^`, `$`, literal characters, `[...]` classes
/// with ranges, and the quantifiers `{n}`, `+`, `*`, `?`. As in Python, `$` also matches before a final newline.
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    #[derive(Clone)]
    enum Atom {
        Start,
        End,
        Set(Vec<(char, char)>),
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut atoms: Vec<(Atom, usize, usize)> = Vec::new(); // atom, min, max repeat
    let mut i = 0;
    while i < chars.len() {
        let atom = match chars[i] {
            '^' => Atom::Start,
            '$' => Atom::End,
            '[' => {
                let mut ranges = Vec::new();
                i += 1;
                while i < chars.len() && chars[i] != ']' {
                    if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
                        ranges.push((chars[i], chars[i + 2]));
                        i += 3;
                    } else {
                        ranges.push((chars[i], chars[i]));
                        i += 1;
                    }
                }
                Atom::Set(ranges)
            }
            '\\' if i + 1 < chars.len() => {
                i += 1;
                Atom::Set(vec![(chars[i], chars[i])])
            }
            c => Atom::Set(vec![(c, c)]),
        };
        i += 1;
        let (mut min, mut max) = (1, 1);
        match chars.get(i) {
            Some('+') => (min, max, i) = (1, usize::MAX, i + 1),
            Some('*') => (min, max, i) = (0, usize::MAX, i + 1),
            Some('?') => (min, max, i) = (0, 1, i + 1),
            Some('{') => {
                let close = chars[i..].iter().position(|&c| c == '}').map_or(chars.len(), |p| i + p);
                let count: String = chars[i + 1..close.min(chars.len())].iter().collect();
                if let Ok(n) = count.parse::<usize>() {
                    (min, max) = (n, n);
                }
                i = close + 1;
            }
            _ => {}
        }
        atoms.push((atom, min, max));
    }
    let text: Vec<char> = value.chars().collect();

    // Backtracking matcher from position `at` over atoms[index..].
    fn run(atoms: &[(Atom, usize, usize)], index: usize, text: &[char], at: usize) -> bool {
        let Some((atom, min, max)) = atoms.get(index) else { return true };
        match atom {
            Atom::Start => at == 0 && run(atoms, index + 1, text, at),
            Atom::End => {
                (at == text.len() || (at + 1 == text.len() && text[at] == '\n')) && run(atoms, index + 1, text, at)
            }
            Atom::Set(ranges) => {
                let hit = |c: char| ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi);
                let mut count = 0;
                while count < *max && at + count < text.len() && hit(text[at + count]) {
                    count += 1;
                }
                loop {
                    if count >= *min && run(atoms, index + 1, text, at + count) {
                        return true;
                    }
                    if count == 0 || count <= *min {
                        return false;
                    }
                    count -= 1;
                }
            }
        }
    }
    (0..=text.len()).any(|start| run(&atoms, 0, &text, start))
}

/// Port of `mcp_tools.check`: validates `value` against the schema subset these tools use; the error names the
/// place (`path`, `arguments` at the top).
pub fn check(schema: &Value, value: &Value, path: &str) -> Result<(), ServiceError> {
    let Some(schema) = schema.as_object() else { return Ok(()) };
    if let Some(kind) = schema.get("type") {
        if !is_kind(kind, value) {
            let name = match kind {
                Value::Array(kinds) => kinds.iter().map(crate::pyjson::py_str).collect::<Vec<_>>().join(" or "),
                other => crate::pyjson::py_str(other),
            };
            let article = if name.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
            return fail(format!("{path} must be {article} {name}"));
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum") {
        if !options.iter().any(|option| python_equal(option, value)) {
            let names: Vec<String> = options.iter().map(crate::pyjson::py_str).collect();
            return fail(format!("{path} must be one of {}", names.join(", ")));
        }
    }
    if let (Value::Number(n), Some(Value::Number(low))) = (value, schema.get("minimum")) {
        if compare(n, low) == Some(Ordering::Less) {
            return fail(format!("{path} must be at least {}", show(&schema["minimum"])));
        }
    }
    if let (Value::Number(n), Some(Value::Number(high))) = (value, schema.get("maximum")) {
        if compare(n, high) == Some(Ordering::Greater) {
            return fail(format!("{path} must be at most {}", show(&schema["maximum"])));
        }
    }
    if let Value::String(text) = value {
        let length = text.chars().count() as u64;
        let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0);
        if length < min {
            return fail(format!("{path} must have at least {min} characters"));
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
            if length > max {
                return fail(format!("{path} must have at most {max} characters"));
            }
        }
        if let Some(Value::String(pattern)) = schema.get("pattern") {
            if !pattern_matches(pattern, text) {
                return fail(format!("{path} does not have the expected form"));
            }
        }
    }
    if let Value::Array(items) = value {
        let count = items.len() as u64;
        let min = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
        if count < min {
            return fail(format!("{path} needs at least {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
            if count > max {
                return fail(format!("{path} takes at most {max} items"));
            }
        }
        if schema.get("uniqueItems").is_some_and(truthy) {
            let mut seen: Vec<(&'static str, String)> = Vec::new();
            for item in items {
                let key = (type_name(item), py_repr(item));
                if seen.contains(&key) {
                    return fail(format!("{path} must not repeat items"));
                }
                seen.push(key);
            }
        }
        if let Some(item_schema @ Value::Object(_)) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                check(item_schema, item, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Value::Object(map) = value {
        let empty = serde_json::Map::new();
        let properties = schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        if let Some(Value::Array(required)) = schema.get("required") {
            for key in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(key) {
                    return fail(format!("{path}.{key} is required"));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            for key in map.keys() {
                if !properties.contains_key(key) {
                    return fail(format!("{path}.{key} is not a known argument"));
                }
            }
        }
        let min = schema.get("minProperties").and_then(Value::as_u64).unwrap_or(0);
        if (map.len() as u64) < min {
            return fail(format!("{path} needs at least {min} entries"));
        }
        for (key, item) in map {
            if let Some(property) = properties.get(key) {
                check(property, item, &format!("{path}.{key}"))?;
            }
        }
    }
    Ok(())
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `==` between an enum entry and a checked value (`1 == True` and `1 == 1.0` hold in Python).
fn python_equal(a: &Value, b: &Value) -> bool {
    let number = |v: &Value| match v {
        Value::Bool(x) => Some(f64::from(u8::from(*x))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}
