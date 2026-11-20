//! Tests of behaviour the oracle's goldens cannot show (other platforms, time, safety) and of small helpers.

use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use grainy_library::file_access::{guide_text, Guide, GuideStep};
use grainy_library::folder_locations::{directory_identity, rebase};
use grainy_library::folder_sync::{FolderSync, SyncHost};
use grainy_library::fsops::{copy_new_verified, move_dir, move_file};
use grainy_library::pathx;
use grainy_library::quick_export::export_concurrency_here;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("grainy-lib-unit-").tempdir().unwrap()
}

#[test]
fn access_guide_is_shown_once_and_waits_for_other_dialogs() {
    let mut guide = Guide::default();
    assert_eq!(guide.request(true), GuideStep::RetryLater);
    assert_eq!(guide.request(true), GuideStep::Skip, "one retry waits at a time");
    guide.retry_fired();
    assert_eq!(guide.request(false), GuideStep::Show);
    assert_eq!(guide.request(false), GuideStep::Skip, "once per session");
    let mac = guide_text("/Volumes/Cards", true);
    assert!(mac.settings_button.is_some() && mac.informative.ends_with("/Volumes/Cards"));
    let other = guide_text("", false);
    assert!(other.settings_button.is_none() && !other.informative.contains("\n\n위치"));
}

#[test]
fn mac_and_posix_paths_follow_the_oracle() {
    let mac = Platform::Mac;
    assert_eq!(pathx::norm_str(mac, "/Users//kim/./Photos/"), "/Users/kim/Photos");
    assert_eq!(pathx::norm_str(mac, "//server/share"), "//server/share");
    assert_eq!(pathx::norm_str(mac, "///x"), "/x");
    assert_eq!(pathx::parent(mac, "/a/b"), "/a");
    assert_eq!(pathx::parent(mac, "/a"), "/");
    assert_eq!(pathx::parents(mac, "/a/b/c"), ["/a/b", "/a", "/"]);
    assert!(pathx::same(mac, "/A/b", "/A/b") && !pathx::same(mac, "/A/b", "/a/b"), "PosixPath equality is exact");
    assert!(pathx::same(Platform::Windows, "C:/A/b", "c:\\a\\B"));
    assert_eq!(pathx::stem_suffix("a.tar.gz"), ("a.tar".into(), ".gz".into()));
    assert_eq!(pathx::stem_suffix(".hidden"), (".hidden".into(), String::new()));
    assert_eq!(pathx::stem_suffix("a."), ("a.".into(), String::new()));
    assert_eq!(pathx::splitext_ext("x/.hidden.jpg"), ".jpg");
    assert_eq!(pathx::splitext_ext("x/.jpg"), "");
    assert_eq!(pathx::splitext_ext("a..jpg"), ".jpg");
    assert_eq!(pathx::with_suffix(mac, "/a/b.jpg", ".xmp"), "/a/b.xmp");
    assert_eq!(pathx::with_suffix(mac, "/a/b", ".xmp"), "/a/b.xmp");
    // folder rebasing compares the way the catalogue does: NFC and case-insensitive on macOS
    let decomposed = "/Users/kim/\u{1112}\u{1161}\u{11ab}/x";
    assert_eq!(
        rebase(mac, decomposed, "/USERS/kim/\u{d55c}", "/Volumes/New").unwrap(),
        "/Volumes/New/x",
        "decomposed Korean and different case still count as inside the folder"
    );
    assert_eq!(rebase(Platform::Posix, "/a/B/x", "/a/b", "/n").unwrap(), "/a/B/x", "POSIX is case-sensitive");
    assert_eq!(rebase(Platform::Posix, "/a/b/x", "/a/b", "/n").unwrap(), "/n/x");
}

#[test]
fn identity_survives_a_rename_and_files_have_none() {
    let dir = temp();
    let folder = dir.path().join("a");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(dir.path().join("f.txt"), b"x").unwrap();
    let before = directory_identity(&folder).expect("a folder has an identity");
    assert!(before[0] != 0 || before[1] != 0);
    if cfg!(windows) {
        assert!(before[2] > 0, "creation time in ns");
    } else {
        assert_eq!(before[2], 0, "the oracle has no st_birthtime_ns outside Windows");
    }
    let renamed = dir.path().join("b");
    std::fs::rename(&folder, &renamed).unwrap();
    assert_eq!(directory_identity(&renamed), Some(before));
    assert_eq!(directory_identity(&dir.path().join("f.txt")), None);
    assert_eq!(directory_identity(&dir.path().join("missing")), None);
}

#[test]
fn moves_and_copies_never_overwrite() {
    let dir = temp();
    let (a, b) = (dir.path().join("a.jpg"), dir.path().join("b.jpg"));
    std::fs::write(&a, b"original").unwrap();
    std::fs::write(&b, b"precious").unwrap();
    assert!(move_file(&a, &b).is_err());
    assert!(copy_new_verified(&a, &b).is_err());
    assert_eq!(std::fs::read(&b).unwrap(), b"precious");
    assert_eq!(std::fs::read(&a).unwrap(), b"original", "a refused move leaves the source");
    let folder = dir.path().join("f");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("x"), b"1").unwrap();
    let other = dir.path().join("g");
    std::fs::create_dir(&other).unwrap();
    assert!(move_dir(&folder, &other).is_err());
    assert!(folder.join("x").exists());
    // a copy is verified and keeps the content; the source stays
    let c = dir.path().join("c.jpg");
    copy_new_verified(&a, &c).unwrap();
    assert_eq!(std::fs::read(&c).unwrap(), b"original");
    assert!(a.exists());
}

