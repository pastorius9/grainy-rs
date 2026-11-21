//! Work package 22, file management and catalogue maintenance, against the oracle (`golden/app/files_*`, built by
//! `tools/golden/gen_app_files.py` with the Python Grainy's own `LibraryManager.rename`, `move`,
//! `move_selected_folder`, `remove(True)`, `relink`, `duplicates`, `backup`, `restore`, `integrity`,
//! `relink_folder` and `FolderPanel.locate` / `apply_many`):
//!
//! * a scripted session through the commands after which the catalogue tables, the files on disk, the backups,
//!   the boxes' texts and the errors are the oracle's, and no original's bytes ever changed;
//! * the recycle bin is a fake here (files move into the test's own `bin` folder): the system's is never used;
//! * offline packets (`smart_previews`) equal the oracle's, and a photo whose original is gone is developed from
//!   its packet;
//! * a folder renamed on disk is relinked by the folder scan (`FolderPanel.refresh`'s moves), and one moved
//!   elsewhere by the automatic search (`recover`);
//! * the window: the duplicates window, the maintenance window and the relink question (snapshots in Korean and
//!   English).
//!
//! Every test works in temporary folders on generated PNG files.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::files::{self as files, FilesState, Trash};
use grainy_app::commands::Invocation;
use grainy_app::dialogs::duplicates::DuplicatesDialog;
use grainy_app::dialogs::maintenance::{MaintenanceDialog, Shared, Update};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::TestCard;
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::folders::Platform;
use grainy_catalog::{Catalog, Sources};
use grainy_core::normalized;
use grainy_i18n::Language;
use grainy_library::pathx;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

// ---- the fake recycle bin -----------------------------------------------------------------------------------

/// Moves a file into `<root>/bin` under a numbered name (numbered from 1 in each step, as the generator does).
#[derive(Clone)]
struct FakeBin {
    root: PathBuf,
    trashed: Arc<Mutex<Vec<String>>>,
}

impl Trash for FakeBin {
    fn trash(&self, path: &Path) -> Result<(), String> {
        assert!(path.starts_with(&self.root), "the fake bin only takes files of the test folder: {}", path.display());
        let mut list = self.trashed.lock().unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        let target = self.root.join("bin").join(format!("{:02}-{name}", list.len() + 1));
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::rename(path, &target).map_err(|e| e.to_string())?;
        list.push(pathx::text(path));
        Ok(())
    }
}

// ---- the library of the script ------------------------------------------------------------------------------

struct Env {
    root: PathBuf,
    root_text: String,
    inputs: BTreeMap<String, Vec<u8>>,
}

