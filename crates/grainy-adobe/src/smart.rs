//! Port of `luma/smart_rules.py`: validated Lightroom search descriptors, evaluated against live photo
//! metadata. Nothing here reads the clock: `evaluate` takes `now`.

use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::lua::{parse, sequence, Key, Limits, Lua, Table};
use crate::preset::RAW_EXTENSIONS;
use crate::pyutil::{
    casefold, date_from_iso, datetime_from_iso, is_space, json_repr, json_str, now_local, parse_float, strip,
    to_zone_naive, truthy, WinPath, Zone,
};

/// `smart_rules.KEY`: where a smart collection's validated rules are stored.
pub const KEY: &str = "_adobe_smart";
/// `smart_rules.VERSION`.
pub const VERSION: i64 = 1;
/// `engine.VIDEO_EXTENSIONS`.
pub const VIDEO_EXTENSIONS: [&str; 5] = [".mp4", ".mov", ".avi", ".mkv", ".m4v"];

/// `smart_rules.NUMBERS`: Lightroom field to photo field.
pub const NUMBERS: [(&str, &str); 3] = [("rating", "rating"), ("pick", "flag"), ("isoSpeedRating", "iso")];
/// `smart_rules.TEXT`.
pub const TEXT: [(&str, &str); 15] = [
    ("filename", "filename"),
    ("copyname", "copyname"),
    ("folder", "folder"),
    ("labelText", "label"),
    ("title", "title"),
    ("caption", "caption"),
    ("keywords", "keywords"),
    ("camera", "camera"),
    ("cameraSN", "serial"),
    ("lens", "lens"),
    ("country", "country"),
    ("state", "state"),
    ("city", "city"),
    ("location", "location"),
    ("creator", "creator"),
];
const TEXT_JOB: (&str, &str) = ("jobIdentifier", "job");
/// `smart_rules.DATE`.
pub const DATE: [(&str, &str); 2] = [("captureTime", "date"), ("touchTime", "edited")];
const COMPARISONS: [&str; 7] = ["==", "!=", ">", "<", ">=", "<=", "in"];
const TEXT_OPS: [&str; 10] =
    ["any", "all", "words", "noneOf", "beginsWith", "endsWith", "empty", "notEmpty", "==", "!="];
const DATE_OPS: [&str; 12] =
    ["==", "!=", ">", "<", "in", "inLast", "notInLast", "today", "yesterday", "thisWeek", "thisMonth", "thisYear"];
const RELATIVE_OPS: [&str; 7] = ["inLast", "notInLast", "today", "yesterday", "thisWeek", "thisMonth", "thisYear"];
const FORMATS: [&str; 10] = ["DNG", "RAW", "JPG", "TIFF", "PNG", "PSD", "VIDEO", "PSB", "AVIF", "JXL"];

fn text_field(field: &str) -> Option<&'static str> {
    TEXT.iter().chain(std::iter::once(&TEXT_JOB)).find(|(k, _)| *k == field).map(|(_, v)| *v)
}

fn number_field(field: &str) -> Option<&'static str> {
    NUMBERS.iter().find(|(k, _)| *k == field).map(|(_, v)| *v)
}

fn date_field(field: &str) -> Option<&'static str> {
    DATE.iter().find(|(k, _)| *k == field).map(|(_, v)| *v)
}

/// `smart_rules.LABELS[key]`: `Some(Some(label))`, `Some(None)` for `'custom'`, `None` when not a key.
fn label_of(value: &Lua) -> Option<Option<&'static str>> {
    match value {
        Lua::Int(1) => Some(Some("빨강")),
        Lua::Int(2) => Some(Some("노랑")),
        Lua::Int(3) => Some(Some("초록")),
        Lua::Int(4) => Some(Some("파랑")),
        Lua::Int(5) => Some(Some("보라")),
        Lua::Str(s) if s == "none" => Some(Some("")),
        Lua::Str(s) if s == "custom" => Some(None),
        _ => None,
    }
}

fn label_of_json(value: &Value) -> Option<Option<&'static str>> {
    match value {
        Value::Number(n) if n.as_i64().is_some() && !n.is_f64() => {
            label_of(&Lua::Int(i128::from(n.as_i64().unwrap_or(0))))
        }
        Value::String(s) => label_of(&Lua::Str(s.clone())),
        _ => None,
    }
}

