//! Updater goldens: the oracle's results for version order, release files, hostile release payloads, staging and
//! swapping fake releases, cleanup, the pre-install checks and verified downloads (exact comparisons).

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use grainy_platform::json::Value;
use grainy_platform::updater::{self, Hooks, Layout, Release, UpdateError};
use grainy_platform::{Environment, System};

fn layout_for(spec: &serde_json::Value, root: &Path) -> Layout {
    let machine = spec["machine"].as_str().unwrap();
    if spec["mac"].as_bool().unwrap() {
        mac_layout(root, machine)
    } else {
        let mut layout = windows_layout(root);
        layout.machine = machine.into();
        layout
    }
}

#[test]
fn version_order_and_keys_match_the_oracle() {
    let case = golden("versions");
    for pair in case["pairs"].as_array().unwrap() {
        let (a, b) = (pair["a"].as_str().unwrap(), pair["b"].as_str().unwrap());
        assert_eq!(updater::newer(a, b), pair["newer"].as_bool().unwrap(), "newer({a:?}, {b:?})");
    }
    for key in case["keys"].as_array().unwrap() {
        let text = key["text"].as_str();
        let expected = |name: &str| -> Vec<String> {
            key[name].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
        };
        assert_eq!(
            updater::version_key(text.unwrap_or("None")).parts(),
            expected("updater"),
            "updater key of {text:?}"
        );
        assert_eq!(
            grainy_platform::whats_new::version_key(text).parts(),
            expected("whats_new"),
            "whats_new key of {text:?}"
        );
    }
}

#[test]
fn each_system_takes_its_own_release_file() {
    let case = golden("asset_names");
    let root = Temp::new();
    for system in case["systems"].as_array().unwrap() {
        let layout = layout_for(&system["system"], &root.0);
        let pattern = updater::asset_pattern(&layout);
        for result in system["results"].as_array().unwrap() {
            let name = result["name"].as_str().unwrap();
            assert_eq!(
                pattern.captures(name).as_deref(),
                result["version"].as_str(),
                "{:?} for {name:?}",
                system["system"]["name"]
            );
        }
    }
}

#[test]
fn release_payloads_are_trusted_or_rejected_like_the_oracle() {
    let case = golden("release_parse");
    let root = Temp::new();
    let mut count = 0;
    for entry in case["cases"].as_array().unwrap() {
        let layout = layout_for(&entry["system"], &root.0);
        let payload = Value::from(&entry["payload"]);
        let trusted = entry["trusted"].as_str();
        let got = updater::parse_release(&payload, trusted, &layout);
        let expected = &entry["expected"];
        let label = format!("{} / {:?} / {}", entry["name"], trusted, entry["system"]["name"]);
        match (got, expected.is_null()) {
            (None, true) => {}
            (Some(release), false) => {
                assert_eq!(release.version, expected["version"].as_str().unwrap(), "{label}");
                assert_eq!(release.name, expected["name"].as_str().unwrap(), "{label}");
                assert_eq!(release.url, expected["url"].as_str().unwrap(), "{label}");
                assert_eq!(release.size, expected["size"].as_u64().unwrap(), "{label}");
                assert_eq!(release.sha256, expected["sha256"].as_str().unwrap(), "{label}");
                assert_eq!(release.notes, expected["notes"].as_str().unwrap(), "{label}");
            }
            (got, _) => panic!("{label}: got {got:?}, oracle gave {expected}"),
        }
        count += 1;
    }
    println!("release_parse: {count} cases equal");
}

fn kind(error: &UpdateError) -> &'static str {
    match error {
        UpdateError::Io(_) => "OSError",
        UpdateError::Value(_) => "ValueError",
        UpdateError::Zip(_) => "BadZipFile",
    }
}

