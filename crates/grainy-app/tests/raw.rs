//! The RAW package against the oracle (`golden/app/raw_*`, built by `tools/golden/gen_app_raw.py` with the Python
//! Grainy's own `profile_library`, `ProfileDialog`, `RawDefaultsDialog` and the `StudioTools` methods on stand-ins):
//!
//! * the profile library: the roots, three scans of the same tree of DCP files (first, cached, after a change and a
//!   removal) with the index rows, a capped and a cancelled scan, `compatible` for four cameras, six searches and
//!   the favourites, `load_selected` of every row, a profile changed after its scan; the browser's list, detail,
//!   status and buttons after the oracle's clicks;
//! * the RAW import defaults box: the same script of scopes, sources, names, groups, ISO anchors (and their
//!   refusals), toggling, removal and applying, on a catalogue of the same rows: rules, stored preference, list,
//!   status, warnings, undo steps and the applied settings;
//! * the calibration panel's methods: working space, DCP on and off (refusals included), the RAW white-balance
//!   modes with the resolved camera white of a generated DNG, the temperature sliders, the colour matrix files.
//!
//! And the window, in Korean and English: the calibration panel with its RAW fold-out, the camera profile browser
//! and the RAW defaults box on a RAW photo of a library in a fixed temporary folder (snapshots `raw-*`).
//!
//! Every test works on temporary folders and generated files; no library or profile folder of the owner is read.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::raw;
use grainy_app::dialogs::profiles::{self, ProfileBrowser, ProfileEnvironment, ProfileRow, ScanResult};
use grainy_app::dialogs::raw_defaults::{Host, RawDefaults, PREFERENCE};
use grainy_app::state::{py_eq, EditState};
use grainy_app::GrainyApp;
use grainy_catalog::{Catalog, Sources};
use grainy_core::{normalized, Settings};
use grainy_i18n::Language;
use grainy_raw::dcp;
use grainy_raw::rawcolor::CameraInfo;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

/// The interface language is process-wide: the tests that read texts or render take turns (`common::language_lock`).
fn korean() -> std::sync::MutexGuard<'static, ()> {
    let turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    turn
}

fn case(name: &str) -> Value {
    Case::open("app", name).json().clone()
}

fn golden() -> PathBuf {
    grainy_testkit::root().join("golden")
}

fn dng() -> PathBuf {
    golden().join("rawload").join("_files").join("triple.dng")
}

fn fixture(name: &str) -> PathBuf {
    golden().join("raw").join("_files").join(format!("profile_{name}")).join("profile.dcp")
}

/// The generator's `slim`: pinned DCP records written as their name, camera and digest.
fn slim(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if k == "dcp_profile" && v.is_object() { summary(v) } else { slim(v) };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(slim).collect()),
        other => other.clone(),
    }
}

fn summary(record: &Value) -> Value {
    if !grainy_core::settings::truthy(record) {
        return Value::Null;
    }
    json!({"name": record["name"], "camera": record["camera"], "sha256": record["sha256"]})
}

fn same(actual: &Value, expected: &Value, at: &str) {
    assert!(py_eq(actual, expected), "{at}:\n  rust:   {actual}\n  oracle: {expected}");
}

// ---- the profile library ------------------------------------------------------------------------------------