impl Env {
    fn rel(&self, text: &str) -> String {
        text.replace(&self.root_text, "<ROOT>")
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
}

fn inputs() -> BTreeMap<String, Vec<u8>> {
    let case = Case::open("app", "files_inputs");
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

/// The generator's `build`: the files, and the library with its six photos, a virtual copy, an edit, a rating.
fn build(name: &str, golden: &Value) -> (Env, AppContext, Arc<Mutex<Vec<String>>>) {
    let dir = common::temp_dir(name);
    let root_text = pathx::resolve(Platform::native(), &pathx::text(&dir));
    let env = Env { root: PathBuf::from(&root_text), root_text, inputs: inputs() };
    for pair in golden["files"].as_array().unwrap() {
        let path = env.root.join(pair[0].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &env.inputs[pair[1].as_str().unwrap()]).unwrap();
    }
    std::fs::create_dir_all(env.root.join("elsewhere")).unwrap();
    let stamp = golden["stamp"].as_str().unwrap().to_owned();
    let catalog =
        Catalog::with_sources(env.root.join("library"), Sources::new(move || stamp.clone(), || "token".to_owned()))
            .unwrap();
    for path in golden["library"].as_array().unwrap() {
        let path = pathx::text(&env.root.join(path.as_str().unwrap()));
        catalog.add(&path, Some(&normalized(&Map::new())), Some(&json!({"width": 1, "height": 1})), false).unwrap();
    }
    catalog.virtual_copy(1).unwrap();
    catalog.edit(2, &common::settings(json!({"exposure": 0.5})), "노출").unwrap();
    catalog.update_many(&[4], "rating", &json!(3)).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    let trashed = Arc::new(Mutex::new(Vec::new()));
    files::set_trash(&mut cx, Arc::new(FakeBin { root: env.root.clone(), trashed: trashed.clone() }));
    (env, cx, trashed)
}

/// The catalogue tables as the generator reads them.
fn tables(env: &Env, cx: &AppContext) -> Value {
    let catalog = cx.doc.metadata_catalog().unwrap();
    let photos: Vec<Value> = catalog
        .rows("SELECT id,path,name,virtual_source,rating,imported FROM photos ORDER BY id", [])
        .unwrap()
        .into_iter()
        .map(|mut r| {
            r["path"] = json!(env.rel(r["path"].as_str().unwrap()));
            r
        })
        .collect();
    let mut roots: Vec<String> = catalog.folder_roots().unwrap().iter().map(|r| env.rel(r)).collect();
    roots.sort();
    let mut prefs = Map::new();
    for key in ["removed_folders", "folder_panel", "watch_folders", "folder_identities", "sync_excluded"] {
        let value = catalog.preference(key, Value::Null).unwrap();
        prefs.insert(key.into(), env.rel_value(&value));
    }
    let history: Vec<Value> = catalog
        .rows("SELECT id,photo_id,label FROM history ORDER BY id", [])
        .unwrap()
        .into_iter()
        .map(|r| json!([r["id"], r["photo_id"], r["label"]]))
        .collect();
    json!({"photos": photos, "folder_roots": roots, "preferences": prefs, "history": history})
}

/// Every file below the root but the library's, with the input its bytes are (sorted).
fn tree(env: &Env) -> Vec<(String, String)> {
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
    out
}

/// The backups of the library by kind (`before-restore`, `folder-location`…).
fn backups(env: &Env) -> BTreeMap<String, u64> {
    let mut kinds = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(env.root.join("library").join("backups")) {
        for entry in entries {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            let cut =
                name.char_indices().find(|(i, c)| *c == '-' && name[i + 1..].starts_with(|d: char| d.is_ascii_digit()));
            let kind = cut.map_or(name.clone(), |(i, _)| name[..i].to_owned());
            *kinds.entry(kind).or_insert(0) += 1;
        }
    }
    kinds
}

/// Runs the finished jobs until none is running (the window's `poll_background` for jobs).
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

fn last_error(cx: &mut AppContext) -> Option<String> {
    cx.state::<FilesState>().last_error.take()
}

fn select(cx: &mut AppContext, ids: &[u64]) {
    // The open photo is part of every selection in the app; the script selects it first.
    for id in ids.iter().skip(1) {
        let index = cx.doc.index_of(*id).unwrap();
        if !cx.doc.is_selected(*id) {
            cx.doc.toggle_selected(index);
        }
    }
}

fn open(cx: &mut AppContext, id: u64) {
    let index = cx.doc.index_of(id).unwrap_or_else(|| panic!("photo {id} is not listed"));
    cx.doc.select(index);
}

fn str_arg(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

// ---- the session against the oracle -------------------------------------------------------------------------

#[test]
fn a_scripted_session_equals_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut golden = Case::open("app", "files_steps").json().clone();
    // The steps' backslashes are the separators of `<ROOT>\...` paths: on macOS the oracle writes `/`.
    golden["steps"] = common::native_json(&golden["steps"]);
    let (env, mut cx, trashed) = build("files-steps", &golden);
    let platform = Platform::native();
    let mut previous_tables = None;
    for want in golden["steps"].as_array().unwrap() {
        let step = want["step"].as_array().unwrap();
        let action = step[0].as_str().unwrap();
        let a = &step[1..];
        let at = format!("{}", want["step"]);
        trashed.lock().unwrap().clear();
        let mut error: Option<String> = None;
        let mut texts: Vec<String> = Vec::new();
        match action {
            "open" => open(&mut cx, a[0].as_u64().unwrap()),
            "select" => {
                let ids: Vec<u64> = a[0].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
                select(&mut cx, &ids);
            }
            "set" => cx.doc.edit_mut().set(a[0].as_str().unwrap(), a[1].clone()),
            "folder" => {
                let mut q = cx.doc.query();
                q.folder = str_arg(&a[0]).map(|f| env.full(&f));
                cx.doc.set_query(q).unwrap();
            }
            "fs_move" => std::fs::rename(env.full(a[0].as_str().unwrap()), env.full(a[1].as_str().unwrap())).unwrap(),
            "rating" => {
                let ids: Vec<i64> = a[0].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
                cx.doc.metadata_catalog().unwrap().update_many(&ids, "rating", &a[1]).unwrap();
            }
            "rename" => match str_arg(&a[0]) {
                Some(template) => files::rename_with(&mut cx, &template),
                None => {
                    assert!(cx.run("file.rename"));
                    assert!(cx.dialog_open("files_rename"));
                    cx.close_dialog("files_rename");
                }
            },
            "move" => {
                if let Some(folder) = str_arg(&a[0]) {
                    files::move_to(&mut cx, Path::new(&env.full(&folder)));
                }
            }
            "move_folder" => {
                if let Some(destination) = str_arg(&a[0]) {
                    cx.run_with("file.move_folder", &Invocation::value(env.full(&destination)));
                } else if cx.doc.query().folder.is_some() {
                    assert!(cx.run("file.move_folder"));
                    assert!(cx.dialog_open("files_move_folder"));
                    cx.close_dialog("files_move_folder");
                }
            }
            "trash" => {
                let ids = files::selected_ids(&cx);
                assert!(cx.run("file.trash"));
                assert!(cx.dialog_open("files_trash"), "{at}");
                cx.close_dialog("files_trash");
                texts.push(format!(
                    "{}\n{}",
                    grainy_app::text::tr_with("선택한 {0}장을 카탈로그에서 제거합니다.", &[&ids.len().to_string()]),
                    files::tr_files("원본 파일은 휴지통으로 이동하며 같은 원본의 가상 사본도 제거됩니다.")
                ));
                if a[0] == true {
                    files::trash_now(&mut cx, &ids);
                }
            }
            "relink" => files::relink_to(&mut cx, Path::new(&env.full(a[0].as_str().unwrap()))),
            "duplicates" => {
                assert!(cx.run("file.duplicates"));
                drain(&mut cx);
                assert!(cx.dialog_open("duplicates"), "{at}");
                cx.close_dialog("duplicates");
                let photos = cx.doc.metadata_catalog().unwrap().photos().unwrap();
                let groups = grainy_library::library::duplicate_groups(platform, &photos).unwrap();
                let ids: Vec<Vec<i64>> =
                    groups.iter().map(|g| g.iter().map(|p| p["id"].as_i64().unwrap()).collect()).collect();
                assert_eq!(json!(ids), want["duplicates"], "{at}");
            }
            "integrity" => {
                assert!(cx.run("file.integrity"));
                drain(&mut cx);
                assert!(cx.dialog_open("files_integrity"), "{at}");
                cx.close_dialog("files_integrity");
                let photos = cx.doc.metadata_catalog().unwrap().photos().unwrap();
                let states: Vec<_> = photos
                    .iter()
                    .map(|p| grainy_library::file_access::access(Path::new(p["path"].as_str().unwrap()), false))
                    .collect();
                let count = |s| states.iter().filter(|x| **x == s).count();
                use grainy_library::file_access::Access;
                texts.push(files::integrity_text("ok", photos.len(), count(Access::Missing), count(Access::Denied)));
            }
            "backup" => {
                files::backup_to(&mut cx, Path::new(&env.full(a[0].as_str().unwrap())));
                if cx.dialog_open("files_backup") {
                    cx.close_dialog("files_backup");
                    texts.push(grainy_i18n::tr("카탈로그·보정·컬렉션을 백업했습니다. 원본 사진은 별도로 보관하세요."));
                }
            }
            "restore" => {
                let path = env.full(a[0].as_str().unwrap());
                assert!(cx.run_with("file.restore", &Invocation::value(path.clone())));
                assert!(cx.dialog_open("files_restore"));
                cx.close_dialog("files_restore");
                texts.push(grainy_i18n::tr("현재 카탈로그를 자동 백업한 뒤 선택한 백업으로 복원할까요?"));
                if a[1] == true {
                    files::restore_from(&mut cx, Path::new(&path));
                }
            }
            "locate" | "relink_folder" => {
                let (source, destination, yes) = if action == "locate" {
                    (str_arg(&a[0]).map(|s| env.full(&s)), a[1].clone(), a[2].clone())
                } else {
                    (cx.doc.query().folder, a[0].clone(), a[1].clone())
                };
                match source {
                    None => {
                        assert!(cx.run("file.relink_folder"));
                    }
                    Some(source) => {
                        let destination = env.full(destination.as_str().unwrap());
                        grainy_app::folders::locate_to(&mut cx, &source, Path::new(&destination));
                        if cx.dialog_open("folder_locate") {
                            cx.close_dialog("folder_locate");
                            let plan = grainy_library::folder_locations::relink_plan(
                                cx.doc.metadata_catalog().unwrap(),
                                &source,
                                &destination,
                            )
                            .unwrap();
                            texts.push(grainy_app::folders::locate_text(&plan));
                            if yes == true {
                                grainy_app::folders::locate_now(&mut cx, plan);
                            }
                        }
                    }
                }
            }
            other => panic!("unknown step {other}"),
        }
        cx.doc.refresh().unwrap();
        error = error.or_else(|| last_error(&mut cx));
        cx.close_dialog("files_error");

        // The error the oracle's `run` box shows.
        assert_eq!(error.map(|e| env.rel(&e)), want["error"].as_str().map(str::to_owned), "error at {at}");
        // The texts of the boxes that carry the step's content (questions and information).
        let boxes: Vec<String> = want["boxes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["box"] == "question" || b["box"] == "information")
            .map(|b| b["text"].as_str().unwrap().to_owned())
            .collect();
        let shown: Vec<String> = texts.iter().map(|t| env.rel(t)).collect();
        for text in &shown {
            assert!(boxes.contains(text), "box text at {at}: {text:?} not in {boxes:?}");
        }
        // The open photo (the oracle's stand-in opens nothing after `ensure_current`, where the app opens the first;
        // and the oracle's `activate` opens a photo the list's folder no longer shows, the app only listed ones).
        if let Some(id) = want["current"].as_u64().filter(|id| cx.doc.index_of(*id).is_some()) {
            assert_eq!(cx.doc.current().map(|p| p.id), Some(id), "open photo at {at}");
        }
        assert_eq!(
            cx.doc.query().folder.map(|f| env.rel(&f)),
            want["folder"].as_str().map(str::to_owned),
            "folder at {at}"
        );
        let trashed_now: Vec<String> = trashed.lock().unwrap().iter().map(|p| env.rel(p)).collect();
        assert_eq!(json!(trashed_now), want["trashed"], "trashed at {at}");
        let mut files_want: Vec<(String, String)> = want["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_owned(), p[1].as_str().unwrap().to_owned()))
            .collect();
        files_want.sort_by_key(|(p, _)| p.to_lowercase());
        assert_eq!(tree(&env), files_want, "files at {at}");
        let kinds: BTreeMap<String, u64> = serde_json::from_value(want["backups"].clone()).unwrap();
        assert_eq!(backups(&env), kinds, "backups at {at}");
        let now = tables(&env, &cx);
        if let Some(expected) = want.get("tables") {
            assert_eq!(&now, expected, "tables at {at}");
        } else if let Some(previous) = &previous_tables {
            assert_eq!(&now, previous, "the tables did not change at {at}");
        }
        previous_tables = Some(now);
    }

    // 카탈로그 최적화 on the library after the script: the oracle's counts.
    assert!(cx.run("file.optimize"));
    assert!(cx.dialog_open("maintenance"));
    drain(&mut cx);
    let report = cx.state::<FilesState>().last_optimization.clone().expect("a report");
    let want = &golden["optimize"];
    assert_eq!(report.converted, want["converted"].as_u64().unwrap());
    assert_eq!(report.examined, want["examined"].as_u64().unwrap());
    assert_eq!(report.total, want["total"].as_u64().unwrap());
    assert_eq!(report.profiles, want["profiles"].as_u64().unwrap());
    assert_eq!(report.cancelled, want["cancelled"].as_bool().unwrap());
    assert_eq!(report.integrity, want["integrity"].as_str().unwrap());
    let kinds: BTreeMap<String, u64> = serde_json::from_value(want["backups"].clone()).unwrap();
    assert_eq!(backups(&env), kinds);
    assert!(cx.dialog_open("files_optimize"));
    assert!(files::optimize_text(&report).starts_with("카탈로그 최적화를 마쳤습니다.\n정리한 기록: 0개"));
}

/// Originals are never written: after every kind of command, each file still holds the bytes of the input it was
/// made from (moved, renamed or in the bin, never changed), and a refused move leaves both files as they were.
#[test]
fn originals_keep_their_bytes() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let golden = Case::open("app", "files_steps").json().clone();
    let (env, mut cx, _) = build("files-bytes", &golden);
    let before = tree(&env);
    // A rename onto an existing name is refused before anything moves.
    open(&mut cx, 5);
    files::rename_with(&mut cx, "b1");
    assert_eq!(last_error(&mut cx).as_deref(), Some("대상 파일이 이미 있습니다: b1.png"));
    assert_eq!(tree(&env), before);
    // A move onto a file of the same name is refused too.
    open(&mut cx, 1);
    std::fs::copy(env.root.join("photos/B/b1.png"), env.root.join("elsewhere/a1.png")).unwrap();
    files::move_to(&mut cx, &env.root.join("elsewhere"));
    assert_eq!(last_error(&mut cx).as_deref(), Some("대상 파일이 이미 있습니다: a1.png"));
    assert_eq!(std::fs::read(env.root.join("elsewhere/a1.png")).unwrap(), env.inputs["four"], "never overwritten");
    assert_eq!(std::fs::read(env.root.join("photos/A/a1.png")).unwrap(), env.inputs["one"]);
    // Rename, move, folder move, relink, backup, restore, optimise: the inputs' bytes are all still there.
    files::rename_with(&mut cx, "{stem}-x");
    cx.doc.refresh().unwrap();
    files::move_to(&mut cx, &env.root.join("photos/C"));
    cx.doc.refresh().unwrap();
    cx.run_with("file.backup", &Invocation::value(pathx::text(&env.root.join("elsewhere/b.sqlite"))));
    files::restore_from(&mut cx, &env.root.join("elsewhere/b.sqlite"));
    cx.doc.refresh().unwrap();
    assert!(cx.run("file.optimize"));
    drain(&mut cx);
    let mut kinds: Vec<String> = tree(&env).into_iter().map(|(_, k)| k).filter(|k| k != "other").collect();
    let mut want: Vec<String> = before.into_iter().map(|(_, k)| k).collect();
    want.push("four".into());
    kinds.sort();
    want.sort();
    assert_eq!(kinds, want, "every original is still there with its bytes");
    assert!(env.root.join("photos/C/a1-x.png").is_file());
}

/// The rename box's mapping: the template with `stem` and `n`, the suffix kept, folder parts refused.
#[test]
fn rename_templates() {
    // Paths as this system writes them (`C:\p\...` is one file name on macOS, whose stem holds `:` and `\`).
    let at =
        |windows: &str| if cfg!(windows) { windows.to_owned() } else { windows.replace("C:", "").replace('\\', "/") };
    let paths = vec![at(r"C:\p\A\one.png"), at(r"C:\p\B\two.JPG")];
    let mapping = files::rename_mapping(&paths, "{stem}_{n:03d}").unwrap();
    assert_eq!(mapping[0].1, at(r"C:\p\A\one_001.png"));
    assert_eq!(mapping[1].1, at(r"C:\p\B\two_002.JPG"));
    assert_eq!(files::rename_mapping(&paths, "trip-{n}").unwrap()[1].1, at(r"C:\p\B\trip-2.JPG"));
    for bad in ["a/{stem}", "a\\{stem}", "x:{stem}", "{stem}?"] {
        assert_eq!(
            files::rename_mapping(&paths, bad).unwrap_err(),
            "파일 이름에 경로나 특수문자를 넣을 수 없습니다.",
            "{bad}"
        );
    }
    assert_eq!(files::rename_mapping(&paths, "{name}").unwrap_err(), "'name'");
}

/// A session (no library) has no file management: the commands are greyed out and the
/// document refuses the operations.
#[test]
fn a_session_has_no_file_management() {
    let mut cx = AppContext::new(
        &egui::Context::default(),
        Box::new(TestCard::default()),
        grainy_app::context::memory_document(),
    );
    for id in
        ["file.rename", "file.move", "file.trash", "file.backup", "file.restore", "file.optimize", "file.integrity"]
    {
        assert!(!grainy_app::commands::enabled(&cx, id), "{id}");
    }
    assert!(cx.doc.file_op(grainy_app::document::catalog::files::FileOp::Commit).is_err());
}

// ---- offline packets ----------------------------------------------------------------------------------------

/// `smart_previews` writes the oracle's packet (float16 pixels of the photo at its working space), and a photo
/// whose original is gone is developed from it.
#[test]
fn offline_packets_equal_the_oracle_and_replace_a_missing_original() {
    let _turn = common::language_lock();
    let golden = Case::open("app", "files_steps").json().clone();
    let offline = Case::open("app", "files_offline");
    let (env, mut cx, _) = build("files-offline", &golden);
    open(&mut cx, 3);
    cx.doc.edit_mut().set("exposure", json!(0.4));
    assert!(cx.run("file.smart_previews"));
    drain(&mut cx);
    let packet_path = env.root.join("library").join("previews").join("3.npz");
    let packet = grainy_catalog::preview::read_offline(&packet_path).expect("the packet");
    assert!(packet.pixels.half, "float16 like the oracle's");
    assert_eq!(json!(packet.pixels.shape), offline.json()["shape"]);
    let want: ndarray::Array3<f32> = offline.array("pixels");
    let got = ndarray::Array3::from_shape_vec(
        (packet.pixels.shape[0], packet.pixels.shape[1], packet.pixels.shape[2]),
        packet.pixels.values.clone(),
    )
    .unwrap();
    let (largest, _) = grainy_testkit::max_abs_diff(&got, &want);
    println!("offline packet: largest difference {largest}");
    assert!(largest <= 1e-3, "largest difference {largest}");
    for key in ["width", "height", "working_space"] {
        assert_eq!(packet.info[key], offline.json()[key], "{key}");
    }
    // The original goes away: the packet is developed instead (`preview_store.load_offline`).
    let original = env.root.join("photos/A/sub/a3.png");
    std::fs::rename(&original, env.root.join("elsewhere/a3.png")).unwrap();
    grainy_app::worker::register_offline(&original, &env.root.join("library"), 3);
    let settings = cx.doc.stored_settings_of(3).unwrap();
    let loaded = grainy_app::worker::offline_photo(&env.root.join("library"), 3, &settings).expect("offline source");
    assert_eq!(loaded.photo.pixels().dim(), (30, 20, 3));
    assert_eq!(loaded.metadata["offline_preview"], true);
    // The thumbnail is made again from the packet (선택 사진 미리보기 다시 만들기).
    files::start_batch(&mut cx, &[3, 1]);
    drain(&mut cx);
    assert!(cx.status().starts_with("미리보기 2/2장 처리"), "{}", cx.status());
    assert!(env.root.join("library/thumbnails/3.jpg").is_file());
    assert!(env.root.join("library/thumbnails/1.jpg").is_file());
}

// ---- folders renamed or moved outside Grainy ----------------------------------------------------------------

/// The window over a library of the generator's files; folder scans and searches run as in the app.
fn library_harness<'a>(name: &str) -> (Harness<'a, GrainyApp>, Env) {
    let golden = Case::open("app", "files_steps").json().clone();
    let (env, cx, _) = build(name, &golden);
    drop(cx);
    let library = env.root.join("library");
    let data = env.root.join("data");
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app
    });
    common::snapshot::settle(&mut harness);
    (harness, env)
}

