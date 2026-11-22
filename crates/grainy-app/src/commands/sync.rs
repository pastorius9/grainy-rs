//! Sync and copy settings (APP_PLAN package 12): the copy set and its dialog (`LibraryManager.choose_copy`), paste
//! on the selected photos (`MainWindow.paste_edits`), 동기화 (`sync_selected`), Auto Sync (`toggle_auto_sync`,
//! `after_commit`), 이전 사진 보정 붙이기 (`WorkflowTools.previous_edits`) and 선택 사진 보정 초기화
//! (`photo_actions.reset_selected`).
//!
//! The oracle writes these through two paths and so does this file: `manager.edit_photo` (one history row and one
//! undo step per photo: paste, sync, Auto Sync, reset; [`Document::edit_photos`]) and `command_batch.apply` without
//! a batch record (the previous photo's settings; [`Document::edit_photos_batch`]). Neither writes the AI batch
//! index (`photo_service.apply_batch`), so no batch record is written here either.
//!
//! What a pasted value needs fitted to the receiving photo is the oracle's: the panels' on/off state travels with
//! the pasted values (`panel_keys.carried`), a lens profile must fit the photo's lens (`optics.retarget`), and the
//! RAW colour settings follow the file format and camera (`rawcolor.retarget`).

use std::collections::BTreeSet;

use grainy_core::panel_keys::{carried, panels_of};
use grainy_core::Settings;
use grainy_service::edits::{raw_retarget, Fitting, LensMismatch};
use serde_json::{Map, Value};

use super::{always, photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::copy_items::{group, CopyItems};
use crate::dialogs::import_errors::Notice;
use crate::state::{py_eq, Commit};
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "sync.choose_items", label: "복사할 항목 선택", enabled: always, run: choose },
    Command { id: "sync.selected", label: "현재 보정 → 선택 사진", enabled: photo_open, run: selected },
    Command {
        id: "sync.auto", label: "Auto Sync · 선택한 사진 함께 보정", enabled: always, run: toggle_auto
    },
    Command {
        id: "sync.copy_crop", label: "붙여넣기에 크롭·회전도 포함", enabled: always, run: copy_crop
    },
    Command {
        id: "develop.paste_previous", label: "이전 사진 보정 붙이기", enabled: photo_open, run: previous
    },
    Command {
        id: "sync.reset_selected", label: "선택 사진 보정 초기화", enabled: photo_open, run: reset_selected
    },
];

/// The history labels of the whole-photo writes of this file. The open photo takes them through its edit state;
/// the oracle writes them without `commit`, so they never reach `after_commit` (Auto Sync).
const OWN_LABELS: [&str; 4] = ["보정 붙여넣기", "보정 동기화", "Auto Sync", "보정 초기화"];

/// `crop`, `rotation`, `straighten`, `flip`: the group a paste leaves alone unless 붙여넣기에 크롭·회전도 포함.
const CROP: [&str; 4] = ["crop", "rotation", "straighten", "flip"];

/// What the feature keeps between frames (`LibraryManager.copy_keys` / `auto_sync`, `MainWindow.copy_crop`,
/// `WorkflowTools.previous_id`).
#[derive(Debug, Clone)]
pub struct SyncState {
    /// The settings copy, paste and sync take over (`manager.copy_keys`).
    pub copy_keys: BTreeSet<String>,
    /// 붙여넣기에 크롭·회전도 포함 (`copy_crop`).
    pub copy_crop: bool,
    /// Auto Sync is on (`manager.auto_sync`).
    pub auto: bool,
    /// The photo that was open before the current one (`workflow.previous_id`).
    pub previous: Option<u64>,
    /// The open photo at the last frame.
    seen: u64,
    /// Auto Sync is writing (`manager.syncing`).
    syncing: bool,
    /// Labels of commits made with `manager.syncing=True` (spatial repairs that stay on their photo), each skipped
    /// once by Auto Sync.
    pub local_commits: Vec<String>,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            // `set(defaults()) - set(EDIT_GROUPS['크롭·회전']) - {'panels_off'}`.
            copy_keys: grainy_service::edits::copy_keys(None).unwrap_or_default(),
            copy_crop: false,
            auto: false,
            previous: None,
            seen: 0,
            syncing: false,
            local_commits: Vec::new(),
        }
    }
}

