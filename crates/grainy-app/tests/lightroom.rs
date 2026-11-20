//! Work package 29, the Lightroom import, against the oracle (`golden/app/lightroom_*`, built by
//! `tools/golden/gen_app_lightroom.py` with the Python Grainy's own `lightroom_import.import_catalog`,
//! `inspect_catalog`, `report_text`, `adobe_history.transfer`, `lightroom_import_dialog.begin_import` and
//! `ExtraTools.lightroom_report`, on Lightroom catalogues made by the oracle's own test fixture):
//!
//! * every scenario (plain import, re-import, a changed source, a photo already in the library, converted edits
//!   with history and snapshots, a history gap, broken records and odd paths, smart collections, a catalogue
//!   without an identity, a cancel at each stage, a failure, the library itself) leaves every table of the
//!   library equal to the oracle's after each step, with the same result dict, result text and stored summary;
//! * the window's flow through the commands (review box declined, cancelled, accepted with and without
//!   conversion), the stored report, the refusal while a job runs;
//! * the `.lrcat` is never written (its bytes stay the same) and photo files are never needed;
//! * the review box and the result box (snapshots in Korean and English).
//!
//! The catalogues are rebuilt here with plain SQLite from the generator's recipe (the oracle's 15.5.1 table
//! definitions and every insert in order), in temporary folders.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::files::FilesState;
#[cfg(windows)]
use grainy_app::commands::lightroom::import::Progress;
use grainy_app::commands::lightroom::import::{self, Failure, TickingClock};
use grainy_app::commands::lightroom::{self as lightroom, LightroomState, Review};
use grainy_app::commands::Invocation;
use grainy_app::dialogs::about::MessageBox;
use grainy_app::dialogs::lightroom::ReviewDialog;
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::TestCard;
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::folders::Platform;
use grainy_catalog::rusqlite::{self, types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_library::{pathx, Cancel};
use grainy_testkit::Case;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

// ---- building the catalogues ----------------------------------------------------------------------------------

fn bind(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match value {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => V::Integer(i),
            None => V::Real(n.as_f64().unwrap()),
        },
        Value::String(s) => V::Text(s.clone()),
        other => panic!("cannot bind {other}"),
    }
}

/// Runs recipe operations `ops` on the catalogue at `path` (the oracle's `insert` and statements).
fn apply(path: &Path, ops: &[Value]) {
    let db = Connection::open(path).unwrap();
    for op in ops {
        match op[0].as_str().unwrap() {
            "insert" => {
                let table = op[1].as_str().unwrap();
                let values = op[2].as_object().unwrap();
                let columns: Vec<&str> = values.keys().map(String::as_str).collect();
                let marks = vec!["?"; columns.len()].join(",");
                let sql = format!("INSERT INTO {table}({}) VALUES({marks})", columns.join(","));
                let params: Vec<rusqlite::types::Value> = values.values().map(bind).collect();
                db.execute(&sql, rusqlite::params_from_iter(params)).unwrap();
            }
            "sql" => {
                let params: Vec<rusqlite::types::Value> = op[2].as_array().unwrap().iter().map(bind).collect();
                db.execute(op[1].as_str().unwrap(), rusqlite::params_from_iter(params)).unwrap();
            }
            other => panic!("unknown recipe operation {other}"),
        }
    }
}

/// The `.lrcat` of a scenario: the schema, then the first `count` operations of the recipe.
fn build_source(path: &Path, schema: &[Value], recipe: &[Value], count: usize) {
    {
        let db = Connection::open(path).unwrap();
        for statement in schema {
            db.execute_batch(statement.as_str().unwrap()).unwrap();
        }
    }
    apply(path, &recipe[..count]);
}

// ---- comparing ---------------------------------------------------------------------------------------------

struct Env {
    root_text: String,
}

impl Env {
    fn new(name: &str) -> (Self, PathBuf) {
        let dir = common::temp_dir(name);
        let root_text = pathx::resolve(Platform::native(), &pathx::text(&dir));
        (Self { root_text: root_text.clone() }, PathBuf::from(root_text))
    }

    fn rel(&self, text: &str) -> String {
        // Also inside stored JSON text, where backslashes are doubled.
        // And casefolded, in the source keys of catalogues without an identity.
        let mut text = text.to_owned();
        for root in [self.root_text.clone(), pathx::casefold(&self.root_text)] {
            text = text.replace(&root.replace('\\', "\\\\"), "<ROOT>").replace(&root, "<ROOT>");
        }
        text
    }

