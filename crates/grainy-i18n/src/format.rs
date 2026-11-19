//! The subset of Python's `str.format` that `tr(source, *values)` relies on, with positional string values.

use std::fmt;

/// Why a template could not be formatted. Each kind is the Python exception the oracle raises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// `IndexError`: the template wants a value that was not given.
    MissingValue(usize),
    /// `KeyError`: a named field such as `{stem}`; `tr` passes positional values only.
    NamedField(String),
    /// `ValueError`: a single brace, an unterminated field, a mix of `{}` and `{0}`, or a bad format spec.
    Syntax(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::MissingValue(i) => write!(f, "no value for placeholder {i}"),
            FormatError::NamedField(n) => write!(f, "named placeholder {{{n}}} cannot take positional values"),
            FormatError::Syntax(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for FormatError {}

impl FormatError {
    /// The Python exception class the oracle raises for this error.
    pub fn python_class(&self) -> &'static str {
        match self {
            FormatError::MissingValue(_) => "IndexError",
            FormatError::NamedField(_) => "KeyError",
            FormatError::Syntax(_) => "ValueError",
        }
    }
}

/// One field of a template as `string.Formatter().parse` reports it: name (maybe empty), conversion, spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub name: String,
    pub conversion: Option<char>,
    pub spec: String,
}

/// A parsed template piece.
pub(crate) enum Piece {
    Literal(String),
    Field(Field),
}

/// Split a template into literals and fields (`{{`/`}}` become literal braces).
/// Port of the parsing done by `str.format` / `string.Formatter.parse`.
pub(crate) fn parse(template: &str) -> Result<Vec<Piece>, FormatError> {
    let chars: Vec<char> = template.chars().collect();
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            if chars.get(i + 1) == Some(&'{') {
                literal.push('{');
                i += 2;
                continue;
            }
            // Find the matching close brace, allowing nested braces in the spec.
            let mut depth = 1;
            let mut j = i + 1;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth > 0 {
                return Err(FormatError::Syntax("expected '}' before end of string".into()));
            }
            let inner: String = chars[i + 1..j - 1].iter().collect();
            if !literal.is_empty() {
                pieces.push(Piece::Literal(std::mem::take(&mut literal)));
            }
            pieces.push(Piece::Field(parse_field(&inner)?));
            i = j;
        } else if c == '}' {
            if chars.get(i + 1) == Some(&'}') {
                literal.push('}');
                i += 2;
            } else {
                return Err(FormatError::Syntax("Single '}' encountered in format string".into()));
            }
        } else {
            literal.push(c);
            i += 1;
        }
    }
    if !literal.is_empty() {
        pieces.push(Piece::Literal(literal));
    }
    Ok(pieces)
}

fn parse_field(inner: &str) -> Result<Field, FormatError> {
    // The name ends at the first '!' or ':'.
    let end = inner.find(['!', ':']).unwrap_or(inner.len());
    let name = &inner[..end];
    let rest = &inner[end..];
    let (conversion, spec) = if let Some(after) = rest.strip_prefix('!') {
        let mut it = after.chars();
        let conv = it
            .next()
            .ok_or_else(|| FormatError::Syntax("end of string while looking for conversion specifier".into()))?;
        let tail = it.as_str();
        if let Some(spec) = tail.strip_prefix(':') {
            (Some(conv), spec.to_string())
        } else if tail.is_empty() {
            (Some(conv), String::new())
        } else {
            return Err(FormatError::Syntax("expected ':' after conversion specifier".into()));
        }
    } else if let Some(spec) = rest.strip_prefix(':') {
        (None, spec.to_string())
    } else {
        (None, String::new())
    };
    Ok(Field { name: name.to_string(), conversion, spec })
}

/// The sorted field names of a template, as `sorted(f for _, f, _, _ in Formatter().parse(s) if f is not None)`.
pub(crate) fn field_names(template: &str) -> Result<Vec<String>, FormatError> {
    let mut names: Vec<String> = parse(template)?
        .into_iter()
        .filter_map(|p| match p {
            Piece::Field(f) => Some(f.name),
            Piece::Literal(_) => None,
        })
        .collect();
    names.sort();
    Ok(names)
}

/// Port of `str.format(*values)` for string values: `{}`, `{0}`, `{{`, `}}`, `!s`, and `[[fill]align][width][s]`
/// specs. The first error wins, left to right, as in Python.
pub fn format(template: &str, values: &[&str]) -> Result<String, FormatError> {
    let mut out = String::new();
    let mut auto = 0usize;
    // None: undecided; Some(true): automatic numbering; Some(false): manual.
    let mut numbering: Option<bool> = None;
    for piece in parse(template)? {
        match piece {
            Piece::Literal(s) => out.push_str(&s),
            Piece::Field(field) => {
                let index = if field.name.is_empty() {
                    if numbering == Some(false) {
                        return Err(FormatError::Syntax(
                            "cannot switch from manual field specification to automatic field numbering".into(),
                        ));
                    }
                    numbering = Some(true);
                    auto += 1;
                    auto - 1
                } else if field.name.chars().all(|c| c.is_ascii_digit()) {
                    if numbering == Some(true) {
                        return Err(FormatError::Syntax(
                            "cannot switch from automatic field numbering to manual field specification".into(),
                        ));
                    }
                    numbering = Some(false);
                    field.name.parse::<usize>().map_err(|_| FormatError::Syntax("too many decimal digits".into()))?
                } else {
                    return Err(FormatError::NamedField(field.name));
                };
                let value = values.get(index).ok_or(FormatError::MissingValue(index))?;
                let converted;
                let value: &str = match field.conversion {
                    None | Some('s') => value,
                    Some('r') => {
                        converted = python_repr(value);
                        &converted
                    }
                    Some(c) => return Err(FormatError::Syntax(format!("unsupported conversion '!{c}'"))),
                };
                out.push_str(&apply_spec(value, &field.spec)?);
            }
        }
    }
    Ok(out)
}

fn apply_spec(value: &str, spec: &str) -> Result<String, FormatError> {
    if spec.is_empty() {
        return Ok(value.to_string());
    }
    let mut chars: Vec<char> = spec.chars().collect();
    if chars.last() == Some(&'s') {
        chars.pop();
    }
    let is_align = |c: char| matches!(c, '<' | '>' | '^');
    let (fill, align, rest) = if chars.len() >= 2 && is_align(chars[1]) {
        (chars[0], chars[1], &chars[2..])
    } else if !chars.is_empty() && is_align(chars[0]) {
        (' ', chars[0], &chars[1..])
    } else {
        (' ', '<', &chars[..])
    };
    let width_text: String = rest.iter().collect();
    let width = if width_text.is_empty() {
        0
    } else if width_text.chars().all(|c| c.is_ascii_digit()) && !width_text.starts_with('0') {
        width_text
            .parse::<usize>()
            .map_err(|_| FormatError::Syntax("too many decimal digits in format string".into()))?
    } else {
        return Err(FormatError::Syntax(format!("unsupported format spec '{spec}' for a string")));
    };
    let length = value.chars().count();
    if width <= length {
        return Ok(value.to_string());
    }
    let pad = width - length;
    let (left, right) = match align {
        '>' => (pad, 0),
        '^' => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    let mut out: String = std::iter::repeat_n(fill, left).collect();
    out.push_str(value);
    out.extend(std::iter::repeat_n(fill, right));
    Ok(out)
}

/// Python's `repr()` of a string (single quotes unless the text has only single quotes inside).
fn python_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in value.chars() {
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
