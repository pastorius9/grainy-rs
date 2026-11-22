//! Guided upright (가이드 원근): four clicks on the photo give the corners of a rectangle, and the photo is warped so
//! that they become one (the `guides` setting, applied by the geometry stage).
//!
//! Port of luma/studio.py `StudioTools.start_guides`, `mode('guide')` / `mode('')`, the guide branch of `stroke`
//! (the corner carried to the source by `source_points`, the convexity and area check, `guides`, the commit
//! `부분 보정`) and of the photo view's 'guide' tool (luma/widgets.py `PhotoView`: the stroke pressed inside the
//! photo, moved, released; Esc cancels the stroke, a second Esc ends the tool; the cross cursor; the stroke drawn
//! while the button is down).
//!
//! The oracle draws nothing for the corners already placed (the status line counts them); the port marks them with
//! small dots joined by lines so the quadrilateral being drawn is visible. Nothing else differs: there is no
//! dragging of corners in the oracle (the fourth click applies the guides and ends the tool).
//!
//! The tool keeps its state in [`GuideState`] (`cx.state()`); the Transform panel's buttons call [`start`] and
//! [`clear`]. Every function works without a window, so tests drive them directly.

use std::cell::Cell;

use eframe::egui::{Color32, CursorIcon, Key, Modifiers, Painter, Pos2, Stroke};
use serde_json::{json, Value};

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::{AppContext, Mode};
use crate::dialogs::import_errors::Notice;
use crate::imagespace::{CoordinateMap, ViewTransform};
use crate::panels::Tool;
use crate::text::{tr, tr_with};
use crate::tools::targeted::TargetedAdjust;

/// The undo label of placed guides (the label `stroke` commits with).
pub const LABEL: &str = "부분 보정";
/// The smallest area of the corners' quadrilateral, in source fractions (`cv2.contourArea(pts) > .005`).
pub const MIN_AREA: f64 = 0.005;

/// The guide tool's state between frames.
#[derive(Debug, Default)]
pub struct GuideState {
    /// Whether the view's tool is 'guide' (`tool_mode == 'guide'`). A cell: the tool ends itself when it sees
    /// another tool take the view (`tab_changed` → `set_tool('')`).
    on: Cell<bool>,
    /// The stroke being drawn, in frame fractions (`tool_points`).
    pub stroke: Vec<[f64; 2]>,
    /// The corners placed, in source fractions (`guide_points`).
    pub corners: Vec<[f64; 2]>,
    /// Where each corner was clicked, in frame fractions (for the overlay).
    pub marks: Vec<[f64; 2]>,
    /// The photo the corners belong to (`load_settings` forgets them when another photo opens).
    photo: u64,
    /// `coordinates` with its key (`coordinate_key`).
    map: Option<(String, CoordinateMap)>,
}

impl GuideState {
    /// Whether the guide tool is the view's tool.
    pub fn on(&self) -> bool {
        self.on.get()
    }
}

fn st(cx: &mut AppContext) -> &mut GuideState {
    cx.state::<GuideState>()
}

/// The view is where the guide tool can be the view's tool: Develop, no strip tool, no picker, no targeted
/// adjustment.
fn on_screen(cx: &AppContext) -> bool {
    cx.mode() == Mode::Develop
        && cx.tool() == Tool::Edit
        && !cx.wb_picker
        && cx.state_ref::<TargetedAdjust>().is_none_or(|s| s.kind.is_none())
}

/// Whether the guide tool takes the pointer now.
pub fn is_on(cx: &AppContext) -> bool {
    on_screen(cx) && cx.state_ref::<GuideState>().is_some_and(GuideState::on)
}

/// `self.w.source is not None`: a photo is open and its size is known.
fn has_source(cx: &AppContext) -> bool {
    cx.doc.current().is_some_and(|p| p.original.is_some())
}

/// The coordinate map's key: the photo, its live size and the geometry of the settings as shown.
fn map_key(cx: &AppContext) -> Option<(String, (usize, usize))> {
    let current = cx.doc.current()?;
    let (w, h) = current.original?;
    let size = super::healing::live_size(w as usize, h as usize);
    let s = grainy_core::effective(cx.settings());
    let geometry: serde_json::Map<String, Value> =
        grainy_engine::GEOMETRY_KEYS.iter().map(|k| ((*k).to_owned(), s.value(k).clone())).collect();
    Some((format!("{}:{}x{}:{}", current.id, size.0, size.1, Value::Object(geometry)), size))
}

