//! Port of `luma/lua_data.py`: the bounded parser for Lua data tables saved by Lightroom. It never executes
//! Lua: calls, expressions, references, metatables and multiple statements are not part of the grammar.
//!
//! Deviation from the oracle: Python integers are unbounded. Here integers are `i128`; a larger integer is kept
//! as [`Lua::Huge`] (its `f64` value, infinite when beyond `f64`), which behaves like a Python int for the
//! checks the oracle makes (type tests, `float()` overflow).

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::pyutil::is_space;

/// A table key: Lua tables here have integer, non-integer-number or string keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// An integer key (also the auto-numbered array part).
    Int(i128),
    /// An integer too large for `i128` (the `f64` bits).
    Huge(u64),
    /// A non-integer number key (the `f64` bits).
    Float(u64),
    /// A string or name key.
    Str(String),
}

impl Key {
    /// The key as a value.
    pub fn to_lua(&self) -> Lua {
        match self {
            Key::Int(i) => Lua::Int(*i),
            Key::Huge(b) => Lua::Huge(f64::from_bits(*b)),
            Key::Float(b) => Lua::Float(f64::from_bits(*b)),
            Key::Str(s) => Lua::Str(s.clone()),
        }
    }
}

/// An insertion-ordered table (a Python `dict`).
#[derive(Clone, Debug, Default)]
pub struct Table {
    items: Vec<(Key, Lua)>,
    index: HashMap<Key, usize>,
}

impl Table {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a new key; returns false (and changes nothing) if the key exists.
    pub fn insert(&mut self, key: Key, value: Lua) -> bool {
        if self.index.contains_key(&key) {
            return false;
        }
        self.index.insert(key.clone(), self.items.len());
        self.items.push((key, value));
        true
    }

    /// Sets a key, replacing an existing value in place (`dict[key] = value`).
    pub fn set(&mut self, key: Key, value: Lua) {
        match self.index.get(&key) {
            Some(&i) => self.items[i].1 = value,
            None => {
                self.insert(key, value);
            }
        }
    }

    /// The value of a key.
    pub fn get(&self, key: &Key) -> Option<&Lua> {
        self.index.get(key).map(|&i| &self.items[i].1)
    }

    /// The value of a string key.
    pub fn get_str(&self, key: &str) -> Option<&Lua> {
        self.get(&Key::Str(key.to_owned()))
    }

    /// Whether a string key exists.
    pub fn contains_str(&self, key: &str) -> bool {
        self.get_str(key).is_some()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &(Key, Lua)> {
        self.items.iter()
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &Key> {
        self.items.iter().map(|(k, _)| k)
    }
}

impl PartialEq for Table {
    fn eq(&self, other: &Self) -> bool {
        self.items.len() == other.items.len() && self.items.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

/// A value of a Lua data table, or of an XMP value (strings, lists of strings, `{'xml': ...}` tables).
#[derive(Clone, Debug, PartialEq)]
pub enum Lua {
    /// `nil` (Python `None`).
    Nil,
    /// `true` / `false`.
    Bool(bool),
    /// An integer.
    Int(i128),
    /// An integer beyond `i128` (see the module note).
    Huge(f64),
    /// A non-integer number.
    Float(f64),
    /// A string.
    Str(String),
    /// A table (Python `dict`).
    Table(Table),
    /// A list (only XMP `rdf:Seq` values produce one).
    List(Vec<Lua>),
}

impl Lua {
    /// A table with string keys from pairs.
    pub fn table_of(pairs: Vec<(&str, Lua)>) -> Lua {
        let mut t = Table::new();
        for (k, v) in pairs {
            t.set(Key::Str(k.to_owned()), v);
        }
        Lua::Table(t)
    }

    /// The Python type name (`type(x).__name__`), for messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Lua::Nil => "NoneType",
            Lua::Bool(_) => "bool",
            Lua::Int(_) | Lua::Huge(_) => "int",
            Lua::Float(_) => "float",
            Lua::Str(_) => "str",
            Lua::Table(_) => "dict",
            Lua::List(_) => "list",
        }
    }

    /// The table, if this is one.
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Lua::Table(t) => Some(t),
            _ => None,
        }
    }
}