    #[cfg(windows)]
    fn rel_value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.rel(s)),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.rel_value(v)).collect()),
            Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (self.rel(k), self.rel_value(v))).collect()),
            other => other.clone(),
        }
    }

    /// The generator's `plain(rel(cell))`.
    fn cell(&self, value: ValueRef<'_>) -> Value {
        match value {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(i) => json!(i),
            ValueRef::Real(f) => json!(f),
            ValueRef::Text(t) => {
                let text = self.rel(std::str::from_utf8(t).unwrap());
                if text.chars().count() > 160 {
                    let head: String = text.chars().take(80).collect();
                    let sha: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
                    json!({"sha256": sha, "head": head})
                } else {
                    Value::String(text)
                }
            }
            ValueRef::Blob(b) => {
                use base64::Engine;
                json!({"base64": base64::engine::general_purpose::STANDARD.encode(b)})
            }
        }
    }

    /// The generator's `tables(db)`.
    fn tables(&self, db: &Connection) -> BTreeMap<String, Vec<Value>> {
        let names: Vec<String> = db
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut out = BTreeMap::new();
        for name in names {
            let mut statement = db.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
            let columns: Vec<String> = statement.column_names().iter().map(|s| (*s).to_owned()).collect();
            let mut rows = statement.query([]).unwrap();
            let mut list: Vec<(String, Value)> = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                let mut map = Map::new();
                for (i, column) in columns.iter().enumerate() {
                    map.insert(column.clone(), self.cell(row.get_ref(i).unwrap()));
                }
                if name == "preview_state" {
                    // A trigger writes random tokens (randomblob): only their presence counts.
                    map.insert("token".into(), json!("<token>"));
                }
                let value = Value::Object(map);
                list.push((grainy_catalog::json::dumps(&value, false, true, true), value));
            }
            list.sort_by(|a, b| a.0.cmp(&b.0));
            out.insert(name, list.into_iter().map(|(_, v)| v).collect());
        }
        out
    }
}

/// Compares the tables a golden step lists (the ones that changed) and the set of table names.
#[cfg(windows)]
fn compare_tables(what: &str, now: &BTreeMap<String, Vec<Value>>, step: &Value) {
    if let Some(names) = step.get("table_names") {
        let expected: Vec<&str> = names.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let actual: Vec<&str> = now.keys().map(String::as_str).collect();
        assert_eq!(actual, expected, "{what}: table names");
    }
    let Some(tables) = step.get("tables").and_then(Value::as_object) else { return };
    for (name, rows) in tables {
        let actual = Value::Array(now.get(name).cloned().unwrap_or_default());
        if &actual != rows {
            let a = actual.as_array().unwrap();
            let e = rows.as_array().unwrap();
            for (i, (x, y)) in a.iter().zip(e).enumerate() {
                assert_eq!(x, y, "{what}: table {name} row {i}");
            }
            assert_eq!(a.len(), e.len(), "{what}: table {name} row count");
        }
    }
}

/// Also checks that unchanged tables did not change on this side.
#[cfg(windows)]
fn unchanged(what: &str, before: &BTreeMap<String, Vec<Value>>, now: &BTreeMap<String, Vec<Value>>, step: &Value) {
    let listed = step.get("tables").and_then(Value::as_object);
    for (name, rows) in now {
        if listed.is_some_and(|t| t.contains_key(name)) {
            continue;
        }
        assert_eq!(before.get(name), Some(rows), "{what}: table {name} changed but the oracle's did not");
    }
}

fn stamp() -> String {
    Case::open("app", "lightroom_import").json()["stamp"].as_str().unwrap().to_owned()
}

fn library(folder: &Path, stamp: &str) -> Catalog {
    let stamp = stamp.to_owned();
    Catalog::with_sources(folder, Sources::new(move || stamp.clone(), || "token".to_owned())).unwrap()
}

// ---- the scenarios -----------------------------------------------------------------------------------------

// The scenario catalogues are Windows Lightroom catalogues (root folders `C:\Grainy Lightroom Test\Photos`):
// on macOS no such path exists and both the oracle and the app skip their photos as unsupported paths, so the
// scenarios and the window's flow compare with the goldens on Windows only. The source's safety (never
// written), the inspection, the cancel and the damaged catalogue run everywhere.

