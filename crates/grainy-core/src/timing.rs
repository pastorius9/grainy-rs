//! Optional diagnostic spans for the oracle's load/develop/export pipeline.
//!
//! Set `GRAINY_TIMING=1` before starting the process. Disabled spans do not read
//! the clock, allocate, or write; only a cached boolean is checked at each span.

use std::sync::OnceLock;
use std::time::Instant;

/// Diagnostic span for a stage of `luma/engine.py export_image` or its callees.
pub struct Span<'a> {
    name: &'a str,
    start: Option<Instant>,
}

impl<'a> Span<'a> {
    /// Start an optional diagnostic span; timings are inclusive of nested spans.
    #[inline]
    pub fn new(name: &'a str) -> Self {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| std::env::var_os("GRAINY_TIMING").is_some_and(|v| v == "1"));
        Self { name, start: enabled.then(Instant::now) }
    }
}

impl Drop for Span<'_> {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            eprintln!("GRAINY_TIMING\t{}\t{:.3}", self.name, start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
