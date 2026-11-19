//! Port of luma/feedback.py (everything except the Qt dialog): the text a person types, plus the app version and a
//! one-line system description, is posted to a Google Form that fills a spreadsheet. Nothing is sent here:
//! [`send`] takes the sender as a function, and the tests never touch the network.
//!
//! Each message carries a short random reference at the end of the system line. The references sent are kept on
//! this computer only (preference `feedback_sent`), so that an update can say when one shipped
//! ([`crate::whats_new::shipped`]).

use crate::json::{Object, Value};
use crate::preferences::Preferences;
use crate::user_paths::{Environment, System};
use crate::util::{py_strip, random_below};
use crate::APP_VERSION;

/// Google Form receiving the answers (the part of its public URL between `/d/e/` and `/viewform`).
pub const FORM_ID: &str = "1FAIpQLSd586uKnf3TmtV25wV0kthw_YwOiNHAIpXE8AbefLpSNaQXEw";
/// The entry number of each question: version, message, system.
pub const FIELDS: [(&str, &str); 3] = [("version", "1045781291"), ("message", "1065046570"), ("system", "839337160")];
/// Longest message kept, in characters.
pub const LIMIT: usize = 4000;
/// Seconds before a post gives up.
pub const TIMEOUT: u64 = 15;
/// The reference alphabet: no look-alikes (0/O, 1/I/L).
pub const ALPHABET: &str = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// The preference holding the references sent from this computer, newest last.
pub const SENT: &str = "feedback_sent";
/// How many sent references are kept.
pub const KEEP: usize = 50;

/// Port of `configured`.
pub fn configured() -> bool {
    !FORM_ID.is_empty() && FIELDS.iter().all(|(_, entry)| !entry.is_empty())
}

/// The graphics adapter part of the system line.
#[derive(Debug, Clone)]
pub struct Gpu {
    /// The adapter name.
    pub adapter: String,
    /// Whether the GPU path is switched on.
    pub enabled: bool,
}

/// What `system_info` reads from the machine, as parameters.
#[derive(Debug, Clone)]
pub struct SystemFacts {
    /// The operating system.
    pub system: System,
    /// macOS product version (`platform.mac_ver()[0]`).
    pub mac_version: String,
    /// Windows release (`platform.release()`, such as `11`).
    pub windows_release: String,
    /// Windows version (`platform.version()`, such as `10.0.26200`).
    pub windows_version: String,
    /// `platform.machine()`.
    pub machine: String,
    /// The graphics adapter, when there is one.
    pub gpu: Option<Gpu>,
}

/// Port of `system_info`: OS and graphics adapter only (no user, computer or file names).
pub fn system_info(facts: &SystemFacts) -> String {
    let system = if facts.system.is_mac() {
        format!("macOS {}", facts.mac_version)
    } else {
        format!("Windows {} ({})", facts.windows_release, facts.windows_version)
    };
    let mut parts = vec![system, facts.machine.clone()];
    if let Some(gpu) = facts.gpu.as_ref().filter(|g| !g.adapter.is_empty()) {
        parts.push(if gpu.enabled { format!("GPU {}", gpu.adapter) } else { format!("GPU {} (off)", gpu.adapter) });
    }
    parts.join(" · ")
}

/// Port of `reference`: five random characters naming one message; nothing about the person or the computer.
pub fn reference() -> String {
    reference_with(&mut random_below)
}

/// [`reference`] with the random choice given (`pick(n)` returns a number below `n`).
pub fn reference_with(pick: &mut dyn FnMut(usize) -> usize) -> String {
    let letters: Vec<char> = ALPHABET.chars().collect();
    (0..5).map(|_| letters[pick(letters.len())]).collect()
}

/// Port of `tagged`: the reference rides at the end of the existing system answer.
pub fn tagged(system: &str, reference: &str) -> String {
    format!("{system} · #{reference}")
}

/// Port of `records`: where sent references are kept, the per-user settings.
pub fn records(env: &Environment) -> Preferences {
    Preferences::at_default(env)
}

