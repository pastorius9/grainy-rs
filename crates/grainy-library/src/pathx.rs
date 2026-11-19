//! Python `pathlib` / `os.path` behaviour on path text, so that stored catalogue paths and comparisons come out as
//! the oracle's do. Everything works on `&str` with an explicit [`Platform`] (Windows rules are the ones that run
//! here; macOS and POSIX rules are kept for the oracle's branches).

use grainy_catalog::folders::Platform;
use std::io;
use std::path::{Path, PathBuf};
use unicode_casefold::UnicodeCaseFold;

/// `str.casefold()`.
pub fn casefold(text: &str) -> String {
    text.case_fold().collect()
}

fn sep(platform: Platform) -> char {
    if platform == Platform::Windows {
        '\\'
    } else {
        '/'
    }
}

/// Splits like `PurePath`: anchor (drive and root) and the remaining parts without `.` and empty parts.
pub fn split(platform: Platform, path: &str) -> (String, Vec<String>) {
    let windows = platform == Platform::Windows;
    let text = if windows { path.replace('/', "\\") } else { path.to_owned() };
    let s = sep(platform);
    let mut rest = text.as_str();
    let mut anchor = String::new();
    if windows && rest.starts_with("\\\\") {
        let bits: Vec<&str> = rest[2..].splitn(3, '\\').collect();
        if bits.len() >= 2 && !bits[0].is_empty() {
            anchor = format!("\\\\{}\\{}\\", bits[0], bits[1]);
            rest = bits.get(2).copied().unwrap_or("");
        } else {
            anchor = "\\".into();
            rest = rest.trim_start_matches('\\');
        }
    } else if windows && rest.len() >= 2 && rest.as_bytes()[1] == b':' && rest.as_bytes()[0].is_ascii_alphabetic() {
        anchor = rest[..2].into();
        rest = &rest[2..];
        if rest.starts_with(s) {
            anchor.push(s);
            rest = rest.trim_start_matches(s);
        }
    } else if rest.starts_with(s) {
        anchor =
            if !windows && rest.starts_with("//") && !rest.starts_with("///") { "//".into() } else { s.to_string() };
        rest = rest.trim_start_matches(s);
    }
    (anchor, rest.split(s).filter(|p| !p.is_empty() && *p != ".").map(str::to_owned).collect())
}

fn assemble(platform: Platform, anchor: &str, parts: &[String]) -> String {
    let text = format!("{anchor}{}", parts.join(&sep(platform).to_string()));
    if text.is_empty() {
        ".".into()
    } else {
        text
    }
}

/// `str(Path(path))`.
pub fn norm_str(platform: Platform, path: &str) -> String {
    let (anchor, parts) = split(platform, path);
    assemble(platform, &anchor, &parts)
}

/// `str(Path(path).parent)`.
pub fn parent(platform: Platform, path: &str) -> String {
    let (anchor, mut parts) = split(platform, path);
    parts.pop();
    assemble(platform, &anchor, &parts)
}

/// `Path(path).name`.
pub fn name(platform: Platform, path: &str) -> String {
    split(platform, path).1.pop().unwrap_or_default()
}

/// `len(Path(path).parts)`.
pub fn parts_len(platform: Platform, path: &str) -> usize {
    let (anchor, parts) = split(platform, path);
    parts.len() + usize::from(!anchor.is_empty())
}

/// `[str(p) for p in Path(path).parents]`.
pub fn parents(platform: Platform, path: &str) -> Vec<String> {
    let (anchor, mut parts) = split(platform, path);
    let mut result = Vec::new();
    while !parts.is_empty() {
        parts.pop();
        result.push(assemble(platform, &anchor, &parts));
    }
    result
}

/// `Path(path) == Path(path).parent`: a drive or filesystem root.
pub fn is_root(platform: Platform, path: &str) -> bool {
    split(platform, path).1.is_empty()
}

/// `str(Path(base) / relative)`; an absolute `relative` replaces the base.
pub fn join(platform: Platform, base: &str, relative: &str) -> String {
    let (anchor, parts) = split(platform, relative);
    if !anchor.is_empty() {
        return assemble(platform, &anchor, &parts);
    }
    let (base_anchor, mut all) = split(platform, base);
    all.extend(parts);
    assemble(platform, &base_anchor, &all)
}

/// `Path(a) == Path(b)`: case-insensitive on Windows, exact elsewhere.
pub fn same(platform: Platform, a: &str, b: &str) -> bool {
    let (x, y) = (norm_str(platform, a), norm_str(platform, b));
    if platform == Platform::Windows {
        x.to_lowercase() == y.to_lowercase()
    } else {
        x == y
    }
}

