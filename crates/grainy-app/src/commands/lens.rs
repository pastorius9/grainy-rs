//! Lens correction (렌즈 교정): the automatic profile from the photo's metadata, the lens dialog, applying a
//! profile, the panel's check boxes and labels, and the manual lens saved to and loaded from a file.
//!
//! Port of luma/studio.py `StudioTools.auto_lens`, `choose_lens`, `apply_lens`, `save_lens`, `load_lens` and the
//! lens lines of `load_settings` (the two labels and the enabling of the five check boxes). The profile logic is
//! `grainy_lens` (`optics.match_metadata`, `automatic_profile`, `make_profile`, `description`); the dialog is
//! [`crate::dialogs::lens`]; the panel is [`crate::panels::lens`].
//!
//! **The lensfun database.** `grainy_lens::Database` parses a folder of lensfun `*.xml` files given by the caller.
//! The folder is, in this order: `--lensfun <folder>` ([`set_folder_override`]), the environment variable
//! `GRAINY_RS_LENSFUN`, the folder `lensfun` inside the Rust app's data folder, the folder `lensfun` next to the
//! program. If none of them holds a database, the **built-in** one is used: the unmodified lensfun files of
//! `grainy-lens/data/lensfun/` are embedded in the executable (`Database::builtin`), so a bare program, an
//! installation without extra files and `cargo run` all find a database with no setup, and the person's own
//! newer `lensfun` folder still wins. It is read once, by a background job, when a command first needs it. An
//! explicit folder that cannot be read gives an empty database: the oracle's "no profile" texts show, and the
//! dialog says that the database is missing. (The data is lensfun's, CC BY-SA 3.0: the notice is in
//! `THIRD_PARTY_NOTICES.md`, `grainy-lens/data/lensfun/NOTICE` and the About box.)

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use grainy_core::settings::truthy;
use grainy_core::Settings;
use grainy_i18n::Language;
use grainy_lens::Database;
use serde_json::{json, Map, Value};

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::import_errors::Notice;
use crate::dialogs::lens::{LensChoice, LensDialog};
use crate::dialogs::preset_review::FileWait;
use crate::paths::Locations;
use crate::text::tr_with;

pub const COMMANDS: &[Command] = &[
    Command { id: "lens.auto", label: "렌즈 자동 선택", enabled: photo_open, run: auto },
    Command { id: "lens.choose", label: "카메라·렌즈 선택", enabled: photo_open, run: choose },
    Command { id: "lens.save", label: "현재 렌즈 프로파일 저장", enabled: photo_open, run: save },
    Command { id: "lens.load", label: "렌즈 프로파일 불러오기", enabled: photo_open, run: load },
];

/// The settings a manual lens file holds (`StudioTools.lens_keys`).
pub const LENS_KEYS: [&str; 7] =
    ["distortion", "distortion_k2", "distortion_k3", "lens_vignette", "ca_red", "ca_blue", "defringe"];

/// The environment variable that names the lensfun folder.
pub const FOLDER_VARIABLE: &str = "GRAINY_RS_LENSFUN";

/// The strings this feature adds to the oracle's (Korean source → English).
const ENGLISH: [(&str, &str); 2] = [
    ("렌즈 데이터베이스를 읽는 중…", "Reading the lens database…"),
    (
        "렌즈 데이터베이스(lensfun)를 찾지 못했습니다. 폴더: {0}",
        "The lens database (lensfun) was not found. Folder: {0}",
    ),
];

/// [`crate::text::tr`] with this feature's own strings first.
pub fn tr(source: &str) -> String {
    if grainy_i18n::language() == Language::English {
        if let Some((_, english)) = ENGLISH.iter().find(|(k, _)| *k == source) {
            return (*english).to_owned();
        }
    }
    crate::text::tr(source)
}

// ---- the database --------------------------------------------------------------------------------------------

static FOLDER_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// `--lensfun <folder>` (the command line): the folder is used instead of the default ones.
pub fn set_folder_override(folder: PathBuf) {
    let _ = FOLDER_OVERRIDE.set(folder);
}

