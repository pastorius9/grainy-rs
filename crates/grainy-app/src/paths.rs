//! Where the Rust Grainy keeps its data, and the Python Grainy's folders it must never write.
//!
//! The Rust app has its own data folder, `%LOCALAPPDATA%\GrainyRS` on Windows,
//! `~/Library/Application Support/GrainyRS` on macOS and `$XDG_DATA_HOME/GrainyRS` (or `~/.local/share/GrainyRS`)
//! elsewhere; the environment variable `GRAINY_RS_HOME` replaces it and `--library <folder>` names the library
//! folder directly. The Python Grainy's library (`%LOCALAPPDATA%\Luma\Library`) and settings (`%LOCALAPPDATA%\Grainy`,
//! on macOS `~/Library/Application Support/Grainy`) are only ever opened read-only, by an explicit menu command
//! ([`oracle_library`]); [`is_oracle_folder`] guards every writable open.
//!
//! The path logic follows `grainy_platform::user_paths` (port of luma/user_paths.py), with the Rust app's folder
//! name; every function takes an [`grainy_platform::Environment`] so all systems are tested anywhere.

use std::path::{Component, Path, PathBuf};

use grainy_catalog::folders::Platform;
use grainy_platform::{Environment, System};

/// The folder name of the Rust app's data.
pub const DATA_FOLDER: &str = "GrainyRS";
/// The environment variable that replaces the data folder.
pub const HOME_VARIABLE: &str = "GRAINY_RS_HOME";

/// What decides the folders: the machine (system, `LOCALAPPDATA`, home) and the overrides.
#[derive(Debug, Clone)]
pub struct Locations {
    pub env: Environment,
    /// `GRAINY_RS_HOME`, when set and not empty.
    pub home_override: Option<PathBuf>,
    /// `XDG_DATA_HOME` (other systems).
    pub xdg_data_home: Option<PathBuf>,
}