fn lua_json(value: &Lua) -> Value {
    match value {
        Lua::Nil => Value::Null,
        Lua::Bool(b) => Value::Bool(*b),
        Lua::Int(i) => match i64::try_from(*i) {
            Ok(n) => Value::from(n),
            Err(_) => match u64::try_from(*i) {
                Ok(n) => Value::from(n),
                Err(_) => serde_json::Number::from_f64(*i as f64).map_or(Value::Null, Value::Number),
            },
        },
        Lua::Huge(f) | Lua::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Lua::Str(s) => Value::String(s.clone()),
        Lua::Table(_) | Lua::List(_) => Value::Null,
    }
}

fn lua_as_f64(value: &Lua) -> Option<f64> {
    match value {
        Lua::Int(i) => Some(*i as f64),
        Lua::Huge(f) | Lua::Float(f) => Some(*f),
        _ => None,
    }
}

fn lua_cmp(a: &Lua, b: &Lua) -> Ordering {
    if let (Lua::Int(x), Lua::Int(y)) = (a, b) {
        return x.cmp(y);
    }
    lua_as_f64(a).unwrap_or(0.0).partial_cmp(&lua_as_f64(b).unwrap_or(0.0)).unwrap_or(Ordering::Equal)
}

/// Port of `smart_rules.finite`: a finite number (not a boolean), as given.
pub fn finite(value: &Lua) -> Result<&Lua> {
    match value {
        Lua::Int(_) | Lua::Float(_) => Ok(value),
        Lua::Huge(f) if f.is_infinite() => Err(Error::overflow("int too large to convert to float")),
        Lua::Huge(_) => Ok(value),
        _ => Err(Error::data("숫자 검색 조건이 올바르지 않습니다.")),
    }
}

/// Port of `smart_rules.translate`: parses a Lightroom search definition and returns `{KEY: validated rules}`.
pub fn translate(text: &str) -> Result<Map<String, Value>> {
    let node = parse(text, Limits { max_bytes: 1024 * 1024, max_items: 10_000, max_depth: 32 })?;
    let node = match node.get_str("value") {
        Some(inner @ Lua::Table(_)) => inner.clone(),
        _ => Lua::Table(node),
    };
    let mut out = Map::new();
    out.insert(KEY.into(), validate(&node, 0)?);
    Ok(out)
}

fn iso_date(value: &Lua) -> Result<String> {
    match value {
        Lua::Str(s) => match date_from_iso(s) {
            Some(d) => Ok(d.format("%Y-%m-%d").to_string()),
            None => Err(Error::value(format!("Invalid isoformat string: '{s}'"))),
        },
        _ => Err(Error::type_error("fromisoformat: argument must be str")),
    }
}