/// The generator's tree in a temporary folder: (folder, `text` of it for the placeholders).
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let dir = common::temp_dir(name);
        Self { root: profiles::resolve(&dir) }
    }

    fn place(&self, destination: &str, source: &Value, time: i64) {
        let target = self.root.join(destination);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        match source.as_str() {
            Some(source) => {
                std::fs::copy(golden().join(source), &target).unwrap();
            }
            None => std::fs::write(&target, b"not a profile").unwrap(),
        }
        let file = std::fs::File::options().write(true).open(&target).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(time as u64)).unwrap();
    }

    /// Paths below the folder written as `<T>` (and lower-cased index keys as `<t>`).
    fn text(&self, value: &str) -> String {
        let root = self.root.to_string_lossy().into_owned();
        value.replace(&root, "<T>").replace(&root.to_lowercase(), "<t>")
    }

    fn env(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for (key, folder) in [("PROGRAMDATA", "programdata"), ("APPDATA", "appdata")] {
            map.insert(key.to_owned(), self.root.join(folder).to_string_lossy().into_owned());
        }
        map
    }

    fn own(&self) -> PathBuf {
        self.root.join("localappdata").join("Luma").join("CameraProfiles")
    }

    fn roots(&self, folders: &[String]) -> Vec<String> {
        let env = self.env();
        profiles::profile_roots(&self.root.join("lib"), folders, &move |k| env.get(k).cloned(), &self.own())
    }

    /// The roots the tests scan: [`Tree::roots`] without the computer's own folders (macOS adds Adobe's
    /// `/Library/Application Support/...`, which a Mac with Adobe's software fills), so a scan reads only the
    /// generated tree. On Windows every root is in the tree.
    fn scan_roots(&self, folders: &[String]) -> Vec<String> {
        self.roots(folders).into_iter().filter(|r| Path::new(r).starts_with(&self.root)).collect()
    }

    /// The oracle's `profile_roots` on this system for the generator's tree: the golden's (Windows) list, and on
    /// macOS Adobe's all-users folder before Grainy's own (`if MAC: roots.extend([Path('/')/support,
    /// Path.home()/support, local_data('Luma')/'CameraProfiles'])`; the home folder comes from the environment,
    /// which has none here).
    fn expected_roots(&self, golden: &Value) -> Value {
        let mut roots: Vec<Value> = golden.as_array().unwrap().clone();
        if cfg!(target_os = "macos") {
            let own = json!(self.text(&self.own().to_string_lossy()));
            let at = roots.iter().position(|r| *r == own).expect("Grainy's own folder is a root");
            roots.insert(at, json!("/Library/Application Support/Adobe/CameraRaw/CameraProfiles"));
        }
        Value::Array(roots)
    }

    fn row_json(&self, row: &ProfileRow) -> Value {
        let mut value = row.to_json();
        value["error"] = Value::Bool(!row.error.is_empty());
        value["path"] = json!(self.text(&row.path));
        value["paths"] = json!(row.paths.iter().map(|p| self.text(p)).collect::<Vec<_>>());
        value
    }

    fn scan_json(&self, result: &ScanResult) -> Value {
        json!({
            "files": result.files, "parsed": result.parsed, "errors": result.errors.len(),
            "cancelled": result.cancelled, "truncated": result.truncated,
            "profiles": result.profiles.iter().map(|r| self.row_json(r)).collect::<Vec<_>>(),
        })
    }

    /// The oracle's index rows on this system: its keys are `os.path.normcase(path)`, lower case on Windows (the
    /// golden's `<t>` rows) and the path itself elsewhere, read in the order of the keys (`ORDER BY path`).
    fn expected_index(golden: &Value) -> Value {
        if cfg!(windows) {
            return golden.clone();
        }
        let mut rows: Vec<Value> = golden.as_array().unwrap().clone();
        for row in &mut rows {
            row["path"] = row["summary"]["path"].clone();
        }
        rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Value::Array(rows)
    }

    fn index_json(&self, cache: &Path) -> Value {
        let db = grainy_catalog::rusqlite::Connection::open(cache).unwrap();
        let mut statement = db.prepare("SELECT path,stamp,summary FROM profiles ORDER BY path").unwrap();
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .unwrap();
        let mut out = Vec::new();
        for row in rows {
            let (path, stamp, text) = row.unwrap();
            let mut value: Value = serde_json::from_str(&text).unwrap();
            value["error"] = Value::Bool(!value["error"].as_str().unwrap().is_empty());
            value["path"] = json!(self.text(value["path"].as_str().unwrap()));
            let parts: Vec<&str> = stamp.split(':').collect();
            out.push(
                json!({"path": self.text(&path), "stamp": format!("{}:{}", parts[0], parts[2]), "summary": value}),
            );
        }
        Value::Array(out)
    }
}

