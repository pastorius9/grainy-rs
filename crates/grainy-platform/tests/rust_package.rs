//! The Rust build's Windows release zip (`GrainyRS/` with `GrainyRS.exe`, `grainy-cli.exe`, `vcomp140.dll`,
//! `LICENSE`, `THIRD_PARTY_NOTICES.txt`, `README.txt`): name checks, unpacking, the start command and the swap with
//! the oracle's safety (previous version kept, restored on failure, other files of the folder untouched), and the
//! update from a 0.1.0 folder (five items, no `vcomp140.dll`) to 0.2.0. Generated zips in temporary folders;
//! nothing is started.

mod common;

use common::*;
use grainy_platform::updater::{self, Hooks, Package, RS_PACKAGE};

fn rs_entries(version: &str) -> Vec<(String, String)> {
    updater::RS_ITEMS.iter().map(|name| (format!("GrainyRS/{name}"), format!("{name} {version}"))).collect()
}

fn names(entries: &[(String, String)]) -> Vec<String> {
    entries.iter().map(|(n, _)| n.clone()).collect()
}

#[test]
fn the_rust_zip_names_are_checked() {
    let good = names(&rs_entries("0.1.1"));
    assert!(updater::check_package_names(&good, &RS_PACKAGE).is_ok());
    // Only the program is required.
    assert!(updater::check_package_names(&["GrainyRS/GrainyRS.exe".into()], &RS_PACKAGE).is_ok());
    let refused = |list: &[&str]| {
        let list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        updater::check_package_names(&list, &RS_PACKAGE).is_err()
    };
    assert!(refused(&["GrainyRS/grainy-cli.exe"]), "no program");
    assert!(refused(&["GrainyRS.exe"]), "files at the zip root");
    assert!(refused(&["GrainyRS/GrainyRS.exe", "README.txt"]), "a second top entry");
    assert!(refused(&["GrainyRS/GrainyRS.exe", "GrainyRS/../evil.exe"]));
    assert!(refused(&["GrainyRS/GrainyRS.exe", "C:/evil.exe"]));
    assert!(refused(&["GrainyRS/GrainyRS.exe", "/evil.exe"]));
    assert!(refused(&["GrainyRS/GrainyRS.exe", "GrainyRS\\..\\evil.exe"]));
    // The Python Grainy's zip is not the Rust build's, and the reverse.
    assert!(refused(&["Grainy/Grainy.exe", "Grainy/_internal/x"]));
    assert!(updater::check_release_names(&good).is_err());
    assert!(updater::check_release_names(&["Grainy/Grainy.exe".into(), "Grainy/_internal/x".into()]).is_ok());
}

#[test]
fn a_rust_zip_is_staged_and_started_by_its_own_program() {
    let root = Temp::new();
    let layout = windows_layout(&root.0);
    let app = root.0.join("app");
    let work = app.join(".update");
    zip_file(&root.0.join("new.zip"), &rs_entries("0.1.1"));
    let staged = updater::stage_package(&root.0.join("new.zip"), &work, &RS_PACKAGE).unwrap();
    assert_eq!(staged, work.join("new"));
    for name in updater::RS_ITEMS {
        assert_eq!(std::fs::read_to_string(staged.join(name)).unwrap(), format!("{name} 0.1.1"));
    }
    // A second staging before the first was tidied is refused, as the oracle's.
    zip_file(&root.0.join("again.zip"), &rs_entries("0.1.2"));
    assert!(updater::stage_package(&root.0.join("again.zip"), &work, &RS_PACKAGE).is_err());
    let (program, command, cwd) =
        updater::launch_command_for(&staged, &app, &["--memory".into()], 42, &layout, &RS_PACKAGE);
    assert_eq!(program, staged.join("GrainyRS.exe"));
    assert_eq!(cwd, staged);
    assert_eq!(command, ["--finish-update", &app.display().to_string(), "--wait-pid", "42", "--", "--memory"]);
    // The oracle's command is unchanged.
    assert_eq!(updater::launch_command(&staged, &app, &[], 42, &layout).0, staged.join("Grainy.exe"));
}

