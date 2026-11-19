//! Handing a folder, a file or a web address to the system: the oracle's `QDesktopServices.openUrl` (of
//! `QUrl.fromLocalFile(folder)` or of an address). The oracle opens folders and never selects a file in them, so
//! neither does this.
//!
//! * Windows: a folder goes to Explorer (`explorer <folder>`), a file or an address to its default program
//!   (`rundll32 url.dll,FileProtocolHandler <target>`).
//! * macOS: `/usr/bin/open <target>` for all of them: a folder opens in Finder, a file in its default app, an
//!   address in the browser, a `x-apple.systempreferences:` address in System Settings.
//! * Elsewhere: `xdg-open <target>`.
//!
//! The menu words follow the system as in the oracle (`'Finder에서 열기' if MAC else '탐색기에서 열기'`).

use std::process::{Child, Command};

use grainy_platform::System;

/// The folder menu's item: `'Finder에서 열기' if MAC else '탐색기에서 열기'` (luma/app.py `folder_context_menu`).
pub const SHOW_IN_FILE_MANAGER: &str =
    if cfg!(target_os = "macos") { "Finder에서 열기" } else { "탐색기에서 열기" };

/// The photo menu's item: `'Finder에서 폴더 열기' if MAC else '탐색기에서 폴더 열기'` (luma/photo_actions.py).
pub const SHOW_FOLDER_IN_FILE_MANAGER: &str =
    if cfg!(target_os = "macos") { "Finder에서 폴더 열기" } else { "탐색기에서 폴더 열기" };

/// What is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A folder: shown in the file manager.
    Folder,
    /// A file (its default program) or an address (the browser, a settings page).
    Document,
}

/// The program and its arguments that open `target` on `system`.
pub fn command(system: System, kind: Target, target: &str) -> (&'static str, Vec<String>) {
    match (system, kind) {
        (System::Windows, Target::Folder) => ("explorer", vec![target.to_owned()]),
        (System::Windows, Target::Document) => {
            ("rundll32", vec!["url.dll,FileProtocolHandler".to_owned(), target.to_owned()])
        }
        (System::MacOs, _) => ("/usr/bin/open", vec![target.to_owned()]),
        (System::Other, _) => ("xdg-open", vec![target.to_owned()]),
    }
}

/// Opens `target` with this system's program; the program is not waited for.
pub fn open(kind: Target, target: &str) -> std::io::Result<()> {
    let (program, arguments) = command(System::current(), kind, target);
    run(program, &arguments)
}

/// Starts `program` with `arguments` and does not wait for it.
pub fn run(program: &str, arguments: &[String]) -> std::io::Result<()> {
    let child = Command::new(program).args(arguments).spawn()?;
    reap(child);
    Ok(())
}

/// `open` returns at once: its exit status is collected on a small thread, so no finished process is left behind.
#[cfg(unix)]
fn reap(mut child: Child) {
    let _ = std::thread::Builder::new().name("grainy-open".into()).spawn(move || {
        let _ = child.wait();
    });
}

/// Windows keeps no finished processes around.
#[cfg(not(unix))]
fn reap(_child: Child) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_system_opens_with_its_own_program() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(command(System::Windows, Target::Folder, r"D:\Photos"), ("explorer", args(&[r"D:\Photos"])));
        assert_eq!(
            command(System::Windows, Target::Document, "https://example.invalid/"),
            ("rundll32", args(&["url.dll,FileProtocolHandler", "https://example.invalid/"]))
        );
        assert_eq!(
            command(System::MacOs, Target::Folder, "/Users/a/Photos"),
            ("/usr/bin/open", args(&["/Users/a/Photos"]))
        );
        assert_eq!(
            command(System::MacOs, Target::Document, "x-apple.systempreferences:x"),
            ("/usr/bin/open", args(&["x-apple.systempreferences:x"]))
        );
        assert_eq!(command(System::Other, Target::Folder, "/home/a"), ("xdg-open", args(&["/home/a"])));
    }

    #[test]
    fn the_menu_words_follow_the_system() {
        if cfg!(target_os = "macos") {
            assert_eq!(
                (SHOW_IN_FILE_MANAGER, SHOW_FOLDER_IN_FILE_MANAGER),
                ("Finder에서 열기", "Finder에서 폴더 열기")
            );
        } else {
            assert_eq!(
                (SHOW_IN_FILE_MANAGER, SHOW_FOLDER_IN_FILE_MANAGER),
                ("탐색기에서 열기", "탐색기에서 폴더 열기")
            );
        }
    }
}