/// Port of `source_point`'s cache: the map of the settings as shown, rebuilt when the geometry changes.
fn ensure_map(cx: &mut AppContext) -> bool {
    let Some((key, (w, h))) = map_key(cx) else { return false };
    if st(cx).map.as_ref().is_some_and(|(k, _)| *k == key) {
        return true;
    }
    let Ok(map) = CoordinateMap::new(w, h, cx.settings()) else { return false };
    st(cx).map = Some((key, map));
    true
}

/// Port of `source_points` keeping the first point on the photo, with the frame point it came from.
fn first_on_photo(cx: &mut AppContext, points: &[[f64; 2]]) -> Option<([f64; 2], [f64; 2])> {
    if points.is_empty() || !ensure_map(cx) {
        return None;
    }
    let (_, map) = st(cx).map.as_ref()?;
    points.iter().find_map(|p| map.source_points(&[*p]).first().map(|s| (*s, *p)))
}

/// Port of `start_guides` (가이드 원근 · 네 모서리 선택): `mode('guide')`, then the hint.
pub fn start(cx: &mut AppContext) {
    if has_source(cx) {
        // mode('guide'): an open crop frame is cancelled, the Develop view takes the tool, the picker is off.
        if crate::tools::crop::is_open(cx) {
            crate::tools::crop::cancel(cx);
        }
        if cx.mode() != Mode::Develop {
            cx.set_mode(Mode::Develop);
        }
        if cx.tool() != Tool::Edit {
            cx.select_tool(Tool::Edit);
        }
        cx.wb_picker = false;
        cx.state::<TargetedAdjust>().stop();
        crate::tools::point_color::stop(cx);
        let photo = cx.doc.current_id();
        let s = st(cx);
        s.on.set(true);
        s.stroke.clear();
        s.corners.clear();
        s.marks.clear();
        s.photo = photo;
    }
    cx.set_status(tr("좌상 → 우상 → 우하 → 좌하 순으로 클릭하세요."));
}

/// Silently cancels the guide tool when another canvas picker activates.
pub fn stop(cx: &mut AppContext) {
    let s = st(cx);
    s.on.set(false);
    s.stroke.clear();
}

/// Port of `mode('')` from the guide tool: the tool ends.
fn end(cx: &mut AppContext) {
    let s = st(cx);
    s.on.set(false);
    s.stroke.clear();
    cx.set_status(tr("도구를 종료했습니다."));
}

/// 가이드 원근 초기화: `set_setting('guides', None)`, committed by the commit timer.
pub fn clear(cx: &mut AppContext) {
    if !has_source(cx) {
        return;
    }
    st(cx).stroke.clear();
    cx.doc.edit_mut().set("guides", Value::Null);
    cx.before = false;
    cx.doc.edit_mut().commit("보정 변경");
}

/// `PhotoView.mousePressEvent` in the 'guide' mode: a stroke starts where the photo is pressed.
pub fn press(cx: &mut AppContext, point: [f64; 2]) -> bool {
    if !is_on(cx) || !has_source(cx) {
        return false;
    }
    if !((0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1])) {
        return false;
    }
    st(cx).stroke = vec![point];
    true
}

/// `normalized_point`: the frame point clamped to the frame.
fn clamped(point: [f64; 2]) -> [f64; 2] {
    [point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)]
}

/// `PhotoView.mouseMoveEvent`: the stroke follows the pointer.
pub fn move_to(cx: &mut AppContext, point: [f64; 2]) -> bool {
    let point = clamped(point);
    let s = st(cx);
    let Some(last) = s.stroke.last() else { return false };
    if *last != point {
        s.stroke.push(point);
    }
    true
}

/// `PhotoView.mouseReleaseEvent`: the stroke ends and is handed to `stroke`.
pub fn release(cx: &mut AppContext, point: [f64; 2]) -> bool {
    let point = clamped(point);
    let s = st(cx);
    let Some(last) = s.stroke.last() else { return false };
    if *last != point {
        s.stroke.push(point);
    }
    let points = std::mem::take(&mut s.stroke);
    stroke(cx, &points);
    true
}

/// A click without a drag: a press and a release at the same point.
pub fn click(cx: &mut AppContext, point: [f64; 2]) -> bool {
    press(cx, point) && release(cx, point)
}

/// `PhotoView.keyPressEvent` in the 'guide' mode: Esc cancels the stroke, or ends the tool.
pub fn key(cx: &mut AppContext, key: Key) -> bool {
    if key != Key::Escape || !is_on(cx) {
        return false;
    }
    let s = st(cx);
    if s.stroke.is_empty() {
        s.on.set(false);
    } else {
        s.stroke.clear();
    }
    true
}

