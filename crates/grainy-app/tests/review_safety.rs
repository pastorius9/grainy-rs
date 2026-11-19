//! Regression tests for the workspace review. Generated catalogues and fake processes only.
mod common;

use grainy_app::ai::connection::{Connection, Setup};
use grainy_app::ai::host::AppHost;
use grainy_app::commands::files::FilesState;
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::TestCard;
use grainy_app::AppContext;
use grainy_mcp::Host;
use std::sync::Arc;

#[test]
fn mcp_reports_catalogue_maintenance_to_the_tool_guard() {
    let root = common::temp_dir("review-mcp-maintenance");
    let doc = CatalogDocument::open(&root).unwrap();
    let mut cx = AppContext::new(&eframe::egui::Context::default(), Box::<TestCard>::default(), Box::new(doc));
    assert!(!AppHost::new(&mut cx).unwrap().maintenance_running());
    cx.state::<FilesState>().maintenance = true;
    assert!(AppHost::new(&mut cx).unwrap().maintenance_running());
    cx.state::<FilesState>().maintenance = false;
    assert!(!AppHost::new(&mut cx).unwrap().maintenance_running());
}

#[test]
fn the_real_mcp_binder_opens_no_listener_while_offline() {
    let root = common::temp_dir("review-mcp-offline");
    let mut connection = Connection::new(Setup::new(root.join("settings"), root.join("library"), Arc::new(|| {})));
    assert!(!connection.start(Some(47821)));
    assert!(connection.bound_port().is_none());
    // The tool path guard also excludes the Python application's folders, by name only.
    let own = connection.service.own_folders();
    for protected in grainy_app::paths::Locations::current().oracle_folders() {
        assert!(own.contains(&protected));
    }
}

#[cfg(windows)]
#[test]
fn a_new_directory_below_a_junction_cannot_bypass_the_oracle_guard() {
    let root = common::temp_dir("review-path-alias");
    let protected = root.join("Grainy");
    let alias = root.join("alias");
    std::fs::create_dir(&protected).unwrap();
    let result = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&protected)
        .output()
        .unwrap();
    assert!(result.status.success(), "create a junction between generated directories");
    let locations = grainy_app::paths::Locations {
        env: grainy_platform::Environment {
            system: grainy_platform::System::Windows,
            local_app_data: Some(root.to_string_lossy().into_owned()),
            home: root.clone(),
        },
        home_override: None,
        xdg_data_home: None,
    };
    assert!(grainy_app::paths::is_oracle_folder(&alias.join("new/child"), &locations));
    assert!(!protected.join("new").exists());
    std::fs::remove_dir(alias).unwrap();
}

#[cfg(windows)]
#[test]
fn session_folder_scanning_does_not_follow_junction_cycles() {
    let root = common::temp_dir("review-folder-cycle");
    let photo = root.join("generated.png");
    std::fs::write(&photo, b"only the extension is inspected").unwrap();
    let alias = root.join("loop");
    let result =
        std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(&alias).arg(&root).output().unwrap();
    assert!(result.status.success(), "create a junction between generated directories");
    let found = grainy_app::session::photo_files(std::slice::from_ref(&root));
    std::fs::remove_dir(alias).unwrap();
    assert_eq!(found, vec![photo]);
}
