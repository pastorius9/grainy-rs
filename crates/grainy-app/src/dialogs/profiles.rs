//! The camera profile browser (카메라 프로파일): DCP files found in the profile folders, filtered to the open RAW
//! photo's camera, with a name search, favourites and extra folders; the chosen profile is pinned into the photo's
//! settings (`dcp_profile`, the file's bytes: the catalogue stores them once in `profile_assets`).
//!
//! Ports luma/profile_library.py (`profile_roots`, `scan` with its disposable index `cache/dcp-index.sqlite`,
//! `compatible`, `load_selected`) and luma/profile_dialog.py `ProfileDialog` (`refresh` as a job, `populate`,
//! `selection_changed`, `toggle_favorite`, `add_folder`, `remove_folder`, `choose`). The preferences are the
//! oracle's: `dcp_favorites` (sorted digests) and `dcp_folders` (extra folders).
//!
//! The browser's state is [`ProfileBrowser`] (no window needed); [`ProfileDialog`] draws it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use eframe::egui;
use grainy_adobe::pyutil::casefold;
use grainy_catalog::rusqlite::{self, OptionalExtension};
use grainy_raw::dcp::{self, Tag};
use serde_json::{json, Value};

use super::Dialog;
use crate::context::AppContext;
use crate::jobs::JobId;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// Port of `profile_library.INDEX_VERSION` (rechecks cached rejections of triple/custom illuminants).
pub const INDEX_VERSION: u32 = 3;

/// The scan's file limit (`scan(..., limit=20000)`).
pub const LIMIT: usize = 20000;

/// The tags read for a profile's identity even when its transform is not supported: name, camera, copyright.
const IDENTITY: [u16; 3] = [50936, 50708, 50942];

// ---- profile_library --------------------------------------------------------------------------------------------

/// One profile of a scan (`summary` in the oracle, plus `paths`: every file with the same bytes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfileRow {
    pub path: String,
    pub name: String,
    pub camera: String,
    pub copyright: String,
    pub sha256: String,
    pub error: String,
    pub paths: Vec<String>,
}

impl ProfileRow {
    /// The summary as the index stores it (`json.dumps(summary)`: the oracle's keys and order).
    fn summary(&self) -> Value {
        json!({
            "path": self.path, "name": self.name, "camera": self.camera, "copyright": self.copyright,
            "sha256": self.sha256, "error": self.error,
        })
    }

    fn from_summary(value: &Value) -> Option<Self> {
        let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            path: text("path")?,
            name: text("name")?,
            camera: text("camera")?,
            copyright: text("copyright")?,
            sha256: text("sha256")?,
            error: text("error")?,
            paths: Vec::new(),
        })
    }

    /// The row as JSON (the summary and its `paths`), for tests and goldens.
    pub fn to_json(&self) -> Value {
        let mut value = self.summary();
        value["paths"] = json!(self.paths);
        value
    }
}

/// What [`scan`] found (`dict(profiles=..., files=..., parsed=..., errors=..., cancelled=..., truncated=...)`).
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub profiles: Vec<ProfileRow>,
    pub files: usize,
    pub parsed: usize,
    pub errors: Vec<String>,
    pub cancelled: bool,
    pub truncated: bool,
}

/// `Path.resolve()`: absolute, with links and `..` resolved where the path exists, without `\\?\`.
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match std::fs::canonicalize(&absolute) {
        Ok(real) => {
            let text = real.to_string_lossy().into_owned();
            match text.strip_prefix(r"\\?\UNC\") {
                Some(rest) => PathBuf::from(format!(r"\\{rest}")),
                None => PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)),
            }
        }
        Err(_) => absolute,
    }
}

/// `os.path.normcase` (Windows: lower case, backslashes).
fn normcase(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase().replace('/', "\\")
    } else {
        path.to_owned()
    }
}

