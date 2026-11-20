//! Presets: the preset tree's data, applying a preset to the open photo, saving the current edit as a preset,
//! importing presets from `.json`, `.xmp` and `.lrtemplate` files (with the review box and its test develop),
//! exporting and deleting them.
//!
//! Ports luma/app.py `refresh_presets` (`all_presets`), `apply_preset`, `save_preset` and luma/manager.py
//! `import_preset`, `export_preset`, `delete_preset`. The tree itself is drawn by [`crate::panels::presets`], the
//! boxes by [`crate::dialogs::preset_review`].
//!
//! The presets live in the library's catalogue (`presets` table, `grainy_catalog::Catalog::{save_preset,
//! save_adobe_preset, presets, delete_preset}`); a session or a read-only library has no catalogue of its own
//! and keeps its presets in memory for as long as the window is open. All catalogue work runs as jobs on a
//! second connection ([`Store`]); the functions below the commands take a [`Store`] so tests run them without a
//! window.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use grainy_catalog::Catalog;
use grainy_core::{defaults, normalized, Settings};
use grainy_service::edits::{preset_settings, Fitting};
use grainy_service::LensMismatch;
use serde_json::{json, Map, Value};

use super::{always, photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::preset_review::{self, FileWait, NamePrompt, PresetDelete, PresetReview};
use crate::document::Document;
use crate::text::tr;

pub const COMMANDS: &[Command] = &[
    Command { id: "presets.save", label: "현재 보정을 프리셋으로 저장", enabled: photo_open, run: save },
    Command { id: "presets.import", label: "프리셋 파일 가져오기", enabled: idle, run: import },
    Command { id: "presets.export", label: "프리셋 파일 내보내기", enabled: always, run: export },
    Command { id: "presets.delete", label: "사용자 프리셋 삭제", enabled: idle, run: delete },
];

/// The Adobe source record inside an Adobe preset (`adobe_preset.KEY`).
pub const KEY: &str = grainy_adobe::preset::KEY;

/// The keys a preset never takes from its own values: the photo's composition (`apply_preset`).
pub const GEOMETRY: [&str; 4] = ["crop", "rotation", "straighten", "flip"];

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn idle(cx: &AppContext) -> bool {
    !cx.jobs.running("preset")
}

// ---- the preset list --------------------------------------------------------------------------------------------

/// Port of `engine.PRESETS`: Grainy's built-in presets as values over the defaults, in the oracle's order.
fn built_in_values() -> Vec<(&'static str, Value)> {
    vec![
        ("원본의 색", json!({})),
        (
            "Soft daylight",
            json!({"exposure": 0.15, "contrast": -12, "highlights": -22, "shadows": 18, "temperature": 4, "saturation": -6}),
        ),
        (
            "Amber film",
            json!({"temperature": 16, "tint": 4, "contrast": 12, "highlights": -28, "shadows": 12, "saturation": -14,
                   "grain": 12, "vignette": 12}),
        ),
        ("Silver / B&W", json!({"monochrome": true, "contrast": 24, "highlights": -15, "shadows": 10, "grain": 16})),
        (
            "Crisp landscape",
            json!({"contrast": 15, "vibrance": 22, "highlights": -24, "shadows": 22, "clarity": 22, "sharpen": 25}),
        ),
        ("Quiet blue", json!({"temperature": -13, "saturation": -16, "contrast": -6, "shadows": 20, "blacks": 8})),
    ]
}

/// The built-in presets as `refresh_presets` makes them: `normalized({'tone_version': ..., **values})`.
pub fn built_in() -> &'static [(String, Value)] {
    static LIST: std::sync::OnceLock<Vec<(String, Value)>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        let tone = defaults().value("tone_version").clone();
        built_in_values()
            .into_iter()
            .map(|(name, values)| {
                let mut map = Map::new();
                map.insert("tone_version".into(), tone.clone());
                if let Value::Object(v) = values {
                    map.extend(v);
                }
                (name.to_owned(), Value::Object(normalized(&map).0))
            })
            .collect()
    })
}

