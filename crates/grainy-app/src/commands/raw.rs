//! RAW development controls: the working space, the camera (DCP) profile of a RAW photo, the RAW white balance
//! and the colour matrix profile, and the commands that open the profile browser and the RAW import defaults.
//!
//! Ports luma/studio.py `change_working`, `apply_dcp` / `apply_dcp_record`, `load_dcp`, `clear_dcp`, `browse_dcp`,
//! `raw_defaults`, `change_raw_mode`, `raw_temperature_changed`, `display_raw_white` and `camera_profile`. The
//! functions on an [`EditState`] are the oracle's methods without the widgets, so tests run them without a window;
//! the panel ([`crate::panels::calibration`]) and the dialogs ([`crate::dialogs::profiles`],
//! [`crate::dialogs::raw_defaults`]) call them.
//!
//! The camera calibration a RAW photo was read with (`source.raw_info` in the oracle) is read on a job once per
//! photo ([`camera_info`]) and kept in [`RawInfo`].

use std::path::{Path, PathBuf};

use grainy_core::Settings;
use grainy_raw::dcp;
use grainy_raw::rawcolor::{self, CameraInfo};
use serde_json::Value;

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::import_errors::Notice;
use crate::dialogs::preset_review::FileWait;
use crate::state::EditState;
use crate::text::tr;

pub const COMMANDS: &[Command] = &[
    Command { id: "raw.browse_dcp", label: "카메라 프로파일 찾아보기", enabled: raw_open, run: browse_dcp },
    Command { id: "raw.load_dcp", label: "DCP 파일 직접 불러오기", enabled: raw_open, run: load_dcp },
    Command { id: "raw.clear_dcp", label: "DCP 프로파일 해제", enabled: has_dcp, run: clear },
    Command { id: "raw.defaults", label: "RAW 가져오기 기본 보정", enabled: super::always, run: raw_defaults },
    Command {
        id: "raw.camera_matrix",
        label: "색 보정 행렬 프로파일 불러오기",
        enabled: photo_open,
        run: camera_matrix,
    },
];

/// The RAW white-balance modes of the choice box: (value, Korean text), in the oracle's order.
pub const RAW_MODES: [(&str, &str); 5] = [
    ("legacy", "RAW · 기존 현상 유지"),
    ("as_shot", "RAW · 촬영 시 화이트밸런스"),
    ("daylight", "RAW · 주광"),
    ("custom", "RAW · 색온도 직접 지정"),
    ("sample", "RAW · 스포이드"),
];

/// Whether `path` is a RAW file (`Path(path).suffix.lower() in RAW_EXTENSIONS`).
pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| grainy_engine::files::RAW_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

fn raw_open(cx: &AppContext) -> bool {
    cx.doc.current().is_some_and(|p| is_raw_path(&p.path))
}

fn has_dcp(cx: &AppContext) -> bool {
    cx.doc.current().is_some() && grainy_core::settings::truthy(cx.settings().value("dcp_profile"))
}

fn clip(v: f64, low: f64, high: f64) -> f64 {
    v.clamp(low, high)
}

// ---- the oracle's methods on the edit state ------------------------------------------------------------------

/// Port of `StudioTools.change_working`: the working space, committed as 작업 색공간.
pub fn change_working(edit: &mut EditState, space: &str) {
    edit.set("working_space", space);
    edit.commit("작업 색공간");
}

/// Port of `StudioTools.apply_dcp_record`: a pinned DCP record (`dcp.load_profile`) on the open photo, one undo
/// step DCP 프로파일; a RAW photo in the old development moves to the camera white balance. The photo must be a
/// RAW file of the profile's camera.
pub fn apply_dcp_record(edit: &mut EditState, path: &Path, info: &Value, record: Value) -> Result<(), String> {
    if !is_raw_path(path) {
        return Err("DCP는 RAW 사진에 적용합니다.".to_owned());
    }
    let profile = dcp::compiled(&record).map_err(|e| e.to_string())?;
    if !dcp::camera_matches(&profile, info) {
        return Err("프로파일의 카메라 모델이 선택한 RAW와 다릅니다.".to_owned());
    }
    edit.commit("보정 변경");
    edit.set("dcp_profile", record);
    if edit.settings().str("raw_mode") == "legacy" {
        edit.set("raw_mode", "as_shot");
    }
    edit.commit("DCP 프로파일");
    Ok(())
}

/// Port of `StudioTools.apply_dcp`: the DCP file at `path` read and pinned, then [`apply_dcp_record`].
pub fn apply_dcp_file(edit: &mut EditState, photo: &Path, info: &Value, file: &Path) -> Result<(), String> {
    let record = dcp::load_profile(file).map_err(|e| e.to_string())?;
    apply_dcp_record(edit, photo, info, record)
}

/// Port of `StudioTools.clear_dcp`: the profile removed, committed as DCP 해제.
pub fn clear_dcp(edit: &mut EditState) {
    edit.set("dcp_profile", Value::Null);
    edit.commit("DCP 해제");
}

