//! The update rule of the Rust build (HANDOFF2 A): only a release file marked `GrainyRS-` qualifies, never the
//! Python Grainy's `Grainy-` files. Synthetic feeds only; nothing touches the network.

use std::path::PathBuf;

use grainy_platform::json::Value;
use grainy_platform::updater::{self, Layout, Release};
use grainy_platform::{Environment, System};

fn layout(system: System, machine: &str) -> Layout {
    Layout {
        system,
        machine: machine.into(),
        env: Environment { system, local_app_data: Some("C:\\Local".into()), home: PathBuf::from("C:\\Home") },
    }
}

fn asset(name: &str, size: i64, hex: char) -> String {
    format!(
        r#"{{"name": "{name}", "size": {size}, "digest": "sha256:{}", "browser_download_url": "{}v1/{name}"}}"#,
        hex.to_string().repeat(64),
        updater::RS_DOWNLOADS
    )
}

fn feed(assets: &[String]) -> Value {
    let text =
        format!(r#"{{"draft": false, "prerelease": false, "body": "notes", "assets": [{}]}}"#, assets.join(", "));
    Value::parse(&text).unwrap()
}

/// The shape of the project's feed on 2026-11-19: the Python Grainy's files only.
fn python_feed() -> Value {
    feed(&[
        asset("Grainy-0.6.8-macos-arm64.dmg", 105_809_575, 'd'),
        asset("Grainy-0.6.8-macos-arm64.zip", 92_349_723, 'c'),
        asset("Grainy-0.6.8-windows-x64.zip", 129_357_785, '0'),
    ])
}

#[test]
fn the_python_grainys_files_never_qualify() {
    let payload = python_feed();
    for (system, machine) in [(System::Windows, "AMD64"), (System::MacOs, "arm64"), (System::MacOs, "x86_64")] {
        let layout = layout(system, machine);
        assert_eq!(updater::parse_rust_release(&payload, Some(updater::RS_DOWNLOADS), &layout), None);
        assert_eq!(updater::parse_rust_release(&payload, None, &layout), None);
    }
    // The oracle's own rule still finds the Python file (unchanged port).
    let python = updater::parse_release(&payload, Some(updater::RS_DOWNLOADS), &layout(System::Windows, "AMD64"));
    assert_eq!(python.map(|r| r.name).as_deref(), Some("Grainy-0.6.8-windows-x64.zip"));
}

#[test]
fn a_marked_file_qualifies_beside_the_python_files() {
    let mut assets = vec![asset("Grainy-0.7.0-windows-x64.zip", 100, 'a')];
    assets.push(asset("GrainyRS-0.7.0-macos-arm64.zip", 300, 'c'));
    assets.push(asset("GrainyRS-0.7.0-windows-x64.zip", 200, 'b'));
    let payload = feed(&assets);
    let windows = layout(System::Windows, "AMD64");
    let release = updater::parse_rust_release(&payload, Some(updater::RS_DOWNLOADS), &windows).unwrap();
    assert_eq!(release.name, "GrainyRS-0.7.0-windows-x64.zip");
    assert_eq!(release.version, "0.7.0");
    assert_eq!(release.size, 200);
    assert_eq!(release.sha256, "b".repeat(64));
    assert!(updater::is_rust_release(&release, &windows));
    let mac = layout(System::MacOs, "arm64");
    assert_eq!(updater::parse_rust_release(&payload, None, &mac).unwrap().name, "GrainyRS-0.7.0-macos-arm64.zip");
    // The Python rule never takes the marked file.
    assert_eq!(updater::parse_release(&payload, None, &windows).unwrap().name, "Grainy-0.7.0-windows-x64.zip");
}

#[test]
fn a_marked_file_still_needs_the_oracles_conditions() {
    let windows = layout(System::Windows, "AMD64");
    let take = |assets: &[String]| updater::parse_rust_release(&feed(assets), Some(updater::RS_DOWNLOADS), &windows);
    // No digest, a wrong digest, no size, a foreign address, another system, a bad version.
    let no_digest = r#"{"name": "GrainyRS-0.7.0-windows-x64.zip", "size": 5, "browser_download_url": "https://github.com/pastorius9/grainy/releases/download/v/x"}"#;
    assert_eq!(take(&[no_digest.to_string()]), None);
    assert_eq!(take(&[asset("GrainyRS-0.7.0-windows-x64.zip", 0, 'a')]), None);
    assert_eq!(take(&[asset("GrainyRS-0.7.0-windows-x64.zip", 5, 'A')]), None);
    assert_eq!(take(&[asset("GrainyRS-0.7.0-macos-x64.zip", 5, 'a')]), None);
    assert_eq!(take(&[asset("GrainyRS-0.7-beta-windows-x64.zip", 5, 'a')]), None);
    let foreign =
        asset("GrainyRS-0.7.0-windows-x64.zip", 5, 'a').replace(updater::RS_DOWNLOADS, "https://example.invalid/");
    assert_eq!(take(&[foreign]), None);
    // A draft or a prerelease.
    let draft =
        Value::parse(&format!(r#"{{"draft": true, "assets": [{}]}}"#, asset("GrainyRS-0.7.0-windows-x64.zip", 5, 'a')))
            .unwrap();
    assert_eq!(updater::parse_rust_release(&draft, None, &windows), None);
}

#[test]
fn an_override_cannot_relax_the_rust_download_boundary() {
    let windows = layout(System::Windows, "AMD64");
    let name = "GrainyRS-0.7.0-windows-x64.zip";
    let valid = asset(name, 5, 'a');
    for address in [
        format!("https://example.invalid/{name}"),
        format!("{}../../../../../other/project/releases/download/v1/{name}", updater::RS_DOWNLOADS),
        format!("{}%2e%2e/{name}", updater::RS_DOWNLOADS),
        format!("{}v1/{name}?redirect=elsewhere", updater::RS_DOWNLOADS),
        format!("{}v1/{name}", updater::ORACLE_DOWNLOADS),
        format!("{}v1/{name}", updater::RS_DOWNLOADS.replace("https:", "http:")),
    ] {
        let payload = feed(&[valid.replace(&format!("{}v1/{name}", updater::RS_DOWNLOADS), &address)]);
        assert!(updater::parse_rust_release(&payload, None, &windows).is_none());
        let text = payload.dump(None, false);
        let open = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> {
            Ok(Box::new(std::io::Cursor::new(text.clone().into_bytes())))
        };
        assert!(updater::latest_rust(&open, Some("https://example.invalid/feed"), &windows).unwrap().is_none());
    }
}

#[test]
fn the_rust_check_of_todays_feed_finds_nothing() {
    let folder = std::env::temp_dir().join(format!("grainy-platform-rust-release-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("feed.json");
    std::fs::write(&file, python_feed().dump(None, false)).unwrap();
    // A local feed: the opener is never asked.
    let open = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> { panic!("no network in tests") };
    let windows = layout(System::Windows, "AMD64");
    let found = updater::latest_rust(&open, Some(file.to_str().unwrap()), &windows).unwrap();
    assert_eq!(found, None);
    assert!(updater::latest(&open, Some(file.to_str().unwrap()), &windows).unwrap().is_some());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_python_release_is_not_a_rust_release() {
    let windows = layout(System::Windows, "AMD64");
    let release = |name: &str, version: &str| Release {
        version: version.into(),
        name: name.into(),
        url: format!("{}v0.7.0/{name}", updater::RS_DOWNLOADS),
        size: 1,
        sha256: "a".repeat(64),
        notes: String::new(),
    };
    assert!(!updater::is_rust_release(&release("Grainy-0.7.0-windows-x64.zip", "0.7.0"), &windows));
    assert!(!updater::is_rust_release(&release("GrainyRS-0.7.0-windows-x64.zip", "0.7.1"), &windows));
    assert!(!updater::is_rust_release(&release("GrainyRS-0.7.0-macos-arm64.zip", "0.7.0"), &windows));
    assert!(updater::is_rust_release(&release("GrainyRS-0.7.0-windows-x64.zip", "0.7.0"), &windows));
}
