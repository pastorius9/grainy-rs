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

use std::path::{Path, PathBuf};

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

pub(crate) use grainy_platform::user_paths::is_within;

/// Whether a selected path lies inside the Python Grainy's protected data folders.
pub fn is_oracle_folder(path: &Path, locations: &Locations) -> bool {
    grainy_platform::user_paths::is_oracle_folder(path, &locations.env)
}

/// Check a user-selected output before creating, replacing or recycling it.
pub(crate) fn writable(path: &Path) -> std::io::Result<()> {
    if is_oracle_folder(path, &Locations::current()) {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the Python Grainy folder is read-only"))
    } else {
        Ok(())
    }
}

/// Publish a settings export without opening an existing photo or following a destination symlink.
pub(crate) fn write_json(path: &Path, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    writable(path)?;
    if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
        return Err(std::io::Error::other("a JSON filename is required"));
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(data.as_bytes())?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|e| e.error)?;
    Ok(())
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

    #[test]
    fn preset_output_cannot_truncate_a_photo() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("photo.tiff");
        std::fs::write(&original, b"generated").unwrap();
        assert!(write_json(&original, "{}").is_err());
        assert_eq!(std::fs::read(original).unwrap(), b"generated");
    }

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
        #[cfg(windows)]
        {
            let env = windows(r"C:\\Generated\\Local").env;
            assert!(grainy_platform::user_paths::contains_oracle(Path::new(r"C:\\Generated"), &env));
            assert!(!grainy_platform::user_paths::contains_oracle(Path::new(r"C:\\Generated\\Photos"), &env));
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