/// Port of `profile_library.profile_roots`: the library's `camera-profiles`, Adobe's shared and per-user profile
/// folders (`%PROGRAMDATA%`, `%APPDATA%`, and on macOS Adobe's folders), Grainy's own `CameraProfiles` folder, then
/// `extra`; resolved, without repeats. `env` reads an environment variable. Grainy's own folder is `own`: the
/// oracle's `%LOCALAPPDATA%\Luma\CameraProfiles` is the Python Grainy's data folder, this app's is its own data
/// folder's `CameraProfiles`.
pub fn profile_roots(
    directory: &Path,
    extra: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    own: &Path,
) -> Vec<String> {
    let mut roots: Vec<PathBuf> = vec![directory.join("camera-profiles")];
    for variable in ["PROGRAMDATA", "APPDATA"] {
        if let Some(value) = env(variable).filter(|v| !v.is_empty()) {
            roots.push(Path::new(&value).join("Adobe").join("CameraRaw").join("CameraProfiles"));
        }
    }
    if cfg!(target_os = "macos") {
        let support =
            Path::new("Library").join("Application Support").join("Adobe").join("CameraRaw").join("CameraProfiles");
        roots.push(Path::new("/").join(&support));
        if let Some(home) = env("HOME") {
            roots.push(Path::new(&home).join(&support));
        }
    }
    roots.push(own.to_path_buf());
    roots.extend(extra.iter().map(PathBuf::from));
    let mut seen = BTreeSet::new();
    roots.iter().map(|p| resolve(p).to_string_lossy().into_owned()).filter(|p| seen.insert(p.clone())).collect()
}

/// The roots for the open library: [`profile_roots`] with this process's environment, or the fixed ones of
/// [`ProfileEnvironment`].
pub fn roots_for(cx: &mut AppContext) -> (Vec<String>, PathBuf) {
    let library = cx.doc.library_folder().unwrap_or_else(|| crate::paths::Locations::current().data_folder());
    let extra = folders(cx);
    let environment = cx.state::<ProfileEnvironment>();
    let own = environment
        .own
        .clone()
        .unwrap_or_else(|| crate::paths::Locations::current().data_folder().join("CameraProfiles"));
    let roots = match &environment.env {
        Some(map) => {
            let map = map.clone();
            profile_roots(&library, &extra, &move |k| map.get(k).cloned(), &own)
        }
        None => profile_roots(&library, &extra, &|k| std::env::var(k).ok(), &own),
    };
    (roots, library.join("cache").join("dcp-index.sqlite"))
}

/// Where the profile folders come from; tests replace the environment so that no folder of the computer is read.
#[derive(Default)]
pub struct ProfileEnvironment {
    /// The environment variables to use instead of the process's.
    pub env: Option<BTreeMap<String, String>>,
    /// Grainy's own profile folder instead of the data folder's `CameraProfiles`.
    pub own: Option<PathBuf>,
    /// A folder written under another name in the window's texts (tests: a fixed name for a temporary folder, so
    /// that a picture of the window is the same in every run).
    pub shown_as: Option<(String, String)>,
}

/// `text` with the folder of [`ProfileEnvironment::shown_as`] renamed.
fn shown(cx: &mut AppContext, text: &str) -> String {
    match &cx.state::<ProfileEnvironment>().shown_as {
        Some((folder, name)) => text.replace(folder.as_str(), name),
        None => text.to_owned(),
    }
}

/// The extra folders (`catalog.preference('dcp_folders', [])`).
fn folders(cx: &AppContext) -> Vec<String> {
    strings(cx.doc.preference("dcp_folders"))
}

fn strings(value: Option<Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array().cloned())
        .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

fn tag_text(tag: &Tag) -> String {
    match tag {
        Tag::Text(text) => text.clone(),
        Tag::Numbers(values) => format!("{values:?}"),
        Tag::Bytes(bytes) => format!("{bytes:?}"),
    }
}

/// The modification time in nanoseconds (`st_mtime_ns`).
fn mtime_ns(metadata: &std::fs::Metadata) -> i128 {
    match metadata.modified() {
        Ok(time) => match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(after) => after.as_nanos() as i128,
            Err(before) => -(before.duration().as_nanos() as i128),
        },
        Err(_) => 0,
    }
}

/// A folder's entries as `os.walk` sorts them: sub-folders (links and junctions are left out later) and files.
fn list(folder: &Path, errors: &mut Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    match std::fs::read_dir(folder) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() {
                    dirs.push(name);
                } else {
                    files.push(name);
                }
            }
        }
        Err(error) => errors.push(format!("{error}: '{}'", folder.display())),
    }
    (dirs, files)
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// What the walk of one root decided.
enum Walk {
    Go,
    Stop,
}

