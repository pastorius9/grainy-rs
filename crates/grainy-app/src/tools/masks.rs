//! The Masks tool on the photo: linear and radial gradients drawn by dragging (also from or to outside the photo),
//! the handles of the selected gradient (move an end, move the whole by its middle; Esc puts them back), and the
//! red overlay of the selected mask. Port of luma/widgets.py `PhotoView` in the modes `linear`, `radial` and
//! `edit_component` (`mousePressEvent`, `mouseMoveEvent`, `mouseReleaseEvent`, `keyPressEvent`, the handles and
//! the gradient outline of `drawForeground`) with the `StudioTools` slots they signal (`preview_stroke`, `stroke`,
//! `move_component_handles`).
//!
//! The tool is active while the Masks tool strip entry is open; it takes the pointer only in its own modes (in no
//! mode the view pans). Work package 9's brush and range tools register before it and take their modes.

use std::cell::Cell;

use eframe::egui::{Color32, Key, Modifiers, Painter, Pos2, Shape, Stroke};

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::AppContext;
use crate::imagespace::ViewTransform;
use crate::masks::{actions, gradient, overlay, HandleDrag, MaskState, Mode};
use crate::panels::Tool;

/// The Masks canvas tool.
#[derive(Default)]
pub struct Masks {
    /// Where the frame was drawn at the last paint (the handles' hit test is in screen pixels).
    view: Cell<Option<ViewTransform>>,
}

/// The handle colour (`#f7dfbc`) and dot fill (`#202327`).
const HANDLE: Color32 = Color32::from_rgb(0xf7, 0xdf, 0xbc);
const HANDLE_FILL: Color32 = Color32::from_rgb(0x20, 0x23, 0x27);
/// The outline of a gradient being drawn (`QColor(245, 100, 70, 150)`).
const OUTLINE: Color32 = Color32::from_rgba_unmultiplied_const(245, 100, 70, 150);

impl Masks {
    /// Hands one pointer event to the tool with the frame drawn at `view` (scripted interaction in tests).
    pub fn pointer_at(&mut self, event: &PointerEvent, view: ViewTransform, cx: &mut AppContext) -> bool {
        self.view.set(Some(view));
        self.pointer(event, cx)
    }
}

/// Points of an ellipse with half axes `rx`, `ry` turned by `angle` degrees around `centre`.
fn ellipse(centre: Pos2, rx: f32, ry: f32, angle: f32) -> Vec<Pos2> {
    let (s, c) = angle.to_radians().sin_cos();
    (0..72)
        .map(|i| {
            let t = i as f32 / 72.0 * std::f32::consts::TAU;
            let (x, y) = (rx * t.cos(), ry * t.sin());
            Pos2::new(centre.x + x * c - y * s, centre.y + x * s + y * c)
        })
        .collect()
}

impl CanvasTool for Masks {
    fn name(&self) -> &'static str {
        "masks"
    }

    fn tool(&self) -> Option<Tool> {
        Some(Tool::Masks)
    }

    fn edits(&self) -> &'static [&'static str] {
        &["masks"]
    }

    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        if cx.doc.current().is_none() {
            return false;
        }
        actions::sync(cx);
        let mode = actions::state(cx).mode;
        let point = event.free;
        match mode {
            Mode::EditComponent => {
                let state = actions::state(cx);
                match event.kind {
                    PointerKind::Press => {
                        if state.handles.is_empty() {
                            return true;
                        }
                        if let Some(view) = self.view.get() {
                            if let Some(index) =
                                gradient::hit_handle(&state.handles, event.screen, |p| view.to_screen(p))
                            {
                                state.handle_drag =
                                    Some(HandleDrag { index, start: point, original: state.handles.clone() });
                            }
                        }
                        true
                    }
                    PointerKind::Drag => {
                        if let Some(drag) = &state.handle_drag {
                            state.handles = gradient::dragged(&drag.original, drag.index, drag.start, point);
                        }
                        true
                    }
                    PointerKind::Release => {
                        if let Some(drag) = state.handle_drag.take() {
                            let values = gradient::dragged(&drag.original, drag.index, drag.start, point);
                            state.handles = values.clone();
                            if !gradient::allclose(&values, &drag.original) {
                                actions::move_component_handles(cx, &values);
                            }
                        }
                        true
                    }
                    PointerKind::Click => true,
                    PointerKind::Hover => false,
                }
            }
            Mode::Linear | Mode::Radial => {
                match event.kind {
                    PointerKind::Press => {
                        actions::state(cx).points = vec![point];
                        actions::preview_stroke(cx, &[point]);
                    }
                    PointerKind::Drag => {
                        let state = actions::state(cx);
                        let Some(&first) = state.points.first() else { return true };
                        state.points = vec![first, point];
                        let points = state.points.clone();
                        actions::preview_stroke(cx, &points);
                    }
                    PointerKind::Release => {
                        let state = actions::state(cx);
                        let Some(&first) = state.points.first() else { return true };
                        state.points.clear();
                        actions::stroke(cx, &[first, point], event.modifiers.alt);
                    }
                    PointerKind::Click => {
                        actions::preview_stroke(cx, &[point]);
                        actions::stroke(cx, &[point, point], event.modifiers.alt);
                    }
                    PointerKind::Hover => return false,
                }
                true
            }
            _ => false,
        }
    }

    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        if key != Key::Escape || cx.doc.current().is_none() {
            return false;
        }
        let state = actions::state(cx);
        if state.mode == Mode::None {
            return false;
        }
        if let Some(drag) = state.handle_drag.take() {
            state.handles = drag.original;
        } else if !state.points.is_empty() || state.pending.is_some() {
            actions::cancel_stroke(cx);
        } else {
            actions::set_tool(cx, Mode::None);
        }
        true
    }

    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        self.view.set(Some(*view));
        overlay::paint(painter, view, cx);
        let Some(state) = cx.state_ref::<MaskState>() else { return };
        if state.mode == Mode::EditComponent && state.handles.len() >= 2 {
            let stroke = Stroke::new(1.5, HANDLE);
            let points: Vec<Pos2> = state.handles.iter().map(|p| view.to_screen(*p)).collect();
            let centre = Pos2::new((points[0].x + points[1].x) / 2.0, (points[0].y + points[1].y) / 2.0);
            if state.handle_kind == "radial" {
                let turn = (state.handle_angle as f32).to_radians();
                let (dx, dy) = (points[1].x - centre.x, points[1].y - centre.y);
                let local = (dx * turn.cos() + dy * turn.sin(), -dx * turn.sin() + dy * turn.cos());
                painter.add(Shape::closed_line(
                    ellipse(centre, local.0.abs(), local.1.abs(), state.handle_angle as f32),
                    stroke,
                ));
            } else {
                painter.line_segment([points[0], points[1]], stroke);
            }
            for p in [points[0], points[1], centre] {
                painter.circle(p, 5.0, HANDLE_FILL, stroke);
            }
        }
        if state.mode.gradient() && state.points.len() >= 2 {
            let a = view.to_screen(state.points[0]);
            let b = view.to_screen(state.points[state.points.len() - 1]);
            let stroke = Stroke::new(2.0, OUTLINE);
            if state.mode == Mode::Radial {
                let centre = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                let (rx, ry) = ((b.x - a.x).abs() / 2.0, (b.y - a.y).abs() / 2.0);
                painter.add(Shape::closed_line(ellipse(centre, rx, ry, 0.0), stroke));
            } else {
                painter.line_segment([a, b], stroke);
            }
        }
    }
}
