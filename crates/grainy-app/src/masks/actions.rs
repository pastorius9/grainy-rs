//! The mask methods of luma/studio.py `StudioTools` on the application context, with the oracle's commits
//! (`self.w.commit()` → `commit("보정 변경")`, `finish_interaction` → the same, labelled commits as the oracle
//! labels them). The selection (`layer_index`, `component_index`), the tool mode and the handles live in
//! [`MaskState`]; the layers live in the settings key `masks`.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::gradient;
use super::layers::{self, Obj};
use super::{MaskState, Mode, Operation, Pending, SKY_DEFAULTS};
use crate::context::AppContext;
use crate::imagespace::CoordinateMap;
use crate::panels::Tool;
use crate::text::{tr, tr_with};

/// The mask state, created on first use; another photo starts again at its first mask (the tool options and the
/// overlay switch stay).
pub fn state(cx: &mut AppContext) -> &mut MaskState {
    let photo = cx.doc.current_id();
    let s = cx.state::<MaskState>();
    if s.photo != photo {
        let options = s.options.clone();
        let overlay_off = s.overlay_off;
        let seen = s.seen;
        *s = MaskState { photo, options, overlay_off, seen, ..MaskState::default() };
    }
    s
}

/// Keeps the selection inside the list (`load_layer`: `layer_index` past the end goes to the last mask).
pub fn sync(cx: &mut AppContext) {
    let len = layers::list(cx.settings()).len() as isize;
    let s = state(cx);
    if s.layer >= len {
        s.layer = len - 1;
    }
}

/// The settings' layers.
pub fn masks(cx: &AppContext) -> Vec<Value> {
    layers::list(cx.settings())
}

/// Port of `current_layer`.
pub fn current_layer(cx: &mut AppContext) -> Option<Obj> {
    let index = state(cx).layer;
    layers::at(&masks(cx), index).cloned()
}

/// Port of `current_component`.
pub fn current_component(cx: &mut AppContext) -> Option<Obj> {
    let (layer, component) = {
        let s = state(cx);
        (s.layer, s.component)
    };
    layers::component_at(&masks(cx), layer, component).cloned()
}

/// The coordinate map of the open photo on its live source (`source_point`'s `coordinates`), rebuilt when the
/// geometry changes.
pub fn coordinate_map(cx: &mut AppContext) -> Option<Arc<CoordinateMap>> {
    let photo = cx.doc.current()?;
    let (h, w) = super::overlay::live_shape(photo.original?);
    let effective = grainy_core::effective(cx.settings());
    let key =
        format!("{}|{w}x{h}|{}", photo.id, grainy_engine::settings_key(&effective, &grainy_engine::GEOMETRY_KEYS));
    if let Some((k, map)) = &state(cx).map {
        if *k == key {
            return Some(map.clone());
        }
    }
    let map = match CoordinateMap::new(w, h, cx.settings()) {
        Ok(map) => Arc::new(map),
        Err(error) => {
            log::warn!("mask coordinates: {error}");
            return None;
        }
    };
    state(cx).map = Some((key, map.clone()));
    Some(map)
}

fn commit(cx: &mut AppContext, label: &str) {
    cx.doc.edit_mut().commit(label);
}

/// `finish_interaction`: the edit is one undo step.
fn finish(cx: &mut AppContext) {
    commit(cx, "보정 변경");
}

/// `set_setting('masks', layers)`: a stroke being drawn is cancelled first, as the oracle's `set_setting` does.
pub fn set_masks(cx: &mut AppContext, masks: Vec<Value>) {
    cancel_stroke(cx);
    cx.doc.edit_mut().set("masks", Value::Array(masks));
    cx.set_before(false);
}

/// Port of `pause_overlay`.
pub fn pause_overlay(cx: &mut AppContext) {
    let s = state(cx);
    if !s.overlay_paused && s.overlay_shown() {
        s.overlay_paused = true;
    }
}

/// Port of `new_layer`.
pub fn new_layer(cx: &mut AppContext) {
    commit(cx, "보정 변경");
    let mut list = masks(cx);
    let name = tr_with("마스크 {0}", &[&(list.len() + 1).to_string()]);
    list.push(layers::new_layer(&name));
    let len = list.len() as isize;
    set_masks(cx, list);
    let s = state(cx);
    s.layer = len - 1;
    s.component = -1;
}

