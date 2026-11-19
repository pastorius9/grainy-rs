//! Version numbers compared the way the oracle does: the runs of digits of the text, as numbers, in order.

use std::cmp::Ordering;

/// One run of digits, ordered as a number of any size (leading zeros ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Number(String);

impl Number {
    /// The number as decimal text without leading zeros (`"0"` for zero), as Python prints the `int`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Ord for Number {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.len().cmp(&other.0.len()).then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The oracle's version tuple: compares like a Python tuple of ints (a shorter prefix is smaller).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionKey(pub Vec<Number>);

impl VersionKey {
    /// Whether no digits were found (Python's empty tuple, which is false).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The numbers as decimal text.
    pub fn parts(&self) -> Vec<String> {
        self.0.iter().map(|n| n.0.clone()).collect()
    }
}

/// `tuple(int(part) for part in re.findall(r'\d+', text))`, cut to `limit` parts when given.
pub fn key(text: &str, limit: Option<usize>) -> VersionKey {
    let mut parts = Vec::new();
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            run.push(c);
        } else if !run.is_empty() {
            let trimmed = run.trim_start_matches('0');
            parts.push(Number(if trimmed.is_empty() { "0".into() } else { trimmed.into() }));
            run.clear();
        }
    }
    if let Some(limit) = limit {
        parts.truncate(limit);
    }
    VersionKey(parts)
}
