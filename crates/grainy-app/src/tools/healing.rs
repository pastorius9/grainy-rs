//! The Healing tool on the photo (복구): clone stamp, healing brush, dust and scratch removal (inpaint) and red
//! eye, as the Python Grainy offers them. Port of luma/studio.py `mode`, `preview_stroke`, `stroke`,
//! `make_retouch`, `stroke_valid`, `cancel_stroke`, `apply_healing`, `remove_healing`, `clear_healing`,
//! `update_healing_overlay`, `retouch_cursor`, `set_brush_size`, `automatic_healing_source`, `remove_retouch`,
//! `heal_shortcut`, `tab_changed` and of the tool part of luma/widgets.py `PhotoView` (`set_tool`,
//! `cancel_stroke`, the stroke in `mousePressEvent` / `mouseMoveEvent` / `mouseReleaseEvent`, `keyPressEvent`,
//! `resize_brush`, the brush cursor and the healing overlay of `drawForeground`).
//!
//! A stroke becomes one `retouch` operation (`{type, version: 2, points, source, radius, feather}`) whose points
//! are carried to the source through the whole geometry ([`CoordinateMap::source_points`]). Clone, inpaint and
//! red eye are added at once (one undo step `부분 보정`); healing strokes are staged: they show as a red coverage
//! overlay and are applied together (Enter, 실행) as one undo step. The pixels come from the engine
//! (`grainy-local` `apply_retouch` in `grainy-engine`), so the photo shows what an export gives.
//!
//! The tool keeps its state in [`HealingState`] (`cx.state()`); the panel ([`crate::panels::healing`]) and the
//! commands ([`crate::commands::healing`]) call the functions here. Every function works without a window, so
//! scripted strokes are tested against the oracle's own methods (`tools/golden/gen_app.py`, `healing_*`).

use std::cell::Cell;

use eframe::egui::{self, Color32, Key, Modifiers, Painter, Pos2, Rect, Shape, Stroke};
use grainy_core::{Plane, Settings};
use serde_json::{json, Value};

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::{AppContext, Mode};
use crate::imagespace::{CoordinateMap, ViewTransform};
use crate::panels::Tool;
use crate::state::settings_eq;
use crate::text::{tr, tr_with};

/// The retouch kinds (`tool_mode` values of the Healing tool), in the order of the panel's buttons.
pub const KINDS: [&str; 4] = ["clone", "heal", "inpaint", "red_eye"];
/// The healing brush.
pub const HEAL: &str = "heal";
/// The clone stamp.
pub const CLONE: &str = "clone";

/// `Adjustment('radius', '복구 브러시 크기', .002, .12, 1000)`: slider positions.
pub const RADIUS_RANGE: (i64, i64) = (2, 120);
/// `Adjustment('feather', '복구 경계 흐림', 0, 100)`.
pub const FEATHER_RANGE: (i64, i64) = (0, 100);
/// The live source's long side (`resize_float(source, 720)` in luma/app.py).
pub const LIVE_SIZE: usize = 720;

/// The stroke being drawn (`pending_stroke`).
#[derive(Debug, Clone)]
pub struct PendingStroke {
    /// The photo it belongs to (`photo`).
    pub photo: u64,
    /// The settings when it started (`base`).
    pub base: Settings,
    /// The settings it expects now: `base` for a staged heal, `base` with the operation shown live otherwise (the
    /// oracle draws the pending operation through `preview_settings`; here it is set as a live value).
    expected: Settings,
    /// A healing stroke, staged until applied (`staged`).
    pub staged: bool,
    /// The operation (`component`).
    pub component: Value,
}

/// The red coverage of the staged strokes over the displayed frame (`healing_overlay`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub width: usize,
    pub height: usize,
    /// `uint8(clip(coverage, 0, 1) * 140)` per frame pixel; the colour is (255, 80, 65).
    pub alpha: Vec<u8>,
}

