//! Port of luma/lightroom_import.py: the read-only Lightroom Classic catalogue migration (`inspect_catalog`,
//! `import_catalog` with `_stage`, `_keyword_paths`, `_path`, `_metadata`, `_photos`, `_collections`, `_stacks`).
//!
//! The `.lrcat` is opened read-only (`SQLITE_OPEN_READ_ONLY`, `query_only`, one read transaction), never written.
//! Every row of the archived tables is copied as JSON text into the library's `lightroom_archive`; photos,
//! collections, stacks (and, when asked, the converted develop settings, history and snapshots) are added in one
//! `BEGIN IMMEDIATE` transaction that is rolled back on any failure or cancel: the import is all or nothing. A
//! copy of the library is written to `backups/before-lightroom-<run>.sqlite` first. Original photos are never
//! opened: the import only records their paths.
//!
//! The SQL is the oracle's, statement for statement, run on the same temporary staging tables, so rows, ids,
//! digests and orders equal the Python Grainy's (`tests/lightroom.rs` compares every table).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use grainy_catalog::folders::Platform;
use grainy_catalog::rusqlite::{self, params, types::ValueRef, Connection, OpenFlags, OptionalExtension};
use grainy_core::settings::truthy;
use grainy_library::{pathx, Cancel};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::history::{self, Warnings};

/// `REQUIRED`: the tables and columns a catalogue must have.
pub const REQUIRED: [(&str, &[&str]); 4] = [
    ("AgLibraryRootFolder", &["id_local", "absolutePath"]),
    ("AgLibraryFolder", &["id_local", "rootFolder", "pathFromRoot"]),
    ("AgLibraryFile", &["id_local", "folder", "baseName", "extension"]),
    ("Adobe_images", &["id_local", "rootFile"]),
];

/// `ARCHIVE_TABLES`: every table copied into the archive, in this order.
pub const ARCHIVE_TABLES: [&str; 30] = [
    "AgLibraryRootFolder",
    "AgLibraryFolder",
    "AgLibraryFile",
    "Adobe_images",
    "AgLibraryKeyword",
    "AgLibraryKeywordImage",
    "AgLibraryKeywordSynonym",
    "AgLibraryCollection",
    "AgLibraryCollectionImage",
    "AgLibraryCollectionContent",
    "AgLibraryFolderStack",
    "AgLibraryFolderStackImage",
    "AgLibraryCollectionStack",
    "AgLibraryCollectionStackImage",
    "AgLibraryIPTC",
    "AgHarvestedExifMetadata",
    "AgHarvestedIptcMetadata",
    "AgInternedExifCameraModel",
    "AgInternedExifCameraSN",
    "AgInternedExifLens",
    "AgInternedIptcCreator",
    "AgInternedIptcCity",
    "AgInternedIptcCountry",
    "AgInternedIptcState",
    "AgInternedIptcLocation",
    "AgInternedIptcJobIdentifier",
    "Adobe_imageDevelopSettings",
    "Adobe_libraryImageDevelopHistoryStep",
    "Adobe_libraryImageDevelopSnapshot",
    "Adobe_imageProofSettings",
];

/// `SCHEMA`: the library's own Lightroom tables and triggers.
pub const SCHEMA: [&str; 6] = [
    "CREATE TABLE IF NOT EXISTS lightroom_archive (
        source_key TEXT,kind TEXT,source_id TEXT,digest TEXT,payload TEXT NOT NULL,
        PRIMARY KEY(source_key,kind,source_id,digest))",
    "CREATE TABLE IF NOT EXISTS lightroom_import_items (
        source_key TEXT,source_id TEXT,digest TEXT,photo_id INTEGER NOT NULL,
        PRIMARY KEY(source_key,source_id,digest))",
    "CREATE TABLE IF NOT EXISTS lightroom_import_sets (
        source_key TEXT,digest TEXT,collection_id INTEGER NOT NULL,
        PRIMARY KEY(source_key,digest))",
    "CREATE TABLE IF NOT EXISTS lightroom_import_runs (
        id TEXT PRIMARY KEY,created TEXT,source_path TEXT,source_key TEXT,summary TEXT)",
    "CREATE TRIGGER IF NOT EXISTS lightroom_photo_removed AFTER DELETE ON photos BEGIN
        DELETE FROM lightroom_import_items WHERE photo_id=OLD.id; END",
    "CREATE TRIGGER IF NOT EXISTS lightroom_collection_removed AFTER DELETE ON collections BEGIN
        DELETE FROM lightroom_import_sets WHERE collection_id=OLD.id; END",
];

// ---- failures, progress, clocks -------------------------------------------------------------------------------

/// Why an import or inspection stopped.
#[derive(Debug)]
pub enum Failure {
    /// `ImportCancelled`: the cancel flag was seen.
    Cancelled,
    /// A SQLite error (`sqlite3.OperationalError` and its relatives).
    Sqlite(rusqlite::Error),
    /// Any other exception, as the last line of the oracle's traceback (`ValueError: …`).
    Error(String),
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Failure::Sqlite(error)
    }
}

impl Failure {
    /// The oracle's `ValueError(message)`.
    pub fn value(message: impl Into<String>) -> Self {
        Failure::Error(format!("ValueError: {}", message.into()))
    }

    /// Whether the oracle's `except (ImportCancelled, sqlite3.OperationalError)` catches it.
    fn cancel_like(&self) -> bool {
        match self {
            Failure::Cancelled => true,
            Failure::Sqlite(rusqlite::Error::SqliteFailure(e, _)) => e.code != rusqlite::ErrorCode::ConstraintViolation,
            Failure::Sqlite(_) | Failure::Error(_) => false,
        }
    }

    /// The text the window shows (the last line of the oracle's traceback).
    pub fn text(&self) -> String {
        match self {
            Failure::Cancelled => "luma.lightroom_import.ImportCancelled".into(),
            Failure::Sqlite(rusqlite::Error::SqliteFailure(e, message)) => {
                let class = if e.code == rusqlite::ErrorCode::ConstraintViolation {
                    "IntegrityError"
                } else {
                    "OperationalError"
                };
                format!("sqlite3.{class}: {}", message.clone().unwrap_or_else(|| e.to_string()))
            }
            Failure::Sqlite(other) => format!("sqlite3.Error: {other}"),
            Failure::Error(text) => text.clone(),
        }
    }
}