struct Scanner<'a> {
    db: &'a rusqlite::Transaction<'a>,
    cancel: &'a dyn Fn() -> bool,
    limit: usize,
    rows: Vec<ProfileRow>,
    errors: Vec<String>,
    seen: Vec<String>,
    seen_set: std::collections::HashSet<String>,
    parsed: usize,
    cancelled: bool,
    truncated: bool,
}

impl Scanner<'_> {
    /// `os.walk(root, followlinks=False)` top-down, one folder then its sub-folders in order.
    fn walk(&mut self, folder: &Path) -> Result<Walk, String> {
        let (dirs, mut files) = list(folder, &mut self.errors);
        let dirs: Vec<String> = dirs.into_iter().filter(|n| !is_link(&folder.join(n))).collect();
        if (self.cancel)() {
            self.cancelled = true;
            return Ok(Walk::Stop);
        }
        files.sort();
        for name in files {
            if !name.to_lowercase().ends_with(".dcp") {
                continue;
            }
            let path = resolve(&folder.join(&name));
            let shown = path.to_string_lossy().into_owned();
            let key = normcase(&shown);
            if self.seen_set.contains(&key) {
                continue;
            }
            if self.seen.len() >= self.limit {
                self.truncated = true;
                break;
            }
            self.seen_set.insert(key.clone());
            self.seen.push(key.clone());
            if (self.cancel)() {
                self.cancelled = true;
                break;
            }
            let row = self.profile(&path, &shown, &key)?;
            self.rows.push(row);
        }
        if self.cancelled || self.truncated {
            return Ok(Walk::Stop);
        }
        for name in dirs {
            if let Walk::Stop = self.walk(&folder.join(name))? {
                return Ok(Walk::Stop);
            }
        }
        Ok(Walk::Go)
    }

    /// One file: the index's summary when its stamp is current, else parsed again and stored.
    fn profile(&mut self, path: &Path, shown: &str, key: &str) -> Result<ProfileRow, String> {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut summary = ProfileRow { path: shown.to_owned(), name: stem.clone(), ..ProfileRow::default() };
        let stat = match std::fs::metadata(path) {
            Ok(stat) => stat,
            Err(error) => {
                summary.error = error.to_string();
                return Ok(summary);
            }
        };
        let stamp = format!("{INDEX_VERSION}:{}:{}", mtime_ns(&stat), stat.len());
        let old: Option<String> = self
            .db
            .query_row("SELECT summary FROM profiles WHERE path=? AND stamp=?", [key, stamp.as_str()], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(cached) = old.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok()) {
            if let Some(row) = ProfileRow::from_summary(&cached) {
                return Ok(row);
            }
        }
        self.parsed += 1;
        let outcome: Result<(), String> = (|| {
            if stat.len() > dcp::MAX_PROFILE_BYTES as u64 {
                return Err("DCP가 4MB를 초과합니다.".to_owned());
            }
            let data = std::fs::read(path).map_err(|e| e.to_string())?;
            summary.sha256 = grainy_library::fsops::file_digest(path).map_err(|e| e.to_string())?;
            let tags = dcp::read_tags(&data, Some(&IDENTITY)).map_err(|e| e.to_string())?;
            summary.name = tags.get(&50936).map(tag_text).unwrap_or(stem.clone());
            summary.camera = tags.get(&50708).map(tag_text).unwrap_or_default();
            summary.copyright = tags.get(&50942).map(tag_text).unwrap_or_default();
            dcp::parse(&data).map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(error) = outcome {
            summary.error = error;
        }
        let text = grainy_catalog::json::dumps(&summary.summary(), true, false, false);
        self.db
            .execute("INSERT OR REPLACE INTO profiles VALUES(?,?,?)", [key, stamp.as_str(), text.as_str()])
            .map_err(|e| e.to_string())?;
        Ok(summary)
    }
}

/// Port of `profile_library.scan`: the `.dcp` files below `roots` (links not followed), parsed when their size or
/// time changed since the index `cache` saw them (only names, digests and errors are kept there, never the bytes).
/// A cancelled or capped scan keeps every index entry; profiles with the same bytes become one row with all their
/// paths; rows are sorted by camera, name and path.
pub fn scan(roots: &[String], cache: &Path, cancel: &dyn Fn() -> bool, limit: usize) -> Result<ScanResult, String> {
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut connection = rusqlite::Connection::open(cache).map_err(|e| e.to_string())?;
    connection.busy_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
    let db = connection.transaction().map_err(|e| e.to_string())?;
    db.execute("CREATE TABLE IF NOT EXISTS profiles(path TEXT PRIMARY KEY,stamp TEXT,summary TEXT)", [])
        .map_err(|e| e.to_string())?;
    let mut scanner = Scanner {
        db: &db,
        cancel,
        limit,
        rows: Vec::new(),
        errors: Vec::new(),
        seen: Vec::new(),
        seen_set: Default::default(),
        parsed: 0,
        cancelled: false,
        truncated: false,
    };
    for root in roots {
        if cancel() {
            scanner.cancelled = true;
            break;
        }
        if !Path::new(root).is_dir() {
            continue;
        }
        if let Walk::Stop = scanner.walk(Path::new(root))? {
            break;
        }
    }
    let Scanner { rows, errors, seen, parsed, cancelled, truncated, .. } = scanner;
    if !cancelled && !truncated {
        let seen: std::collections::HashSet<&String> = seen.iter().collect();
        let stale: Vec<String> = {
            let mut statement = db.prepare("SELECT path FROM profiles").map_err(|e| e.to_string())?;
            let paths = statement.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            paths.filter_map(Result::ok).filter(|p| !seen.contains(p)).collect()
        };
        for path in stale {
            db.execute("DELETE FROM profiles WHERE path=?", [path]).map_err(|e| e.to_string())?;
        }
    }
    db.commit().map_err(|e| e.to_string())?;
    let files = seen.len();
    // Identical profiles in overlapping vendor and user folders appear once.
    let mut unique: Vec<ProfileRow> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for row in rows {
        let key = if row.sha256.is_empty() { row.path.clone() } else { row.sha256.clone() };
        match at.get(&key) {
            Some(&i) => unique[i].paths.push(row.path.clone()),
            None => {
                at.insert(key, unique.len());
                let mut row = row;
                row.paths = vec![row.path.clone()];
                unique.push(row);
            }
        }
    }
    unique.sort_by(|a, b| {
        (casefold(&a.camera), casefold(&a.name), &a.path).cmp(&(casefold(&b.camera), casefold(&b.name), &b.path))
    });
    Ok(ScanResult { profiles: unique, files, parsed, errors, cancelled, truncated })
}

/// `dcp.camera_matches` on a row (its camera only).
fn row_matches(row: &ProfileRow, metadata: &Value) -> bool {
    let profile = dcp::Profile { camera: row.camera.clone(), ..dcp::Profile::default() };
    dcp::camera_matches(&profile, metadata)
}

/// Port of `profile_library.compatible`: the rows for `metadata`'s camera whose name and camera hold every word of
/// `search` (ignoring case), only favourites when asked.
pub fn compatible(
    rows: &[ProfileRow],
    metadata: &Value,
    search: &str,
    favorites: &BTreeSet<String>,
    only_favorites: bool,
) -> Vec<ProfileRow> {
    let words: Vec<String> = casefold(search).split_whitespace().map(str::to_owned).collect();
    rows.iter()
        .filter(|r| {
            let haystack = casefold(&format!("{} {}", r.name, r.camera));
            row_matches(r, metadata)
                && (!only_favorites || favorites.contains(&r.sha256))
                && words.iter().all(|w| haystack.contains(w.as_str()))
        })
        .cloned()
        .collect()
}

/// Port of `profile_library.load_selected`: the chosen row read again at application time (a file changed or
/// gone since the scan is refused, the next copy is tried).
pub fn load_selected(row: &ProfileRow, metadata: &Value) -> Result<Value, String> {
    let paths = if row.paths.is_empty() { vec![row.path.clone()] } else { row.paths.clone() };
    let mut error: Option<String> = None;
    for path in paths {
        let attempt = dcp::load_profile(&path).map_err(|e| e.to_string()).and_then(|record| {
            if record["sha256"].as_str() != Some(row.sha256.as_str()) {
                return Err("프로파일이 변경되었습니다. 목록을 다시 검색하세요.".to_owned());
            }
            let profile = dcp::compiled(&record).map_err(|e| e.to_string())?;
            if !dcp::camera_matches(&profile, metadata) {
                return Err("현재 카메라와 다른 프로파일입니다.".to_owned());
            }
            Ok(record)
        });
        match attempt {
            Ok(record) => return Ok(record),
            Err(e) => error = Some(e),
        }
    }
    Err(format!(
        "프로파일을 읽을 수 없습니다. {}",
        error.filter(|e| !e.is_empty()).unwrap_or_else(|| "목록을 다시 검색하세요.".to_owned())
    ))
}

/// Python's `f"{n:,}"`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---- the dialog's state -----------------------------------------------------------------------------------------

/// The profile browser without its window (`ProfileDialog`'s attributes and methods).
#[derive(Debug, Clone)]
pub struct ProfileBrowser {
    /// The photo's metadata (`info`).
    pub metadata: Value,
    /// The last scan's rows.
    pub rows: Vec<ProfileRow>,
    /// Favourite digests (`dcp_favorites`).
    pub favorites: BTreeSet<String>,
    /// The search field and 즐겨찾기만.
    pub search: String,
    pub favorites_only: bool,
    /// The list as shown and its current row.
    pub shown: Vec<ProfileRow>,
    pub current: Option<usize>,
    /// The detail and status texts.
    pub detail: String,
    pub status: String,
    /// The status's tooltip (folder read errors) and the refresh button's (the roots).
    pub status_tip: String,
    pub roots_tip: String,
    pub scanning: bool,
    /// Whether the selection was looked at once (the oracle's 즐겨찾기 전환 is enabled until then).
    touched: bool,
}

impl ProfileBrowser {
    /// The browser for a photo's metadata with the stored favourites.
    pub fn new(metadata: Value, favorites: &[String]) -> Self {
        Self {
            metadata,
            rows: Vec::new(),
            favorites: favorites.iter().cloned().collect(),
            search: String::new(),
            favorites_only: false,
            shown: Vec::new(),
            current: None,
            detail: tr("설치된 프로파일을 찾고 있습니다…"),
            status: String::new(),
            status_tip: String::new(),
            roots_tip: String::new(),
            scanning: false,
            touched: false,
        }
    }

    /// The window's first line (`'카메라 프로파일 · '+str(metadata.get('camera','모델 정보 없음'))`).
    pub fn heading(&self) -> String {
        let camera = match self.metadata.get("camera") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => crate::commands::metadata::python_str(other),
            None => tr("모델 정보 없음"),
        };
        tr("카메라 프로파일 · ") + &camera
    }

    /// The start of `refresh`: `false` when a scan runs already.
    pub fn begin_scan(&mut self, roots: &[String]) -> bool {
        if self.scanning {
            return false;
        }
        self.scanning = true;
        self.status = tr("설치된 프로파일을 검색하는 중…");
        self.roots_tip = roots.join("\n");
        true
    }

    /// `refresh`'s `ready`.
    pub fn ready(&mut self, result: &ScanResult) {
        self.scanning = false;
        self.rows = result.profiles.clone();
        let problems = self.rows.iter().filter(|r| !r.error.is_empty()).count();
        let mut status = tr_with(
            "{0}개 파일 확인 · {1}개 갱신 · 미지원/읽기 오류 {2}개",
            &[&grouped(result.files), &grouped(result.parsed), &grouped(problems)],
        );
        if result.truncated {
            status += &tr(" · 검색 상한에 도달했습니다. 폴더 범위를 줄이세요.");
        }
        if !result.errors.is_empty() {
            status += &tr_with(" · 폴더 읽기 오류 {0}개", &[&result.errors.len().to_string()]);
        }
        self.status = status;
        self.status_tip = result.errors.join("\n");
        self.populate();
    }

    /// `refresh`'s `failed`.
    pub fn failed(&mut self, error: &str) {
        self.scanning = false;
        self.status = error.to_owned();
    }

    /// The text of a list row.
    pub fn item_text(&self, row: &ProfileRow) -> String {
        format!(
            "{}{}{}",
            if self.favorites.contains(&row.sha256) { "★ " } else { "" },
            row.name,
            if row.error.is_empty() { String::new() } else { tr(" · 지원 안 됨") }
        )
    }

    /// Port of `populate`: the compatible rows, favourites first then by name; the selected profile stays
    /// selected, else the first row is.
    pub fn populate(&mut self) {
        let digest = self.selected().map(|r| r.sha256.clone());
        let mut rows = compatible(&self.rows, &self.metadata, &self.search, &self.favorites, self.favorites_only);
        rows.sort_by_cached_key(|r| (!self.favorites.contains(&r.sha256), casefold(&r.name)));
        self.current = digest.and_then(|d| rows.iter().rposition(|r| r.sha256 == d));
        self.shown = rows;
        if self.current.is_none() && !self.shown.is_empty() {
            self.current = Some(0);
        }
        if self.shown.is_empty() {
            self.detail = tr("일치하는 프로파일이 없습니다. 검색어를 지우거나 DCP 폴더를 추가하세요.");
        }
        self.selection_changed();
    }

    /// The current row.
    pub fn selected(&self) -> Option<&ProfileRow> {
        self.current.and_then(|i| self.shown.get(i))
    }

    /// A row clicked.
    pub fn select(&mut self, index: usize) {
        if index < self.shown.len() {
            self.current = Some(index);
            self.selection_changed();
        }
    }

    /// Port of `selection_changed`'s detail text.
    fn selection_changed(&mut self) {
        self.touched = true;
        if let Some(row) = self.selected() {
            let camera = if row.camera.is_empty() { tr("공용") } else { row.camera.clone() };
            let last = if row.error.is_empty() { &row.path } else { &row.error };
            self.detail = format!("{} · {camera}\n{}\n{last}", row.name, row.copyright);
        }
    }

    /// 선택한 프로파일 적용 is enabled.
    pub fn apply_enabled(&self) -> bool {
        self.selected().is_some_and(|r| r.error.is_empty()) && !self.scanning
    }

    /// 즐겨찾기 전환 is enabled.
    pub fn favorite_enabled(&self) -> bool {
        !self.touched || self.selected().is_some_and(|r| !r.sha256.is_empty())
    }

    /// Port of `toggle_favorite`: returns the list to store as `dcp_favorites` (sorted).
    pub fn toggle_favorite(&mut self) -> Option<Vec<String>> {
        let digest = self.selected()?.sha256.clone();
        if self.favorites.contains(&digest) {
            self.favorites.remove(&digest);
        } else if !digest.is_empty() {
            self.favorites.insert(digest);
        }
        self.populate();
        Some(self.favorites.iter().cloned().collect())
    }
}