/// What the Healing tool keeps between frames.
pub struct HealingState {
    /// `view.tool_mode` while it is a retouch kind; `None` once the tool is ended (`set_tool('')`).
    pub mode: Option<&'static str>,
    /// `retouch_radius` slider position (radius × 1000).
    pub radius: i64,
    /// `retouch_feather` slider value.
    pub feather: i64,
    /// The source chosen with Alt+click (`donor`), in source fractions.
    pub donor: Option<[f64; 2]>,
    /// The source note says 직접 지정 (Alt+클릭).
    pub manual_source: bool,
    /// Staged healing operations (`healing_strokes`).
    pub staged: Vec<Value>,
    /// The photo the staged strokes belong to (`healing_context`).
    pub context: Option<u64>,
    /// The stroke being drawn.
    pub pending: Option<PendingStroke>,
    /// The frame points of the stroke being drawn (`view.tool_points`).
    pub points: Vec<[f64; 2]>,
    /// Alt was held when the stroke started (`tool_alt`).
    pub alt: bool,
    /// The cursor's radius (fraction of the shorter side) and feather (`brush_radius`, `brush_feather`).
    pub brush_radius: f64,
    pub brush_feather: f64,
    /// `healing_generation`.
    pub generation: u64,
    /// The photo whose controls were loaded (`photo_id`).
    photo: u64,
    map: Option<(String, CoordinateMap)>,
    /// The staged strokes' mask on the live source (`healing_mask`).
    mask: Option<Plane>,
    overlay: Option<Overlay>,
    overlay_key: Option<(String, u64, usize)>,
    texture: Option<egui::TextureHandle>,
    /// The last frame the tool was seen open (opening the tool picks the healing brush).
    seen: Cell<Option<u64>>,
}

impl Default for HealingState {
    fn default() -> Self {
        Self {
            mode: None,
            radius: 15,
            feather: 60,
            donor: None,
            manual_source: false,
            staged: Vec::new(),
            context: None,
            pending: None,
            points: Vec::new(),
            alt: false,
            brush_radius: 0.02,
            brush_feather: 70.0,
            generation: 0,
            photo: 0,
            map: None,
            mask: None,
            overlay: None,
            overlay_key: None,
            texture: None,
            seen: Cell::new(None),
        }
    }
}

impl HealingState {
    /// The brush radius of new operations (`retouch_radius.slider.value() / 1000`).
    pub fn radius_value(&self) -> f64 {
        self.radius as f64 / 1000.0
    }

    /// The overlay shown now (`view.healing_overlay`).
    pub fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    /// Whether 표시 모두 취소 can run (`update_healing_controls`).
    pub fn can_clear(&self) -> bool {
        !self.staged.is_empty() || self.pending.as_ref().is_some_and(|p| p.staged)
    }

    /// The source note (`retouch_source_note`), Korean source text.
    pub fn source_note(&self) -> &'static str {
        if self.manual_source {
            "힐링 원본: 직접 지정 (Alt+클릭)"
        } else {
            "힐링 원본: 자동 선택"
        }
    }
}

fn st(cx: &mut AppContext) -> &mut HealingState {
    cx.state::<HealingState>()
}

/// Python's `round` (half to even).
fn py_round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// The live source's size for an original of `width` × `height` (`resize_float(source, 720)`).
pub fn live_size(width: usize, height: usize) -> (usize, usize) {
    let longest = width.max(height);
    if longest <= LIVE_SIZE {
        return (width.max(1), height.max(1));
    }
    let factor = LIVE_SIZE as f64 / longest as f64;
    (py_round(width as f64 * factor).max(1) as usize, py_round(height as f64 * factor).max(1) as usize)
}

/// The open photo and its live source's size, once its size is known (`self.w.source is not None`).
fn photo(cx: &AppContext) -> Option<(u64, (usize, usize))> {
    let current = cx.doc.current()?;
    let (w, h) = current.original?;
    Some((current.id, live_size(w as usize, h as usize)))
}

/// The coordinate map's key (`coordinate_key`): the photo, its live size and the geometry of the settings as
/// shown.
fn map_key(cx: &AppContext) -> Option<String> {
    let (id, (w, h)) = photo(cx)?;
    let s = grainy_core::effective(cx.settings());
    let geometry: serde_json::Map<String, Value> =
        grainy_engine::GEOMETRY_KEYS.iter().map(|k| ((*k).to_owned(), s.value(k).clone())).collect();
    Some(format!("{id}:{w}x{h}:{}", Value::Object(geometry)))
}

/// Port of `source_point`'s cache: the map of the settings as shown, built when the geometry changes.
fn ensure_map(cx: &mut AppContext) -> Option<String> {
    let key = map_key(cx)?;
    if st(cx).map.as_ref().is_some_and(|(k, _)| *k == key) {
        return Some(key);
    }
    let (_, (w, h)) = photo(cx)?;
    let map = CoordinateMap::new(w, h, cx.settings()).ok()?;
    st(cx).map = Some((key.clone(), map));
    Some(key)
}