#[cfg(windows)]
struct Run<'a> {
    env: &'a Env,
    scenario: &'a Value,
    source: PathBuf,
    dest: Catalog,
    clock: TickingClock,
    index: usize,
    previous: BTreeMap<String, Vec<Value>>,
}

#[cfg(windows)]
impl Run<'_> {
    fn step(&self) -> &Value {
        &self.scenario["steps"][self.index]
    }

    fn what(&self) -> String {
        format!("{} / {}", self.scenario["name"].as_str().unwrap(), self.step()["step"].as_str().unwrap())
    }

    /// Checks the next golden step is `label` and compares the tables; returns the step.
    fn record(&mut self, label: &str) -> Value {
        self.index += 1;
        let step = self.step().clone();
        assert_eq!(step["step"], label, "{}: the script's order", self.what());
        if step.get("table_names").is_some() {
            let now = self.env.tables(self.dest.connection());
            compare_tables(&self.what(), &now, &step);
            unchanged(&self.what(), &self.previous, &now, &step);
            self.previous = now;
        }
        let stored = lightroom::latest_summary(&self.dest).map(|s| self.env.rel(&lightroom::report_text(&s)));
        assert_eq!(stored.as_deref(), step["stored"].as_str(), "{}: stored report", self.what());
        step
    }

    fn import(&mut self, label: &str, source: Option<&Path>, convert: bool, cancel_at: Option<&str>) {
        let fail_at = (label == "import" && self.scenario["name"] == "failing").then_some("컬렉션과 스택 정리 중");
        let cancel = Cancel::new();
        let mut progress = |p: &Progress| {
            if Some(p.stage) == cancel_at {
                cancel.set();
            }
            if Some(p.stage) == fail_at {
                return Err("OSError: simulated disk failure".to_owned());
            }
            Ok(())
        };
        let source = source.map(Path::to_path_buf).unwrap_or_else(|| self.source.clone());
        let directory = self.dest.directory().to_path_buf();
        let outcome =
            import::import_catalog(&source, &directory, Some(&cancel), &mut progress, convert, &mut self.clock);
        let step = self.record(label);
        match outcome {
            Ok(result) => {
                assert!(step["error"].is_null(), "{}: the oracle failed with {}", self.what(), step["error"]);
                let rel = self.env.rel_value(&Value::Object(result.clone()));
                assert_eq!(rel, step["result"], "{}: result", self.what());
                assert_eq!(
                    self.env.rel(&lightroom::report_text(&result)),
                    step["report"].as_str().unwrap(),
                    "{}: report",
                    self.what()
                );
            }
            Err(failure) => {
                assert_eq!(
                    self.env.rel(&failure.text()),
                    step["error"].as_str().unwrap_or("(none)"),
                    "{}: error",
                    self.what()
                );
            }
        }
        let backups: Vec<String> = match std::fs::read_dir(directory.join("backups")) {
            Ok(entries) => {
                let mut names: Vec<String> =
                    entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
                names.sort();
                names
            }
            Err(_) => Vec::new(),
        };
        assert_eq!(json!(backups), step["backups"], "{}: backups", self.what());
        if let Some(pending) = step.get("pending") {
            assert_eq!(pending, &json!([]), "{}", self.what());
        }
    }
}

#[cfg(windows)]
fn first_photo(dest: &Catalog) -> i64 {
    dest.photos().unwrap()[0]["id"].as_i64().unwrap()
}

#[cfg(windows)]
fn fields(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}

