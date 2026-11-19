//! The Rust build's macOS update (HANDOFF4): `GrainyRS-<version>-macos-universal.dmg` holding `Grainy RS.app`.
//! Everything that is plain logic runs on every system: the bundle constants, the `Info.plist` reader (XML and
//! binary), the bundle checks, the staging of a disk image with fake `hdiutil`/`ditto` tools (attach, checks,
//! copy, the detach that always follows, `-force` as the second try), the start command, the swap of the whole
//! bundle in temporary folders and the tidying of the work folder. The `real` module at the end (macOS only)
//! makes small disk images with `hdiutil create` and stages them with the real tools. Nothing is started and
//! nothing touches the network.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use common::*;
use grainy_platform::updater::{self, plist, DiskImageTools, Hooks, RS_MAC, RS_PACKAGE};

const BAD: &str = "업데이트 파일의 구조가 올바르지 않습니다.";

/// An `Info.plist` as Xcode and the release scripts write it: a DOCTYPE, other keys before and after, and a nested
/// dictionary holding a `CFBundleIdentifier` of its own (a document type), which is not the bundle's.
fn xml_plist(identifier: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleIdentifier</key>
			<string>not.the.bundle</string>
		</dict>
	</array>
	<key>CFBundleExecutable</key>
	<string>GrainyRS</string>
	<!-- the identifier -->
	<key>CFBundleIdentifier</key>
	<string>{identifier}</string>
	<key>CFBundleName</key>
	<string>Grainy RS</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#
    )
}

fn object_header(kind: u8, length: usize) -> Vec<u8> {
    assert!(length < 256);
    if length < 15 {
        vec![(kind << 4) | length as u8]
    } else {
        vec![(kind << 4) | 0x0f, 0x10, length as u8]
    }
}

fn string_object(text: &str, utf16: bool) -> Vec<u8> {
    if utf16 {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut object = object_header(0x6, units.len());
        units.iter().for_each(|unit| object.extend(unit.to_be_bytes()));
        object
    } else {
        let mut object = object_header(0x5, text.len());
        object.extend(text.as_bytes());
        object
    }
}

/// A binary property list (`bplist00`) of one dictionary of strings, keys as UTF-16 strings when `utf16`.
fn binary_plist(pairs: &[(&str, &str)], utf16: bool) -> Vec<u8> {
    let count = pairs.len();
    let mut dict = object_header(0xd, count);
    dict.extend((0..count).map(|i| (1 + i) as u8));
    dict.extend((0..count).map(|i| (1 + count + i) as u8));
    let mut objects = vec![dict];
    objects.extend(pairs.iter().map(|(key, _)| string_object(key, utf16)));
    objects.extend(pairs.iter().map(|(_, value)| string_object(value, false)));
    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::new();
    for object in &objects {
        offsets.push(out.len());
        out.extend(object);
    }
    let table = out.len();
    let wide = table > 255;
    for offset in offsets {
        if wide {
            out.extend((offset as u16).to_be_bytes());
        } else {
            out.push(offset as u8);
        }
    }
    out.extend([0u8; 6]);
    out.extend([if wide { 2 } else { 1 }, 1]);
    out.extend((objects.len() as u64).to_be_bytes());
    out.extend(0u64.to_be_bytes());
    out.extend((table as u64).to_be_bytes());
    out
}

/// A fake `Grainy RS.app` at `at`: the program, the `Info.plist` and a resource.
fn make_bundle(at: &Path, program: &[u8], plist: &[u8]) {
    std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
    std::fs::create_dir_all(at.join("Contents/Resources")).unwrap();
    std::fs::write(at.join(updater::RS_PROGRAM), program).unwrap();
    std::fs::write(at.join("Contents/Info.plist"), plist).unwrap();
    std::fs::write(at.join("Contents/Resources/AppIcon.icns"), b"icon").unwrap();
}

fn good_bundle(at: &Path, program: &[u8]) {
    make_bundle(at, program, xml_plist(updater::RS_BUNDLE_ID).as_bytes());
}

// ---- constants -----------------------------------------------------------------------------------------------

#[test]
fn the_rust_bundle_is_its_own_app() {
    assert_eq!(updater::RS_BUNDLE, "Grainy RS.app");
    assert_eq!(updater::RS_PROGRAM, "Contents/MacOS/GrainyRS");
    assert_eq!(updater::RS_BUNDLE_ID, "io.github.pastorius9.grainyrs");
    assert_eq!(RS_PACKAGE.mac, RS_MAC);
    assert_eq!(
        (RS_MAC.name, RS_MAC.program, RS_MAC.identifier, RS_MAC.data),
        ("Grainy RS.app", "Contents/MacOS/GrainyRS", Some("io.github.pastorius9.grainyrs"), "GrainyRS")
    );
    // The oracle's bundle stays for its goldens.
    assert_eq!((updater::BUNDLE, updater::PROGRAM), ("Grainy.app", "Contents/MacOS/Grainy"));
    assert_eq!(updater::ORACLE_PACKAGE.mac, updater::ORACLE_MAC);
    assert_eq!(updater::ORACLE_MAC.identifier, None);
}