/// The profile rows of a scan, compared without the error texts (`grainy-raw` words its errors in English).
#[test]
fn the_profile_library_scans_like_the_oracle() {
    let _turn = korean();
    // The golden's backslashes are all path separators: on macOS the oracle's paths read `/`.
    let meta = common::native_json(&case("raw_profiles"));
    let time = meta["time"].as_i64().unwrap();
    let tree = Tree::new("raw-profiles");
    for file in meta["files"].as_array().unwrap() {
        tree.place(file[0].as_str().unwrap(), &file[1], time);
    }
    let extra = tree.root.join("extra").to_string_lossy().into_owned();
    let texts: Vec<String> = tree.roots(std::slice::from_ref(&extra)).iter().map(|r| tree.text(r)).collect();
    same(&json!(texts), &tree.expected_roots(&meta["roots"]), "roots");
    let roots = tree.scan_roots(std::slice::from_ref(&extra));

    let cache = tree.root.join("lib").join("cache").join("dcp-index.sqlite");
    let never = || false;
    let first = profiles::scan(&roots, &cache, &never, profiles::LIMIT).unwrap();
    same(&tree.scan_json(&first), &meta["scans"][0], "first scan");
    let second = profiles::scan(&roots, &cache, &never, profiles::LIMIT).unwrap();
    same(&tree.scan_json(&second), &meta["scans"][1], "cached scan");
    let changed = meta["changed"].as_array().unwrap();
    tree.place(changed[0].as_str().unwrap(), &changed[1], time + 100);
    std::fs::remove_file(tree.root.join(meta["removed"].as_str().unwrap())).unwrap();
    let third = profiles::scan(&roots, &cache, &never, profiles::LIMIT).unwrap();
    same(&tree.scan_json(&third), &meta["scans"][2], "scan after a change and a removal");
    same(&tree.index_json(&cache), &Tree::expected_index(&meta["index"]), "index");

    let capped_cache = tree.root.join("capped.sqlite");
    let capped = profiles::scan(&roots, &capped_cache, &never, 3).unwrap();
    same(&tree.scan_json(&capped), &meta["capped"], "capped scan");
    same(&tree.index_json(&capped_cache), &Tree::expected_index(&meta["capped_index"]), "capped index");
    let cancelled = profiles::scan(&roots, &tree.root.join("cancelled.sqlite"), &|| true, profiles::LIMIT).unwrap();
    same(&tree.scan_json(&cancelled), &meta["cancelled"], "cancelled scan");

    // compatible
    let rows = third.profiles.clone();
    let favorites: std::collections::BTreeSet<String> =
        meta["favorites"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    for expected in meta["compatible"].as_array().unwrap() {
        let metadata = &meta["metadata"][expected["metadata"].as_str().unwrap()];
        let found = profiles::compatible(
            &rows,
            metadata,
            expected["search"].as_str().unwrap(),
            &favorites,
            expected["only"].as_bool().unwrap(),
        );
        let names: Vec<String> = found.iter().map(|r| tree.text(&r.path)).collect();
        same(&json!(names), &expected["names"], &format!("compatible {expected}"));
    }

    // load_selected
    for expected in meta["loads"].as_array().unwrap() {
        let row = rows.iter().find(|r| tree.text(&r.path) == expected["path"].as_str().unwrap()).unwrap();
        let metadata = &meta["metadata"][expected["metadata"].as_str().unwrap()];
        match profiles::load_selected(row, metadata) {
            Ok(record) => same(&summary(&record), &expected["record"], &format!("load {expected}")),
            Err(message) => {
                let oracle = expected["error"].as_str().unwrap_or_else(|| panic!("{expected}: {message}"));
                assert!(message.starts_with("프로파일을 읽을 수 없습니다. "), "{message}");
                if oracle.contains("현재 카메라와 다른 프로파일입니다.") {
                    assert_eq!(message, oracle);
                }
            }
        }
    }
    let stale = rows.iter().find(|r| r.name == "Synthetic triple").unwrap().clone();
    tree.place("extra/synth triple.dcp", &json!("raw/_files/profile_synthetic_dual/profile.dcp"), time);
    assert_eq!(
        profiles::load_selected(&stale, &meta["metadata"]["synthetic"]).unwrap_err(),
        meta["stale"].as_str().unwrap()
    );
    tree.place("extra/synth triple.dcp", &json!("raw/_files/profile_synthetic_triple/profile.dcp"), time);
    println!("profile library: every scan, index row, filter and load equal to the oracle's (exact)");
}

/// The browser's state as the generator reads the dialog (`dialog_state`); the detail of a refused profile
/// keeps its first two lines (its last is `grainy-raw`'s English error text).
fn browser_state(tree: &Tree, b: &ProfileBrowser) -> Value {
    let items: Vec<String> = b.shown.iter().map(|r| b.item_text(r)).collect();
    json!({
        "items": items, "current": b.current.map_or(-1, |c| c as i64), "detail": tree.text(&b.detail),
        "status": b.status, "apply": b.apply_enabled(), "favorite": b.favorite_enabled(),
        "favorites": b.favorites.iter().collect::<Vec<_>>(),
    })
}

fn same_state(tree: &Tree, b: &ProfileBrowser, expected: &Value, at: &str) {
    let mut actual = browser_state(tree, b);
    let mut expected = expected.clone();
    if b.selected().is_some_and(|r| !r.error.is_empty()) {
        for v in [&mut actual, &mut expected] {
            let detail = v["detail"].as_str().unwrap().to_owned();
            let lines: Vec<&str> = detail.lines().take(2).collect();
            v["detail"] = json!(lines.join("\n"));
        }
    }
    same(&actual, &expected, at);
}

/// A step of the generator's dialog script: (name, state).
fn dialog_step<'a>(meta: &'a Value, name: &str) -> &'a Value {
    &meta["dialog"].as_array().unwrap().iter().find(|s| s[0] == name).unwrap_or_else(|| panic!("{name}"))[1]
}

