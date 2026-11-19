//! The mask brush (브러시): painting strokes on the photo with the tool options' size, feather, flow and density,
//! Alt to erase, the automatic mask, the 밝기 제한 range of the mask and `[` / `]` sizes. Port of luma/studio.py
//! `preview_stroke`, `stroke`, `brush_range_changed`, `set_brush_size`, `brush_cursor` and the brush part of
//! `load_layer`, with luma/widgets.py `PhotoView` in the mode `brush` (`mousePressEvent`, `mouseMoveEvent`,
//! `mouseReleaseEvent`, `keyPressEvent`, `resize_brush` and the brush cursor of `drawForeground`).
//!
//! Strokes go to the selected mask through [`actions::add_component`] (inside the mask's brush range,
//! [`layers::ranged`]); while one is drawn the settings' masks hold it (the pending stroke of the masks package),
//! so the picture and the overlay show it live; the release commits 부분 보정 and Esc puts the masks back.

use eframe::egui::{self, Color32, Key, Modifiers, Painter, Pos2, Shape, Stroke};
use serde_json::Value;

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::commands::Invocation;
use crate::context::AppContext;
use crate::imagespace::ViewTransform;
use crate::masks::layers::{self, Obj};
use crate::masks::{actions, overlay, MaskState, Mode, Pending, BRUSH_RANGES};
use crate::panels::Tool;
use crate::text::tr;

/// The brush's largest radius (`PhotoView.resize_brush`: `.25` for the mask brush).
pub const MAXIMUM: f64 = 0.25;
/// The smallest radius.
pub const MINIMUM: f64 = 0.002;

/// What the brush keeps between frames besides the masks package's [`MaskState`].
#[derive(Debug, Default)]
pub struct BrushState {
    /// `PhotoView.brush_radius` after `[` / `]` and the 크기 position it was set at (the view keeps the unrounded
    /// radius until the slider moves or the tool is chosen again).
    radius: Option<(f64, i64)>,
    /// `tool_alt` of the stroke being drawn.
    alt: bool,
    /// The (photo, layer, components) the brush range was last loaded for (`load_layer`).
    loaded: Option<String>,
}

fn st(cx: &mut AppContext) -> &mut BrushState {
    cx.state::<BrushState>()
}

/// Python's `round` (half to even).
fn py_round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// `PhotoView.brush_radius` of the mask brush: the view's radius after `[` / `]`, else the 크기 slider's
/// (`mode` and `brush_cursor` set it to `radius.slider.value() / 1000`).
pub fn view_radius(cx: &AppContext) -> f64 {
    let position = cx.state_ref::<MaskState>().map_or(25, |s| s.options.radius);
    match cx.state_ref::<BrushState>().and_then(|b| b.radius) {
        Some((radius, at)) if at == position => radius,
        _ => position as f64 / 1000.0,
    }
}

/// Port of `PhotoView.resize_brush` for the mask brush with `set_brush_size` (`[` and `]`): `false` when the
/// brush is not in use.
pub fn resize_brush(cx: &mut AppContext, bigger: bool) -> bool {
    if cx.doc.current().is_none() || cx.tool() != Tool::Masks || actions::state(cx).mode != Mode::Brush {
        return false;
    }
    let factor = if bigger { 1.2 } else { 1.0 / 1.2 };
    let radius = (view_radius(cx) * factor).clamp(MINIMUM, MAXIMUM);
    // set_brush_size: the 크기 slider (2–250) at round(radius * 1000); a move calls brush_cursor.
    let position = py_round(radius * 1000.0).clamp(2, 250);
    let options = &mut actions::state(cx).options;
    let view = if position != options.radius {
        options.radius = position;
        position as f64 / 1000.0
    } else {
        radius
    };
    st(cx).radius = Some((view, position));
    true
}