impl SyncState {
    /// Clears document-scoped photo IDs when switching libraries.
    pub fn reset_document(&mut self) {
        self.previous = None;
        self.seen = 0;
        self.syncing = false;
        self.local_commits.clear();
    }

    /// Port of `copy_crop_changed`: the crop keys join or leave the copy set.
    pub fn set_copy_crop(&mut self, on: bool) {
        self.copy_crop = on;
        for key in group("크롭·회전") {
            if on {
                self.copy_keys.insert((*key).to_owned());
            } else {
                self.copy_keys.remove(*key);
            }
        }
    }
}

// ---- fitting to the receiving photo ---------------------------------------------------------------------------------

/// `optics.retarget(settings, info)` on `value`, in place: the lens profile of a copied calibration is fitted to the
/// receiving photo, or switched off with a note saying why (the oracle never raises here). The app has no lens
/// database yet, so the photo's camera is looked up in an empty one: exact for photos without camera
/// information; a camera the oracle's database knows would give its crop factor and compatible lenses.
fn lens_retarget(value: &mut Map<String, Value>, info: &Value) -> Result<(), String> {
    static EMPTY: std::sync::OnceLock<Option<grainy_lens::Database>> = std::sync::OnceLock::new();
    if !value.get("lensfun").is_some_and(grainy_core::settings::truthy) {
        return Ok(());
    }
    let database = EMPTY
        .get_or_init(|| grainy_lens::Database::from_xml("<lensdatabase version=\"2\"></lensdatabase>", "").ok())
        .as_ref()
        .ok_or("lens database")?;
    match database.retarget(&Value::Object(value.clone()), info).map_err(|e| e.to_string())? {
        Value::Object(map) => *value = map,
        _ => return Err("settings".into()),
    }
    Ok(())
}

/// The fitting `rawcolor.retarget` needs: whether a DCP camera profile belongs to the photo's camera.
struct Fit;

impl Fitting for Fit {
    fn resolve(&self, preset: &Value, _: &Value, _: &str) -> Result<Value, String> {
        Ok(preset.clone())
    }

    fn lens(&self, settings: &Value, _: &Value) -> Result<Value, LensMismatch> {
        Ok(settings.clone())
    }

    fn camera_matches(&self, profile: &Value, info: &Value) -> bool {
        grainy_raw::dcp::compiled(profile).map(|p| grainy_raw::dcp::camera_matches(&p, info)).unwrap_or(false)
    }
}

/// Port of `rawcolor.retarget(settings, info)`: the RAW colour settings fitted to the photo (what
/// `manager.edit_photo` and `apply_edits(retarget=True)` do to every value they write).
pub fn fitted(settings: &Settings, info: &Value) -> Result<Settings, String> {
    let value = raw_retarget(&Value::Object(settings.0.clone()), info, &Fit).map_err(|e| e.to_string())?;
    match value {
        Value::Object(map) => Ok(Settings(map)),
        _ => Err("settings".into()),
    }
}

/// `value.update({k: source[k] for k in keys})` and `panels_off = carried(target, source, panels_of(keys))`.
fn take_over(
    value: &mut Map<String, Value>,
    target: &Map<String, Value>,
    source: &Map<String, Value>,
    keys: &BTreeSet<String>,
) {
    for key in keys {
        if let Some(v) = source.get(key) {
            value.insert(key.clone(), v.clone());
        }
    }
    let panels = panels_of(keys.iter().map(String::as_str));
    value.insert("panels_off".into(), Value::Array(carried(target, source, &panels)));
}

