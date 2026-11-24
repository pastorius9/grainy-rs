//! 파이썬 Grainy 라이브러리 가져오기 (`grainy_app::python_import`): a library made like the Python Grainy's
//! (`<root>/Luma/Library` with photos, a thumbnail and a camera profile beside it) comes over into this app's
//! library, the source is never written, a library that is not empty is asked about and backed up first, and the
//! question after the window is asked once.

mod common;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use grainy_app::commands::settings;
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::python_import::{self, PythonLibrary, OFFERED};
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::startup::{self, Step};
use grainy_app::AppContext;
use grainy_catalog::rusqlite::{Connection, OpenFlags};
use grainy_catalog::Catalog;
use grainy_platform::json::Value as Pref;
use grainy_platform::preferences::Preferences;

fn drain(cx: &mut AppContext) {
    let started = Instant::now();
    loop {
        settings::pump(cx);
        if cx.jobs.idle() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(60), "a job did not finish");
    }
    settings::pump(cx);
}

/// A Python Grainy library in `<root>/Luma/Library` with `n` photos (one virtual copy among them when `n` > 1),
/// thumbnails of the first photo, a file that is not a thumbnail, and a camera profile in `<root>/Luma/CameraProfiles`.
fn python_library(root: &Path, n: usize) -> PathBuf {
    let library = root.join("Luma").join("Library");
    let photos = root.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let catalog = Catalog::new(&library).unwrap();
    for i in 0..n {
        let path = photos.join(format!("p{i}.jpg"));
        std::fs::write(&path, b"not really a photo").unwrap();
        catalog.add(path.to_str().unwrap(), None, None, false).unwrap();
    }
    catalog.close().unwrap();
    let thumbnails = library.join("thumbnails");
    std::fs::write(thumbnails.join("1.jpg"), b"thumbnail").unwrap();
    std::fs::write(thumbnails.join("1.json"), b"{}").unwrap();
    std::fs::write(thumbnails.join("notes.txt"), b"not a thumbnail").unwrap();
    let profiles = root.join("Luma").join("CameraProfiles").join("Maker");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(profiles.join("Mine.dcp"), b"profile").unwrap();
    library
}

