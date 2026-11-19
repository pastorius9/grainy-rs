//! A minimal logger to standard error (the stubs log what they would do).

use log::{Level, LevelFilter, Metadata, Record};

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Info && metadata.target().starts_with("grainy")
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: Stderr = Stderr;

/// Installs the logger (once; later calls do nothing).
pub fn init() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}