/// The `mask.brush_size` command (`[` / `]` of the shortcut table): the healing brushes while the Healing tool is
/// open, the mask brush otherwise.
pub fn brush_size(cx: &mut AppContext, invocation: &Invocation) {
    let bigger = invocation.key == Some("]");
    if cx.tool() == Tool::Healing && super::healing::resize_brush(cx, bigger) {
        return;
    }
    resize_brush(cx, bigger);
}

/// The view's radius comes back to the slider's when the brush is chosen again (`mode('brush')`).
pub fn forget_size(cx: &mut AppContext) {
    st(cx).radius = None;
}

/// The settings' masks with the stroke being painted (`preview_settings` for a brush stroke).
fn preview_masks(base: &Value, pending: &Pending, brush_range: &str) -> Value {
    let mut list = base.as_array().cloned().unwrap_or_default();
    if layers::at(&list, pending.layer).is_none() {
        list.push(layers::preview_layer());
    }
    let index = if layers::at(&list, pending.layer).is_some() { pending.layer } else { list.len() as isize - 1 };
    if let Some(layer) = layers::at_mut(&mut list, index) {
        let components = layers::components(layer).to_vec();
        *layers::components_mut(layer) =
            layers::ranged(&components, Value::Object(pending.component.clone()), brush_range).0;
    }
    Value::Array(list)
}

fn points_value(points: &[[f64; 2]]) -> Value {
    Value::Array(points.iter().map(|p| Value::Array(vec![Value::from(p[0]), Value::from(p[1])])).collect())
}

/// Port of `preview_stroke` for the brush: the first call commits what was there and starts the pending stroke
/// (`make_component` with the source points of the stroke so far); later calls replace its points.
pub fn preview_stroke(cx: &mut AppContext, points: &[[f64; 2]], alt: bool) {
    if cx.doc.current().is_none() || points.is_empty() || actions::state(cx).mode != Mode::Brush {
        return;
    }
    let photo = cx.doc.current_id();
    if actions::state(cx).pending.as_ref().is_some_and(|p| p.photo != photo) {
        actions::cancel_stroke(cx);
        return;
    }
    let Some(map) = actions::coordinate_map(cx) else { return };
    let source = map.source_points(points);
    if source.is_empty() {
        return;
    }
    actions::state(cx).overlay_paused = false;
    if actions::state(cx).pending.is_none() {
        cx.doc.edit_mut().commit("보정 변경");
        let base = cx.settings().value("masks").clone();
        let s = actions::state(cx);
        let component = actions::make_component(s, "brush", &source, alt);
        s.pending = Some(Pending { photo, base, layer: s.layer, component });
    } else if let Some(pending) = actions::state(cx).pending.as_mut() {
        pending.component.insert("points".into(), points_value(&source));
    }
    let live = {
        let s = actions::state(cx);
        let brush_range = s.options.brush_range;
        let Some(pending) = s.pending.as_ref() else { return };
        preview_masks(&pending.base, pending, brush_range)
    };
    cx.doc.edit_mut().set("masks", live);
}

/// Port of `stroke` for the brush: the component as the press made it (`frozen`) with the stroke's source points,
/// added to the selected mask inside its brush range and committed as 부분 보정.
pub fn stroke(cx: &mut AppContext, points: &[[f64; 2]], alt: bool) {
    if cx.doc.current().is_none() || actions::state(cx).mode != Mode::Brush {
        return;
    }
    let photo = cx.doc.current_id();
    let frozen = actions::state(cx).pending.as_ref().filter(|p| p.photo == photo).map(|p| p.component.clone());
    actions::cancel_stroke(cx);
    let Some(map) = actions::coordinate_map(cx) else { return };
    let source = map.source_points(points);
    if source.is_empty() {
        cx.set_status(tr("변형 후 생긴 빈 가장자리 대신 사진 안쪽을 선택하세요."));
        return;
    }
    let mut component = frozen.unwrap_or_else(|| actions::make_component(actions::state(cx), "brush", &source, alt));
    component.insert("points".into(), points_value(&source));
    actions::add_component(cx, component);
    cx.doc.edit_mut().commit("부분 보정");
    load_brush_range(cx);
}

