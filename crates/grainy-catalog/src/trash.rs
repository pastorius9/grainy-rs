//! A recycle-bin move and a SQLite commit cannot be atomic together. Record the exact
//! photo identities before moving the file, so a committed OS move can be reconciled
//! if SQLite cannot commit or the app exits between the two.
use crate::{invalid, Catalog, Result};
use rusqlite::params;
use std::collections::BTreeSet;
use std::fs::{self, File, TryLockError};
use std::io::ErrorKind;
use std::path::Path;

const PREFIX: &str = "grainy:trash-pending:";
type PhotoIdentity = (i64, String);

/// NotFound is distinct from an inaccessible file. A dangling symlink is itself a file
/// that the recycle bin can move.
fn exists(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

impl Catalog {
    // All writers that move originals and all journal recovery share this lock. The OS
    // releases it on a crash. A catalog opened for ordinary browsing creates no lock file.
    fn trash_lock(&self) -> Result<Option<File>> {
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory().join("trash.lock"))?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(lock)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }

    fn identities_at(&self, path: &str) -> Result<Vec<PhotoIdentity>> {
        // The index is NOCASE; the binary comparison preserves distinct POSIX paths.
        let mut statement = self
            .connection()
            .prepare("SELECT id, imported FROM photos WHERE path=?1 COLLATE NOCASE AND path=?1 ORDER BY id")?;
        let identities = statement.query_map([path], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(identities.collect::<rusqlite::Result<_>>()?)
    }

    /// Removes an original and its virtual copies only when the bin accepts the move.
    /// Missing originals can be explicitly removed from the catalog too.
    pub fn trash_original(&self, path: &str, recycle: impl FnOnce(&Path) -> Result<()>) -> Result<Vec<i64>> {
        if self.is_reader() || !self.connection().is_autocommit() {
            return Err(invalid("recycling an original requires an independent writable catalog transaction"));
        }
        let _lock = self.trash_lock()?.ok_or_else(|| invalid("another recycle-bin operation is in progress"))?;
        let key = format!("{PREFIX}{path}");
        let identities = self.immediate(|| {
            let identities = self.identities_at(path)?;
            if !identities.is_empty() {
                self.connection().execute(
                    "INSERT OR REPLACE INTO preferences(key,value) VALUES(?,?)",
                    params![key, serde_json::to_string(&identities)?],
                )?;
            }
            Ok(identities)
        })?;
        if identities.is_empty() {
            return Ok(Vec::new());
        }
        let source = Path::new(path);
        let mut bin_started = false;
        let result = self.immediate(|| {
            if self.identities_at(path)? != identities {
                return Err(invalid("the catalog entries changed before the file could be recycled"));
            }
            let ids: Vec<_> = identities.iter().map(|(id, _)| *id).collect();
            // Detect statement/trigger failures before touching the original. All database
            // writes still roll back if the OS refuses the move.
            self.delete_photos(&ids)?;
            self.connection().execute("DELETE FROM preferences WHERE key=?", [&key])?;
            if exists(source)? {
                bin_started = true;
                recycle(source)?;
            }
            Ok(ids)
        });
        if result.is_err() && (!bin_started || exists(source).is_ok_and(|present| present)) {
            // If cleanup itself fails, reopening a catalog with the original still present
            // clears this intent without removing its metadata.
            let _ = self.immediate(|| {
                self.connection().execute("DELETE FROM preferences WHERE key=?", [&key])?;
                Ok(())
            });
        }
        result
    }

    pub(crate) fn recover_trash(&self) -> Result<()> {
        const PENDING: &str = "SELECT key,value FROM preferences WHERE key GLOB 'grainy:trash-pending:*'";
        if !self.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM preferences WHERE key GLOB 'grainy:trash-pending:*')",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(());
        }
        let Some(_lock) = self.trash_lock()? else { return Ok(()) };
        self.immediate(|| {
            let mut statement = self.connection().prepare(PENDING)?;
            let pending = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (key, record) in pending {
                let path = &key[PREFIX.len()..];
                let source = Path::new(path);
                match exists(source) {
                    Ok(true) => (), // OS move never happened, or the user restored the original.
                    Ok(false) if source.parent().is_some_and(Path::is_dir) => {
                        let saved: BTreeSet<PhotoIdentity> = serde_json::from_str(&record)?;
                        let ids = self
                            .identities_at(path)?
                            .into_iter()
                            .filter(|entry| saved.contains(entry))
                            .map(|(id, _)| id)
                            .collect::<Vec<_>>();
                        self.delete_photos(&ids)?;
                    }
                    _ => continue, // Offline or inaccessible volume: preserve edits and intent.
                }
                self.connection().execute("DELETE FROM preferences WHERE key=?", [&key])?;
            }
            Ok(())
        })
    }
}