#[test]
fn the_profile_browser_shows_what_the_oracles_dialog_shows() {
    let _turn = korean();
    let meta = common::native_json(&case("raw_profiles"));
    let time = meta["time"].as_i64().unwrap();
    let tree = Tree::new("raw-profile-browser");
    for file in meta["files"].as_array().unwrap() {
        tree.place(file[0].as_str().unwrap(), &file[1], time);
    }
    let changed = meta["changed"].as_array().unwrap();
    tree.place(changed[0].as_str().unwrap(), &changed[1], time + 100);
    std::fs::remove_file(tree.root.join(meta["removed"].as_str().unwrap())).unwrap();
    let extra = tree.root.join("extra").to_string_lossy().into_owned();
    let cache = tree.root.join("dialog-lib").join("cache").join("dcp-index.sqlite");
    let mut folders = vec![extra.clone()];
    let scan = |b: &mut ProfileBrowser, folders: &[String]| {
        let roots = tree.scan_roots(folders);
        assert!(b.begin_scan(&roots));
        let result = profiles::scan(&roots, &cache, &|| false, profiles::LIMIT).unwrap();
        b.ready(&result);
    };

    let mut b = ProfileBrowser::new(meta["metadata"]["test"].clone(), &[]);
    same_state(&tree, &b, dialog_step(&meta, "open"), "open");
    scan(&mut b, &folders);
    same_state(&tree, &b, dialog_step(&meta, "refresh"), "refresh");
    b.select(1);
    same_state(&tree, &b, dialog_step(&meta, "select 1"), "select 1");
    let stored = b.toggle_favorite().unwrap();
    same_state(&tree, &b, dialog_step(&meta, "favorite"), "favorite");
    b.search = "luma".into();
    b.populate();
    same_state(&tree, &b, dialog_step(&meta, "search luma"), "search luma");
    b.search.clear();
    b.populate();
    b.favorites_only = true;
    b.populate();
    same_state(&tree, &b, dialog_step(&meta, "favorites only"), "favorites only");
    b.favorites_only = false;
    b.populate();
    b.select(0);
    let stored_again = b.toggle_favorite().unwrap();
    same_state(&tree, &b, dialog_step(&meta, "unfavorite"), "unfavorite");
    b.search = "zzz".into();
    b.populate();
    same_state(&tree, &b, dialog_step(&meta, "search zzz"), "search zzz");
    b.search.clear();
    b.populate();
    b.select(b.shown.len() - 1);
    same_state(&tree, &b, dialog_step(&meta, "select last"), "select last");
    assert!(!b.apply_enabled());
    same(&Value::Null, &dialog_step(&meta, "choose disabled")["record"], "choose disabled");
    b.select(2);
    let record = profiles::load_selected(b.selected().unwrap(), &b.metadata).unwrap();
    same(&summary(&record), &dialog_step(&meta, "choose")["record"], "choose");
    assert_eq!(stored.len(), 1);
    same(&json!(stored_again), &meta["dialog_favorites"], "stored favourites");

    let mut b = ProfileBrowser::new(meta["metadata"]["synthetic"].clone(), &stored_again);
    scan(&mut b, &folders);
    same_state(&tree, &b, dialog_step(&meta, "synthetic"), "synthetic");
    b.select(0);
    same_state(&tree, &b, dialog_step(&meta, "synthetic select 0"), "synthetic select 0");
    folders.retain(|f| *f != extra);
    scan(&mut b, &folders);
    same_state(&tree, &b, dialog_step(&meta, "remove folder"), "remove folder");
    same(&json!(folders), &meta["folders_after_remove"], "folders after the removal");
    b.status = grainy_app::text::tr("추가한 폴더가 없습니다.");
    same_state(&tree, &b, dialog_step(&meta, "remove folder again"), "remove folder again");
    for _ in 0..2 {
        folders = profiles::with_folder(&folders, &extra);
        scan(&mut b, &folders);
    }
    same_state(&tree, &b, dialog_step(&meta, "add folder"), "add folder");
    let texts: Vec<String> = folders.iter().map(|f| tree.text(f)).collect();
    same(&json!(texts), &meta["folders_after_add"], "folders after adding");
    let record = profiles::load_selected(b.selected().unwrap(), &b.metadata).unwrap();
    same(&summary(&record), &dialog_step(&meta, "synthetic choose")["record"], "synthetic choose");
    println!("profile browser: 16 states equal to the oracle's dialog (exact; refused profiles' error line excepted)");
}