/// Port of `add_folder`'s preference change: `dcp_folders` with `folder` added once.
pub fn with_folder(folders: &[String], folder: &str) -> Vec<String> {
    let mut list = folders.to_vec();
    if !list.iter().any(|f| f == folder) {
        list.push(folder.to_owned());
    }
    list
}

// ---- the dialog --------------------------------------------------------------------------------------------------

type Reply = Rc<RefCell<Option<Result<ScanResult, String>>>>;

/// The 카메라 프로파일 window of the open RAW photo.
pub struct ProfileDialog {
    /// The photo it was opened for (`ident`): the chosen profile goes to it only while it is still open.
    photo: u64,
    pub browser: ProfileBrowser,
    reply: Reply,
    job: Option<JobId>,
    folder: Option<Receiver<Option<PathBuf>>>,
    /// The 검색에서 제외할 폴더 question: the folders and the chosen one.
    removing: Option<(Vec<String>, usize)>,
    started: bool,
}

impl ProfileDialog {
    /// The browser for photo `photo` with its `info` (`ProfileDialog(w, catalog.photo(ident)['info'])`); the first
    /// scan starts with the first frame.
    pub fn open(cx: &mut AppContext, photo: u64, info: Value) -> Self {
        let favorites = strings(cx.doc.preference("dcp_favorites"));
        Self {
            photo,
            browser: ProfileBrowser::new(info, &favorites),
            reply: Rc::new(RefCell::new(None)),
            job: None,
            folder: None,
            removing: None,
            started: false,
        }
    }