/// Port of `make_component`: a component of `kind` from the tool options.
pub fn make_component(state: &MaskState, kind: &str, points: &[[f64; 2]], alt: bool) -> Obj {
    let o = &state.options;
    let mut c = Map::new();
    c.insert("type".into(), Value::from(kind));
    c.insert("points".into(), points_value(points));
    c.insert("radius".into(), Value::from(o.radius as f64 / 1000.0));
    c.insert("feather".into(), Value::from(o.feather));
    c.insert("operation".into(), Value::from(state.operation.as_str()));
    if kind == "brush" {
        c.insert("paint_version".into(), Value::from(2));
        c.insert("flow".into(), Value::from(o.flow));
        c.insert("density".into(), Value::from(o.density));
        if alt {
            c.insert("operation".into(), Value::from("subtract"));
        }
        if o.auto_mask {
            c.insert("auto_mask".into(), Value::Bool(true));
            c.insert("auto_tolerance".into(), Value::from(o.auto_tolerance as f64 / 100.0));
            if let Some(seed) = points.first() {
                c.insert("seed".into(), point_value(*seed));
            }
        }
    }
    c
}

fn point_value(p: [f64; 2]) -> Value {
    Value::Array(vec![Value::from(p[0]), Value::from(p[1])])
}

fn points_value(points: &[[f64; 2]]) -> Value {
    Value::Array(points.iter().map(|p| point_value(*p)).collect())
}

/// Port of `add_component`: to the selected mask (a new one when none is selected), brush strokes inside its
/// brush range; the new component is selected.
pub fn add_component(cx: &mut AppContext, mut component: Obj) {
    if current_layer(cx).is_none() {
        new_layer(cx);
    }
    let (layer, operation, brush_range) = {
        let s = state(cx);
        (s.layer, s.operation, s.options.brush_range)
    };
    component.entry("operation").or_insert_with(|| Value::from(operation.as_str()));
    let mut list = masks(cx);
    let Some(target) = layers::at_mut(&mut list, layer) else { return };
    let brush = component.get("type").and_then(Value::as_str) == Some("brush");
    let existing = layers::components(target).to_vec();
    let (components, index) = if brush {
        layers::ranged(&existing, Value::Object(component), brush_range)
    } else {
        let mut c = existing;
        c.push(Value::Object(component));
        let n = c.len();
        (c, n - 1)
    };
    *layers::components_mut(target) = components;
    set_masks(cx, list);
    state(cx).component = index as isize;
    sync(cx);
}

/// The settings with the gradient being drawn (`preview_settings` for a mask stroke).
fn preview_masks(base: &Value, pending: &Pending) -> Value {
    let mut list = base.as_array().cloned().unwrap_or_default();
    if layers::at(&list, pending.layer).is_none() {
        list.push(layers::preview_layer());
    }
    let index = if layers::at(&list, pending.layer).is_some() { pending.layer } else { list.len() as isize - 1 };
    if let Some(layer) = layers::at_mut(&mut list, index) {
        layers::components_mut(layer).push(Value::Object(pending.component.clone()));
    }
    Value::Array(list)
}

/// Port of `preview_stroke` for gradients: the first call commits what was there and starts the pending
/// component; the picture shows it live (the settings' masks hold it until the release or Esc).
pub fn preview_stroke(cx: &mut AppContext, points: &[[f64; 2]]) {
    if cx.doc.current().is_none() || points.is_empty() {
        return;
    }
    let mode = state(cx).mode;
    if !mode.gradient() {
        return;
    }
    let Some(map) = coordinate_map(cx) else { return };
    let Some(shape) = gradient::gradient_shape(&map, mode.as_str(), points) else { return };
    let photo = cx.doc.current_id();
    if state(cx).pending.as_ref().is_some_and(|p| p.photo != photo) {
        cancel_stroke(cx);
        return;
    }
    state(cx).overlay_paused = false;
    if state(cx).pending.is_none() {
        commit(cx, "보정 변경");
        let base = cx.settings().value("masks").clone();
        let s = state(cx);
        let source = shape_points(&shape);
        let component = make_component(s, mode.as_str(), &source, false);
        s.pending = Some(Pending { photo, base, layer: s.layer, component });
    }
    let live = {
        let s = state(cx);
        let Some(pending) = s.pending.as_mut() else { return };
        for (k, v) in shape {
            pending.component.insert(k, v);
        }
        preview_masks(&pending.base, pending)
    };
    cx.doc.edit_mut().set("masks", live);
}

