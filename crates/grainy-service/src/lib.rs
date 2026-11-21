//! Port of `luma/photo_service.py`, `luma/photo_edits.py`, `luma/photo_labels.py` and `luma/command_batch.py`:
//! the layer between the catalogue and everything that changes photos in bulk (the batch commands and the AI
//! connection's tools).
//!
//! Records, settings and batch entries are JSON values shaped as the oracle's dicts, so stored records stay
//! byte-identical. Everything that needs pixels or the file system beyond the catalogue (image loading, the
//! automatic-value maths, rendering an export, lens and camera-profile fitting) is injected by the caller.

pub mod auto;
pub mod batch;
pub mod edits;
pub mod export;
pub mod labels;
mod panels;
pub mod photo_edits;
mod profiles;
mod pyutil;
pub mod stack;
mod straighten;
pub mod summary;
pub mod targets;

pub use edits::{Changes, Fitting, LensMismatch, Plain};
pub use grainy_catalog;
use serde_json::{Map, Value};

/// Port of `photo_service.FIELDS` (the edit values the AI connection takes): key, lowest and highest value.
pub const FIELDS: [(&str, i64, i64); 28] = [
    ("exposure", -5, 5),
    ("contrast", -100, 100),
    ("highlights", -100, 100),
    ("shadows", -100, 100),
    ("whites", -200, 200),
    ("blacks", -200, 200),
    ("temperature", -100, 100),
    ("tint", -100, 100),
    ("saturation", -100, 100),
    ("vibrance", -100, 100),
    ("clarity", -100, 100),
    ("sharpen", 0, 150),
    ("vignette", 0, 100),
    ("grain", 0, 100),
    ("vignette_midpoint", 0, 100),
    ("vignette_roundness", -100, 100),
    ("vignette_feather", 0, 100),
    ("vignette_highlights", 0, 100),
    ("grain_size", 0, 100),
    ("grain_roughness", 0, 100),
    ("straighten", -45, 45),
    ("monochrome", 0, 1),
    ("negative_density", -3, 3),
    ("negative_range", 3, 10),
    ("film_strength", 0, 100),
    ("film_look", 0, 0),
    ("rotation", 0, 270),
    ("flip", 0, 1),
];
/// Port of `photo_service.NEGATIVE_ONLY`.
pub const NEGATIVE_ONLY: [&str; 2] = ["negative_density", "negative_range"];
/// Port of `photo_service.CURVES`.
pub const CURVES: [&str; 2] = ["curve", "rgb_curves"];
/// Port of `photo_service.CURVE_POINTS`.
pub const CURVE_POINTS: usize = 16;
/// Port of `photo_service.COPY_GROUPS`: apply_edits copy groups by English name, to `manager.EDIT_GROUPS` names.
pub const COPY_GROUPS: [(&str, &str); 15] = [
    ("working_space", "작업 색공간"),
    ("light", "빛"),
    ("white_balance", "화이트 밸런스"),
    ("raw_profile", "RAW 프로파일"),
    ("color", "색상"),
    ("film", "필름"),
    ("color_filter", "컬러 필터"),
    ("curves", "커브"),
    ("detail", "디테일"),
    ("effects", "효과"),
    ("negative", "네거티브 반전"),
    ("lens", "렌즈·원근"),
    ("crop", "크롭·회전"),
    ("masks", "마스크"),
    ("retouch", "복구"),
];
/// Port of `photo_service.GEOMETRY`: what a preset leaves as it is.
pub const GEOMETRY: [&str; 4] = ["crop", "rotation", "straighten", "flip"];
/// Port of `photo_service.MAX_TARGETS`.
pub const MAX_TARGETS: usize = 500;
/// Port of `photo_service.MAX_ID`.
pub const MAX_ID: i64 = (1 << 53) - 1;
/// Port of `photo_service.MCP_DEPTH`: AI batches that can be undone in order.
pub const MCP_DEPTH: usize = 5;
/// Port of `photo_service.INDEX_KEY`: the catalogue preference holding the AI batch index.
pub const INDEX_KEY: &str = "mcp_batches";
/// Port of `photo_service.BATCH_PREFIX`.
pub const BATCH_PREFIX: &str = "undo:mcp:";
/// Port of `photo_service.LABELS`: English label names to the stored Korean names.
pub const LABELS: [(&str, &str); 5] =
    [("red", "빨강"), ("yellow", "노랑"), ("green", "초록"), ("blue", "파랑"), ("purple", "보라")];
/// Port of `photo_service.FLAGS`.
pub const FLAGS: [(&str, i64); 3] = [("picked", 1), ("rejected", -1), ("unflagged", 0)];
/// Port of `photo_service.SORTS`.
pub const SORTS: [&str; 4] = ["imported", "name", "capture", "rating"];
/// Port of `photo_service.PRESET_KEY`: user metadata naming the preset apply_edits applied last.
pub const PRESET_KEY: &str = "_preset";
/// Port of `engine.VIDEO_EXTENSIONS` (copied; batch tools skip videos an older version imported).
pub const VIDEO_EXTENSIONS: [&str; 5] = [".mp4", ".mov", ".avi", ".mkv", ".m4v"];
pub use grainy_core::formats::{IMAGE_EXTENSIONS, RAW_EXTENSIONS};