#[test]
fn staging_unpacks_good_zips_and_refuses_unsafe_ones() {
    let case = golden("stage");
    for entry in case["cases"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let root = Temp::new();
        let layout = windows_layout(&root.0);
        let archive = root.0.join("new.zip");
        let work = root.0.join("work");
        let entries = entries_of(&entry["entries"]);
        zip_file(&archive, &entries);
        let result = &entry["result"];
        if name == "second_stage" {
            updater::stage(&archive, &work, &layout).unwrap();
        }
        let got = updater::stage(&archive, &work, &layout);
        match (got, result["ok"].as_bool().unwrap()) {
            (Ok(path), true) => {
                let rel = path.strip_prefix(&root.0).unwrap().to_string_lossy().replace('\\', "/");
                assert_eq!(rel, result["value"].as_str().unwrap(), "{name}");
            }
            (Err(error), false) => {
                assert_eq!(kind(&error), result["error"].as_str().unwrap(), "{name}: {error}");
                assert_eq!(error.to_string(), result["message"].as_str().unwrap(), "{name}");
            }
            (got, _) => panic!("{name}: got {got:?}, oracle {result}"),
        }
        let listing = if work.exists() { listing(&work, None) } else { Default::default() };
        assert_eq!(listing, expected_listing(&entry["listing"]), "{name}: files after staging");
        assert!(!root.0.join("evil.txt").exists());
    }
}

#[test]
fn macos_bundle_names_are_checked_like_the_oracle() {
    let case = golden("bundle_names");
    for entry in case["cases"].as_array().unwrap() {
        let names = strings(&entry["names"]);
        let rejected = updater::check_bundle_names(&names).is_err();
        assert_eq!(rejected, entry["rejected"].as_bool().unwrap(), "{}", entry["name"]);
    }
}

static PLACE_CALLS: AtomicUsize = AtomicUsize::new(0);

fn failing_place(source: &Path, target: &Path) -> std::io::Result<()> {
    if PLACE_CALLS.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
        return Err(std::io::Error::other("disk full"));
    }
    updater::place(source, target)
}

fn broken_copy(_source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir(target)?;
    std::fs::write(target.join("half.dll"), "partial")?;
    Err(std::io::Error::other("disk full"))
}

fn log_lines(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .map(|line| normal(line.split_once(' ').map_or(line, |(_, rest)| rest)))
        .collect()
}

#[test]
fn swapping_versions_keeps_the_previous_one_and_restores_on_failure() {
    let case = golden("finish");
    for entry in case["cases"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let root = Temp::new();
        let mut log: Vec<u8> = Vec::new();
        let want_log = !strings(&entry["log"]).is_empty();
        let (got, got_listing);
        if entry["kind"] == "windows" {
            let layout = windows_layout(&root.0);
            let app = root.0.join("app");
            put_map(&app, &entry["installed"]);
            let work = app.join(".update");
            let staged = if name == "incomplete" {
                std::fs::create_dir_all(root.0.join("staged")).unwrap();
                root.0.join("staged")
            } else {
                zip_file(&root.0.join("new.zip"), &entries_of(&entry["entries"]));
                updater::stage(&root.0.join("new.zip"), &work, &layout).unwrap()
            };
            let hooks = Hooks {
                copy_tree: if name == "copy_fails" { broken_copy } else { updater::copy_tree },
                ..Hooks::default()
            };
            got = updater::finish_with(
                &app,
                &staged,
                0,
                want_log.then_some(&mut log as &mut dyn std::io::Write),
                &layout,
                &hooks,
            );
            got_listing = listing(&app, None);
        } else {
            let layout = mac_layout(&root.0, "arm64");
            let work = updater::work_folder(&root.0.join("Applications").join("Grainy.app"), &layout);
            let app = root.0.join("Applications").join("Grainy.app");
            put_map(&app, &entry["bundle"]);
            std::fs::create_dir_all(root.0.join("Applications").join("Other.app")).unwrap();
            let staged = work.join("new").join("Grainy.app");
            put_map(&staged, &entry["new_bundle"]);
            PLACE_CALLS.store(0, Ordering::SeqCst);
            let hooks = Hooks {
                place: if entry["fail_second_place"].as_bool().unwrap() { failing_place } else { updater::place },
                ..Hooks::default()
            };
            got = updater::finish_with(
                &app,
                &staged,
                0,
                want_log.then_some(&mut log as &mut dyn std::io::Write),
                &layout,
                &hooks,
            );
            let mut found = listing(&root.0, Some(&work));
            // The oracle's test keeps its work folder at root/Support/Update; ours sits under Library/Application Support/Grainy.
            for parent in ["Library", "Library/Application Support", "Library/Application Support/Grainy"] {
                found.remove(parent);
            }
            found.insert("Support".into(), None);
            got_listing = found;
        }
        assert_eq!(got, entry["result"].as_str().unwrap(), "{name}: result");
        assert_eq!(log_lines(&log), strings(&entry["log"]), "{name}: log");
        assert_eq!(got_listing, expected_listing(&entry["listing"]), "{name}: files after the swap");
    }
}