/// Port of `all_presets` in `refresh_presets`: the built-in presets, then the catalogue's (a user preset named
/// like a built-in one takes its place in the list, as a Python dict merge does).
pub fn all_presets(user: &[(String, Value)]) -> Vec<(String, Value)> {
    let mut all: Vec<(String, Value)> = built_in().to_vec();
    for (name, value) in user {
        match all.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value.clone(),
            None => all.push((name.clone(), value.clone())),
        }
    }
    all
}

// ---- where the presets are kept ---------------------------------------------------------------------------------

/// Where user presets are stored: a catalogue, or memory. One lives for the length of one job.
#[allow(clippy::large_enum_variant)]
pub enum Store {
    /// The library's catalogue.
    Catalog(Catalog),
    /// A session's presets, in the order of `Catalog.presets` (by name).
    Memory(Vec<(String, Value)>),
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Store {
    /// The store of a library folder (`None`: the in-memory presets given).
    pub fn open(folder: Option<&Path>, memory: &[(String, Value)]) -> Result<Self, String> {
        match folder {
            Some(folder) => Ok(Self::Catalog(Catalog::new(folder).map_err(text)?)),
            None => Ok(Self::Memory(memory.to_vec())),
        }
    }

    /// Port of `Catalog.presets`: name and stored value, by name.
    pub fn list(&self) -> Result<Vec<(String, Value)>, String> {
        match self {
            Self::Catalog(catalog) => {
                let value = catalog.presets().map_err(text)?;
                Ok(value
                    .as_object()
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default())
            }
            Self::Memory(list) => Ok(list.clone()),
        }
    }

    /// Port of `Catalog.save_preset`: the settings normalized, the photo's geometry reset to the defaults.
    pub fn save(&mut self, name: &str, settings: &Settings) -> Result<(), String> {
        match self {
            Self::Catalog(catalog) => catalog.save_preset(name, settings).map_err(text),
            Self::Memory(list) => {
                let mut value = normalized(&settings.0);
                let base = defaults();
                for key in GEOMETRY {
                    value.set(key, base.value(key).clone());
                }
                memory_put(list, name, Value::Object(value.0));
                Ok(())
            }
        }
    }

    /// Port of `Catalog.save_adobe_preset`: the translated settings with the Adobe source record kept.
    pub fn save_adobe(&mut self, name: &str, preset: &Map<String, Value>) -> Result<(), String> {
        match self {
            Self::Catalog(catalog) => catalog
                .save_adobe_preset(name, &Value::Object(preset.clone()), |record| {
                    translated(record).map_err(grainy_catalog::Error::Invalid)
                })
                .map_err(text),
            Self::Memory(list) => {
                let record = preset.get(KEY).ok_or("missing Adobe preset source")?;
                let mut value = translated(record)?["settings"].as_object().cloned().unwrap_or_default();
                value.insert(KEY.into(), record.clone());
                memory_put(list, name, Value::Object(value));
                Ok(())
            }
        }
    }

    /// Port of `Catalog.delete_preset`.
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        match self {
            Self::Catalog(catalog) => catalog.delete_preset(name).map_err(text),
            Self::Memory(list) => {
                list.retain(|(n, _)| n != name);
                Ok(())
            }
        }
    }
}

fn memory_put(list: &mut Vec<(String, Value)>, name: &str, value: Value) {
    list.retain(|(n, _)| n != name);
    list.push((name.to_owned(), value));
    list.sort_by(|a, b| a.0.cmp(&b.0));
}

/// `adobe_preset.translate(record)` (base `None`, RAW) as `{'settings': ..., 'mapped': [...]}`; refused without a
/// mapped value, as `Catalog.save_adobe_preset` does.
fn translated(record: &Value) -> Result<Value, String> {
    let record = record.as_object().ok_or("the Adobe preset source is not a record")?;
    let converted = grainy_adobe::preset::translate(record, None, true).map_err(|e| e.message)?;
    if converted.mapped.is_empty() {
        return Err("변환 가능한 프리셋 보정이 없습니다.".to_owned());
    }
    // Stored in the defaults' key order, as the oracle's `normalized` leaves them.
    let ordered = grainy_catalog::settings(&Value::Object(converted.settings.0)).map_err(text)?;
    Ok(json!({"settings": ordered.0, "mapped": converted.mapped}))
}