/// Kind of `command_batch.BatchConflict`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// A photo's edits or path changed.
    Changed,
    /// A photo's undo history changed.
    History,
    /// No such batch in the expected state.
    None,
}
impl Conflict {
    /// The oracle's `kind` string.
    pub fn name(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::History => "history",
            Self::None => "none",
        }
    }
}

/// A failure of an injected step (loading, computing, writing a file), named as the Python exception it stands
/// for so that the oracle's English texts (`reason`, `english_error`) come out the same.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The Python exception class name (`ValueError`, `PermissionError`, `FileNotFoundError`, `OSError`, …).
    pub kind: String,
    /// `str(error)`.
    pub message: String,
}
impl Failure {
    /// A failure of the given Python class.
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self { kind: kind.into(), message: message.into() }
    }
    /// A file-system error as the Python `OSError` subclass Python raises for it.
    pub fn io(error: &std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        let kind = match error.kind() {
            K::PermissionDenied => "PermissionError",
            K::NotFound => "FileNotFoundError",
            K::AlreadyExists => "FileExistsError",
            K::IsADirectory => "IsADirectoryError",
            K::NotADirectory => "NotADirectoryError",
            _ => "OSError",
        };
        Self::new(kind, error.to_string())
    }
    /// Whether the class is `OSError` or one of its subclasses.
    pub fn is_os_error(&self) -> bool {
        [
            "OSError",
            "PermissionError",
            "FileNotFoundError",
            "FileExistsError",
            "IsADirectoryError",
            "NotADirectoryError",
            "TimeoutError",
            "InterruptedError",
            "BlockingIOError",
            "ConnectionError",
            "BrokenPipeError",
            "ChildProcessError",
            "ProcessLookupError",
        ]
        .contains(&self.kind.as_str())
    }
}

/// Errors of this crate, one variant per Python exception class the oracle raises.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `photo_service.ServiceError`: a request the caller can correct; English text for the AI client.
    #[error("{0}")]
    Service(String),
    /// `photo_service.Busy`: nothing was started, so nothing changed.
    #[error("{0}")]
    Busy(String),
    /// `command_batch.BatchConflict` (Korean text, as the oracle).
    #[error("{message}")]
    Conflict {
        /// The message.
        message: String,
        /// The conflict kind.
        kind: Conflict,
    },
    /// A Python `ValueError` raised by the oracle with this text.
    #[error("{0}")]
    Value(String),
    /// A Python `RuntimeError` raised by the oracle with this text.
    #[error("{0}")]
    Runtime(String),
    /// A Python `KeyError` (an unknown key in a fixed table); the text is the key's repr.
    #[error("{0}")]
    Key(String),
    /// A Python `TypeError` on a value of the wrong type.
    #[error("{0}")]
    Type(String),
    /// `photo_service.OriginalChanged`: the original changed while it was read.
    #[error("{0}")]
    OriginalChanged(String),
    /// A failure of an injected step.
    #[error("{}", .0.message)]
    Failed(Failure),
    /// A catalogue or SQLite failure.
    #[error(transparent)]
    Catalog(#[from] grainy_catalog::Error),
}
impl From<grainy_catalog::rusqlite::Error> for Error {
    fn from(error: grainy_catalog::rusqlite::Error) -> Self {
        Self::Catalog(error.into())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Catalog(error.into())
    }
}
impl Error {
    /// The Python exception class name of the oracle's equivalent error.
    pub fn python_name(&self) -> &str {
        match self {
            Self::Service(_) => "ServiceError",
            Self::Busy(_) => "Busy",
            Self::Conflict { .. } => "BatchConflict",
            Self::Value(_) => "ValueError",
            Self::Runtime(_) => "RuntimeError",
            Self::Key(_) => "KeyError",
            Self::Type(_) => "TypeError",
            Self::OriginalChanged(_) => "OriginalChanged",
            Self::Failed(f) => &f.kind,
            Self::Catalog(grainy_catalog::Error::Sql(_)) => "sqlite3.Error",
            Self::Catalog(grainy_catalog::Error::Invalid(_)) => "ValueError",
            Self::Catalog(_) => "CatalogError",
        }
    }
    /// The conflict kind of a `BatchConflict`.
    pub fn conflict(&self) -> Option<Conflict> {
        match self {
            Self::Conflict { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

/// Result of this crate.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn service(message: impl Into<String>) -> Error {
    Error::Service(message.into())
}

pub(crate) fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(|| Error::Type("expected an object".into()))
}

pub(crate) fn id_of(value: &Value) -> Result<i64> {
    value.get("id").and_then(Value::as_i64).ok_or_else(|| Error::Type("a record needs an integer id".into()))
}

pub(crate) fn text_of<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| Error::Key(format!("'{key}'")))
}

/// Port of `engine.normalized` with the oracle's key order (the defaults' order), as a JSON object.
pub fn normalized(value: &Value) -> Result<Value> {
    Ok(Value::Object(grainy_catalog::settings(value)?.0))
}