/// The key of a 밝기 제한 value (`findData`, unknown: 전체).
fn range_key(key: &str) -> &'static str {
    BRUSH_RANGES.iter().find(|(k, _, _)| *k == key).map_or("all", |(k, _, _)| *k)
}

/// The brush part of `load_layer`: the range belongs to the mask. An existing range is shown; a mask with strokes
/// and none shows 전체; an empty mask being painted keeps the choice for its first stroke.
pub fn load_brush_range(cx: &mut AppContext) {
    actions::sync(cx);
    let layer = actions::current_layer(cx).unwrap_or_default();
    let components = layers::components(&layer).to_vec();
    let place = layers::brush_range_place(&components);
    let brush = actions::state(cx).mode == Mode::Brush;
    if place.is_some() || !components.is_empty() || !brush {
        let key = place.and_then(|p| components[p].get("brush_range")).and_then(Value::as_str).map_or("all", range_key);
        actions::state(cx).options.brush_range = key;
    }
    st(cx).loaded = Some(loaded_key(cx));
}

fn loaded_key(cx: &mut AppContext) -> String {
    let photo = cx.doc.current_id();
    let index = actions::state(cx).layer;
    let layer = actions::current_layer(cx).map(|l| Value::Array(layers::components(&l).to_vec()).to_string());
    format!("{photo}|{index}|{}", layer.unwrap_or_default())
}

/// `load_layers` runs after every change of the mask list: the brush range is loaded again when the selected mask
/// or its components changed since it was last loaded.
pub fn sync_brush_range(cx: &mut AppContext) {
    if cx.doc.current().is_none() {
        return;
    }
    let key = loaded_key(cx);
    if st(cx).loaded.as_deref() != Some(key.as_str()) {
        load_brush_range(cx);
    }
}

/// Port of `brush_range_changed` (the 밝기 제한 box set to `key`): the selected mask's range is started, changed or
/// taken away at once when it has strokes (otherwise it starts with the first stroke).
pub fn set_brush_range(cx: &mut AppContext, key: &str) {
    let key = range_key(key);
    if actions::state(cx).options.brush_range == key {
        return;
    }
    actions::state(cx).options.brush_range = key;
    brush_range_changed(cx);
}

/// Port of `brush_range_changed`.
pub fn brush_range_changed(cx: &mut AppContext) {
    let Some(layer) = actions::current_layer(cx) else { return };
    let mut components = layers::components(&layer).to_vec();
    let place = layers::brush_range_place(&components);
    let key = actions::state(cx).options.brush_range;
    match place {
        None => {
            let brushed = components.iter().any(|c| c.get("type").and_then(Value::as_str) == Some("brush"));
            if key == "all" || !brushed {
                return;
            }
            components.push(layers::range_component(key));
        }
        Some(place) if key == "all" => {
            components.remove(place);
        }
        Some(place) if components[place].get("brush_range").and_then(Value::as_str) == Some(key) => return,
        Some(place) => components[place] = layers::range_component(key),
    }
    cx.doc.edit_mut().commit("보정 변경");
    let index = actions::state(cx).layer;
    let mut list = actions::masks(cx);
    let len = components.len() as isize;
    if let Some(target) = layers::at_mut(&mut list, index) {
        *layers::components_mut(target) = components;
    }
    let s = actions::state(cx);
    s.component = s.component.min(len - 1);
    actions::set_masks(cx, list);
    cx.doc.edit_mut().commit("브러시 밝기 범위");
    load_brush_range(cx);
}

/// A stroke component as `make_component` makes it (for tests and the panel's preview of the options).
pub fn component(cx: &mut AppContext, source: &[[f64; 2]], alt: bool) -> Obj {
    actions::make_component(actions::state(cx), "brush", source, alt)
}