impl Locations {
    /// This machine.
    pub fn current() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        Self { env: Environment::current(), home_override: var(HOME_VARIABLE), xdg_data_home: var("XDG_DATA_HOME") }
    }

    /// The Rust app's data folder.
    pub fn data_folder(&self) -> PathBuf {
        if let Some(home) = &self.home_override {
            return home.clone();
        }
        match self.env.system {
            System::Windows => match &self.env.local_app_data {
                Some(base) => PathBuf::from(base).join(DATA_FOLDER),
                None => self.env.home.join(DATA_FOLDER),
            },
            System::MacOs => self.env.home.join("Library").join("Application Support").join(DATA_FOLDER),
            System::Other => self
                .xdg_data_home
                .clone()
                .unwrap_or_else(|| self.env.home.join(".local").join("share"))
                .join(DATA_FOLDER),
        }
    }

    /// The Rust app's library (catalogue) folder: `<data>/Library`.
    pub fn library_folder(&self) -> PathBuf {
        self.data_folder().join("Library")
    }

    /// The Python Grainy's library folder (`%LOCALAPPDATA%\Luma\Library`; on macOS inside the one Grainy folder),
    /// port of `user_paths.local_data(..., 'Luma') / 'Library'`.
    pub fn oracle_library(&self) -> PathBuf {
        grainy_platform::local_data(&self.env, "Luma", None).join("Library")
    }

    /// The Python Grainy's settings folder (`%LOCALAPPDATA%\Grainy`).
    pub fn oracle_settings(&self) -> PathBuf {
        grainy_platform::local_data(&self.env, "Grainy", None)
    }

    /// The Python Grainy's folders the Rust app never writes.
    pub fn oracle_folders(&self) -> [PathBuf; 2] {
        [grainy_platform::local_data(&self.env, "Luma", None), self.oracle_settings()]
    }
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
                    System::MacOs => Platform::Mac.stored_path(&text).to_lowercase(),
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
pub(crate) fn is_within(path: &Path, root: &Path) -> bool {
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
pub fn is_oracle_folder(path: &Path, locations: &Locations) -> bool {
    let system = locations.env.system;
    let folders = locations.oracle_folders();
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

/// The command line of the Rust app: `[--library <folder>] [--debug] [<photo or folder>...]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arguments {
    /// `--library <folder>`: the library folder to open instead of the default one.
    pub library: Option<PathBuf>,
    /// `--debug`: the overlay of changed settings and render times.
    pub debug: bool,
    /// `--memory`: no library; photos open in an in-memory session (nothing is written).
    pub memory: bool,
    /// `--lensfun <folder>`: the lensfun database folder to use instead of the default ones.
    pub lensfun: Option<PathBuf>,
    /// `--ask-ai`: the Ask AI window opens at start (main.py `window.show_ai()`).
    pub ask_ai: bool,
    /// `--offline`: no network use at all (feedback and update checks fail at once); `GRAINY_OFFLINE=1` too.
    pub offline: bool,
    /// `--smoke <folder>`: export the first photo into the folder, write `smoke.json` there and close
    /// ([`crate::smoke`], the release smoke test).
    pub smoke: Option<PathBuf>,
    /// Photos and folders to import (added in place).
    pub paths: Vec<PathBuf>,
}

impl Arguments {
    /// Parses the arguments after the program name.
    pub fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, String> {
        let mut out = Arguments::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--library") => {
                    let folder = args.next().ok_or("--library needs a folder")?;
                    out.library = Some(PathBuf::from(folder));
                }
                Some("--lensfun") => {
                    let folder = args.next().ok_or("--lensfun needs a folder")?;
                    out.lensfun = Some(PathBuf::from(folder));
                }
                Some("--smoke") => {
                    let folder = args.next().ok_or("--smoke needs a folder")?;
                    out.smoke = Some(PathBuf::from(folder));
                }
                Some("--debug") => out.debug = true,
                Some("--memory") => out.memory = true,
                Some("--ask-ai") => out.ask_ai = true,
                Some("--offline") => out.offline = true,
                Some(flag) if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                _ => out.paths.push(PathBuf::from(arg)),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn windows(local: &str) -> Locations {
        Locations {
            env: Environment {
                system: System::Windows,
                local_app_data: Some(local.into()),
                home: PathBuf::from(r"C:\Users\someone"),
            },
            home_override: None,
            xdg_data_home: None,
        }
    }

    #[test]
    fn data_folders_per_system() {
        // Windows folders are spelled with `\`, which only a Windows `PathBuf` reads as a separator (elsewhere the
        // join adds `/`): the Windows half runs on Windows.
        #[cfg(windows)]
        {
            let w = windows(r"C:\Users\someone\AppData\Local");
            assert_eq!(w.data_folder(), PathBuf::from(r"C:\Users\someone\AppData\Local\GrainyRS"));
            assert_eq!(w.library_folder(), PathBuf::from(r"C:\Users\someone\AppData\Local\GrainyRS\Library"));
            assert_eq!(w.oracle_library(), PathBuf::from(r"C:\Users\someone\AppData\Local\Luma\Library"));
            assert_eq!(w.oracle_settings(), PathBuf::from(r"C:\Users\someone\AppData\Local\Grainy"));
        }
        let mac = Locations {
            env: Environment { system: System::MacOs, local_app_data: None, home: PathBuf::from("/Users/a") },
            home_override: None,
            xdg_data_home: None,
        };
        assert_eq!(mac.data_folder(), PathBuf::from("/Users/a/Library/Application Support/GrainyRS"));
        assert_eq!(mac.oracle_library(), PathBuf::from("/Users/a/Library/Application Support/Grainy/Library"));
        let other = Locations {
            env: Environment { system: System::Other, local_app_data: None, home: PathBuf::from("/home/a") },
            home_override: None,
            xdg_data_home: None,
        };
        assert_eq!(other.data_folder(), PathBuf::from("/home/a/.local/share/GrainyRS"));
        let custom = Locations { home_override: Some(PathBuf::from("/tmp/x")), ..other };
        assert_eq!(custom.data_folder(), PathBuf::from("/tmp/x"));
    }

    #[cfg(windows)]
    #[test]
    fn the_python_grainys_folders_are_recognised() {
        let w = windows(r"C:\Users\someone\AppData\Local");
        for path in [
            r"C:\Users\someone\AppData\Local\Luma\Library",
            r"c:\users\SOMEONE\appdata\local\luma\library\",
            r"C:\Users\someone\AppData\Local\Luma\Library\thumbnails",
            r"C:\Users\someone\AppData\Local\Luma",
            r"C:\Users\someone\AppData\Local\Grainy",
            r"C:\Users\someone\AppData\Local\GrainyRS\..\Grainy\x",
        ] {
            assert!(is_oracle_folder(Path::new(path), &w), "{path}");
        }
        for path in [
            r"C:\Users\someone\AppData\Local\GrainyRS\Library",
            r"C:\Users\someone\AppData\Local\GrainyX",
            r"D:\Photos",
        ] {
            assert!(!is_oracle_folder(Path::new(path), &w), "{path}");
        }
    }

    /// macOS names its folders without case or Unicode composition: the Python Grainy's folder is recognised however
    /// a path spells it (the paths are only compared, never looked at on disk).
    #[test]
    fn the_python_grainys_folder_is_recognised_on_macos_in_any_case_and_composition() {
        let mac = Locations {
            env: Environment { system: System::MacOs, local_app_data: None, home: PathBuf::from("/Users/kim") },
            home_override: None,
            xdg_data_home: None,
        };
        for path in [
            "/Users/kim/Library/Application Support/Grainy",
            "/Users/kim/Library/Application Support/Grainy/Library/catalog.sqlite",
            "/users/KIM/library/application support/GRAINY/mcp",
            "/Users/kim/Library/Application Support/GrainyRS/../Grainy/x",
        ] {
            assert!(is_oracle_folder(Path::new(path), &mac), "{path}");
        }
        for path in
            ["/Users/kim/Library/Application Support/GrainyRS/Library", "/Users/kim/Pictures/Grainy", "/Volumes/x"]
        {
            assert!(!is_oracle_folder(Path::new(path), &mac), "{path}");
        }
        // A user folder written decomposed (as Finder may) is the composed one.
        let composed = Locations {
            env: Environment { system: System::MacOs, local_app_data: None, home: PathBuf::from("/Users/\u{d55c}") },
            ..mac
        };
        assert!(is_oracle_folder(
            Path::new("/Users/\u{1112}\u{1161}\u{11ab}/Library/Application Support/Grainy"),
            &composed
        ));
    }

    #[test]
    fn arguments() {
        let parse = |a: &[&str]| Arguments::parse(a.iter().map(std::ffi::OsString::from));
        let a = parse(&["--library", "lib", "--debug", "a.png", "dir"]).unwrap();
        assert_eq!(a.library, Some(PathBuf::from("lib")));
        assert!(a.debug && !a.memory);
        assert_eq!(a.paths, vec![PathBuf::from("a.png"), PathBuf::from("dir")]);
        assert!(parse(&["--library"]).is_err());
        assert_eq!(parse(&["--smoke", "out", "a.png"]).unwrap().smoke, Some(PathBuf::from("out")));
        assert!(parse(&["--smoke"]).is_err());
        assert!(parse(&["--nope"]).is_err());
    }
}
