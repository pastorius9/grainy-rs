//! Work package 24, import copy, watched folders and folder sync, against the oracle (`golden/app/import_*`, built
//! by `tools/golden/gen_app_import.py` with the Python Grainy's own `import_copy.dialog`, its job, `import_paths`,
//! `import_next`, `apply_import_extras`, `FolderSync.scan` / `found` and `ExtraTools.watch` / `watch_scan` /
//! `unwatch`):
//!
//! * a scripted session through the dialog's model and the commands after which the catalogue tables, every file
//!   on disk, the dialog's fields, the problems shown, the excluded keys and the watched folders are the oracle's;
//!   the card's files are byte for byte what they were, and nothing was overwritten;
//! * a cancelled copy stops between photos, leaves no partial file and imports what was copied;
//! * the automatic import's timers (first check after 4 s, then every 120 s, on returning to the window) on an
//!   injected clock, its switch, and the watched folders' timer, without waiting;
//! * the window: the copy dialog (snapshots in Korean and English).
//!
//! Every test works in temporary folders on generated PNG files.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::import::{self, Copied, CopyRun, ImportRun};
use grainy_app::dialogs::import_copy::{CopyShared, Form, ImportCopyDialog};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::TestCard;
use grainy_app::{folder_watch, AppContext, GrainyApp};
use grainy_catalog::folders::Platform;
use grainy_catalog::{Catalog, Sources};
use grainy_core::normalized;
use grainy_i18n::Language;
use grainy_library::pathx;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

// ---- the files of the script ----------------------------------------------------------------------------------

struct Env {
    root: PathBuf,
    root_text: String,
    inputs: BTreeMap<String, Vec<u8>>,
    old: f64,
}

impl Env {
    fn rel(&self, text: &str) -> String {
        text.replace(&self.root_text, "<ROOT>").replace(&self.root_text.to_lowercase(), "<root>")
    }

    fn full(&self, text: &str) -> String {
        text.replace("<ROOT>", &self.root_text)
    }

    fn rel_value(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.rel(s)),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.rel_value(v)).collect()),
            Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (self.rel(k), self.rel_value(v))).collect()),
            other => other.clone(),
        }
    }

    /// Writes `input` at `relative` with the time OLD (as the generator's `os.utime`).
    fn write(&self, relative: &str, input: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &self.inputs[input]).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs_f64(self.old)).unwrap();
    }
}