/// The brush on the photo view.
#[derive(Default)]
pub struct Brush;

fn inside(p: [f64; 2]) -> bool {
    (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1])
}

/// The pointer was pressed on the photo at `point` (`mousePressEvent` + `strokeProgress`).
pub fn press(cx: &mut AppContext, point: [f64; 2], alt: bool) {
    actions::state(cx).points = vec![point];
    st(cx).alt = alt;
    preview_stroke(cx, &[point], alt);
}

/// The pointer moved while pressed (`mouseMoveEvent` + `strokeProgress`): a point is kept only when it moved.
pub fn move_to(cx: &mut AppContext, point: [f64; 2]) {
    let s = actions::state(cx);
    if s.points.is_empty() {
        return;
    }
    if s.points.last() != Some(&point) {
        s.points.push(point);
    }
    let points = s.points.clone();
    let alt = st(cx).alt;
    preview_stroke(cx, &points, alt);
}

/// The pointer was released (`mouseReleaseEvent` + `strokeCompleted`).
pub fn release(cx: &mut AppContext, point: [f64; 2]) {
    let s = actions::state(cx);
    if s.points.is_empty() {
        return;
    }
    if s.points.last() != Some(&point) {
        s.points.push(point);
    }
    let points = std::mem::take(&mut s.points);
    let alt = st(cx).alt;
    stroke(cx, &points, alt);
}

impl CanvasTool for Brush {
    fn name(&self) -> &'static str {
        "mask_brush"
    }

    fn tool(&self) -> Option<Tool> {
        Some(Tool::Masks)
    }

    fn active(&self, cx: &AppContext) -> bool {
        cx.tool() == Tool::Masks
            && cx.state_ref::<MaskState>().is_some_and(|s| s.mode == Mode::Brush && s.photo == cx.doc.current_id())
    }

    fn edits(&self) -> &'static [&'static str] {
        &["masks"]
    }

    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        if cx.doc.current().is_none() {
            return false;
        }
        actions::sync(cx);
        sync_brush_range(cx);
        match event.kind {
            PointerKind::Press => {
                if !inside(event.free) {
                    return false;
                }
                press(cx, event.frame, event.modifiers.alt);
            }
            PointerKind::Drag => move_to(cx, event.frame),
            PointerKind::Release => release(cx, event.frame),
            PointerKind::Click => {
                if !inside(event.free) {
                    return false;
                }
                press(cx, event.frame, event.modifiers.alt);
                release(cx, event.frame);
            }
            PointerKind::Hover => return inside(event.free),
        }
        true
    }

    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        if cx.doc.current().is_none() {
            return false;
        }
        match key {
            Key::Escape => {
                let s = actions::state(cx);
                if !s.points.is_empty() || s.pending.is_some() {
                    actions::cancel_stroke(cx);
                } else {
                    actions::set_tool(cx, Mode::None);
                }
                true
            }
            Key::OpenBracket | Key::CloseBracket => resize_brush(cx, key == Key::CloseBracket),
            _ => false,
        }
    }

    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        overlay::paint(painter, view, cx);
        // The brush cursor: the outer circle and the dotted start of the feather (`brush_feather`).
        let Some(pointer) = painter.ctx().pointer_hover_pos() else { return };
        if !view.rect.contains(pointer) {
            return;
        }
        let Some(state) = cx.state_ref::<MaskState>() else { return };
        let feather = state
            .pending
            .as_ref()
            .and_then(|p| p.component.get("feather"))
            .and_then(Value::as_f64)
            .unwrap_or(state.options.feather as f64);
        let radius = view_radius(cx) as f32 * view.rect.width().min(view.rect.height());
        let pen = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 210));
        painter.circle_stroke(pointer, radius, pen);
        let inner = radius * (1.0 - feather as f32 / 100.0);
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