#[cfg(windows)]
fn run_scenario(env: &Env, root: &Path, golden: &Value, scenario: &Value) {
    let name = scenario["name"].as_str().unwrap();
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("fixture.lrcat");
    let recipe = scenario["recipe"].as_array().unwrap();
    let built = scenario["built"].as_u64().unwrap() as usize;
    build_source(&source, golden["schema"].as_array().unwrap(), recipe, built);
    let stamp = golden["stamp"].as_str().unwrap();
    let dest = library(&folder.join("luma"), stamp);
    let mut run = Run {
        env,
        scenario,
        source: source.clone(),
        dest,
        clock: TickingClock::new(stamp).unwrap(),
        index: 0,
        previous: BTreeMap::new(),
    };
    // The start: an empty library.
    let start = run.step().clone();
    let now = env.tables(run.dest.connection());
    compare_tables(&format!("{name} / start"), &now, &start);
    run.previous = now;
    let photos = golden["photos"].as_str().unwrap();
    let photo = |file: &str| pathx::join(Platform::native(), photos, file);
    match name {
        "plain" => {
            run.import("import", None, false, None);
            let ident = first_photo(&run.dest);
            run.dest.edit(ident, &common::settings(json!({"exposure": 0.75})), "보정").unwrap();
            run.dest.update(ident, &fields(&[("rating", json!(5)), ("keywords", json!("keep mine"))])).unwrap();
            run.record("edited");
            run.import("again", None, false, None);
            let step = scenario["steps"][run.index + 1].clone();
            let length = step["recipe_length"].as_u64().unwrap() as usize;
            apply(&source, &recipe[built..length]);
            run.index += 1;
            assert_eq!(step["step"], "source changed");
            run.import("changed", None, false, None);
        }
        "existing" => {
            let ident = run
                .dest
                .add(&photo("photo0.jpg"), Some(&common::settings(json!({"exposure": 2}))), None, false)
                .unwrap();
            run.dest
                .update(ident, &fields(&[("rating", json!(5)), ("keywords", json!("Luma")), ("label", json!("초록"))]))
                .unwrap();
            run.record("added");
            run.import("import", None, false, None);
        }
        "convert" => {
            run.import("import", None, true, None);
            let master = run
                .dest
                .photos()
                .unwrap()
                .into_iter()
                .find(|p| p["path"].as_str().unwrap().ends_with("photo0.jpg") && p["virtual_source"].is_null())
                .unwrap();
            let id = master["id"].as_i64().unwrap();
            run.dest.edit(id, &common::settings(json!({"exposure": 2.5})), "보정").unwrap();
            run.dest.save_preference(&format!("undo:{id}"), &json!({"undo": [{"exposure": 1}], "redo": []})).unwrap();
            run.record("edited");
            run.import("again", None, true, None);
            run.import("without conversion", None, false, None);
        }
        "gap" | "broken_convert" => run.import("import", None, true, None),
        "broken" | "smart" | "no_provider" => run.import("import", None, false, None),
        "cancel_backup" | "cancel_reading" | "cancel_photos" | "cancel_collections" => {
            run.dest.add(&photo("keep.jpg"), None, None, false).unwrap();
            run.record("added");
            let stage = match name {
                "cancel_backup" => "안전 백업 만드는 중",
                "cancel_reading" => "Lightroom 기록 읽는 중",
                "cancel_photos" => "사진 분류 가져오는 중",
                _ => "컬렉션과 스택 정리 중",
            };
            run.import("import", None, false, Some(stage));
        }
        "failing" => {
            run.import("import", None, false, None);
            let itself = run.dest.directory().join("catalog.sqlite");
            run.import("itself", Some(&itself), false, None);
        }
        other => panic!("unknown scenario {other}"),
    }
    assert_eq!(run.index + 1, scenario["steps"].as_array().unwrap().len(), "{name}: every step ran");
}

fn file_digest(path: &Path) -> Vec<u8> {
    Sha256::digest(std::fs::read(path).unwrap()).to_vec()
}

#[cfg(windows)]
#[test]
fn every_scenario_equals_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let golden = Case::open("app", "lightroom_import").json().clone();
    let (env, root) = Env::new("lightroom-import");
    for scenario in golden["scenarios"].as_array().unwrap() {
        run_scenario(&env, &root, &golden, scenario);
    }
}

#[test]
fn the_source_catalogue_is_never_written() {
    let golden = Case::open("app", "lightroom_import").json().clone();
    let scenario = golden["scenarios"].as_array().unwrap().iter().find(|s| s["name"] == "convert").unwrap();
    let (_, root) = Env::new("lightroom-readonly");
    let source = root.join("fixture.lrcat");
    let recipe = scenario["recipe"].as_array().unwrap();
    build_source(&source, golden["schema"].as_array().unwrap(), recipe, recipe.len());
    let before = file_digest(&source);
    let dest = library(&root.join("luma"), &stamp());
    let mut clock = TickingClock::new(&stamp()).unwrap();
    let result = import::import_catalog(&source, dest.directory(), None, &mut |_| Ok(()), true, &mut clock).unwrap();
    // The fixture's photos are on `C:\...`: imported on Windows, skipped as unsupported paths elsewhere.
    if cfg!(windows) {
        assert_eq!(result["imported"], 3);
    }
    assert_eq!(file_digest(&source), before);
    // No sidecar files appear next to the catalogue.
    let mut names: Vec<String> =
        std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["fixture.lrcat", "luma"]);
}

