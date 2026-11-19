//! Grainy's interface translation. The interface text is written in Korean in the source and translated to
//! English through the table `assets/en.json` (the same table the Python Grainy compiles from `assets/i18n/en*.txt`).
//!
//! Also holds the keyboard shortcut table ([`shortcuts`]) and the editable fields ([`fields`]).

pub mod compile;
pub mod fields;
mod format;
pub mod shortcuts;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

pub use format::{format, FormatError};

const EN_JSON: &str = include_str!("../assets/en.json");

/// An interface language. Port of `LANGUAGES = ('ko', 'en')` in luma/i18n.py.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// Korean, the language of the source strings (no translation happens).
    Korean,
    /// English, translated through the table.
    English,
}

impl Language {
    /// The code the oracle stores: `"ko"` or `"en"`.
    pub fn code(self) -> &'static str {
        match self {
            Language::Korean => "ko",
            Language::English => "en",
        }
    }

    /// Port of the validation in `set_language`: only `"ko"` and `"en"` are accepted.
    pub fn parse(code: &str) -> Result<Language, UnsupportedLanguage> {
        match code {
            "ko" => Ok(Language::Korean),
            "en" => Ok(Language::English),
            _ => Err(UnsupportedLanguage),
        }
    }
}

/// The oracle's `ValueError('Unsupported interface language')`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedLanguage;

impl std::fmt::Display for UnsupportedLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Unsupported interface language")
    }
}

impl std::error::Error for UnsupportedLanguage {}

/// The operating system, as far as the shortcut text is concerned (`sys.platform == 'darwin'` in the oracle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// Windows.
    Windows,
    /// macOS (`darwin`): Qt's Ctrl is the Command key.
    MacOs,
    /// Any other system; written like Windows.
    Linux,
}

impl Platform {
    /// The platform this program was built for.
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

static LANGUAGE: AtomicU8 = AtomicU8::new(0);

/// Port of luma/i18n.py `language()`. Starts as Korean.
pub fn language() -> Language {
    if LANGUAGE.load(Ordering::Relaxed) == 1 {
        Language::English
    } else {
        Language::Korean
    }
}

/// Port of luma/i18n.py `set_language()`. Thread-safe.
pub fn set_language(value: Language) {
    LANGUAGE.store(u8::from(value == Language::English), Ordering::Relaxed);
}

/// The English table, parsed once from the embedded `assets/en.json`.
pub fn english_table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(EN_JSON).unwrap_or_default())
}

/// The raw embedded `en.json` text.
pub fn english_json() -> &'static str {
    EN_JSON
}

/// Port of luma/i18n.py `keys(text)`: on macOS every `Ctrl` or `Ctrl+` becomes the Command sign; elsewhere the
/// text is unchanged. Takes the platform as a parameter; see [`keys_here`].
pub fn keys(text: &str, platform: Platform) -> String {
    if platform != Platform::MacOs {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("Ctrl") {
        out.push_str(&rest[..at]);
        out.push('\u{2318}');
        rest = &rest[at + 4..];
        if let Some(after) = rest.strip_prefix('+') {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// [`keys`] for the platform the program runs on.
pub fn keys_here(text: &str) -> String {
    keys(text, Platform::current())
}

/// The translation of `source` for the given language without any formatting: the table value in English when
/// there is one, otherwise `source` itself.
pub fn translate(source: &str, language: Language) -> String {
    match language {
        Language::Korean => source.to_string(),
        Language::English => english_table().get(source).cloned().unwrap_or_else(|| source.to_string()),
    }
}

/// Port of luma/i18n.py `tr(source, *values)` for a given language. With no values the text is returned as it
/// is (no `{{` unescaping); with values it goes through Python's `str.format`, whose errors are returned.
pub fn try_tr_in(language: Language, source: &str, values: &[&str]) -> Result<String, FormatError> {
    let result = translate(source, language);
    if values.is_empty() {
        Ok(result)
    } else {
        format(&result, values)
    }
}

/// [`try_tr_in`] for the current language.
pub fn try_tr_with(source: &str, values: &[&str]) -> Result<String, FormatError> {
    try_tr_in(language(), source, values)
}

/// Port of luma/i18n.py `tr(source)`: translate for the current language.
pub fn tr(source: &str) -> String {
    translate(source, language())
}

/// Port of luma/i18n.py `tr(source, *values)`: translate, then fill `{0}`, `{1}`... with `values`.
/// Where the oracle would raise (a missing value, a stray brace), this returns the translated text unformatted;
/// use [`try_tr_with`] to see the error.
pub fn tr_with(source: &str, values: &[&str]) -> String {
    try_tr_with(source, values).unwrap_or_else(|_| tr(source))
}

/// `tr_with!("text {0}", a, b)`: values are anything that is `Display`.
#[macro_export]
macro_rules! tr_with {
    ($source:expr $(, $value:expr)* $(,)?) => {{
        let values: ::std::vec::Vec<::std::string::String> = ::std::vec![$(::std::string::ToString::to_string(&$value)),*];
        let refs: ::std::vec::Vec<&str> = values.iter().map(|v| v.as_str()).collect();
        $crate::tr_with($source, &refs)
    }};
}
