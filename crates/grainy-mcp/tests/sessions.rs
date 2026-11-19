//! The scripted tool sessions of `tools/golden/gen_mcp.py` (golden/mcp/sessions/) replayed against the headless
//! host: every reply text, error flag, job view, UI event, written file name and catalogue change equal to the
//! oracle's (JSON compared as text); the pixels handed to the JPEG encoder within the measured limits below; the JPEG
//! replies' sizes and byte lengths reported.
//!
//! One test per binary: the pixel observer is process-wide.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_mcp::headless::{Headless, Mode};
use grainy_mcp::service::{Config, Limits, McpService};
use grainy_mcp::{observe, GrainyTools, Gui, Tools};
use ndarray::Array3;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const PLACE: &str = "<ROOT>";
const LONG: usize = 400;

fn golden_dir(name: &str) -> PathBuf {
    grainy_testkit::root().join("golden").join("mcp").join("sessions").join(name)
}

fn fresh(name: &str) -> PathBuf {
    let base = grainy_testkit::root().join("target").join("mcp-tests");
    std::fs::create_dir_all(&base).unwrap();
    let dir = std::fs::canonicalize(&base).unwrap().join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
    PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(&text))
}

struct Scrub {
    forms: Vec<String>,
}

impl Scrub {
    fn new(root: &str) -> Self {
        let escaped = |s: &str| {
            let d = serde_json::to_string(s).unwrap();
            d[1..d.len() - 1].to_owned()
        };
        let mut forms = Vec::new();
        for form in [escaped(root), root.to_owned(), escaped(&root.to_lowercase()), root.to_lowercase()] {
            if !forms.contains(&form) {
                forms.push(form);
            }
        }
        Self { forms }
    }
    fn text(&self, s: &str) -> String {
        let mut s = s.to_owned();
        for form in &self.forms {
            s = s.replace(form.as_str(), PLACE);
        }
        s
    }
    fn value(&self, value: &Value) -> Value {
        match value {
            Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (self.text(k), self.value(v))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.value(v)).collect()),
            Value::String(s) => Value::String(self.text(s)),
            other => other.clone(),
        }
    }
}

fn unplace(value: &Value, root: &str) -> Value {
    match value {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), unplace(v, root))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|v| unplace(v, root)).collect()),
        Value::String(s) => Value::String(s.replace(PLACE, root)),
        other => other.clone(),
    }
}

