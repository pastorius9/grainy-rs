//! The bits of Python the oracle's text handling leans on: `str.format` for file-name templates
//! (`string.Formatter().parse` plus `str.format` with string and integer fields), `datetime.strptime` for the EXIF
//! time format, and `str()`, `float()`, `int()` of JSON values.

use crate::{Error, Result};
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::Value;
use std::collections::BTreeSet;

/// One piece of a parsed format string.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// Literal text (braces already unescaped).
    Literal(String),
    /// A replacement field.
    Field {
        /// Text before the format spec and conversion.
        name: String,
        /// Format spec (may contain nested fields).
        spec: String,
        /// `!r`, `!s`, `!a`.
        conversion: Option<char>,
    },
}

/// Port of CPython's `MarkupIterator` (what `string.Formatter().parse` uses), including its error messages.
pub fn parse(template: &str) -> Result<Vec<Piece>> {
    let chars: Vec<char> = template.chars().collect();
    let end = chars.len();
    let mut pos = 0;
    let mut pieces = Vec::new();
    while pos < end {
        let start = pos;
        let mut c = '\0';
        let mut markup = false;
        while pos < end {
            c = chars[pos];
            pos += 1;
            if c == '{' || c == '}' {
                markup = true;
                break;
            }
        }
        let at_end = pos >= end;
        let mut len = pos - start;
        if c == '}' && markup && (at_end || chars[pos] != '}') {
            return Err(Error::value("Single '}' encountered in format string"));
        }
        if at_end && c == '{' && markup {
            return Err(Error::value("Single '{' encountered in format string"));
        }
        let mut field = markup;
        if markup && !at_end {
            if chars[pos] == c {
                pos += 1;
                field = false;
            } else {
                len -= 1;
            }
        }
        let literal: String = chars[start..start + len].iter().collect();
        if !literal.is_empty() {
            pieces.push(Piece::Literal(literal));
        }
        if !field {
            continue;
        }
        // parse_field
        let name_start = pos;
        let mut c = '\0';
        while pos < end {
            c = chars[pos];
            pos += 1;
            match c {
                '{' => return Err(Error::value("unexpected '{' in field name")),
                '[' => {
                    while pos < end && chars[pos] != ']' {
                        pos += 1;
                    }
                    continue;
                }
                '}' | ':' | '!' => break,
                _ => continue,
            }
        }
        let name: String = chars[name_start..pos.saturating_sub(1).max(name_start)].iter().collect();
        let mut conversion = None;
        let mut spec = String::new();
        if c == '!' || c == ':' {
            let mut closed = false;
            if c == '!' {
                if pos >= end {
                    return Err(Error::value("end of string while looking for conversion specifier"));
                }
                conversion = Some(chars[pos]);
                pos += 1;
                if pos < end {
                    let next = chars[pos];
                    pos += 1;
                    if next == '}' {
                        closed = true;
                    } else if next != ':' {
                        return Err(Error::value("expected ':' after conversion specifier"));
                    }
                }
            }
            if !closed {
                let spec_start = pos;
                let mut depth = 1;
                let mut done = false;
                while pos < end {
                    let d = chars[pos];
                    pos += 1;
                    if d == '{' {
                        depth += 1;
                    } else if d == '}' {
                        depth -= 1;
                        if depth == 0 {
                            spec = chars[spec_start..pos - 1].iter().collect();
                            done = true;
                            break;
                        }
                    }
                }
                if !done {
                    return Err(Error::value("unmatched '{' in format spec"));
                }
            }
        } else if c != '}' {
            return Err(Error::value("expected '}' before end of string"));
        }
        pieces.push(Piece::Field { name, spec, conversion });
    }
    Ok(pieces)
}

/// The base names of a template's fields as `import_copy.check_template` collects them
/// (`f.split('.')[0].split('[')[0]`), with the parse errors of `string.Formatter().parse`.
pub fn field_names(template: &str) -> Result<BTreeSet<String>> {
    Ok(parse(template)?
        .into_iter()
        .filter_map(|p| match p {
            Piece::Field { name, .. } => {
                Some(name.split('.').next().unwrap_or("").split('[').next().unwrap_or("").to_owned())
            }
            Piece::Literal(_) => None,
        })
        .collect())
}