/// The folders that may hold the database, best first.
pub fn candidates(
    explicit: Option<&Path>,
    variable: Option<&Path>,
    locations: &Locations,
    program_folder: Option<&Path>,
) -> Vec<PathBuf> {
    let mut list = Vec::new();
    list.extend(explicit.map(Path::to_path_buf));
    list.extend(variable.map(Path::to_path_buf));
    list.push(locations.data_folder().join("lensfun"));
    list.extend(program_folder.map(|p| p.join("lensfun")));
    list
}

/// The folder the database is read from: an explicit one (the state's, `--lensfun`) exactly as given, else the
/// first of the other candidates that exists, else the first of them.
pub fn database_folder(explicit: Option<&Path>) -> PathBuf {
    if let Some(folder) = explicit.map(Path::to_path_buf).or_else(|| FOLDER_OVERRIDE.get().cloned()) {
        return folder;
    }
    let variable = std::env::var_os(FOLDER_VARIABLE).filter(|v| !v.is_empty()).map(PathBuf::from);
    let program = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let list = candidates(None, variable.as_deref(), &Locations::current(), program.as_deref());
    list.iter().find(|p| p.is_dir()).or(list.first()).cloned().unwrap_or_default()
}

/// Where the database is read from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A folder of lensfun `*.xml` files (the command line, a setting, the data folder, beside the program).
    Folder(PathBuf),
    /// The database embedded in the program.
    Builtin,
}

/// The place the database is read from, in the order: `explicit` (the state's folder, else `--lensfun`) exactly as
/// given, the environment variable, the `lensfun` folder of the data folder, the one beside the program (the
/// last three only when they hold at least one `*.xml`), else the built-in database.
/// `locations` replaces the machine's (tests).
pub fn database_source(explicit: Option<&Path>, locations: Option<&Locations>) -> Source {
    if let Some(folder) = explicit.map(Path::to_path_buf).or_else(|| FOLDER_OVERRIDE.get().cloned()) {
        return Source::Folder(folder);
    }
    let variable = std::env::var_os(FOLDER_VARIABLE).filter(|v| !v.is_empty()).map(PathBuf::from);
    let program = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let locations = locations.cloned().unwrap_or_else(Locations::current);
    source_among(&candidates(None, variable.as_deref(), &locations, program.as_deref()))
}

/// The first of `folders` that holds lensfun files, else the built-in database.
pub fn source_among(folders: &[PathBuf]) -> Source {
    folders.iter().find(|f| has_xml(f)).cloned().map_or(Source::Builtin, Source::Folder)
}

fn has_xml(folder: &Path) -> bool {
    std::fs::read_dir(folder)
        .is_ok_and(|entries| entries.filter_map(Result::ok).any(|e| e.path().extension().is_some_and(|x| x == "xml")))
}

/// What the lens correction keeps between frames.
#[derive(Default)]
pub struct LensState {
    /// The lensfun folder to use instead of the default ones (tests, a setting).
    pub folder: Option<PathBuf>,
    /// The data folder logic to use instead of this machine's (tests).
    pub locations: Option<Locations>,
    database: Option<Arc<Database>>,
    error: Option<String>,
    loading: bool,
    /// What was asked while the database was being read, and for which photo.
    pending: Option<(Want, u64, u64)>,
    /// The message boxes shown so far (title, text); the oracle's are modal, tests read them here.
    pub boxes: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Want {
    Auto,
    Choose,
}

impl LensState {
    /// Uses `database` (tests).
    pub fn set_database(&mut self, database: Database) {
        self.database = Some(Arc::new(database));
        self.error = None;
    }

    /// The database, once read.
    pub fn database(&self) -> Option<&Arc<Database>> {
        self.database.as_ref()
    }