fn inputs() -> BTreeMap<String, Vec<u8>> {
    let case = Case::open("app", "import_inputs");
    let names: Vec<String> =
        case.json()["names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    names
        .into_iter()
        .map(|name| {
            let bytes: ndarray::Array1<u8> = case.array(&name);
            (name, bytes.to_vec())
        })
        .collect()
}

/// The generator's `build`: the card, the folders, the library with its one photo and the preset.
fn build(name: &str, golden: &Value) -> (Env, AppContext) {
    let dir = common::temp_dir(name);
    let root_text = pathx::resolve(Platform::native(), &pathx::text(&dir));
    let env =
        Env { root: PathBuf::from(&root_text), root_text, inputs: inputs(), old: golden["old"].as_f64().unwrap() };
    for pair in golden["files"].as_array().unwrap() {
        env.write(pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
    }
    let stamp = golden["stamp"].as_str().unwrap().to_owned();
    let catalog =
        Catalog::with_sources(env.root.join("library"), Sources::new(move || stamp.clone(), || "token".to_owned()))
            .unwrap();
    for pair in golden["existing"].as_array().unwrap() {
        let path = pathx::text(&env.root.join(pair[0].as_str().unwrap()));
        catalog.add(&path, Some(&normalized(&Map::new())), Some(&pair[1]), false).unwrap();
    }
    let preset = &golden["preset"];
    catalog.save_preset(preset[0].as_str().unwrap(), &common::settings(preset[1].clone())).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    cx.state::<folder_watch::WatchState>().automatic_off = true;
    (env, cx)
}

/// Runs the finished jobs until none is running; returns the status line after each.
fn drain(cx: &mut AppContext) -> Vec<String> {
    let started = Instant::now();
    let mut statuses = Vec::new();
    loop {
        for finished in cx.jobs.poll() {
            finished(cx);
            statuses.push(cx.status().to_owned());
        }
        if cx.jobs.jobs_idle() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(120), "a job did not finish");
    }
    statuses
}

// ---- what is compared -------------------------------------------------------------------------------------------

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-9,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn tables(env: &Env, cx: &AppContext) -> Value {
    let catalog = cx.doc.metadata_catalog().unwrap();
    let defaults = normalized(&Map::new());
    let mut photos = Vec::new();
    for row in
        catalog.rows("SELECT id,path,name,keywords,rating,virtual_source,metadata FROM photos ORDER BY id", []).unwrap()
    {
        let id = row["id"].as_i64().unwrap();
        let record = catalog.photo(id).unwrap().unwrap();
        let changed: Map<String, Value> = record["settings"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, v)| !same(v, defaults.value(k)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let meta: Value = serde_json::from_str(row["metadata"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
        photos.push(json!({
            "id": id, "path": env.rel(row["path"].as_str().unwrap()), "name": row["name"], "keywords": row["keywords"],
            "rating": row["rating"], "virtual_source": row["virtual_source"], "settings": changed,
            "date": meta.get("date").cloned().unwrap_or(Value::Null),
            "size_mb": meta.get("size_mb").cloned().unwrap_or(Value::Null),
        }));
    }
    let roots: Vec<String> = catalog.folder_roots().unwrap().iter().map(|r| env.rel(r)).collect();
    let mut prefs = Map::new();
    for key in ["import_copy", "watch_folders", "sync_excluded", "removed_folders", "auto_sync_folders"] {
        prefs.insert(key.into(), env.rel_value(&catalog.preference(key, Value::Null).unwrap()));
    }
    let history: Vec<Value> = catalog
        .rows("SELECT id,photo_id,label FROM history ORDER BY id", [])
        .unwrap()
        .into_iter()
        .map(|r| json!([r["id"], r["photo_id"], r["label"]]))
        .collect();
    json!({"photos": photos, "folder_roots": roots, "preferences": prefs, "history": history})
}

/// Every file below the root but the library's, with the input its bytes are.
fn tree(env: &Env) -> Vec<Value> {
    fn walk(env: &Env, folder: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path != env.root.join("library") {
                    walk(env, &path, out);
                }
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let name = env.inputs.iter().find(|(_, b)| **b == bytes).map_or("other", |(n, _)| n.as_str());
            let relative = path.strip_prefix(&env.root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((relative, name.to_owned()));
        }
    }
    let mut out = Vec::new();
    walk(env, &env.root, &mut out);
    out.sort_by_key(|(p, _)| p.to_lowercase());
    out.into_iter().map(|(p, n)| json!([p, n])).collect()
}

fn card_bytes(env: &Env) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (path, _) in tree(env).iter().map(|v| (v[0].as_str().unwrap().to_owned(), ())) {
        if path.starts_with("card/") {
            out.push((path.clone(), std::fs::read(env.root.join(&path)).unwrap()));
        }
    }
    out
}

fn form_of(env: &Env, a: &Value) -> Form {
    let s = |k: &str| a[k].as_str().unwrap().to_owned();
    Form {
        source: env.full(&s("source")),
        destination: env.full(&s("destination")),
        subfolders: s("subfolders"),
        template: s("template"),
        use_backup: a["use_backup"].as_bool().unwrap(),
        backup: env.full(&s("backup")),
        keywords: s("keywords"),
        preset: s("preset"),
        skip: a["skip"].as_bool().unwrap(),
    }
}

fn opened(env: &Env, form: &Form) -> Value {
    json!({
        "source": env.rel(&form.source), "destination": env.rel(&form.destination), "template": form.template,
        "backup": env.rel(&form.backup), "keywords": form.keywords, "subfolders": form.subfolders,
        "preset": form.preset, "use_backup": form.use_backup, "skip": form.skip,
    })
}

// ---- the session against the oracle -------------------------------------------------------------------------

#[test]
fn copy_import_watch_and_sync_equal_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut golden = Case::open("app", "import_steps").json().clone();
    // The steps' backslashes are the separators of `<ROOT>\...` paths: on macOS the oracle writes `/`.
    golden["steps"] = common::native_json(&golden["steps"]);
    let (env, mut cx) = build("import-steps", &golden);
    assert!(folder_watch::ensure(&mut cx));
    let card = card_bytes(&env);
    assert_eq!(card.len(), 7);
    for (index, expected) in golden["steps"].as_array().unwrap().iter().enumerate() {
        let step = expected["step"].as_array().unwrap();
        let action = step[0].as_str().unwrap();
        let what = format!("step {index}: {}", serde_json::to_string(step).unwrap());
        let mut problems = Vec::new();
        let mut statuses = Vec::new();
        match action {
            "copy" => {
                // The fields the dialog opens with: the saved preference.
                let saved = cx.doc.preference("import_copy").unwrap_or(Value::Null);
                let form = Form::from_saved(&saved);
                assert_eq!(opened(&env, &form), expected["opened"][0], "{what}: the dialog's fields");
                for attempt in step[1].as_array().unwrap() {
                    if !attempt["accept"].as_bool().unwrap() {
                        continue;
                    }
                    match import::submit(&mut cx, &form_of(&env, attempt)) {
                        Ok(()) => statuses = drain(&mut cx),
                        Err(problem) => problems.push(problem),
                    }
                }
            }
            "remove" => {
                cx.doc.refresh().unwrap();
                let name = step[1].as_str().unwrap();
                let index = cx.doc.photos().iter().position(|p| p.path.file_name().unwrap() == name).unwrap();
                cx.doc.select(index);
                cx.doc.remove_selected().unwrap();
            }
            "write" => env.write(step[1].as_str().unwrap(), step[2].as_str().unwrap()),
            "sync" => {
                folder_watch::scan_now(&mut cx);
                statuses = drain(&mut cx);
            }
            "folder" => {
                let mut query = cx.doc.query();
                query.folder = step[1].as_str().map(|f| env.full(f));
                cx.doc.set_query(query).unwrap();
            }
            "watch" => {
                match step[1].as_str() {
                    // The folder dialog's answer.
                    Some(folder) => import::add_watch(&mut cx, &env.full(folder)),
                    None => assert!(cx.run("import.watch"), "{what}"),
                }
                statuses = drain(&mut cx);
            }
            "watch_scan" => {
                import::watch_scan(&mut cx);
                statuses = drain(&mut cx);
            }
            "unwatch" => assert!(cx.run("import.unwatch")),
            other => panic!("{other}"),
        }
        let boxes: Vec<Value> = expected["boxes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b.get("text").is_some())
            .map(|b| b["text"].clone())
            .collect();
        assert_eq!(json!(problems), json!(boxes), "{what}: the problems");
        if let Some(summary) =
            expected["messages"].as_array().unwrap().iter().filter_map(Value::as_str).find(|m| m.contains("장 복사"))
        {
            assert!(statuses.iter().any(|s| s == summary), "{what}: {summary} in {statuses:?}");
        }
        if let Some(found) = expected["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .find(|m| m.starts_with("등록한 폴더에서"))
        {
            assert!(statuses.iter().any(|s| s == found) || cx.status() == found, "{what}: {found} in {statuses:?}");
        }
        let now = tables(&env, &cx);
        assert!(same(&now, &expected["tables"]), "{what}: tables\n{now:#}\nexpected\n{:#}", expected["tables"]);
        assert_eq!(json!(tree(&env)), expected["files"], "{what}: files");
        let (excluded, auto) = folder_watch::keys(&mut cx);
        let rel = |list: Vec<String>| json!(list.iter().map(|k| env.rel(k)).collect::<Vec<_>>());
        assert_eq!(rel(excluded), expected["excluded"], "{what}: excluded");
        assert_eq!(rel(auto), expected["auto"], "{what}: auto");
        assert_eq!(rel(import::watch_paths(&cx)), expected["watch_paths"], "{what}: watched folders");
    }
    // The card was only read.
    assert_eq!(card_bytes(&env), card, "the sources are unchanged");
    assert!(!folder_watch::watch_timer_running(&mut cx), "자동 가져오기 해제 stops the timer");
}

// ---- cancelling ------------------------------------------------------------------------------------------------

/// A copy cancelled after its first photo: the rest is not copied, no `.grainy-part` file stays, the card is
/// unchanged, and the photo copied is imported (the oracle imports what `execute` copied before it stopped).
#[test]
fn a_cancelled_copy_stops_between_photos_and_imports_what_was_copied() {
    let golden = Case::open("app", "import_steps").json().clone();
    let (env, mut cx) = build("import-cancel", &golden);
    // More photos on the card than the five of the golden, so that the copy is far from its end when the cancel
    // comes, however fast the machine copies.
    const EXTRA: usize = 60;
    for n in 0..EXTRA {
        env.write(&format!("card/DCIM/103/EXTRA_{n:03}.png"), "one");
    }
    let total = 5 + EXTRA;
    let card = card_bytes(&env);
    let photos = env.root.join("photos");
    let copies_in = |folder: &Path| -> usize {
        std::fs::read_dir(folder).map_or(0, |entries| {
            entries.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".png")).count()
        })
    };
    let before = copies_in(&photos);
    let run = CopyRun {
        source: env.full(&common::native(r"<ROOT>\card")),
        destination: env.full(&common::native(r"<ROOT>\photos")),
        backup: None,
        subfolders: "none".into(),
        template: "{stem}".into(),
        skip: false,
    };
    let library = env.root.join("library");
    let shared = CopyShared::default();
    let watcher = shared.clone();
    let id = cx.jobs.spawn(
        import::COPY_KIND,
        "",
        move |handle| {
            // The box's 취소 as soon as the first photo is copied. The copy reports each photo in the shared state:
            // once it has started (its total is known), the state is held until the first copy is on disk, so the
            // copy cannot go past its first or second photo before the cancel is seen, whatever the timing.
            let flag = handle.clone();
            let stop = std::thread::spawn(move || {
                let started = Instant::now();
                let waited = || assert!(started.elapsed() < Duration::from_secs(60), "the copy did not start");
                while watcher.get().total == 0 {
                    waited();
                    std::thread::yield_now();
                }
                watcher.update(|_| {
                    while copies_in(&photos) == before {
                        waited();
                        std::thread::yield_now();
                    }
                    flag.cancel_flag().store(true, Ordering::Relaxed);
                });
            });
            let result = run.run(&library, handle, &shared);
            stop.join().unwrap();
            result
        },
        move |cx: &mut AppContext, result: Result<Copied, String>| {
            let copied = result.unwrap();
            assert!(!copied.copied.is_empty() && copied.copied.len() < total, "{copied:?}");
            assert!(copied.errors.is_empty(), "{copied:?}");
            import::start_import(
                cx,
                ImportRun::copies(copied.copied.iter().map(PathBuf::from).collect(), BTreeMap::new()),
            );
        },
    );
    assert!(id > 0);
    drain(&mut cx);
    let files: Vec<String> = tree(&env).iter().map(|v| v[0].as_str().unwrap().to_owned()).collect();
    assert!(!files.iter().any(|f| f.ends_with(".grainy-part")), "{files:?}");
    let copies: Vec<&String> = files.iter().filter(|f| f.starts_with("photos/") && f.ends_with(".png")).collect();
    // photos/2026/2026-09-25/IMG_0002.png was there before; the copies made before the stop are beside it.
    let made = copies.len() - 1;
    assert!((1..total).contains(&made), "{copies:?}");
    assert_eq!(card_bytes(&env), card);
    let catalog = cx.doc.metadata_catalog().unwrap();
    assert_eq!(catalog.paths().unwrap().len(), 1 + made, "the existing photo and the copies");
}

/// The copy dialog refuses to open while an import runs (`진행 중인 가져오기가 끝난 뒤 다시 시도하세요.`), and the
/// dialog's checks run before anything is saved or copied.
#[test]
fn the_dialog_waits_for_a_running_import() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let golden = Case::open("app", "import_steps").json().clone();
    let (env, mut cx) = build("import-busy", &golden);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    cx.jobs.spawn(
        "import",
        "",
        move |_| Ok(rx.recv_timeout(Duration::from_secs(60)).is_ok()),
        |_, _: Result<bool, String>| {},
    );
    assert!(cx.run("import.copy"));
    assert_eq!(cx.status(), "진행 중인 가져오기가 끝난 뒤 다시 시도하세요.");
    assert!(!cx.dialog_open("import_copy"));
    tx.send(()).unwrap();
    drain(&mut cx);
    assert!(cx.run("import.copy"));
    assert!(cx.dialog_open("import_copy"));
    // A problem: nothing saved.
    let photos = env.full(&common::native(r"<ROOT>\photos"));
    let form =
        Form { source: env.full(&common::native(r"<ROOT>\nowhere")), destination: photos.clone(), ..Form::default() };
    assert_eq!(import::submit(&mut cx, &form), Err("가져올 폴더를 선택하세요.".to_owned()));
    let empty = Form { source: String::new(), destination: photos, ..Form::default() };
    assert_eq!(
        empty.problem().as_deref(),
        Some("가져올 폴더를 선택하세요."),
        "an empty source is not the current folder"
    );
    assert!(cx.doc.preference("import_copy").is_none());
}

// ---- the timers --------------------------------------------------------------------------------------------------

/// `FolderSync`'s timers on an injected clock: nothing before 4 s, a check at 4 s, the next one 120 s after
/// creation; turning the switch off stops them, turning it on checks at once. A new photo in a registered folder
/// is imported by the check; nothing is ever written to the files.
#[test]
fn the_automatic_import_follows_its_timers() {
    let golden = Case::open("app", "import_steps").json().clone();
    let (env, mut cx) = build("import-timers", &golden);
    let millis = Arc::new(AtomicU64::new(0));
    let clock = millis.clone();
    folder_watch::set_clock(&mut cx, move || clock.load(Ordering::Relaxed) as f64 / 1000.0);
    cx.state::<folder_watch::WatchState>().automatic_off = false;
    let scans = |cx: &mut AppContext, at: u64| {
        millis.store(at, Ordering::Relaxed);
        folder_watch::poll(cx);
        let started = cx.jobs.running(folder_watch::SCAN_KIND);
        drain(cx);
        started
    };
    assert!(!scans(&mut cx, 0));
    assert!(!scans(&mut cx, 3_900));
    // A new photo in the registered folder of the existing one.
    env.write("lib/existing/new.png", "late");
    let before = std::fs::read(env.root.join("lib/existing/new.png")).unwrap();
    assert!(scans(&mut cx, 4_000), "the first check after 4 s");
    let catalog = cx.doc.metadata_catalog().unwrap();
    assert!(catalog.paths().unwrap().iter().any(|p| p.ends_with("new.png")), "imported by the check");
    assert_eq!(std::fs::read(env.root.join("lib/existing/new.png")).unwrap(), before);
    assert!(!scans(&mut cx, 60_000));
    assert!(scans(&mut cx, 120_000), "every 120 s");
    assert!(!scans(&mut cx, 200_000));
    // The switch: off, then nothing at the next interval; on checks at once.
    assert!(folder_watch::sync_enabled(&cx));
    assert!(cx.run("import.folder_sync"));
    assert!(!folder_watch::sync_enabled(&cx));
    assert_eq!(cx.doc.preference("auto_sync_folders"), Some(json!(false)));
    assert!(!scans(&mut cx, 240_000));
    assert!(cx.run("import.folder_sync"));
    assert!(cx.jobs.running(folder_watch::SCAN_KIND), "turning it on checks at once");
    drain(&mut cx);
}

/// The watched folders' 20 s timer (`ExtraTools.timer`): it runs while folders are watched, reads them with
/// `restore=False` (a folder removed from the catalogue below a watched one stays out), and stops with 자동
/// 가져오기 해제.
#[test]
fn watched_folders_are_read_on_their_timer() {
    let golden = Case::open("app", "import_steps").json().clone();
    let (env, mut cx) = build("import-watch-timer", &golden);
    cx.state::<folder_watch::WatchState>().automatic_off = false;
    cx.state::<folder_watch::WatchState>().watch_interval = Some(Duration::from_millis(30));
    folder_watch::set_sync_enabled(&mut cx, false);
    drain(&mut cx);
    import::add_watch(&mut cx, &env.full(&common::native(r"<ROOT>\inbox")));
    drain(&mut cx);
    let count = |cx: &AppContext| cx.doc.metadata_catalog().unwrap().paths().unwrap().len();
    assert_eq!(count(&cx), 3, "in1 and in2 joined the existing photo");
    // The subfolder leaves the catalogue; a new photo appears there and one at the top.
    cx.doc.remove_folder(&env.full(&common::native(r"<ROOT>\inbox\sub"))).unwrap();
    assert_eq!(count(&cx), 2);
    env.write("inbox/sub/in3.png", "one");
    env.write("inbox/in4.png", "five");
    std::thread::sleep(Duration::from_millis(40));
    folder_watch::poll(&mut cx);
    assert!(cx.jobs.running("import"), "the timer read the watched folder");
    drain(&mut cx);
    let paths = cx.doc.metadata_catalog().unwrap().paths().unwrap();
    assert!(paths.iter().any(|p| p.ends_with("in4.png")));
    assert!(!paths.iter().any(|p| p.contains(&common::native(r"\sub\"))), "the removed folder stays out: {paths:?}");
    assert!(cx.run("import.unwatch"));
    std::thread::sleep(Duration::from_millis(40));
    folder_watch::poll(&mut cx);
    assert!(!cx.jobs.running("import"));
}

// ---- the window ---------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn dialog(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        let form = Form {
            source: r"E:\DCIM".into(),
            destination: r"D:\Photos".into(),
            subfolders: "year/date".into(),
            template: "{date}_{n:04d}".into(),
            use_backup: true,
            backup: r"F:\Backup".into(),
            keywords: "trip, card".into(),
            preset: String::new(),
            skip: true,
        };
        app.cx.open_dialog(Box::new(ImportCopyDialog::new(form)));
        app
    });
    harness.run_steps(4);
    common::snapshot::save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn copy_dialog_korean() {
    dialog(Language::Korean, "import-copy-dialog-ko");
}

#[test]
fn copy_dialog_english() {
    dialog(Language::English, "import-copy-dialog-en");
}