/// Port of `cv2.isContourConvex(pts) and cv2.contourArea(pts) > .005` on the float32 corners.
pub fn valid_quad(corners: &[[f64; 2]]) -> bool {
    let points: Vec<[f32; 2]> = corners.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
    let convex = grainy_imgproc::is_contour_convex(&points).unwrap_or(false);
    convex && grainy_imgproc::contour_area(&points, false).is_ok_and(|area| area > MIN_AREA)
}

/// Port of the guide branch of `stroke`: the stroke's first point on the photo is the next corner; the fourth
/// corner sets `guides` when the corners make a convex quadrilateral of some size (a message box otherwise), and
/// ends the tool. The stroke is committed as `부분 보정`.
pub fn stroke(cx: &mut AppContext, points: &[[f64; 2]]) {
    if !has_source(cx) {
        return;
    }
    let Some((corner, mark)) = first_on_photo(cx, points) else {
        cx.set_status(tr("변형 후 생긴 빈 가장자리 대신 사진 안쪽을 선택하세요."));
        return;
    };
    let photo = cx.doc.current_id();
    let s = st(cx);
    if s.photo != photo {
        s.photo = photo;
        s.corners.clear();
        s.marks.clear();
    }
    s.corners.push(corner);
    s.marks.push(mark);
    let count = s.corners.len();
    cx.set_status(tr_with("가이드 모서리 {0}/4", &[&count.to_string()]));
    if count == 4 {
        let corners = std::mem::take(&mut st(cx).corners);
        st(cx).marks.clear();
        if valid_quad(&corners) {
            let value = Value::Array(corners.iter().map(|p| json!([p[0], p[1]])).collect());
            cx.doc.edit_mut().set("guides", value);
            cx.before = false;
        } else {
            cx.open_dialog(Box::new(Notice {
                id: "guides",
                title: tr("가이드"),
                text: tr("네 점이 겹치지 않는 사각형이 되도록 다시 선택해 주세요."),
            }));
        }
        end(cx);
    }
    cx.doc.edit_mut().commit(LABEL);
}

/// The canvas tool: active while the view's tool is 'guide'.
#[derive(Default)]
pub struct Guides;

impl CanvasTool for Guides {
    fn name(&self) -> &'static str {
        "guides"
    }

    /// While the guide tool is on. Another tool taking the view (the strip, the picker, the targeted adjustment)
    /// ends it, as the oracle's `tab_changed` / `set_tool` do; this tool is first in the registry so it sees that.
    fn active(&self, cx: &AppContext) -> bool {
        let Some(s) = cx.state_ref::<GuideState>() else { return false };
        if s.on() && !on_screen(cx) {
            s.on.set(false);
        }
        s.on()
    }

    fn edits(&self) -> &'static [&'static str] {
        &["guides"]
    }

    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        match event.kind {
            PointerKind::Hover => false,
            PointerKind::Press => press(cx, event.free),
            PointerKind::Drag => move_to(cx, event.free),
            PointerKind::Release => release(cx, event.free),
            PointerKind::Click => click(cx, event.free),
        }
    }

    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        self::key(cx, key)
    }

    /// The cross cursor over the photo, the stroke being drawn (`drawForeground`: a red round pen as wide as the
    /// mask brush's default size) and the corners placed.
    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        if cx.egui.pointer_hover_pos().is_some_and(|p| view.rect.contains(p)) {
            cx.egui.set_cursor_icon(CursorIcon::Crosshair);
        }
        let Some(s) = cx.state_ref::<GuideState>() else { return };
        let mark = Color32::from_rgb(0xf7, 0xdf, 0xbc);
        let corners: Vec<Pos2> = s.marks.iter().map(|p| view.to_screen(*p)).collect();
        for pair in corners.windows(2) {
            painter.line_segment([pair[0], pair[1]], Stroke::new(1.5, mark));
        }
        for p in &corners {
            painter.circle(*p, 4.0, Color32::from_rgb(0x20, 0x23, 0x27), Stroke::new(1.5, mark));
        }
        if s.stroke.is_empty() {
            return;
        }
        let width = (0.025 * view.rect.width().min(view.rect.height()) * 2.0).max(2.0);
        let colour = Color32::from_rgba_unmultiplied(245, 100, 70, 150);
        let points: Vec<Pos2> = s.stroke.iter().map(|p| view.to_screen(*p)).collect();
        if points.len() > 1 {
            painter.add(eframe::egui::Shape::line(points.clone(), Stroke::new(width, colour)));
        }
        for p in [points[0], points[points.len() - 1]] {
            painter.circle_filled(p, width / 2.0, colour);
        }
    }
}
