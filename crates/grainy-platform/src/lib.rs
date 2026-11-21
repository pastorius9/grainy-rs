//! Grainy's platform logic: where the app keeps its data, its settings file, the "what's new" notes, update
//! checks and installs, the crash-safe start and the feedback records. Plain logic over files and JSON, no UI.
//!
//! Everything reads and writes the same files in the same places as the Python Grainy (the oracle), so a person
//! can move from one to the other without losing settings. Ports of `luma/user_paths.py`, `preferences.py`,
//! `safe_start.py`, `whats_new.py`, `updater.py` and `feedback.py`.

pub mod feedback;
pub mod json;
pub mod monitor;
pub mod net;
pub mod preferences;
pub mod recycle;
pub mod safe_start;
pub mod updater;
pub mod user_paths;
mod util;
pub mod version;
pub mod whats_new;

pub use preferences::Preferences;
pub use user_paths::{local_data, Environment, System};
pub use util::{rename_new, wait_for_exit};

/// The oracle's `__version__` this port is made from.
pub const APP_VERSION: &str = "0.6.8";

/// The Rust build's own version: the workspace crate version. Updates compare against it and the update and About
/// texts show it; [`APP_VERSION`] stays where data is shared with the Python Grainy (what's-new notes, preference
/// records, safe start).
pub const RS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How this Rust build names its version where the receiving side sees it (the feedback form's version field and
/// the `User-Agent` of its requests): [`RS_VERSION`] with the prefix `rs-`, so its messages are told apart from the
/// Python Grainy's without changing the shape of the field.
pub const BUILD_VERSION: &str = concat!("rs-", env!("CARGO_PKG_VERSION"));

#[cfg(test)]
mod tests {
    #[test]
    fn the_build_version_is_the_rust_version_marked() {
        assert_eq!(super::BUILD_VERSION, format!("rs-{}", super::RS_VERSION));
        assert_eq!(super::RS_VERSION, env!("CARGO_PKG_VERSION"));
    }
}