    /// Why the database could not be read (it is empty then).
    pub fn database_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Runs `want` once the database is there: now, or when the job that reads it has finished.
fn with_database(cx: &mut AppContext, want: Want) {
    let id = cx.doc.current_id();
    let generation = cx.document_generation;
    let state = cx.state::<LensState>();
    if let Some(database) = state.database.clone() {
        run(cx, &database, want);
        return;
    }
    state.pending = Some((want, id, generation));
    if state.loading {
        return;
    }
    state.loading = true;
    let source = database_source(state.folder.as_deref(), state.locations.as_ref());
    cx.jobs.spawn(
        "lens_db",
        tr("렌즈 데이터베이스를 읽는 중…"),
        move |_| {
            let (loaded, folder) = match &source {
                Source::Builtin => (Database::builtin(), PathBuf::new()),
                Source::Folder(folder) => (Database::from_folder(folder), folder.clone()),
            };
            Ok(match loaded {
                Ok(database) => (database, None),
                Err(_) => {
                    let note = tr_with(
                        "렌즈 데이터베이스(lensfun)를 찾지 못했습니다. 폴더: {0}",
                        &[&folder.to_string_lossy()],
                    );
                    (Database::default(), Some(note))
                }
            })
        },
        |cx: &mut AppContext, result| {
            let (database, error) = result.unwrap_or_else(|e| (Database::default(), Some(e)));
            let database = Arc::new(database);
            let state = cx.state::<LensState>();
            state.database = Some(database.clone());
            state.error = error;
            state.loading = false;
            if let Some((want, photo, generation)) = state.pending.take() {
                if cx.doc.current_id() == photo && cx.document_generation == generation {
                    run(cx, &database, want);
                }
            }
        },
    );
}

fn run(cx: &mut AppContext, database: &Arc<Database>, want: Want) {
    match want {
        Want::Auto => auto_with(cx, database),
        Want::Choose => choose_with(cx, database),
    }
}

// ---- the commands --------------------------------------------------------------------------------------------

/// `auto_lens` (the 렌즈 자동 선택 button).
fn auto(cx: &mut AppContext, _: &Invocation) {
    if photo_open(cx) {
        with_database(cx, Want::Auto);
    }
}

/// `choose_lens` (the 카메라·렌즈 선택 button).
fn choose(cx: &mut AppContext, _: &Invocation) {
    if photo_open(cx) {
        with_database(cx, Want::Choose);
    }
}

/// Port of `auto_lens` with the database: the profile of the photo's camera, lens and focal length is applied;
/// without one the reason goes to the status bar and the lens dialog opens.
pub fn auto_with(cx: &mut AppContext, database: &Arc<Database>) {
    let Some(id) = cx.doc.current().map(|p| p.id) else { return };
    let info = cx.doc.photo_info(id);
    if let Some(choice) = auto_for(cx, database, &info) {
        open(cx, choice);
    }
}

/// `auto_lens` on the metadata `info`: applies the profile (`None`), or sets the reason as the status message and
/// gives the dialog's state that `choose_lens` follows with.
pub fn auto_for(cx: &mut AppContext, database: &Arc<Database>, info: &Value) -> Option<LensChoice> {
    let (profile, reason) = database.automatic_profile(info);
    match profile {
        Some(profile) => {
            apply_lens(cx, profile);
            None
        }
        None => {
            cx.set_status(reason);
            Some(choose_for(cx, database, info))
        }
    }
}

/// Port of `choose_lens` with the database: the lens dialog on the photo's metadata and its current profile.
pub fn choose_with(cx: &mut AppContext, database: &Arc<Database>) {
    let Some(id) = cx.doc.current().map(|p| p.id) else { return };
    let info = cx.doc.photo_info(id);
    let choice = choose_for(cx, database, &info);
    open(cx, choice);
}

/// The dialog's state for the metadata `info` and the open photo's current profile.
pub fn choose_for(cx: &AppContext, database: &Arc<Database>, info: &Value) -> LensChoice {
    let current = cx.settings().value("lensfun").clone();
    let current = truthy(&current).then_some(current);
    LensChoice::new(database.clone(), info.clone(), current)
}

fn open(cx: &mut AppContext, choice: LensChoice) {
    cx.open_dialog(Box::new(LensDialog::from_choice(choice)));
}

/// What `dialog.exec()` returning true does: the dialog's profile is applied to the photo.
pub fn accepted(cx: &mut AppContext, choice: &mut LensChoice) {
    if let Some(profile) = choice.result.take() {
        apply_lens(cx, profile);
    }
}

/// Port of `apply_lens`: the profile and 렌즈 프로파일 사용 are set, committed as 렌즈 자동 보정.
pub fn apply_lens(cx: &mut AppContext, profile: Value) {
    if !photo_open(cx) {
        return;
    }
    let edit = cx.doc.edit_mut();
    edit.commit("보정 변경");
    edit.set("lensfun", profile);
    edit.set("lensfun_enabled", true);
    edit.commit("렌즈 자동 보정");
    cx.before = false;
    cx.set_status(crate::text::tr("렌즈 보정을 적용했습니다. 실행 취소로 되돌릴 수 있습니다."));
}

// ---- the manual lens file ------------------------------------------------------------------------------------

/// A message box (`QMessageBox.warning(self.w, tr('프로파일'), str(e))`).
fn warn(cx: &mut AppContext, text: &str) {
    let title = crate::text::tr("프로파일");
    cx.state::<LensState>().boxes.push((title.clone(), text.to_owned()));
    cx.open_dialog(Box::new(Notice { id: "lens_error", title, text: text.to_owned() }));
}

fn filter() -> (String, Vec<&'static str>) {
    ("Grainy lens".to_owned(), vec!["json"])
}

/// `save_lens`: asks for a file name (`lens.luma-lens.json`) and writes the manual lens there.
fn save(cx: &mut AppContext, _: &Invocation) {
    if !photo_open(cx) {
        return;
    }
    let title = crate::text::tr("렌즈 보정 프로파일 저장");
    cx.open_dialog(Box::new(FileWait::save(title, "lens.luma-lens.json", filter(), None, save_to)));
}

/// `load_lens`: asks for a file and reads the manual lens from it.
fn load(cx: &mut AppContext, _: &Invocation) {
    if !photo_open(cx) {
        return;
    }
    let title = crate::text::tr("렌즈 보정 프로파일");
    cx.open_dialog(Box::new(FileWait::open(title, filter(), None, load_from)));
}

/// The text `save_lens` writes: `{'type': 'luma-lens', 'version': 1, 'name': stem, 'settings': {...}}` as
/// `json.dumps(ensure_ascii=False, indent=2)`; line ends as Python's `write_text` makes them on this system.
pub fn lens_file_text(path: &Path, settings: &Settings) -> String {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut values = Map::new();
    for key in LENS_KEYS {
        values.insert(key.to_owned(), settings.value(key).clone());
    }
    let text =
        serde_json::to_string_pretty(&json!({"type": "luma-lens", "version": 1, "name": name, "settings": values}))
            .unwrap_or_default();
    if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text
    }
}

/// `save_lens`'s write.
pub fn save_to(cx: &mut AppContext, path: PathBuf) {
    let text = lens_file_text(&path, cx.settings());
    if let Err(error) = crate::paths::write_json(&path, &text) {
        let message = tr_with("처리하지 못했습니다: {0}", &[&error.to_string()]);
        cx.set_status(message);
    }
}

/// Python's `float(value)` for a JSON value: its number, or the message of the exception it raises.
pub fn py_float(value: &Value) -> Result<f64, String> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| "int too large to convert to float".to_owned()),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::String(s) => parse_float(s).ok_or_else(|| format!("could not convert string to float: {}", py_repr(s))),
        other => {
            let kind = match other {
                Value::Null => "NoneType",
                Value::Array(_) => "list",
                _ => "dict",
            };
            Err(format!("float() argument must be a string or a real number, not '{kind}'"))
        }
    }
}