fn map_of(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// Port of `smart_rules.validate`: checks a parsed search tree and returns it in Grainy's own shape
/// (`combine`/`children`, or `criteria`/`operation`/`value`...).
pub fn validate(node: &Lua, depth: usize) -> Result<Value> {
    let table = match node {
        Lua::Table(t) if depth <= 24 => t,
        _ => return Err(Error::data("검색 조건의 중첩이 지원 범위를 넘었습니다.")),
    };
    if !table.contains_str("criteria") {
        let mode = table.get_str("combine").cloned().unwrap_or_else(|| Lua::Str("intersect".into()));
        let mode_name = match &mode {
            Lua::Str(s) if matches!(s.as_str(), "union" | "intersect" | "exclude") => s.clone(),
            _ => return Err(Error::data("지원하지 않는 검색 조합입니다.")),
        };
        if table.keys().any(|k| !matches!(k, Key::Int(_) | Key::Huge(_)) && *k != Key::Str("combine".into())) {
            return Err(Error::data("지원하지 않는 검색 조합입니다."));
        }
        let mut ints = Table::new();
        for (k, v) in table.iter() {
            if matches!(k, Key::Int(_) | Key::Huge(_)) {
                ints.set(k.clone(), v.clone());
            }
        }
        let children = sequence(&Lua::Table(ints))?;
        if children.is_empty() || children.len() > 1000 {
            return Err(Error::data("검색 조건이 비어 있거나 너무 많습니다."));
        }
        let validated: Result<Vec<Value>> = children.iter().map(|c| validate(c, depth + 1)).collect();
        return Ok(map_of(vec![("combine", Value::String(mode_name)), ("children", Value::Array(validated?))]));
    }
    let allowed = ["criteria", "operation", "value", "value2", "value_units"];
    if table.keys().any(|k| !matches!(k, Key::Str(s) if allowed.contains(&s.as_str()))) {
        return Err(Error::data("추가 검색 옵션은 아직 변환하지 않습니다."));
    }
    let nil = Lua::Nil;
    let value = table.get_str("value").unwrap_or(&nil);
    let (Some(Lua::Str(field)), Some(Lua::Str(op))) = (table.get_str("criteria"), table.get_str("operation")) else {
        return Err(Error::data("검색 항목과 비교 방식은 문자열이어야 합니다."));
    };
    let (field, op) = (field.as_str(), op.as_str());
    let mut result = Map::new();
    result.insert("criteria".into(), Value::String(field.into()));
    result.insert("operation".into(), Value::String(op.into()));
    let empty_ops = op == "empty" || op == "notEmpty";
    if number_field(field).is_some() {
        if !COMPARISONS.contains(&op) {
            return Err(Error::data("지원하지 않는 숫자 비교입니다."));
        }
        let first = finite(value)?;
        result.insert("value".into(), lua_json(first));
        if op == "in" {
            let second = finite(table.get_str("value2").unwrap_or(&nil))?;
            result.insert("value2".into(), lua_json(second));
            if lua_cmp(first, second) == Ordering::Greater {
                return Err(Error::data("검색 범위의 시작이 끝보다 큽니다."));
            }
        }
    } else if text_field(field).is_some() {
        if !TEXT_OPS.contains(&op) || (!empty_ops && !matches!(value, Lua::Str(_))) {
            return Err(Error::data("지원하지 않는 문자 비교입니다."));
        }
        if !empty_ops {
            result.insert("value".into(), lua_json(value));
        }
    } else if date_field(field).is_some() {
        if !DATE_OPS.contains(&op) {
            return Err(Error::data("지원하지 않는 날짜 비교입니다."));
        }
        if op == "inLast" || op == "notInLast" {
            let n = finite(value)?;
            let units = table.get_str("value_units");
            let integral = match n {
                Lua::Int(_) => true,
                Lua::Float(f) => f.fract() == 0.0,
                _ => false,
            };
            let unit_ok = matches!(units, Some(Lua::Str(u)) if ["hours", "days", "weeks", "months", "years"].contains(&u.as_str()));
            if lua_cmp(n, &Lua::Int(0)) == Ordering::Less
                || lua_cmp(n, &Lua::Int(10000)) == Ordering::Greater
                || !integral
                || !unit_ok
            {
                return Err(Error::data("지원하지 않는 상대 날짜 범위입니다."));
            }
            let count = lua_as_f64(n).unwrap_or(0.0) as i64;
            result.insert("value".into(), Value::from(count));
            result.insert("value_units".into(), lua_json(units.unwrap_or(&nil)));
        } else if !RELATIVE_OPS.contains(&op) {
            let first = iso_date(value)?;
            result.insert("value".into(), Value::String(first.clone()));
            if op == "in" {
                let second = iso_date(table.get_str("value2").unwrap_or(&nil))?;
                result.insert("value2".into(), Value::String(second.clone()));
                if first > second {
                    return Err(Error::data("날짜 범위의 시작이 끝보다 큽니다."));
                }
            }
        }
    } else if field == "labelColor" {
        if !(op == "==" || op == "!=")
            || !matches!(value, Lua::Int(_) | Lua::Huge(_) | Lua::Str(_))
            || label_of(value).is_none()
        {
            return Err(Error::data("지원하지 않는 색 라벨 조건입니다."));
        }
        result.insert("value".into(), lua_json(value));
    } else if field == "fileFormat" {
        if !(op == "==" || op == "!=") || !matches!(value, Lua::Str(s) if FORMATS.contains(&s.as_str())) {
            return Err(Error::data("지원하지 않는 파일 형식 조건입니다."));
        }
        result.insert("value".into(), lua_json(value));
    } else if field == "hasGPSData" {
        if op != "isTrue" && op != "isFalse" {
            return Err(Error::data("지원하지 않는 GPS 검색 조건입니다."));
        }
    } else {
        return Err(Error::data(format!("현재 변환하지 않는 검색 항목: {field}")));
    }
    Ok(Value::Object(result))
}

fn as_number(value: &Value) -> Result<f64> {
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        _ => Err(Error::type_error("unsupported operand type for comparison")),
    }
}

/// Port of `smart_rules._compare` for numbers: `actual` against `value` (and `value2` for `in`); an unknown
/// operator is false.
pub fn compare_number(actual: f64, op: &str, value: &Value, value2: Option<&Value>) -> Result<bool> {
    Ok(match op {
        // `==` between a number and a non-number is simply false (no error).
        "==" => as_number(value).is_ok_and(|v| actual == v),
        "!=" => !as_number(value).is_ok_and(|v| actual == v),
        ">" => actual > as_number(value)?,
        "<" => actual < as_number(value)?,
        ">=" => actual >= as_number(value)?,
        "<=" => actual <= as_number(value)?,
        "in" => as_number(value)? <= actual && actual <= as_number(value2.unwrap_or(&Value::Null))?,
        _ => false,
    })
}