/// Every file under `folder` and its bytes.
fn files(folder: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// `(id, path, edits)` of every photo in the catalogue of `library`.
fn rows(library: &Path) -> Vec<(i64, String, String)> {
    let db = Connection::open_with_flags(library.join("catalog.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = db.prepare("SELECT id, path, edits FROM photos ORDER BY id").unwrap();
    let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

/// A context on this app's library `<root>/rs/Library` with `<root>/rs` as data folder, holding `photos` photos.
fn rust_app(root: &Path, photos: usize) -> (AppContext, PathBuf) {
    let data = root.join("rs");
    let library = data.join("Library");
    if photos > 0 {
        let catalog = Catalog::new(&library).unwrap();
        for i in 0..photos {
            let path = root.join(format!("rust-{i}.jpg"));
            std::fs::write(&path, b"x").unwrap();
            catalog.add(path.to_str().unwrap(), None, None, false).unwrap();
        }
    }
    let mut cx =
        AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(Session::default()));
    cx.set_document(Box::new(CatalogDocument::open(&library).unwrap()));
    cx.set_data_folder(Some(data.clone()));
    (cx, data)
}

fn offered(data: &Path) -> Option<Pref> {
    Preferences::new(data.join("settings.json")).values.get(OFFERED).cloned()
}

#[test]
fn an_empty_library_takes_the_python_library_and_leaves_it_untouched() {
    let root = common::temp_dir("python-import-empty");
    let source = python_library(&root, 3);
    let before = files(&root.join("Luma"));
    let expected = rows_of_file(&source.join("catalog.sqlite"), &root.join("check"));
    let (mut cx, data) = rust_app(&root, 0);
    python_import::start(&mut cx, &source, false);
    drain(&mut cx);
    assert!(cx.dialog_open("python_import_done"), "an empty library is replaced without a second question");
    drop(cx);
    let after = files(&root.join("Luma"));
    let changed: Vec<&PathBuf> =
        before.keys().chain(after.keys()).filter(|path| before.get(*path) != after.get(*path)).collect();
    let detail: Vec<String> = changed
        .iter()
        .map(|p| {
            format!("{} before {:?} after {:?}", p.display(), before.get(*p).map(Vec::len), after.get(*p).map(Vec::len))
        })
        .collect();
    assert!(changed.is_empty(), "the Python Grainy's folders are only read: {detail:?}");
    assert_eq!(rows(&data.join("Library")), expected, "photos, ids and edits come over");
    let thumbnails = data.join("Library").join("thumbnails");
    assert_eq!(std::fs::read(thumbnails.join("1.jpg")).unwrap(), b"thumbnail");
    assert!(thumbnails.join("1.json").is_file());
    assert!(!thumbnails.join("notes.txt").exists(), "only thumbnails and their stamps");
    assert_eq!(std::fs::read(data.join("CameraProfiles").join("Maker").join("Mine.dcp")).unwrap(), b"profile");
    assert_eq!(offered(&data), Some(Pref::Bool(true)));
    let left = std::fs::read_dir(data.join("python import")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0, "the private snapshot is removed");
}

#[test]
fn a_library_with_photos_is_asked_about_and_backed_up() {
    let root = common::temp_dir("python-import-full");
    let source = python_library(&root, 2);
    let (mut cx, data) = rust_app(&root, 1);
    let library = data.join("Library");
    let old = library.join("previews/1.npz");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, b"former photo").unwrap();
    std::fs::create_dir_all(source.join("previews")).unwrap();
    std::fs::write(source.join("previews/2.npz"), b"incoming photo").unwrap();
    let mine = rows(&library);
    let expected = rows_of_file(&source.join("catalog.sqlite"), &root.join("check"));
    python_import::start(&mut cx, &source, false);
    drain(&mut cx);
    assert!(cx.dialog_open("python_import"), "a library with photos is always asked about");
    cx.close_dialog("python_import");
    assert_eq!(rows(&library), mine, "no answer, no change");
    let prepared = python_import::prepare(&source, &data, &library, &|| false).unwrap();
    assert_eq!(prepared.photos, 2);
    python_import::apply(&mut cx, prepared);
    drain(&mut cx);
    assert!(cx.dialog_open("python_import_done"));
    drop(cx);
    assert_eq!(rows(&library), expected);
    assert!(!old.exists());
    assert_eq!(std::fs::read(library.join("previews/2.npz")).unwrap(), b"incoming photo");
    assert!(library.join("thumbnails").join("1.jpg").is_file(), "the thumbnails are placed after the catalogue");
    let backups: Vec<String> = std::fs::read_dir(library.join("backups"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let backup = backups.iter().find(|name| name.starts_with("before-restore-")).expect("the backup of the library");
    let replaced = rows_of_file(&library.join("backups").join(backup), &root.join("check-backup"));
    assert_eq!(replaced, mine, "the backup holds the replaced library");
}

/// [`rows`] of a copy of the catalogue file `path` in `folder` (opening SQLite on a live WAL database writes beside
/// it, so the Python Grainy's own file is never opened).
fn rows_of_file(path: &Path, folder: &Path) -> Vec<(i64, String, String)> {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::copy(path, folder.join("catalog.sqlite")).unwrap();
    rows(folder)
}

#[test]
fn without_a_python_library_nothing_changes() {
    let root = common::temp_dir("python-import-missing");
    let (mut cx, data) = rust_app(&root, 0);
    python_import::start(&mut cx, &root.join("Luma").join("Library"), true);
    drain(&mut cx);
    assert!(cx.dialog_open("python_import_missing"));
    assert!(offered(&data).is_none());
}

#[test]
fn the_question_after_the_window_is_asked_once_for_an_empty_library() {
    let root = common::temp_dir("python-import-offer");
    let source = python_library(&root, 1);
    let (mut cx, data) = rust_app(&root, 0);
    startup::plan(&mut cx, &data, false);
    assert_eq!(
        cx.state::<startup::Startup>().steps,
        VecDeque::from([Step::UpdateStartup, Step::PythonLibrary, Step::CheckFolders])
    );
    // Without the program's start naming the Python Grainy's library (tests, the smoke run) nothing is asked.
    startup::run_step(&mut cx, Step::PythonLibrary);
    assert!(!cx.dialog_open("python_import_offer"));
    cx.state::<PythonLibrary>().offer = Some(source.clone());
    startup::run_step(&mut cx, Step::PythonLibrary);
    assert!(cx.dialog_open("python_import_offer"));
    cx.close_dialog("python_import_offer");
    Preferences::new(data.join("settings.json")).save(OFFERED, Pref::Bool(true)).unwrap();
    startup::run_step(&mut cx, Step::PythonLibrary);
    assert!(!cx.dialog_open("python_import_offer"), "answered once, never asked again");

    // A library that already has photos is not asked about at start.
    let other = common::temp_dir("python-import-offer-full");
    let (mut cx, _) = rust_app(&other, 1);
    cx.state::<PythonLibrary>().offer = Some(source);
    startup::run_step(&mut cx, Step::PythonLibrary);
    assert!(!cx.dialog_open("python_import_offer"));
}

/// By hand, on a **copy** of a real Python Grainy library (never the library itself):
/// `GRAINY_PYTHON_LIBRARY=<copy>/Luma/Library cargo test -p grainy-app --test python_import -- --ignored --nocapture`.
#[test]
#[ignore]
fn a_real_python_library_comes_over() {
    let Some(source) = std::env::var_os("GRAINY_PYTHON_LIBRARY").map(PathBuf::from) else { return };
    let root = common::temp_dir("python-import-real");
    let before = files(&source);
    let expected = rows_of_file(&source.join("catalog.sqlite"), &root.join("check"));
    let (mut cx, data) = rust_app(&root, 0);
    let library = data.join("Library");
    let started = Instant::now();
    let prepared = python_import::prepare(&source, &data, &library, &|| false).unwrap();
    let copied = started.elapsed();
    let started = Instant::now();
    python_import::apply(&mut cx, prepared);
    let took = (copied, started.elapsed());
    assert!(cx.dialog_open("python_import_done"), "status: {}", cx.status());
    let started = Instant::now();
    drain(&mut cx);
    let took = (took.0, took.1, started.elapsed());
    cx.doc.refresh().unwrap();
    let listed = cx.doc.photos().len();
    drop(cx);
    assert!(files(&source) == before, "the source is only read");
    assert_eq!(rows(&data.join("Library")), expected);
    let thumbnails = std::fs::read_dir(data.join("Library").join("thumbnails")).unwrap().count();
    println!(
        "photos {} · listed {listed} · thumbnail files {thumbnails} · copies, replace (window), thumbnails (background) {took:?} · library {}",
        expected.len(),
        data.display()
    );
}

#[test]
fn prepared_import_is_bound_to_library_and_repairs_partial_profile() {
    let root = common::temp_dir("python-import-target");
    let source = python_library(&root, 2);
    let (mut a, data) = rust_app(&root, 0);
    let (mut b, bdata) = rust_app(&root.join("other"), 1);
    let prepare = || python_import::prepare(&source, &data, &data.join("Library"), &|| false).unwrap();
    let before = rows(&bdata.join("Library"));
    python_import::apply(&mut b, prepare());
    assert_eq!(rows(&bdata.join("Library")), before);
    let profile = data.join("CameraProfiles/Maker/Mine.dcp");
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(&profile, b"pro").unwrap();
    python_import::apply(&mut a, prepare());
    drain(&mut a);
    assert_eq!(std::fs::read(profile).unwrap(), b"profile");
}