#[test]
fn the_app_and_work_folders_on_a_mac() {
    let root = Temp::new();
    let layout = mac_layout(&root.0, "arm64");
    let app = PathBuf::from("/Applications/Grainy RS.app");
    let exe = app.join(updater::RS_PROGRAM);
    assert_eq!(updater::app_folder(&exe, &layout), Some(app.clone()));
    assert_eq!(updater::app_folder(Path::new("/usr/local/bin/GrainyRS"), &layout), None, "not in a bundle");
    let work = updater::work_folder_for(&app, &layout, &RS_PACKAGE);
    assert_eq!(work, root.0.join("Library").join("Application Support").join("GrainyRS").join("Update"));
    // The Python Grainy's work folder is another one, and stays the oracle's.
    assert_eq!(updater::work_folder(&app, &layout), root.0.join("Library/Application Support/Grainy/Update"));
    assert_eq!(updater::work_folder_for(&app, &layout, &updater::ORACLE_PACKAGE), updater::work_folder(&app, &layout));
    // Windows keeps `.update` in the program folder.
    let windows = windows_layout(&root.0);
    assert_eq!(
        updater::work_folder_for(&root.0.join("app"), &windows, &RS_PACKAGE),
        root.0.join("app").join(".update")
    );
}

#[test]
fn the_staged_bundles_own_program_finishes_the_update() {
    let root = Temp::new();
    let layout = mac_layout(&root.0, "x86_64");
    let app = PathBuf::from("/Applications/Grainy RS.app");
    let staged = updater::work_folder_for(&app, &layout, &RS_PACKAGE).join("new").join("Grainy RS.app");
    let (program, command, cwd) =
        updater::launch_command_for(&staged, &app, &["--memory".into()], 77, &layout, &RS_PACKAGE);
    assert_eq!(program, staged.join("Contents/MacOS/GrainyRS"));
    assert_eq!(cwd, staged.parent().unwrap());
    assert_eq!(command, ["--finish-update", &app.display().to_string(), "--wait-pid", "77", "--", "--memory"]);
    // The oracle's command is unchanged.
    let python = staged.with_file_name("Grainy.app");
    assert_eq!(updater::launch_command(&python, &app, &[], 77, &layout).0, python.join("Contents/MacOS/Grainy"));
}

// ---- Info.plist -----------------------------------------------------------------------------------------------

#[test]
fn the_identifier_is_read_from_xml_and_binary_lists() {
    assert_eq!(
        plist::bundle_identifier(xml_plist(updater::RS_BUNDLE_ID).as_bytes()).as_deref(),
        Some(updater::RS_BUNDLE_ID)
    );
    let bom = format!("\u{feff}{}", xml_plist("a&amp;b"));
    assert_eq!(plist::bundle_identifier(bom.as_bytes()).as_deref(), Some("a&b"), "a BOM and an entity");
    let cdata = xml_plist("x").replace("<string>x</string>", "<string>io.<![CDATA[github]]>.x</string>");
    assert_eq!(plist::bundle_identifier(cdata.as_bytes()).as_deref(), Some("io.github.x"));
    let pairs = [
        ("CFBundleName", "Grainy RS"),
        ("CFBundleIdentifier", updater::RS_BUNDLE_ID),
        ("CFBundleExecutable", "GrainyRS"),
    ];
    for utf16 in [false, true] {
        let bytes = binary_plist(&pairs, utf16);
        assert_eq!(plist::bundle_identifier(&bytes).as_deref(), Some(updater::RS_BUNDLE_ID), "utf16 keys: {utf16}");
        assert_eq!(plist::top_level_string(&bytes, "CFBundleExecutable").as_deref(), Some("GrainyRS"));
        assert_eq!(plist::top_level_string(&bytes, "Missing"), None);
        // Every truncation is refused, never a panic.
        for end in 0..bytes.len() {
            let _ = plist::bundle_identifier(&bytes[..end]);
        }
    }
    // A list wide enough for two-byte offsets.
    let long = "x".repeat(200);
    let bytes = binary_plist(&[("A", &long), ("B", &long), ("CFBundleIdentifier", "wide")], false);
    assert_eq!(plist::bundle_identifier(&bytes).as_deref(), Some("wide"));
}

#[test]
fn a_list_without_the_bundles_identifier_gives_none() {
    let cases = [
        // Only in a nested dictionary.
        xml_plist("x").replace("<key>CFBundleIdentifier</key>\n\t<string>x</string>", ""),
        // Not a string.
        xml_plist("x").replace("<string>x</string>", "<integer>1</integer>"),
        // Not a property list.
        "<html><dict><key>CFBundleIdentifier</key><string>x</string></dict></html>".to_string(),
        "<plist><array><string>CFBundleIdentifier</string></array></plist>".to_string(),
        "<plist><dict><key>CFBundleIdentifier</key>".to_string(),
        "not xml".to_string(),
        String::new(),
    ];
    for (index, text) in cases.iter().enumerate() {
        assert_eq!(plist::bundle_identifier(text.as_bytes()), None, "case {index}");
    }
    assert_eq!(plist::bundle_identifier(&binary_plist(&[("CFBundleName", "x")], false)), None);
    assert_eq!(plist::bundle_identifier(&[0xff, 0xfe, 0x00]), None);
}