// ---- the RAW import defaults ----------------------------------------------------------------------------------

/// The generator's stand-in window over a catalogue of its rows.
struct TestHost {
    catalog: Catalog,
    current: u64,
    edit: EditState,
    presets: Vec<(String, Value)>,
    commits: Vec<String>,
}

impl TestHost {
    fn commit(&mut self, label: &str) {
        if self.current != 0 && self.edit.commit(label) {
            self.catalog.edit(self.current as i64, self.edit.settings(), label).unwrap();
            self.commits.push(label.to_owned());
        }
    }
}

impl Host for TestHost {
    fn current_id(&self) -> u64 {
        self.current
    }

    fn photo(&self, id: u64) -> Option<Value> {
        self.catalog.photo(id as i64).unwrap()
    }

    fn settings(&self) -> Settings {
        self.edit.settings().clone()
    }

    fn preset(&self, name: &str) -> Option<Value> {
        self.presets.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }

    fn has_source(&self) -> bool {
        self.current != 0
    }

    fn save_rules(&mut self, rules: &Value) -> Result<(), String> {
        self.catalog.save_preference(PREFERENCE, rules).map_err(|e| e.to_string())
    }

    fn apply(&mut self, settings: Settings, label: &str) {
        self.commit("보정 변경");
        self.edit.set_all(normalized(&settings.0));
        self.commit(label);
    }
}

fn defaults_state(model: &RawDefaults, host: &TestHost, kind: &str) -> Value {
    let stored = host.catalog.preference(PREFERENCE, json!([])).unwrap();
    let groups: Map<String, Value> = grainy_raw::raw_defaults::GROUPS
        .iter()
        .zip(&model.groups)
        .map(|((g, _), on)| ((*g).to_owned(), Value::Bool(*on)))
        .collect();
    slim(&json!({
        "rules": model.rules,
        "stored_same": py_eq(&stored, &Value::Array(model.rules.clone())),
        "items": model.items(),
        "scopes": model.scopes.iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>(),
        "scope": model.scope,
        "sources": model.sources.iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>(),
        "source": model.source,
        "groups": groups,
        "status": model.status,
        "apply": model.apply_enabled(),
        "warnings": model.warning.iter().map(|(t, m)| json!([t, m])).collect::<Vec<_>>(),
        "commits": host.commits,
        "settings": if kind == "apply" { Value::Object(host.edit.settings().0.clone()) } else { Value::Null },
    }))
}

