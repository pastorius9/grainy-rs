//! Port of luma/whats_new.py (everything except `show`, the Qt window): what changed, shown once when a newer
//! version is opened for the first time.
//!
//! The text is the change log that ships with the app: `CHANGELOG.md` (Korean) and `assets/whats-new-en.md`
//! (English, shorter; a version without an English entry falls back to the Korean one) under a root folder.
//! `assets/feedback-done.json` maps a release to the feedback references it addressed.

use std::collections::HashSet;
use std::path::Path;

use grainy_i18n::Language;

use crate::feedback::SENT;
use crate::json::{Object, Value};
use crate::preferences::Preferences;
use crate::util::py_strip;
use crate::version::{self, VersionKey};

/// Versions listed at most, newest first.
pub const LIMIT: usize = 5;
/// The preference remembering the last version that was opened.
pub const KEY: &str = "version_seen";
/// The preference remembering the feedback references already thanked for.
pub const THANKED: &str = "feedback_thanked";

/// One change log section: `(version, title, body)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The version number text, such as `0.6.8`.
    pub version: String,
    /// The title after the dash, or empty.
    pub title: String,
    /// The text under the heading, trimmed.
    pub body: String,
}

/// Port of `version_key`: the first four numbers of the text (none for `None`).
pub fn version_key(text: Option<&str>) -> VersionKey {
    version::key(text.unwrap_or(""), Some(4))
}

/// Parses `## 1.2.3[ — title]` as the oracle's `HEADING` pattern does for one line; returns the end of the version
/// and the title. The line has no `\n`, but may end with `\r`, which then belongs to the title (or spoils the match).
fn heading(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("## ")?;
    let bytes = rest.as_bytes();
    let digits = |from: usize| bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let mut end = digits(0);
    if end == 0 {
        return None;
    }
    let mut dots = 0;
    while end < bytes.len() && bytes[end] == b'.' {
        let more = digits(end + 1);
        if more == 0 {
            break;
        }
        end += 1 + more;
        dots += 1;
    }
    if dots == 0 {
        return None;
    }
    let version = &rest[..end];
    let tail = rest[end..].trim_start_matches([' ', '\t']);
    match tail.strip_prefix('—') {
        Some(title) => {
            let title = title.trim_start_matches([' ', '\t']).trim_end_matches([' ', '\t']);
            Some((version.to_string(), title.to_string()))
        }
        None if tail.is_empty() => Some((version.to_string(), String::new())),
        None => None,
    }
}

/// Port of `sections`: `[(version, title, body)]` of a change log, in the order written.
pub fn sections(text: &str) -> Vec<Entry> {
    let mut found: Vec<(usize, usize, String, String)> = Vec::new(); // heading start, end of the heading text
    let mut offset = 0;
    for line in text.split('\n') {
        if let Some((version, title)) = heading(line) {
            found.push((offset, offset + line.len(), version, title));
        }
        offset += line.len() + 1;
    }
    let mut result = Vec::new();
    for (index, (_, end, version, title)) in found.iter().enumerate() {
        let next = found.get(index + 1).map_or(text.len(), |f| f.0);
        result.push(Entry {
            version: version.clone(),
            title: title.clone(),
            body: py_strip(&text[*end..next]).to_string(),
        });
    }
    result
}

fn read(path: &Path) -> Vec<Entry> {
    match std::fs::read(path).ok().and_then(|bytes| String::from_utf8(bytes).ok()) {
        // Python's read_text translates line ends: CRLF and a lone CR become LF.
        Some(text) => sections(&text.replace("\r\n", "\n").replace('\r', "\n")),
        None => Vec::new(),
    }
}

/// Port of `notes`: sections newer than `previous` up to `current`, newest first; only `current` when `previous`
/// is unknown. `lang` defaults to the interface language in use.
pub fn notes(previous: Option<&str>, current: &str, lang: Option<Language>, root: &Path) -> Vec<Entry> {
    let korean = read(&root.join("CHANGELOG.md"));
    let mut english: Vec<Entry> = Vec::new();
    for entry in read(&root.join("assets").join("whats-new-en.md")) {
        match english.iter_mut().find(|e| e.version == entry.version) {
            Some(slot) => *slot = entry,
            None => english.push(entry),
        }
    }
    let (low, high) = (version_key(previous), version_key(Some(current)));
    let mut chosen: Vec<&Entry> = korean
        .iter()
        .filter(|entry| {
            let key = version_key(Some(&entry.version));
            key <= high && if low.is_empty() { key == high } else { key > low }
        })
        .collect();
    chosen.sort_by_key(|e| std::cmp::Reverse(version_key(Some(&e.version))));
    let english_wanted = lang.unwrap_or_else(grainy_i18n::language) == Language::English;
    chosen
        .into_iter()
        .map(|entry| {
            if english_wanted {
                english.iter().find(|e| e.version == entry.version).unwrap_or(entry).clone()
            } else {
                entry.clone()
            }
        })
        .take(LIMIT)
        .collect()
}

