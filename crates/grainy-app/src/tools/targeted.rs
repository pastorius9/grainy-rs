//! The targeted adjustment (사진에서 드래그로 조정): press on the photo and drag up or down to raise or lower the
//! tone curve at the pressed tone. Port of luma/targeted.py (`sample`, `curve_input`, `drag_curve`,
//! `TargetedAdjust.toggle`, `started`, `dragged`, `finished`, `tool_changed`) and of the photo view's 'adjust'
//! tool (luma/widgets.py `PhotoView`: the press inside the photo, the vertical drag in screen pixels, Esc).
//!
//! The HSL part (work package 7): the colour mixer's button drags the mixer value of the colours at the pressed
//! pixel's hue (`hsl_weights`, `drag_hsl`), the column chosen by the mixer's 사진에서 조정할 항목 box
//! ([`TargetedAdjust::hsl_target`]); the press selects the colour with the largest weight in the mixer
//! ([`TargetedAdjust::hsl_color`]).

use eframe::egui::{CursorIcon, Key, Modifiers, Painter};
use grainy_i18n::tr;

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::{AppContext, Mode};
use crate::imagespace::ViewTransform;
use crate::panels::Tool;
use crate::render::Preview;
use crate::state::EditState;
use crate::widgets::curve::{points_of, points_value};
use serde_json::Value;

/// Curve output per dragged screen pixel (`CURVE_PER_PIXEL`).
pub const CURVE_PER_PIXEL: f64 = 1.0 / 300.0;
/// A curve point this close to the pressed tone is reused (`NEAR_POINT`).
pub const NEAR_POINT: f64 = 0.04;
/// The history label of a targeted curve drag.
pub const CURVE_LABEL: &str = "사진에서 톤 커브 조정";
/// The hues of the mixer's eight colours (`HSL_CENTERS`, `processing.mix_hsl`).
pub const HSL_CENTERS: [f32; 8] = [0.0, 1.0 / 12.0, 1.0 / 6.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 5.0 / 6.0];
/// HSL slider units per dragged screen pixel for the dominant colour (`HSL_PER_PIXEL`).
pub const HSL_PER_PIXEL: f64 = 0.5;
/// The history label of a targeted mixer drag.
pub const HSL_LABEL: &str = "사진에서 색상 혼합 조정";

/// What the tool adjusts (the curve panel's button or the mixer's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Curve,
    Hsl,
}

/// What a press remembers until the drag ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// The curve when pressed and the curve input of the pressed tone.
    Curve { points: Vec<[f64; 2]>, x: f64 },
    /// The mixer value (`hsl`) when pressed and the weights of the pressed pixel's hue.
    Hsl { groups: Value, weights: [f32; 8] },
}

/// Port of `TargetedAdjust`'s state, kept in the context (`cx.state::<TargetedAdjust>()`).
#[derive(Debug, Clone)]
pub struct TargetedAdjust {
    /// The checked button (`self.kind`).
    pub kind: Option<Kind>,
    /// `self.start`.
    pub start: Option<Start>,
    /// The press's screen y (`adjust_origin`).
    pub origin: Option<f32>,
    /// The mixer's colour box (`w.hsl_color`): 0 빨강 … 7 자홍. A mixer press selects its colour here.
    pub hsl_color: usize,
    /// The mixer's 사진에서 조정할 항목 box (`w.hsl_target`): the column a mixer drag changes (0 색상, 1 채도,
    /// 2 밝기); 채도 at start.
    pub hsl_target: usize,
}

impl Default for TargetedAdjust {
    fn default() -> Self {
        Self { kind: None, start: None, origin: None, hsl_color: 0, hsl_target: 1 }
    }
}

impl TargetedAdjust {
    /// Port of `tool_changed` to another tool: nothing checked, no drag.
    pub fn stop(&mut self) {
        self.kind = None;
        self.start = None;
        self.origin = None;
    }

    /// Port of `started` once the pressed pixel is sampled: commits the pending edit, remembers the curve and the
    /// curve input of the pixel's tone.
    pub fn started_with(&mut self, edit: &mut EditState, rgb: [f32; 3]) {
        let Some(kind) = self.kind else { return };
        edit.commit("보정 변경");
        match kind {
            Kind::Curve => {
                let points = points_of(edit.settings().value("curve"));
                let x = curve_input(&points, rgb);
                self.start = Some(Start::Curve { points, x });
            }
            Kind::Hsl => {
                let weights = hsl_weights(rgb);
                self.start = Some(Start::Hsl { groups: edit.settings().value("hsl").clone(), weights });
                self.hsl_color = argmax(&weights);
            }
        }
    }

