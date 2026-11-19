//! Port of tools/compile_translations.py: the `.txt` translation sources (`source => translation`, one pair per
//! line, `\n` for a line break) and the rules that turn them into `en.json`.

use crate::format::field_names;
use std::collections::HashMap;
use std::fmt;

/// A translation source that the compiler rejects (the oracle raises `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// The same source with two different translations.
    Duplicate { file: String, line: usize, source: String },
    /// Source and translation have different placeholders (or a template does not parse).
    ChangedPlaceholders { file: String, line: usize },
    /// A non-blank line without ` => `.
    NoSeparator { file: String, line: usize },
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::Duplicate { file, line, source } => {
                write!(f, "Duplicate translation: {file}:{line} {source}")
            }
            CompileError::ChangedPlaceholders { file, line } => write!(f, "Changed placeholders: {file}:{line}"),
            CompileError::NoSeparator { file, line } => write!(f, "No ' => ' separator: {file}:{line}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Python's `str.splitlines()` (no line ends kept): splits at the same characters, `\r\n` counting once.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let is_break =
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if is_break {
            lines.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = chars.peek() {
                    chars.next();
                    end += 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Compile `(file name, text)` sources, given in the order `sorted(directory.glob('en*.txt'))` yields them,
/// into the source -> English table.
pub fn compile_sources(sources: &[(String, String)]) -> Result<HashMap<String, String>, CompileError> {
    let mut result: HashMap<String, String> = HashMap::new();
    for (file, text) in sources {
        for (index, line) in splitlines(text).into_iter().enumerate() {
            let number = index + 1;
            if line.trim().is_empty() {
                continue;
            }
            let (source, target) = line
                .split_once(" => ")
                .ok_or_else(|| CompileError::NoSeparator { file: file.clone(), line: number })?;
            let source = source.replace("\\n", "\n");
            let target = target.replace("\\n", "\n");
            if let Some(old) = result.get(&source) {
                if *old != target {
                    return Err(CompileError::Duplicate { file: file.clone(), line: number, source });
                }
            }
            let a = field_names(&source);
            let b = field_names(&target);
            match (a, b) {
                (Ok(a), Ok(b)) if a == b => {}
                _ => return Err(CompileError::ChangedPlaceholders { file: file.clone(), line: number }),
            }
            result.insert(source, target);
        }
    }
    Ok(result)
}

/// Read every `en*.txt` file of a directory (sorted by file name, as Python's `sorted(Path.glob)`) and compile it.
pub fn compile_directory(directory: &std::path::Path) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let mut names: Vec<String> = std::fs::read_dir(directory)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("en") && n.ends_with(".txt"))
        .collect();
    names.sort();
    let mut sources = Vec::new();
    for name in names {
        let text = std::fs::read_to_string(directory.join(&name))?;
        sources.push((name, text));
    }
    Ok(compile_sources(&sources)?)
}