#[test]
fn inspection_equals_the_oracle() {
    let golden = Case::open("app", "lightroom_inspect").json().clone();
    let (env, root) = Env::new("lightroom-inspect");
    std::fs::create_dir_all(root.join("inspect")).unwrap();
    let source = root.join("inspect").join("fixture.lrcat");
    let schema = Case::open("app", "lightroom_import").json()["schema"].clone();
    let recipe = golden["recipe"].as_array().unwrap();
    build_source(&source, schema.as_array().unwrap(), recipe, recipe.len());
    let info = import::inspect_catalog(&source, None).unwrap();
    assert_eq!(env.rel(&info.path), common::native(golden["info"]["path"].as_str().unwrap()));
    assert_eq!(info.source_key, golden["info"]["source_key"].as_str().unwrap());
    let counts: Map<String, Value> = info.counts.iter().map(|(k, n)| (k.clone(), json!(n))).collect();
    assert_eq!(Value::Object(counts), golden["info"]["counts"]);
    let wrong = root.join("inspect").join("wrong.lrcat");
    Connection::open(&wrong).unwrap().execute_batch("CREATE TABLE Adobe_images(id_local INTEGER)").unwrap();
    match import::inspect_catalog(&wrong, None) {
        Err(Failure::Error(text)) => assert_eq!(text, format!("ValueError: {}", golden["refused"].as_str().unwrap())),
        other => panic!("{other:?}"),
    }
    // A cancel before the counts stops the inspection.
    let cancel = Cancel::new();
    cancel.set();
    assert!(matches!(import::inspect_catalog(&source, Some(&cancel)), Err(Failure::Cancelled)));
}

// ---- the window's flow -----------------------------------------------------------------------------------------

fn drain(cx: &mut AppContext) {
    let started = Instant::now();
    loop {
        for finished in cx.jobs.poll() {
            finished(cx);
        }
        if cx.jobs.idle() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(120), "a job did not finish");
    }
}