    /// Port of `dragged`: the curve from the press, raised by `pixels` (up is positive).
    pub fn dragged(&mut self, edit: &mut EditState, pixels: f64) {
        let Some(start) = &self.start else { return };
        match start {
            Start::Curve { points, x } => {
                let value = drag_curve(points, *x, pixels * CURVE_PER_PIXEL);
                edit.set("curve", points_value(&value));
            }
            Start::Hsl { groups, weights } => {
                let value = drag_hsl(groups, weights, self.hsl_target, pixels * HSL_PER_PIXEL);
                edit.set("hsl", value);
            }
        }
    }

    /// Port of `finished`: one undo step with the tool's label.
    pub fn finished(&mut self, edit: &mut EditState) {
        let Some(start) = self.start.take() else { return };
        let label = match start {
            Start::Curve { .. } => CURVE_LABEL,
            Start::Hsl { .. } => HSL_LABEL,
        };
        edit.commit(label);
    }
}

/// Port of `sample` on any picture: the mean RGB of the 3×3 pixels around the normalized point (float32).
pub fn sample_with(width: usize, height: usize, x: f64, y: f64, pixel: impl Fn(usize, usize) -> [f32; 3]) -> [f32; 3] {
    if width == 0 || height == 0 {
        return [0.0; 3];
    }
    // int() truncates towards zero; min(w-1, …) as the oracle (a negative index cannot happen: x, y ≥ 0).
    let px = ((x * width as f64) as i64).clamp(0, width as i64 - 1) as usize;
    let py = ((y * height as f64) as i64).clamp(0, height as i64 - 1) as usize;
    let (x0, x1) = (px.saturating_sub(1), (px + 2).min(width));
    let (y0, y1) = (py.saturating_sub(1), (py + 2).min(height));
    let mut sum = [0f32; 3];
    let mut count = 0usize;
    for row in y0..y1 {
        for column in x0..x1 {
            let p = pixel(column, row);
            for c in 0..3 {
                sum[c] += p[c];
            }
            count += 1;
        }
    }
    sum.map(|v| v / count as f32)
}

/// Port of `sample` on a float picture (H×W×3, the view's display-referred `on_screen`).
pub fn sample(image: &ndarray::ArrayView3<f32>, x: f64, y: f64) -> [f32; 3] {
    let (h, w, _) = image.dim();
    sample_with(w, h, x, y, |c, r| [image[[r, c, 0]], image[[r, c, 1]], image[[r, c, 2]]])
}

/// Port of `sample` on the frame on screen (8-bit RGBA, read as `value / 255`).
pub fn sample_preview(preview: &Preview, x: f64, y: f64) -> [f32; 3] {
    let w = preview.width;
    sample_with(w, preview.height, x, y, |c, r| {
        let i = (r * w + c) * 4;
        [0, 1, 2].map(|k| f32::from(preview.rgba.get(i + k).copied().unwrap_or(0)) / 255.0)
    })
}

/// Port of `curve_input`: the curve input that produced the displayed tone (the inverse of a curve with rising
/// outputs at the pixel's luminance; otherwise the luminance itself).
pub fn curve_input(points: &[[f64; 2]], rgb: [f32; 3]) -> f64 {
    let luminance = rgb[0] * 0.2126f32 + rgb[1] * 0.7152f32 + rgb[2] * 0.0722f32;
    let y = f64::from(luminance.clamp(0.0, 1.0));
    let p: Vec<[f32; 2]> = points.iter().map(|q| [q[0] as f32, q[1] as f32]).collect();
    if p.len() < 2 || p.windows(2).any(|w| w[1][1] - w[0][1] <= 0.0) {
        return y;
    }
    let outputs: Vec<f64> = p.iter().map(|q| f64::from(q[1])).collect();
    let inputs: Vec<f64> = p.iter().map(|q| f64::from(q[0])).collect();
    grainy_core::numpy::interp(y, &outputs, &inputs).clamp(0.0, 1.0)
}