// ---- the bundle checks ------------------------------------------------------------------------------------------

#[test]
fn a_bundle_is_checked_before_and_after_the_copy() {
    let root = Temp::new();
    let ok = root.0.join("ok").join("Grainy RS.app");
    good_bundle(&ok, b"program");
    assert!(updater::check_bundle_folder(&ok, &RS_MAC).is_ok());
    let binary = root.0.join("binary").join("Grainy RS.app");
    make_bundle(&binary, b"program", &binary_plist(&[("CFBundleIdentifier", updater::RS_BUNDLE_ID)], true));
    assert!(updater::check_bundle_folder(&binary, &RS_MAC).is_ok(), "a binary Info.plist");
    let refused = |label: &str, path: &Path| {
        let error = updater::check_bundle_folder(path, &RS_MAC).unwrap_err();
        assert_eq!(error.to_string(), BAD, "{label}");
    };
    // The Python Grainy's identifier: the two apps must never take each other's place.
    let python = root.0.join("python").join("Grainy RS.app");
    make_bundle(&python, b"program", xml_plist("io.github.pastorius9.grainy").as_bytes());
    refused("the Python Grainy's identifier", &python);
    let other = root.0.join("other").join("Grainy RS.app");
    make_bundle(&other, b"program", xml_plist("io.github.pastorius9.grainyrs.other").as_bytes());
    refused("another identifier", &other);
    let no_program = root.0.join("no-program").join("Grainy RS.app");
    good_bundle(&no_program, b"program");
    std::fs::remove_file(no_program.join(updater::RS_PROGRAM)).unwrap();
    refused("no program", &no_program);
    let folder_program = root.0.join("folder-program").join("Grainy RS.app");
    good_bundle(&folder_program, b"program");
    std::fs::remove_file(folder_program.join(updater::RS_PROGRAM)).unwrap();
    std::fs::create_dir(folder_program.join(updater::RS_PROGRAM)).unwrap();
    refused("a folder named like the program", &folder_program);
    let no_plist = root.0.join("no-plist").join("Grainy RS.app");
    good_bundle(&no_plist, b"program");
    std::fs::remove_file(no_plist.join("Contents/Info.plist")).unwrap();
    refused("no Info.plist", &no_plist);
    let big = root.0.join("big").join("Grainy RS.app");
    good_bundle(&big, b"program");
    let mut text = xml_plist(updater::RS_BUNDLE_ID);
    text.push_str(&" ".repeat(1 << 20));
    std::fs::write(big.join("Contents/Info.plist"), text).unwrap();
    refused("an Info.plist over 1 MiB", &big);
    let file = root.0.join("file").join("Grainy RS.app");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"not a folder").unwrap();
    refused("a file named like the bundle", &file);
    refused("nothing there", &root.0.join("missing").join("Grainy RS.app"));
    // The oracle's bundle has no identifier to check.
    let oracle = root.0.join("oracle").join("Grainy.app");
    make_bundle(&oracle, b"program", b"anything");
    std::fs::create_dir_all(oracle.join("Contents/MacOS")).unwrap();
    std::fs::write(oracle.join(updater::PROGRAM), b"python").unwrap();
    assert!(updater::check_bundle_folder(&oracle, &updater::ORACLE_MAC).is_ok());
}

#[cfg(unix)]
#[test]
fn a_linked_bundle_or_program_is_refused() {
    let root = Temp::new();
    let real = root.0.join("real").join("Grainy RS.app");
    good_bundle(&real, b"program");
    let linked = root.0.join("volume").join("Grainy RS.app");
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    assert_eq!(updater::check_bundle_folder(&linked, &RS_MAC).unwrap_err().to_string(), BAD);
    let inner = root.0.join("inner").join("Grainy RS.app");
    good_bundle(&inner, b"program");
    std::fs::remove_file(inner.join(updater::RS_PROGRAM)).unwrap();
    std::os::unix::fs::symlink("/bin/sh", inner.join(updater::RS_PROGRAM)).unwrap();
    assert_eq!(updater::check_bundle_folder(&inner, &RS_MAC).unwrap_err().to_string(), BAD, "a linked program");
    let contents = root.0.join("contents").join("Grainy RS.app");
    std::fs::create_dir_all(&contents).unwrap();
    std::os::unix::fs::symlink(real.join("Contents"), contents.join("Contents")).unwrap();
    assert_eq!(updater::check_bundle_folder(&contents, &RS_MAC).unwrap_err().to_string(), BAD, "a linked Contents");
}

// ---- staging a disk image with fake tools -------------------------------------------------------------------------

/// Fake `hdiutil` and `ditto`: attaching copies the image's prepared content into the mount folder, detaching
/// empties it again; every call is recorded.
struct FakeTools {
    volume: PathBuf,
    calls: Mutex<Vec<String>>,
    attach_fails: bool,
    attach_mounts_anyway: bool,
    detach_failures: AtomicUsize,
    copy: fn(&Path, &Path) -> std::io::Result<()>,
    attached: Mutex<Option<PathBuf>>,
}