#[test]
fn the_swap_replaces_only_the_release_items_and_keeps_the_previous_version() {
    let root = Temp::new();
    let layout = windows_layout(&root.0);
    let app = root.0.join("app");
    let mut installed = rs_entries("0.1.0");
    installed.iter_mut().for_each(|(name, _)| *name = name.trim_start_matches("GrainyRS/").to_string());
    installed.push(("my notes.txt".into(), "the person's own file".into()));
    put(&app, &installed);
    let work = app.join(".update");
    zip_file(&root.0.join("new.zip"), &rs_entries("0.1.1"));
    let staged = updater::stage_package(&root.0.join("new.zip"), &work, &RS_PACKAGE).unwrap();
    let hooks = Hooks { stamp: Some("20260101-000000".into()), ..Hooks::default() };
    let mut log: Vec<u8> = Vec::new();
    let error = updater::finish_package(&app, &staged, 0, Some(&mut log), &layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "");
    for name in updater::RS_ITEMS {
        assert_eq!(std::fs::read_to_string(app.join(name)).unwrap(), format!("{name} 0.1.1"));
        let previous = work.join("previous-20260101-000000").join(name);
        assert_eq!(std::fs::read_to_string(previous).unwrap(), format!("{name} 0.1.0"));
    }
    assert_eq!(std::fs::read_to_string(app.join("my notes.txt")).unwrap(), "the person's own file");
    let log = String::from_utf8(log).unwrap();
    assert!(log.contains("new version copied"), "{log}");
}