/// A keyword argument of `str.format`.
#[derive(Debug, Clone)]
pub enum Arg {
    /// A `str`.
    Str(String),
    /// An `int`.
    Int(i64),
}

/// `template.format(**args)` for string and integer arguments.
pub fn format(template: &str, args: &[(&str, Arg)]) -> Result<String> {
    format_depth(template, args, 2)
}

fn format_depth(template: &str, args: &[(&str, Arg)], depth: u32) -> Result<String> {
    if depth == 0 {
        return Err(Error::value("Max string recursion exceeded"));
    }
    let mut out = String::new();
    for piece in parse(template)? {
        match piece {
            Piece::Literal(text) => out.push_str(&text),
            Piece::Field { name, spec, conversion } => {
                let (base, rest) = match name.find(['.', '[']) {
                    Some(i) => (&name[..i], &name[i..]),
                    None => (name.as_str(), ""),
                };
                if base.is_empty() || base.chars().all(|c| c.is_ascii_digit()) {
                    return Err(Error::Type(format!(
                        "IndexError: Replacement index {} out of range for positional args tuple",
                        if base.is_empty() { "0" } else { base }
                    )));
                }
                let mut value = args
                    .iter()
                    .find(|(k, _)| *k == base)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| Error::Type(format!("KeyError: '{base}'")))?;
                let mut remaining = rest;
                while !remaining.is_empty() {
                    if let Some(after) = remaining.strip_prefix('.') {
                        let end = after.find(['.', '[']).unwrap_or(after.len());
                        // An attribute of a str/int is a bound method or a number the oracle's
                        // file-name check rejects ("<built-in method ...>"); keep the effect.
                        value = Arg::Str("<attribute>".into());
                        remaining = &after[end..];
                    } else if let Some(after) = remaining.strip_prefix('[') {
                        let close = after.find(']').ok_or_else(|| Error::value("Missing ']' in format string"))?;
                        let key = &after[..close];
                        match &value {
                            Arg::Int(_) => {
                                return Err(Error::Type("TypeError: 'int' object is not subscriptable".into()))
                            }
                            Arg::Str(text) => {
                                if key.is_empty() || !key.chars().all(|c| c.is_ascii_digit()) {
                                    return Err(Error::Type(
                                        "TypeError: string indices must be integers, not 'str'".into(),
                                    ));
                                }
                                let index: usize = key.parse().unwrap_or(usize::MAX);
                                let c = text
                                    .chars()
                                    .nth(index)
                                    .ok_or_else(|| Error::Type("IndexError: string index out of range".into()))?;
                                value = Arg::Str(c.to_string());
                            }
                        }
                        remaining = &after[close + 1..];
                        if !remaining.is_empty() && !remaining.starts_with(['.', '[']) {
                            return Err(Error::value("Only '.' or '[' may follow ']' in format field specifier"));
                        }
                    } else {
                        return Err(Error::value("Only '.' or '[' may follow ']' in format field specifier"));
                    }
                }
                let value = match conversion {
                    None => value,
                    Some('s') => Arg::Str(display(&value)),
                    Some('r') => Arg::Str(match &value {
                        Arg::Str(t) => crate::pathx::py_repr(t),
                        Arg::Int(n) => n.to_string(),
                    }),
                    Some('a') => Arg::Str(match &value {
                        Arg::Str(t) => ascii_repr(t),
                        Arg::Int(n) => n.to_string(),
                    }),
                    Some(c) => return Err(Error::value(format!("Unknown conversion specifier {c}"))),
                };
                let spec = if spec.contains('{') { format_depth(&spec, args, depth - 1)? } else { spec };
                out.push_str(&apply_spec(&value, &spec)?);
            }
        }
    }
    Ok(out)
}

fn unknown_code(c: char, type_name: &str) -> String {
    let n = c as u32;
    if n > 32 && n < 128 {
        format!("Unknown format code '{c}' for object of type '{type_name}'")
    } else {
        format!("Unknown format code '\\x{n:x}' for object of type '{type_name}'")
    }
}

fn display(value: &Arg) -> String {
    match value {
        Arg::Str(t) => t.clone(),
        Arg::Int(n) => n.to_string(),
    }
}