// ---- applying -----------------------------------------------------------------------------------------------------

/// What `apply_preset` needs from outside the settings: `adobe_preset.resolve`, `optics.retarget` (without a lens
/// database a lens profile is kept only when the target photo's lens is the profile's) and
/// `dcp.camera_matches`.
struct Fit {
    error: std::cell::RefCell<Option<String>>,
}

impl Fitting for Fit {
    fn resolve(&self, preset: &Value, settings: &Value, path: &str) -> Result<Value, String> {
        let (Some(preset), Some(base)) = (preset.as_object(), settings.as_object()) else {
            return Err("TypeError".into());
        };
        grainy_adobe::preset::resolve(preset, base, path).map(|s| Value::Object(s.0)).map_err(|e| {
            *self.error.borrow_mut() = Some(e.message.clone());
            e.kind.python_name().to_owned()
        })
    }

    fn lens(&self, settings: &Value, info: &Value) -> Result<Value, LensMismatch> {
        let Some(data) = settings.get("lensfun").filter(|v| truthy(v)) else { return Ok(settings.clone()) };
        let name = info.get("lens").and_then(Value::as_str).unwrap_or("");
        let maker = data.get("maker").and_then(Value::as_str).unwrap_or("");
        let own = data.get("name").and_then(Value::as_str).unwrap_or("");
        if !name.is_empty() && grainy_lens::model_key(name, maker) != grainy_lens::model_key(own, maker) {
            *self.error.borrow_mut() = Some(tr("대상 렌즈가 달라 프로파일을 적용하지 않았습니다. "));
            return Err(LensMismatch);
        }
        Ok(settings.clone())
    }

    fn camera_matches(&self, profile: &Value, info: &Value) -> bool {
        grainy_raw::dcp::compiled(profile).map(|p| grainy_raw::dcp::camera_matches(&p, info)).unwrap_or(false)
    }
}

/// Port of `apply_preset`'s computation: the settings a preset gives for the photo `path` (with the stored
/// `info`): `adobe_preset.resolve`, the lens and RAW retargets, and the photo's own crop, rotation, straighten
/// and flip kept. `grainy_service::edits::preset_settings` does the steps.
pub fn applied(preset: &Value, settings: &Settings, path: &Path, info: &Value) -> Result<Settings, String> {
    let fit = Fit { error: Default::default() };
    let record = json!({"path": path.to_string_lossy(), "info": info});
    match preset_settings(preset, &Value::Object(settings.0.clone()), &record, &fit) {
        Ok(Value::Object(map)) => Ok(Settings(map)),
        Ok(_) => Err("the preset gave no settings".to_owned()),
        Err(error) => Err(fit.error.take().unwrap_or_else(|| error.to_string())),
    }
}

/// The stored `info` of a photo (`catalog.photo(id)['info']`); a photo without a catalogue entry gets its file
/// format, which is what the RAW retarget reads.
pub fn photo_info(folder: Option<&Path>, id: u64, path: &Path) -> Value {
    if let Some(folder) = folder {
        if let Ok(catalog) = Catalog::new(folder) {
            if let Ok(Some(row)) = catalog.photo(id as i64) {
                return row["info"].clone();
            }
        }
    }
    let format = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    json!({ "format": format })
}

/// Applies `preset` to the open photo of `doc` as `apply_preset` does after resolving: one undo step labelled
/// with the oracle's default label.
pub fn apply_to(doc: &mut dyn Document, preset: &Value, info: &Value) -> Result<(), String> {
    let Some(photo) = doc.current() else { return Ok(()) };
    let path = photo.path.clone();
    let edit = doc.edit_mut();
    edit.commit("보정 변경");
    let value = applied(preset, edit.settings(), &path, info)?;
    edit.replace(value, "보정 변경");
    Ok(())
}

