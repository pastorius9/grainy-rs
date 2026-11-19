#![allow(dead_code)]
//! Helpers shared by the test files: temporary folders, trees of small fake files, zips and listings.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use grainy_platform::updater::Layout;
use grainy_platform::{Environment, System};
use serde_json::Value as Json;

pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new() -> Temp {
        let path = std::env::temp_dir().join(format!("grainy-platform-test-{}", unique()));
        std::fs::create_dir_all(&path).unwrap();
        Temp(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{:x}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )
}

pub fn golden(name: &str) -> Json {
    grainy_testkit::Case::open("platform", name).json().clone()
}

pub fn notes_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("root")
}

pub fn windows_layout(root: &Path) -> Layout {
    Layout {
        system: System::Windows,
        machine: "AMD64".into(),
        env: Environment {
            system: System::Windows,
            local_app_data: Some(root.join("local").display().to_string()),
            home: root.join("home"),
        },
    }
}

pub fn mac_layout(root: &Path, machine: &str) -> Layout {
    Layout {
        system: System::MacOs,
        machine: machine.into(),
        env: Environment { system: System::MacOs, local_app_data: None, home: root.to_path_buf() },
    }
}

pub fn put(root: &Path, files: &[(String, String)]) {
    for (rel, text) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

pub fn put_map(root: &Path, files: &Json) {
    let list: Vec<(String, String)> =
        files.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
    put(root, &list);
}

pub fn zip_file(path: &Path, entries: &[(String, String)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        if name.ends_with('/') {
            writer.add_directory(name.trim_end_matches('/'), options).unwrap();
        } else {
            writer.start_file(name.as_str(), options).unwrap();
            writer.write_all(data.as_bytes()).unwrap();
        }
    }
    writer.finish().unwrap();
}

pub fn entries_of(value: &Json) -> Vec<(String, String)> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e[0].as_str().unwrap().to_string(), e[1].as_str().unwrap().to_string()))
        .collect()
}

/// Stamps and random ids of folder names written as the goldens write them.
pub fn normal(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    'outer: while !rest.is_empty() {
        for (prefix, shape, replacement) in [
            ("previous-", "stamp", "previous-STAMP"),
            ("failed-", "stamp", "failed-STAMP"),
            ("extract-", "hex", "extract-ID"),
        ] {
            if let Some(after) = rest.strip_prefix(prefix) {
                let taken = if shape == "stamp" {
                    let b = after.as_bytes();
                    (b.len() >= 15
                        && b[..8].iter().all(u8::is_ascii_digit)
                        && b[8] == b'-'
                        && b[9..15].iter().all(u8::is_ascii_digit))
                    .then_some(15)
                } else {
                    let b = after.as_bytes();
                    (b.len() >= 32 && b[..32].iter().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
                        .then_some(32)
                };
                if let Some(n) = taken {
                    out.push_str(replacement);
                    rest = &after[n..];
                    continue 'outer;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `{relative path: text or None for a folder}` with stamps normalised; `work` is written as `<work>`.
pub fn listing(root: &Path, work: Option<&Path>) -> BTreeMap<String, Option<String>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            out.push(path.clone());
            if path.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut paths = Vec::new();
    walk(root, &mut paths);
    let mut out = BTreeMap::new();
    for path in paths {
        let mut rel = path
            .strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if let Some(work) = work {
            let prefix = work
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if rel == prefix || rel.starts_with(&format!("{prefix}/")) {
                rel = format!("<work>{}", &rel[prefix.len()..]);
            }
        }
        let text = if path.is_file() {
            Some(String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned())
        } else {
            None
        };
        out.insert(normal(&rel), text);
    }
    out
}

pub fn expected_listing(value: &Json) -> BTreeMap<String, Option<String>> {
    value.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string))).collect()
}

pub fn strings(value: &Json) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}