/// Port of `source_points`: the stroke's frame points on the source.
fn source_points(cx: &mut AppContext, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if points.is_empty() || ensure_map(cx).is_none() {
        return Vec::new();
    }
    st(cx).map.as_ref().map(|(_, m)| m.source_points(points)).unwrap_or_default()
}

fn points_value(points: &[[f64; 2]]) -> Value {
    Value::Array(points.iter().map(|p| json!([p[0], p[1]])).collect())
}

/// Port of `make_retouch`.
pub fn make_retouch(state: &HealingState, kind: &str, points: &[[f64; 2]]) -> Value {
    json!({
        "type": kind,
        "version": 2,
        "points": points_value(points),
        "source": state.donor.map_or(Value::Null, |d| json!([d[0], d[1]])),
        "radius": state.radius_value(),
        "feather": state.feather,
    })
}

fn retouch_list(settings: &Settings) -> Vec<Value> {
    settings.value("retouch").as_array().cloned().unwrap_or_default()
}

/// Port of `stroke_valid`: the pending stroke still belongs to the photo and settings it started on.
fn stroke_valid(cx: &AppContext) -> bool {
    let Some(state) = cx.state_ref::<HealingState>() else { return false };
    state.pending.as_ref().is_some_and(|p| p.photo == cx.doc.current_id() && settings_eq(&p.expected, cx.settings()))
}

/// Keeps the tool in step with the window: a photo change cancels the staged marks and the manual source
/// (`load_settings`), and opening the tool picks the healing brush (`select_tool(6)` → `mode('heal')`). Called
/// by the panel, the tool and the commands before they act.
pub fn sync(cx: &mut AppContext) {
    let current = cx.doc.current_id();
    let state = st(cx);
    if state.context.is_some_and(|c| c != current) {
        clear_healing(cx);
    }
    let state = st(cx);
    if state.photo != current {
        state.photo = current;
        state.donor = None;
        state.manual_source = false;
    }
    if cx.tool() != Tool::Healing {
        return;
    }
    let pass = cx.egui.cumulative_pass_nr();
    let seen = st(cx).seen.replace(Some(pass));
    if seen.is_none_or(|s| pass > s + 1) {
        open(cx);
    }
    update_overlay(cx);
}

/// Port of `tab_changed` followed by `mode('heal')`: what selecting the strip's 복구 does.
pub fn open(cx: &mut AppContext) {
    let pass = cx.egui.cumulative_pass_nr();
    st(cx).seen.set(Some(pass));
    set_tool(cx, None);
    update_overlay(cx);
    set_mode(cx, Some(HEAL));
}

/// Port of `heal_shortcut` (Q): opens the Healing tool with the healing brush.
pub fn heal_shortcut(cx: &mut AppContext) {
    if photo(cx).is_none() {
        return;
    }
    if cx.tool() != Tool::Healing {
        cx.open_tool(Tool::Healing);
        open(cx);
    }
    if st(cx).mode != Some(HEAL) {
        set_mode(cx, Some(HEAL));
    }
}

/// Port of `PhotoView.set_tool` for this tool: cancels the stroke and sets the mode without the rest of `mode`.
fn set_tool(cx: &mut AppContext, mode: Option<&'static str>) {
    view_cancel_stroke(cx);
    let state = st(cx);
    state.mode = mode;
    state.points.clear();
    state.overlay = None;
    state.texture = None;
}

/// Port of `StudioTools.mode` for the retouch kinds and `''` (도구 끝내기).
pub fn set_mode(cx: &mut AppContext, kind: Option<&'static str>) {
    if photo(cx).is_none() {
        return;
    }
    if kind.is_some() && cx.before() {
        cx.set_before(false);
    }
    cx.set_mode(Mode::Develop);
    set_tool(cx, kind);
    if kind.is_some() {
        let state = st(cx);
        state.brush_radius = state.radius_value();
        state.brush_feather = state.feather as f64;
    }
    update_overlay(cx);
    let message = match kind {
        Some(HEAL) => "여러 곳을 칠한 뒤 실행 또는 Enter를 누르세요. Alt+클릭: 원본 지정 · [ / ]: 크기",
        Some(_) => "사진에서 드래그하세요. Alt+클릭: 복구 원본 지정",
        None => "도구를 종료했습니다.",
    };
    cx.set_status(tr(message));
}

