//! The local time zone of the oracle's `datetime.fromtimestamp(...)` and `datetime.astimezone()`.
//!
//! Normally that is the system's zone (`chrono::Local`). Golden tests fix it to the zone their reference data was
//! written in with [`fix_offset`], so that they give the same result on every machine (CI runners use UTC, the
//! goldens were written at UTC+9). Nothing in the application calls [`fix_offset`].

use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Utc};

/// No fixed offset: the system's zone.
const SYSTEM: i64 = i64::MIN;

static FIXED: AtomicI64 = AtomicI64::new(SYSTEM);

/// Makes every conversion of this module use a fixed offset from UTC (`seconds_east`, e.g. `9 * 3600` for the
/// zone the goldens were written in) instead of the system's zone, for the rest of the process. For tests.
pub fn fix_offset(seconds_east: i32) {
    FIXED.store(i64::from(seconds_east), Ordering::Relaxed);
}

/// The offset [`fix_offset`] set, if any.
fn fixed() -> Option<FixedOffset> {
    let seconds = FIXED.load(Ordering::Relaxed);
    if seconds == SYSTEM {
        None
    } else {
        i32::try_from(seconds).ok().and_then(FixedOffset::east_opt)
    }
}

/// The local wall time of an instant (`datetime.fromtimestamp(t)`, `aware.astimezone()` without its zone).
pub fn wall_time<Tz: TimeZone>(instant: &DateTime<Tz>) -> NaiveDateTime {
    match fixed() {
        Some(offset) => instant.with_timezone(&offset).naive_local(),
        None => instant.with_timezone(&Local).naive_local(),
    }
}

/// The local wall time of a file time (`datetime.fromtimestamp(stat().st_mtime)`).
pub fn wall_time_of(time: std::time::SystemTime) -> NaiveDateTime {
    wall_time(&DateTime::<Utc>::from(time))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_offset_moves_the_wall_time() {
        let instant = DateTime::parse_from_rfc3339("2027-01-15T12:00:00+00:00").unwrap();
        let system = instant.with_timezone(&Local).naive_local();
        assert_eq!(wall_time(&instant), system);
        fix_offset(9 * 3600);
        assert_eq!(wall_time(&instant).to_string(), "2027-01-15 21:00:00");
        let file = std::time::UNIX_EPOCH + std::time::Duration::from_secs(instant.timestamp() as u64);
        assert_eq!(wall_time_of(file).to_string(), "2027-01-15 21:00:00");
        FIXED.store(SYSTEM, Ordering::Relaxed);
        assert_eq!(wall_time(&instant), system);
    }
}