fn wait_for(harness: &mut Harness<'_, GrainyApp>, what: &str, mut done: impl FnMut(&mut GrainyApp) -> bool) {
    let started = Instant::now();
    while !done(harness.state_mut()) {
        harness.step();
        std::thread::sleep(Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_secs(120), "waited too long for {what}");
    }
}

fn paths(app: &GrainyApp, env: &Env) -> Vec<String> {
    let catalog = app.cx.doc.metadata_catalog().unwrap();
    catalog.photos().unwrap().iter().map(|p| env.rel(p["path"].as_str().unwrap())).collect()
}

/// `FolderPanel.refresh`: a folder renamed on disk next to where it was keeps its identity, and the scan relinks
/// it (a backup first); the photos keep their ids and edits.
#[test]
fn a_renamed_folder_is_relinked_by_the_scan() {
    let (mut harness, env) = library_harness("files-rename-scan");
    harness.state_mut().cx.state::<grainy_app::folders::FolderPanel>().automatic_off = true;
    // The scan saw folder C (its identity is saved), then it is renamed.
    wait_for(&mut harness, "the identities", |app| {
        app.cx.doc.preference("folder_identities").is_some_and(|v| v.as_object().is_some_and(|m| m.len() >= 3))
    });
    std::fs::rename(env.root.join("photos/C"), env.root.join("photos/C-renamed")).unwrap();
    harness.state_mut().cx.state::<grainy_app::session_state::ColumnState>().rescan();
    wait_for(&mut harness, "the relink", |app| {
        paths(app, &env).contains(&common::native(r"<ROOT>\photos\C-renamed\c1.png"))
    });
    assert_eq!(backups(&env).get("folder-location"), Some(&1));
    assert!(harness.state().status().contains("C-renamed"), "{}", harness.state().status());
}