/// The parser's limits (`Parser.__init__` keyword arguments).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Largest text, in characters and in UTF-8 bytes.
    pub max_bytes: usize,
    /// Largest number of values.
    pub max_items: usize,
    /// Deepest table nesting.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_bytes: 16 * 1024 * 1024, max_items: 200_000, max_depth: 64 }
    }
}

const BAD_SIZE: &str = "저장된 데이터의 크기 또는 문자 형식을 확인할 수 없습니다.";
const BAD_LIMIT: &str = "데이터의 중첩 또는 항목 수가 지원 범위를 넘었습니다.";
const BAD_DUPLICATE: &str = "같은 키가 중복된 데이터는 변환하지 않습니다.";

/// Port of `lua_data.Parser`.
pub struct Parser {
    text: Vec<char>,
    position: usize,
    items: usize,
    max_items: usize,
    max_depth: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn int_value(negative: bool, digits: &[char], radix: u32) -> Lua {
    let mut value: i128 = 0;
    let mut approx: f64 = 0.0;
    let mut overflow = false;
    for &c in digits {
        let d = c.to_digit(radix).unwrap_or(0);
        approx = approx * f64::from(radix) + f64::from(d);
        if !overflow {
            match value.checked_mul(i128::from(radix)).and_then(|v| v.checked_add(i128::from(d))) {
                Some(v) => value = v,
                None => overflow = true,
            }
        }
    }
    if overflow {
        Lua::Huge(if negative { -approx } else { approx })
    } else {
        Lua::Int(if negative { -value } else { value })
    }
}

/// `float.fromhex` of a token like `-0x1.8p3`; `None` on overflow.
fn from_hex(token: &[char]) -> Option<f64> {
    let mut i = 0;
    let mut negative = false;
    if i < token.len() && (token[i] == '+' || token[i] == '-') {
        negative = token[i] == '-';
        i += 1;
    }
    i += 2; // 0x
    let mut mantissa: u128 = 0;
    let mut exponent: i64 = 0;
    let mut seen_point = false;
    while i < token.len() && (token[i].is_ascii_hexdigit() || token[i] == '.') {
        if token[i] == '.' {
            seen_point = true;
        } else {
            let d = u128::from(token[i].to_digit(16).unwrap_or(0));
            if mantissa >> 120 == 0 {
                mantissa = mantissa * 16 + d;
                if seen_point {
                    exponent -= 4;
                }
            } else if !seen_point {
                exponent += 4;
            }
        }
        i += 1;
    }
    if i < token.len() && (token[i] == 'p' || token[i] == 'P') {
        let text: String = token[i + 1..].iter().collect();
        let e: i64 = text.parse().unwrap_or(0);
        exponent += e.clamp(-100_000, 100_000);
    }
    let mut value = mantissa as f64;
    let mut e = exponent;
    while e > 0 && value.is_finite() {
        let step = e.min(512);
        value *= 2f64.powi(step as i32);
        e -= step;
    }
    while e < 0 && value != 0.0 {
        let step = (-e).min(512);
        value *= 2f64.powi(-(step as i32));
        e += step;
    }
    if !value.is_finite() {
        return None;
    }
    Some(if negative { -value } else { value })
}

impl Parser {
    /// Port of `Parser.__init__`: checks the size and strips leading byte-order marks.
    pub fn new(text: &str, limits: Limits) -> Result<Self> {
        if text.chars().count() > limits.max_bytes || text.len() > limits.max_bytes {
            return Err(Error::data(BAD_SIZE));
        }
        let chars: Vec<char> = text.trim_start_matches('\u{feff}').chars().collect();
        Ok(Self { text: chars, position: 0, items: 0, max_items: limits.max_items, max_depth: limits.max_depth })
    }

    fn fail<T>(&self) -> Result<T> {
        Err(Error::data(format!("지원하지 않거나 손상된 데이터 구문입니다 (위치 {}).", self.position)))
    }

