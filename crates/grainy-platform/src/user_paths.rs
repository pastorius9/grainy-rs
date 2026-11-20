//! Port of luma/user_paths.py: where Grainy keeps its per-user data.

use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

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

/// The path with `.` and `..` resolved lexically and compared as the system's file system compares names: on
/// Windows without case, on macOS (APFS, HFS+) without case and Unicode composition (NFC and lower case, the
/// oracle's `folders.path_key` there), elsewhere exactly.
fn key(path: &Path, system: System) -> Vec<String> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut parts: Vec<String> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir => {}
            other => {
                let text = other.as_os_str().to_string_lossy().replace('/', "\\");
                let text = text.trim_end_matches('\\').to_owned();
                parts.push(match system {
                    System::Windows => text.to_lowercase(),
                    System::MacOs => text.nfc().collect::<String>().to_lowercase(),
                    System::Other => text,
                });
            }
        }
    }
    parts
}

// Resolve the existing prefix even when the final directory has not been created.
// canonicalize(path) alone misses a new directory below a symlink or junction.
fn resolved(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut suffix = Vec::new();
    let mut ancestor = absolute.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            let text = real.to_string_lossy();
            let mut real = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{unc}"))
            } else {
                PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
            };
            for part in suffix.iter().rev() {
                real.push(part);
            }
            return real;
        }
        let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) else {
            return absolute;
        };
        suffix.push(name.to_owned());
        ancestor = parent;
    }
}

/// Whether a snapshot target is the source directory or below it, including path aliases.
pub fn is_within(path: &Path, root: &Path) -> bool {
    let system = System::current();
    key(path, system).starts_with(&key(root, system))
        || key(&resolved(path), system).starts_with(&key(&resolved(root), system))
}

/// Whether `path` is one of the Python Grainy's folders or inside one (so it must not be opened for writing).
/// The path as written and its resolved form (links, junctions and 8.3 short names of its existing part
/// followed) are compared with the Python Grainy's folders as written and resolved: an 8.3 or linked
/// `LOCALAPPDATA` (or home), or a link to one of the folders, names the same folder differently. For the Python
/// Grainy's folders only their parent folder's real name is asked: the folders themselves and everything inside
/// them are never opened, read or written.
pub fn is_oracle_folder(path: &Path, env: &Environment) -> bool {
    let system = env.system;
    let folders = [local_data(env, "Luma", None), local_data(env, "Grainy", None)];
    let protected: Vec<Vec<String>> = folders
        .iter()
        .map(|folder| key(folder, system))
        .chain(folders.iter().map(|f| key(&resolved_parent(f), system)))
        .collect();
    let inside = |candidate: Vec<String>| protected.iter().any(|f| !f.is_empty() && candidate.starts_with(f));
    inside(key(path, system)) || inside(key(&resolved(path), system))
}

/// `folder` with its parent resolved (links, junctions, 8.3 short names): the folder itself is never opened.
fn resolved_parent(folder: &Path) -> PathBuf {
    match (folder.parent(), folder.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => folder.to_path_buf(),
    }
}

/// True when the resolved destination remains below the selected directory (including linked parents).
pub fn resolved_within(path: &Path, root: &Path) -> bool {
    let system = System::current();
    key(&resolved(path), system).starts_with(&key(&resolved(root), system))
}

/// Reject moving a directory which contains protected application data as a descendant.
pub fn contains_oracle(path: &Path, env: &Environment) -> bool {
    let actual = key(&resolved(path), env.system);
    ["Luma", "Grainy"].iter().any(|name| {
        let root = local_data(env, name, None);
        key(&resolved_parent(&root), env.system).starts_with(&actual)
    })
}
