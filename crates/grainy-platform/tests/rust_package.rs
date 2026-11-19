//! The Rust build's release zip (`GrainyRS/` with `GrainyRS.exe`, `grainy-cli.exe`, `LICENSE`,
//! `THIRD_PARTY_NOTICES.txt`, `README.txt`): name checks, unpacking, the start command and the swap with the
//! oracle's safety (previous version kept, restored on failure, other files of the folder untouched). Generated
//! zips in temporary folders; nothing is started.

mod common;

use common::*;
use grainy_platform::updater::{self, Hooks, RS_PACKAGE};

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

fn broken_copy(_: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::other("copy failed"))
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
    // A copy that fails halfway (an item that is a folder goes through copy_tree, which breaks here): the new files
    // go aside and the previous version is put back.
    let staged = root.0.join("staged");
    put(&staged, &[("GrainyRS.exe".into(), "new".into()), ("LICENSE".into(), "new licence".into())]);
    std::fs::create_dir_all(staged.join("README.txt")).unwrap();
    let hooks = Hooks { copy_tree: broken_copy, ..hooks };
    let error = updater::finish_package(&app, &staged, 0, None, &layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "copy failed");
    assert_eq!(std::fs::read_to_string(app.join("GrainyRS.exe")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(app.join("LICENSE")).unwrap(), "old licence");
    assert!(!app.join("README.txt").exists());
    assert!(work.join("failed-20260101-000000").join("GrainyRS.exe").is_file(), "the new files are kept aside");
}
