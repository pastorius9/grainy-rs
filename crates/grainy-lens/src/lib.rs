//! Port of `luma/optics.py`: saved profiles, database matching, and the three lens modifier operations.
//! Models are independently derived from public documentation and black-box lensfunpy measurements.
//! No lensfun or lensfunpy implementation source was consulted; no native library is required at runtime.

mod builtin;
mod database;
mod interpolation;
mod model;
mod profile;
mod sqrt_table;

pub use database::{Calibration, Camera, Database, Lens, LensRecord};
pub use model::ProfileModel;
pub use profile::{description, key, make_profile, model_key, profile_lens, saved_lens, Conditions, Match};

/// Port of `optics` validation and database errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Port of `optics` ValueError, or an unsupported model/invalid image request.
    #[error("{0}")]
    Invalid(String),
    /// Port of `optics.lens_records` file errors.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Port of `optics._saved_lens` XML parse errors.
    #[error(transparent)]
    Xml(#[from] roxmltree::Error),
}

/// Port of the fallible `optics` operations.
pub type Result<T> = std::result::Result<T, Error>;

fn invalid(message: &str) -> Error {
    Error::Invalid(message.to_owned())
}
