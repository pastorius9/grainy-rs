//! Port of luma/safe_start.py: a marker file while Grainy runs. Found at the next start, it means the run before
//! ended without closing (a crash, or a forced quit).
//!
//! Process liveness uses the `sysinfo` crate (no `unsafe`, no shelling out), on Windows, macOS and Linux alike.

use std::path::Path;

use crate::json::{Object, Value};

/// Name of the marker file in Grainy's settings folder.
pub const NAME: &str = "running.json";

fn read_marker(path: &Path) -> Option<Value> {
    let text = String::from_utf8(std::fs::read(path).ok()?).ok()?;
    Value::parse(&text).ok()
}

/// Port of `_alive`: whether another Grainy that left the marker is still running.
pub fn alive(pid: Option<&Value>) -> bool {
    let Some(Value::Int(pid)) = pid else { return false };
    if *pid <= 0 || *pid == i128::from(std::process::id()) {
        return false;
    }
    match u32::try_from(*pid) {
        Ok(pid) => crate::util::process_running(pid),
        Err(_) => false,
    }
}

/// Port of `begin`: leaves this run's marker in `folder`. Returns the record of the run before when that one
/// ended without closing (`pid`, `version`, `started`), else `None`.
pub fn begin(folder: &Path, version: &str) -> Option<Object> {
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    begin_with(folder, version, std::process::id(), started, &alive)
}

/// [`begin`] with the process number, the start time and the liveness check given (for tests).
pub fn begin_with(
    folder: &Path,
    version: &str,
    pid: u32,
    started: f64,
    is_alive: &dyn Fn(Option<&Value>) -> bool,
) -> Option<Object> {
    let path = folder.join(NAME);
    let mut previous = None;
    if let Some(Value::Object(data)) = read_marker(&path) {
        if !is_alive(data.get("pid")) {
            previous = Some(data);
        }
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        let mut marker = Object::new();
        marker.insert("pid", Value::Int(i128::from(pid)));
        marker.insert("version", Value::from(version));
        marker.insert("started", Value::Float(started));
        std::fs::write(&path, Value::Object(marker).dump(None, true))
    };
    let _ = write();
    previous
}

/// Port of `end`: a normal close removes this run's marker (not one another Grainy left).
pub fn end(folder: &Path) {
    end_as(folder, std::process::id());
}

/// [`end`] for a given process number (for tests).
pub fn end_as(folder: &Path, pid: u32) {
    let path = folder.join(NAME);
    let Some(Value::Object(data)) = read_marker(&path) else { return };
    let same = match data.get("pid") {
        Some(Value::Int(number)) => *number == i128::from(pid),
        Some(Value::Float(number)) => *number == f64::from(pid),
        _ => false,
    };
    if same {
        let _ = std::fs::remove_file(&path);
    }
}