fn shape_points(shape: &Obj) -> Vec<[f64; 2]> {
    shape
        .get("points")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect())
        .unwrap_or_default()
}

/// Port of `cancel_stroke`: the gradient being drawn is dropped and the masks are as before it.
pub fn cancel_stroke(cx: &mut AppContext) {
    let s = state(cx);
    s.points.clear();
    let Some(pending) = s.pending.take() else { return };
    if pending.photo == cx.doc.current_id() {
        cx.doc.edit_mut().set("masks", pending.base);
    }
}

/// Port of `stroke` for linear and radial gradients: the component from the first to the last point (as the press
/// made it, with the final shape), added to the selected mask and committed as 부분 보정.
pub fn stroke(cx: &mut AppContext, points: &[[f64; 2]], alt: bool) {
    if cx.doc.current().is_none() {
        return;
    }
    let mode = state(cx).mode;
    if !mode.gradient() {
        return;
    }
    let frozen = state(cx).pending.as_ref().map(|p| p.component.clone());
    cancel_stroke(cx);
    let Some(map) = coordinate_map(cx) else { return };
    let Some(shape) = gradient::gradient_shape(&map, mode.as_str(), points) else { return };
    let source = shape_points(&shape);
    let mut component = frozen.unwrap_or_else(|| make_component(state(cx), mode.as_str(), &source, alt));
    component.insert("points".into(), points_value(&source));
    for (k, v) in shape {
        component.insert(k, v);
    }
    add_component(cx, component);
    commit(cx, "부분 보정");
}

/// Port of `set_tool` of the view with `mask_tool_changed`: the stroke and the handles end; leaving the tool
/// drops a new mask that never received a component.
pub fn set_tool(cx: &mut AppContext, mode: Mode) {
    cancel_stroke(cx);
    let s = state(cx);
    s.mode = mode;
    s.points.clear();
    s.handles.clear();
    s.handle_drag = None;
    if mode == Mode::None {
        drop_fresh_layer(cx);
    }
}

/// Port of `StudioTools.mode` for the mask kinds: the view's tool and the status line.
pub fn mode(cx: &mut AppContext, kind: Mode) {
    if cx.doc.current().is_none() {
        return;
    }
    // Work package 9: the pick generation, the 색상 범위 추가 guard, the pick note and the brush size.
    if !crate::tools::range_pick::mode_starts(cx, kind) {
        return;
    }
    if kind.picking() && cx.before() {
        cx.set_before(false);
    }
    if kind.drawing() {
        state(cx).overlay_paused = false;
    }
    cx.wb_picker = false;
    set_tool(cx, kind);
    let message = match kind {
        Mode::Object => "사진 안에서 개체를 둘러싸 주세요.",
        Mode::Brush => "사진에서 드래그하세요. Alt: 지우기 · [ / ]: 크기",
        Mode::None => "도구를 종료했습니다.",
        _ => "사진에서 드래그하세요. Alt+클릭: 복구 원본 지정",
    };
    cx.set_status(tr(message));
    if kind.picking() {
        cx.set_status(tr("사진에서 클릭하거나 사각형을 드래그하세요."));
    }
}

/// Port of `drop_fresh_layer`.
pub fn drop_fresh_layer(cx: &mut AppContext) {
    let Some(index) = state(cx).fresh_layer.take() else { return };
    if cx.doc.current().is_none() {
        return;
    }
    let mut list = masks(cx);
    match layers::at(&list, index) {
        Some(layer) if layers::components(layer).is_empty() => {}
        _ => return,
    }
    list.remove(index as usize);
    let len = list.len() as isize;
    let s = state(cx);
    s.layer = s.layer.min(len - 1).max(0);
    s.component = -1;
    set_masks(cx, list);
    finish(cx);
    sync(cx);
}