    /// Port of `refresh`: the scan on a job.
    pub fn refresh(&mut self, cx: &mut AppContext) {
        let (roots, cache) = roots_for(cx);
        if !self.browser.begin_scan(&roots) {
            return;
        }
        let reply: Reply = Rc::new(RefCell::new(None));
        self.reply = reply.clone();
        let id = cx.jobs.spawn(
            "dcp_scan",
            tr("설치된 프로파일을 검색하는 중…"),
            move |handle| scan(&roots, &cache, &|| handle.cancelled(), LIMIT),
            move |_: &mut AppContext, result| {
                *reply.borrow_mut() = Some(result);
            },
        );
        self.job = Some(id);
    }

    fn receive_scan(&mut self) {
        let Some(result) = self.reply.borrow_mut().take() else { return };
        self.job = None;
        match result {
            Ok(result) => self.browser.ready(&result),
            Err(error) => self.browser.failed(&error),
        }
    }

    fn add_folder(&mut self) {
        let (sender, receiver) = channel();
        let title = tr("DCP 프로파일이 있는 폴더");
        // Started here, on the window's thread; only the answer is awaited elsewhere (`native`).
        let answer = rfd::AsyncFileDialog::new().set_title(&title).pick_folder();
        let send = move |chosen: Option<rfd::FileHandle>| {
            let _ = sender.send(chosen.map(super::native::path_of));
        };
        if super::native::await_answer(answer, send) {
            self.folder = Some(receiver);
        }
    }

