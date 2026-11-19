//! Library housekeeping for Grainy: everything that moves, finds, copies or relinks photo files and keeps the
//! catalogue consistent with the disk. A port of the oracle's `library.py`, `file_access.py`,
//! `folder_locations.py`, `folder_sync.py`, `import_copy.py`, `quick_export.py`, `catalog_maintenance.py`,
//! `folder_recovery.py` and the numerical part of `workflow_data.py`.
//!
//! **Safety rule above all:** original photos are never overwritten or deleted by this crate. Moves and copies
//! refuse existing targets, copies are created exclusively and verified, and a source is removed only after a
//! verified copy (the cross-volume case of a move).

pub mod catalog_maintenance;
mod error;
pub mod file_access;
pub mod folder_locations;
pub mod folder_recovery;
pub mod folder_sync;
pub mod fsops;
pub mod import_copy;
pub mod library;
pub mod npx;
pub mod pathx;
mod profiles;
pub mod pyformat;
pub mod quick_export;
pub mod workflow_data;
pub mod xmp;

pub use error::{Cancel, Error, Result};
pub use grainy_catalog::folders::Platform;

use grainy_catalog::Catalog;

/// Port of Python's catalog connection context manager.
pub(crate) fn tx<T>(catalog: &Catalog, f: impl FnOnce() -> Result<T>) -> Result<T> {
    if catalog.is_reader() {
        return Err(Error::value("catalog is read-only"));
    }
    grainy_catalog::transaction::run(catalog.connection(), false, f)
}
