use serde_json::Value;

pub use grainy_core::settings::equivalent;

/// Python `json.dumps(value, ensure_ascii=ascii, separators=(",", ":") if compact, sort_keys=sort)`, byte for
/// byte (float formatting included): compact batch history hashes the stored text.
pub fn dumps(value: &Value, ascii: bool, compact: bool, sort: bool) -> String {
    fn quoted(s: &str, ascii: bool) -> String {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 32 || (ascii && !c.is_ascii()) => {
                    let mut buf = [0u16; 2];
                    for part in c.encode_utf16(&mut buf) {
                        out.push_str(&format!("\\u{part:04x}"));
                    }
                }
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let comma = if compact { "," } else { ", " };
    let colon = if compact { ":" } else { ": " };
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::String(v) => quoted(v, ascii),
        Value::Number(n) => {
            if n.is_f64() {
                let f = n.as_f64().unwrap_or(0.0);
                if f != 0.0 && (f.abs() < 1e-4 || f.abs() >= 1e16) {
                    let s = format!("{f:e}");
                    if let Some((m, e)) = s.split_once('e') {
                        let exp = e.parse::<i32>().unwrap_or(0);
                        return format!("{m}e{exp:+03}");
                    }
                }
                let s = f.to_string();
                if !s.contains('.') {
                    format!("{s}.0")
                } else {
                    s
                }
            } else {
                n.to_string()
            }
        }
        Value::Array(v) => {
            format!("[{}]", v.iter().map(|v| dumps(v, ascii, compact, sort)).collect::<Vec<_>>().join(comma))
        }
        Value::Object(v) => {
            let mut keys = v.keys().collect::<Vec<_>>();
            if sort {
                keys.sort();
            }
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}{colon}{}", quoted(k, ascii), dumps(&v[*k], ascii, compact, sort)))
                    .collect::<Vec<_>>()
                    .join(comma)
            )
        }
    }
}

/// Python `str(value)` for the scalar values the oracle formats into text.
pub fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(v) => if *v { "True" } else { "False" }.into(),
        _ => dumps(value, false, false, false),
    }
}