fn ascii_repr(text: &str) -> String {
    let repr = crate::pathx::py_repr(text);
    let mut out = String::new();
    for c in repr.chars() {
        let n = c as u32;
        if n < 0x80 {
            out.push(c);
        } else if n < 0x100 {
            out.push_str(&format!("\\x{n:02x}"));
        } else if n < 0x10000 {
            out.push_str(&format!("\\u{n:04x}"));
        } else {
            out.push_str(&format!("\\U{n:08x}"));
        }
    }
    out
}

struct Spec {
    fill: Option<char>,
    align: Option<char>,
    sign: Option<char>,
    alternate: bool,
    zero: bool,
    width: Option<usize>,
    grouping: Option<char>,
    precision: Option<usize>,
    kind: Option<char>,
}

fn parse_spec(spec: &str, type_name: &str) -> Result<Spec> {
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let mut result = Spec {
        fill: None,
        align: None,
        sign: None,
        alternate: false,
        zero: false,
        width: None,
        grouping: None,
        precision: None,
        kind: None,
    };
    let is_align = |c: char| matches!(c, '<' | '>' | '=' | '^');
    if chars.len() >= 2 && is_align(chars[1]) {
        result.fill = Some(chars[0]);
        result.align = Some(chars[1]);
        i = 2;
    } else if !chars.is_empty() && is_align(chars[0]) {
        result.align = Some(chars[0]);
        i = 1;
    }
    if i < chars.len() && matches!(chars[i], '+' | '-' | ' ') {
        result.sign = Some(chars[i]);
        i += 1;
    }
    if i < chars.len() && chars[i] == '#' {
        result.alternate = true;
        i += 1;
    }
    if i < chars.len() && chars[i] == '0' {
        result.zero = true;
        i += 1;
    }
    let number = |i: &mut usize| -> Option<usize> {
        let start = *i;
        while *i < chars.len() && chars[*i].is_ascii_digit() {
            *i += 1;
        }
        if start == *i {
            None
        } else {
            chars[start..*i].iter().collect::<String>().parse().ok()
        }
    };
    result.width = number(&mut i);
    if i < chars.len() && matches!(chars[i], ',' | '_') {
        result.grouping = Some(chars[i]);
        i += 1;
    }
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        result.precision = Some(number(&mut i).ok_or_else(|| Error::value("Format specifier missing precision"))?);
    }
    if i < chars.len() {
        result.kind = Some(chars[i]);
        i += 1;
    }
    if i < chars.len() {
        return Err(Error::value(format!("Invalid format specifier '{spec}' for object of type '{type_name}'")));
    }
    Ok(result)
}

fn pad(text: &str, spec: &Spec, default_align: char) -> String {
    let width = spec.width.unwrap_or(0);
    let len = text.chars().count();
    if len >= width {
        return text.to_owned();
    }
    let fill = spec.fill.unwrap_or(' ');
    let gap = width - len;
    let (left, right) = match spec.align.unwrap_or(default_align) {
        '<' => (0, gap),
        '^' => (gap / 2, gap - gap / 2),
        _ => (gap, 0),
    };
    format!("{}{}{}", fill.to_string().repeat(left), text, fill.to_string().repeat(right))
}

fn group(digits: &str, sep: char, size: usize) -> String {
    let chars: Vec<char> = digits.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(size) {
            out.push(sep);
        }
        out.push(*c);
    }
    out
}