/// `progress({'stage', 'done', 'total'})`. The receiver may fail (the oracle's callback may raise); the import
/// then stops and is rolled back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub stage: &'static str,
    pub done: u64,
    pub total: u64,
}

/// The progress receiver of [`import_catalog`].
pub type Report<'a> = &'a mut dyn FnMut(&Progress) -> Result<(), String>;

/// The clock and run ids of an import (`datetime.now`, `uuid.uuid4`). The app uses [`SystemClock`]; tests use
/// [`TickingClock`] to give the oracle's fixed values.
pub trait Clock: Send {
    /// `datetime.now(timezone.utc)`.
    fn utc(&mut self) -> DateTime<Utc>;
    /// `datetime.now()` (the computer's wall clock).
    fn local(&mut self) -> NaiveDateTime;
    /// `uuid.uuid4().hex`.
    fn run_id(&mut self) -> String;
}

/// The computer's clock; run ids from the time, the process and a counter.
#[derive(Debug, Default)]
pub struct SystemClock {
    runs: u64,
}

impl Clock for SystemClock {
    fn utc(&mut self) -> DateTime<Utc> {
        Utc::now()
    }

    fn local(&mut self) -> NaiveDateTime {
        chrono::Local::now().naive_local()
    }

    fn run_id(&mut self) -> String {
        self.runs += 1;
        let seed = format!(
            "{}:{}:{}:{:?}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            std::process::id(),
            self.runs,
            std::thread::current().id()
        );
        hex(&Sha256::digest(seed.as_bytes()))[..32].to_owned()
    }
}

/// A test clock: `start` plus one second more at every reading (both [`Clock::utc`] and [`Clock::local`], the
/// local wall clock being the UTC one); run ids numbered from 1 (`0000…0001`).
#[derive(Debug, Clone)]
pub struct TickingClock {
    pub start: DateTime<Utc>,
    pub ticks: i64,
    pub runs: u64,
}

impl TickingClock {
    /// A clock starting at `start` (an ISO time with an offset, as `datetime.fromisoformat` reads it).
    pub fn new(start: &str) -> Option<Self> {
        let start = DateTime::parse_from_rfc3339(start).ok()?.with_timezone(&Utc);
        Some(Self { start, ticks: 0, runs: 0 })
    }

    fn next(&mut self) -> DateTime<Utc> {
        let value = self.start + chrono::Duration::seconds(self.ticks);
        self.ticks += 1;
        value
    }
}

impl Clock for TickingClock {
    fn utc(&mut self) -> DateTime<Utc> {
        self.next()
    }

    fn local(&mut self) -> NaiveDateTime {
        self.next().naive_utc()
    }

    fn run_id(&mut self) -> String {
        self.runs += 1;
        format!("{:032x}", self.runs)
    }
}

/// `datetime.isoformat()` of a UTC time (`+00:00`; microseconds only when not zero).
pub fn isoformat(moment: DateTime<Utc>) -> String {
    let micro = moment.timestamp_subsec_micros();
    let base = moment.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micro == 0 {
        format!("{base}+00:00")
    } else {
        format!("{base}.{micro:06}+00:00")
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn check(cancel: Option<&Cancel>) -> Result<(), Failure> {
    if cancel.is_some_and(Cancel::is_set) {
        Err(Failure::Cancelled)
    } else {
        Ok(())
    }
}

// ---- Python values -------------------------------------------------------------------------------------------

/// `_json(value)`: sorted keys, compact, non-ASCII kept (bytes are `{"base64": …}` already).
pub fn py_json(value: &Value) -> String {
    grainy_catalog::json::dumps(value, false, true, true)
}

/// `_digest(text)`: SHA-256 hex.
pub fn sha_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// Python `str(value)` of a value read from JSON.
pub fn py_str(value: &Value) -> String {
    grainy_catalog::json::text(value)
}

fn get<'a>(row: &'a Value, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    row.get(key).unwrap_or(&NULL)
}

/// `str(row.get(key))` (missing: `'None'`).
fn get_str(row: &Value, key: &str) -> String {
    py_str(get(row, key))
}

/// A SQLite cell as Python's `sqlite3` gives it, then as `_json` writes it.
fn cell(value: ValueRef<'_>) -> Result<Value, Failure> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => {
            if !f.is_finite() {
                return Err(Failure::value("Out of range float values are not JSON compliant"));
            }
            Value::from(f)
        }
        ValueRef::Text(t) => Value::String(
            std::str::from_utf8(t)
                .map_err(|_| Failure::Error("sqlite3.OperationalError: Could not decode to UTF-8 column".into()))?
                .to_owned(),
        ),
        ValueRef::Blob(b) => {
            use base64::Engine;
            let mut map = Map::new();
            map.insert("base64".into(), Value::String(base64::engine::general_purpose::STANDARD.encode(b)));
            Value::Object(map)
        }
    })
}

/// `max(low, min(high, int(value or 0)))`, 0 where `int` raises.
pub fn py_number(value: &Value, low: i64, high: i64) -> i64 {
    let n: Option<i128> = match value {
        Value::Null => Some(0),
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Some(i128::from(i)),
            None => n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i128),
        },
        Value::String(s) if s.is_empty() => Some(0),
        Value::String(s) => parse_int(s),
        _ => None,
    };
    n.map_or(0, |n| n.clamp(i128::from(low), i128::from(high)) as i64)
}