/// Port of `record`: keeps a sent reference, newest last, at most [`KEEP`]. `today` is an ISO date.
pub fn record(preferences: &mut Preferences, reference: &str, version: &str, today: &str) -> std::io::Result<()> {
    let mut sent: Vec<Value> = match preferences.values.get(SENT) {
        Some(Value::List(items)) => items.iter().filter(|e| matches!(e, Value::Object(_))).cloned().collect(),
        _ => Vec::new(),
    };
    let mut entry = Object::new();
    entry.insert("ref", Value::from(reference));
    entry.insert("version", Value::from(version));
    entry.insert("sent", Value::from(today));
    sent.push(Value::Object(entry));
    let start = sent.len().saturating_sub(KEEP);
    preferences.save(SENT, Value::List(sent.split_off(start)))
}

/// Today's date as `YYYY-MM-DD` in local time (`date.today().isoformat()`).
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Port of `payload`: the form fields `entry.<number>` in the order version, message, system.
pub fn payload(message: &str, system: &str, version: &str) -> Vec<(String, String)> {
    let message: String = py_strip(message).chars().take(LIMIT).collect();
    let values = [("version", version.to_string()), ("message", message), ("system", system.to_string())];
    values
        .into_iter()
        .map(|(key, value)| {
            let entry = FIELDS.iter().find(|(name, _)| *name == key).map_or("", |(_, entry)| entry);
            (format!("entry.{entry}"), value)
        })
        .collect()
}

/// `urllib.parse.quote_plus`: unreserved characters stay, a space is `+`, the rest `%XX` of the UTF-8 bytes.
pub fn quote_plus(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `urllib.parse.urlencode`.
pub fn urlencode(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v))).collect::<Vec<_>>().join("&")
}

/// The post that [`send`] hands to the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The form's `formResponse` address.
    pub url: String,
    /// Header name and value pairs.
    pub headers: Vec<(String, String)>,
    /// The url-encoded form body.
    pub body: Vec<u8>,
}

/// How a post can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendFailure {
    /// The server answered with a status other than 200.
    Http(u16),
    /// The request failed before an answer (the text of the reason).
    Other(String),
}

/// Port of `send`: posts one answer through `post` (given the request and [`TIMEOUT`]; it returns the HTTP status).
/// Returns an empty string on success, otherwise a short reason.
pub fn send(
    message: &str,
    system: &str,
    version: &str,
    post: &dyn Fn(&Request, u64) -> Result<u16, SendFailure>,
) -> String {
    if !configured() {
        return "not configured".into();
    }
    let request = Request {
        url: format!("https://docs.google.com/forms/d/e/{FORM_ID}/formResponse"),
        headers: vec![
            ("User-Agent".into(), format!("Grainy/{version}")),
            ("Content-Type".into(), "application/x-www-form-urlencoded".into()),
        ],
        body: urlencode(&payload(message, system, version)).into_bytes(),
    };
    match post(&request, TIMEOUT) {
        Ok(200) => String::new(),
        Ok(status) => format!("HTTP {status}"),
        Err(SendFailure::Http(code)) => format!("HTTP {code}"),
        // `str(error) or type(error).__name__`
        Err(SendFailure::Other(reason)) if reason.is_empty() => "OSError".into(),
        Err(SendFailure::Other(reason)) => reason,
    }
}

/// The send state of the feedback dialog without the window: the reference is kept while the same text is sent
/// again after a failure, and a new text gets a new reference.
#[derive(Debug, Clone, Default)]
pub struct Outbox {
    reference: Option<String>,
    attempt: Option<String>,
}

impl Outbox {
    /// Port of the first lines of `FeedbackDialog.submit`: `text` is stripped; returns the stripped text, the
    /// reference for it and the tagged system line, or none when the text is empty.
    pub fn prepare(&mut self, text: &str, system: &str) -> Option<(String, String, String)> {
        let text = py_strip(text).to_string();
        if text.is_empty() {
            return None;
        }
        if self.reference.is_none() || self.attempt.as_deref() != Some(&text) {
            self.reference = Some(reference());
            self.attempt = Some(text.clone());
        }
        let reference = self.reference.clone().unwrap_or_default();
        let tagged = tagged(system, &reference);
        Some((text, tagged, reference))
    }

    /// After a successful send: forgets the attempt and returns the reference to record.
    pub fn sent(&mut self) -> Option<String> {
        self.attempt = None;
        self.reference.take()
    }
}

/// The version recorded with a reference when the caller has no other.
pub const VERSION: &str = APP_VERSION;