    fn at(&self, i: usize) -> Option<char> {
        self.text.get(i).copied()
    }

    fn starts_with(&self, token: &str, position: usize) -> bool {
        token.chars().enumerate().all(|(i, c)| self.at(position + i) == Some(c))
    }

    /// `LONG.match(text, position)`: the number of `=` signs of a long-bracket opener.
    fn long_open(&self, position: usize) -> Option<usize> {
        if self.at(position) != Some('[') {
            return None;
        }
        let mut p = position + 1;
        while self.at(p) == Some('=') {
            p += 1;
        }
        if self.at(p) == Some('[') {
            Some(p - position - 1)
        } else {
            None
        }
    }

    fn long_string(&mut self) -> Result<String> {
        let Some(equals) = self.long_open(self.position) else {
            return self.fail();
        };
        let start = self.position + equals + 2;
        let mut closer: Vec<char> = vec![']'];
        closer.extend(std::iter::repeat_n('=', equals));
        closer.push(']');
        let mut end = None;
        let mut p = start;
        while p + closer.len() <= self.text.len() {
            if self.text[p..p + closer.len()] == closer[..] {
                end = Some(p);
                break;
            }
            p += 1;
        }
        let Some(end) = end else {
            return self.fail();
        };
        self.position = end + closer.len();
        let raw: String = self.text[start..end].iter().collect();
        let value = raw.replace("\r\n", "\n").replace('\r', "\n");
        Ok(match value.strip_prefix('\n') {
            Some(rest) => rest.to_owned(),
            None => value,
        })
    }