/// Port of `create_mask`: a new mask whose first component is drawn with the chosen tool (the sky is added at
/// once).
pub fn create_mask(cx: &mut AppContext, kind: &str) {
    if cx.doc.current().is_none() {
        return;
    }
    if super::selection::choose(cx, kind, true, Operation::Add) {
        return;
    }
    if kind == "sky" {
        mode(cx, Mode::None);
        new_layer(cx);
        add_sky(cx, Operation::Add);
        return;
    }
    new_layer(cx);
    let s = state(cx);
    s.fresh_layer = Some(s.layer);
    s.operation = Operation::Add;
    mode(cx, Mode::from_kind(kind));
}

/// Port of `extend_mask`: add to, subtract from or intersect the selected mask with a component of this kind.
pub fn extend_mask(cx: &mut AppContext, kind: &str, operation: Operation) {
    if current_layer(cx).is_none() {
        create_mask(cx, kind);
        return;
    }
    if super::selection::choose(cx, kind, false, operation) {
        return;
    }
    if kind == "sky" {
        commit(cx, "보정 변경");
        add_sky(cx, operation);
        return;
    }
    state(cx).operation = operation;
    mode(cx, Mode::from_kind(kind));
}

/// Port of `add_sky` (the component with the detection defaults; its editor is work package 9).
pub fn add_sky(cx: &mut AppContext, operation: Operation) {
    if cx.doc.current().is_none() {
        return;
    }
    state(cx).fresh_layer = None;
    mode(cx, Mode::None);
    state(cx).operation = operation;
    let mut component = Map::new();
    component.insert("type".into(), Value::from("sky"));
    for (key, value) in SKY_DEFAULTS {
        component.insert(key.into(), Value::from(value));
    }
    add_component(cx, component);
    commit(cx, "하늘 선택");
    cx.set_status(tr("하늘을 선택했습니다. 민감도와 경계 흐림으로 다듬으세요."));
}

/// Port of `mask_shortcut` (K, M, Shift+M): the Masks tool opens and New Mask › kind runs; a new mask without a
/// stroke yet is used again.
pub fn mask_shortcut(cx: &mut AppContext, kind: &str) {
    if cx.doc.current().is_none() {
        return;
    }
    cx.open_tool(Tool::Masks);
    sync(cx);
    let list = masks(cx);
    let (fresh, layer) = {
        let s = state(cx);
        (s.fresh_layer, s.layer)
    };
    let reuse =
        fresh.is_some_and(|f| f == layer && layers::at(&list, f).is_some_and(|l| layers::components(l).is_empty()));
    if reuse {
        mode(cx, Mode::from_kind(kind));
    } else {
        create_mask(cx, kind);
    }
}

/// Port of `toggle_overlay` (O).
pub fn toggle_overlay(cx: &mut AppContext) {
    if cx.tool() != Tool::Masks {
        return;
    }
    let s = state(cx);
    if s.overlay_paused && s.overlay_shown() {
        s.overlay_paused = false;
    } else {
        s.overlay_off = !s.overlay_off;
        s.overlay_paused = false;
    }
}

/// Port of `layer_property`: one value of the selected mask, without a commit (sliders commit on release).
pub fn layer_property(cx: &mut AppContext, key: &str, value: Value) {
    let index = state(cx).layer;
    let mut list = masks(cx);
    let Some(layer) = layers::at_mut(&mut list, index) else { return };
    layer.insert(key.into(), value);
    set_masks(cx, list);
}

/// Port of `local_changed`: one adjustment of the selected mask.
pub fn local_changed(cx: &mut AppContext, key: &str, value: f64) {
    let Some(layer) = current_layer(cx) else { return };
    pause_overlay(cx);
    let mut values = layer.get("adjustments").and_then(Value::as_object).cloned().unwrap_or_default();
    values.insert(key.into(), Value::from(value));
    layer_property(cx, "adjustments", Value::Object(values));
}

/// Port of `rename_layer` once the name is typed (`ok and text.strip()`).
pub fn rename_layer(cx: &mut AppContext, text: &str) {
    let text = text.trim();
    if current_layer(cx).is_none() || text.is_empty() {
        return;
    }
    layer_property(cx, "name", Value::from(text));
}