    /// A folder chosen in 프로파일 폴더 추가 (or by a test): stored in `dcp_folders`, then searched.
    pub fn folder_chosen(&mut self, cx: &mut AppContext, folder: &Path) {
        let list = with_folder(&folders(cx), &folder.to_string_lossy());
        if let Err(message) = cx.doc.save_preference("dcp_folders", &json!(list)) {
            log::warn!("cannot store the profile folders: {message}");
        }
        self.refresh(cx);
    }

    /// Port of `remove_folder` after the question: `folder` leaves `dcp_folders`, then searched again.
    pub fn remove_folder(&mut self, cx: &mut AppContext, folder: &str) {
        let list: Vec<String> = folders(cx).into_iter().filter(|f| f != folder).collect();
        if let Err(message) = cx.doc.save_preference("dcp_folders", &json!(list)) {
            log::warn!("cannot store the profile folders: {message}");
        }
        self.refresh(cx);
    }

    /// Port of `toggle_favorite` with its preference.
    pub fn toggle_favorite(&mut self, cx: &mut AppContext) {
        if let Some(list) = self.browser.toggle_favorite() {
            if let Err(message) = cx.doc.save_preference("dcp_favorites", &json!(list)) {
                log::warn!("cannot store the favourite profiles: {message}");
            }
        }
    }