#[test]
fn a_failed_swap_restores_the_previous_version() {
    let root = Temp::new();
    let layout = windows_layout(&root.0);
    let app = root.0.join("app");
    put(&app, &[("GrainyRS.exe".into(), "old".into()), ("LICENSE".into(), "old licence".into())]);
    let work = app.join(".update");
    // A staged copy without the program is refused before anything moves.
    std::fs::create_dir_all(root.0.join("empty")).unwrap();
    let hooks = Hooks { stamp: Some("20260101-000000".into()), ..Hooks::default() };
    let error = updater::finish_package(&app, &root.0.join("empty"), 0, None, &layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "staged version is incomplete");
    assert_eq!(std::fs::read_to_string(app.join("GrainyRS.exe")).unwrap(), "old");
    // An invalid directory in a flat file slot is rejected during private staging.
    let staged = root.0.join("staged");
    put(&staged, &[("GrainyRS.exe".into(), "new".into()), ("LICENSE".into(), "new licence".into())]);
    std::fs::create_dir_all(staged.join("README.txt")).unwrap();
    let error = updater::finish_package(&app, &staged, 0, None, &layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "unexpected directory in flat release");
    assert_eq!(std::fs::read_to_string(app.join("GrainyRS.exe")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(app.join("LICENSE")).unwrap(), "old licence");
    assert!(!app.join("README.txt").exists());
    assert!(work.join("failed-20260101-000000").join("GrainyRS.exe").is_file(), "the new files are kept aside");
}

// ---- 0.2.0: vcomp140.dll beside the program, the update from 0.1.0 ----------------------------------------------

/// The release list of 0.1.0, as that build has it (`updater::RS_ITEMS` and `RS_PACKAGE` of v0.1.0). Its name
/// checks read only the top folder, the program and the required folder; `mac` did not exist in 0.1.0 and plays
/// no part on Windows.
const RS_ITEMS_0_1_0: [&str; 5] =
    ["GrainyRS.exe", "grainy-cli.exe", "LICENSE", "THIRD_PARTY_NOTICES.txt", "README.txt"];
const RS_PACKAGE_0_1_0: Package = Package {
    folder: "GrainyRS",
    program: "GrainyRS.exe",
    items: &RS_ITEMS_0_1_0,
    required_folder: None,
    mac: updater::RS_MAC,
};

/// Distinct bytes per file and version (a few hundred, not text).
fn bytes_of(name: &str, version: &str) -> Vec<u8> {
    let seed: Vec<u8> = format!("{name}@{version}").into_bytes();
    (0..300u32).map(|i| seed[i as usize % seed.len()].wrapping_mul(31).wrapping_add(i as u8)).collect()
}

fn zip_bytes(path: &std::path::Path, entries: &[(String, Vec<u8>)]) {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, data) in entries {
        writer.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap();
}

/// The 0.2.0 release zip: `GrainyRS/` with the six items.
fn zip_0_2_0(path: &std::path::Path) {
    let entries: Vec<(String, Vec<u8>)> =
        updater::RS_ITEMS.iter().map(|name| (format!("GrainyRS/{name}"), bytes_of(name, "0.2.0"))).collect();
    zip_bytes(path, &entries);
}

/// A program folder as 0.1.0 left it: its five items (with an old modification time) and the person's own files.
fn folder_0_1_0(app: &std::path::Path) {
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    for name in RS_ITEMS_0_1_0 {
        std::fs::create_dir_all(app).unwrap();
        std::fs::write(app.join(name), bytes_of(name, "0.1.0")).unwrap();
        std::fs::OpenOptions::new().write(true).open(app.join(name)).unwrap().set_modified(old).unwrap();
    }
    put(
        app,
        &[
            ("my notes.txt".into(), "the person's own file".into()),
            ("presets/warm.json".into(), "{\"exposure\": 0.3}".into()),
            ("grainy.ini".into(), "an item no release names".into()),
        ],
    );
}

/// Every file and folder under `root` except the work folder `.update`: bytes and modification time.
fn state(root: &std::path::Path) -> std::collections::BTreeMap<String, Option<(Vec<u8>, std::time::SystemTime)>> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut std::collections::BTreeMap<String, Option<(Vec<u8>, std::time::SystemTime)>>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if rel == ".update" {
                continue;
            }
            if path.is_dir() {
                out.insert(rel, None);
                walk(root, &path, out);
            } else {
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                out.insert(rel, Some((std::fs::read(&path).unwrap(), modified)));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn the_release_list_ships_openmp_beside_the_program() {
    assert_eq!(
        updater::RS_ITEMS,
        ["GrainyRS.exe", "grainy-cli.exe", "vcomp140.dll", "LICENSE", "THIRD_PARTY_NOTICES.txt", "README.txt"]
    );
    assert_eq!(RS_PACKAGE.items, &updater::RS_ITEMS);
    assert_eq!((RS_PACKAGE.folder, RS_PACKAGE.program, RS_PACKAGE.required_folder), ("GrainyRS", "GrainyRS.exe", None));
    // The one-line form `tools/release/layout.py` reads with a regular expression.
    let source = include_str!("../src/updater.rs");
    assert!(source.contains(
        "pub const RS_ITEMS: [&str; 6] = [\"GrainyRS.exe\", \"grainy-cli.exe\", \"vcomp140.dll\", \"LICENSE\", \"THIRD_PARTY_NOTICES.txt\", \"README.txt\"];"
    ));
}

#[test]
fn a_0_1_0_build_accepts_and_stages_the_0_2_0_zip() {
    let root = Temp::new();
    let layout = windows_layout(&root.0);
    let app = root.0.join("app");
    let zip = root.0.join("GrainyRS-0.2.0-windows-x64.zip");
    zip_0_2_0(&zip);
    // 0.1.0's name check allows the extra entry `GrainyRS/vcomp140.dll`: every entry under `GrainyRS/`, the program
    // present.
    let names: Vec<String> = updater::RS_ITEMS.iter().map(|name| format!("GrainyRS/{name}")).collect();
    assert!(names.contains(&"GrainyRS/vcomp140.dll".to_string()));
    assert!(updater::check_package_names(&names, &RS_PACKAGE_0_1_0).is_ok());
    // 0.1.0's staging unpacks every entry, so the staged 0.2.0 program finds its vcomp140.dll beside it when 0.1.0
    // starts it; that program then swaps with its own six-item list.
    let work = app.join(".update");
    let staged = updater::stage_package(&zip, &work, &RS_PACKAGE_0_1_0).unwrap();
    assert_eq!(staged, work.join("new"));
    for name in updater::RS_ITEMS {
        assert_eq!(std::fs::read(staged.join(name)).unwrap(), bytes_of(name, "0.2.0"), "{name}");
    }
    let (program, command, cwd) = updater::launch_command_for(&staged, &app, &[], 4242, &layout, &RS_PACKAGE_0_1_0);
    assert_eq!((program, cwd), (staged.join("GrainyRS.exe"), staged.clone()));
    assert_eq!(command, ["--finish-update", &app.display().to_string(), "--wait-pid", "4242", "--"]);
}

#[test]
fn the_swap_from_0_1_0_adds_openmp_and_keeps_everything_else() {
    let root = Temp::new();
    let layout = windows_layout(&root.0);
    let app = root.0.join("app");
    folder_0_1_0(&app);
    let before = state(&app);
    let work = app.join(".update");
    zip_0_2_0(&root.0.join("new.zip"));
    let staged = updater::stage_package(&root.0.join("new.zip"), &work, &RS_PACKAGE_0_1_0).unwrap();
    let hooks = Hooks { stamp: Some("20261120-120000".into()), ..Hooks::default() };
    let mut log: Vec<u8> = Vec::new();
    let error = updater::finish_package(&app, &staged, 0, Some(&mut log), &layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "");
    for name in updater::RS_ITEMS {
        assert_eq!(std::fs::read(app.join(name)).unwrap(), bytes_of(name, "0.2.0"), "{name} is the new one");
    }
    let previous = work.join("previous-20261120-120000");
    let mut kept: Vec<String> =
        std::fs::read_dir(&previous).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    kept.sort();
    let mut old: Vec<String> = RS_ITEMS_0_1_0.iter().map(|s| s.to_string()).collect();
    old.sort();
    assert_eq!(kept, old, "exactly the five old items are kept; vcomp140.dll had nothing to move");
    for name in RS_ITEMS_0_1_0 {
        assert_eq!(std::fs::read(previous.join(name)).unwrap(), bytes_of(name, "0.1.0"));
        assert_eq!(
            std::fs::metadata(previous.join(name)).unwrap().modified().unwrap(),
            before[name].as_ref().unwrap().1
        );
    }
    let after = state(&app);
    for (path, value) in &before {
        if !updater::RS_ITEMS.contains(&path.as_str()) {
            assert_eq!(after.get(path), Some(value), "{path} is untouched");
        }
    }
    let mut names: Vec<&String> = after.keys().collect();
    names.retain(|n| !before.contains_key(*n));
    assert_eq!(names, ["vcomp140.dll"], "the only new entry of the folder");
    let log = String::from_utf8(log).unwrap();
    assert!(
        log.contains("previous version saved to previous-20261120-120000: ['GrainyRS.exe', 'grainy-cli.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.txt']"),
        "{log}"
    );
    assert!(
        log.contains("new version copied: ['GrainyRS.exe', 'grainy-cli.exe', 'vcomp140.dll', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.txt']"),
        "{log}"
    );
}

/// A copy that writes half of the file and then fails (a full disk, a scanner holding the target).
fn half_copy(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    let bytes = std::fs::read(source)?;
    std::fs::write(target, &bytes[..bytes.len() / 2])?;
    Err(std::io::Error::other("disk full"))
}

fn copy_failing_on_openmp(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    if source.file_name().is_some_and(|n| n == "vcomp140.dll") {
        return half_copy(source, target);
    }
    updater::copy_file(source, target)
}

fn copy_failing_on_readme(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    if source.file_name().is_some_and(|n| n == "README.txt") {
        return half_copy(source, target);
    }
    updater::copy_file(source, target)
}

#[test]
fn a_swap_from_0_1_0_that_fails_halfway_restores_it_byte_for_byte() {
    // Failing on the added file itself, and on the last file (everything else, the added file too, already copied).
    type Copy = fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>;
    let cases: [(Copy, &str); 2] = [(copy_failing_on_openmp, "vcomp140.dll"), (copy_failing_on_readme, "README.txt")];
    for (failing, name) in cases {
        let root = Temp::new();
        let layout = windows_layout(&root.0);
        let app = root.0.join("app");
        folder_0_1_0(&app);
        let before = state(&app);
        let work = app.join(".update");
        zip_0_2_0(&root.0.join("new.zip"));
        let staged = updater::stage_package(&root.0.join("new.zip"), &work, &RS_PACKAGE_0_1_0).unwrap();
        let hooks = Hooks { stamp: Some("20261120-120000".into()), copy_file: failing, ..Hooks::default() };
        let mut log: Vec<u8> = Vec::new();
        let error = updater::finish_package(&app, &staged, 0, Some(&mut log), &layout, &hooks, &RS_PACKAGE);
        assert_eq!(error, "disk full", "failing on {name}");
        assert_eq!(state(&app), before, "failing on {name}: the 0.1.0 folder, byte for byte, with its own files");
        assert!(!app.join("vcomp140.dll").exists(), "the added file is removed again");
        // What was copied went aside, the half-written file too; the staged copy is still there for a retry.
        let failed = work.join("failed-20261120-120000");
        assert_eq!(std::fs::read(failed.join(name)).unwrap().len(), bytes_of(name, "0.2.0").len() / 2);
        assert_eq!(std::fs::read(failed.join("GrainyRS.exe")).unwrap(), bytes_of("GrainyRS.exe", "0.2.0"));
        assert!(staged.join("GrainyRS.exe").is_file());
        let log = String::from_utf8(log).unwrap();
        assert!(log.contains("failed: OSError('disk full'); restoring"), "{log}");
    }
}

#[test]
fn a_0_2_0_zip_without_the_program_or_with_an_escaping_path_is_refused() {
    let good: Vec<(String, Vec<u8>)> =
        updater::RS_ITEMS.iter().map(|name| (format!("GrainyRS/{name}"), bytes_of(name, "0.2.0"))).collect();
    let without_program: Vec<(String, Vec<u8>)> =
        good.iter().filter(|(n, _)| n != "GrainyRS/GrainyRS.exe").cloned().collect();
    let with = |extra: &str| {
        let mut entries = good.clone();
        entries.push((extra.to_string(), b"evil".to_vec()));
        entries
    };
    let cases = [
        ("no program", without_program),
        ("parent", with("GrainyRS/../vcomp140.dll")),
        ("backslash parent", with("GrainyRS\\..\\..\\vcomp140.dll")),
        ("absolute", with("/vcomp140.dll")),
        ("drive", with("C:/Windows/vcomp140.dll")),
        ("other top folder", with("vcomp140.dll")),
    ];
    for (label, entries) in cases {
        for package in [&RS_PACKAGE, &RS_PACKAGE_0_1_0] {
            let root = Temp::new();
            let work = root.0.join("app").join(".update");
            let zip = root.0.join("bad.zip");
            zip_bytes(&zip, &entries);
            let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
            assert!(updater::check_package_names(&names, package).is_err(), "{label}: the names");
            let error = updater::stage_package(&zip, &work, package).unwrap_err();
            assert_eq!(error.to_string(), "업데이트 파일의 구조가 올바르지 않습니다.", "{label}");
            assert!(!work.join("new").exists(), "{label}: nothing staged");
            assert!(!root.0.join("vcomp140.dll").exists() && !root.0.join("app").join("vcomp140.dll").exists());
        }
    }
}