#[test]
fn the_raw_defaults_box_does_what_the_oracles_does() {
    let _turn = korean();
    let meta = case("raw_defaults");
    let dir = common::temp_dir("raw-defaults");
    let catalog = Catalog::with_sources(
        &dir,
        Sources::new(
            || "2026-11-13T01:02:03.456789+00:00".to_owned(),
            || "0123456789abcdef0123456789abcdef".to_owned(),
        ),
    )
    .unwrap();
    let root = meta["root"].as_str().unwrap();
    for photo in meta["photos"].as_array().unwrap() {
        let settings = Settings(photo["settings"].as_object().unwrap().clone());
        let path = format!("{root}\\{}", photo["name"].as_str().unwrap());
        catalog.add(&path, Some(&settings), Some(&photo["info"]), true).unwrap();
    }
    catalog.save_preset("Mine", &Settings(meta["preset"].as_object().unwrap().clone())).unwrap();
    let user: Vec<(String, Value)> =
        catalog.presets().unwrap().as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let presets = grainy_app::commands::presets::all_presets(&user);
    let names: Vec<String> = presets.iter().map(|(n, _)| n.clone()).collect();
    let mut host = TestHost { catalog, current: 0, edit: EditState::default(), presets, commits: Vec::new() };
    let mut model: Option<RawDefaults> = None;
    let mut last_rules: Option<Value> = None;
    for (index, (step, expected)) in
        meta["script"].as_array().unwrap().iter().zip(meta["steps"].as_array().unwrap()).enumerate()
    {
        let kind = step[0].as_str().unwrap();
        if let Some(m) = model.as_mut() {
            m.warning = None;
        }
        match kind {
            "open" => {
                host.current = step[1].as_u64().unwrap_or(0);
                host.commits.clear();
                if host.current != 0 {
                    let stored = host.catalog.photo(host.current as i64).unwrap().unwrap()["settings"].clone();
                    host.edit = EditState::new(Settings(stored.as_object().unwrap().clone()));
                    let mut live = host.edit.settings().clone();
                    for (key, value) in step[2].as_object().cloned().unwrap_or_default() {
                        let value = if key == "dcp_profile" {
                            dcp::load_profile(fixture(value.as_str().unwrap())).unwrap()
                        } else {
                            value
                        };
                        live.set(&key, value);
                    }
                    host.edit.set_all(live);
                }
                // `RawDefaultsDialog.__init__`: `w.commit()` first.
                host.commit("보정 변경");
                let rules = host.catalog.preference(PREFERENCE, json!([])).unwrap().as_array().cloned().unwrap();
                let ids: Vec<u64> = step[3].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
                let ids = if ids.is_empty() && host.current != 0 { vec![host.current] } else { ids };
                model = Some(RawDefaults::new(&host, rules, ids, &names));
            }
            "set" => {
                let m = model.as_mut().unwrap();
                let values = &step[1];
                if let Some(scope) = values.get("scope") {
                    m.scope = m.scopes.iter().position(|(_, v)| Some(*v) == scope.as_str()).unwrap();
                }
                if let Some(source) = values.get("source") {
                    m.source = match source.as_str() {
                        None => 0,
                        Some(name) => m.sources.iter().position(|(_, n)| n.as_deref() == Some(name)).unwrap(),
                    };
                }
                if let Some(name) = values.get("name") {
                    m.name = name.as_str().unwrap().to_owned();
                }
                for (group, on) in values.get("groups").and_then(Value::as_object).cloned().unwrap_or_default() {
                    let i = grainy_raw::raw_defaults::GROUPS.iter().position(|(g, _)| *g == group).unwrap();
                    m.groups[i] = on.as_bool().unwrap();
                }
                if let Some(iso) = values.get("iso") {
                    m.iso = iso.as_bool().unwrap();
                }
            }
            "select" => {
                model.as_mut().unwrap().photo_ids =
                    step[1].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
            }
            "save" => model.as_mut().unwrap().save_rule(&mut host),
            "row" => model.as_mut().unwrap().current = step[1].as_u64().map(|v| v as usize),
            "toggle" => model.as_mut().unwrap().toggle(&mut host),
            "remove" => model.as_mut().unwrap().remove(&mut host),
            "apply" => model.as_mut().unwrap().apply_current(&mut host),
            other => panic!("unknown step {other}"),
        }
        let mut state = defaults_state(model.as_ref().unwrap(), &host, kind);
        if index > 0 && last_rules.as_ref() == Some(&state["rules"]) {
            state["rules"] = Value::Null;
        } else {
            last_rules = Some(state["rules"].clone());
        }
        let at = format!("step {index} {step}");
        for key in [
            "rules",
            "stored_same",
            "items",
            "scopes",
            "scope",
            "sources",
            "source",
            "groups",
            "status",
            "apply",
            "warnings",
            "commits",
            "settings",
        ] {
            same(&state[key], &expected[key], &format!("{at} {key}"));
        }
    }
    println!("RAW defaults: {} steps equal to the oracle's box (exact)", meta["steps"].as_array().unwrap().len());
}