/// Port of one pass of `MainWindow.paste_edits`: `copied` over `original` for `keys`, the lens retargeted when the
/// lens group is copied, the photo's own crop and rotation kept unless `copy_crop`, then the RAW fit.
pub fn pasted(
    copied: &Settings,
    original: &Settings,
    info: &Value,
    keys: &BTreeSet<String>,
    copy_crop: bool,
) -> Result<Settings, String> {
    let mut value = original.0.clone();
    take_over(&mut value, &original.0, &copied.0, keys);
    if keys.contains("lensfun") {
        lens_retarget(&mut value, info)?;
    }
    if !copy_crop {
        for key in CROP {
            if let Some(v) = original.0.get(key) {
                value.insert(key.into(), v.clone());
            }
        }
    }
    fitted(&Settings(value), info)
}

/// Port of one pass of `LibraryManager.sync_selected`: `values` (the open photo) over `target` for `keys`.
pub fn synced(values: &Settings, target: &Settings, info: &Value, keys: &BTreeSet<String>) -> Result<Settings, String> {
    let mut value = target.0.clone();
    take_over(&mut value, &target.0, &values.0, keys);
    if keys.contains("lensfun") {
        lens_retarget(&mut value, info)?;
    }
    Ok(Settings(value))
}

/// What one commit changed for Auto Sync (`after_commit`): the changed values of the copy set and the panels that
/// follow them (the changed values' panels and the switched panels the copy set owns).
pub fn auto_changes(commit: &Commit, keys: &BTreeSet<String>) -> (Map<String, Value>, Vec<&'static str>) {
    let (previous, live) = (&commit.previous.0, &commit.settings.0);
    let changes: Map<String, Value> = live
        .iter()
        .filter(|(k, v)| keys.contains(*k) && !py_eq(previous.get(*k).unwrap_or(&Value::Null), v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let off = |s: &Map<String, Value>| -> BTreeSet<String> {
        s.get("panels_off")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };
    let moved: BTreeSet<String> = off(previous).symmetric_difference(&off(live)).cloned().collect();
    let owned = panels_of(keys.iter().map(String::as_str));
    let mut panels = panels_of(changes.keys().map(String::as_str));
    for panel in owned {
        if moved.contains(panel) && !panels.contains(&panel) {
            panels.push(panel);
        }
    }
    (changes, panels)
}

/// Port of the values `after_commit` writes to one other photo.
pub fn auto_synced(
    changes: &Map<String, Value>,
    panels: &[&str],
    live: &Settings,
    target: &Settings,
    info: &Value,
) -> Result<Settings, String> {
    let mut value = target.0.clone();
    for (key, v) in changes {
        value.insert(key.clone(), v.clone());
    }
    let off = carried(&value, &live.0, panels);
    value.insert("panels_off".into(), Value::Array(off));
    if changes.contains_key("lensfun") {
        lens_retarget(&mut value, info)?;
    }
    Ok(Settings(value))
}

// ---- the commands ---------------------------------------------------------------------------------------------------

fn selected_ids(cx: &AppContext) -> Vec<u64> {
    cx.doc.selected_indices().iter().filter_map(|i| cx.doc.photos().get(*i).map(|p| p.id)).collect()
}

/// `MainWindow.commit()`: the pending change becomes a step and is stored.
fn commit_pending(cx: &mut AppContext) {
    cx.doc.edit_mut().commit("보정 변경");
    cx.flush();
}

/// `manager.run`: an error is a message box.
fn failed(cx: &mut AppContext, message: &str) {
    cx.status = tr_with("처리하지 못했습니다: {0}", &[&tr(message)]);
    cx.open_dialog(Box::new(Notice {
        id: "sync_error", title: tr("작업을 완료하지 못했습니다"), text: tr(message)
    }));
}

fn choose(cx: &mut AppContext, _: &Invocation) {
    let keys = cx.state::<SyncState>().copy_keys.clone();
    cx.open_dialog(Box::new(CopyItems::new(&keys)));
}

/// The answer of 복사·동기화 항목: the copy set is the ticked groups, 붙여넣기에 크롭·회전도 포함 follows the crop
/// group, and the current edits are copied (`copy_edits`).
pub fn choose_done(cx: &mut AppContext, checks: &[(&'static str, bool)]) {
    let keys = crate::dialogs::copy_items::keys_of(checks);
    let crop = checks.iter().any(|(title, on)| *title == "크롭·회전" && *on);
    let state = cx.state::<SyncState>();
    state.copy_keys = keys;
    state.copy_crop = crop;
    cx.copy_edits();
}

fn copy_crop(cx: &mut AppContext, invocation: &Invocation) {
    let state = cx.state::<SyncState>();
    let on = invocation.value.as_ref().and_then(Value::as_bool).unwrap_or(!state.copy_crop);
    state.set_copy_crop(on);
}

/// Port of `MainWindow.paste_edits` on the selected photos.
pub fn paste(cx: &mut AppContext) {
    let Some(copied) = cx.doc.copied() else {
        cx.status = tr("먼저 현재 보정을 복사하세요.");
        return;
    };
    commit_pending(cx);
    let ids = selected_ids(cx);
    let (keys, crop) = {
        let state = cx.state::<SyncState>();
        (state.copy_keys.clone(), state.copy_crop)
    };
    for id in &ids {
        let result = cx.doc.stored_settings_of(*id).and_then(|original| {
            let info = cx.doc.photo_info(*id);
            pasted(&copied, &original, &info, &keys, crop)
        });
        let written = result.and_then(|value| cx.doc.edit_photos(&[*id], &[value], "보정 붙여넣기"));
        if let Err(message) = written {
            failed(cx, &message);
            return;
        }
    }
    cx.before = false;
    cx.status = tr_with("{0}장에 보정값을 적용했습니다.", &[&ids.len().to_string()]);
}

/// Port of `LibraryManager.sync_selected` (동기화): the open photo's copy-set settings onto the other selected
/// photos, history label 보정 동기화.
pub fn sync_selected(cx: &mut AppContext) -> Result<usize, String> {
    let current = cx.doc.current_id();
    if current == 0 {
        return Ok(0);
    }
    commit_pending(cx);
    let values = cx.doc.edit().settings().clone();
    let keys = cx.state::<SyncState>().copy_keys.clone();
    let ids: Vec<u64> = selected_ids(cx).into_iter().filter(|id| *id != current).collect();
    for id in &ids {
        let target = cx.doc.stored_settings_of(*id)?;
        let value = synced(&values, &target, &cx.doc.photo_info(*id), &keys)?;
        cx.doc.edit_photos(&[*id], &[value], "보정 동기화")?;
    }
    cx.status = tr_with("{0}장에 선택한 보정 항목을 동기화했습니다.", &[&ids.len().to_string()]);
    Ok(ids.len())
}

fn selected(cx: &mut AppContext, _: &Invocation) {
    if let Err(message) = sync_selected(cx) {
        failed(cx, &message);
    }
}

fn toggle_auto(cx: &mut AppContext, invocation: &Invocation) {
    let state = cx.state::<SyncState>();
    state.auto = invocation.value.as_ref().and_then(Value::as_bool).unwrap_or(!state.auto);
    let on = state.auto;
    cx.status = if on {
        tr("Auto Sync: 현재 선택한 사진들에 변경 항목을 함께 적용합니다.")
    } else {
        tr("Auto Sync 해제")
    };
}

/// Whether a command is a check mark in a menu (Auto Sync).
pub fn checked(cx: &mut AppContext, id: &str) -> Option<bool> {
    match id {
        "sync.auto" => Some(cx.state::<SyncState>().auto),
        _ => None,
    }
}

/// Port of `LibraryManager.after_commit`: one commit of the open photo, written to the other selected photos
/// when Auto Sync is on.
fn auto_sync(cx: &mut AppContext, id: u64, commit: &Commit) -> Result<(), String> {
    if OWN_LABELS.contains(&commit.label.as_str()) || id != cx.doc.current_id() {
        return Ok(());
    }
    let keys = {
        let state = cx.state::<SyncState>();
        if let Some(i) = state.local_commits.iter().position(|l| *l == commit.label) {
            state.local_commits.remove(i);
            return Ok(());
        }
        if !state.auto || state.syncing {
            return Ok(());
        }
        state.copy_keys.clone()
    };
    let (changes, panels) = auto_changes(commit, &keys);
    if changes.is_empty() && panels.is_empty() {
        return Ok(());
    }
    cx.state::<SyncState>().syncing = true;
    let result = (|| {
        for target_id in selected_ids(cx).into_iter().filter(|i| *i != id) {
            let target = cx.doc.stored_settings_of(target_id)?;
            let value = auto_synced(&changes, &panels, &commit.settings, &target, &cx.doc.photo_info(target_id))?;
            cx.doc.edit_photos(&[target_id], &[value], "Auto Sync")?;
        }
        Ok(())
    })();
    cx.state::<SyncState>().syncing = false;
    result
}

/// What the window does every frame after it stored the edits: remembers the photo that was open before
/// (`previous_id`) and gives each commit to Auto Sync.
pub fn after_flush(cx: &mut AppContext) {
    let current = cx.doc.current_id();
    {
        let state = cx.state::<SyncState>();
        if current != 0 && current != state.seen {
            if state.seen != 0 {
                state.previous = Some(state.seen);
            }
            state.seen = current;
        }
    }
    for (id, commit) in cx.doc.take_commits() {
        if let Err(message) = auto_sync(cx, id, &commit) {
            failed(cx, &message);
        }
    }
    // Writes of Auto Sync itself never start another round.
    cx.doc.take_commits();
}

/// Port of `WorkflowTools.previous_edits` (이전 사진 보정 붙이기): the previous photo's copy-set settings onto the
/// selected photos, one batch labelled 이전 사진 보정. Returns the changed ids.
pub fn paste_previous(cx: &mut AppContext) -> Result<Vec<u64>, String> {
    commit_pending(cx);
    let ids = selected_ids(cx);
    let source = cx.state::<SyncState>().previous.and_then(|id| cx.doc.stored_settings_of(id).ok().map(|s| (id, s)));
    let Some((source_id, source)) = source else {
        return Err("먼저 기준 사진을 열고 다음 사진으로 이동하세요.".into());
    };
    let keys = cx.state::<SyncState>().copy_keys.clone();
    let targets: Vec<u64> = ids.into_iter().filter(|id| *id != source_id).collect();
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for id in &targets {
        let own = cx.doc.stored_settings_of(*id)?;
        values.push(synced(&source, &own, &cx.doc.photo_info(*id), &keys)?);
    }
    let changed = cx.doc.edit_photos_batch(&targets, &values, "이전 사진 보정")?;
    cx.before = false;
    cx.status = tr_with("{0} · {1}장 변경", &[&tr("이전 사진 보정"), &group_thousands(changed.len())]);
    Ok(changed)
}

/// Python's `f'{n:,}'`.
fn group_thousands(n: usize) -> String {
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

fn previous(cx: &mut AppContext, _: &Invocation) {
    if let Err(message) = paste_previous(cx) {
        failed(cx, &message);
    }
}

/// Port of `photo_actions.reset_selected` (`edit_selected` with the defaults): every selected photo's settings
/// back to the defaults, label 보정 초기화, one undo step each.
pub fn reset_photos(cx: &mut AppContext) -> Result<usize, String> {
    commit_pending(cx);
    let ids = selected_ids(cx);
    let defaults = grainy_core::defaults();
    for id in &ids {
        let mut value = cx.doc.stored_settings_of(*id)?;
        for (key, v) in &defaults.0 {
            value.0.insert(key.clone(), v.clone());
        }
        cx.doc.edit_photos(&[*id], &[value], "보정 초기화")?;
    }
    cx.before = false;
    cx.status =
        tr_with("{0}장의 사진에 적용했습니다. 사진별 실행 취소로 되돌릴 수 있습니다.", &[&ids.len().to_string()]);
    Ok(ids.len())
}

fn reset_selected(cx: &mut AppContext, _: &Invocation) {
    if let Err(message) = reset_photos(cx) {
        failed(cx, &message);
    }
}
