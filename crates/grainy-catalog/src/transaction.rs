//! Transaction boundaries from luma/catalog.py and command_batch.py.
use rusqlite::Connection;
/// Port of Python's connection context manager, with optional BEGIN IMMEDIATE.
pub fn run<T, E: From<rusqlite::Error>>(
    db: &Connection,
    immediate: bool,
    f: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    db.execute_batch(if immediate { "BEGIN IMMEDIATE" } else { "BEGIN" })?;
    match f() {
        Ok(v) => {
            if let Err(e) = db.execute_batch("COMMIT") {
                let _ = db.execute_batch("ROLLBACK");
                return Err(e.into());
            }
            Ok(v)
        }
        Err(e) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