fn apply_spec(value: &Arg, spec: &str) -> Result<String> {
    let s = parse_spec(spec, if matches!(value, Arg::Str(_)) { "str" } else { "int" })?;
    match value {
        Arg::Str(text) => {
            match s.kind {
                None | Some('s') => {}
                Some(c) => return Err(Error::value(unknown_code(c, "str"))),
            }
            if s.sign.is_some() {
                return Err(Error::value("Sign not allowed in string format specifier"));
            }
            if s.alternate {
                return Err(Error::value("Alternate form (#) not allowed in string format specifier"));
            }
            if s.align == Some('=') {
                return Err(Error::value("'=' alignment not allowed in string format specifier"));
            }
            if let Some(g) = s.grouping {
                return Err(Error::value(format!("Cannot specify '{g}' with 's'.")));
            }
            let mut text = text.clone();
            if let Some(p) = s.precision {
                text = text.chars().take(p).collect();
            }
            let mut s = s;
            if s.zero && s.fill.is_none() {
                s.fill = Some('0');
                if s.align.is_none() {
                    s.align = Some('<');
                }
            }
            Ok(pad(&text, &s, '<'))
        }
        Arg::Int(n) => {
            let kind = s.kind.unwrap_or('d');
            if s.precision.is_some() {
                return Err(Error::value("Precision not allowed in integer format specifier"));
            }
            let magnitude = n.unsigned_abs();
            let negative = *n < 0;
            let (digits, prefix) = match kind {
                'd' | 'n' => (magnitude.to_string(), ""),
                'b' => (format!("{magnitude:b}"), "0b"),
                'o' => (format!("{magnitude:o}"), "0o"),
                'x' => (format!("{magnitude:x}"), "0x"),
                'X' => (format!("{magnitude:X}"), "0X"),
                'c' => {
                    if s.sign.is_some() {
                        return Err(Error::value("Sign not allowed with integer format specifier 'c'"));
                    }
                    let c = u32::try_from(*n)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| Error::value("%c arg not in range(0x110000)"))?;
                    return Ok(pad(&c.to_string(), &s, '<'));
                }
                'f' | 'F' => {
                    let text = format!("{:.*}", s.precision.unwrap_or(6), *n as f64);
                    return Ok(pad(&text, &s, '>'));
                }
                c => return Err(Error::value(unknown_code(c, "int"))),
            };
            let digits = match s.grouping {
                Some(g) if kind == 'd' => group(&digits, g, 3),
                Some('_') => group(&digits, '_', 4),
                Some(g) => return Err(Error::value(format!("Cannot specify '{g}' with '{kind}'."))),
                None => digits,
            };
            let sign = if negative {
                "-"
            } else {
                match s.sign {
                    Some('+') => "+",
                    Some(' ') => " ",
                    _ => "",
                }
            };
            let prefix = if s.alternate { prefix } else { "" };
            let body = format!("{sign}{prefix}{digits}");
            let width = s.width.unwrap_or(0);
            if s.zero && s.align.is_none() && s.fill.is_none() {
                let have = body.chars().count();
                let zeros = width.saturating_sub(have);
                return Ok(format!("{sign}{prefix}{}{digits}", "0".repeat(zeros)));
            }
            if s.align == Some('=') {
                let have = body.chars().count();
                let fill = s.fill.unwrap_or(' ');
                let gap = width.saturating_sub(have);
                return Ok(format!("{sign}{prefix}{}{digits}", fill.to_string().repeat(gap)));
            }
            Ok(pad(&body, &s, '>'))
        }
    }
}