/// `Path(parent) in Path(child).parents` with `PurePath` equality.
pub fn is_parent_of(platform: Platform, parent_path: &str, child: &str) -> bool {
    parents(platform, child).iter().any(|p| same(platform, p, parent_path))
}

/// `Path.stem` and `Path.suffix` of a file name.
pub fn stem_suffix(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => (name[..i].to_owned(), name[i..].to_owned()),
        _ => (name.to_owned(), String::new()),
    }
}

/// `str(Path(path).with_suffix(suffix))`.
pub fn with_suffix(platform: Platform, path: &str, suffix: &str) -> String {
    let (anchor, mut parts) = split(platform, path);
    if let Some(last) = parts.last_mut() {
        let (stem, _) = stem_suffix(last);
        *last = format!("{stem}{suffix}");
    }
    assemble(platform, &anchor, &parts)
}

/// `str(Path(path).with_name(name))`.
pub fn with_name(platform: Platform, path: &str, new_name: &str) -> String {
    let (anchor, mut parts) = split(platform, path);
    if let Some(last) = parts.last_mut() {
        *last = new_name.to_owned();
    }
    assemble(platform, &anchor, &parts)
}

/// `os.path.splitext(path)[1]`.
pub fn splitext_ext(path: &str) -> String {
    let sep_index = path.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let Some(dot) = path.rfind('.') else { return String::new() };
    if dot + 1 > sep_index && dot >= sep_index && path[sep_index..dot].chars().any(|c| c != '.') {
        return path[dot..].to_owned();
    }
    String::new()
}

/// Whether the path has a root (`Path.is_absolute`).
pub fn is_absolute(platform: Platform, path: &str) -> bool {
    let (anchor, _) = split(platform, path);
    anchor.ends_with(sep(platform))
}

/// `str(Path(path).absolute())`.
pub fn absolute(platform: Platform, path: &str) -> String {
    if is_absolute(platform, path) {
        return norm_str(platform, path);
    }
    let cwd = std::env::current_dir().map(|p| text(&p)).unwrap_or_default();
    join(platform, &cwd, path)
}

/// A path as text without the Windows extended-length prefix.
pub fn text(path: &Path) -> String {
    let t = path.to_string_lossy();
    if let Some(unc) = t.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        t.strip_prefix("\\\\?\\").unwrap_or(&t).to_owned()
    }
}

/// `Path(path).resolve()` (non-strict): the existing part of the path is resolved, the rest is kept.
pub fn resolve(platform: Platform, path: &str) -> String {
    let absolute_text = absolute(platform, path);
    let absolute_path = PathBuf::from(&absolute_text);
    let mut ancestor = absolute_path.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for part in tail.into_iter().rev() {
                    resolved.push(part);
                }
                return norm_str(platform, &text(&resolved));
            }
            Err(_) => {
                let (Some(file), Some(up)) = (ancestor.file_name(), ancestor.parent()) else { return absolute_text };
                tail.push(file.to_owned());
                ancestor = up;
            }
        }
    }
}

/// `repr(str)` for a path in an error message.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
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

/// Python's `str(OSError)` for an `io::Error` on `path`.
pub fn os_error_text(error: &io::Error, path: &Path, errno_style: bool) -> String {
    let code = error.raw_os_error();
    let place = if path.as_os_str().is_empty() { String::new() } else { format!(": {}", py_repr(&text(path))) };
    let errno_name = match error.kind() {
        io::ErrorKind::NotFound => Some((2, "No such file or directory")),
        io::ErrorKind::PermissionDenied => Some((13, "Permission denied")),
        io::ErrorKind::AlreadyExists => Some((17, "File exists")),
        io::ErrorKind::NotADirectory => Some((20, "Not a directory")),
        io::ErrorKind::IsADirectory => Some((21, "Is a directory")),
        io::ErrorKind::StorageFull => Some((28, "No space left on device")),
        _ => None,
    };
    if errno_style || !cfg!(windows) {
        if let Some((n, message)) = errno_name {
            return format!("[Errno {n}] {message}{place}");
        }
    }
    match code {
        Some(code) if cfg!(windows) => {
            let full = error.to_string();
            let message = full.split(" (os error").next().unwrap_or(&full).trim_end_matches(['.', ' ', '\r', '\n']);
            format!("[WinError {code}] {message}{place}")
        }
        Some(code) => format!("[Errno {code}] {error}{place}"),
        None => format!("{error}{place}"),
    }
}
