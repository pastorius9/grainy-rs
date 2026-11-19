//! Port of luma/user_paths.py: where Grainy keeps its per-user data.

use std::path::{Path, PathBuf};

/// The operating system, as far as folder layout is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    /// Windows: `%LOCALAPPDATA%/<folder>`.
    Windows,
    /// macOS: `~/Library/Application Support/Grainy`.
    MacOs,
    /// Anything else (the oracle treats it like Windows with the home folder as fallback).
    Other,
}

impl System {
    /// The system this program was built for.
    pub fn current() -> System {
        if cfg!(target_os = "windows") {
            System::Windows
        } else if cfg!(target_os = "macos") {
            System::MacOs
        } else {
            System::Other
        }
    }

    /// Port of `MAC = sys.platform == 'darwin'`.
    pub fn is_mac(self) -> bool {
        self == System::MacOs
    }

    /// Port of `NATIVE_SUFFIX`: `.dll` on Windows, `.dylib` on macOS, none elsewhere.
    pub fn native_suffix(self) -> Option<&'static str> {
        match self {
            System::Windows => Some(".dll"),
            System::MacOs => Some(".dylib"),
            System::Other => None,
        }
    }
}

/// What `local_data` reads from the machine, as parameters so every system can be tested anywhere.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The operating system.
    pub system: System,
    /// The `LOCALAPPDATA` variable, when set.
    pub local_app_data: Option<String>,
    /// The user's home folder (`Path.home()`).
    pub home: PathBuf,
}

impl Environment {
    /// The real machine: this system, `%LOCALAPPDATA%` and the home folder.
    pub fn current() -> Environment {
        let home =
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
        Environment { system: System::current(), local_app_data: std::env::var("LOCALAPPDATA").ok(), home }
    }
}

/// Port of luma/user_paths.py `local_data`: the app's own folder, `%LOCALAPPDATA%/<windows_folder>`
/// (`fallback` or the home folder when the variable is missing), or the one Grainy folder on macOS.
pub fn local_data(env: &Environment, windows_folder: &str, fallback: Option<&Path>) -> PathBuf {
    if env.system.is_mac() {
        return env.home.join("Library").join("Application Support").join("Grainy");
    }
    let base = match &env.local_app_data {
        Some(value) => PathBuf::from(value),
        None => fallback.map_or_else(|| env.home.clone(), Path::to_path_buf),
    };
    base.join(windows_folder)
}