/// The profile the oracle develops with: the pinned DCP, else the camera's own (`compiled(...) if ... else
/// default_profile(info)`).
fn profile_of(settings: &Settings, info: &CameraInfo) -> Option<dcp::Profile> {
    let record = settings.value("dcp_profile");
    if grainy_core::settings::truthy(record) {
        dcp::compiled(record).ok()
    } else {
        rawcolor::default_profile(info).ok()
    }
}

/// Port of `StudioTools.change_raw_mode`: the RAW white-balance mode `mode`, one undo step RAW 화이트밸런스. The old
/// development drops the DCP; moving to 색온도 직접 지정 from a camera mode starts from the resolved camera white
/// (`info` is the calibration the photo was read with: the oracle's `source.raw_info`, present only when the photo
/// was read in the camera's colours, see [`source_info`]).
pub fn change_raw_mode(edit: &mut EditState, mode: &str, info: Option<&CameraInfo>) {
    if mode == "legacy" && grainy_core::settings::truthy(edit.settings().value("dcp_profile")) {
        edit.set("dcp_profile", Value::Null);
    }
    if mode == "custom" && ["legacy", "as_shot", "daylight"].contains(&edit.settings().str("raw_mode")) {
        if let Some(info) = info {
            let settings = edit.settings().clone();
            if let Some(white) =
                profile_of(&settings, info).and_then(|p| rawcolor::resolve_white(&p, info, &settings).ok())
            {
                edit.set("raw_kelvin", clip(white.1, 2000.0, 50000.0));
                edit.set("raw_tint", clip(white.2, -150.0, 150.0));
            }
        }
    }
    edit.set("raw_mode", mode);
    edit.commit("RAW 화이트밸런스");
}

/// Port of `StudioTools.raw_temperature_changed`: the RAW temperature and tint sliders moved, so the mode becomes
/// 색온도 직접 지정 and the sampled neutral goes (no commit: the slider's release commits).
pub fn raw_temperature_changed(edit: &mut EditState, kelvin: f64, tint: f64) {
    edit.set("raw_kelvin", kelvin);
    edit.set("raw_tint", tint);
    edit.set("raw_mode", "custom");
    edit.set("raw_neutral", Value::Null);
}

/// Port of `StudioTools.display_raw_white`: in the camera modes the RAW sliders show the resolved camera white
/// (temperature, tint), clipped to the sliders' ranges; `None` where they show the stored values.
pub fn display_raw_white(settings: &Settings, info: Option<&CameraInfo>) -> Option<(f64, f64)> {
    let info = info?;
    if !["as_shot", "daylight", "sample"].contains(&settings.str("raw_mode")) {
        return None;
    }
    let p = profile_of(settings, info)?;
    let (_, temperature, tint, _) = rawcolor::resolve_white(&p, info, settings).ok()?;
    Some((clip(temperature, 2000.0, 50000.0), clip(tint, -150.0, 150.0)))
}

/// The oracle's `source.raw_info` for settings: a RAW photo is read in the camera's colours (and has its
/// calibration) only when `rawcolor.enabled(settings)`.
pub fn source_info<'a>(settings: &Settings, info: Option<&'a CameraInfo>) -> Option<&'a CameraInfo> {
    info.filter(|_| rawcolor::enabled(settings))
}

/// Python's `float(x)` as `np.asarray(..., np.float32)` takes a JSON value.
fn matrix_entry(value: &Value) -> Result<f32, String> {
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN) as f32),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Null => Ok(f32::NAN),
        Value::String(s) => grainy_adobe::pyutil::parse_float(s)
            .map(|v| v as f32)
            .ok_or_else(|| format!("could not convert string to float: '{s}'")),
        Value::Array(_) | Value::Object(_) => Err("3×3 유한 행렬이 필요합니다.".to_owned()),
    }
}

/// Port of the reading in `StudioTools.camera_profile`: the `matrix` of a JSON file as float32 (`np.asarray(...,
/// np.float32).tolist()`), 3×3, finite, no entry above 10 in size.
pub fn camera_matrix_from(text: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let matrix = value.get("matrix").ok_or_else(|| "'matrix'".to_owned())?;
    let need = || "3×3 유한 행렬이 필요합니다.".to_owned();
    let rows = matrix.as_array().ok_or_else(need)?;
    if rows.len() != 3 {
        return Err(need());
    }
    let mut out = Vec::new();
    for row in rows {
        let row = row.as_array().filter(|r| r.len() == 3).ok_or_else(need)?;
        let mut values = Vec::new();
        for entry in row {
            let v = matrix_entry(entry)?;
            if !v.is_finite() || v.abs() > 10.0 {
                return Err(need());
            }
            values.push(Value::from(f64::from(v)));
        }
        out.push(Value::Array(values));
    }
    Ok(Value::Array(out))
}

// ---- the camera calibration of the open photo -------------------------------------------------------------------

/// The camera calibration of the open RAW photo, read once per photo on a job.
#[derive(Default)]
pub struct RawInfo {
    /// The photo it belongs to (id and path) and the calibration (`None` while reading or when it cannot be read).
    key: Option<(u64, PathBuf)>,
    info: Option<CameraInfo>,
}