/// Port of `PhotoView.cancel_stroke`: drops the points of the stroke being drawn.
fn view_cancel_stroke(cx: &mut AppContext) {
    let state = st(cx);
    if !state.points.is_empty() {
        state.points.clear();
        cancel_stroke(cx);
    }
}

/// Port of `StudioTools.cancel_stroke`. A clone, inpaint or red eye stroke shown live is taken off the settings.
pub fn cancel_stroke(cx: &mut AppContext) {
    let valid = stroke_valid(cx);
    let state = st(cx);
    let Some(pending) = state.pending.take() else { return };
    state.points.clear();
    if pending.staged {
        state.generation += 1;
        update_overlay(cx);
    } else if valid {
        cx.doc.edit_mut().set_all(pending.base);
    }
}

/// Port of `preview_stroke`: the stroke grows (a press or a move).
fn preview_stroke(cx: &mut AppContext) {
    let state = st(cx);
    let (Some(kind), points, alt) = (state.mode, state.points.clone(), state.alt) else { return };
    if photo(cx).is_none() || points.is_empty() {
        return;
    }
    if alt || (kind == CLONE && st(cx).donor.is_none()) {
        return;
    }
    if st(cx).pending.is_some() && !stroke_valid(cx) {
        view_cancel_stroke(cx);
        return;
    }
    let source = source_points(cx, &points);
    if source.is_empty() {
        return;
    }
    if st(cx).pending.is_none() {
        cx.doc.edit_mut().commit("보정 변경");
        let base = cx.settings().clone();
        let component = make_retouch(st(cx), kind, &source);
        st(cx).pending = Some(PendingStroke {
            photo: cx.doc.current_id(),
            expected: base.clone(),
            base,
            staged: kind == HEAL,
            component,
        });
    } else if let Some(p) = st(cx).pending.as_mut() {
        p.component["points"] = points_value(&source);
    }
    if kind == HEAL {
        st(cx).generation += 1;
        update_overlay(cx);
        return;
    }
    // `preview_settings`: the operation drawn live on top of the stroke's base settings.
    let Some(pending) = st(cx).pending.as_mut() else { return };
    let mut live = pending.base.clone();
    let mut list = retouch_list(&live);
    list.push(pending.component.clone());
    live.set("retouch", Value::Array(list));
    pending.expected = live.clone();
    cx.doc.edit_mut().set_all(live);
}

/// Port of `stroke`: the stroke is finished (release).
fn stroke(cx: &mut AppContext, points: &[[f64; 2]], alt: bool) {
    let Some(kind) = st(cx).mode else { return };
    if photo(cx).is_none() {
        return;
    }
    if st(cx).pending.is_some() && !stroke_valid(cx) {
        cancel_stroke(cx);
        return;
    }
    let frozen = st(cx).pending.as_ref().map(|p| p.component.clone());
    cancel_stroke(cx);
    let source = source_points(cx, points);
    if source.is_empty() {
        cx.set_status(tr("변형 후 생긴 빈 가장자리 대신 사진 안쪽을 선택하세요."));
        return;
    }
    if alt {
        let state = st(cx);
        state.donor = Some(source[0]);
        state.manual_source = true;
        cx.set_status(tr("복구 원본 위치를 지정했습니다."));
        return;
    }
    if kind == CLONE && st(cx).donor.is_none() {
        cx.set_status(tr("Alt+클릭으로 복구 원본 위치부터 지정하세요."));
        return;
    }
    let mut operation = frozen.unwrap_or_else(|| make_retouch(st(cx), kind, &source));
    operation["points"] = points_value(&source);
    if kind == HEAL {
        let current = cx.doc.current_id();
        let shape = photo(cx).map(|(_, (w, h))| (h, w));
        let state = st(cx);
        state.context = Some(current);
        state.overlay_key = None;
        if let (Some(mask), Some(shape)) = (state.mask.as_mut(), shape) {
            if let Ok(added) = stroke_mask(shape, &operation) {
                mask.zip_mut_with(&added, |m, a| *m = m.max(*a));
            }
        }
        state.staged.push(operation);
        update_overlay(cx);
        return;
    }
    let mut values = retouch_list(cx.settings());
    values.push(operation);
    cx.set_before(false);
    let edit = cx.doc.edit_mut();
    edit.set("retouch", Value::Array(values));
    edit.commit("부분 보정");
}

