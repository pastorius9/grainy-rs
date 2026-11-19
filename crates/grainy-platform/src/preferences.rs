//! Port of luma/preferences.py: the application settings file, independent of any photo library.
//!
//! The file is `settings.json` in `local_data('Grainy')`, written as Python's `json.dump(..., ensure_ascii=False,
//! indent=2)` through a temporary file in the same folder that is flushed, synced and renamed over the old one.

use std::io::Write;
use std::path::{Path, PathBuf};

use grainy_i18n::Language;

use crate::json::{Object, Value};
use crate::user_paths::{local_data, Environment};

/// The line end the settings file is written with (Python's `os.linesep`): CRLF on Windows, LF elsewhere.
pub const LINE_END: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// What `save_language` can fail with.
#[derive(Debug, thiserror::Error)]
pub enum PreferencesError {
    /// `save_language` was given something other than `ko` or `en` (the oracle raises `ValueError`).
    #[error("Unsupported interface language")]
    UnsupportedLanguage,
    /// The file could not be written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Port of `Preferences`.
#[derive(Debug, Clone)]
pub struct Preferences {
    /// The settings file.
    pub path: PathBuf,
    /// The values as last read or saved.
    pub values: Object,
}

fn read_object(path: &Path) -> Option<Object> {
    let text = String::from_utf8(std::fs::read(path).ok()?).ok()?;
    match Value::parse(&text) {
        Ok(Value::Object(object)) => Some(object),
        _ => None,
    }
}

fn replace_file(temporary: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    // A failed exclusive create does not give us ownership of an existing temporary file.
    let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(temporary)?;
    let result = (|| {
        output.write_all(text.as_bytes())?;
        output.flush()?;
        output.sync_all()?;
        drop(output);
        std::fs::rename(temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

impl Preferences {
    /// Reads the file at `path`; a missing, damaged or non-object file gives no values.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let values = read_object(&path).unwrap_or_default();
        Preferences { path, values }
    }

    /// `Preferences()` without a path: `settings.json` in the Grainy folder of this machine.
    pub fn at_default(env: &Environment) -> Self {
        Self::new(local_data(env, "Grainy", None).join("settings.json"))
    }

    /// Port of the `language` property: `ko` or `en` when stored, else none.
    pub fn language(&self) -> Option<Language> {
        Language::parse(self.values.get("language")?.as_str()?).ok()
    }

    /// Port of `use_gpu`: on unless the file says `false`.
    pub fn use_gpu(&self) -> bool {
        !matches!(self.values.get("use_gpu"), Some(Value::Bool(false)))
    }

    /// Port of `save_language`.
    pub fn save_language(&mut self, value: &str) -> Result<(), PreferencesError> {
        if Language::parse(value).is_err() {
            return Err(PreferencesError::UnsupportedLanguage);
        }
        Ok(self.save("language", Value::from(value))?)
    }

    /// Port of `save_use_gpu`.
    pub fn save_use_gpu(&mut self, value: bool) -> std::io::Result<()> {
        self.save("use_gpu", Value::Bool(value))
    }

    /// Port of `stored`: the file as it is now (another writer, the feedback dialog's, may have written to it);
    /// the values held here when the file cannot be read.
    pub fn stored(&self) -> Object {
        read_object(&self.path).unwrap_or_else(|| self.values.clone())
    }

    /// Port of `save`: merges `key` into what is stored now and replaces the file atomically.
    pub fn save(&mut self, key: &str, value: Value) -> std::io::Result<()> {
        let mut updated = self.stored();
        updated.insert(key, value);
        let folder = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        if !folder.as_os_str().is_empty() {
            std::fs::create_dir_all(&folder)?;
        }
        let temporary = folder.join(format!("tmp{}.tmp", &crate::util::unique_hex()[..8]));
        // The oracle writes in text mode: line ends become the system's (CRLF on Windows).
        let text = Value::Object(updated.clone()).dump(Some(2), false).replace('\n', LINE_END);
        replace_file(&temporary, &self.path, &text)?;
        self.values = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temporary_name_collision_preserves_both_existing_files() {
        let root = std::env::temp_dir().join(format!("grainy-preferences-review-{}", crate::util::unique_hex()));
        std::fs::create_dir(&root).unwrap();
        let temporary = root.join("collision.tmp");
        let settings = root.join("settings.json");
        std::fs::write(&temporary, b"another writer's data").unwrap();
        std::fs::write(&settings, b"old settings").unwrap();
        assert_eq!(
            replace_file(&temporary, &settings, "new settings").unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&temporary).unwrap(), b"another writer's data");
        assert_eq!(std::fs::read(&settings).unwrap(), b"old settings");
        std::fs::remove_dir_all(root).unwrap();
    }
}