// ---- importing and exporting ---------------------------------------------------------------------------------------

/// An Adobe preset waiting for the review box: its name in the library, the translated preset and the texts of
/// the box.
#[derive(Clone, Debug)]
pub struct Review {
    pub name: String,
    pub preset: Map<String, Value>,
    /// `preset[KEY]['preview']`.
    pub preview: String,
    /// `preset[KEY]['report']`.
    pub report: String,
}

/// What importing a file did.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// A Grainy preset was added (or replaced) under this name.
    Saved(String),
    /// An Adobe preset to review before it is added.
    Review(Review),
}

/// The test develop of `import_preset`: `develop(np.full((32, 32, 3), .2), normalized(preset))`.
pub fn test_develop(settings: &Settings) -> Result<(), String> {
    let image = ndarray::Array3::<f32>::from_elem((32, 32, 3), 0.2);
    grainy_engine::develop(&image, &settings.0, &grainy_engine::Options::default()).map(|_| ()).map_err(text)
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Port of `import_preset` after the file dialog: reads `path` and adds a Grainy preset (`.json`, or a Grainy
/// `.xmp` sidecar), or returns the Adobe preset (`.xmp`, `.lrtemplate`) for the review box.
pub fn import_file(store: &mut Store, path: &Path) -> Result<Outcome, String> {
    let suffix = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut name = stem.clone();
    // `data.get('settings') or data.get('edits')`
    let mut found: Option<Value> = None;
    if suffix == ".xmp" {
        let sidecar = grainy_library::xmp::read_sidecar(path).map_err(text)?;
        found = sidecar.settings.map(|s| Value::Object(s.0));
    } else if suffix != ".lrtemplate" {
        let bytes = std::fs::read(path).map_err(text)?;
        let data: Value = serde_json::from_slice(&bytes).map_err(text)?;
        let Value::Object(data) = data else {
            return Err(format!("'{}' object has no attribute 'get'", type_name(&data)));
        };
        if let Some(Value::String(given)) = data.get("name") {
            name = given.clone();
        }
        found = match data.get("settings") {
            Some(v) if truthy(v) => Some(v.clone()),
            _ => data.get("edits").cloned(),
        };
    }
    let settings = match found {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    };
    if settings.is_none() && (suffix == ".xmp" || suffix == ".lrtemplate") {
        let preset = grainy_adobe::preset::read(path).map_err(|e| e.message)?;
        let existing: HashSet<String> = store.list()?.into_iter().map(|(n, _)| n).collect();
        let mut name = stem.clone();
        let mut number = 2;
        while existing.contains(&name) {
            name = format!("{stem} ({number})");
            number += 1;
        }
        let record = preset.get(KEY).and_then(Value::as_object).ok_or("the Adobe preset has no source record")?;
        let field = |k: &str| record.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        let review = Review { name, preview: field("preview"), report: field("report"), preset };
        return Ok(Outcome::Review(review));
    }
    let Some(settings) = settings else {
        return Err("Luma 보정값이 들어 있는 프리셋이 아닙니다. Adobe 현상값은 직접 호환되지 않습니다.".to_owned());
    };
    // Exercise the processing pipeline before accepting a foreign preset.
    let settings = normalized(&settings);
    test_develop(&settings)?;
    store.save(&name, &settings)?;
    Ok(Outcome::Saved(name))
}

/// The tail of `import_preset` for an Adobe preset after 프리셋 추가: the test develop, then the catalogue.
pub fn add_reviewed(store: &mut Store, review: &Review) -> Result<(), String> {
    test_develop(&normalized(&review.preset))?;
    store.save_adobe(&review.name, &review.preset)
}

/// Port of `export_preset`'s file: `{'name': stem, 'settings': settings}`, indented by two spaces.
pub fn export_text(path: &Path, settings: &Settings) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    serde_json::to_string_pretty(&json!({"name": stem, "settings": settings.0})).unwrap_or_default()
}

// ---- the window's side -------------------------------------------------------------------------------------------

/// What the preset tree and its boxes keep between frames.
#[derive(Default)]
pub struct PresetState {
    /// The user presets of the open library (or the session's, in memory).
    pub user: Vec<(String, Value)>,
    /// The library folder `user` was read for (`Some(None)`: no catalogue).
    pub loaded_for: Option<Option<PathBuf>>,
    /// The tree folders that are open (`opened` in `refresh_presets`), by key.
    pub open: HashSet<String>,
    /// The clicked row's key.
    pub selected: Option<String>,
}

/// Runs `work` on a second connection to the library's presets; `done` follows on the window's thread, after the
/// list was refreshed (`refresh_presets`). An error opens the error box.
fn store_job<T: Send + 'static>(
    cx: &mut AppContext,
    work: impl FnOnce(&mut Store) -> Result<T, String> + Send + 'static,
    done: impl FnOnce(&mut AppContext, T) + 'static,
) {
    let folder = cx.doc.library_folder();
    let memory = cx.state::<PresetState>().user.clone();
    let owner = folder.clone();
    cx.state::<PresetState>().loaded_for = Some(folder.clone());
    cx.jobs.spawn(
        "preset",
        tr("프리셋"),
        move |_| {
            let mut store = Store::open(folder.as_deref(), &memory)?;
            let result = work(&mut store)?;
            Ok((result, store.list()?))
        },
        move |cx: &mut AppContext, result| match result {
            Ok((value, list)) => {
                if cx.doc.library_folder() == owner {
                    cx.state::<PresetState>().user = list;
                }
                done(cx, value);
            }
            Err(message) => {
                cx.state::<PresetState>().loaded_for = None;
                preset_review::show_error(cx, &message);
            }
        },
    );
}