    /// Port of `choose` and of `browse_dcp` after it: the selected profile read again and pinned to the photo the
    /// window was opened for. Returns whether the window closes.
    pub fn choose(&mut self, cx: &mut AppContext) -> bool {
        if !self.browser.apply_enabled() {
            return false;
        }
        let Some(row) = self.browser.selected().cloned() else { return false };
        match load_selected(&row, &self.browser.metadata) {
            Ok(record) => {
                if cx.doc.current_id() == self.photo {
                    crate::commands::raw::apply_record(cx, record, "DCP 프로파일");
                }
                true
            }
            Err(message) => {
                crate::commands::raw::warn(cx, "프로파일을 적용하지 못했습니다", &message);
                false
            }
        }
    }

    fn close(&mut self, cx: &mut AppContext) {
        if let Some(id) = self.job.take() {
            cx.jobs.cancel(id);
        }
    }

    fn remove_question(&mut self, ui: &mut egui::Ui, cx: &mut AppContext) {
        let Some((list, chosen)) = self.removing.as_mut() else { return };
        let mut answer = None;
        ui.separator();
        ui.label(egui::RichText::new(tr("검색에서 제외할 폴더")).font(t::bold(13.0)).color(t::BRAND));
        widgets::muted(ui, &tr("파일은 그대로 두고 검색 목록에서 제외합니다."));
        egui::ScrollArea::vertical().id_salt("dcp_remove_folder").max_height(90.0).show(ui, |ui| {
            for (index, folder) in list.iter().enumerate() {
                ui.selectable_value(chosen, index, folder);
            }
        });
        ui.horizontal(|ui| {
            if widgets::button(ui, &tr("취소")).clicked() {
                answer = Some(false);
            }
            if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                answer = Some(true);
            }
        });
        match answer {
            Some(true) => {
                let folder = list.get(*chosen).cloned();
                self.removing = None;
                if let Some(folder) = folder {
                    self.remove_folder(cx, &folder);
                }
            }
            Some(false) => self.removing = None,
            None => {}
        }
    }
}