#[cfg(windows)]
#[test]
fn the_window_flow_equals_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let golden = Case::open("app", "lightroom_flow").json().clone();
    let schema = Case::open("app", "lightroom_import").json()["schema"].clone();
    let stamp = golden["stamp"].as_str().unwrap().to_owned();
    let (env, root) = Env::new("lightroom-flow");
    let folder = root.join("flow");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("fixture.lrcat");
    let recipe = golden["recipe"].as_array().unwrap();
    build_source(&source, schema.as_array().unwrap(), recipe, recipe.len());
    let before = file_digest(&source);
    let catalog = library(&folder.join("luma"), &stamp);
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    lightroom::set_clock(&mut cx, Arc::new(Mutex::new(TickingClock::new(&stamp).unwrap())));
    let path = Invocation { key: None, value: Some(json!(pathx::text(&source))) };
    let mut previous = BTreeMap::new();
    for step in golden["steps"].as_array().unwrap() {
        let what = step["step"].as_str().unwrap();
        assert!(cx.run_with("extras.lightroom_import", &path), "{what}");
        assert!(cx.dialog_open("maintenance"), "{what}: the maintenance window");
        assert!(cx.state::<FilesState>().maintenance, "{what}");
        drain(&mut cx);
        let review = cx.state::<LightroomState>().review.clone().expect("the review box");
        let expected = &step["reviews"][0];
        assert_eq!(review.text, expected["text"].as_str().unwrap(), "{what}: review text");
        assert!(!review.convert && !expected["checked"].as_bool().unwrap());
        assert!(cx.dialog_open("lightroom_review"), "{what}");
        let answer = &step["answer"];
        if answer.get("cancel").is_some_and(|c| c.as_bool() == Some(true)) {
            cx.state::<LightroomState>().shared.as_ref().unwrap().cancel.set();
        }
        lightroom::answer_review(&mut cx, answer["import"].as_bool().unwrap(), answer["convert"].as_bool().unwrap());
        drain(&mut cx);
        assert!(!lightroom::running(&cx), "{what}: the run ended");
        assert!(!cx.state::<FilesState>().maintenance, "{what}");
        let last = cx.state::<LightroomState>().last_import.clone().map(|r| env.rel_value(&Value::Object(r)));
        assert_eq!(last.unwrap_or(Value::Null), step["last"], "{what}: last import");
        let shown: Vec<&Value> = step["shown"].as_array().unwrap().iter().filter(|s| s[0] == "information").collect();
        match shown.first() {
            Some(info) => {
                assert!(cx.dialog_open("lightroom_result"), "{what}: the result box");
                let result = cx.state::<LightroomState>().last_import.clone().unwrap();
                assert_eq!(info[1], "Lightroom 가져오기 결과");
                assert_eq!(env.rel(&lightroom::report_text(&result)), info[2].as_str().unwrap(), "{what}");
                cx.close_dialog("lightroom_result");
            }
            None => assert!(!cx.dialog_open("lightroom_result"), "{what}"),
        }
        assert!(cx.state::<LightroomState>().last_error.is_none(), "{what}");
        let now = env.tables(cx.doc.metadata_catalog().unwrap().connection());
        compare_tables(what, &now, step);
        if !previous.is_empty() {
            unchanged(what, &previous, &now, step);
        }
        previous = now;
    }
    assert_eq!(file_digest(&source), before, "the .lrcat was not written");
    // The listing shows the imported photos (the refresh after the import).
    assert!(cx.doc.listing_pending() || cx.doc.photos().len() == 6, "a refresh was asked for");
    cx.doc.refresh().unwrap();
    assert_eq!(cx.doc.photos().len(), 6);
    // 최근 Lightroom 가져오기 결과.
    assert!(cx.run("extras.lightroom_report"));
    assert!(cx.dialog_open("lightroom_report"));
    let summary = lightroom::latest_summary(cx.doc.metadata_catalog().unwrap()).unwrap();
    assert_eq!(golden["report"][0][1], "최근 Lightroom 가져오기 결과");
    assert_eq!(env.rel(&lightroom::report_text(&summary)), golden["report"][0][2].as_str().unwrap());
    cx.close_dialog("lightroom_report");
    // Refused while a job runs.
    let (release, wait) = std::sync::mpsc::channel::<()>();
    cx.jobs.spawn("busy", "busy", move |_| wait.recv().map_err(|e| e.to_string()), |_: &mut AppContext, _| {});
    cx.run_with("extras.lightroom_import", &path);
    assert_eq!(cx.state::<LightroomState>().last_error.take().as_deref(), golden["refusal"].as_str());
    assert!(!lightroom::running(&cx));
    release.send(()).unwrap();
    drain(&mut cx);
    // A library without imports.
    let empty = library(&folder.join("empty"), &stamp);
    assert!(lightroom::latest_summary(&empty).is_none());
    assert_eq!(golden["nothing"][0][2], "저장된 가져오기 결과가 없습니다.");
}

#[test]
fn a_cancel_in_the_maintenance_window_leaves_nothing() {
    let golden = Case::open("app", "lightroom_flow").json().clone();
    let schema = Case::open("app", "lightroom_import").json()["schema"].clone();
    let stamp = golden["stamp"].as_str().unwrap().to_owned();
    let (env, root) = Env::new("lightroom-cancel");
    let source = root.join("fixture.lrcat");
    let recipe = golden["recipe"].as_array().unwrap();
    build_source(&source, schema.as_array().unwrap(), recipe, recipe.len());
    let catalog = library(&root.join("luma"), &stamp);
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    let before = env.tables(cx.doc.metadata_catalog().unwrap().connection());
    let path = Invocation { key: None, value: Some(json!(pathx::text(&source))) };
    // 취소 while the catalogue is inspected: no review box, no error.
    cx.run_with("extras.lightroom_import", &path);
    cx.state::<LightroomState>().shared.as_ref().unwrap().cancel.set();
    drain(&mut cx);
    assert!(cx.state::<LightroomState>().review.is_none());
    assert!(!lightroom::running(&cx));
    assert!(cx.state::<LightroomState>().last_error.is_none());
    // 취소 during the import: the result box says it was cancelled, nothing is added.
    cx.run_with("extras.lightroom_import", &path);
    drain(&mut cx);
    lightroom::answer_review(&mut cx, true, true);
    cx.state::<FilesState>(); // the job runs meanwhile
    if let Some(shared) = cx.state::<LightroomState>().shared.clone() {
        shared.cancel.set();
    }
    drain(&mut cx);
    let last = cx.state::<LightroomState>().last_import.clone();
    if let Some(last) = last {
        if last["cancelled"] == true {
            assert_eq!(env.tables(cx.doc.metadata_catalog().unwrap().connection()), before);
            assert!(cx.dialog_open("lightroom_result"));
        }
    } else {
        // The cancel arrived before the job started: the run ended without an import.
        assert_eq!(env.tables(cx.doc.metadata_catalog().unwrap().connection()), before);
    }
    assert!(!lightroom::running(&cx));
}

