//! Reading layer, component and operation objects as the oracle's `dict.get` does.

use grainy_core::settings::truthy;
use serde_json::{Map, Value};

use crate::{Error, Result};

pub(crate) type Obj = Map<String, Value>;

pub(crate) fn bad(key: &str, value: &Value) -> Error {
    Error::BadValue { key: key.to_owned(), found: value.to_string() }
}

/// A JSON number as Python reads it (`True`/`False` are 1/0).
pub(crate) fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// `obj.get(key, default)` as a number.
pub(crate) fn num(obj: &Obj, key: &str, default: f64) -> Result<f64> {
    match obj.get(key) {
        None => Ok(default),
        Some(v) => number(v).ok_or_else(|| bad(key, v)),
    }
}

/// `obj.get(key)` truthiness (missing is false).
pub(crate) fn flag(obj: &Obj, key: &str) -> bool {
    obj.get(key).is_some_and(truthy)
}

/// `obj.get('enabled', True)` truthiness.
pub(crate) fn enabled(obj: &Obj) -> bool {
    obj.get("enabled").is_none_or(truthy)
}

/// `obj.get(key)` as a string (`None` when missing or not a string).
pub(crate) fn text<'a>(obj: &'a Obj, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

/// The object of a JSON value.
pub(crate) fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Obj> {
    value.as_object().ok_or_else(|| bad(what, value))
}

/// A list of numbers of the given length.
pub(crate) fn numbers(value: &Value, key: &str, len: usize) -> Result<Vec<f64>> {
    let list = value.as_array().ok_or_else(|| bad(key, value))?;
    if list.len() != len {
        return Err(bad(key, value));
    }
    list.iter().map(|v| number(v).ok_or_else(|| bad(key, value))).collect()
}

/// A list of `[x, y]` points (`obj.get(key, default)`).
pub(crate) fn points(obj: &Obj, key: &str, default: &[[f64; 2]]) -> Result<Vec<[f64; 2]>> {
    let Some(value) = obj.get(key) else { return Ok(default.to_vec()) };
    let list = value.as_array().ok_or_else(|| bad(key, value))?;
    list.iter()
        .map(|p| {
            let v = numbers(p, key, 2)?;
            Ok([v[0], v[1]])
        })
        .collect()
}

/// A list of objects (`obj.get(key, [])`).
pub(crate) fn objects<'a>(obj: &'a Obj, key: &str) -> Result<Vec<&'a Obj>> {
    match obj.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(list)) => list.iter().map(|v| object(v, key)).collect(),
        Some(v) => Err(bad(key, v)),
    }
}