/// Port of `drag_curve`: `points` with the point at input `x` (added on the curve if none is within
/// [`NEAR_POINT`]) raised by `delta`, within 0–1. The endpoints only move vertically.
pub fn drag_curve(points: &[[f64; 2]], x: f64, delta: f64) -> Vec<[f64; 2]> {
    let mut p = points.to_vec();
    if p.is_empty() {
        return p;
    }
    let mut index = 0;
    for (i, q) in p.iter().enumerate() {
        if (q[0] - x).abs() < (p[index][0] - x).abs() {
            index = i;
        }
    }
    if (p[index][0] - x).abs() > NEAR_POINT {
        let xs: Vec<f64> = p.iter().map(|q| q[0]).collect();
        let ys: Vec<f64> = p.iter().map(|q| q[1]).collect();
        p.push([x, grainy_core::numpy::interp(x, &xs, &ys)]);
        p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        index = p.iter().position(|q| q[0] == x).unwrap_or(0);
    }
    p[index][1] = (p[index][1] + delta).clamp(0.0, 1.0);
    p
}

/// `np.argmax`: the first index of the largest value.
fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best] {
            best = i;
        }
    }
    best
}

/// Port of `hsl_weights`: `processing.mix_hsl`'s weight of each of the eight colours at the pixel's hue (OpenCV's
/// HLS of a 1×1 picture), scaled so the largest is 1. float32 as the oracle (NumPy 2 keeps the float32 hue).
pub fn hsl_weights(rgb: [f32; 3]) -> [f32; 8] {
    let (hue, _, _) = grainy_colorgrade::convert::rgb_to_hsl_pixel(&rgb, false);
    let weights = HSL_CENTERS.map(|center| {
        let distance = (grainy_core::numpy::py_mod(hue - center + 0.5, 1.0) - 0.5).abs();
        let w = (1.0 - distance / 0.125f32).max(0.0);
        w * w
    });
    let largest = weights.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    // weights / max(float(weights.max()), 1e-6): a Python float divides float32 weights in float32.
    let divisor = f64::from(largest).max(1e-6) as f32;
    weights.map(|w| w / divisor)
}

/// Port of `drag_hsl`: the mixer value with every colour of positive weight moved by `delta * weight` in the
/// `column` (float32 arithmetic, as NumPy 2 mixes a float32 weight with Python floats), within ±100. Entries of
/// colours with no weight keep their stored value.
pub fn drag_hsl(groups: &Value, weights: &[f32; 8], column: usize, delta: f64) -> Value {
    let mut value = groups.clone();
    for (index, weight) in weights.iter().enumerate() {
        if *weight <= 0.0 {
            continue;
        }
        let Some(slot) = value.get_mut(index).and_then(|group| group.get_mut(column)) else { continue };
        let current = slot.as_f64().unwrap_or(0.0) as f32;
        let moved = (current + delta as f32 * weight).clamp(-100.0, 100.0);
        *slot = Value::from(f64::from(moved));
    }
    value
}

/// Whether the button of `kind` is checked (the tool is on for it).
pub fn checked(cx: &AppContext, kind: Kind) -> bool {
    cx.state_ref::<TargetedAdjust>().is_some_and(|s| s.kind == Some(kind)) && on_screen(cx)
}

/// The view is in the state where the 'adjust' tool can be the view's tool: Develop, no other tool, no picker.
fn on_screen(cx: &AppContext) -> bool {
    cx.mode() == Mode::Develop && cx.tool() == Tool::Edit && !cx.wb_picker
}

/// Port of `tool_changed`: another tool took the view (crop, healing, masks, the white-balance picker), so the
/// button is unchecked. The panels call it every frame.
pub fn sync(cx: &mut AppContext) {
    if !on_screen(cx) && cx.state_ref::<TargetedAdjust>().is_some_and(|s| s.kind.is_some()) {
        cx.state::<TargetedAdjust>().stop();
    }
}