#[test]
fn cleanup_recycles_leftovers_but_keeps_the_newest_previous_version() {
    let case = golden("cleanup");
    for entry in case["cases"].as_array().unwrap() {
        let root = Temp::new();
        let layout = windows_layout(&root.0);
        let app = root.0.join("app");
        let work = app.join(".update");
        std::fs::create_dir_all(&work).unwrap();
        for name in strings(&entry["present"]) {
            std::fs::create_dir(work.join(name)).unwrap();
        }
        let mut trashed = Vec::new();
        updater::cleanup(&app, &layout, &root.0.join("python.exe"), &mut |p: &Path| {
            trashed.push(p.file_name().unwrap().to_string_lossy().into_owned());
            true
        });
        trashed.sort();
        assert_eq!(trashed, strings(&entry["trashed"]), "present {}", entry["present"]);
    }
}

#[test]
fn cleanup_leaves_everything_when_the_running_program_is_the_staged_copy() {
    let root = Temp::new();
    let layout = windows_layout(&root.0);
    let app = root.0.join("app");
    let staged = app.join(".update").join("new");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::create_dir_all(app.join(".update").join("download")).unwrap();
    let mut trashed = Vec::new();
    updater::cleanup(&app, &layout, &staged.join("Grainy.exe"), &mut |p: &Path| {
        trashed.push(p.to_path_buf());
        true
    });
    assert!(trashed.is_empty());
}

#[test]
fn the_checks_before_installing_match_the_oracle() {
    let case = golden("blocked");
    for entry in case["cases"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let root = Temp::new();
        let mac = entry["mac"].as_bool().unwrap();
        let layout = if mac { mac_layout(&root.0, "arm64") } else { windows_layout(&root.0) };
        let app: Option<PathBuf> = match entry["app_kind"].as_str() {
            None => None,
            Some(kind) => {
                let path = if mac {
                    if kind == "exists" {
                        root.0.join("Apps").join("Grainy.app")
                    } else {
                        root.0.join("nowhere").join("Grainy.app")
                    }
                } else if kind == "exists" {
                    root.0.join("app")
                } else {
                    root.0.join("nowhere")
                };
                if kind == "exists" {
                    std::fs::create_dir_all(&path).unwrap();
                }
                Some(path)
            }
        };
        let free = entry["free"].as_u64().unwrap();
        let got = updater::blocked(app.as_deref(), entry["size"].as_u64().unwrap(), &layout, &|_: &Path| Ok(free));
        assert_eq!(got, entry["expected"].as_str().unwrap(), "{name}");
        if let Some(path) = &app {
            let leftovers: Vec<_> = std::fs::read_dir(if mac { path.parent().unwrap() } else { path.as_path() })
                .map(|r| r.flatten().map(|e| e.file_name()).collect())
                .unwrap_or_default();
            assert!(
                leftovers.iter().all(|n: &std::ffi::OsString| !n.to_string_lossy().starts_with(".update-probe")),
                "{name}: probe file left"
            );
        }
    }
}

