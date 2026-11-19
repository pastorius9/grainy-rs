//! Small Python semantics the oracle relies on: truthiness, `==`, `round`, `repr` of strings, `str.strip`,
//! `json.dumps` and the `pathlib` name parts of the native flavour.

use grainy_catalog::json;
use serde_json::Value;

pub(crate) use grainy_core::settings::truthy;

/// Python `a == b` on JSON values (0 == 0.0 == False).
pub(crate) fn eq(a: &Value, b: &Value) -> bool {
    json::equivalent(a, b)
}

/// `dict.get(key)` with a missing key read as `None`.
pub(crate) fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

/// Python `float(x or 0)` for a JSON value.
pub(crate) fn float_or_zero(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Python `int(x or 0)` for a JSON value (truncation toward zero).
pub(crate) fn int_or_zero(value: &Value) -> i64 {
    match value {
        Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0).trunc() as i64),
        Value::Bool(b) => i64::from(*b),
        _ => 0,
    }
}

/// `photo_service._number`: an int or float (not a bool) that is finite.
pub(crate) fn number(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}

/// Python `round(x, digits)` for floats: the exact binary value rounded half to even in decimal.
pub(crate) fn round(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// Python `round(x)` for floats: half to even, as an integer.
pub(crate) fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Python `json.dumps(value, ensure_ascii=ascii)` with the default separators.
pub(crate) fn dumps(value: &Value, ascii: bool) -> String {
    json::dumps(value, ascii, false, false)
}

/// Python `str(value)` for scalars.
pub(crate) fn text(value: &Value) -> String {
    json::text(value)
}

fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()`.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// Python `repr(str)`.
pub(crate) fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// The drive/root anchor and the parts of a path as `pathlib` of this system splits it.
fn split(path: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        let s = path.replace('/', "\\");
        let mut rest = s.as_str();
        let mut anchor = String::new();
        if let Some(unc) = rest.strip_prefix("\\\\") {
            let bits = unc.splitn(3, '\\').collect::<Vec<_>>();
            if bits.len() >= 2 && !bits[0].is_empty() && !bits[1].is_empty() {
                anchor = format!("\\\\{}\\{}\\", bits[0], bits[1]);
                rest = bits.get(2).copied().unwrap_or("");
            }
        } else if rest.as_bytes().get(1) == Some(&b':') && rest.is_char_boundary(2) {
            anchor = rest[..2].into();
            rest = &rest[2..];
        }
        if (anchor.len() == 2 || anchor.is_empty()) && rest.starts_with('\\') {
            anchor.push('\\');
        }
        let parts = rest.split('\\').filter(|p| !p.is_empty() && *p != ".").map(str::to_owned).collect();
        (anchor, parts)
    } else {
        let anchor = if path.starts_with("//") && !path.starts_with("///") {
            "//"
        } else if path.starts_with('/') {
            "/"
        } else {
            ""
        };
        (anchor.into(), path.split('/').filter(|p| !p.is_empty() && *p != ".").map(str::to_owned).collect())
    }
}

fn join(anchor: &str, parts: &[String]) -> String {
    let sep = if cfg!(windows) { "\\" } else { "/" };
    let s = format!("{anchor}{}", parts.join(sep));
    if s.is_empty() {
        ".".into()
    } else {
        s
    }
}

/// `str(Path(path))`.
pub(crate) fn path_str(path: &str) -> String {
    let (anchor, parts) = split(path);
    join(&anchor, &parts)
}

/// `str(Path(path).parent)`.
pub(crate) fn parent(path: &str) -> String {
    let (anchor, mut parts) = split(path);
    parts.pop();
    join(&anchor, &parts)
}

/// `Path(path).name`.
pub(crate) fn name(path: &str) -> String {
    split(path).1.last().cloned().unwrap_or_default()
}

/// `Path(path).suffix` of a file name.
pub(crate) fn suffix(path: &str) -> String {
    let name = name(path);
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i..].into(),
        _ => String::new(),
    }
}

/// `Path(path).stem`.
pub(crate) fn stem(path: &str) -> String {
    let name = name(path);
    let suffix = suffix(&name);
    name[..name.len() - suffix.len()].into()
}

/// `str(path)` of a resolved path without the `\\?\` prefix Windows APIs add.
pub(crate) fn path_text(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    }
}

/// `Path.resolve(strict=False)`: the existing ancestor resolved, the missing tail appended.
pub(crate) fn resolved(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut tail = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(found) => {
                let mut result = std::path::PathBuf::from(path_text(&found));
                for part in tail.into_iter().rev() {
                    result.push(part);
                }
                return Ok(normal(&result));
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) else {
                    return Ok(normal(&absolute));
                };
                tail.push(name.to_owned());
                ancestor = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Lexical `..` removal for the missing tail of a resolved path (`os.path.realpath` does it too).
fn normal(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