fn dump(db: &Connection, scrub: &Scrub) -> Map<String, Value> {
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
                            let n = s.chars().count();
                            if n > LONG {
                                json!({"sha": format!("{:x}", Sha256::digest(s.as_bytes())), "len": n})
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

fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

/// What one replay found besides exact equality.
#[derive(Default)]
struct Measure {
    mismatches: Vec<String>,
    raw_compared: usize,
    raw_equal: usize,
    raw_max: u8,
    sheet_pictures_max: u8,
    sheet_text_max: u8,
    jpeg_bytes: Vec<(usize, usize)>,
    byte_lengths_only: Vec<String>,
    output_sizes: Vec<(String, u64, u64)>,
    decoded_max: u8,
    settings_max: f64,
    explained_rows: usize,
}

fn max_diff(a: &Array3<u8>, b: &Array3<u8>) -> u8 {
    a.iter().zip(b.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

/// `"bytes":<digits>` replaced by `"bytes":#` (JSON inside reply texts is escaped once more).
fn mask_bytes(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("bytes") {
        let (head, tail) = rest.split_at(at + 5);
        out.push_str(head);
        let skip = tail.chars().take_while(|c| matches!(c, '"' | '\\' | ':')).count();
        let (sep, after) = tail.split_at(skip);
        out.push_str(sep);
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            out.push('#');
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// The text rows of a contact sheet: label strips, and the "not ready" line of cells without a picture.
fn text_rows(height: usize, cell: usize, strip: usize, not_ready_rows: &[usize]) -> Vec<bool> {
    let mut text = vec![false; height];
    for (y, t) in text.iter_mut().enumerate() {
        let within = y % (cell + 2);
        *t = within >= cell.saturating_sub(strip) && within < cell;
    }
    for &row in not_ready_rows {
        let middle = row * (cell + 2) + (cell - strip) / 2;
        for t in text.iter_mut().take((middle + 10).min(height)).skip(middle.saturating_sub(10)) {
            *t = true;
        }
    }
    text
}

/// (max difference outside the text rows, max difference inside them).
fn sheet_diff(a: &Array3<u8>, b: &Array3<u8>, rows: &[bool]) -> (u8, u8) {
    let (mut picture, mut text) = (0u8, 0u8);
    for ((y, x, c), v) in a.indexed_iter() {
        let d = v.abs_diff(b[[y, x, c]]);
        if rows[y] {
            text = text.max(d);
        } else {
            picture = picture.max(d);
        }
    }
    (picture, text)
}

/// Largest relative difference of two JSON values with the same shape (numbers compared as floats).
fn float_diff(a: &Value, b: &Value) -> Option<f64> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64()?, y.as_f64()?);
            Some((x - y).abs() / x.abs().max(y.abs()).max(1e-300))
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            x.iter().zip(y).map(|(p, q)| float_diff(p, q)).try_fold(0.0f64, |m, d| Some(m.max(d?)))
        }
        (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
            x.iter().map(|(k, p)| float_diff(p, y.get(k)?)).try_fold(0.0f64, |m, d| Some(m.max(d?)))
        }
        _ => (a == b).then_some(0.0),
    }
}

/// Whether two dump rows differ only in hashed long texts.
fn only_hashes_differ(a: &Value, b: &Value) -> bool {
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return false };
    a.len() == b.len()
        && a.iter().all(|(k, v)| {
            let w = &b[k];
            v == w || (v.get("sha").is_some() && w.get("sha").is_some())
        })
}

fn replay(name: &str) -> Measure {
    let golden_folder = golden_dir(name);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(golden_folder.join("case.json")).unwrap()).unwrap();
    let root = fresh(name);
    let root_text = root.to_string_lossy().into_owned();
    let scrub = Scrub::new(&root_text);
    let files = grainy_testkit::root().join("golden").join("mcp").join("_files");
    for (target, source) in case["layout"].as_object().unwrap() {
        let path = root.join(target);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(files.join(source.as_str().unwrap()), &path).unwrap();
    }
    let stamp = case["stamp"].as_str().unwrap().to_owned();
    let local = case["local"].as_str().unwrap().to_owned();
    let preview_tokens = Arc::new(AtomicUsize::new(0));
    let sources =
        Sources::new(move || stamp.clone(), move || format!("{:032x}", preview_tokens.fetch_add(1, Ordering::SeqCst)));
    let catalog = Catalog::with_sources(root.join("library"), sources).unwrap();
    for photo in case["photos"].as_array().unwrap() {
        let path = root.join(photo[0].as_str().unwrap());
        catalog.add(&path.to_string_lossy(), None, Some(&photo[1]), false).unwrap();
    }
    let counter = Arc::new(AtomicUsize::new(0));
    let config = Config {
        directory: root.join("library"),
        settings_folder: root.join("settings"),
        program_folders: vec![grainy_testkit::root().join("crates").join("grainy-mcp")],
        pictures: root.join("Pictures").join("Grainy"),
    };
    let service = McpService::with_sources(
        config,
        Box::new(move |_| Ok(format!("{:06x}", counter.fetch_add(1, Ordering::SeqCst) + 1))),
        Box::new(move || local.clone()),
    );
    service.set_limits(Limits { wait_seconds: 0.0, retry_seconds: 60.0 });
    let headless = Headless::new(catalog, Mode::Deferred);
    let gui: Arc<dyn Gui> = headless.clone();
    let tools = GrainyTools::new(service.clone(), Some(gui));
    let unbound = GrainyTools::new(service.clone(), None);
    let reader = Connection::open(root.join("library").join("catalog.sqlite")).unwrap();
    let mut measure = Measure::default();
    let mut oracle_state: Map<String, Value> = case["initial"].as_object().unwrap().clone();
    if text(&Value::Object(dump(&reader, &scrub))) != text(&case["initial"]) {
        measure.mismatches.push(format!("{name}: initial catalogue differs"));
    }
    // Rows allowed to differ in hashed settings only, after a step whose settings were checked within tolerance.
    let mut diverged: Vec<(String, String, Value)> = Vec::new();
    let mut reported: Vec<String> = Vec::new();
    observe::record(true);
    for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
        let op = step["op"].as_str().unwrap();
        let tool = step.get("tool").and_then(Value::as_str).unwrap_or("").to_owned();
        let args = unplace(step.get("args").unwrap_or(&Value::Null), &root_text);
        let mut problems: Vec<(String, String, String)> = Vec::new();
        let mut bytes_only: Vec<String> = Vec::new();
        let mut compare =
            |what: &str, actual: String, expected: String, problems: &mut Vec<(String, String, String)>| {
                if actual != expected {
                    if mask_bytes(&actual) == mask_bytes(&expected) {
                        bytes_only.push(format!("{name} step {index} {what}"));
                    } else {
                        problems.push((what.into(), actual, expected));
                    }
                }
            };
        let fail = |problems: &mut Vec<(String, String, String)>, what: &str, actual: String, expected: String| {
            problems.push((what.into(), actual, expected));
        };
        headless.with(|h| h.events.clear());
        observe::take();
        match op {
            "call" => {
                let which = if step.get("unbound").and_then(Value::as_bool) == Some(true) { &unbound } else { &tools };
                let result = which.call(&tool, &args, false, Instant::now() + Duration::from_secs(45)).unwrap();
                let error = result["isError"].as_bool().unwrap();
                let content = result["content"].as_array().unwrap();
                let texts: Vec<Value> = content
                    .iter()
                    .filter(|c| c["type"] == "text")
                    .map(|c| Value::String(scrub.text(c["text"].as_str().unwrap())))
                    .collect();
                if error != step["error"].as_bool().unwrap() {
                    fail(&mut problems, "isError", error.to_string(), step["error"].to_string());
                }
                compare("texts", text(&Value::Array(texts.clone())), text(&step["texts"]), &mut problems);
                let images: Vec<&Value> = content.iter().filter(|c| c["type"] == "image").collect();
                let expected_images = step["images"].as_array().unwrap();
                if images.len() != expected_images.len() {
                    fail(&mut problems, "image count", images.len().to_string(), expected_images.len().to_string());
                }
                for (image, expected) in images.iter().zip(expected_images) {
                    let data = grainy_mcp::b64::decode(image["data"].as_str().unwrap()).unwrap();
                    measure.jpeg_bytes.push((data.len(), expected["bytes"].as_u64().unwrap() as usize));
                    let decoded = grainy_mcp::images::decode(&data).unwrap();
                    let dims = (decoded.dim().1, decoded.dim().0);
                    let wanted = (expected["w"].as_u64().unwrap() as usize, expected["h"].as_u64().unwrap() as usize);
                    if dims != wanted {
                        fail(&mut problems, "image size", format!("{dims:?}"), format!("{wanted:?}"));
                    }
                    if let Some(file) = expected["file"].as_str() {
                        let oracle: Array3<u8> = ndarray_npy::read_npy(golden_folder.join(file)).unwrap();
                        if oracle.dim() == decoded.dim() {
                            measure.decoded_max = measure.decoded_max.max(max_diff(&decoded, &oracle));
                        }
                    }
                }
                let seen = observe::take();
                let raws = step["raw"].as_array().unwrap();
                if seen.len() != raws.len() {
                    fail(&mut problems, "jpeg calls", seen.len().to_string(), raws.len().to_string());
                }
                let reply: Value = step["texts"]
                    .as_array()
                    .and_then(|t| t.last())
                    .and_then(|t| serde_json::from_str(t.as_str()?).ok())
                    .unwrap_or(Value::Null);
                for (k, (pixels, file)) in seen.iter().zip(raws).enumerate() {
                    let oracle: Array3<u8> = ndarray_npy::read_npy(golden_folder.join(file.as_str().unwrap())).unwrap();
                    if oracle.dim() != pixels.dim() {
                        fail(&mut problems, "raw size", format!("{:?}", pixels.dim()), format!("{:?}", oracle.dim()));
                        continue;
                    }
                    measure.raw_compared += 1;
                    let d = max_diff(pixels, &oracle);
                    if d == 0 {
                        measure.raw_equal += 1;
                    }
                    if tool != "contact_sheet" {
                        measure.raw_max = measure.raw_max.max(d);
                        continue;
                    }
                    let page = ids(&reply["ids"]);
                    let not_ready = ids(&reply["not_ready"]);
                    let columns = reply["columns"].as_u64().unwrap_or(1) as usize;
                    let count = reply["variants"].as_array().map_or(page.len(), Vec::len);
                    let (cell, strip) = if k == 0 {
                        (args.get("cell").and_then(Value::as_u64).unwrap_or(160) as usize, 18)
                    } else {
                        let side = columns.max(count.div_ceil(columns));
                        let size = args.get("size").and_then(Value::as_u64).unwrap_or(2400) as usize;
                        let saved = (size - 2 * (side - 1)) / side;
                        let font = 13.max(22.min(saved / 20));
                        (saved, (font + 5) * 3 + 4)
                    };
                    let rows: Vec<usize> = page
                        .iter()
                        .enumerate()
                        .filter(|(_, id)| reply["variants"].is_null() && not_ready.contains(id))
                        .map(|(i, _)| i / columns)
                        .collect();
                    let mask = text_rows(pixels.dim().0, cell, strip, &rows);
                    let (picture, letters) = sheet_diff(pixels, &oracle, &mask);
                    measure.sheet_pictures_max = measure.sheet_pictures_max.max(picture);
                    measure.sheet_text_max = measure.sheet_text_max.max(letters);
                }
                if let Some(settings) = step.get("settings").and_then(Value::as_object) {
                    for (id, expected) in settings {
                        let id: i64 = id.parse().unwrap();
                        let actual = headless.with(|h| h.catalog_ref().photo(id).unwrap().unwrap()["settings"].clone());
                        match float_diff(&actual, expected) {
                            Some(d) if d < 1e-12 => {
                                measure.settings_max = measure.settings_max.max(d);
                                if d > 0.0 {
                                    diverged.push(("photos".into(), "id".into(), json!(id)));
                                    diverged.push(("history".into(), "photo_id".into(), json!(id)));
                                    diverged.push(("snapshots".into(), "photo_id".into(), json!(id)));
                                    if let Some(bid) = reply["batch"].as_str() {
                                        diverged.push((
                                            "preferences".into(),
                                            "key".into(),
                                            json!(format!("undo:mcp:{bid}")),
                                        ));
                                    }
                                }
                            }
                            _ => fail(&mut problems, "settings", text(&actual), text(expected)),
                        }
                    }
                }
            }
            "window" => headless.with(|h| {
                if let Some(set) = args.get("set").and_then(Value::as_object) {
                    for (key, value) in set {
                        let on = value.as_bool().unwrap_or(false);
                        match key.as_str() {
                            "command_running" => h.command_running = on,
                            "maintenance_running" => h.maintenance_running = on,
                            "import_busy" => h.import_busy = on,
                            "import_scans" => h.import_scans = on,
                            "export_running" => h.export_running = on,
                            "library_loading" => h.library_loading = on,
                            "adjusting" => h.adjusting = on,
                            "nested" => h.modal = on,
                            "pending_stroke" => h.pending_stroke = on,
                            "healing_strokes" => h.healing_strokes = on,
                            "crop_mode" => h.crop_mode = on,
                            "sample_mode" => h.sample_mode = on,
                            "loop_level" => h.loop_level = value.as_i64().unwrap() as i32,
                            "selection" => h.select(&ids(value)),
                            "listed" => h.listed = value.as_array().map(|_| ids(value)),
                            "inline" => h.set_mode(if on { Mode::Inline } else { Mode::Deferred }),
                            "closing" => {
                                h.closing = on;
                                service.set_window_alive(!on);
                            }
                            other => panic!("window flag {other}"),
                        }
                    }
                }
                if let Some(id) = args.get("open").and_then(Value::as_i64) {
                    grainy_mcp::Host::activate(h, id).unwrap();
                }
                if let Some(edit) = args.get("edit").and_then(Value::as_object) {
                    for (key, value) in edit {
                        h.edit(key, value.clone());
                    }
                }
                if args.get("commit").and_then(Value::as_bool) == Some(true) {
                    grainy_mcp::Host::commit(h).unwrap();
                }
            }),
            "gui" => headless.set_closed(args["closed"].as_bool().unwrap()),
            "run_jobs" => {
                let ran = headless.run_pending(args.get("limit").and_then(Value::as_u64).map(|v| v as usize));
                if ran as u64 != step["ran"].as_u64().unwrap() {
                    fail(&mut problems, "ran", ran.to_string(), step["ran"].to_string());
                }
            }
            "stop_job" => service.stop_job(),
            "restart" => service.restart(),
            "limits" => {
                service.set_limits(Limits { wait_seconds: 0.0, retry_seconds: args["retry"].as_f64().unwrap() })
            }
            "preference" => headless
                .with(|h| h.catalog_ref().save_preference(args["key"].as_str().unwrap(), &args["value"]).unwrap()),
            "mcp_preference" => service.set_preference(args["key"].as_str().unwrap(), args["value"].clone()),
            "delete" => std::fs::remove_file(root.join(args["path"].as_str().unwrap())).unwrap(),
            "mkdir" => std::fs::create_dir_all(root.join(args["path"].as_str().unwrap())).unwrap(),
            "save_preset" => headless.with(|h| {
                let settings = grainy_core::Settings(args["settings"].as_object().unwrap().clone());
                h.catalog_ref().save_preset(args["name"].as_str().unwrap(), &settings).unwrap();
            }),
            other => panic!("unknown op {other}"),
        }
        let jobs: Vec<Value> = service.jobs().iter().map(|j| scrub.value(&j.view())).collect();
        compare("jobs", text(&Value::Array(jobs)), text(&step["jobs"]), &mut problems);
        if headless.pending() as u64 != step["queue"].as_u64().unwrap() {
            fail(&mut problems, "queue", headless.pending().to_string(), step["queue"].to_string());
        }
        let events = scrub.value(&Value::Array(headless.with(|h| h.events.clone())));
        if text(&events) != text(&step["events"]) {
            fail(&mut problems, "events", text(&events), text(&step["events"]));
        }
        let mut listing = Vec::new();
        for base in ["Pictures", "out"] {
            let mut stack = vec![root.join(base)];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        let relative = p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                        listing.push((relative, std::fs::metadata(&p).unwrap().len()));
                    }
                }
            }
        }
        listing.sort();
        let expected: Vec<(String, u64)> = step["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| (o[0].as_str().unwrap().to_owned(), o[1].as_u64().unwrap()))
            .collect();
        let names = |l: &[(String, u64)]| l.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
        if names(&listing) != names(&expected) {
            fail(&mut problems, "output files", format!("{:?}", names(&listing)), format!("{:?}", names(&expected)));
        } else {
            for ((n, a), (_, b)) in listing.iter().zip(&expected) {
                let exported_jpeg = n.ends_with(".jpg") && !n.contains("contact-sheet");
                if a != b && exported_jpeg {
                    fail(&mut problems, "exported JPEG size", format!("{n} {a}"), format!("{n} {b}"));
                }
                if index + 1 == case["steps"].as_array().unwrap().len() {
                    measure.output_sizes.push((n.clone(), *a, *b));
                }
            }
        }
        // The catalogue: every table row by row against the oracle's state after this step.
        if let Some(delta) = step["delta"].as_object() {
            for (table, rows) in delta {
                oracle_state.insert(table.clone(), rows.clone());
            }
        }
        let current = dump(&reader, &scrub);
        for (table, rows) in &current {
            let theirs = oracle_state.get(table).and_then(Value::as_array).cloned().unwrap_or_default();
            let ours = rows.as_array().unwrap();
            if ours.len() != theirs.len() {
                fail(&mut problems, &format!("catalogue {table} rows"), text(rows), text(&Value::Array(theirs)));
                continue;
            }
            for (a, b) in ours.iter().zip(&theirs) {
                if a == b {
                    continue;
                }
                let explained =
                    only_hashes_differ(a, b) && diverged.iter().any(|(t, k, v)| t == table && a.get(k) == Some(v));
                let key = format!("{table}:{}", text(a));
                if explained {
                    if !reported.contains(&key) {
                        reported.push(key);
                        measure.explained_rows += 1;
                    }
                } else {
                    fail(&mut problems, &format!("catalogue {table}"), text(a), text(b));
                }
            }
        }
        for (what, actual, expected) in problems {
            let (a, e) = around(&actual, &expected);
            measure
                .mismatches
                .push(format!("{name} step {index} ({op} {tool}): {what}\n   rust:   {a}\n   oracle: {e}"));
        }
        measure.byte_lengths_only.extend(bytes_only);
    }
    observe::record(false);
    measure
}