/// Python `int(text)` for base-10 text (whitespace, a sign, single underscores between digits).
fn parse_int(text: &str) -> Option<i128> {
    let t = grainy_adobe::pyutil::strip(text);
    let (negative, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let digits: String = body.chars().filter(|&c| c != '_').collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i128 = digits.parse().unwrap_or(i128::MAX);
    Some(if negative { -n } else { n })
}

// ---- Windows-style paths ---------------------------------------------------------------------------------------

fn platform() -> Platform {
    Platform::native()
}

/// `Path(text).drive` is not empty.
fn has_drive(text: &str) -> bool {
    let (anchor, _) = pathx::split(platform(), text);
    platform() == Platform::Windows && (anchor.starts_with("\\\\") || anchor.as_bytes().get(1) == Some(&b':'))
}

/// `Path(text).is_absolute()`.
fn is_absolute(text: &str) -> bool {
    let (anchor, _) = pathx::split(platform(), text);
    if platform() == Platform::Windows {
        has_drive(text) && anchor.ends_with('\\')
    } else {
        anchor.starts_with('/')
    }
}

/// `Path(path).resolve()` as text.
fn resolve(path: &Path) -> String {
    pathx::resolve(platform(), &pathx::text(path))
}

/// `Path(value)` of a stored value: text, or the oracle's `TypeError`.
fn path_text(value: &Value) -> Result<String, Failure> {
    match value {
        Value::String(s) => Ok(s.clone()),
        other => Err(Failure::Error(format!(
            "TypeError: argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{}'",
            grainy_adobe::pyutil::json_type_name(other)
        ))),
    }
}

// ---- the source catalogue ------------------------------------------------------------------------------------

/// Port of `Source`: the `.lrcat`, opened read-only in one read transaction.
pub struct Source {
    /// `str(Path(path).resolve())`.
    pub path: String,
    pub db: Connection,
    /// The catalogue's identity: `catalog:<Adobe_storeProviderID>`, else `path:<casefolded path>`.
    pub key: String,
    tables: HashSet<String>,
    cancel: Option<Cancel>,
}

impl Source {
    /// `Source(path, cancel)`.
    pub fn open(path: &Path, cancel: Option<&Cancel>) -> Result<Self, Failure> {
        let resolved = resolve(path);
        let db =
            Connection::open_with_flags(&resolved, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
        if let Some(cancel) = cancel {
            let flag = cancel.clone();
            db.progress_handler(1000, Some(move || flag.is_set()))?;
        }
        db.execute_batch("BEGIN")?; // One consistent source snapshot, including committed WAL.
        let tables: HashSet<String> = {
            let mut statement = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
            let names = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
            names
        };
        let mut source = Self {
            key: format!("path:{}", pathx::casefold(&resolved)),
            path: resolved,
            db,
            tables,
            cancel: cancel.cloned(),
        };
        for (table, wanted) in REQUIRED {
            let columns = source.columns(table)?;
            if !source.tables.contains(table) || !wanted.iter().all(|c| columns.contains(*c)) {
                return Err(Failure::value(format!("이 Lightroom 카탈로그 구조는 지원하지 않습니다: {table}")));
            }
        }
        for table in ["Adobe_variablesTable", "Adobe_variables"] {
            let columns = source.columns(table)?;
            if source.tables.contains(table) && columns.contains("name") && columns.contains("value") {
                let value: Option<Value> = source
                    .db
                    .query_row(&format!("SELECT value FROM \"{table}\" WHERE name=?"), ["Adobe_storeProviderID"], |r| {
                        r.get_ref(0).map(|v| cell(v).ok())
                    })
                    .optional()?
                    .flatten();
                if let Some(value) = value.filter(truthy) {
                    source.key = format!("catalog:{}", py_str(&value));
                    break;
                }
            }
        }
        Ok(source)
    }

    /// The column names of `table` (`PRAGMA table_info`).
    pub fn columns(&self, table: &str) -> Result<HashSet<String>, Failure> {
        let mut statement = self.db.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
        let names = statement.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<_>>()?;
        Ok(names)
    }

    /// `counts()`: the row count of every archived table the catalogue has, in [`ARCHIVE_TABLES`] order.
    pub fn counts(&self) -> Result<Vec<(String, i64)>, Failure> {
        let mut result = Vec::new();
        for table in ARCHIVE_TABLES {
            check(self.cancel.as_ref())?;
            if self.tables.contains(table) {
                let n: i64 = self.db.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| r.get(0))?;
                result.push((table.to_owned(), n));
            }
        }
        Ok(result)
    }
}

/// What [`inspect_catalog`] finds.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub path: String,
    pub source_key: String,
    pub counts: Vec<(String, i64)>,
}

impl Inspection {
    /// `info['counts'].get(table, 0)`.
    pub fn count(&self, table: &str) -> i64 {
        self.counts.iter().find(|(t, _)| t == table).map_or(0, |(_, n)| *n)
    }
}

/// Port of `inspect_catalog(path, cancel)`.
pub fn inspect_catalog(path: &Path, cancel: Option<&Cancel>) -> Result<Inspection, Failure> {
    let source = Source::open(path, cancel)?;
    Ok(Inspection { path: source.path.clone(), source_key: source.key.clone(), counts: source.counts()? })
}

fn counts_value(counts: &[(String, i64)]) -> Value {
    Value::Object(counts.iter().map(|(k, n)| (k.clone(), Value::from(*n))).collect())
}

// ---- staging -----------------------------------------------------------------------------------------------

/// Port of `_stage`: every archived row into `lightroom_archive` and the temporary `lr_stage`.
fn stage(
    source: &Source,
    db: &Connection,
    cancel: Option<&Cancel>,
    progress: Report<'_>,
    warnings: &mut Warnings,
) -> Result<Vec<(String, i64)>, Failure> {
    db.execute_batch("PRAGMA temp_store=FILE")?;
    db.execute_batch("CREATE TEMP TABLE lr_stage(kind TEXT,id TEXT,image TEXT,payload TEXT,digest TEXT)")?;
    let counts = source.counts()?;
    let total: i64 = counts.iter().map(|(_, n)| n).sum();
    let mut done: u64 = 0;
    let mut archive = db.prepare("INSERT OR IGNORE INTO lightroom_archive VALUES(?,?,?,?,?)")?;
    let mut staged = db.prepare("INSERT INTO lr_stage VALUES(?,?,?,?,?)")?;
    for (table, _) in &counts {
        let mut statement = source.db.prepare(&format!("SELECT * FROM \"{table}\""))?;
        let names: Vec<String> = statement.column_names().iter().map(|s| (*s).to_owned()).collect();
        let mut rows = statement.query([])?;
        let mut ordinal: i64 = 0;
        while let Some(row) = rows.next()? {
            check(cancel)?;
            let mut data = Map::new();
            for (i, name) in names.iter().enumerate() {
                data.insert(name.clone(), cell(row.get_ref(i)?)?);
            }
            let data = Value::Object(data);
            let key = match data.get("id_local") {
                Some(v) => py_str(v),
                None => ordinal.to_string(),
            };
            let image = match data.get("image") {
                Some(v) => py_str(v),
                None => String::new(),
            };
            let payload = py_json(&data);
            let digest = sha_hex(&payload);
            archive.execute(params![source.key, table, key, digest, payload])?;
            staged.execute(params![table, key, image, payload, digest])?;
            done += 1;
            ordinal += 1;
            if done.is_multiple_of(250) {
                progress(&Progress { stage: "Lightroom 기록 읽는 중", done, total: total as u64 })
                    .map_err(Failure::Error)?;
            }
        }
    }
    db.execute_batch("CREATE INDEX lr_stage_id ON lr_stage(kind,id)")?;
    db.execute_batch("CREATE INDEX lr_stage_image ON lr_stage(image,kind)")?;
    for table in [
        "Adobe_imageDevelopSettings",
        "Adobe_libraryImageDevelopHistoryStep",
        "Adobe_libraryImageDevelopSnapshot",
        "AgLibraryCollectionStackImage",
        "AgLibraryKeywordSynonym",
    ] {
        if let Some((_, n)) = counts.iter().find(|(t, _)| t == table) {
            if *n != 0 {
                warnings.add(table, *n);
            }
        }
    }
    Ok(counts)
}

