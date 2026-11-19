//! An order-keeping JSON value that reads and writes the way Python's `json` module does.
//!
//! The settings file is written by the Python Grainy with `json.dump(..., ensure_ascii=False, indent=2)` and a
//! dict keeps the order its keys were first inserted in. The Rust app must leave the same bytes behind, so objects
//! here are ordered lists, and floats are written with Python's `repr` rules.

use std::fmt::Write as _;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// A JSON value with ordered objects (a Python value read by `json.loads`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null` (Python `None`).
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An integer (Python `int`).
    Int(i128),
    /// A number written with a fraction or exponent (Python `float`).
    Float(f64),
    /// A string.
    Str(String),
    /// A list.
    List(Vec<Value>),
    /// An object whose keys keep their first-insertion order (Python `dict`).
    Object(Object),
}

/// An ordered string-keyed map: a Python `dict`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Object(Vec<(String, Value)>);

impl Object {
    /// An empty object.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value of `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Sets `key`: an existing key keeps its place (like `dict.update`), a new one goes last.
    pub fn insert(&mut self, key: &str, value: Value) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Removes `key`.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let at = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(at).1)
    }

    /// Number of keys.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no keys.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The pairs in order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.0.iter().map(|(k, v)| (k, v))
    }
}

impl FromIterator<(String, Value)> for Object {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        let mut object = Object::new();
        for (key, value) in iter {
            object.insert(&key, value);
        }
        object
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Value::Str(text.to_string())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Value::Str(text)
    }
}

impl From<bool> for Value {
    fn from(flag: bool) -> Self {
        Value::Bool(flag)
    }
}

impl From<i64> for Value {
    fn from(number: i64) -> Self {
        Value::Int(i128::from(number))
    }
}

impl From<f64> for Value {
    fn from(number: f64) -> Self {
        Value::Float(number)
    }
}

impl Value {
    /// The string, when this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The list, when this is one.
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    /// The object, when this is one.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(object) => Some(object),
            _ => None,
        }
    }

    /// Python truthiness (`bool(value)`).
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::Int(number) => *number != 0,
            Value::Float(number) => *number != 0.0,
            Value::Str(text) => !text.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Object(object) => !object.is_empty(),
        }
    }

    /// Python `str(value)`.
    pub fn py_str(&self) -> String {
        match self {
            Value::Str(text) => text.clone(),
            other => other.py_repr(),
        }
    }

    /// Python `repr(value)`.
    pub fn py_repr(&self) -> String {
        match self {
            Value::Null => "None".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Int(number) => number.to_string(),
            Value::Float(number) => float_repr(*number),
            Value::Str(text) => str_repr(text),
            Value::List(items) => format!("[{}]", items.iter().map(Value::py_repr).collect::<Vec<_>>().join(", ")),
            Value::Object(object) => format!(
                "{{{}}}",
                object.iter().map(|(k, v)| format!("{}: {}", str_repr(k), v.py_repr())).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// Reads JSON text (Python `json.loads`; `NaN` and `Infinity`, which Python accepts, are errors here).
    pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Writes JSON text like Python `json.dumps(value, ensure_ascii=..., indent=...)`: with `indent` None the
    /// default separators `", "` and `": "`.
    pub fn dump(&self, indent: Option<usize>, ensure_ascii: bool) -> String {
        let mut out = String::new();
        write_value(&mut out, self, indent, ensure_ascii, 0);
        out
    }
}

impl From<&serde_json::Value> for Value {
    /// Converts a `serde_json` value (its objects are sorted, so their order is the key order).
    fn from(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(flag) => Value::Bool(*flag),
            serde_json::Value::Number(number) => match (number.as_i64(), number.as_u64()) {
                (Some(i), _) => Value::Int(i128::from(i)),
                (None, Some(u)) => Value::Int(i128::from(u)),
                _ => Value::Float(number.as_f64().unwrap_or(f64::NAN)),
            },
            serde_json::Value::String(text) => Value::Str(text.clone()),
            serde_json::Value::Array(items) => Value::List(items.iter().map(Value::from).collect()),
            serde_json::Value::Object(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect())
            }
        }
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Int(i128::from(v)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Int(i128::from(v)))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Float(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::Str(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::Str(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::List(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Object::new();
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            object.insert(&key, value); // a repeated key keeps its first place and the last value, as in Python
        }
        Ok(Value::Object(object))
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// Python `repr(float)`: the shortest text that reads back the same, `1e-05` and `1e+16` for small and large.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = if value == 0.0 { "0".to_string() } else { digits };
    let point = exponent + 1; // decimal point position relative to the first digit
    if value != 0.0 && !(-4..16).contains(&exponent) {
        let (head, tail) = digits.split_at(1);
        let fraction = if tail.is_empty() { String::new() } else { format!(".{tail}") };
        let sign_e = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{head}{fraction}e{sign_e}{:02}", exponent.abs());
    }
    let len = digits.len() as i32;
    let body = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point >= len {
        format!("{}{}.0", digits, "0".repeat((point - len) as usize))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    };
    format!("{sign}{body}")
}

/// Python `repr(str)` (quotes chosen as Python does; non-ASCII printable characters stay).
pub fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn write_string(out: &mut String, text: &str, ensure_ascii: bool) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && !c.is_ascii() => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(out: &mut String, value: &Value, indent: Option<usize>, ensure_ascii: bool, depth: usize) {
    let newline = |out: &mut String, depth: usize| {
        if let Some(width) = indent {
            out.push('\n');
            out.push_str(&" ".repeat(width * depth));
        }
    };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::Float(number) => {
            // Python writes these three as bare words.
            out.push_str(&if number.is_nan() {
                "NaN".to_string()
            } else if number.is_infinite() {
                if *number < 0.0 {
                    "-Infinity".into()
                } else {
                    "Infinity".into()
                }
            } else {
                float_repr(*number)
            });
        }
        Value::Str(text) => write_string(out, text, ensure_ascii),
        Value::List(items) if items.is_empty() => out.push_str("[]"),
        Value::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                newline(out, depth + 1);
                write_value(out, item, indent, ensure_ascii, depth + 1);
            }
            newline(out, depth);
            out.push(']');
        }
        Value::Object(object) if object.is_empty() => out.push_str("{}"),
        Value::Object(object) => {
            out.push('{');
            for (index, (key, item)) in object.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                newline(out, depth + 1);
                write_string(out, key, ensure_ascii);
                out.push_str(": ");
                write_value(out, item, indent, ensure_ascii, depth + 1);
            }
            newline(out, depth);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_read_like_python_repr() {
        for (value, text) in [
            (0.1, "0.1"),
            (1.0, "1.0"),
            (100.0, "100.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (123456789.125, "123456789.125"),
            (-2.5e-7, "-2.5e-07"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(float_repr(value), text);
        }
    }

    #[test]
    fn round_trip_keeps_order_and_duplicates_like_python() {
        let value = Value::parse(r#"{"b": 1, "a": {"z": 1, "y": [1, 2.5, "x"]}, "b": 3}"#).unwrap();
        assert_eq!(value.dump(None, true), r#"{"b": 3, "a": {"z": 1, "y": [1, 2.5, "x"]}}"#);
    }

    #[test]
    fn indented_dump_matches_python() {
        let value = Value::parse(r#"{"a": [], "b": {}, "c": [1, {"d": "한"}], "e": null}"#).unwrap();
        let expected = "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": \"한\"\n    }\n  ],\n  \"e\": null\n}";
        assert_eq!(value.dump(Some(2), false), expected);
        assert!(value.dump(None, true).contains("\\ud55c"));
    }
}
