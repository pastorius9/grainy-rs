//! Small Python semantics the ported code relies on.

use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

/// A lock that survives a panicked holder (the state stays usable, as Python's would).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Python `a == b` on JSON values (0 == 0.0 == False).
pub(crate) fn eq(a: &Value, b: &Value) -> bool {
    grainy_catalog::json::equivalent(a, b)
}

/// `dict.get(key)` with a missing key read as `None`.
pub(crate) fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

/// Python truthiness of a JSON value.
pub(crate) fn truthy(value: &Value) -> bool {
    grainy_core::settings::truthy(value)
}

/// `record['id']`.
pub(crate) fn id_of(record: &Value) -> i64 {
    record.get("id").and_then(Value::as_i64).unwrap_or(0)
}

/// `str(path)` of a native path (without Windows' verbatim prefix).
pub(crate) fn path_text(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        return format!("\\\\{unc}");
    }
    text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
}

/// `Path(path).name`.
pub(crate) fn file_name(path: &str) -> String {
    let cut = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    path[cut..].to_owned()
}

/// `Path(path).suffix.lower()`.
pub(crate) fn suffix_lower(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i..].to_lowercase(),
        _ => String::new(),
    }
}

/// Whether a path is one of the video files an older version imported (`engine.VIDEO_EXTENSIONS`).
pub(crate) fn is_video(path: &str) -> bool {
    grainy_service::VIDEO_EXTENSIONS.contains(&suffix_lower(path).as_str())
}

/// Port of `photo_service.english_error` for this crate's errors: stable English at the protocol boundary.
pub(crate) fn english(error: &grainy_service::Error) -> String {
    use grainy_service::{export::english_error, Error, Failure};
    match error {
        Error::Failed(failure) => english_error(failure),
        Error::Catalog(grainy_catalog::Error::Io(e)) => english_error(&Failure::io(e)),
        other => english_error(&Failure::new(python_class(other), other.to_string())),
    }
}

/// The Python class name `type(error).__name__` would give.
pub(crate) fn python_class(error: &grainy_service::Error) -> String {
    match error {
        grainy_service::Error::Catalog(grainy_catalog::Error::Sql(_)) => "OperationalError".into(),
        other => other.python_name().to_owned(),
    }
}

/// Python `str.casefold()`.
pub(crate) fn casefold(text: &str) -> String {
    use unicode_casefold::UnicodeCaseFold;
    text.chars().case_fold().collect()
}

/// Python `str.strip()` (whitespace as Python's `str.isspace`).
pub(crate) fn strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Python `repr(str)`.
pub(crate) fn repr(text: &str) -> String {
    crate::pyjson::py_repr(&Value::String(text.to_owned()))
}