/// Port of `markdown`.
pub fn markdown(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|e| {
            let title = if e.title.is_empty() { String::new() } else { format!(" — {}", e.title) };
            format!("## {}{}\n\n{}", e.version, title, e.body)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What `str(previous or '')` makes of a stored value: nothing for a false one.
fn previous_text(value: Option<&Value>) -> Option<String> {
    value.filter(|v| v.truthy()).map(Value::py_str)
}

/// Port of `pending`: the entries to show now, or none. A first run (nothing to compare with) and a downgrade
/// show nothing.
pub fn pending(
    preferences: &Preferences,
    first_run: bool,
    current: &str,
    lang: Option<Language>,
    root: &Path,
) -> Vec<Entry> {
    let seen = preferences.values.get(KEY);
    let text = previous_text(seen);
    let same = matches!(seen, Some(Value::Str(s)) if s == current);
    let older = text.as_deref().is_some_and(|t| version_key(Some(t)) >= version_key(Some(current)));
    if first_run || same || older {
        return Vec::new();
    }
    notes(text.as_deref(), current, lang, root)
}

/// Port of `remember`: keeps the version just opened (a failure to write is ignored).
pub fn remember(preferences: &mut Preferences, current: &str) {
    if matches!(preferences.values.get(KEY), Some(Value::Str(s)) if s == current) {
        return;
    }
    let _ = preferences.save(KEY, Value::from(current));
}

/// Port of `done`: `{version: [references]}`, the feedback each release addressed (empty when the file is missing
/// or damaged).
pub fn done(root: &Path) -> Object {
    let path = root.join("assets").join("feedback-done.json");
    let data = std::fs::read(path).ok().and_then(|b| String::from_utf8(b).ok()).and_then(|t| Value::parse(&t).ok());
    match data {
        Some(Value::Object(object)) => object,
        _ => Object::new(),
    }
}

fn listed<'a>(preferences: &'a Preferences, key: &str) -> &'a [Value] {
    preferences.values.get(key).and_then(Value::as_list).unwrap_or(&[])
}

fn sent(preferences: &Preferences) -> Vec<String> {
    listed(preferences, SENT)
        .iter()
        .filter_map(|entry| entry.as_object()?.get("ref")?.as_str().map(str::to_string))
        .collect()
}

/// Port of `shipped`: references sent from this computer that the versions after `previous` up to `current`
/// addressed, not thanked for yet. The range of [`notes`]; the same version or a downgrade gives none.
pub fn shipped(preferences: &Preferences, previous: Option<&str>, current: &str, root: &Path) -> Vec<String> {
    let (low, high) = (version_key(previous), version_key(Some(current)));
    if !low.is_empty() && low >= high {
        return Vec::new();
    }
    let mut addressed: HashSet<String> = HashSet::new();
    for (version, refs) in done(root).iter() {
        let Some(refs) = refs.as_list() else { continue };
        let key = version_key(Some(version));
        let inside = if low.is_empty() { key == high } else { low < key && key <= high };
        if inside {
            for reference in refs {
                addressed.insert(reference.py_str().trim_start_matches('#').to_uppercase());
            }
        }
    }
    let thanked: HashSet<&str> = listed(preferences, THANKED).iter().filter_map(Value::as_str).collect();
    let mut seen = HashSet::new();
    sent(preferences)
        .into_iter()
        .filter(|reference| seen.insert(reference.clone()))
        .filter(|reference| addressed.contains(reference) && !thanked.contains(reference.as_str()))
        .collect()
}

/// Port of `thank`: remembers the references thanked for, so the line never repeats; those no longer recorded as
/// sent are dropped.
pub fn thank(preferences: &mut Preferences, refs: &[String]) {
    if refs.is_empty() {
        return;
    }
    let sent: HashSet<String> = sent(preferences).into_iter().collect();
    let mut kept: Vec<Value> = listed(preferences, THANKED)
        .iter()
        .filter(|v| v.as_str().is_some_and(|s| sent.contains(s) && !refs.iter().any(|r| r == s)))
        .cloned()
        .collect();
    kept.extend(refs.iter().map(|r| Value::from(r.as_str())));
    let _ = preferences.save(THANKED, Value::List(kept));
}

/// Port of `thanks`: the one sentence above the notes.
pub fn thanks(refs: &[String], lang: Language) -> String {
    let joined = refs.iter().map(|r| format!("#{r}")).collect::<Vec<_>>().join(", ");
    let source = "보내 주신 피드백({0})이 이번 업데이트에 반영되었습니다. 고맙습니다.";
    grainy_i18n::try_tr_in(lang, source, &[&joined]).unwrap_or_else(|_| grainy_i18n::translate(source, lang))
}
