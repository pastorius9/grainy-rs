//! Transaction boundaries from luma/catalog.py and command_batch.py.
use rusqlite::Connection;
/// Port of Python's connection context manager, with optional BEGIN IMMEDIATE.
pub fn run<T, E: From<rusqlite::Error>>(
    db: &Connection,
    immediate: bool,
    f: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    if db.is_autocommit() {
        let behavior =
            if immediate { rusqlite::TransactionBehavior::Immediate } else { rusqlite::TransactionBehavior::Deferred };
        let transaction = rusqlite::Transaction::new_unchecked(db, behavior)?;
        let result = f()?;
        transaction.commit()?;
        return Ok(result);
    }
    // Each inner operation keeps its own rollback boundary. SQLite resolves repeated
    // savepoint names to the most recently opened one.
    db.execute_batch("SAVEPOINT grainy_operation")?;
    match f() {
        Ok(value) => {
            db.execute_batch("RELEASE grainy_operation")?;
            Ok(value)
        }
        Err(error) => {
            db.execute_batch("ROLLBACK TO grainy_operation; RELEASE grainy_operation")?;
            Err(error)
        }
    }
}