/// `FolderPanel.recover`: a folder renamed while Grainy did not watch (no saved identity) is found again among
/// its parent's folders by its photos' file facts (`file_fingerprint`, three or more) and relinked automatically
/// after the scan marks it missing.
#[test]
fn a_renamed_folder_without_identity_is_found_again() {
    let (mut harness, env) = library_harness("files-recover");
    {
        let app = harness.state_mut();
        let catalog = app.cx.doc.metadata_catalog().unwrap();
        for row in catalog.photos().unwrap() {
            let path = row["path"].as_str().unwrap().to_owned();
            let meta = std::fs::metadata(&path).unwrap();
            let mtime = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
            let info = json!({"width": 1, "height": 1, "file_fingerprint": {"size": meta.len(), "mtime_ns": mtime}});
            let id = row["id"].as_i64().unwrap();
            catalog
                .connection()
                .execute(
                    "UPDATE photos SET metadata=?1 WHERE id=?2",
                    grainy_catalog::rusqlite::params![info.to_string(), id],
                )
                .unwrap();
        }
        app.cx.doc.save_preference("folder_identities", &json!({})).unwrap();
    }
    std::fs::rename(env.root.join("photos/A"), env.root.join("photos/A-later")).unwrap();
    harness.state_mut().cx.state::<grainy_app::session_state::ColumnState>().rescan();
    wait_for(&mut harness, "the recovery", |app| {
        paths(app, &env).contains(&common::native(r"<ROOT>\photos\A-later\sub\a3.png"))
    });
    let report = harness.state().cx.doc.preference("folder_recovery_last").expect("the report");
    assert_eq!(report["photos"], 4, "a1, a2, a3 and the virtual copy of a1");
    let proof = report["moves"][0]["proof"].as_str().unwrap();
    assert!(proof == "file_stats" || proof == "saved_previews", "{proof}");
    assert_eq!(backups(&env).get("folder-location"), Some(&1));
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

fn row(id: i64, path: &str) -> Value {
    json!({"id": id, "name": path.rsplit('\\').next().unwrap(), "path": path})
}

fn duplicates(language: Language, tag: &str) {
    render(language, &format!("files-duplicates-{tag}"), |app| {
        let groups = vec![
            vec![row(1, r"D:\Photos\Trip\a1.png"), row(5, r"D:\Photos\Backup\copy.png")],
            vec![row(2, r"D:\Photos\Trip\a2.png"), row(6, r"D:\Photos\Other\c1.png"), row(9, r"E:\Card\c1.png")],
        ];
        app.cx.open_dialog(Box::new(DuplicatesDialog::new(groups)));
    });
}

#[test]
fn duplicates_window_korean() {
    duplicates(Language::Korean, "ko");
}

#[test]
fn duplicates_window_english() {
    duplicates(Language::English, "en");
}

fn maintenance(language: Language, tag: &str) {
    render(language, &format!("files-maintenance-{tag}"), |app| {
        let shared = Shared::default();
        shared.put(Update { stage: "무결성 확인 중".into(), done: 3, total: 8 });
        app.cx.open_dialog(Box::new(MaintenanceDialog::new(shared)));
    });
}

#[test]
fn maintenance_window_korean() {
    maintenance(Language::Korean, "ko");
}

#[test]
fn maintenance_window_english() {
    maintenance(Language::English, "en");
}

fn locate(language: Language, tag: &str) {
    render(language, &format!("files-locate-{tag}"), |app| {
        let plan = grainy_library::folder_locations::Plan {
            source: r"D:\Photos\Trip".into(),
            destination: r"E:\Archive\Trip 2026".into(),
            updates: vec![(r"E:\Archive\Trip 2026\a.png".into(), 1), (r"E:\Archive\Trip 2026\b.png".into(), 2)],
            found: 2,
            missing: 0,
        };
        let text = grainy_app::folders::locate_text(&plan);
        app.cx.open_dialog(Box::new(grainy_app::dialogs::confirm::Confirm::new(
            "folder_locate",
            grainy_app::text::tr("폴더 위치 다시 지정"),
            text,
            |_| {},
        )));
    });
}

#[test]
fn locate_question_korean() {
    locate(Language::Korean, "ko");
}

#[test]
fn locate_question_english() {
    locate(Language::English, "en");
}

#[test]
fn partial_bin_failure_updates_only_completed_original_and_its_virtual_copies() {
    struct RefuseSecond {
        first: PathBuf,
        moved_to: PathBuf,
    }
    impl Trash for RefuseSecond {
        fn trash(&self, path: &Path) -> Result<(), String> {
            if std::fs::canonicalize(path).map_err(|e| e.to_string())? != self.first {
                return Err("simulated bin refusal".into());
            }
            std::fs::rename(path, &self.moved_to).map_err(|error| error.to_string())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.png");
    let second = dir.path().join("second.png");
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&second, b"second").unwrap();
    let catalog = Catalog::new(dir.path().join("library")).unwrap();
    let a = catalog.add(first.to_str().unwrap(), None, None, false).unwrap();
    let b = catalog.add(second.to_str().unwrap(), None, None, false).unwrap();
    let a_copy = catalog.virtual_copy(a).unwrap();
    let b_copy = catalog.virtual_copy(b).unwrap();
    // Selection order controls which original is recycled first. Virtual copies and
    // repeated IDs must not cause a second filesystem operation.
    let selected = [b_copy, a_copy, b, a, b_copy].map(|id| id as u64);
    let paths = grainy_app::document::catalog::files::paths_of(&catalog, &selected).unwrap();
    assert_eq!(paths, [b, a].map(|id| catalog.photo(id).unwrap().unwrap()["path"].as_str().unwrap().to_owned()));
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    let moved_to = dir.path().join("fake-bin");
    files::set_trash(
        &mut cx,
        Arc::new(RefuseSecond { first: std::fs::canonicalize(&first).unwrap(), moved_to: moved_to.clone() }),
    );
    files::trash_now(&mut cx, &[a_copy as u64, a as u64, b as u64]);
    assert_eq!(cx.state::<FilesState>().last_error.as_deref(), Some("simulated bin refusal"));
    let catalog = cx.doc.metadata_catalog().unwrap();
    assert_eq!(std::fs::read(moved_to).unwrap(), b"first");
    assert!(!first.exists());
    assert_eq!(std::fs::read(second).unwrap(), b"second");
    assert!(catalog.photo(a).unwrap().is_none());
    assert!(catalog.photo(a_copy).unwrap().is_none());
    assert!(catalog.photo(b).unwrap().is_some());
    assert!(catalog.photo(b_copy).unwrap().is_some());
}