fn parse_float(text: &str) -> Option<f64> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    match body {
        "inf" | "infinity" => {
            return Some(if lower.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
        }
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    // Underscores are allowed between digits only.
    let bytes = t.as_bytes();
    let mut cleaned = String::with_capacity(t.len());
    for (i, c) in t.char_indices() {
        if c == '_' {
            let between = i > 0 && bytes[i - 1].is_ascii_digit() && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !between {
                return None;
            }
        } else {
            cleaned.push(c);
        }
    }
    if cleaned.is_empty() || cleaned.chars().any(|c| !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E')))
    {
        return None;
    }
    cleaned.parse().ok()
}

/// Python's `repr` of a string.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
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
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `str(value)` of a JSON value, for the profile name (`str(data.get('name') or stem)`).
fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".to_owned(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// What a lens file holds: the seven values and the profile name.
#[derive(Debug, Clone, PartialEq)]
pub struct LensFile {
    pub values: Vec<(&'static str, f64)>,
    pub name: String,
}

/// The reading half of `load_lens`: the file's text to the values and the name, or the message of the box.
/// A file that is not JSON, not an object, or has no object `settings` is refused with the oracle's messages where
/// it has them (Python's own JSON error text, and the uncaught `AttributeError` of non-objects, are not copied).
pub fn read_lens_file(text: &str, path: &Path) -> Result<LensFile, String> {
    let data: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let refused = "Luma 렌즈 프로파일이 아닙니다.";
    let Some(object) = data.as_object() else { return Err(refused.to_owned()) };
    if object.get("type").and_then(Value::as_str) != Some("luma-lens") {
        return Err(refused.to_owned());
    }
    let settings = object.get("settings").ok_or_else(|| "'settings'".to_owned())?;
    let Some(settings) = settings.as_object() else { return Err(refused.to_owned()) };
    let mut values = Vec::new();
    for key in LENS_KEYS {
        // `data['settings'].get(k, 0)`: a missing key is 0, a null is an error.
        let value = match settings.get(key) {
            Some(v) => py_float(v)?,
            None => 0.0,
        };
        values.push((key, value));
    }
    if values.iter().any(|(_, v)| !v.is_finite() || v.abs() > 100.0) {
        return Err("프로파일 보정값이 범위를 벗어났습니다.".to_owned());
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match object.get("name") {
        Some(v) if truthy(v) => py_str(v),
        _ => stem,
    };
    Ok(LensFile { values, name })
}

/// The applying half of `load_lens`: the pending change is committed, the seven values and the profile name are
/// set, one undo step 렌즈 프로파일.
pub fn apply_lens_file(cx: &mut AppContext, file: &LensFile) {
    if !photo_open(cx) {
        return;
    }
    let edit = cx.doc.edit_mut();
    edit.commit("보정 변경");
    for (key, value) in &file.values {
        edit.set(key, *value);
    }
    edit.set("lens_profile", file.name.clone());
    edit.commit("렌즈 프로파일");
    cx.before = false;
}

/// `load_lens`'s read of `path`.
pub fn load_from(cx: &mut AppContext, path: PathBuf) {
    let read = std::fs::read_to_string(&path).map_err(|e| e.to_string());
    match read.and_then(|text| read_lens_file(&text, &path)) {
        Ok(file) => apply_lens_file(cx, &file),
        Err(message) => warn(cx, &message),
    }
}

// ---- what the panel shows ------------------------------------------------------------------------------------

/// The capability a check box of the panel needs from the profile (`distortion`, `tca`, `vignette`), if any.
fn capability(key: &str) -> Option<&'static str> {
    match key {
        "lensfun_distortion" => Some("distortion"),
        "lensfun_tca" => Some("tca"),
        "lensfun_vignette" => Some("vignette"),
        _ => None,
    }
}

/// `load_settings`: whether the check box of `key` takes clicks: there is a profile, and it can do what the box
/// switches (`capability in caps`).
pub fn switch_enabled(settings: &Settings, key: &str) -> bool {
    let data = settings.value("lensfun");
    if !truthy(data) {
        return false;
    }
    match capability(key) {
        None => true,
        Some(cap) => data
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|caps| caps.iter().any(|c| c.as_str() == Some(cap))),
    }
}

/// `load_settings`' `auto_lens_label`: the profile's description, or 자동 렌즈 프로파일 없음.
pub fn auto_label(settings: &Settings) -> String {
    let data = settings.value("lensfun");
    if !truthy(data) {
        return crate::text::tr("자동 렌즈 프로파일 없음");
    }
    grainy_lens::description(data).unwrap_or_else(|e| e.to_string())
}

/// `load_settings`' `lens_label`: the manual profile's name, or 렌즈 프로파일 없음.
pub fn manual_label(settings: &Settings) -> String {
    match settings.value("lens_profile") {
        v if truthy(v) => py_str(v),
        _ => crate::text::tr("렌즈 프로파일 없음"),
    }
}
