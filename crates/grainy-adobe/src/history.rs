//! Port of the pure parts of `luma/adobe_history.py`: Lightroom's timestamps and develop-history rows.
//! `transfer`, which writes the translated states into the catalog, belongs to the importer and is not ported
//! here.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use serde_json::Value;

use crate::develop::{convert, from_lua, Converted};
use crate::error::{ErrorKind, Result};
use crate::lua::Lua;

/// `adobe_history.HISTORY_PREFIX`.
pub const HISTORY_PREFIX: &str = "Lightroom 상태 · ";

/// Port of `adobe_history.adobe_time`: Lightroom stores seconds since 2001-01-01 UTC; the result is
/// `datetime.isoformat()` of that moment in UTC (`2001-01-01T00:00:00+00:00`, microseconds only when not zero).
/// `None` for a missing, non-numeric, non-finite or out-of-range value.
pub fn adobe_time(value: &Value) -> Option<String> {
    let seconds = match value {
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    if !seconds.is_finite() {
        return None;
    }
    // timedelta rounds the fraction to microseconds, half to even.
    let whole = seconds.trunc();
    let micros = (seconds.fract() * 1e6).round_ties_even();
    if whole.abs() > 1.0e12 {
        return None;
    }
    let total = whole as i64 * 1_000_000 + micros as i64;
    let epoch = NaiveDate::from_ymd_opt(2001, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let moment: NaiveDateTime = epoch.checked_add_signed(chrono::Duration::microseconds(total))?;
    if !(1..=9999).contains(&moment.year()) {
        return None;
    }
    let micro = moment.nanosecond() / 1000;
    let base = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        moment.year(),
        moment.month(),
        moment.day(),
        moment.hour(),
        moment.minute(),
        moment.second()
    );
    Some(if micro == 0 { format!("{base}+00:00") } else { format!("{base}.{micro:06}+00:00") })
}

/// Port of `adobe_history.decode_row(row, is_raw, orientation, warnings)`: translates one history or snapshot
/// row (`row['text']` holds the Lua develop table). Counts problems in `warnings` and returns `None` for a row
/// that cannot be used.
pub fn decode_row(
    row: &Value,
    is_raw: bool,
    orientation: Option<&Lua>,
    warnings: &mut BTreeMap<String, i64>,
) -> Result<Option<Converted>> {
    let Value::Object(fields) = row else {
        return Err(crate::error::Error::attribute(format!(
            "'{}' object has no attribute 'get'",
            crate::pyutil::json_type_name(row)
        )));
    };
    // A missing, empty or non-string `text` makes `from_lua` raise `DataError` in the oracle.
    let converted = match fields.get("text") {
        Some(Value::String(text)) => from_lua(text).and_then(|values| convert(&values, None, is_raw, orientation)),
        _ => Err(crate::error::Error::data("지원하지 않거나 손상된 데이터 구문입니다.")),
    };
    let result = match converted {
        Ok(result) => result,
        Err(e) if matches!(e.kind, ErrorKind::Data | ErrorKind::Value | ErrorKind::Type | ErrorKind::Overflow) => {
            *warnings.entry("develop_unreadable".into()).or_insert(0) += 1;
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    for (key, _) in &result.omitted.0 {
        *warnings.entry(format!("develop_field:{key}")).or_insert(0) += 1;
    }
    if result.mapped.is_empty() {
        *warnings.entry("develop_empty".into()).or_insert(0) += 1;
        return Ok(None);
    }
    Ok(Some(result))
}