fn parse(text: &str) -> Result<Value, Failure> {
    serde_json::from_str(text).map_err(|e| Failure::value(e.to_string()))
}

/// `_rows(db, kind, image)`: the staged rows of a kind (of one image), by id and digest.
fn rows(db: &Connection, kind: &str, image: Option<&str>) -> Result<Vec<Value>, Failure> {
    let texts: Vec<String> = match image {
        None => {
            let mut s = db.prepare_cached("SELECT payload FROM lr_stage WHERE kind=? ORDER BY id,digest")?;
            let texts = s.query_map([kind], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            texts
        }
        Some(image) => {
            let mut s =
                db.prepare_cached("SELECT payload FROM lr_stage WHERE kind=? AND image=? ORDER BY id,digest")?;
            let texts = s.query_map([kind, image], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            texts
        }
    };
    texts.iter().map(|t| parse(t)).collect()
}

/// `_record(db, kind, ident)`: the first staged row with that id, or `{}`.
fn record(db: &Connection, kind: &str, ident: &Value) -> Result<Value, Failure> {
    let mut s = db.prepare_cached("SELECT payload FROM lr_stage WHERE kind=? AND id=? LIMIT 1")?;
    let text: Option<String> = s.query_row(params![kind, py_str(ident)], |r| r.get(0)).optional()?;
    match text {
        Some(text) => parse(&text),
        None => Ok(Value::Object(Map::new())),
    }
}

// ---- keywords, paths, metadata ---------------------------------------------------------------------------------

/// Port of `_keyword_paths`: each keyword's `parent|child` path (keywords with separators or in a cycle are
/// left out).
fn keyword_paths(
    db: &Connection,
    warnings: &mut Warnings,
    cancel: Option<&Cancel>,
) -> Result<HashMap<String, String>, Failure> {
    let mut order: Vec<String> = Vec::new();
    let mut keywords: HashMap<String, Value> = HashMap::new();
    for row in rows(db, "AgLibraryKeyword", None)? {
        let id = get_str(&row, "id_local");
        if keywords.insert(id.clone(), row).is_none() {
            order.push(id);
        }
    }
    let mut result = HashMap::new();
    for ident in &order {
        check(cancel)?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut parts: Vec<String> = Vec::new();
        let mut current = ident.clone();
        while let Some(data) = keywords.get(&current) {
            if seen.contains(&current) || seen.len() > 256 {
                warnings.add("keyword_cycle", 1);
                parts.clear();
                break;
            }
            seen.insert(current.clone());
            let name_value = get(data, "name");
            let name =
                grainy_adobe::pyutil::strip(&if truthy(name_value) { py_str(name_value) } else { String::new() })
                    .to_owned();
            if name.contains(',') || name.contains('|') {
                warnings.add("keyword_separator", 1);
                parts.clear();
                break;
            }
            if !name.is_empty() {
                parts.push(name);
            }
            let parent = get(data, "parent");
            if !parent.is_null() && !keywords.contains_key(&py_str(parent)) {
                warnings.add("keyword_parent", 1);
            }
            current = py_str(parent);
        }
        if !parts.is_empty() {
            parts.reverse();
            result.insert(ident.clone(), parts.join("|"));
        }
    }
    Ok(result)
}

/// Port of `_path`: the photo's file path from its file, folder and root folder, or `None` for a relative or
/// foreign root, a path that climbs out of its root or a name with a folder part.
fn photo_path(db: &Connection, row: &Value) -> Result<Option<String>, Failure> {
    let file = record(db, "AgLibraryFile", get(row, "rootFile"))?;
    let folder = record(db, "AgLibraryFolder", get(&file, "folder"))?;
    let root = record(db, "AgLibraryRootFolder", get(&folder, "rootFolder"))?;
    if !truthy(get(&root, "absolutePath")) || !truthy(get(&file, "baseName")) {
        return Ok(None);
    }
    let root_path = path_text(get(&root, "absolutePath"))?;
    if !is_absolute(&root_path) {
        return Ok(None);
    }
    let from_root = get(&folder, "pathFromRoot");
    let relative = if truthy(from_root) { path_text(from_root)? } else { String::new() };
    let mut name = py_str(get(&file, "baseName"));
    let extension_value = get(&file, "extension");
    let extension = if truthy(extension_value) { py_str(extension_value) } else { String::new() }
        .trim_start_matches('.')
        .to_owned();
    if !extension.is_empty() && !pathx::casefold(&name).ends_with(&format!(".{}", pathx::casefold(&extension))) {
        name = format!("{name}.{extension}");
    }
    let (_, relative_parts) = pathx::split(platform(), &relative);
    if is_absolute(&relative)
        || has_drive(&relative)
        || relative_parts.iter().any(|p| p == "..")
        || pathx::name(platform(), &name) != name
    {
        return Ok(None);
    }
    Ok(Some(join(&join(&root_path, &relative), &name)))
}

/// `str(Path(base) / relative)` for a relative path without a drive: on Windows a relative path that starts at
/// the root keeps the base's drive.
fn join(base: &str, relative: &str) -> String {
    let (anchor, parts) = pathx::split(platform(), relative);
    if platform() == Platform::Windows && anchor == "\\" {
        let (base_anchor, _) = pathx::split(platform(), base);
        let drive = base_anchor.trim_end_matches('\\');
        return pathx::norm_str(platform(), &format!("{drive}\\{}", parts.join("\\")));
    }
    pathx::join(platform(), base, relative)
}

/// Port of `_metadata`: the copy name, edit time, capture time, IPTC and EXIF fields Lightroom harvested.
fn metadata(db: &Connection, row: &Value) -> Result<Map<String, Value>, Failure> {
    let mut meta = Map::new();
    let copy_name = get(row, "copyName");
    if truthy(copy_name) {
        meta.insert("_copy_name".into(), Value::String(py_str(copy_name)));
    }
    if let Some(edited) = grainy_adobe::history::adobe_time(get(row, "touchTime")) {
        meta.insert("_last_edit_time".into(), Value::String(edited));
    }
    let stamp = get(row, "captureTime");
    if truthy(stamp) {
        if let Value::String(text) = stamp {
            if let Some(parsed) = grainy_adobe::pyutil::datetime_from_iso(text) {
                meta.insert(
                    "DateTimeOriginal".into(),
                    Value::String(parsed.naive.format("%Y:%m:%d %H:%M:%S").to_string()),
                );
            }
        }
    }
    let image = get_str(row, "id_local");
    for data in rows(db, "AgLibraryIPTC", Some(&image))? {
        for key in ["caption", "copyright"] {
            let value = get(&data, key);
            if !value.is_null() {
                meta.insert(key.into(), value.clone());
            }
        }
    }
    for data in rows(db, "AgHarvestedIptcMetadata", Some(&image))? {
        for key in ["creator", "city", "country", "state", "location", "jobIdentifier"] {
            let mut table = String::from("AgInternedIptc");
            let mut chars = key.chars();
            if let Some(first) = chars.next() {
                table.extend(first.to_uppercase());
                table.push_str(chars.as_str());
            }
            let found = record(db, &table, get(&data, &format!("{key}Ref")))?;
            let value = get(&found, "value");
            if !value.is_null() {
                meta.insert(if key == "jobIdentifier" { "job" } else { key }.into(), value.clone());
            }
        }
    }
    for data in rows(db, "AgHarvestedExifMetadata", Some(&image))? {
        for (source, table, key) in [
            ("cameraModelRef", "AgInternedExifCameraModel", "camera"),
            ("cameraSNRef", "AgInternedExifCameraSN", "serial"),
            ("lensRef", "AgInternedExifLens", "lens"),
        ] {
            let found = record(db, table, get(&data, source))?;
            let value = get(&found, "value");
            if !value.is_null() {
                meta.insert(key.into(), value.clone());
            }
        }
        if let Value::Number(n) = get(&data, "isoSpeedRating") {
            if n.as_f64().is_some_and(|f| f > 0.0) {
                meta.insert("iso".into(), Value::Number(n.clone()));
            }
        }
        if truthy(get(&data, "hasGPS")) {
            for (key, source, limit) in [("latitude", "gpsLatitude", 90.0), ("longitude", "gpsLongitude", 180.0)] {
                if let Value::Number(n) = get(&data, source) {
                    if n.as_f64().is_some_and(|v| (-limit..=limit).contains(&v)) {
                        meta.insert(key.into(), Value::Number(n.clone()));
                    }
                }
            }
        }
    }
    Ok(meta)
}

// ---- photos ------------------------------------------------------------------------------------------------

/// `engine.IMAGE_EXTENSIONS`, `engine.RAW_EXTENSIONS`.
fn suffix(path: &str) -> String {
    pathx::stem_suffix(&pathx::name(platform(), path)).1.to_lowercase()
}

/// The `str` of a Lua value for `orientation`.
fn lua_of(value: &Value) -> Option<grainy_adobe::Lua> {
    use grainy_adobe::Lua;
    Some(match value {
        Value::Null => return None,
        Value::Bool(b) => Lua::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Lua::Int(i128::from(i)),
            None => Lua::Float(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => Lua::Str(s.clone()),
        _ => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn photos(
    source: &Source,
    db: &Connection,
    cancel: Option<&Cancel>,
    progress: Report<'_>,
    warnings: &mut Warnings,
    count: i64,
    convert_edits: bool,
    clock: &mut dyn Clock,
    result: &mut Map<String, Value>,
) -> Result<(), Failure> {
    let payload = py_json(&Value::Object(grainy_core::settings::defaults().0));
    let keywords = keyword_paths(db, warnings, cancel)?;
    db.execute_batch("CREATE TEMP TABLE lr_map(source_id TEXT PRIMARY KEY,photo_id INTEGER,created INTEGER)")?;
    let (mut imported, mut reused, mut skipped, mut copies) = (0i64, 0i64, 0i64, 0i64);
    let (mut converted_photos, mut histories, mut snapshots) = (0i64, 0i64, 0i64);
    // Lightroom's masterImage points to the original; originals must be first.
    let mut cursor = db.prepare(
        "SELECT payload FROM lr_stage WHERE kind='Adobe_images' ORDER BY \
         CASE WHEN json_extract(payload,'$.masterImage') IS NULL THEN 0 ELSE 1 END,id",
    )?;
    let mut rows_iter = cursor.query([])?;
    let mut index: i64 = -1;
    while let Some(packed) = rows_iter.next()? {
        index += 1;
        check(cancel)?;
        let row = parse(&packed.get::<_, String>(0)?)?;
        let ident = py_str(row.get("id_local").ok_or_else(|| Failure::Error("KeyError: 'id_local'".into()))?);
        let path = photo_path(db, &row)?;
        let Some(path) = path.filter(|p| grainy_library::library::IMAGE_EXTENSIONS.contains(&suffix(p).as_str()))
        else {
            skipped += 1;
            warnings.add("unsupported_path_or_format", 1);
            continue;
        };
        let master = get(&row, "masterImage");
        let mut source_master: Option<i64> = None;
        if !master.is_null() {
            source_master = db
                .query_row("SELECT photo_id FROM lr_map WHERE source_id=?", [py_str(master)], |r| r.get(0))
                .optional()?;
            let same = match source_master {
                Some(id) => {
                    let stored: String = db.query_row("SELECT path FROM photos WHERE id=?", [id], |r| r.get(0))?;
                    pathx::casefold(&stored) == pathx::casefold(&path)
                }
                None => false,
            };
            if !same {
                skipped += 1;
                warnings.add("virtual_master", 1);
                continue;
            }
        }
        let mut tags: Vec<String> = rows(db, "AgLibraryKeywordImage", Some(&ident))?
            .iter()
            .filter_map(|k| keywords.get(&py_str(get(k, "tag"))).cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        tags.sort();
        let meta = metadata(db, &row)?;
        let mut state = Map::new();
        state.insert("version".into(), Value::from(1));
        state.insert("row".into(), row.clone());
        state.insert("path".into(), Value::String(path.clone()));
        state.insert("keywords".into(), Value::Array(tags.iter().cloned().map(Value::String).collect()));
        state.insert("metadata".into(), Value::Object(meta.clone()));
        if convert_edits {
            state.insert("develop_translation".into(), Value::from(grainy_adobe::develop::VERSION));
        }
        let mut sha = Sha256::new();
        sha.update(py_json(&Value::Object(state)).as_bytes());
        {
            // Include archived develop/history/snapshots without loading them all.
            let mut items =
                db.prepare_cached("SELECT kind,id,digest FROM lr_stage WHERE image=? ORDER BY kind,id,digest")?;
            let mut found = items.query([&ident])?;
            while let Some(item) = found.next()? {
                let triple =
                    Value::Array(vec![cell(item.get_ref(0)?)?, cell(item.get_ref(1)?)?, cell(item.get_ref(2)?)?]);
                sha.update(py_json(&triple).as_bytes());
            }
        }
        let digest = hex(&sha.finalize());
        let previous: Option<i64> = db
            .query_row(
                "SELECT i.photo_id FROM lightroom_import_items i JOIN photos p ON p.id=i.photo_id
            WHERE source_key=? AND source_id=? AND digest=?",
                params![source.key, ident, digest],
                |r| r.get(0),
            )
            .optional()?;
        let photo_id;
        let created;
        if let Some(id) = previous {
            photo_id = id;
            reused += 1;
            created = 0;
        } else {
            let is_raw = grainy_library::library::RAW_EXTENSIONS.contains(&suffix(&path).as_str());
            let orientation = lua_of(get(&row, "orientation"));
            let mut translated = None;
            let mut photo_payload = payload.clone();
            if convert_edits {
                let current_rows = rows(db, "Adobe_imageDevelopSettings", Some(&ident))?;
                if current_rows.len() == 1 {
                    translated = history::decode_row(&current_rows[0], is_raw, orientation.as_ref(), warnings)?;
                    if let Some(t) = &translated {
                        photo_payload = py_json(&Value::Object(t.settings.0.clone()));
                        converted_photos += 1;
                    }
                } else if !current_rows.is_empty() {
                    warnings.add("develop_ambiguous", 1);
                }
            }
            let existing: Option<(i64, Option<i64>)> = db
                .query_row(
                    "SELECT id,virtual_source FROM photos WHERE path=? COLLATE NOCASE ORDER BY virtual_source IS NOT NULL,id LIMIT 1",
                    [&path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let mut original: Option<i64> = match (source_master, existing) {
                (Some(id), _) => Some(id),
                (None, Some((id, virtual_source))) => Some(virtual_source.filter(|v| *v != 0).unwrap_or(id)),
                (None, None) => None,
            };
            if let Some(id) = original.filter(|id| *id != 0) {
                let source_row: Option<Option<i64>> =
                    db.query_row("SELECT virtual_source FROM photos WHERE id=?", [id], |r| r.get(0)).optional()?;
                if let Some(Some(v)) = source_row.filter(|v| v.is_some_and(|v| v != 0)) {
                    original = Some(v);
                }
            }
            let mut name = pathx::name(platform(), &path);
            let copy_name = get(&row, "copyName");
            if truthy(copy_name) {
                name += &format!(" · {}", py_str(copy_name));
            } else if original.is_some_and(|o| o != 0) {
                name += " · Lightroom 사본";
            }
            let label_value = get(&row, "colorLabels");
            let label = if truthy(label_value) { py_str(label_value) } else { String::new() };
            let label = match pathx::casefold(&label).as_str() {
                "red" => "빨강".to_owned(),
                "yellow" => "노랑".to_owned(),
                "green" => "초록".to_owned(),
                "blue" => "파랑".to_owned(),
                "purple" => "보라".to_owned(),
                _ => label,
            };
            db.execute(
                "INSERT INTO photos(path,name,rating,flag,keywords,edits,metadata,imported,label,virtual_source,extras)
                VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    path,
                    name,
                    py_number(get(&row, "rating"), 0, 5),
                    py_number(get(&row, "pick"), -1, 1),
                    tags.join(", "),
                    photo_payload,
                    "{}",
                    isoformat(clock.utc()),
                    label,
                    original,
                    py_json(&Value::Object(meta)),
                ],
            )?;
            photo_id = db.last_insert_rowid();
            db.execute(
                "INSERT OR REPLACE INTO lightroom_import_items VALUES(?,?,?,?)",
                params![source.key, ident, digest, photo_id],
            )?;
            db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", [pathx::parent(platform(), &path)])?;
            imported += 1;
            copies += i64::from(original.is_some());
            created = 1;
            if convert_edits {
                let current = translated.as_ref().map(|t| &t.settings);
                let (h, s) = history::transfer(
                    db,
                    &ident,
                    photo_id,
                    current,
                    is_raw,
                    orientation.as_ref(),
                    warnings,
                    clock,
                    &|| check(cancel),
                )?;
                histories += h;
                snapshots += s;
            }
        }
        db.execute("INSERT INTO lr_map VALUES(?,?,?)", params![ident, photo_id, created])?;
        if index % 100 == 0 {
            progress(&Progress {
                stage: "사진 분류 가져오는 중", done: index as u64 + 1, total: count.max(0) as u64
            })
            .map_err(Failure::Error)?;
        }
    }
    for (key, value) in [
        ("imported", imported),
        ("reused", reused),
        ("skipped", skipped),
        ("copies", copies),
        ("converted_photos", converted_photos),
        ("converted_history", histories),
        ("converted_snapshots", snapshots),
    ] {
        result.insert(key.into(), Value::from(value));
    }
    Ok(())
}

// ---- collections and stacks --------------------------------------------------------------------------------

fn collections(
    source: &Source,
    db: &Connection,
    cancel: Option<&Cancel>,
    warnings: &mut Warnings,
    clock: &mut dyn Clock,
    result: &mut Map<String, Value>,
) -> Result<(), Failure> {
    let mut order: Vec<String> = Vec::new();
    let mut all: HashMap<String, Value> = HashMap::new();
    for row in rows(db, "AgLibraryCollection", None)? {
        if truthy(get(&row, "systemOnly")) {
            continue;
        }
        let id = get_str(&row, "id_local");
        if all.insert(id.clone(), row).is_none() {
            order.push(id);
        }
    }
    let mut sha = Sha256::new();
    sha.update(format!("smart-translation:{}", grainy_adobe::smart::VERSION).as_bytes());
    for kind in ["AgLibraryCollection", "AgLibraryCollectionImage", "AgLibraryCollectionContent"] {
        let mut s = db.prepare("SELECT id,digest FROM lr_stage WHERE kind=? ORDER BY id,digest")?;
        let mut found = s.query([kind])?;
        while let Some(row) = found.next()? {
            check(cancel)?;
            sha.update(py_json(&Value::Array(vec![cell(row.get_ref(0)?)?, cell(row.get_ref(1)?)?])).as_bytes());
        }
    }
    {
        let mut s = db.prepare("SELECT source_id,photo_id FROM lr_map ORDER BY source_id")?;
        let mut found = s.query([])?;
        while let Some(row) = found.next()? {
            sha.update(py_json(&Value::Array(vec![cell(row.get_ref(0)?)?, cell(row.get_ref(1)?)?])).as_bytes());
        }
    }
    let digest = hex(&sha.finalize());
    let old: Option<i64> = db
        .query_row(
            "SELECT i.collection_id FROM lightroom_import_sets i JOIN collections c ON c.id=i.collection_id
        WHERE source_key=? AND digest=?",
            params![source.key, digest],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        result.insert("collections".into(), Value::from(0));
        result.insert("collection_root".into(), Value::from(old));
        result.insert("converted_smart".into(), Value::from(0));
        return Ok(());
    }
    let stem = pathx::stem_suffix(&pathx::name(platform(), &source.path)).0;
    let stamp = clock.local().format("%m/%d %H:%M").to_string();
    db.execute("INSERT INTO collections(name) VALUES(?)", [format!("Lightroom · {stem} · {stamp}")])?;
    let root = db.last_insert_rowid();
    db.execute("INSERT INTO collection_members SELECT ?,photo_id FROM lr_map", [root])?;
    let mut mapped: HashMap<String, i64> = HashMap::new();
    let mut smart_content: HashMap<String, Vec<Value>> = HashMap::new();
    for row in rows(db, "AgLibraryCollectionContent", None)? {
        if get(&row, "owningModule") == "ag.library.smart_collection" {
            smart_content.entry(get_str(&row, "collection")).or_default().push(get(&row, "content").clone());
        }
    }
    let mut converted = 0i64;
    for key in &order {
        check(cancel)?;
        let row = &all[key];
        let name_value = get(row, "name");
        let mut name = if truthy(name_value) { py_str(name_value) } else { "이름 없는 컬렉션".to_owned() };
        let mut rules: Option<String> = None;
        let creation = get(row, "creationId");
        let creation = if truthy(creation) { py_str(creation) } else { String::new() };
        if creation.to_lowercase().contains("smart") {
            let texts = smart_content.get(key).cloned().unwrap_or_default();
            let translated = if texts.len() != 1 {
                Err(grainy_adobe::Error::value("스마트 규칙 원문이 없거나 여러 개입니다."))
            } else {
                match &texts[0] {
                    Value::String(text) => grainy_adobe::smart::translate(text),
                    _ => Err(grainy_adobe::Error::data("저장된 데이터의 크기 또는 문자 형식을 확인할 수 없습니다.")),
                }
            };
            match translated {
                Ok(map) => {
                    rules = Some(py_json(&Value::Object(map)));
                    converted += 1;
                }
                Err(e)
                    if matches!(
                        e.kind,
                        grainy_adobe::ErrorKind::Data
                            | grainy_adobe::ErrorKind::Value
                            | grainy_adobe::ErrorKind::Type
                            | grainy_adobe::ErrorKind::Overflow
                    ) =>
                {
                    name += " · 스마트 규칙 보관";
                    warnings.add("smart_collection", 1);
                    warnings.add(&format!("smart_reason:{}", e.message), 1);
                }
                Err(e) => return Err(Failure::Error(format!("{}: {}", e.kind.python_name(), e.message))),
            }
        }
        db.execute("INSERT INTO collections(name,parent_id,rules) VALUES(?,?,?)", params![name, root, rules])?;
        mapped.insert(key.clone(), db.last_insert_rowid());
    }
    for key in &order {
        check(cancel)?;
        let mut seen: HashSet<String> = HashSet::from([key.clone()]);
        let mut parent = get_str(&all[key], "parent");
        let mut current = parent.clone();
        while all.contains_key(&current) && !seen.contains(&current) {
            seen.insert(current.clone());
            current = get_str(&all[&current], "parent");
        }
        if seen.contains(&current) {
            warnings.add("collection_cycle", 1);
            parent = String::new();
        }
        if let Some(target) = mapped.get(&parent) {
            db.execute("UPDATE collections SET parent_id=? WHERE id=?", params![target, mapped[key]])?;
        }
    }
    for row in rows(db, "AgLibraryCollectionImage", None)? {
        check(cancel)?;
        if let Some(target) = mapped.get(&get_str(&row, "collection")) {
            let member: Option<i64> = db
                .query_row("SELECT photo_id FROM lr_map WHERE source_id=?", [get_str(&row, "image")], |r| r.get(0))
                .optional()?;
            match member {
                Some(member) => {
                    db.execute("INSERT OR IGNORE INTO collection_members VALUES(?,?)", params![target, member])?;
                }
                None => warnings.add("collection_member", 1),
            }
        }
    }
    db.execute("INSERT OR REPLACE INTO lightroom_import_sets VALUES(?,?,?)", params![source.key, digest, root])?;
    result.insert("collections".into(), Value::from(mapped.len() as i64 + 1));
    result.insert("collection_root".into(), Value::from(root));
    result.insert("converted_smart".into(), Value::from(converted));
    Ok(())
}

fn stacks(db: &Connection, cancel: Option<&Cancel>, warnings: &mut Warnings) -> Result<(), Failure> {
    db.execute_batch("CREATE TEMP TABLE lr_stacks(stack TEXT,photo INTEGER,created INTEGER)")?;
    for row in rows(db, "AgLibraryFolderStackImage", None)? {
        check(cancel)?;
        if get(&row, "stack").is_null() {
            warnings.add("stack_reference", 1);
            continue;
        }
        db.execute(
            "INSERT INTO lr_stacks SELECT ?,photo_id,created FROM lr_map WHERE source_id=?",
            params![get_str(&row, "stack"), get_str(&row, "image")],
        )?;
    }
    let groups: Vec<(Value, i64, i64)> = {
        let mut s = db.prepare("SELECT stack,COUNT(*),SUM(created) FROM lr_stacks GROUP BY stack")?;
        let found = s
            .query_map([], |r| Ok((cell(r.get_ref(0)?).unwrap_or(Value::Null), r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        found
    };
    for (stack, count, created) in groups {
        check(cancel)?;
        if count != created {
            if created != 0 {
                warnings.add("stack_existing", 1);
            }
            continue;
        }
        if count > 1 {
            let stack = py_str(&stack);
            let group: Option<i64> =
                db.query_row("SELECT MIN(photo) FROM lr_stacks WHERE stack=?", [&stack], |r| r.get(0))?;
            db.execute(
                "UPDATE photos SET stack_id=? WHERE id IN (SELECT photo FROM lr_stacks WHERE stack=?)",
                params![group, stack],
            )?;
        }
    }
    Ok(())
}

// ---- the import --------------------------------------------------------------------------------------------

/// The result dict of `import_catalog` (keys in the oracle's order; `warnings` in the order they were first
/// counted).
pub type ImportResult = Map<String, Value>;

/// Port of `import_catalog(path, directory, cancel, progress, *, convert_edits)`: the worker-only entry point.
/// Opens its own connections (the source read-only, the library's `catalog.sqlite` read-write); `Ok` with
/// `cancelled: true` when the cancel flag stopped it (every record of this run rolled back).
pub fn import_catalog(
    path: &Path,
    directory: &Path,
    cancel: Option<&Cancel>,
    progress: Report<'_>,
    convert_edits: bool,
    clock: &mut dyn Clock,
) -> Result<ImportResult, Failure> {
    let directory = PathBuf::from(resolve(directory));
    let target = directory.join("catalog.sqlite");
    let source_path = resolve(path);
    if pathx::same(platform(), &source_path, &pathx::text(&target)) {
        return Err(Failure::value("Luma 카탈로그 자신은 가져올 수 없습니다."));
    }
    check(cancel)?;
    let mut warnings = Warnings::default();
    let run_id = clock.run_id();
    let backup = directory.join("backups").join(format!("before-lightroom-{run_id}.sqlite"));
    let pending = backup.with_extension("pending");
    let mut result = Map::new();
    result.insert("cancelled".into(), Value::Bool(false));
    result.insert("source_path".into(), Value::String(source_path));
    result.insert("backup".into(), Value::String(String::new()));
    let source = Source::open(path, cancel)?;
    let db = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    db.busy_timeout(Duration::from_secs(5))?;
    let outcome = run(
        &source,
        &db,
        cancel,
        progress,
        convert_edits,
        clock,
        &mut warnings,
        &mut result,
        &backup,
        &pending,
        &run_id,
    );
    let outcome = match outcome {
        Ok(()) => db.execute_batch("COMMIT").map_err(Failure::from),
        Err(e) => Err(e),
    };
    let answer = match outcome {
        Ok(()) => Ok(result),
        Err(failure) => {
            if !db.is_autocommit() {
                let _ = db.execute_batch("ROLLBACK");
            }
            if failure.cancel_like() && cancel.is_some_and(Cancel::is_set) {
                result.insert("cancelled".into(), Value::Bool(true));
                for key in ["imported", "reused", "skipped", "collections"] {
                    result.insert(key.into(), Value::from(0));
                }
                Ok(result)
            } else {
                Err(failure)
            }
        }
    };
    // An interrupted SQLite backup must never appear as a restorable .sqlite backup. Only this run's uniquely
    // named partial is removed.
    if pending.exists() {
        let _ = std::fs::remove_file(&pending);
    }
    answer
}

#[allow(clippy::too_many_arguments)]
fn run(
    source: &Source,
    db: &Connection,
    cancel: Option<&Cancel>,
    progress: Report<'_>,
    convert_edits: bool,
    clock: &mut dyn Clock,
    warnings: &mut Warnings,
    result: &mut Map<String, Value>,
    backup: &Path,
    pending: &Path,
    run_id: &str,
) -> Result<(), Failure> {
    progress(&Progress { stage: "안전 백업 만드는 중", done: 0, total: 0 }).map_err(Failure::Error)?;
    if let Some(folder) = backup.parent() {
        std::fs::create_dir_all(folder).map_err(|e| Failure::Error(format!("OSError: {e}")))?;
    }
    {
        let mut copy = Connection::open(pending)?;
        let job = rusqlite::backup::Backup::new(db, &mut copy)?;
        loop {
            let step = job.step(256)?;
            check(cancel)?;
            match step {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => {}
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
    check(cancel)?;
    if backup.exists() {
        return Err(Failure::Error(format!("FileExistsError: {}", pathx::text(backup))));
    }
    std::fs::rename(pending, backup).map_err(|e| Failure::Error(format!("OSError: {e}")))?;
    result.insert("backup".into(), Value::String(pathx::text(backup)));
    db.execute_batch("BEGIN IMMEDIATE")?;
    for statement in SCHEMA {
        db.execute_batch(statement)?;
    }
    let counts = stage(source, db, cancel, progress, warnings)?;
    if convert_edits {
        for kind in
            ["Adobe_imageDevelopSettings", "Adobe_libraryImageDevelopHistoryStep", "Adobe_libraryImageDevelopSnapshot"]
        {
            warnings.remove(kind);
        }
    }
    result.insert("convert_edits".into(), Value::Bool(convert_edits));
    let images = counts.iter().find(|(t, _)| t == "Adobe_images").map_or(0, |(_, n)| *n);
    photos(source, db, cancel, progress, warnings, images, convert_edits, clock, result)?;
    progress(&Progress { stage: "컬렉션과 스택 정리 중", done: 0, total: 0 }).map_err(Failure::Error)?;
    collections(source, db, cancel, warnings, clock, result)?;
    stacks(db, cancel, warnings)?;
    check(cancel)?;
    result.insert("source_key".into(), Value::String(source.key.clone()));
    result.insert("source_counts".into(), counts_value(&counts));
    result.insert("warnings".into(), warnings.to_value());
    result.insert("run_id".into(), Value::String(run_id.to_owned()));
    db.execute(
        "INSERT INTO lightroom_import_runs VALUES(?,?,?,?,?)",
        params![run_id, isoformat(clock.utc()), source.path, source.key, py_json(&Value::Object(result.clone()))],
    )?;
    Ok(())
}