/// `brush_mask(shape, op['points'], op['radius'], op['feather'])`.
fn stroke_mask(shape: (usize, usize), op: &Value) -> Result<Plane, grainy_local::Error> {
    let points: Vec<[f64; 2]> = op["points"]
        .as_array()
        .map(|list| list.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect())
        .unwrap_or_default();
    let radius = op["radius"].as_f64().unwrap_or(0.015);
    let feather = op["feather"].as_f64().unwrap_or(60.0);
    grainy_local::brush_mask(shape, &points, radius, feather)
}

/// Port of `apply_healing` (실행 · 표시한 곳 제거, Enter): the staged strokes become one undo step.
pub fn apply_healing(cx: &mut AppContext) {
    if photo(cx).is_none() {
        return;
    }
    let current = cx.doc.current_id();
    if st(cx).context != Some(current) {
        clear_healing(cx);
        return;
    }
    view_cancel_stroke(cx);
    cancel_stroke(cx);
    if st(cx).staged.is_empty() {
        return;
    }
    cx.doc.edit_mut().commit("보정 변경");
    let staged = st(cx).staged.clone();
    let count = staged.len().to_string();
    let mut values = retouch_list(cx.settings());
    values.extend(staged);
    clear_healing(cx);
    cx.set_before(false);
    let label = tr_with("힐링 {0}개 실행", &[&count]);
    let edit = cx.doc.edit_mut();
    edit.set("retouch", Value::Array(values));
    edit.commit(&label);
    cx.set_status(tr_with("표시한 {0}곳에 힐링을 적용했습니다. 한 번의 실행 취소로 되돌릴 수 있습니다.", &[&count]));
}

/// Port of `clear_healing` (표시 모두 취소).
pub fn clear_healing(cx: &mut AppContext) {
    view_cancel_stroke(cx);
    cancel_stroke(cx);
    let state = st(cx);
    state.staged.clear();
    state.context = None;
    state.mask = None;
    state.overlay_key = None;
    state.overlay = None;
    state.texture = None;
}

/// Port of `remove_healing` (마지막 표시 취소).
pub fn remove_healing(cx: &mut AppContext) {
    view_cancel_stroke(cx);
    cancel_stroke(cx);
    let state = st(cx);
    state.staged.pop();
    state.mask = None;
    state.overlay_key = None;
    update_overlay(cx);
}

/// Port of `remove_retouch` (마지막 복구 제거): one undo step.
pub fn remove_retouch(cx: &mut AppContext) {
    if photo(cx).is_none() {
        return;
    }
    let mut values = retouch_list(cx.settings());
    if values.pop().is_some() {
        cancel_stroke(cx);
        cx.set_before(false);
        let edit = cx.doc.edit_mut();
        edit.set("retouch", Value::Array(values));
        edit.commit("보정 변경");
    }
}

/// Port of `automatic_healing_source` (힐링 원본 자동 선택으로 복귀).
pub fn automatic_source(cx: &mut AppContext) {
    view_cancel_stroke(cx);
    let state = st(cx);
    state.donor = None;
    state.manual_source = false;
}

/// Port of `retouch_cursor`: the cursor follows the sliders while a retouch kind is in use.
fn retouch_cursor(state: &mut HealingState) {
    if state.mode.is_some() {
        state.brush_radius = state.radius_value();
        state.brush_feather = state.feather as f64;
    }
}

/// The 복구 브러시 크기 slider moved to `position`.
pub fn set_radius_position(cx: &mut AppContext, position: i64) {
    let state = st(cx);
    let position = position.clamp(RADIUS_RANGE.0, RADIUS_RANGE.1);
    if position != state.radius {
        state.radius = position;
        retouch_cursor(state);
    }
}

/// The 복구 경계 흐림 slider moved to `value`.
pub fn set_feather(cx: &mut AppContext, value: i64) {
    let state = st(cx);
    let value = value.clamp(FEATHER_RANGE.0, FEATHER_RANGE.1);
    if value != state.feather {
        state.feather = value;
        retouch_cursor(state);
    }
}

/// Port of `PhotoView.resize_brush` with `set_brush_size` for the retouch kinds (`[` and `]`): `false` when no
/// retouch kind is in use (the mask brush's size is the masks package's).
pub fn resize_brush(cx: &mut AppContext, bigger: bool) -> bool {
    let state = st(cx);
    if state.mode.is_none() {
        return false;
    }
    let factor = if bigger { 1.2 } else { 1.0 / 1.2 };
    state.brush_radius = (state.brush_radius * factor).clamp(0.002, 0.12);
    let position = py_round(state.brush_radius * 1000.0);
    set_radius_position(cx, position);
    true
}