impl FakeTools {
    fn new(volume: &Path) -> FakeTools {
        FakeTools {
            volume: volume.to_path_buf(),
            calls: Mutex::new(Vec::new()),
            attach_fails: false,
            attach_mounts_anyway: false,
            detach_failures: AtomicUsize::new(0),
            copy: updater::copy_tree,
            attached: Mutex::new(None),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn mount(&self, mount: &Path) {
        assert_eq!(std::fs::read_dir(mount).unwrap().count(), 0, "mounted on an empty folder");
        for entry in std::fs::read_dir(&self.volume).unwrap() {
            let path = entry.unwrap().path();
            let target = mount.join(path.file_name().unwrap());
            if path.is_dir() {
                updater::copy_tree(&path, &target).unwrap();
            } else {
                std::fs::copy(&path, &target).unwrap();
            }
        }
        *self.attached.lock().unwrap() = Some(mount.to_path_buf());
    }
}

impl DiskImageTools for FakeTools {
    fn attach(&self, image: &Path, mount: &Path) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(format!("attach {}", image.file_name().unwrap().to_string_lossy()));
        assert!(mount.file_name().unwrap().to_string_lossy().starts_with("mount-"));
        if self.attach_fails {
            if self.attach_mounts_anyway {
                self.mount(mount);
            }
            return Err(std::io::Error::other("hdiutil: attach failed - image not recognized"));
        }
        self.mount(mount);
        Ok(())
    }

    fn detach(&self, mount: &Path, force: bool) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(if force { "detach -force" } else { "detach" }.into());
        assert_eq!(self.attached.lock().unwrap().as_deref(), Some(mount), "only the attached volume is detached");
        if self.detach_failures.load(Ordering::SeqCst) > 0 {
            self.detach_failures.fetch_sub(1, Ordering::SeqCst);
            return Err(std::io::Error::other("hdiutil: couldn't unmount - Resource busy"));
        }
        for entry in std::fs::read_dir(mount)? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
        *self.attached.lock().unwrap() = None;
        Ok(())
    }

    fn copy(&self, source: &Path, target: &Path) -> std::io::Result<()> {
        self.calls.lock().unwrap().push("copy".into());
        assert!(target.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("extract-"));
        (self.copy)(source, target)
    }

    fn mounted(&self, mount: &Path) -> bool {
        self.attached.lock().unwrap().as_deref() == Some(mount)
    }
}

fn names_in(work: &Path, prefix: &str) -> Vec<String> {
    std::fs::read_dir(work)
        .map(|read| {
            read.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default()
}

/// A temporary root with the disk image's content (`volume/`), the image file and the work folder.
struct Stage {
    root: Temp,
    volume: PathBuf,
    image: PathBuf,
    work: PathBuf,
}

fn stage_setup() -> Stage {
    let root = Temp::new();
    let volume = root.0.join("volume");
    good_bundle(&volume.join("Grainy RS.app"), b"GrainyRS 0.2.0");
    // A disk image usually also holds a link to /Applications; a plain file stands for it here.
    std::fs::write(volume.join("Applications"), b"").unwrap();
    let image = root.0.join("download").join("GrainyRS-0.2.0-macos-universal.dmg");
    std::fs::create_dir_all(image.parent().unwrap()).unwrap();
    std::fs::write(&image, b"a disk image").unwrap();
    let work = root.0.join("Update");
    Stage { root, volume, image, work }
}

#[test]
fn a_disk_image_is_attached_checked_copied_and_always_detached() {
    let s = stage_setup();
    let tools = FakeTools::new(&s.volume);
    let staged = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap();
    assert_eq!(staged, s.work.join("new").join("Grainy RS.app"));
    assert_eq!(std::fs::read(staged.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.2.0");
    assert!(staged.join("Contents/Resources/AppIcon.icns").is_file());
    assert!(!s.work.join("new").join("Applications").exists(), "only the bundle is copied");
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "copy", "detach"]);
    assert!(names_in(&s.work, "mount-").is_empty(), "the empty mount folder is removed");
    assert!(names_in(&s.work, "extract-").is_empty(), "the copy became work/new");
    // A second staging before the first was tidied is refused before anything is attached.
    let again = FakeTools::new(&s.volume);
    let error = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &again).unwrap_err();
    assert!(error.to_string().contains("이전 업데이트가 정리되지 않았습니다"), "{error}");
    assert!(again.calls().is_empty());
    drop(s.root);
}

#[test]
fn a_disk_image_that_fails_a_check_is_detached_and_nothing_is_staged() {
    type Spoil = fn(&Path);
    let cases: [(&str, Spoil); 5] = [
        ("the Python Grainy's identifier", |volume| {
            std::fs::write(volume.join("Grainy RS.app/Contents/Info.plist"), xml_plist("io.github.pastorius9.grainy"))
                .unwrap()
        }),
        ("no program", |volume| std::fs::remove_file(volume.join("Grainy RS.app").join(updater::RS_PROGRAM)).unwrap()),
        ("no Info.plist", |volume| std::fs::remove_file(volume.join("Grainy RS.app/Contents/Info.plist")).unwrap()),
        ("the Python Grainy's bundle", |volume| {
            std::fs::rename(volume.join("Grainy RS.app"), volume.join("Grainy.app")).unwrap()
        }),
        ("an empty volume", |volume| std::fs::remove_dir_all(volume.join("Grainy RS.app")).unwrap()),
    ];
    for (label, spoil) in cases {
        let s = stage_setup();
        spoil(&s.volume);
        let tools = FakeTools::new(&s.volume);
        let error = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap_err();
        assert_eq!(error.to_string(), BAD, "{label}");
        assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "detach"], "{label}");
        assert!(!s.work.join("new").exists(), "{label}: nothing staged");
        assert!(names_in(&s.work, "mount-").is_empty(), "{label}");
    }
}