/// Port of `smart_rules._compare` for strings (dates as ISO text, file formats).
pub fn compare_text(actual: &str, op: &str, value: &Value, value2: Option<&Value>) -> Result<bool> {
    let text = |v: &Value| -> Result<String> {
        match v {
            Value::String(s) => Ok(s.clone()),
            _ => Err(Error::type_error("comparison between str and a non-str value")),
        }
    };
    Ok(match op {
        "==" => Some(actual) == value.as_str(),
        "!=" => Some(actual) != value.as_str(),
        ">" => actual > text(value)?.as_str(),
        "<" => actual < text(value)?.as_str(),
        ">=" => actual >= text(value)?.as_str(),
        "<=" => actual <= text(value)?.as_str(),
        "in" => text(value)?.as_str() <= actual && actual <= text(value2.unwrap_or(&Value::Null))?.as_str(),
        _ => false,
    })
}

/// Port of `smart_rules._date`: a stored capture or edit time as a local wall-clock datetime, or `None`.
/// EXIF `YYYY:MM:DD` dates are accepted; a time with a zone is converted to the computer's local zone.
pub fn parse_date(value: &Value) -> Option<NaiveDateTime> {
    parse_date_in(value, Zone::Local)
}

/// [`parse_date`] with the zone a time carrying an offset is converted to made explicit.
pub fn parse_date_in(value: &Value, zone: Zone) -> Option<NaiveDateTime> {
    if !truthy(value) {
        return None;
    }
    let mut text = json_str(value);
    let b = text.as_bytes();
    if b.len() >= 7
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b':'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b.get(7) == Some(&b':')
    {
        text = text.replacen(':', "-", 2);
    }
    let parsed = datetime_from_iso(&text)?;
    match parsed.offset {
        Some(offset) => to_zone_naive(parsed.naive, offset, zone),
        None => Some(parsed.naive),
    }
}

/// One end of a relative window: a date, or an exact time for windows in hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// A calendar date.
    Date(NaiveDate),
    /// A date and time.
    Time(NaiveDateTime),
}

fn days(n: i64) -> Result<Duration> {
    Duration::try_days(n).ok_or_else(|| Error::overflow("days=... must have magnitude <= 999999999"))
}

fn minus_days(date: NaiveDate, n: i64) -> Result<NaiveDate> {
    date.checked_sub_signed(days(n)?).ok_or_else(|| Error::overflow("date value out of range"))
}

fn rule_count(value: &Value) -> Result<f64> {
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        _ => Err(Error::type_error("unsupported operand type for relative count")),
    }
}

/// Port of `smart_rules._window(node, now)`: the first and last moment of a relative date rule.
pub fn window(node: &Value, now: NaiveDateTime) -> Result<(Moment, Moment)> {
    let op = node.get("operation").and_then(Value::as_str).ok_or_else(|| Error::key("operation"))?;
    let today = now.date();
    let both = |a: NaiveDate, b: NaiveDate| Ok((Moment::Date(a), Moment::Date(b)));
    match op {
        "today" => return both(today, today),
        "yesterday" => {
            let y = minus_days(today, 1)?;
            return both(y, y);
        }
        "thisWeek" => return both(minus_days(today, i64::from(today.weekday().num_days_from_monday()))?, today),
        "thisMonth" => return both(today.with_day(1).ok_or_else(|| Error::value("day is out of range"))?, today),
        "thisYear" => {
            return both(
                NaiveDate::from_ymd_opt(today.year(), 1, 1).ok_or_else(|| Error::value("month must be in 1..12"))?,
                today,
            )
        }
        _ => {}
    }
    // `n, unit = node['value'], node['value_units']`: both lookups come before the value is used.
    let raw = node.get("value").ok_or_else(|| Error::key("value"))?;
    let unit_value = node.get("value_units").ok_or_else(|| Error::key("value_units"))?;
    let unit = unit_value.as_str().unwrap_or("");
    let n = rule_count(raw)?;
    match unit {
        "hours" => {
            let micros = (n * 3.6e9).round_ties_even();
            if !micros.is_finite() || micros.abs() > 8.0e18 {
                return Err(Error::overflow("hours out of range"));
            }
            let start = now
                .checked_sub_signed(Duration::microseconds(micros as i64))
                .ok_or_else(|| Error::overflow("date value out of range"))?;
            return Ok((Moment::Time(start), Moment::Time(now)));
        }
        "days" | "weeks" => {
            // `date - timedelta(days=x)` only uses the whole days of the timedelta (its floor).
            let whole = (n * if unit == "weeks" { 7.0 } else { 1.0 }).floor();
            if !whole.is_finite() || whole.abs() > 1.0e9 {
                return Err(Error::overflow("days out of range"));
            }
            return both(minus_days(today, whole as i64)?, today);
        }
        _ => {}
    }
    if n.fract() != 0.0 {
        return Err(Error::type_error("integer argument expected, got float"));
    }
    let months = n as i64 * if unit == "years" { 12 } else { 1 };
    let absolute = i64::from(today.year()) * 12 + i64::from(today.month()) - 1 - months;
    let year = absolute.div_euclid(12);
    let month = absolute.rem_euclid(12) + 1;
    if year < 1 {
        let min = NaiveDate::from_ymd_opt(1, 1, 1).ok_or_else(|| Error::value("date.min"))?;
        return both(min, today);
    }
    let year = year as i32;
    let month = month as u32;
    let last = last_day(year, month);
    let day = today.day().min(last);
    both(NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| Error::value("day is out of range for month"))?, today)
}