// ---- the calibration panel's methods ----------------------------------------------------------------------------

fn camera_info() -> CameraInfo {
    let mut settings = normalized(&Map::new());
    settings.set("raw_mode", "as_shot");
    let loaded = grainy_engine::files::load_photo(dng(), Some(64), &settings).expect("the DNG");
    serde_json::from_value(loaded.metadata["raw_info"].clone()).expect("the calibration")
}

fn round_to(v: f64, scale: f64) -> f64 {
    (v * scale).round_ties_even() / scale
}

#[test]
fn the_calibration_methods_do_what_the_oracles_do() {
    let _turn = korean();
    let meta = case("raw_studio");
    let dir = common::temp_dir("raw-studio");
    for matrix in meta["matrices"].as_array().unwrap() {
        std::fs::write(dir.join(matrix[0].as_str().unwrap()), serde_json::to_string(&matrix[1]).unwrap()).unwrap();
    }
    let info = camera_info();
    let photos: [(PathBuf, Value); 2] =
        [(dng(), meta["info"].clone()), (PathBuf::from(r"Z:\GrainyGolden\raw\x.jpg"), json!({"format": "JPEG"}))];
    let mut edit = EditState::new(normalized(&Map::new()));
    let mut largest = 0.0f64;
    for (index, (step, expected)) in
        meta["script"].as_array().unwrap().iter().zip(meta["steps"].as_array().unwrap()).enumerate()
    {
        let at = format!("step {index} {step}");
        let mut warnings: Vec<Value> = Vec::new();
        let mut shown = Value::Null;
        match step[0].as_str().unwrap() {
            "working" => raw::change_working(&mut edit, step[1].as_str().unwrap()),
            "dcp" => {
                let record = dcp::load_profile(fixture(step[1][0].as_str().unwrap())).unwrap();
                let (path, photo_info) = &photos[step[1][1].as_u64().unwrap() as usize - 1];
                if let Err(message) = raw::apply_dcp_record(&mut edit, path, photo_info, record) {
                    warnings.push(json!(["DCP 프로파일", message]));
                }
            }
            "display" => {
                let settings = edit.settings().clone();
                if let Some((kelvin, tint)) =
                    raw::display_raw_white(&settings, raw::source_info(&settings, Some(&info)))
                {
                    shown = json!([round_to(kelvin, 1.0), round_to(tint, 10.0)]);
                }
            }
            "mode" => {
                let settings = edit.settings().clone();
                raw::change_raw_mode(&mut edit, step[1].as_str().unwrap(), raw::source_info(&settings, Some(&info)));
            }
            "temperature" => {
                let kelvin = step[1][0].as_f64().unwrap();
                let tint = (step[1][1].as_f64().unwrap() * 10.0).round() / 10.0;
                raw::raw_temperature_changed(&mut edit, kelvin, tint);
                edit.commit("보정 변경");
            }
            "neutral" => {
                edit.set("raw_neutral", step[1].clone());
                edit.commit("보정 변경");
            }
            "clear" => raw::clear_dcp(&mut edit),
            "matrix" => {
                let text = std::fs::read_to_string(dir.join(step[1].as_str().unwrap())).unwrap();
                match raw::camera_matrix_from(&text) {
                    Ok(matrix) => {
                        edit.set("camera_matrix", matrix);
                        edit.commit("보정 변경");
                    }
                    Err(message) => warnings.push(json!(["프로파일", message])),
                }
            }
            other => panic!("unknown step {other}"),
        }
        let s = edit.settings();
        for key in ["working_space", "raw_mode", "raw_neutral", "camera_matrix"] {
            same(s.value(key), &expected["values"][key], &format!("{at} {key}"));
        }
        for key in ["raw_kelvin", "raw_tint"] {
            let (a, b) = (s.f64(key), expected["values"][key].as_f64().unwrap());
            let error = (a - b).abs() / b.abs().max(1.0);
            largest = largest.max(error);
            assert!(error < 1e-6, "{at} {key}: {a} against {b}");
        }
        same(&summary(s.value("dcp_profile")), &expected["dcp"], &format!("{at} dcp"));
        let mut labels: Vec<String> = edit.history().to_vec();
        labels.reverse();
        same(&json!(labels), &expected["commits"], &format!("{at} commits"));
        same(&json!(warnings), &expected["warnings"], &format!("{at} warnings"));
        same(&shown, &expected["shown"], &format!("{at} shown"));
    }
    println!("calibration methods: every step equal; RAW temperature and tint largest relative difference {largest:e} (tolerance 1e-6)");
}