/// Port of `update_healing_overlay`: the coverage of the staged strokes (and the staged stroke being drawn),
/// carried to the displayed frame. Coverage only: nothing is healed while marking.
pub fn update_overlay(cx: &mut AppContext) {
    let valid = stroke_valid(cx);
    let before = cx.before();
    let live = photo(cx);
    let state = st(cx);
    let draft = state.pending.as_ref().filter(|p| valid && p.staged).map(|p| p.component.clone());
    if live.is_none() || state.mode != Some(HEAL) || before || (state.staged.is_empty() && draft.is_none()) {
        state.overlay = None;
        state.overlay_key = None;
        state.texture = None;
        return;
    }
    let Some((_, (w, h))) = live else { return };
    let Some(map_key) = ensure_map(cx) else { return };
    let state = st(cx);
    let key = (map_key, state.generation, state.staged.len());
    if state.overlay_key.as_ref() == Some(&key) && state.overlay.is_some() {
        return;
    }
    if state.mask.is_none() {
        let mut mask = Plane::zeros((h, w));
        for op in &state.staged {
            if let Ok(added) = stroke_mask((h, w), op) {
                mask.zip_mut_with(&added, |m, a| *m = m.max(*a));
            }
        }
        state.mask = Some(mask);
    }
    let Some(base) = state.mask.as_ref() else { return };
    let mut mask = base.clone();
    if let Some(op) = &draft {
        if let Ok(added) = stroke_mask((h, w), op) {
            mask.zip_mut_with(&added, |m, a| *m = m.max(*a));
        }
    }
    let Some((_, map)) = state.map.as_ref() else { return };
    let overlay = coverage(map, &mask);
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [overlay.width, overlay.height],
        &overlay.alpha.iter().flat_map(|a| [255, 80, 65, *a]).collect::<Vec<u8>>(),
    );
    let texture = cx.egui.load_texture("healing-overlay", image, egui::TextureOptions::LINEAR);
    let state = st(cx);
    state.overlay = Some(overlay);
    state.overlay_key = Some(key);
    state.texture = Some(texture);
}

/// `cv2.remap(mask, x*(w-1), y*(h-1), INTER_LINEAR)` over the frame's inverse map (outside the source: 0), then
/// `np.where(valid > .9, coverage, 0)` and the alpha `uint8(clip(coverage, 0, 1) * 140)`. OpenCV's bilinear
/// weights are quantized to 1/32 of a pixel; so are these.
fn coverage(map: &CoordinateMap, mask: &Plane) -> Overlay {
    let (fw, fh) = map.frame_size();
    let (h, w) = mask.dim();
    let at = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            mask[[y as usize, x as usize]]
        }
    };
    let mut alpha = vec![0u8; fw * fh];
    let (dx, dy) = ((fw.max(2) - 1) as f64, (fh.max(2) - 1) as f64);
    for y in 0..fh {
        for x in 0..fw {
            let Some(p) = map.source_point([x as f64 / dx, y as f64 / dy]) else { continue };
            let sx = ((p[0] * (w as f64 - 1.0)) as f32 * 32.0).round() as i64;
            let sy = ((p[1] * (h as f64 - 1.0)) as f32 * 32.0).round() as i64;
            let (x0, y0) = (sx >> 5, sy >> 5);
            let (fx, fy) = ((sx & 31) as f32 / 32.0, (sy & 31) as f32 / 32.0);
            let value = at(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + at(x0 + 1, y0) * fx * (1.0 - fy)
                + at(x0, y0 + 1) * (1.0 - fx) * fy
                + at(x0 + 1, y0 + 1) * fx * fy;
            alpha[y * fw + x] = (value.clamp(0.0, 1.0) * 140.0) as u8;
        }
    }
    Overlay { width: fw, height: fh, alpha }
}

// ---- the view's part: strokes and keys -----------------------------------------------------------------------

/// The pointer is pressed on the photo at frame point `point` (`mousePressEvent`). Returns whether a stroke
/// started.
pub fn press(cx: &mut AppContext, point: [f64; 2], alt: bool) -> bool {
    sync(cx);
    if st(cx).mode.is_none() || photo(cx).is_none() {
        return false;
    }
    let state = st(cx);
    state.points = vec![point];
    state.alt = alt;
    preview_stroke(cx);
    true
}