fn last_day(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1).and_then(|d| d.pred_opt()).map_or(28, |d| d.day())
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn has_word(haystack: &[char], word: &[char]) -> bool {
    if word.is_empty() || haystack.len() < word.len() {
        return false;
    }
    (0..=haystack.len() - word.len()).any(|i| {
        haystack[i..i + word.len()] == *word
            && (i == 0 || !word_char(haystack[i - 1]))
            && haystack.get(i + word.len()).is_none_or(|c| !word_char(*c))
    })
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

/// Python's `float(value)` of a JSON value, `None` where it raises (`TypeError` or `ValueError`).
fn py_float(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => parse_float(s),
        _ => None,
    }
}

struct Context<'a> {
    zone: Zone,
    values: HashMap<String, Value>,
    info: &'a Map<String, Value>,
    now: NaiveDateTime,
    path: WinPath,
}

fn lookup<'a>(values: &'a HashMap<String, Value>, key: &str) -> Option<&'a Value> {
    values.get(key)
}

fn require_object(node: &Value) -> Result<()> {
    match node {
        Value::Object(_) => Ok(()),
        other => Err(Error::type_error(format!(
            "argument of type '{}' is not iterable",
            crate::pyutil::json_type_name(other)
        ))),
    }
}

fn test(rule: &Value, cx: &Context) -> Result<bool> {
    require_object(rule)?;
    if let Some(children) = rule.get("children").filter(|_| rule.get("combine").is_some()) {
        let combine = rule.get("combine").and_then(Value::as_str).unwrap_or("");
        let list = children.as_array().ok_or_else(|| {
            Error::type_error(match children {
                Value::String(_) | Value::Object(_) => "string indices must be integers".to_owned(),
                other => format!("'{}' object is not iterable", crate::pyutil::json_type_name(other)),
            })
        })?;
        // `any`/`all` stop at the first decisive child (and so at the first error that comes before it).
        match combine {
            "union" => {
                for c in list {
                    if test(c, cx)? {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
            "exclude" => {
                for c in list {
                    if test(c, cx)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            _ => {
                for c in list {
                    if !test(c, cx)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
    }
    if rule.get("combine").is_some() {
        return Err(Error::key("children"));
    }
    let field = rule.get("criteria").and_then(Value::as_str).ok_or_else(|| Error::key("criteria"))?;
    let op = rule.get("operation").and_then(Value::as_str).ok_or_else(|| Error::key("operation"))?;
    let expected = rule.get("value");
    let null = Value::Null;
    if let Some(key) = number_field(field) {
        let Some(actual) = py_float(lookup(&cx.values, key)) else {
            return Ok(false);
        };
        return Ok(actual.is_finite() && compare_number(actual, op, expected.unwrap_or(&null), rule.get("value2"))?);
    }
    if let Some(key) = text_field(field) {
        let raw = lookup(&cx.values, key);
        let actual = match raw {
            Some(v) if truthy(v) => casefold(&json_str(v)),
            _ => String::new(),
        };
        if op == "empty" {
            return Ok(strip(&actual).is_empty());
        }
        if op == "notEmpty" {
            return Ok(!strip(&actual).is_empty());
        }
        let Some(Value::String(raw_value)) = expected else {
            return Err(Error::attribute("object has no attribute 'casefold'"));
        };
        let mut value = casefold(raw_value);
        if field == "labelText" {
            value = match value.as_str() {
                "red" => "빨강".into(),
                "yellow" => "노랑".into(),
                "green" => "초록".into(),
                "blue" => "파랑".into(),
                "purple" => "보라".into(),
                _ => value,
            };
        }
        if op == "==" || op == "!=" {
            return compare_text(&actual, op, &Value::String(value), None);
        }
        if op == "beginsWith" {
            return Ok(actual.starts_with(&value));
        }
        if op == "endsWith" {
            return Ok(actual.ends_with(&value));
        }
        let words: Vec<&str> = value.split(|c: char| is_space(c) || c == ',').filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            return Ok(false);
        }
        if op == "words" {
            let hay: Vec<char> = actual.chars().collect();
            return Ok(words.iter().all(|w| has_word(&hay, &w.chars().collect::<Vec<_>>())));
        }
        let hits: Vec<bool> = words.iter().map(|w| contains(&actual, w)).collect();
        if op == "all" {
            return Ok(hits.iter().all(|h| *h));
        }
        if op == "noneOf" {
            return Ok(!hits.iter().any(|h| *h));
        }
        return Ok(hits.iter().any(|h| *h));
    }
    if let Some(key) = date_field(field) {
        let Some(actual) = lookup(&cx.values, key).and_then(|v| parse_date_in(v, cx.zone)) else {
            return Ok(false);
        };
        if matches!(op, "==" | "!=" | ">" | "<" | "in") {
            let day = actual.date().format("%Y-%m-%d").to_string();
            return compare_text(&day, op, expected.unwrap_or(&null), rule.get("value2"));
        }
        let (start, end) = window(rule, cx.now)?;
        let found = match (start, end) {
            (Moment::Time(s), Moment::Time(e)) => s <= actual && actual <= e,
            (Moment::Date(s), Moment::Date(e)) => s <= actual.date() && actual.date() <= e,
            _ => false,
        };
        return Ok(if op == "notInLast" { !found } else { found });
    }
    if field == "labelColor" {
        let label = match lookup(&cx.values, "label") {
            Some(v) if truthy(v) => v.clone(),
            _ => Value::String(String::new()),
        };
        let e = expected.unwrap_or(&null);
        let target = match label_of_json(e) {
            Some(t) => t,
            None if e.as_str() == Some("custom") => None,
            None if matches!(e, Value::Array(_) | Value::Object(_)) => {
                return Err(Error::type_error(format!("unhashable type: '{}'", crate::pyutil::json_type_name(e))))
            }
            None => return Err(Error::key_repr(json_repr(e))),
        };
        let known = ["빨강", "노랑", "초록", "파랑", "보라", ""];
        if matches!(label, Value::Array(_) | Value::Object(_)) {
            return Err(Error::type_error("unhashable type"));
        }
        let found = if e.as_str() == Some("custom") {
            // `label not in set(LABELS.values())`
            !matches!(&label, Value::String(s) if known.contains(&s.as_str()))
        } else {
            matches!((&label, target), (Value::String(s), Some(t)) if s == t)
        };
        return Ok(if op == "==" { found } else { !found });
    }
    if field == "fileFormat" {
        let ext = cx.path.suffix().to_lowercase();
        let actual = if ext == ".dng" {
            "DNG".to_owned()
        } else if RAW_EXTENSIONS.contains(&ext.as_str()) {
            "RAW".to_owned()
        } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            "VIDEO".to_owned()
        } else {
            let bare: String = ext.chars().skip(1).collect();
            match bare.as_str() {
                "jpg" | "jpeg" => "JPG".to_owned(),
                "tif" | "tiff" => "TIFF".to_owned(),
                _ => bare.to_uppercase(),
            }
        };
        return compare_text(&actual, op, expected.unwrap_or(&null), None);
    }
    if field == "hasGPSData" {
        let found = match (py_float(cx.info.get("latitude")), py_float(cx.info.get("longitude"))) {
            (Some(lat), Some(lon)) => (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon),
            _ => false,
        };
        return Ok(if op == "isTrue" { found } else { !found });
    }
    Ok(false)
}

fn json_object(text: &Value) -> Result<Map<String, Value>> {
    let source = match text {
        Value::String(s) if !s.is_empty() => s.as_str(),
        Value::Null | Value::String(_) => "{}",
        Value::Bool(false) => "{}",
        Value::Number(n) if n.as_f64() == Some(0.0) => "{}",
        Value::Array(a) if a.is_empty() => "{}",
        Value::Object(o) if o.is_empty() => "{}",
        _ => return Err(Error::type_error("the JSON object must be str, bytes or bytearray")),
    };
    match serde_json::from_str::<Value>(source) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(Error::type_error("'list' object is not a mapping")),
        Err(e) => Err(Error::value(e.to_string())),
    }
}

/// Port of `smart_rules.evaluate(node, photo, *, now=None, info=None)`: whether a photo (its catalog row as
/// JSON, with `path`, `metadata`, `extras` and the columns the rules read) matches the rules. `now` is the
/// local time relative date rules count from (the computer's clock when `None`); `info` the photo's merged
/// metadata (decoded from `metadata` and `extras` when `None`).
pub fn evaluate(
    node: &Value,
    photo: &Map<String, Value>,
    now: Option<NaiveDateTime>,
    info: Option<&Map<String, Value>>,
) -> Result<bool> {
    evaluate_in(node, photo, now, info, Zone::Local)
}

/// [`evaluate`] with the zone explicit: times that carry an offset are converted to `zone` before their date
/// is compared (the oracle always uses the computer's zone, [`Zone::Local`]).
pub fn evaluate_in(
    node: &Value,
    photo: &Map<String, Value>,
    now: Option<NaiveDateTime>,
    info: Option<&Map<String, Value>>,
    zone: Zone,
) -> Result<bool> {
    let now = now.unwrap_or_else(now_local);
    let merged;
    let info = match info {
        Some(i) => i,
        None => {
            let mut m = json_object(photo.get("metadata").unwrap_or(&Value::Null))?;
            for (k, v) in json_object(photo.get("extras").unwrap_or(&Value::Null))? {
                m.insert(k, v);
            }
            merged = m;
            &merged
        }
    };
    let path_text = match photo.get("path") {
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err(Error::type_error("argument should be a str or an os.PathLike object")),
        None => return Err(Error::key("path")),
    };
    let path = WinPath::new(&path_text);
    let mut values: HashMap<String, Value> = info.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (k, v) in photo {
        values.insert(k.clone(), v.clone());
    }
    values.insert("filename".into(), Value::String(path.name()));
    values.insert("folder".into(), Value::String(path.parent()));
    values.insert(
        "date".into(),
        info.get("DateTimeOriginal").or_else(|| info.get("date")).cloned().unwrap_or(Value::Null),
    );
    values.insert("edited".into(), info.get("_last_edit_time").cloned().unwrap_or(Value::Null));
    values.insert("copyname".into(), info.get("_copy_name").cloned().unwrap_or_else(|| Value::String(String::new())));
    let cx = Context { zone, values, info, now, path };
    test(node, &cx)
}

/// `node['children']` as the oracle iterates it: a list gives its items; a string or a dict gives strings (not
/// nodes), so only an empty one is harmless; anything else is not iterable.
fn iterate_children(node: &Value) -> Result<Vec<Value>> {
    match node.get("children") {
        None => Err(Error::key("children")),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::String(s)) if s.is_empty() => Ok(Vec::new()),
        Some(Value::Object(o)) if o.is_empty() => Ok(Vec::new()),
        Some(Value::String(_)) | Some(Value::Object(_)) => Err(Error::type_error("string indices must be integers")),
        Some(other) => {
            Err(Error::type_error(format!("'{}' object is not iterable", crate::pyutil::json_type_name(other))))
        }
    }
}

/// Port of `smart_rules.sql_condition`: native SQL for the rules the catalog can answer (star, flag, colour
/// label), with its parameters; `None` when any part cannot be expressed (a compound rule is never weakened).
pub fn sql_condition(node: &Value) -> Result<Option<(String, Vec<Value>)>> {
    require_object(node)?;
    if let Some(combine) = node.get("combine") {
        let children = iterate_children(node)?;
        let mut parts = Vec::new();
        for c in &children {
            parts.push(sql_condition(c)?);
        }
        if parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let parts: Vec<(String, Vec<Value>)> = parts.into_iter().flatten().collect();
        let joiner = if combine.as_str() == Some("intersect") { " AND " } else { " OR " };
        let mut expression = format!("({})", parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(joiner));
        if combine.as_str() == Some("exclude") {
            expression = format!("NOT {expression}");
        }
        return Ok(Some((expression, parts.into_iter().flat_map(|p| p.1).collect())));
    }
    let field = node.get("criteria").and_then(Value::as_str).ok_or_else(|| Error::key("criteria"))?;
    let op = node.get("operation").and_then(Value::as_str).ok_or_else(|| Error::key("operation"))?;
    let value = node.get("value").cloned().unwrap_or(Value::Null);
    if field == "rating" || field == "pick" {
        let column = if field == "rating" { "p.rating" } else { "p.flag" };
        if op == "in" {
            let second = node.get("value2").cloned().ok_or_else(|| Error::key("value2"))?;
            return Ok(Some((format!("{column} BETWEEN ? AND ?"), vec![value, second])));
        }
        if COMPARISONS.contains(&op) {
            return Ok(Some((format!("{column} {op} ?"), vec![value])));
        }
    }
    if field == "labelColor" && (op == "==" || op == "!=") {
        if value.as_str() == Some("custom") {
            let word = if op == "==" { "NOT IN" } else { "IN" };
            let labels =
                ["", "빨강", "노랑", "초록", "파랑", "보라"].iter().map(|s| Value::String((*s).into())).collect();
            return Ok(Some((format!("p.label {word} (?,?,?,?,?,?)"), labels)));
        }
        if matches!(value, Value::Array(_) | Value::Object(_)) {
            return Err(Error::type_error(format!("unhashable type: '{}'", crate::pyutil::json_type_name(&value))));
        }
        let label = label_of_json(&value).ok_or_else(|| Error::key_repr(json_repr(&value)))?;
        return Ok(Some((format!("p.label {op} ?"), vec![label.map_or(Value::Null, |s| Value::String(s.into()))])));
    }
    Ok(None)
}

/// Port of `smart_rules.time_dependent`: whether a rule tree contains a relative date rule (its result changes
/// with the clock). Accepts the stored `{KEY: rules}` form, `children` and the older `any`/`all` lists.
pub fn time_dependent(rules: &Value) -> Result<bool> {
    let Value::Object(map) = rules else {
        return Ok(false);
    };
    if let Some(inner) = map.get(KEY) {
        return time_dependent(inner);
    }
    if let Some(criteria) = map.get("criteria") {
        if matches!(criteria, Value::Array(_) | Value::Object(_)) {
            return Err(Error::type_error(format!("unhashable type: '{}'", crate::pyutil::json_type_name(criteria))));
        }
        if let Some(field) = criteria.as_str() {
            if date_field(field).is_some() {
                match map.get("operation") {
                    Some(other @ (Value::Array(_) | Value::Object(_))) => {
                        return Err(Error::type_error(format!(
                            "unhashable type: '{}'",
                            crate::pyutil::json_type_name(other)
                        )))
                    }
                    Some(Value::String(op)) if RELATIVE_OPS.contains(&op.as_str()) => return Ok(true),
                    _ => {}
                }
            }
        }
    }
    for key in ["children", "any", "all"] {
        match map.get(key) {
            None => {}
            Some(Value::Array(list)) => {
                for child in list {
                    if time_dependent(child)? {
                        return Ok(true);
                    }
                }
            }
            // Iterating a dict gives its keys, a string its characters: never dicts themselves.
            Some(Value::Object(_)) | Some(Value::String(_)) => {}
            Some(other) => {
                return Err(Error::type_error(format!(
                    "'{}' object is not iterable",
                    crate::pyutil::json_type_name(other)
                )))
            }
        }
    }
    Ok(false)
}

/// Port of `smart_rules.describe(node, indent=0)`: a short indented description of a rule tree.
pub fn describe(node: &Value, indent: usize) -> Result<String> {
    require_object(node)?;
    let pad = " ".repeat(indent);
    if let Some(combine) = node.get("combine") {
        let title = match combine.as_str() {
            Some("union") => "아래 조건 중 하나",
            Some("intersect") => "아래 조건 모두",
            Some("exclude") => "아래 조건 모두 제외",
            _ => return Err(Error::key_repr(json_repr(combine))),
        };
        let children = iterate_children(node)?;
        let lines: Result<Vec<String>> = children.iter().map(|c| describe(c, indent + 2)).collect();
        return Ok(format!("{pad}{title}\n{}", lines?.join("\n")));
    }
    let criteria = node.get("criteria").ok_or_else(|| Error::key("criteria"))?;
    let operation = node.get("operation").ok_or_else(|| Error::key("operation"))?;
    let name = match criteria.as_str() {
        Some("rating") => "별점".to_owned(),
        Some("pick") => "선택 표시".to_owned(),
        Some("labelColor") => "색 라벨".to_owned(),
        Some("keywords") => "키워드".to_owned(),
        Some("captureTime") => "촬영일".to_owned(),
        Some("touchTime") => "보정일".to_owned(),
        Some("fileFormat") => "파일 형식".to_owned(),
        _ => json_str(criteria),
    };
    let value = node.get("value").map(json_str).unwrap_or_default();
    let units = node.get("value_units").map(json_str).unwrap_or_default();
    let line = format!("{pad}{name} · {} · {value} {units}", json_str(operation));
    Ok(line.trim_end_matches(is_space).to_owned())
}