#[test]
fn failed_copy_never_removes_a_destination_it_did_not_create() {
    let dir = temp();
    let destination = dir.path().join("existing.jpg");
    std::fs::write(&destination, b"original photo").unwrap();
    assert!(copy_new_verified(&dir.path().join("missing.jpg"), &destination).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"original photo");
    assert!(copy_new_verified(dir.path(), &destination).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"original photo");
}

#[test]
fn sidecar_writes_leave_foreign_temporary_files_alone() {
    let dir = temp();
    let sidecar = dir.path().join("photo.xmp");
    let foreign = dir.path().join("photo.xmp.luma-temp");
    std::fs::write(&foreign, b"unrelated file").unwrap();
    let mut photo = serde_json::json!({"rating": 1, "settings": {}, "keywords": "test"});
    grainy_library::xmp::write_sidecar(&photo, &sidecar).unwrap();
    photo["rating"] = 4.into();
    grainy_library::xmp::write_sidecar(&photo, &sidecar).unwrap();
    assert_eq!(grainy_library::xmp::read_sidecar(&sidecar).unwrap().rating, Some(4));
    assert_eq!(std::fs::read(&foreign).unwrap(), b"unrelated file");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn folder_sync_timers_follow_the_oracle() {
    let dir = temp();
    let catalog = Catalog::new(dir.path().join("catalog")).unwrap();
    let clock = Arc::new(AtomicU64::new(100.0f64.to_bits()));
    let shared = Arc::clone(&clock);
    let mut sync = FolderSync::new(&catalog, move || f64::from_bits(shared.load(Ordering::SeqCst))).unwrap();
    let host = SyncHost::default();
    let set = |t: f64| clock.store(t.to_bits(), Ordering::SeqCst);
    assert!(sync.poll(&catalog, &host).unwrap().is_none(), "nothing before the 4 s start delay");
    set(103.9);
    assert!(sync.poll(&catalog, &host).unwrap().is_none());
    set(104.0);
    assert!(sync.poll(&catalog, &host).unwrap().is_some(), "first check 4 s after start");
    sync.failed();
    set(104.5);
    assert!(sync.poll(&catalog, &host).unwrap().is_none());
    set(220.0);
    assert!(sync.poll(&catalog, &host).unwrap().is_some(), "then every two minutes");
    sync.failed();
    // returning to the app within 20 s of the last check does nothing, later it checks
    set(230.0);
    assert!(sync.activated(&catalog, &host, true).unwrap().is_none());
    set(241.0);
    assert!(sync.activated(&catalog, &host, false).unwrap().is_none());
    assert!(sync.activated(&catalog, &host, true).unwrap().is_some());
    sync.failed();
    sync.shutdown(&catalog).unwrap();
    set(1000.0);
    assert!(sync.poll(&catalog, &host).unwrap().is_none(), "no timers after shutdown");
}

#[test]
fn concurrency_for_this_machine_is_within_bounds() {
    let record = serde_json::json!({"info": {"width": 6000, "height": 4000}});
    assert_eq!(export_concurrency_here(std::slice::from_ref(&record)).unwrap(), 1);
    let n = export_concurrency_here(&[record.clone(), record]).unwrap();
    assert!((1..=4).contains(&n));
}

#[test]
fn error_types_name_the_python_exceptions() {
    use grainy_library::Error;
    assert_eq!(Error::value("x").python_type(), "ValueError");
    assert_eq!(Error::Type("KeyError: 'a'".into()).python_type(), "KeyError");
    assert_eq!(Error::Type("IndexError: out".into()).python_type(), "IndexError");
    assert_eq!(Error::Type("boom".into()).python_type(), "TypeError");
    let io = std::io::Error::from(std::io::ErrorKind::NotFound);
    assert_eq!(Error::os(&io, Path::new("x"), true).to_string(), "[Errno 2] No such file or directory: 'x'");
}

#[test]
fn import_retries_past_an_abandoned_partial_without_touching_it() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("card.raw");
    let target = dir.path().join("copy.raw");
    let partial = dir.path().join("copy.raw.grainy-part");
    std::fs::write(&source, b"generated photo").unwrap();
    std::fs::write(&partial, b"another operation").unwrap();
    grainy_library::import_copy::copy_verified(
        grainy_catalog::folders::Platform::native(),
        source.to_str().unwrap(),
        target.to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read(target).unwrap(), b"generated photo");
    assert_eq!(std::fs::read(partial).unwrap(), b"another operation");
}

#[test]
fn move_updates_the_stored_spelling_of_the_photo_path() {
    let dir = temp();
    let db = Catalog::new(dir.path().join("catalog")).unwrap();
    let photo = dir.path().join("image.jpg");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(&photo, b"photo").unwrap();
    let spelling = dir.path().join("sub").join("..").join("image.jpg").display().to_string();
    let target = dir.path().join("renamed.jpg");
    let id = db.add(&spelling, None, None, true).unwrap();
    db.connection()
        .execute("UPDATE photos SET path=? WHERE id=?", grainy_catalog::rusqlite::params![spelling, id])
        .unwrap();
    grainy_library::library::move_files(&db, &[(spelling, target.display().to_string())]).unwrap();
    let stored = db.photo(id).unwrap().unwrap()["path"].as_str().unwrap().to_owned();
    assert_eq!(std::fs::canonicalize(stored).unwrap(), std::fs::canonicalize(target).unwrap());
}