#[test]
fn the_matrix_file_is_read_as_float32() {
    let matrix = raw::camera_matrix_from(r#"{"matrix": [[0.1, 0, 0], [0, 1, 0], [0, 0, 1]]}"#).unwrap();
    assert_eq!(matrix[0][0], json!(0.10000000149011612));
    assert_eq!(raw::camera_matrix_from("{}").unwrap_err(), "'matrix'");
    assert!(raw::camera_matrix_from("not json").is_err());
}

// ---- the window ------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    common::snapshot::save(harness, name);
}

/// The window on a new library with the generated DNG open; the profile folders are the test's own.
fn raw_harness<'a>(tag: &str) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = common::temp_dir(&format!("raw-window-{tag}"));
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let photo = photos.join("test camera.dng");
    std::fs::copy(dng(), &photo).unwrap();
    let profiles = dir.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::copy(fixture("test-camera"), profiles.join("Luma Test.dcp")).unwrap();
    std::fs::copy(fixture("test-triple-illuminant"), profiles.join("triple.dcp")).unwrap();
    std::fs::copy(fixture("synthetic_dual"), profiles.join("other camera.dcp")).unwrap();
    let library = dir.join("Library");
    let data = dir.join("data");
    let own = profiles.clone();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        let environment = app.cx.state::<ProfileEnvironment>();
        environment.env = Some(BTreeMap::new());
        environment.shown_as = Some((own.to_string_lossy().into_owned(), r"D:\Profiles".to_owned()));
        environment.own = Some(own);
        app.open_paths(vec![photo]);
        app
    });
    settle(&mut harness);
    (harness, dir)
}

/// The panel, the profile browser and the RAW defaults box on a RAW photo, end to end through the commands.
fn window_flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let (mut harness, _dir) = raw_harness(tag);
    assert!(harness.state().cx.doc.current().is_some(), "the DNG is open");

    // The camera profile browser lists the profiles of this camera; applying one pins it.
    harness.state_mut().cx.run("raw.browse_dcp");
    settle(&mut harness);
    assert!(harness.state().dialog_open("camera_profiles"));
    save(&mut harness, &format!("raw-profiles-{tag}"));
    harness.state_mut().close_dialog("camera_profiles");
    let record = dcp::load_profile(fixture("test-camera")).unwrap();
    raw::apply_record(&mut harness.state_mut().cx, record, "DCP 프로파일");
    settle(&mut harness);
    assert_eq!(harness.state().settings().str("raw_mode"), "as_shot");
    assert_eq!(harness.state().edit_state().history()[0], "DCP 프로파일");

    // The calibration panel with its RAW fold-out.
    harness.state_mut().open_panel("calibration");
    harness.state_mut().cx.open_foldout("raw_profile");
    settle(&mut harness);
    save(&mut harness, &format!("raw-calibration-{tag}"));

    // The RAW import defaults: a rule for this camera from the open photo.
    harness.state_mut().cx.run("raw.defaults");
    settle(&mut harness);
    assert!(harness.state().dialog_open("raw_defaults"));
    save(&mut harness, &format!("raw-defaults-{tag}"));
    harness.state_mut().close_dialog("raw_defaults");

    // Clearing the profile.
    harness.state_mut().cx.run("raw.clear_dcp");
    settle(&mut harness);
    assert!(!grainy_core::settings::truthy(harness.state().settings().value("dcp_profile")));
    assert_eq!(harness.state().edit_state().history()[0], "DCP 해제");
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_window_shows_the_raw_controls_korean() {
    window_flow(Language::Korean, "ko");
}

#[test]
fn the_window_shows_the_raw_controls_english() {
    window_flow(Language::English, "en");
}