/// Reads the presets of the open library when the tree needs them (the library changed or was not read yet).
pub fn ensure_loaded(cx: &mut AppContext) {
    let folder = cx.doc.library_folder();
    let state = cx.state::<PresetState>();
    if state.loaded_for.as_ref() == Some(&folder) {
        return;
    }
    if folder.is_none() || state.loaded_for.as_ref().is_some_and(|f| f.is_some()) {
        state.user.clear();
    }
    state.loaded_for = Some(folder.clone());
    if folder.is_some() {
        store_job(cx, |_| Ok(()), |_, ()| {});
    }
}

/// Port of `apply_preset`: the preset `name` of the tree on the open photo (a background job fits it to the
/// photo; the result is applied if the same photo is still open and unchanged).
pub fn apply(cx: &mut AppContext, name: &str) {
    let Some(photo) = cx.doc.current() else { return };
    let (id, path) = (photo.id, photo.path.clone());
    let user = cx.state::<PresetState>().user.clone();
    let Some((_, preset)) = all_presets(&user).into_iter().find(|(n, _)| n == name) else { return };
    let edit = cx.doc.edit_mut();
    edit.commit("보정 변경");
    let (settings, version) = (edit.settings().clone(), edit.version());
    let generation = cx.document_generation;
    let folder = cx.doc.library_folder();
    cx.jobs.spawn(
        "preset",
        tr("프리셋"),
        move |_| {
            let info = photo_info(folder.as_deref(), id, &path);
            applied(&preset, &settings, &path, &info)
        },
        move |cx: &mut AppContext, result| {
            if cx.document_generation != generation || cx.doc.current_id() != id || cx.doc.edit().version() != version {
                return;
            }
            match result {
                Ok(value) => {
                    cx.doc.edit_mut().replace(value, "보정 변경");
                    cx.before = false;
                }
                Err(message) => preset_review::show_error(cx, &message),
            }
        },
    );
}

/// The click on a film of the tree: `StudioTools.change_film`.
pub fn film(cx: &mut AppContext, id: &str) {
    if cx.doc.current().is_some() {
        cx.change_film(id);
    }
}

/// 현재 보정을 프리셋으로 저장: asks for a name.
fn save(cx: &mut AppContext, _: &Invocation) {
    if cx.doc.current().is_none() {
        return;
    }
    cx.open_dialog(Box::new(NamePrompt::new()));
}