fn ids(value: &Value) -> Vec<i64> {
    value.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}

/// The two texts around their first difference.
fn around(a: &str, b: &str) -> (String, String) {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let at = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(150);
    let show = |t: &[char]| t[from.min(t.len())..(at + 250).min(t.len())].iter().collect::<String>();
    (show(&a), show(&b))
}

#[test]
fn sessions_replay_like_the_oracle() {
    let mut all = Vec::new();
    for name in ["main", "many"] {
        let m = replay(name);
        let bytes: Vec<String> = m.jpeg_bytes.iter().map(|(a, b)| format!("{a}/{b}")).collect();
        eprintln!(
            "{name}: pixels handed to the JPEG encoder: {} of {} equal; previews max diff {}; contact sheets: pictures \
             max diff {}, caption text max diff {}; decoded reply JPEGs max diff {}; JPEG reply bytes rust/oracle {}; \
             replies differing only in a file's byte length: {:?}; settings within {:e}; rows explained by those \
             settings {}; files written (rust/oracle bytes) {:?}",
            m.raw_equal,
            m.raw_compared,
            m.raw_max,
            m.sheet_pictures_max,
            m.sheet_text_max,
            m.decoded_max,
            bytes.join(" "),
            m.byte_lengths_only,
            m.settings_max,
            m.explained_rows,
            m.output_sizes
        );
        assert_eq!(m.raw_max, 0, "{name}: preview pixels differ");
        assert_eq!(m.sheet_pictures_max, 0, "{name}: contact sheet pictures differ");
        all.extend(m.mismatches);
    }
    for line in all.iter().take(40) {
        eprintln!("{line}");
    }
    assert!(all.is_empty(), "{} mismatches", all.len());
}