fn copy_nothing(_: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target.join("Contents"))
}

#[test]
fn a_copy_that_lost_the_program_is_refused() {
    let s = stage_setup();
    let mut tools = FakeTools::new(&s.volume);
    tools.copy = copy_nothing;
    let error = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap_err();
    assert_eq!(error.to_string(), BAD);
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "copy", "detach"]);
    assert!(!s.work.join("new").exists());
}

#[test]
fn a_disk_image_that_does_not_attach_is_reported() {
    let s = stage_setup();
    let mut tools = FakeTools::new(&s.volume);
    tools.attach_fails = true;
    let error = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap_err();
    assert!(matches!(error, updater::UpdateError::Io(_)));
    assert_eq!(error.to_string(), "hdiutil: attach failed - image not recognized");
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg"], "nothing attached, nothing detached");
    assert!(names_in(&s.work, "mount-").is_empty());
    assert!(!s.work.join("new").exists());
    // hdiutil reported a failure but the volume is there: it is still detached.
    let mut tools = FakeTools::new(&s.volume);
    tools.attach_fails = true;
    tools.attach_mounts_anyway = true;
    assert!(updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).is_err());
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "detach"]);
    assert!(names_in(&s.work, "mount-").is_empty());
}

#[test]
fn a_busy_volume_is_detached_with_force_as_the_second_try() {
    let s = stage_setup();
    let tools = FakeTools::new(&s.volume);
    tools.detach_failures.store(1, Ordering::SeqCst);
    let staged = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap();
    assert!(staged.join(updater::RS_PROGRAM).is_file());
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "copy", "detach", "detach -force"]);
    assert!(names_in(&s.work, "mount-").is_empty());
    // A check failure with a busy volume: still both tries, the check's error is the one reported.
    let s = stage_setup();
    std::fs::remove_file(s.volume.join("Grainy RS.app").join(updater::RS_PROGRAM)).unwrap();
    let tools = FakeTools::new(&s.volume);
    tools.detach_failures.store(1, Ordering::SeqCst);
    let error = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap_err();
    assert_eq!(error.to_string(), BAD);
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "detach", "detach -force"]);
}

#[test]
fn a_volume_that_stays_attached_fails_the_staging_and_nothing_inside_is_deleted() {
    let s = stage_setup();
    let tools = FakeTools::new(&s.volume);
    tools.detach_failures.store(2, Ordering::SeqCst);
    let error = updater::stage_disk_image(&s.image, &s.work, &RS_MAC, &tools).unwrap_err();
    assert_eq!(error.to_string(), "hdiutil: couldn't unmount - Resource busy");
    assert_eq!(tools.calls(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "copy", "detach", "detach -force"]);
    assert!(!s.work.join("new").exists(), "not staged");
    let mounts = names_in(&s.work, "mount-");
    assert_eq!(mounts.len(), 1, "the folder of the attached volume stays");
    assert!(s.work.join(&mounts[0]).join("Grainy RS.app").join(updater::RS_PROGRAM).is_file(), "untouched");
}

#[test]
fn the_hdiutil_arguments_are_a_list() {
    let image =
        Path::new("/Users/x/Library/Application Support/GrainyRS/Update/download/GrainyRS-0.2.0-macos-universal.dmg");
    let mount = Path::new("/Users/x/Library/Application Support/GrainyRS/Update/mount-0123");
    let text =
        |list: Vec<std::ffi::OsString>| list.into_iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
    assert_eq!(
        text(updater::attach_arguments(image, mount)),
        [
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-noverify",
            "-mountpoint",
            "/Users/x/Library/Application Support/GrainyRS/Update/mount-0123",
            "/Users/x/Library/Application Support/GrainyRS/Update/download/GrainyRS-0.2.0-macos-universal.dmg",
        ]
    );
    assert_eq!(
        text(updater::detach_arguments(mount, false)),
        ["detach", "/Users/x/Library/Application Support/GrainyRS/Update/mount-0123"]
    );
    assert_eq!(
        text(updater::detach_arguments(mount, true)),
        ["detach", "/Users/x/Library/Application Support/GrainyRS/Update/mount-0123", "-force"]
    );
    // A plain folder is not a mount point: nothing would be detached there.
    let root = Temp::new();
    std::fs::create_dir_all(root.0.join("mount-x")).unwrap();
    assert!(!updater::is_mount_point(&root.0.join("mount-x")));
    assert!(!updater::SystemTools.mounted(&root.0.join("mount-x")));
}