/// `datetime.strptime(text, '%Y:%m:%d %H:%M:%S')`, `None` where Python raises `ValueError`.
pub fn strptime_exif(text: &str) -> Option<NaiveDateTime> {
    let chars: Vec<char> = text.chars().collect();
    // Alternatives of each regex group, in the order Python's `_strptime` lists them.
    type Alt = &'static [fn(char) -> bool];
    fn digit(c: char) -> bool {
        c.is_ascii_digit()
    }
    macro_rules! eq {
        ($ch:expr) => {
            |c: char| c == $ch
        };
    }
    let year: &[Alt] = &[&[digit, digit, digit, digit]];
    let month: &[Alt] = &[
        &[eq!('1'), |c| ('0'..='2').contains(&c)],
        &[eq!('0'), |c| ('1'..='9').contains(&c)],
        &[|c| ('1'..='9').contains(&c)],
    ];
    let day: &[Alt] = &[
        &[eq!('3'), |c| c == '0' || c == '1'],
        &[|c| c == '1' || c == '2', digit],
        &[eq!('0'), |c| ('1'..='9').contains(&c)],
        &[|c| ('1'..='9').contains(&c)],
        &[eq!(' '), |c| ('1'..='9').contains(&c)],
    ];
    let hour: &[Alt] = &[&[eq!('2'), |c| ('0'..='3').contains(&c)], &[|c| c == '0' || c == '1', digit], &[digit]];
    let minute: &[Alt] = &[&[|c| ('0'..='5').contains(&c), digit], &[digit]];
    let second: &[Alt] = &[&[eq!('6'), |c| c == '0' || c == '1'], &[|c| ('0'..='5').contains(&c), digit], &[digit]];
    enum Item {
        Group(&'static [Alt]),
        Colon,
        Space,
    }
    let items = [
        Item::Group(year),
        Item::Colon,
        Item::Group(month),
        Item::Colon,
        Item::Group(day),
        Item::Space,
        Item::Group(hour),
        Item::Colon,
        Item::Group(minute),
        Item::Colon,
        Item::Group(second),
    ];
    fn run(items: &[Item], chars: &[char], pos: usize, found: &mut Vec<String>) -> Option<usize> {
        let Some((first, rest)) = items.split_first() else { return Some(pos) };
        match first {
            Item::Colon => {
                if chars.get(pos) == Some(&':') {
                    run(rest, chars, pos + 1, found)
                } else {
                    None
                }
            }
            Item::Space => {
                let mut n = 0;
                while chars.get(pos + n).is_some_and(|c| c.is_whitespace()) {
                    n += 1;
                }
                (1..=n).rev().find_map(|k| run(rest, chars, pos + k, found))
            }
            Item::Group(alts) => {
                for alt in alts.iter() {
                    if alt.iter().enumerate().all(|(i, f)| chars.get(pos + i).is_some_and(|c| f(*c))) {
                        found.push(chars[pos..pos + alt.len()].iter().collect());
                        if let Some(end) = run(rest, chars, pos + alt.len(), found) {
                            return Some(end);
                        }
                        found.pop();
                    }
                }
                None
            }
        }
    }
    let mut found = Vec::new();
    let end = run(&items, &chars, 0, &mut found)?;
    if end != chars.len() {
        return None;
    }
    let number = |i: usize| found[i].trim().parse::<u32>().ok();
    if number(0)? < 1 {
        return None; // datetime.MINYEAR
    }
    let date = NaiveDate::from_ymd_opt(number(0)? as i32, number(1)?, number(2)?)?;
    date.and_hms_opt(number(3)?, number(4)?, number(5)?)
}

/// `str(value)` of a JSON value as Python prints it.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => grainy_catalog::json::dumps(other, false, false, false),
    }
}

/// `float(value)` of a JSON value; `TypeError`/`ValueError` as Python raises.
pub fn py_float(value: &Value) -> Result<f64> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| Error::value("number out of range")),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::String(s) => {
            let t = s.trim().replace('_', "");
            let lowered = t.to_lowercase();
            let body = lowered.trim_start_matches(['+', '-']);
            if body == "inf" || body == "infinity" {
                return Ok(if lowered.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
            }
            if body == "nan" {
                return Ok(f64::NAN);
            }
            let valid = !body.is_empty()
                && body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-'))
                && t.parse::<f64>().is_ok();
            if valid {
                t.parse::<f64>().map_err(|_| Error::value("could not convert string to float"))
            } else {
                Err(Error::value(format!("could not convert string to float: {}", crate::pathx::py_repr(s))))
            }
        }
        Value::Null => Err(Error::Type("float() argument must be a string or a real number, not 'NoneType'".into())),
        Value::Array(_) => Err(Error::Type("float() argument must be a string or a real number, not 'list'".into())),
        Value::Object(_) => Err(Error::Type("float() argument must be a string or a real number, not 'dict'".into())),
    }
}

/// `int(value)` of a JSON value; `TypeError`/`ValueError` as Python raises.
pub fn py_int(value: &Value) -> Result<i64> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                n.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.trunc() as i64)
                    .ok_or_else(|| Error::value("cannot convert float to integer"))
            }
        }
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::String(s) => parse_int_text(s).ok_or_else(|| {
            Error::value(format!("invalid literal for int() with base 10: {}", crate::pathx::py_repr(s)))
        }),
        Value::Null => Err(Error::Type(
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'".into(),
        )),
        _ => Err(Error::Type("int() argument must be a string, a bytes-like object or a real number".into())),
    }
}

/// `int(text)` for decimal text (sign, surrounding blanks and single underscores between digits allowed).
pub fn parse_int_text(text: &str) -> Option<i64> {
    let t = text.trim();
    let (negative, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if clean.is_empty() || !clean.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: i64 = clean.parse().ok()?;
    Some(if negative { -value } else { value })
}