#[test]
fn downloads_are_verified_and_nothing_unverified_is_left() {
    let case = golden("download");
    let data: Vec<u8> = (0..case["pattern_length"].as_u64().unwrap() as usize).map(|i| (i % 256) as u8).collect();
    for entry in case["cases"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let root = Temp::new();
        let source = root.0.join("source.zip");
        let blob = if name == "empty_file" { Vec::new() } else { data.clone() };
        std::fs::write(&source, &blob).unwrap();
        assert_eq!(blob.len() as u64, entry["length"].as_u64().unwrap());
        let real = {
            use sha2::{Digest, Sha256};
            Sha256::digest(&blob).iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let release = Release {
            version: "9.9.9".into(),
            name: "Grainy-9.9.9-windows-x64.zip".into(),
            url: source.display().to_string(),
            size: entry["release_size"].as_u64().unwrap(),
            sha256: if entry["release_sha256_is_real"].as_bool().unwrap() { real } else { "0".repeat(64) },
            notes: String::new(),
        };
        let mut seen: Vec<(u64, u64)> = Vec::new();
        let mut progress = |done: u64, total: u64| seen.push((done, total));
        let never = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> { panic!("a local file needs no network") };
        let got = updater::download(&release, &root.0.join("work").join("download"), Some(&mut progress), None, &never);
        let result = &entry["result"];
        match (got, result["ok"].as_bool().unwrap()) {
            (Ok(Some(path)), true) => {
                assert_eq!(
                    path.strip_prefix(&root.0).unwrap().to_string_lossy().replace('\\', "/"),
                    result["value"].as_str().unwrap()
                );
                assert_eq!(std::fs::read(&path).unwrap(), blob);
            }
            (Err(error), false) => {
                assert_eq!(kind(&error), result["error"].as_str().unwrap());
                assert_eq!(error.to_string(), result["message"].as_str().unwrap());
            }
            (got, _) => panic!("{name}: got {got:?}, oracle {result}"),
        }
        let expected_progress: Vec<(u64, u64)> = entry["progress"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_u64().unwrap(), p[1].as_u64().unwrap()))
            .collect();
        assert_eq!(seen, expected_progress, "{name}: progress calls");
        let mut files: Vec<String> =
            listing(&root.0.join("work"), None).into_iter().filter(|(_, v)| v.is_some()).map(|(k, _)| k).collect();
        files.sort();
        assert_eq!(files, strings(&entry["files"]), "{name}: files left");
    }
}

#[test]
fn the_other_system_helpers_behave() {
    let root = Temp::new();
    let win = windows_layout(&root.0);
    let mac = mac_layout(&root.0, "arm64");
    assert_eq!(updater::work_folder(&root.0.join("app"), &win), root.0.join("app").join(".update"));
    assert_eq!(
        updater::work_folder(&root.0.join("Grainy.app"), &mac),
        root.0.join("Library").join("Application Support").join("Grainy").join("Update")
    );
    assert_eq!(updater::feed(None), updater::ORACLE_FEED);
    assert_eq!(updater::feed(Some("")), updater::ORACLE_FEED);
    assert_eq!(updater::feed(Some("C:/feed.json")), "C:/feed.json");
    assert_eq!(
        updater::restart_arguments(&["--update-now".into(), "a.jpg".into(), "--update-now".into()]),
        vec!["a.jpg".to_string()]
    );
    let exe = Path::new("/Apps/Grainy.app/Contents/MacOS/Grainy");
    assert_eq!(updater::app_folder(exe, &mac), Some(PathBuf::from("/Apps/Grainy.app")));
    assert_eq!(updater::app_folder(Path::new("/Apps/Grainy/Grainy"), &mac), None);
    assert_eq!(updater::app_folder(Path::new("/Apps/Grainy/Grainy.exe"), &win), Some(PathBuf::from("/Apps/Grainy")));
    let (program, args, cwd) = updater::launch_command(Path::new("/w/new"), Path::new("/a"), &["x".into()], 77, &win);
    assert_eq!(program, Path::new("/w/new").join("Grainy.exe"));
    assert_eq!(args, ["--finish-update", "/a", "--wait-pid", "77", "--", "x"]);
    assert_eq!(cwd, Path::new("/w/new"));
    let (program, _, cwd) = updater::launch_command(Path::new("/w/new/Grainy.app"), Path::new("/a"), &[], 1, &mac);
    assert_eq!(program, Path::new("/w/new/Grainy.app").join("Contents/MacOS/Grainy"));
    assert_eq!(cwd, Path::new("/w/new"));
    let _ = (Environment::current(), System::current());
}