/// The calibration the open RAW photo is read with (`source.raw_info`), when it was read already; starts the
/// reading job otherwise.
pub fn camera_info(cx: &mut AppContext) -> Option<CameraInfo> {
    let photo = cx.doc.current()?;
    if !is_raw_path(&photo.path) {
        return None;
    }
    let key = (photo.id, photo.path.clone());
    let state = cx.state::<RawInfo>();
    if state.key.as_ref() == Some(&key) {
        return state.info.clone();
    }
    state.key = Some(key.clone());
    state.info = None;
    let path = key.1.clone();
    let mut settings = cx.settings().clone();
    settings.set("raw_mode", "as_shot");
    settings.set("negative", false);
    cx.jobs.spawn(
        "raw_info",
        "",
        move |_| {
            let loaded = grainy_engine::files::load_photo(&path, Some(64), &settings).map_err(|e| e.to_string())?;
            let info = loaded.metadata.get("raw_info").cloned().ok_or_else(|| "no camera calibration".to_owned())?;
            serde_json::from_value::<CameraInfo>(info).map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            let state = cx.state::<RawInfo>();
            if state.key.as_ref() != Some(&key) {
                return;
            }
            match result {
                Ok(info) => state.info = Some(info),
                Err(message) => log::info!("camera calibration of photo {} not read: {message}", key.0),
            }
        },
    );
    None
}

// ---- the commands ----------------------------------------------------------------------------------------------

/// A warning box (`QMessageBox.warning`).
pub fn warn(cx: &mut AppContext, title: &str, text: &str) {
    cx.open_dialog(Box::new(Notice { id: "raw_warning", title: tr(title), text: tr(text) }));
}

/// The open photo's path and stored info.
fn open_photo(cx: &AppContext) -> Option<(u64, PathBuf, Value)> {
    let photo = cx.doc.current()?;
    Some((photo.id, photo.path.clone(), cx.doc.photo_info(photo.id)))
}

/// Port of `apply_dcp_record` on the open photo; a refusal shows `title`'s warning.
pub fn apply_record(cx: &mut AppContext, record: Value, title: &str) {
    let Some((_, path, info)) = open_photo(cx) else { return };
    match apply_dcp_record(cx.doc.edit_mut(), &path, &info, record) {
        Ok(()) => cx.before = false,
        Err(message) => warn(cx, title, &message),
    }
}

/// Port of `browse_dcp`: the camera profile browser for the open photo's camera.
fn browse_dcp(cx: &mut AppContext, _: &Invocation) {
    let Some((id, _, info)) = open_photo(cx) else { return };
    let dialog = crate::dialogs::profiles::ProfileDialog::open(cx, id, info);
    cx.open_dialog(Box::new(dialog));
}

/// Port of `load_dcp`: a DCP file chosen in the file dialog, applied to the open photo.
fn load_dcp(cx: &mut AppContext, _: &Invocation) {
    if cx.doc.current().is_none() {
        return;
    }
    let filter = ("DCP".to_owned(), vec!["dcp", "DCP"]);
    cx.open_dialog(Box::new(FileWait::open(tr("DCP 카메라 프로파일 선택"), filter, None, dcp_chosen)));
}

fn dcp_chosen(cx: &mut AppContext, file: PathBuf) {
    let Some((_, path, info)) = open_photo(cx) else { return };
    match apply_dcp_file(cx.doc.edit_mut(), &path, &info, &file) {
        Ok(()) => cx.before = false,
        Err(message) => warn(cx, "DCP 프로파일", &message),
    }
}

fn clear(cx: &mut AppContext, _: &Invocation) {
    clear_dcp(cx.doc.edit_mut());
}

/// Port of `raw_defaults`: the RAW import defaults box.
fn raw_defaults(cx: &mut AppContext, _: &Invocation) {
    let dialog = crate::dialogs::raw_defaults::RawDefaultsDialog::open(cx);
    cx.open_dialog(Box::new(dialog));
}

/// Port of `camera_profile`: a 3×3 RGB matrix from a JSON file (`camera_matrix`); the oracle's commit timer
/// stores it as 보정 변경.
fn camera_matrix(cx: &mut AppContext, _: &Invocation) {
    let filter = ("JSON".to_owned(), vec!["json"]);
    cx.open_dialog(Box::new(FileWait::open(tr("RGB 행렬 프로파일"), filter, None, matrix_chosen)));
}

fn matrix_chosen(cx: &mut AppContext, file: PathBuf) {
    if cx.doc.current().is_none() {
        return;
    }
    let read = std::fs::read(&file).map_err(|e| e.to_string()).and_then(|bytes| {
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        camera_matrix_from(&text)
    });
    match read {
        Ok(matrix) => {
            let edit = cx.doc.edit_mut();
            edit.set("camera_matrix", matrix);
            edit.commit("보정 변경");
        }
        Err(message) => warn(cx, "프로파일", &message),
    }
}