/// Port of `TargetedAdjust.toggle`: the button of `kind` checked or unchecked.
pub fn toggle(cx: &mut AppContext, kind: Kind, checked: bool) {
    if !checked {
        let state = cx.state::<TargetedAdjust>();
        if state.kind == Some(kind) {
            state.stop();
        }
        return;
    }
    let has_photo = cx.doc.current().is_some();
    if !has_photo || (kind == Kind::Curve && cx.settings().bool("hdr")) {
        if has_photo {
            cx.set_status(tr("HDR 모드에서는 사진 위 커브 조정을 쓸 수 없습니다."));
        }
        return;
    }
    // A crop (or another tool) in progress is left; the white-balance picker is switched off.
    if cx.mode() != Mode::Develop {
        cx.set_mode(Mode::Develop);
    }
    if cx.tool() != Tool::Edit {
        cx.select_tool(Tool::Edit);
    }
    cx.wb_picker = false;
    let state = cx.state::<TargetedAdjust>();
    state.stop();
    state.kind = Some(kind);
    cx.set_status(tr("사진을 누른 채 위아래로 끌어 그 부분을 조정하세요. 버튼을 다시 누르면 끝납니다."));
}

/// Port of `started` on the view: samples the frame on screen at the normalized point.
pub fn started(cx: &mut AppContext, x: f64, y: f64) {
    if cx.state_ref::<TargetedAdjust>().is_none_or(|s| s.kind.is_none()) {
        return;
    }
    let Some(frame) = cx.frame().cloned() else { return };
    let rgb = sample_preview(&frame.preview, x, y);
    let mut state = std::mem::take(cx.state::<TargetedAdjust>());
    state.started_with(cx.doc.edit_mut(), rgb);
    *cx.state::<TargetedAdjust>() = state;
}

/// Port of `dragged` on the view.
pub fn dragged(cx: &mut AppContext, pixels: f64) {
    let mut state = std::mem::take(cx.state::<TargetedAdjust>());
    state.dragged(cx.doc.edit_mut(), pixels);
    *cx.state::<TargetedAdjust>() = state;
    cx.before = false;
}

/// Port of `finished` on the view.
pub fn finished(cx: &mut AppContext) {
    let mut state = std::mem::take(cx.state::<TargetedAdjust>());
    state.finished(cx.doc.edit_mut());
    *cx.state::<TargetedAdjust>() = state;
}

/// The canvas tool: active while a targeted button is checked.
pub struct Targeted;

impl CanvasTool for Targeted {
    fn name(&self) -> &'static str {
        "targeted"
    }

    fn active(&self, cx: &AppContext) -> bool {
        on_screen(cx) && cx.state_ref::<TargetedAdjust>().is_some_and(|s| s.kind.is_some())
    }

    fn edits(&self) -> &'static [&'static str] {
        &["curve", "hsl"]
    }

    /// `PhotoView.mousePressEvent` / `mouseMoveEvent` / `mouseReleaseEvent` in the 'adjust' mode: the press must be
    /// on the photo; the drag is the press's y minus the pointer's, in screen pixels.
    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        match event.kind {
            PointerKind::Hover => false,
            PointerKind::Press | PointerKind::Click => {
                // egui reports a drag once the pointer has moved a little: the press is where the button went down.
                let origin = cx.egui.input(|i| i.pointer.press_origin()).unwrap_or(event.screen);
                let origin = if event.kind == PointerKind::Click { event.screen } else { origin };
                let point = match cx.photo_rect() {
                    Some(rect) => ViewTransform { rect }.to_frame_free(origin),
                    None => event.free,
                };
                if !(0.0..=1.0).contains(&point[0]) || !(0.0..=1.0).contains(&point[1]) {
                    return true;
                }
                cx.state::<TargetedAdjust>().origin = Some(origin.y);
                started(cx, point[0], point[1]);
                if event.kind == PointerKind::Click {
                    dragged(cx, 0.0);
                    cx.state::<TargetedAdjust>().origin = None;
                    finished(cx);
                }
                true
            }
            PointerKind::Drag => {
                if let Some(origin) = cx.state::<TargetedAdjust>().origin {
                    dragged(cx, f64::from(origin - event.screen.y));
                }
                true
            }
            PointerKind::Release => {
                if let Some(origin) = cx.state::<TargetedAdjust>().origin.take() {
                    dragged(cx, f64::from(origin - event.screen.y));
                    finished(cx);
                }
                true
            }
        }
    }

    /// Esc leaves the tool (`set_tool('')`).
    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        if key == Key::Escape {
            cx.state::<TargetedAdjust>().stop();
            return true;
        }
        false
    }

    /// The vertical-resize cursor over the photo (`SizeVerCursor`).
    fn paint(&self, _painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        if cx.egui.pointer_hover_pos().is_some_and(|p| view.rect.contains(p)) {
            cx.egui.set_cursor_icon(CursorIcon::ResizeVertical);
        }
    }
}
