//! Small helpers that reproduce the Python behaviour the oracle relies on: `float(str)`, `repr(float)`,
//! `str.isspace`, `str.casefold`, ISO date parsing, and Windows path splitting (the oracle runs on Windows).

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::Value;

/// `str.isspace` for one character.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` (Unicode whitespace).
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.casefold` (lower-casing plus the full case foldings that differ from it).
pub fn casefold(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'ß' | 'ẞ' => out.push_str("ss"),
            'ſ' => out.push('s'),
            'ς' => out.push('σ'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// Python's `float(text)`; `None` where Python raises `ValueError`. Accepts surrounding whitespace, single
/// underscores between digits, `inf`, `infinity` and `nan`.
pub fn parse_float(text: &str) -> Option<f64> {
    let t = strip(text);
    if t.is_empty() {
        return None;
    }
    let cleaned: String = if t.contains('_') {
        let chars: Vec<char> = t.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c == '_' {
                let before = i > 0 && chars[i - 1].is_ascii_digit();
                let after = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
                if !(before && after) {
                    return None;
                }
            }
        }
        t.chars().filter(|&c| c != '_').collect()
    } else {
        t.to_owned()
    };
    let body = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" || lower == "nan" {
        return cleaned.parse::<f64>().ok();
    }
    if !body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

/// Python's `repr(float)`.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exp) {
        if exp >= 0 {
            let split = exp as usize + 1;
            if digits.len() <= split {
                format!("{sign}{digits}{}.0", "0".repeat(split - digits.len()))
            } else {
                format!("{sign}{}.{}", &digits[..split], &digits[split..])
            }
        } else {
            format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize))
        }
    } else {
        let rest = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        format!("{sign}{}{rest}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

fn repr_string(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `repr()` of a value that came from JSON (`json.loads`).
pub fn json_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else {
                float_repr(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => repr_string(s),
        Value::Array(a) => format!("[{}]", a.iter().map(json_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => {
            format!(
                "{{{}}}",
                m.iter().map(|(k, v)| format!("{}: {}", repr_string(k), json_repr(v))).collect::<Vec<_>>().join(", ")
            )
        }
    }
}

/// Python's `str()` of a value that came from JSON.
pub fn json_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => json_repr(other),
    }
}

/// Python's `type(x).__name__` of a value that came from JSON.
pub fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    grainy_core::settings::truthy(v)
}

fn digits(s: &str, n: usize) -> Option<u32> {
    if s.len() == n && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

/// `datetime.date.fromisoformat` (Python 3.11+): `YYYY-MM-DD`, `YYYYMMDD`, and ISO week dates.
pub fn date_from_iso(text: &str) -> Option<NaiveDate> {
    if !text.is_ascii() {
        return None;
    }
    let b = text.as_bytes();
    let year = |s: &str| digits(s, 4).map(|y| y as i32);
    match (text.len(), b.get(4).copied(), b.get(5).copied()) {
        (10, Some(b'-'), Some(c)) if c != b'W' && b[7] == b'-' => {
            NaiveDate::from_ymd_opt(year(&text[..4])?, digits(&text[5..7], 2)?, digits(&text[8..10], 2)?)
        }
        (8, _, _) if b.iter().all(u8::is_ascii_digit) => {
            NaiveDate::from_ymd_opt(year(&text[..4])?, digits(&text[4..6], 2)?, digits(&text[6..8], 2)?)
        }
        (8, Some(b'-'), Some(b'W')) => iso_week(year(&text[..4])?, digits(&text[6..8], 2)?, 1),
        (10, Some(b'-'), Some(b'W')) if b[8] == b'-' => {
            iso_week(year(&text[..4])?, digits(&text[6..8], 2)?, digits(&text[9..10], 1)?)
        }
        (7, Some(b'W'), _) => iso_week(year(&text[..4])?, digits(&text[5..7], 2)?, 1),
        (8, Some(b'W'), _) => iso_week(year(&text[..4])?, digits(&text[5..7], 2)?, digits(&text[7..8], 1)?),
        _ => None,
    }
}

fn iso_week(year: i32, week: u32, day: u32) -> Option<NaiveDate> {
    if !(1..=7).contains(&day) || !(1..=53).contains(&week) {
        return None;
    }
    let date = NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)?;
    date.checked_add_signed(chrono::Duration::days(i64::from(day) - 1))
}

/// A parsed `datetime.fromisoformat` value: naive, or with a UTC offset in seconds.
pub struct IsoDateTime {
    /// The wall-clock time as written.
    pub naive: NaiveDateTime,
    /// The offset when the text carried one.
    pub offset: Option<i32>,
}

/// `datetime.datetime.fromisoformat` (Python 3.11+, the common subset: date forms of [`date_from_iso`],
/// any one-character separator, `HH[:MM[:SS[.f]]]` or the basic forms, `Z` or a numeric offset).
pub fn datetime_from_iso(text: &str) -> Option<IsoDateTime> {
    if !text.is_ascii() || text.len() < 7 {
        return None;
    }
    let b = text.as_bytes();
    let date_len = if text.len() >= 8 && b[..8].iter().all(u8::is_ascii_digit) {
        8
    } else if text.len() >= 8 && b[4] == b'-' && b[5] == b'W' {
        if text.len() >= 10 && b[8] == b'-' && b[9].is_ascii_digit() {
            10
        } else {
            8
        }
    } else if b[4] == b'W' {
        if b[7].is_ascii_digit() {
            8
        } else {
            7
        }
    } else if text.len() >= 10 && b[4] == b'-' {
        10
    } else {
        return None;
    };
    let date = date_from_iso(&text[..date_len])?;
    let rest = &text[date_len..];
    if rest.is_empty() {
        return Some(IsoDateTime { naive: date.and_time(NaiveTime::MIN), offset: None });
    }
    let time_text = &rest[1..];
    if time_text.is_empty() {
        return None;
    }
    let (clock, offset) = match time_text.find(['Z', '+', '-']) {
        Some(i) if i >= 2 => {
            let (c, z) = time_text.split_at(i);
            (c.trim_end_matches(' '), Some(parse_offset(z)?))
        }
        Some(_) => return None,
        None => (time_text, None),
    };
    Some(IsoDateTime { naive: date.and_time(parse_clock(clock)?), offset })
}

fn parse_clock(s: &str) -> Option<NaiveTime> {
    let (main, fraction) = match s.find(['.', ',']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (h, m, sec) = if main.contains(':') {
        let parts: Vec<&str> = main.split(':').collect();
        match parts.as_slice() {
            [h, m] => (digits(h, 2)?, digits(m, 2)?, 0),
            [h, m, s] => (digits(h, 2)?, digits(m, 2)?, digits(s, 2)?),
            _ => return None,
        }
    } else {
        match main.len() {
            2 => (digits(main, 2)?, 0, 0),
            4 => (digits(&main[..2], 2)?, digits(&main[2..], 2)?, 0),
            6 => (digits(&main[..2], 2)?, digits(&main[2..4], 2)?, digits(&main[4..], 2)?),
            _ => return None,
        }
    };
    let micro = match fraction {
        None => 0,
        Some(f) => {
            if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let mut padded: String = f.chars().take(6).collect();
            while padded.len() < 6 {
                padded.push('0');
            }
            padded.parse::<u32>().ok()?
        }
    };
    NaiveTime::from_hms_micro_opt(h, m, sec, micro)
}

fn parse_offset(z: &str) -> Option<i32> {
    if z == "Z" {
        return Some(0);
    }
    let sign = match z.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = &z[1..];
    let (h, m, s) = if body.contains(':') {
        let parts: Vec<&str> = body.split(':').collect();
        match parts.as_slice() {
            [h, m] => (digits(h, 2)?, digits(m, 2)?, 0),
            [h, m, s] => (digits(h, 2)?, digits(m, 2)?, digits(s.split('.').next()?, 2)?),
            _ => return None,
        }
    } else {
        match body.len() {
            2 => (digits(body, 2)?, 0, 0),
            4 => (digits(&body[..2], 2)?, digits(&body[2..], 2)?, 0),
            6 => (digits(&body[..2], 2)?, digits(&body[2..4], 2)?, digits(&body[4..], 2)?),
            _ => return None,
        }
    };
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    Some(sign * (h * 3600 + m * 60 + s) as i32)
}

/// The zone a time with an offset is converted to (`datetime.astimezone()` uses the computer's zone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The computer's local zone (the oracle's behaviour).
    Local,
    /// A fixed offset east of UTC, in seconds (UTC is `Fixed(0)`); makes results machine independent.
    Fixed(i32),
}

/// `parsed.astimezone().replace(tzinfo=None)`: the wall-clock time of a time that carries an offset, in `zone`.
pub fn to_zone_naive(naive: NaiveDateTime, offset_seconds: i32, zone: Zone) -> Option<NaiveDateTime> {
    let offset = FixedOffset::east_opt(offset_seconds)?;
    let aware: DateTime<FixedOffset> = naive.and_local_timezone(offset).single()?;
    Some(match zone {
        Zone::Local => aware.with_timezone(&Local).naive_local(),
        Zone::Fixed(seconds) => aware.with_timezone(&FixedOffset::east_opt(seconds)?).naive_local(),
    })
}

/// The current local wall-clock time (`datetime.now()`).
pub fn now_local() -> NaiveDateTime {
    Local::now().naive_local()
}

/// The pieces of `pathlib.PureWindowsPath` the oracle uses (the oracle runs on Windows).
pub struct WinPath {
    anchor: String,
    parts: Vec<String>,
}

impl WinPath {
    /// Splits a path as `Path(text)` does on Windows.
    pub fn new(text: &str) -> Self {
        let norm: Vec<char> = text.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
        let mut i = 0;
        let mut anchor = String::new();
        if norm.len() >= 2 && norm[1] == ':' && norm[0].is_alphabetic() {
            anchor.push(norm[0]);
            anchor.push(':');
            i = 2;
        }
        if i < norm.len() && norm[i] == '\\' {
            anchor.push('\\');
            while i < norm.len() && norm[i] == '\\' {
                i += 1;
            }
        }
        let rest: String = norm[i..].iter().collect();
        let parts = rest.split('\\').filter(|p| !p.is_empty() && *p != ".").map(str::to_owned).collect();
        Self { anchor, parts }
    }

    fn join(&self, parts: &[String]) -> String {
        let body = parts.join("\\");
        if self.anchor.is_empty() && body.is_empty() {
            ".".into()
        } else {
            format!("{}{}", self.anchor, body)
        }
    }

    /// `path.name`.
    pub fn name(&self) -> String {
        self.parts.last().cloned().unwrap_or_default()
    }

    /// `str(path.parent)`.
    pub fn parent(&self) -> String {
        if self.parts.is_empty() {
            self.join(&[])
        } else {
            self.join(&self.parts[..self.parts.len() - 1])
        }
    }

    /// `path.suffix`.
    pub fn suffix(&self) -> String {
        let name = self.name();
        match name.rfind('.') {
            Some(i) if i > 0 && i + 1 < name.len() => name[i..].to_owned(),
            _ => String::new(),
        }
    }
}