#[test]
fn a_damaged_catalogue_shows_the_error_box() {
    let _turn = common::language_lock();
    let (_, root) = Env::new("lightroom-damaged");
    let stamp = stamp();
    let catalog = library(&root.join("luma"), &stamp);
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    let wrong = root.join("wrong.lrcat");
    Connection::open(&wrong).unwrap().execute_batch("CREATE TABLE Adobe_images(id_local INTEGER)").unwrap();
    cx.run_with("extras.lightroom_import", &Invocation { key: None, value: Some(json!(pathx::text(&wrong))) });
    drain(&mut cx);
    assert_eq!(
        cx.state::<LightroomState>().last_error.as_deref(),
        Some("ValueError: 이 Lightroom 카탈로그 구조는 지원하지 않습니다: AgLibraryRootFolder")
    );
    assert!(cx.dialog_open("lightroom_error"));
    assert!(!lightroom::running(&cx));
}

#[test]
fn numbers_and_texts() {
    assert_eq!(lightroom::thousands(0), "0");
    assert_eq!(lightroom::thousands(1234567), "1,234,567");
    assert_eq!(lightroom::thousands(-1000), "-1,000");
    assert_eq!(import::py_number(&json!("4"), 0, 5), 4);
    assert_eq!(import::py_number(&json!(" 9 "), 0, 5), 5);
    assert_eq!(import::py_number(&json!("2.5"), 0, 5), 0);
    assert_eq!(import::py_number(&json!(-2.7), -1, 1), -1);
    assert_eq!(import::py_number(&json!(null), -1, 1), 0);
    assert_eq!(import::py_number(&json!(true), 0, 5), 1);
}

// ---- the window ---------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn render(language: Language, name: &str, setup: impl FnOnce(&mut GrainyApp) + 'static) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        setup(&mut app);
        app
    });
    harness.run_steps(4);
    common::snapshot::save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

fn review(language: Language, tag: &str) {
    render(language, &format!("lightroom-review-{tag}"), |app| {
        let info = import::Inspection {
            path: r"D:\Lightroom\Catalog.lrcat".into(),
            source_key: "catalog:1".into(),
            counts: vec![("Adobe_images".into(), 12480)],
        };
        let text = lightroom::review_text(&info);
        app.cx.state::<LightroomState>().review =
            Some(Review { path: PathBuf::from(r"D:\Lightroom\Catalog.lrcat"), info, text, convert: false });
        app.cx.open_dialog(Box::new(ReviewDialog));
    });
}

#[test]
fn review_box_korean() {
    review(Language::Korean, "ko");
}

#[test]
fn review_box_english() {
    review(Language::English, "en");
}

fn result(language: Language, tag: &str) {
    render(language, &format!("lightroom-result-{tag}"), |app| {
        let golden = Case::open("app", "lightroom_import").json().clone();
        let scenario = golden["scenarios"].as_array().unwrap().iter().find(|s| s["name"] == "broken_convert").unwrap();
        let mut result = scenario["steps"][1]["result"].as_object().unwrap().clone();
        result.insert(
            "backup".into(),
            json!(r"D:\Grainy\backups\before-lightroom-00000000000000000000000000000001.sqlite"),
        );
        let text = lightroom::report_text(&result);
        app.cx.open_dialog(Box::new(MessageBox::information(
            "lightroom_result",
            grainy_app::text::tr("Lightroom 가져오기 결과"),
            text,
        )));
    });
}

#[test]
fn result_box_korean() {
    result(Language::Korean, "ko");
}

#[test]
fn result_box_english() {
    result(Language::English, "en");
}
