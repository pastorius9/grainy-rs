//! Canvas tools: tools that take the pointer and keys on the photo and draw over it (crop, healing, masks, the
//! white-balance and colour pickers, guided upright, targeted adjustment). The photo view converts the pointer to
//! image coordinates ([`crate::imagespace`]) and hands it to the open tool; the tool edits the settings it owns
//! through the context and paints its overlay.
//!
//! **Adding a tool:** write `tools/<name>.rs` with a type implementing [`CanvasTool`] and add one line to
//! [`registry`]. The tool is active while `cx.tool()` is its [`Tool`] (the strip) or while [`CanvasTool::active`]
//! says so (pickers that are buttons of a panel).

pub mod brush;
pub mod crop;
pub mod eyedropper;
pub mod guides;
pub mod healing;
pub mod point_color;
pub mod range_pick;
pub mod targeted;

use eframe::egui::{self, Key, Modifiers, Painter, Pos2};

use crate::context::AppContext;
use crate::imagespace::ViewTransform;
use crate::panels::Tool;

pub mod masks;

/// What the pointer did on the photo, in frame coordinates (fractions of the displayed picture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerKind {
    /// Pressed (a drag may follow).
    Press,
    /// Moved while pressed.
    Drag,
    /// Released.
    Release,
    /// A click without a drag.
    Click,
    /// Moved without a button (hover feedback).
    Hover,
}

/// A pointer event on the photo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    pub kind: PointerKind,
    /// The screen position.
    pub screen: Pos2,
    /// `normalized_point`: on the frame, clamped to it.
    pub frame: [f64; 2],
    /// `free_point`: on the frame, off it too.
    pub free: [f64; 2],
    pub modifiers: Modifiers,
}

/// A tool working on the photo view.
pub trait CanvasTool {
    /// A stable name (`crop`, `healing`, `masks`, `white_balance`…).
    fn name(&self) -> &'static str;

    /// The strip entry that opens it, if any.
    fn tool(&self) -> Option<Tool> {
        None
    }

    /// Whether it takes the pointer now (by default: while its strip entry is selected).
    fn active(&self, cx: &AppContext) -> bool {
        self.tool().is_some_and(|t| cx.tool() == t)
    }

    /// The settings keys it edits (its panel's, for the switch-on rule and tests).
    fn edits(&self) -> &'static [&'static str] {
        &[]
    }

    /// A pointer event; returns whether the tool used it (otherwise the view pans and zooms).
    fn pointer(&mut self, _event: &PointerEvent, _cx: &mut AppContext) -> bool {
        false
    }

    /// A key while the tool is active (Enter, Esc, [ and ]…); returns whether it was used.
    fn key(&mut self, _key: Key, _modifiers: Modifiers, _cx: &mut AppContext) -> bool {
        false
    }

    /// Draws the overlay over the photo.
    fn paint(&self, _painter: &Painter, _view: &ViewTransform, _cx: &AppContext) {}

    /// Draws what shows over the photo whether the tool is active or not (a panel's overlay), under the active
    /// tool's overlay.
    fn paint_passive(&self, _painter: &Painter, _view: &ViewTransform, _cx: &AppContext) {}
}

/// A tool whose interaction is a later work package: it is registered (so its strip entry and its panel exist)
/// and takes nothing.
pub struct Pending {
    pub name: &'static str,
    pub tool: Tool,
    pub edits: &'static [&'static str],
}

impl CanvasTool for Pending {
    fn name(&self) -> &'static str {
        self.name
    }

    fn tool(&self) -> Option<Tool> {
        Some(self.tool)
    }

    fn edits(&self) -> &'static [&'static str] {
        self.edits
    }
}

/// The tools, one line each.
pub fn registry() -> Vec<Box<dyn CanvasTool>> {
    vec![
        Box::new(guides::Guides),
        Box::new(crop::CropTool),
        Box::new(healing::Healing),
        Box::new(brush::Brush),
        Box::new(range_pick::RangePick),
        Box::new(crate::panels::masks_local::LocalPointTool),
        Box::new(masks::Masks::default()),
        Box::new(targeted::Targeted),
        Box::new(point_color::PointColor),
        Box::new(eyedropper::Eyedropper),
    ]
}

/// The photo view's part: hands this frame's pointer and keys to the active tool (its overlay is drawn by [`paint`]
/// once the picture is on screen). Returns
/// whether a tool used the pointer (the view then does not pan).
pub fn dispatch(
    ui: &egui::Ui,
    response: &egui::Response,
    view: &ViewTransform,
    cx: &mut AppContext,
    tools: &mut [Box<dyn CanvasTool>],
) -> bool {
    let Some(tool) = tools.iter_mut().find(|t| t.active(cx)) else { return false };
    let modifiers = ui.input(|i| i.modifiers);
    let mut used = false;
    let last_pos_id = egui::Id::unique("canvas_tool_last_pos");
    let raw_pos = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos())
        .or_else(|| ui.input(|i| i.pointer.latest_pos()));
    if let Some(pos) = raw_pos {
        ui.ctx().data_mut(|d| d.insert_temp(last_pos_id, pos));
    }
    let position = raw_pos.or_else(|| ui.ctx().data(|d| d.get_temp::<Pos2>(last_pos_id)));
    if let Some(screen) = position {
        let press = response.drag_started_by(egui::PointerButton::Primary);
        let drag = response.dragged_by(egui::PointerButton::Primary) && !press;
        let release = response.drag_stopped_by(egui::PointerButton::Primary);
        let click = response.clicked_by(egui::PointerButton::Primary);
        let hover = response.hovered() && !response.dragged_by(egui::PointerButton::Primary) && raw_pos.is_some();
        let kinds = [
            (press, PointerKind::Press),
            (drag, PointerKind::Drag),
            (release, PointerKind::Release),
            (click, PointerKind::Click),
            (hover, PointerKind::Hover),
        ];
        for (happened, kind) in kinds {
            if happened {
                // A press is where the button went down (egui reports a drag a few points later).
                let screen = match kind {
                    PointerKind::Press => ui.input(|i| i.pointer.press_origin()).unwrap_or(screen),
                    _ => screen,
                };
                let event = PointerEvent {
                    kind,
                    screen,
                    frame: view.to_frame(screen),
                    free: view.to_frame_free(screen),
                    modifiers,
                };
                used |= tool.pointer(&event, cx);
            }
        }
    }
    if !cx.modal_open() && !ui.ctx().text_edit_focused() && !egui::Popup::is_any_open(ui.ctx()) {
        let keys: Vec<Key> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, .. } => Some(*key),
                    _ => None,
                })
                .collect()
        });
        for key in keys {
            tool.key(key, modifiers, cx);
        }
    }
    used
}

/// Draws the active tool's overlay over the photo (after the photo, at the frame's place in this frame).
pub fn paint(ui: &egui::Ui, view: &ViewTransform, cx: &AppContext, tools: &[Box<dyn CanvasTool>]) {
    for tool in tools {
        tool.paint_passive(ui.painter(), view, cx);
    }
    if let Some(tool) = tools.iter().find(|t| t.active(cx)) {
        tool.paint(ui.painter(), view, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_strip_tool_is_registered() {
        let tools = registry();
        for (tool, _) in crate::panels::TOOL_STRIP.iter().filter(|(t, _)| *t != Tool::Edit) {
            assert!(tools.iter().any(|t| t.tool() == Some(*tool)), "{tool:?}");
        }
        let mut names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        names.dedup();
        assert_eq!(names.len(), tools.len());
    }
}