/// Port of `remove_layer`.
pub fn remove_layer(cx: &mut AppContext) {
    if current_layer(cx).is_none() {
        return;
    }
    let mut list = masks(cx);
    let s = state(cx);
    list.remove(s.layer as usize);
    s.layer = (s.layer - 1).max(0);
    set_masks(cx, list);
    finish(cx);
    sync(cx);
}

/// Port of `select_layer`.
pub fn select_layer(cx: &mut AppContext, index: isize) {
    state(cx).overlay_paused = false;
    commit(cx, "보정 변경");
    let s = state(cx);
    s.layer = index;
    s.component = -1;
    set_tool(cx, Mode::None);
}

/// Port of `select_component`.
pub fn select_component(cx: &mut AppContext, index: isize) {
    state(cx).overlay_paused = false;
    commit(cx, "보정 변경");
    state(cx).component = index;
    if state(cx).mode == Mode::EditComponent {
        edit_component_handles(cx);
    }
}

/// Port of `tree_selected` for the row (layer, component): a gradient is ready to drag at once.
pub fn tree_selected(cx: &mut AppContext, layer: isize, component: isize) {
    if layer != state(cx).layer {
        select_layer(cx, layer);
    }
    if component != state(cx).component {
        select_component(cx, component);
    }
    let gradient = current_component(cx)
        .is_some_and(|c| matches!(c.get("type").and_then(Value::as_str), Some("linear" | "radial")));
    if gradient && matches!(state(cx).mode, Mode::None | Mode::EditComponent) {
        edit_component_handles(cx);
    }
}

/// Port of `set_layer_enabled` (the check box of a mask, 마스크 숨기기 / 표시).
pub fn set_layer_enabled(cx: &mut AppContext, index: isize, enabled: bool) {
    let mut list = masks(cx);
    let Some(layer) = layers::at_mut(&mut list, index) else { return };
    if layers::truthy(layer.get("enabled"), true) == enabled {
        return;
    }
    layer.insert("enabled".into(), Value::Bool(enabled));
    commit(cx, "보정 변경");
    set_masks(cx, list);
    finish(cx);
}

/// Port of `remove_component`.
pub fn remove_component(cx: &mut AppContext) {
    let (layer, component) = {
        let s = state(cx);
        (s.layer, s.component)
    };
    let mut list = masks(cx);
    let Some(target) = layers::at_mut(&mut list, layer) else { return };
    let components = layers::components_mut(target);
    let Ok(at) = usize::try_from(component) else { return };
    if at >= components.len() {
        return;
    }
    commit(cx, "보정 변경");
    components.remove(at);
    let len = components.len() as isize;
    state(cx).component = component.min(len - 1);
    set_masks(cx, list);
    finish(cx);
    sync(cx);
}

/// Port of `replace_component`.
pub fn replace_component(cx: &mut AppContext, component: Obj) {
    if current_component(cx).is_none() {
        return;
    }
    commit(cx, "보정 변경");
    let (layer, index) = {
        let s = state(cx);
        (s.layer, s.component)
    };
    let mut list = masks(cx);
    if let Some(target) = layers::at_mut(&mut list, layer) {
        if let Some(slot) = layers::components_mut(target).get_mut(index as usize) {
            *slot = Value::Object(component);
        }
    }
    set_masks(cx, list);
    finish(cx);
    sync(cx);
}

/// Port of `edit_component` (세부 편집…): the component dialog (`dialogs/mask_component.rs`).
pub fn edit_component(cx: &mut AppContext) {
    crate::dialogs::mask_component::open(cx);
}

/// Port of `duplicate_component`.
pub fn duplicate_component(cx: &mut AppContext) {
    let Some(component) = current_component(cx) else { return };
    commit(cx, "보정 변경");
    add_component(cx, component);
    finish(cx);
}

/// Port of `move_component` (구성 요소 위로 / 아래로).
pub fn move_component(cx: &mut AppContext, delta: isize) {
    if current_component(cx).is_none() {
        return;
    }
    let (layer, index) = {
        let s = state(cx);
        (s.layer, s.component)
    };
    let mut list = masks(cx);
    let Some(target) = layers::at_mut(&mut list, layer) else { return };
    let components = layers::components_mut(target);
    let to = index + delta;
    if to < 0 || to >= components.len() as isize {
        return;
    }
    commit(cx, "보정 변경");
    components.swap(index as usize, to as usize);
    state(cx).component = to;
    set_masks(cx, list);
    finish(cx);
    sync(cx);
    set_tool(cx, Mode::None);
}

