//! Shared helpers of the golden tests: strict comparison, the root placeholder, settings reduction, table dumps.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_service::Error;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const PLACE: &str = "<ROOT>";
pub const LONG: usize = 400;

pub fn case(name: &str) -> Value {
    grainy_testkit::Case::open("service", name).json().clone()
}

pub fn defaults() -> Map<String, Value> {
    case("pure")["defaults"].as_object().unwrap().clone()
}

/// `{**defaults(), **overrides}`.
pub fn full(defaults: &Map<String, Value>, overrides: &Value) -> Value {
    let mut map = defaults.clone();
    if let Some(o) = overrides.as_object() {
        for (k, v) in o {
            map.insert(k.clone(), v.clone());
        }
    }
    Value::Object(map)
}

/// A fresh folder below target/service-tests, resolved as Python's `Path.resolve()` writes it.
pub fn fresh(name: &str) -> PathBuf {
    let base = grainy_testkit::root().join("target").join("service-tests");
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(base).unwrap();
    let dir = base.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    PathBuf::from(text_path(&std::fs::canonicalize(dir).unwrap()))
}

pub fn text_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
}

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn dumps(value: &Value) -> String {
    grainy_catalog::json::dumps(value, true, false, false)
}

fn is_settings(value: &Map<String, Value>) -> bool {
    value.len() >= 90 && value.contains_key("exposure") && value.contains_key("grain_size")
}

/// The root placeholder, and settings dicts reduced to their key-order hash and non-default values.
pub struct Scrub {
    forms: Vec<String>,
    defaults: Map<String, Value>,
}
impl Scrub {
    pub fn new(root: Option<&str>, defaults: &Map<String, Value>) -> Self {
        let mut forms: Vec<String> = Vec::new();
        if let Some(r) = root {
            let escaped = |s: &str| {
                let d = dumps(&Value::String(s.into()));
                d[1..d.len() - 1].to_owned()
            };
            for form in [escaped(r), r.to_owned(), escaped(&r.to_lowercase()), r.to_lowercase()] {
                if !forms.contains(&form) {
                    forms.push(form);
                }
            }
        }
        Self { forms, defaults: defaults.clone() }
    }
    pub fn text(&self, s: &str) -> String {
        let mut s = s.to_owned();
        for form in &self.forms {
            s = s.replace(form.as_str(), PLACE);
        }
        s
    }
    fn reduce(&self, settings: &Map<String, Value>) -> Value {
        let keys: Vec<Value> = settings.keys().map(|k| Value::String(k.clone())).collect();
        let hash = sha(&dumps(&Value::Array(keys)))[..16].to_owned();
        let mut diff = Map::new();
        for (k, v) in settings {
            if self.defaults.get(k).is_none_or(|d| dumps(d) != dumps(v)) {
                diff.insert(k.clone(), v.clone());
            }
        }
        let missing: Vec<&String> = self.defaults.keys().filter(|k| !settings.contains_key(*k)).collect();
        json!({"$settings": hash, "diff": diff, "missing": missing})
    }
    pub fn value(&self, value: &Value) -> Value {
        match value {
            Value::Object(m) => {
                if is_settings(m) {
                    let inner: Map<String, Value> = m.iter().map(|(k, v)| (k.clone(), self.value(v))).collect();
                    return self.reduce(&inner);
                }
                Value::Object(m.iter().map(|(k, v)| (self.text(k), self.value(v))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(|v| self.value(v)).collect()),
            Value::String(s) => Value::String(self.text(s)),
            other => other.clone(),
        }
    }
}

/// Golden values with the paths below the placeholder as the oracle writes them on this system
/// ([`grainy_testkit::native_paths`]).
pub fn native_paths(value: &Value) -> Value {
    grainy_testkit::native_paths(value, &[PLACE])
}

/// Arguments with the placeholder replaced by this test's root (paths as this system writes them).
pub fn unplace(value: &Value, root: &str) -> Value {
    if !cfg!(windows) {
        return unplace_text(&native_paths(value), root);
    }
    unplace_text(value, root)
}

fn unplace_text(value: &Value, root: &str) -> Value {
    match value {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), unplace_text(v, root))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|v| unplace_text(v, root)).collect()),
        Value::String(s) => Value::String(s.replace(PLACE, root)),
        other => other.clone(),
    }
}

/// The oracle's `run`: `{'result': value}` or `{'error': {'type', 'message'[, 'kind']}}`.
pub fn outcome(result: Result<Value, Error>) -> Value {
    match result {
        Ok(v) => json!({"result": v}),
        Err(error) => {
            // Operating-system texts are localized and machine-specific: only the class is compared.
            let os = matches!(&error, Error::Failed(f) if f.is_os_error());
            let message = if os { "<os message>".to_owned() } else { error.to_string() };
            let mut e = json!({"type": error.python_name(), "message": message});
            if let Some(kind) = error.conflict() {
                e["kind"] = kind.name().into();
            }
            json!({"error": e})
        }
    }
}

/// Strict equality: key order, integer/float kind and every value.
pub fn same(actual: &Value, expected: &Value) -> bool {
    serde_json::to_string(actual).unwrap() == serde_json::to_string(expected).unwrap()
}

pub fn check(actual: &Value, expected: &Value, what: &str) {
    let expected = &native_paths(expected);
    if !same(actual, expected) {
        let a = serde_json::to_string(actual).unwrap();
        let e = serde_json::to_string(expected).unwrap();
        let at = a.bytes().zip(e.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(e.len()));
        let from = at.saturating_sub(200);
        let cut = |s: &str| s.get(from..(at + 200).min(s.len())).unwrap_or("").to_owned();
        panic!("{what}: differs at byte {at}\n actual:   …{}\n expected: …{}", cut(&a), cut(&e));
    }
}

/// The generator's `dump`: every table but preview_state, rows sorted by all columns, long text by SHA-256.
pub fn dump(db: &Connection, scrub: &Scrub) -> Map<String, Value> {
    let mut result = Map::new();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in tables {
        if table == "preview_state" {
            continue;
        }
        let columns: Vec<String> = db
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let order = columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
        let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY {order}")).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |r| {
                let mut row = Map::new();
                for (i, c) in columns.iter().enumerate() {
                    let v = match r.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(v) => json!(v),
                        ValueRef::Real(v) => json!(v),
                        ValueRef::Text(t) => {
                            let s = scrub.text(std::str::from_utf8(t).unwrap());
                            // A long text is compared by its hash: the hash of the text as the goldens' Windows
                            // oracle wrote its paths (on Windows the text itself).
                            let windows = grainy_testkit::windows_paths(&s, &[PLACE]);
                            let n = windows.chars().count();
                            if n > LONG {
                                json!({"sha": sha(&windows), "len": n})
                            } else {
                                Value::String(s)
                            }
                        }
                        ValueRef::Blob(b) => json!({"blob": format!("{:x}", Sha256::digest(b)), "len": b.len()}),
                    };
                    row.insert(c.clone(), v);
                }
                Ok(Value::Object(row))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        result.insert(table, Value::Array(rows));
    }
    result
}