#[test]
fn the_rust_release_is_staged_for_its_system() {
    let s = stage_setup();
    let tools = FakeTools::new(&s.volume);
    let mac = mac_layout(&s.root.0, "arm64");
    let staged = updater::stage_rust(&s.image, &s.work, &mac, &tools).unwrap();
    assert_eq!(staged, s.work.join("new").join("Grainy RS.app"));
    // Windows unpacks its zip; the disk image tools are never asked.
    let windows = windows_layout(&s.root.0);
    let zip = s.root.0.join("GrainyRS-0.2.0-windows-x64.zip");
    let entries: Vec<(String, String)> =
        updater::RS_ITEMS.iter().map(|name| (format!("GrainyRS/{name}"), format!("{name} 0.2.0"))).collect();
    zip_file(&zip, &entries);
    let tools = FakeTools::new(&s.volume);
    let work = s.root.0.join("app").join(".update");
    let staged = updater::stage_rust(&zip, &work, &windows, &tools).unwrap();
    assert_eq!(staged, work.join("new"));
    assert!(staged.join("vcomp140.dll").is_file());
    assert!(tools.calls().is_empty());
}

// ---- the swap of the whole bundle ------------------------------------------------------------------------------

struct Swap {
    root: Temp,
    layout: updater::Layout,
    app: PathBuf,
    work: PathBuf,
    staged: PathBuf,
}

/// `/Applications` with the old `Grainy RS.app` and the Python Grainy's app beside it; the staged new bundle in the
/// Rust build's work folder.
fn swap_setup() -> Swap {
    let root = Temp::new();
    let layout = mac_layout(&root.0, "arm64");
    let applications = root.0.join("Applications");
    let app = applications.join("Grainy RS.app");
    good_bundle(&app, b"GrainyRS 0.1.0");
    std::fs::write(app.join("Contents/Resources/old-only.txt"), b"old").unwrap();
    good_bundle(&applications.join("Grainy.app"), b"the Python Grainy");
    let work = updater::work_folder_for(&app, &layout, &RS_PACKAGE);
    let staged = work.join("new").join("Grainy RS.app");
    good_bundle(&staged, b"GrainyRS 0.2.0");
    Swap { root, layout, app, work, staged }
}