/// Port of `duplicate_layer` (마스크 복제: the copy is named "… 사본" and selected).
pub fn duplicate_layer(cx: &mut AppContext) {
    let Some(mut copy) = current_layer(cx) else { return };
    commit(cx, "보정 변경");
    let name = layers::layer_name(&copy) + " 사본";
    copy.insert("name".into(), Value::from(name));
    let mut list = masks(cx);
    let s = state(cx);
    list.insert((s.layer + 1) as usize, Value::Object(copy));
    s.layer += 1;
    s.component = -1;
    set_masks(cx, list);
    finish(cx);
    sync(cx);
}

/// Port of `move_layer` (위로 / 아래로 이동).
pub fn move_layer(cx: &mut AppContext, delta: isize) {
    let mut list = masks(cx);
    let index = state(cx).layer;
    let len = list.len() as isize;
    if !(0..len).contains(&index) || !(0..len).contains(&(index + delta)) {
        return;
    }
    commit(cx, "보정 변경");
    list.swap(index as usize, (index + delta) as usize);
    state(cx).layer += delta;
    set_masks(cx, list);
    finish(cx);
    sync(cx);
    set_tool(cx, Mode::None);
}

/// Port of `edit_component_handles`: the selected gradient's handles on screen, ready to drag.
pub fn edit_component_handles(cx: &mut AppContext) {
    let component = current_component(cx);
    set_tool(cx, Mode::None);
    let Some(component) = component else { return };
    let kind = match component.get("type").and_then(Value::as_str) {
        Some("linear") => "linear",
        Some("radial") => "radial",
        _ => return,
    };
    let Some(map) = coordinate_map(cx) else { return };
    let (handles, angle) = gradient::component_handles(&map, &component);
    mode(cx, Mode::EditComponent);
    let s = state(cx);
    s.handles = handles;
    s.handle_kind = kind;
    s.handle_angle = angle;
    cx.set_status(tr("양 끝점으로 크기를 조정하거나 가운데 점으로 이동하세요."));
}

/// Port of `move_component_handles` (`componentEdited`): the gradient after its handles moved.
pub fn move_component_handles(cx: &mut AppContext, handles: &[[f64; 2]]) {
    let Some(component) = current_component(cx) else { return };
    let Some(map) = coordinate_map(cx) else { return };
    let angle = state(cx).handle_angle;
    let updated = gradient::moved_component(&map, &component, handles, angle);
    replace_component(cx, updated);
    edit_component_handles(cx);
}

/// Port of `radial_turned` (타원 회전): the selected radial turned to `value` degrees on screen, live.
pub fn radial_turned(cx: &mut AppContext, value: f64) {
    let Some(component) = current_component(cx) else { return };
    if component.get("type").and_then(Value::as_str) != Some("radial") {
        return;
    }
    let Some(map) = coordinate_map(cx) else { return };
    pause_overlay(cx);
    let (centre, half, _) = gradient::radial_on_screen(&map, &component);
    let updated = gradient::radial_from_screen(&map, centre, half, value, &component);
    let (layer, index) = {
        let s = state(cx);
        (s.layer, s.component)
    };
    let mut list = masks(cx);
    if let Some(target) = layers::at_mut(&mut list, layer) {
        if let Some(slot) = layers::components_mut(target).get_mut(index as usize) {
            *slot = Value::Object(updated.clone());
        }
    }
    set_masks(cx, list);
    if state(cx).mode == Mode::EditComponent {
        let (handles, angle) = gradient::component_handles(&map, &updated);
        let s = state(cx);
        s.handles = handles;
        s.handle_angle = angle;
    }
}

/// The radial's angle on screen as the 타원 회전 slider shows it (`(angle + 180) % 360 - 180`).
pub fn radial_angle(cx: &mut AppContext) -> Option<f64> {
    let component = current_component(cx)?;
    if component.get("type").and_then(Value::as_str) != Some("radial") {
        return None;
    }
    let map = coordinate_map(cx)?;
    let angle = gradient::radial_on_screen(&map, &component).2;
    Some((angle + 180.0).rem_euclid(360.0) - 180.0)
}
