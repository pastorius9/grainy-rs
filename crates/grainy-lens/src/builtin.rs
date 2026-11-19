//! The Lensfun database that ships inside the program (`data/lensfun/`, unmodified; see `NOTICE`).
//!
//! The files are embedded in the executable, so a bare `grainy-app.exe` (or `cargo run`) finds a lens database
//! with no installation step and no folder to keep beside it. 4.3 MB of XML is small next to the program.

include!(concat!(env!("OUT_DIR"), "/builtin_files.rs"));

/// Number of embedded database files.
pub fn file_count() -> usize {
    FILES.len()
}