/// The pointer moved while pressed (`mouseMoveEvent`).
pub fn move_to(cx: &mut AppContext, point: [f64; 2]) -> bool {
    let state = st(cx);
    if state.mode.is_none() || state.points.is_empty() {
        return false;
    }
    if state.points.last() != Some(&point) {
        state.points.push(point);
    }
    preview_stroke(cx);
    true
}

/// The pointer was released (`mouseReleaseEvent`): the stroke is finished.
pub fn release(cx: &mut AppContext, point: [f64; 2]) -> bool {
    let state = st(cx);
    if state.mode.is_none() || state.points.is_empty() {
        return false;
    }
    if state.points.last() != Some(&point) {
        state.points.push(point);
    }
    let points = std::mem::take(&mut state.points);
    let alt = state.alt;
    stroke(cx, &points, alt);
    true
}

/// Port of `keyPressEvent` for the tool: Esc cancels the stroke, then the staged marks, then ends the tool; Enter
/// applies the staged marks. Returns whether the key was used.
pub fn key(cx: &mut AppContext, key: Key) -> bool {
    sync(cx);
    let Some(mode) = st(cx).mode else { return false };
    match key {
        Key::Escape => {
            if !st(cx).points.is_empty() {
                view_cancel_stroke(cx);
            } else if mode == HEAL && (!st(cx).staged.is_empty() || st(cx).overlay.is_some()) {
                clear_healing(cx);
            } else {
                set_tool(cx, None);
            }
            true
        }
        Key::Enter if mode == HEAL => {
            apply_healing(cx);
            true
        }
        Key::OpenBracket | Key::CloseBracket => resize_brush(cx, key == Key::CloseBracket),
        _ => false,
    }
}

/// The Healing tool on the photo view.
#[derive(Default)]
pub struct Healing;

impl CanvasTool for Healing {
    fn name(&self) -> &'static str {
        "healing"
    }

    fn tool(&self) -> Option<Tool> {
        Some(Tool::Healing)
    }

    fn edits(&self) -> &'static [&'static str] {
        &["retouch"]
    }

    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        let inside = (0.0..=1.0).contains(&event.free[0]) && (0.0..=1.0).contains(&event.free[1]);
        match event.kind {
            PointerKind::Press => {
                if !inside {
                    return false;
                }
                // The stroke starts where the button went down, not where egui recognised the drag.
                let origin = cx.egui.input(|i| i.pointer.press_origin());
                let start = match (origin, cx.photo_rect()) {
                    (Some(origin), Some(rect)) => ViewTransform { rect }.to_frame(origin),
                    _ => event.frame,
                };
                let used = press(cx, start, event.modifiers.alt);
                if used && start != event.frame {
                    move_to(cx, event.frame);
                }
                used
            }
            PointerKind::Drag => move_to(cx, event.frame),
            PointerKind::Release => release(cx, event.frame),
            PointerKind::Click => {
                if !inside {
                    return false;
                }
                let used = press(cx, event.frame, event.modifiers.alt);
                release(cx, event.frame) || used
            }
            PointerKind::Hover => {
                sync(cx);
                st(cx).mode.is_some() && inside
            }
        }
    }

    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        self::key(cx, key)
    }

    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        let Some(state) = cx.state_ref::<HealingState>() else { return };
        state.seen.set(Some(cx.egui.cumulative_pass_nr()));
        let Some(mode) = state.mode else { return };
        if mode == HEAL {
            if let Some(texture) = &state.texture {
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(texture.id(), view.rect, uv, Color32::WHITE);
            }
        }
        // The brush cursor: the outer circle and the dotted start of the feather.
        let Some(pointer) = painter.ctx().pointer_hover_pos() else { return };
        if !view.rect.contains(pointer) {
            return;
        }
        let radius = (state.brush_radius as f32) * view.rect.width().min(view.rect.height());
        let pen = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 210));
        painter.circle_stroke(pointer, radius, pen);
        let inner = radius * (1.0 - state.brush_feather as f32 / 100.0);
        if inner > 0.5 {
            let steps = 64;
            let circle: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                    pointer + egui::vec2(a.cos(), a.sin()) * inner
                })
                .collect();
            painter.extend(Shape::dashed_line(&circle, pen, 1.0, 2.0));
        }
    }
}