/// Port of `save_preset` after the name box: the live settings under `name.strip()`; the other-presets folder
/// opens, where the preset went.
pub fn save_named(cx: &mut AppContext, name: &str) {
    let name = name.trim().to_owned();
    if name.is_empty() || cx.doc.current().is_none() {
        return;
    }
    let settings = cx.settings().clone();
    store_job(
        cx,
        move |store| store.save(&name, &settings),
        |cx, ()| {
            cx.state::<PresetState>().open.insert("others".to_owned());
        },
    );
}

/// 프리셋 파일 가져오기: a file dialog, then [`import_path`].
fn import(cx: &mut AppContext, _: &Invocation) {
    let filter = ("사진 프리셋 (*.json *.xmp *.lrtemplate)".to_owned(), vec!["json", "xmp", "lrtemplate"]);
    let start = cx.doc.current().and_then(|p| p.path.parent().map(PathBuf::from));
    cx.open_dialog(Box::new(FileWait::open(tr("프리셋 가져오기"), filter, start, import_path)));
}

/// `import_preset` for the chosen `path`.
pub fn import_path(cx: &mut AppContext, path: PathBuf) {
    store_job(
        cx,
        move |store| import_file(store, &path),
        |cx, outcome| {
            if let Outcome::Review(review) = outcome {
                cx.open_dialog(Box::new(PresetReview::new(review)));
            }
        },
    );
}

/// 프리셋 추가 of the review box: the test develop and the catalogue.
pub fn add_review(cx: &mut AppContext, review: Review) {
    store_job(cx, move |store| add_reviewed(store, &review), |_, ()| {});
}

/// 프리셋 파일 내보내기: a save dialog, then the file with the current settings.
fn export(cx: &mut AppContext, _: &Invocation) {
    let filter = ("Grainy preset (*.json)".to_owned(), vec!["json"]);
    let start = cx.doc.current().and_then(|p| p.path.parent().map(PathBuf::from));
    cx.open_dialog(Box::new(FileWait::save(
        tr("현재 보정 프리셋 내보내기"),
        "preset.luma.json",
        filter,
        start,
        export_to,
    )));
}

/// `export_preset` for the chosen `path`: written on a job.
pub fn export_to(cx: &mut AppContext, path: PathBuf) {
    let settings = cx.settings().clone();
    cx.jobs.spawn(
        "preset",
        tr("프리셋"),
        move |_| crate::paths::write_json(&path, &export_text(&path, &settings)).map_err(text),
        |cx: &mut AppContext, result| {
            if let Err(message) = result {
                preset_review::show_error(cx, &message);
            }
        },
    );
}

/// 사용자 프리셋 삭제: the names are read, then the choice box opens.
fn delete(cx: &mut AppContext, _: &Invocation) {
    store_job(
        cx,
        |_| Ok(()),
        |cx, ()| {
            let names: Vec<String> = cx.state::<PresetState>().user.iter().map(|(n, _)| n.clone()).collect();
            if names.is_empty() {
                preset_review::show_error(cx, &tr("삭제할 사용자 프리셋이 없습니다."));
            } else {
                cx.open_dialog(Box::new(PresetDelete::new(names)));
            }
        },
    );
}

/// `delete_preset` after the choice box.
pub fn delete_named(cx: &mut AppContext, name: String) {
    store_job(cx, move |store| store.delete(&name), |_, ()| {});
}

/// The tooltip of a film row (`refresh_presets`): name, kind and ISO.
pub fn film_tooltip(entry: &grainy_film::Entry) -> String {
    let mut parts = vec![entry.name.clone(), tr(entry.raw["kind_title"].as_str().unwrap_or(""))];
    if let Some(iso) = entry.raw.get("iso").filter(|v| truthy(v)) {
        parts.push(format!("ISO {}", iso_text(iso)));
    }
    parts.join(" · ")
}

fn iso_text(iso: &Value) -> String {
    iso.as_str().map(str::to_owned).unwrap_or_else(|| iso.to_string())
}