    fn space(&mut self) -> Result<()> {
        while self.position < self.text.len() {
            if is_space(self.text[self.position]) {
                self.position += 1;
            } else if self.starts_with("--", self.position) {
                self.position += 2;
                if self.long_open(self.position).is_some() {
                    self.long_string()?;
                } else {
                    match self.text[self.position..].iter().position(|&c| c == '\n') {
                        Some(offset) => self.position += offset + 1,
                        None => self.position = self.text.len(),
                    }
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    fn take(&mut self, token: &str) -> Result<bool> {
        self.space()?;
        if self.starts_with(token, self.position) {
            self.position += token.chars().count();
            return Ok(true);
        }
        Ok(false)
    }

    fn quoted(&mut self) -> Result<String> {
        let quote = self.text[self.position];
        self.position += 1;
        let mut output: Vec<u8> = Vec::new();
        while self.position < self.text.len() {
            let c = self.text[self.position];
            self.position += 1;
            if c == quote {
                return match String::from_utf8(output) {
                    Ok(s) => Ok(s),
                    Err(_) => self.fail(),
                };
            }
            if c == '\r' || c == '\n' {
                return self.fail();
            }
            if c != '\\' {
                let mut buffer = [0u8; 4];
                output.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                continue;
            }
            if self.position == self.text.len() {
                return self.fail();
            }
            let c = self.text[self.position];
            self.position += 1;
            let simple = match c {
                'a' => Some(7u8),
                'b' => Some(8),
                'f' => Some(12),
                'n' => Some(10),
                'r' => Some(13),
                't' => Some(9),
                'v' => Some(11),
                '\\' => Some(92),
                '"' => Some(34),
                '\'' => Some(39),
                _ => None,
            };
            if let Some(byte) = simple {
                output.push(byte);
            } else if c.is_ascii_digit() {
                let mut digits = String::from(c);
                while digits.len() < 3 && self.at(self.position).is_some_and(|d| d.is_ascii_digit()) {
                    digits.push(self.text[self.position]);
                    self.position += 1;
                }
                let number: u32 = digits.parse().unwrap_or(999);
                if number > 255 {
                    return self.fail();
                }
                output.push(number as u8);
            } else if c == 'x' {
                let a = self.at(self.position).and_then(|d| d.to_digit(16));
                let b = self.at(self.position + 1).and_then(|d| d.to_digit(16));
                match (a, b) {
                    (Some(a), Some(b)) if self.at(self.position).is_some_and(|d| d.is_ascii_hexdigit()) => {
                        output.push((a * 16 + b) as u8);
                        self.position += 2;
                    }
                    _ => return self.fail(),
                }
            } else if c == 'z' {
                while self.position < self.text.len() && is_space(self.text[self.position]) {
                    self.position += 1;
                }
            } else if c == '\r' || c == '\n' {
                if c == '\r' && self.at(self.position) == Some('\n') {
                    self.position += 1;
                }
                output.push(10);
            } else {
                return self.fail();
            }
        }
        self.fail()
    }

    /// The span of a `NUMBER` match at `position`: `(end, is_hex_float_or_int_form)`.
    fn number_end(&self, position: usize) -> Option<usize> {
        let mut p = position;
        if matches!(self.at(p), Some('+') | Some('-')) {
            p += 1;
        }
        let digit_run = |from: usize, hex: bool| {
            let mut q = from;
            while self.at(q).is_some_and(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }) {
                q += 1;
            }
            q
        };
        // Hexadecimal form first (regex alternation order).
        if self.at(p) == Some('0') && matches!(self.at(p + 1), Some('x') | Some('X')) {
            let q = digit_run(p + 2, true);
            if q > p + 2 {
                let mut end = q;
                if self.at(end) == Some('.') {
                    end = digit_run(end + 1, true);
                }
                if matches!(self.at(end), Some('p') | Some('P')) {
                    let mut e = end + 1;
                    if matches!(self.at(e), Some('+') | Some('-')) {
                        e += 1;
                    }
                    let f = digit_run(e, false);
                    if f > e {
                        end = f;
                    }
                }
                return Some(end);
            }
        }
        let mut end;
        let int_end = digit_run(p, false);
        if int_end > p {
            end = int_end;
            if self.at(end) == Some('.') {
                end = digit_run(end + 1, false);
            }
        } else if self.at(p) == Some('.') {
            let f = digit_run(p + 1, false);
            if f == p + 1 {
                return None;
            }
            end = f;
        } else {
            return None;
        }
        if matches!(self.at(end), Some('e') | Some('E')) {
            let mut e = end + 1;
            if matches!(self.at(e), Some('+') | Some('-')) {
                e += 1;
            }
            let f = digit_run(e, false);
            if f > e {
                end = f;
            }
        }
        Some(end)
    }

    fn name_end(&self, position: usize) -> Option<usize> {
        if !self.at(position).is_some_and(is_name_start) {
            return None;
        }
        let mut p = position + 1;
        while self.at(p).is_some_and(is_name_char) {
            p += 1;
        }
        Some(p)
    }

    fn value(&mut self, depth: usize) -> Result<Lua> {
        self.space()?;
        self.items += 1;
        if depth > self.max_depth || self.items > self.max_items {
            return Err(Error::data(BAD_LIMIT));
        }
        if self.position == self.text.len() {
            return self.fail();
        }
        let c = self.text[self.position];
        if c == '{' {
            return self.table(depth + 1).map(Lua::Table);
        }
        if c == '"' || c == '\'' {
            return self.quoted().map(Lua::Str);
        }
        if self.long_open(self.position).is_some() {
            return self.long_string().map(Lua::Str);
        }
        if let Some(end) = self.number_end(self.position) {
            let token: Vec<char> = self.text[self.position..end].to_vec();
            self.position = end;
            let (negative, body) = match token.first() {
                Some('-') => (true, &token[1..]),
                Some('+') => (false, &token[1..]),
                _ => (false, &token[..]),
            };
            if body.iter().all(char::is_ascii_digit) {
                return Ok(int_value(negative, body, 10));
            }
            let is_hex = body.len() > 2 && body[0] == '0' && matches!(body[1], 'x' | 'X');
            if is_hex && body[2..].iter().all(char::is_ascii_hexdigit) {
                return Ok(int_value(negative, &body[2..], 16));
            }
            let value = if is_hex {
                match from_hex(&token) {
                    Some(v) => v,
                    None => return Err(Error::overflow("hexadecimal value too large to represent as a float")),
                }
            } else {
                let text: String = token.iter().collect();
                text.parse::<f64>().unwrap_or(f64::INFINITY)
            };
            if !value.is_finite() {
                return self.fail();
            }
            if value.fract() == 0.0 {
                return Ok(if value.abs() < 1.0e38 { Lua::Int(value as i128) } else { Lua::Huge(value) });
            }
            return Ok(Lua::Float(value));
        }
        if let Some(end) = self.name_end(self.position) {
            let word: String = self.text[self.position..end].iter().collect();
            let value = match word.as_str() {
                "true" => Some(Lua::Bool(true)),
                "false" => Some(Lua::Bool(false)),
                "nil" => Some(Lua::Nil),
                _ => None,
            };
            if let Some(value) = value {
                self.position = end;
                return Ok(value);
            }
        }
        self.fail()
    }

    fn table(&mut self, depth: usize) -> Result<Table> {
        self.position += 1;
        let mut result = Table::new();
        let mut auto_key: i128 = 1;
        if self.take("}")? {
            return Ok(result);
        }
        loop {
            self.space()?;
            let start = self.position;
            let name = self.name_end(start);
            if let Some(end) = name {
                self.position = end;
            }
            let key;
            if name.is_some() && self.take("=")? {
                key = Key::Str(self.text[start..name.unwrap_or(start)].iter().collect());
            } else if self.at(start) == Some('[') && self.long_open(start).is_none() {
                self.position = start + 1;
                let k = self.value(depth)?;
                key = match k {
                    Lua::Int(i) => Key::Int(i),
                    Lua::Huge(f) => Key::Huge(f.to_bits()),
                    Lua::Float(f) => Key::Float(f.to_bits()),
                    Lua::Str(s) => Key::Str(s),
                    _ => return self.fail(),
                };
                if !self.take("]")? || !self.take("=")? {
                    return self.fail();
                }
            } else {
                self.position = start;
                key = Key::Int(auto_key);
                auto_key += 1;
            }
            if result.get(&key).is_some() {
                return Err(Error::data(BAD_DUPLICATE));
            }
            let value = self.value(depth)?;
            result.insert(key, value);
            if self.take("}")? {
                return Ok(result);
            }
            if !(self.take(",")? || self.take(";")?) {
                return self.fail();
            }
            if self.take("}")? {
                return Ok(result);
            }
        }
    }

    /// Port of `Parser.parse`.
    pub fn parse(&mut self) -> Result<Table> {
        self.space()?;
        if let Some(end) = self.name_end(self.position) {
            let word: String = self.text[self.position..end].iter().collect();
            self.position = end;
            if word != "return" && (word != "s" || !self.take("=")?) {
                return self.fail();
            }
        }
        let result = self.value(0)?;
        self.take(";")?;
        self.space()?;
        match result {
            Lua::Table(t) if self.position == self.text.len() => Ok(t),
            _ => self.fail(),
        }
    }
}

/// Port of `lua_data.parse(text, **limits)`.
pub fn parse(text: &str, limits: Limits) -> Result<Table> {
    Parser::new(text, limits)?.parse()
}

/// Port of `lua_data.sequence`: the values of a table whose keys are exactly 1..n, in order.
pub fn sequence(table: &Lua) -> Result<Vec<Lua>> {
    let Lua::Table(t) = table else {
        return Err(Error::data("순서가 있는 데이터 배열이 아닙니다."));
    };
    for key in t.keys() {
        let bad = match key {
            Key::Int(i) => *i < 1,
            Key::Huge(bits) => f64::from_bits(*bits) < 0.0,
            _ => true,
        };
        if bad {
            return Err(Error::data("순서가 있는 데이터 배열이 아닙니다."));
        }
    }
    let n = t.len() as i128;
    let mut out = Vec::with_capacity(t.len());
    for i in 1..=n {
        match t.get(&Key::Int(i)) {
            Some(v) => out.push(v.clone()),
            None => return Err(Error::data("데이터 배열의 순서가 연속되지 않습니다.")),
        }
    }
    Ok(out)
}