#[test]
fn the_whole_bundle_is_swapped_and_the_previous_one_kept() {
    let s = swap_setup();
    let hooks = Hooks { stamp: Some("20261120-120000".into()), ..Hooks::default() };
    let mut log: Vec<u8> = Vec::new();
    let error = updater::finish_package(&s.app, &s.staged, 0, Some(&mut log), &s.layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "");
    assert_eq!(std::fs::read(s.app.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.2.0");
    assert!(!s.app.join("Contents/Resources/old-only.txt").exists(), "the bundle is replaced as a whole");
    let previous = s.work.join("previous-20261120-120000").join("Grainy RS.app");
    assert_eq!(std::fs::read(previous.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.1.0");
    assert!(previous.join("Contents/Resources/old-only.txt").is_file());
    assert!(!s.staged.exists(), "the staged bundle was moved into place");
    let python = s.root.0.join("Applications").join("Grainy.app");
    assert_eq!(std::fs::read(python.join(updater::RS_PROGRAM)).unwrap(), b"the Python Grainy", "untouched");
    assert!(!s.root.0.join("Library/Application Support/Grainy").exists(), "the Python Grainy's folder is not used");
    let log = String::from_utf8(log).unwrap();
    assert!(log.contains("previous version moved to previous-20261120-120000"), "{log}");
    assert!(log.contains("new version in place"), "{log}");
    // finish_logged_for writes update.log in the Rust build's work folder.
    let s = swap_setup();
    let error = updater::finish_logged_for(&s.app, &s.staged, 0, &s.layout, &RS_PACKAGE);
    assert_eq!(error, "");
    assert!(std::fs::read_to_string(s.work.join("update.log")).unwrap().contains("new version in place"));
}

/// A move that fails for the staged bundle (the second move of the swap).
fn place_failing_from_new(source: &Path, target: &Path) -> std::io::Result<()> {
    if source.parent().and_then(Path::file_name).is_some_and(|n| n == "new") {
        return Err(std::io::Error::other("disk full"));
    }
    updater::place(source, target)
}

#[test]
fn a_failed_bundle_swap_puts_the_previous_bundle_back() {
    let s = swap_setup();
    let hooks = Hooks { stamp: Some("20261120-120000".into()), place: place_failing_from_new, ..Hooks::default() };
    let mut log: Vec<u8> = Vec::new();
    let error = updater::finish_package(&s.app, &s.staged, 0, Some(&mut log), &s.layout, &hooks, &RS_PACKAGE);
    assert_eq!(error, "disk full");
    assert_eq!(std::fs::read(s.app.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.1.0");
    assert!(s.app.join("Contents/Resources/old-only.txt").is_file());
    assert!(s.staged.join(updater::RS_PROGRAM).is_file(), "the staged bundle is still there");
    assert!(String::from_utf8(log).unwrap().contains("failed: OSError('disk full'); restoring"));
    // A staged bundle without its program is refused before anything moves.
    let s = swap_setup();
    std::fs::remove_file(s.staged.join(updater::RS_PROGRAM)).unwrap();
    let error = updater::finish_package(&s.app, &s.staged, 0, None, &s.layout, &Hooks::default(), &RS_PACKAGE);
    assert_eq!(error, "staged version is incomplete");
    assert_eq!(std::fs::read(s.app.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.1.0");
}

#[test]
fn tidying_a_mac_work_folder_keeps_the_newest_previous_bundle() {
    let root = Temp::new();
    let layout = mac_layout(&root.0, "arm64");
    let app = root.0.join("Applications").join("Grainy RS.app");
    let work = updater::work_folder_for(&app, &layout, &RS_PACKAGE);
    for name in [
        "new",
        "download",
        "extract-1",
        "failed-20260101-000000",
        "previous-20260101-000000",
        "previous-20261120-120000",
        "mount-empty",
        "mount-busy",
    ] {
        std::fs::create_dir_all(work.join(name)).unwrap();
    }
    std::fs::write(work.join("mount-busy").join("on the volume"), b"x").unwrap();
    let staged_program = work.join("new").join("Grainy RS.app").join(updater::RS_PROGRAM);
    std::fs::create_dir_all(staged_program.parent().unwrap()).unwrap();
    std::fs::write(&staged_program, b"GrainyRS 0.2.0").unwrap();
    let python_work = updater::work_folder(&app, &layout);
    std::fs::create_dir_all(python_work.join("new")).unwrap();
    let mut trashed = Vec::new();
    let exe = app.join(updater::RS_PROGRAM);
    updater::cleanup_for(
        &app,
        &layout,
        &exe,
        &mut |p: &Path| {
            trashed.push(p.file_name().unwrap().to_string_lossy().into_owned());
            true
        },
        &RS_PACKAGE,
    );
    trashed.sort();
    assert_eq!(trashed, ["download", "extract-1", "failed-20260101-000000", "new", "previous-20260101-000000"]);
    assert!(!work.join("mount-empty").exists(), "an empty mount folder is removed");
    assert!(work.join("mount-busy").join("on the volume").is_file(), "never anything inside one");
    assert!(python_work.join("new").is_dir(), "the Python Grainy's work folder is never tidied");
    // Run from the staged bundle (the finishing step): nothing is tidied.
    let mut asked = 0;
    updater::cleanup_for(
        &app,
        &layout,
        &staged_program,
        &mut |_| {
            asked += 1;
            true
        },
        &RS_PACKAGE,
    );
    assert_eq!(asked, 0);
}

// ---- macOS only: real disk images ------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod real {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    /// `hdiutil create -volname "Grainy RS" -srcfolder <folder> -format UDZO <image>` (retried: the runners'
    /// disk arbitration is sometimes briefly busy).
    fn make_image(folder: &Path, image: &Path) {
        let mut last = String::new();
        for _ in 0..4 {
            let out = Command::new("/usr/bin/hdiutil")
                .args(["create", "-volname", "Grainy RS", "-srcfolder"])
                .arg(folder)
                .args(["-format", "UDZO", "-ov"])
                .arg(image)
                .output()
                .unwrap();
            if out.status.success() && image.is_file() {
                return;
            }
            last = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        panic!("hdiutil create failed: {last}");
    }

    fn volume_with(root: &Path, plist: &[u8]) -> PathBuf {
        let volume = root.join("volume");
        let bundle = volume.join("Grainy RS.app");
        make_bundle(&bundle, b"#!/bin/sh\necho GrainyRS 0.2.0\n", plist);
        std::fs::set_permissions(bundle.join(updater::RS_PROGRAM), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("/Applications", volume.join("Applications")).unwrap();
        volume
    }

    fn attached_volumes() -> String {
        String::from_utf8_lossy(&Command::new("/usr/bin/hdiutil").arg("info").output().unwrap().stdout).into_owned()
    }

    #[test]
    fn a_real_disk_image_is_staged_and_detached() {
        for binary in [false, true] {
            let root = Temp::new();
            let volume = volume_with(&root.0, xml_plist(updater::RS_BUNDLE_ID).as_bytes());
            if binary {
                let info = volume.join("Grainy RS.app/Contents/Info.plist");
                let out = Command::new("/usr/bin/plutil").args(["-convert", "binary1"]).arg(&info).output().unwrap();
                assert!(out.status.success());
                let bytes = std::fs::read(&info).unwrap();
                assert!(bytes.starts_with(b"bplist00"));
                assert_eq!(
                    plist::bundle_identifier(&bytes).as_deref(),
                    Some(updater::RS_BUNDLE_ID),
                    "plutil's binary list"
                );
            }
            let image = root.0.join("GrainyRS-0.2.0-macos-universal.dmg");
            make_image(&volume, &image);
            let work = root.0.join("Update");
            let staged = updater::stage_disk_image(&image, &work, &RS_MAC, &updater::SystemTools).unwrap();
            assert_eq!(staged, work.join("new").join("Grainy RS.app"));
            let program = staged.join(updater::RS_PROGRAM);
            assert_eq!(std::fs::read(&program).unwrap(), b"#!/bin/sh\necho GrainyRS 0.2.0\n");
            assert_ne!(std::fs::metadata(&program).unwrap().permissions().mode() & 0o111, 0, "ditto keeps the mode");
            assert!(!work.join("new").join("Applications").exists(), "only the bundle");
            assert!(names_in(&work, "mount-").is_empty(), "detached and its folder removed");
            assert!(!attached_volumes().contains(&work.display().to_string()), "nothing of ours stays attached");
        }
    }

    #[test]
    fn a_real_disk_image_with_another_identifier_is_refused_and_detached() {
        for identifier in ["io.github.pastorius9.grainy", "com.example.other"] {
            let root = Temp::new();
            let volume = volume_with(&root.0, xml_plist(identifier).as_bytes());
            let image = root.0.join("GrainyRS-0.2.0-macos-universal.dmg");
            make_image(&volume, &image);
            let work = root.0.join("Update");
            let error = updater::stage_disk_image(&image, &work, &RS_MAC, &updater::SystemTools).unwrap_err();
            assert_eq!(error.to_string(), BAD, "{identifier}");
            assert!(!work.join("new").exists());
            assert!(names_in(&work, "mount-").is_empty());
            assert!(!attached_volumes().contains(&work.display().to_string()));
        }
    }

    #[test]
    fn a_real_disk_image_with_a_linked_bundle_is_refused() {
        let root = Temp::new();
        let outside = root.0.join("outside").join("Grainy RS.app");
        good_bundle(&outside, b"elsewhere");
        let volume = root.0.join("volume");
        std::fs::create_dir_all(&volume).unwrap();
        std::os::unix::fs::symlink(&outside, volume.join("Grainy RS.app")).unwrap();
        let image = root.0.join("GrainyRS-0.2.0-macos-universal.dmg");
        make_image(&volume, &image);
        let work = root.0.join("Update");
        let error = updater::stage_disk_image(&image, &work, &RS_MAC, &updater::SystemTools).unwrap_err();
        assert_eq!(error.to_string(), BAD);
        assert!(names_in(&work, "mount-").is_empty());
    }

    #[test]
    fn a_file_that_is_not_a_disk_image_does_not_attach() {
        let root = Temp::new();
        let image = root.0.join("GrainyRS-0.2.0-macos-universal.dmg");
        std::fs::write(&image, b"not a disk image").unwrap();
        let work = root.0.join("Update");
        assert!(updater::stage_disk_image(&image, &work, &RS_MAC, &updater::SystemTools).is_err());
        assert!(!work.join("new").exists());
        assert!(names_in(&work, "mount-").is_empty());
    }

    #[test]
    fn stage_then_swap_on_real_folders() {
        let root = Temp::new();
        let layout = mac_layout(&root.0, if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" });
        let app = root.0.join("Applications").join("Grainy RS.app");
        good_bundle(&app, b"GrainyRS 0.1.0");
        let volume = volume_with(&root.0, xml_plist(updater::RS_BUNDLE_ID).as_bytes());
        let image = root.0.join("GrainyRS-0.2.0-macos-universal.dmg");
        make_image(&volume, &image);
        let work = updater::work_folder_for(&app, &layout, &RS_PACKAGE);
        let staged = updater::stage_rust(&image, &work, &layout, &updater::SystemTools).unwrap();
        let hooks = Hooks { stamp: Some("20261120-120000".into()), ..Hooks::default() };
        assert_eq!(updater::finish_package(&app, &staged, 0, None, &layout, &hooks, &RS_PACKAGE), "");
        assert_eq!(std::fs::read(app.join(updater::RS_PROGRAM)).unwrap(), b"#!/bin/sh\necho GrainyRS 0.2.0\n");
        let previous = work.join("previous-20261120-120000").join("Grainy RS.app");
        assert_eq!(std::fs::read(previous.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.1.0");
    }

    #[test]
    fn a_bundle_in_a_read_only_place_cannot_update_itself() {
        let root = Temp::new();
        let layout = mac_layout(&root.0, "arm64");
        let folder = root.0.join("read-only");
        let app = folder.join("Grainy RS.app");
        good_bundle(&app, b"GrainyRS 0.1.0");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let reason = updater::blocked(Some(&app), 1, &layout, &|_: &Path| Ok(u64::MAX));
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(reason, "프로그램 폴더에 쓸 수 없습니다.");
        assert_eq!(updater::blocked(Some(&app), 1, &layout, &|_: &Path| Ok(u64::MAX)), "");
    }
}