impl Dialog for ProfileDialog {
    fn id(&self) -> &'static str {
        "camera_profiles"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        if !self.started {
            self.started = true;
            self.refresh(cx);
        }
        self.receive_scan();
        if let Some(receiver) = &self.folder {
            match receiver.try_recv() {
                Ok(chosen) => {
                    self.folder = None;
                    if let Some(folder) = chosen {
                        self.folder_chosen(cx, &folder);
                    }
                }
                Err(TryRecvError::Disconnected) => self.folder = None,
                Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        if self.browser.scanning {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut close = false;
        let response = egui::Modal::new(egui::Id::unique("camera_profiles")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(700.0);
            ui.label(egui::RichText::new(tr("카메라 프로파일")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            widgets::label(ui, &self.browser.heading(), t::TEXT, t::FONT_SIZE);
            widgets::muted(ui, &tr("이 카메라와 호환되는 DCP를 보여줍니다. 선택한 프로파일은 보정과 함께 저장됩니다."));
            ui.add_space(4.0);
            let before = (self.browser.search.clone(), self.browser.favorites_only);
            widgets::line_edit(ui, "dcp_search", &mut self.browser.search, &tr("프로파일 이름 검색"));
            widgets::checkbox(ui, &mut self.browser.favorites_only, &tr("즐겨찾기만"));
            if before != (self.browser.search.clone(), self.browser.favorites_only) {
                self.browser.populate();
            }
            let mut clicked = None;
            let mut double = false;
            egui::Frame::new().fill(t::LIST_BG).stroke(egui::Stroke::new(1.0, t::INPUT_BORDER)).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("dcp_list")
                    .min_scrolled_height(260.0)
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, row) in self.browser.shown.iter().enumerate() {
                            let text = self.browser.item_text(row);
                            let response = ui
                                .selectable_label(self.browser.current == Some(index), text)
                                .on_hover_text(if row.error.is_empty() { &row.path } else { &row.error });
                            if response.clicked() {
                                clicked = Some(index);
                            }
                            if response.double_clicked() {
                                clicked = Some(index);
                                double = true;
                            }
                        }
                    });
            });
            if let Some(index) = clicked {
                self.browser.select(index);
            }
            ui.add_space(4.0);
            ui.allocate_ui(egui::Vec2::new(ui.available_width(), 68.0), |ui| {
                ui.set_min_height(68.0);
                widgets::label(ui, &shown(cx, &self.browser.detail), t::TEXT, t::FONT_SIZE);
            });
            let status = widgets::muted(ui, &self.browser.status);
            if !self.browser.status_tip.is_empty() {
                status.on_hover_text(&self.browser.status_tip);
            }
            ui.add_space(6.0);
            let idle = !self.browser.scanning;
            let labels = [tr("다시 검색"), tr("프로파일 폴더 추가"), tr("추가 폴더 제외"), tr("즐겨찾기 전환")];
            let row = [
                Button::new(&labels[0]).enabled(idle),
                Button::new(&labels[1]).enabled(idle),
                Button::new(&labels[2]).enabled(idle),
                Button::new(&labels[3]).enabled(self.browser.favorite_enabled()),
            ];
            match widgets::button_row(ui, &row, ui.available_width(), 6.0) {
                Some(0) => self.refresh(cx),
                Some(1) => self.add_folder(),
                Some(2) => {
                    let list = folders(cx);
                    if list.is_empty() {
                        self.browser.status = tr("추가한 폴더가 없습니다.");
                    } else {
                        self.removing = Some((list, 0));
                    }
                }
                Some(_) => self.toggle_favorite(cx),
                None => {}
            }
            self.remove_question(ui, cx);
            ui.add_space(4.0);
            let labels = [tr("취소"), tr("선택한 프로파일 적용")];
            let row = [
                Button::new(&labels[0]),
                Button::new(&labels[1]).kind(Kind::Primary).enabled(self.browser.apply_enabled()),
            ];
            match widgets::button_row(ui, &row, ui.available_width(), 6.0) {
                Some(0) => close = true,
                Some(_) => close = self.choose(cx),
                None => {}
            }
            if double && self.choose(cx) {
                close = true;
            }
        });
        if response.should_close() {
            close = true;
        }
        if close {
            self.close(cx);
        }
        !close
    }
}
