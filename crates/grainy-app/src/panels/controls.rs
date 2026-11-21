//! The controls panels are made of, on the application context: slider rows bound to settings keys, list
//! sliders, check boxes, choice boxes, and the placeholder of editors that come in a later work package.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind, Ui, Vec2};
use grainy_i18n::shortcuts::key_hint;
use grainy_i18n::{tr, Platform};
use serde_json::Value;

use crate::adjustments::{self, Place, Reset, Spec};
use crate::context::AppContext;
use crate::theme as t;
use crate::widgets;

/// A slider bound to one entry of a list value (`parametric[i]`, `grading[i][j]`, `calibration[i][j]`).
pub fn list_spec(key: &'static str, low: f64, high: f64) -> Spec {
    Spec { key, label: "", low, high, scale: 1.0, reset: Reset::Zero, place: Place::Panel("") }
}

/// The key written for a tooltip (`key_hint`).
pub fn hint(key: &str) -> String {
    key_hint(key, Platform::current()).unwrap_or_else(|| key.to_owned())
}

impl AppContext {
    /// The slider rows of one place of [`adjustments::ADJUSTMENTS`].
    pub fn sliders(&mut self, ui: &mut Ui, place: Place) {
        for spec in adjustments::at(place) {
            self.slider(ui, spec, true);
        }
    }

    /// One slider row on its settings key: live values while dragging, one undo step when released.
    pub fn slider(&mut self, ui: &mut Ui, spec: &Spec, enabled: bool) {
        let value = self.settings().f64(spec.key);
        let event = widgets::adjustment(ui, spec, value, enabled);
        self.apply_row(spec.key, event);
    }

    /// A slider on `key[index]` or `key[index][column]`.
    pub fn list_slider(&mut self, ui: &mut Ui, spec: &Spec, label: &str, index: usize, column: Option<usize>) {
        let list = self.settings().value(spec.key).clone();
        let entry = match column {
            Some(c) => &list[index][c],
            None => &list[index],
        };
        let value = entry.as_f64().unwrap_or(0.0);
        let id = match column {
            Some(c) => format!("{}.{index}.{c}", spec.key),
            None => format!("{}.{index}", spec.key),
        };
        let event = widgets::adjustment_row(ui, &id, &tr(label), spec, value, true);
        if let Some(new) = event.set {
            let mut list = list;
            let slot = match column {
                Some(c) => list.get_mut(index).and_then(|row| row.get_mut(c)),
                None => list.get_mut(index),
            };
            if let Some(slot) = slot {
                *slot = Value::from(new);
                self.doc.edit_mut().set(spec.key, list);
            }
        }
        if event.commit {
            self.doc.edit_mut().commit("보정 변경");
        }
    }

    /// A check box on a boolean settings key: one undo step.
    pub fn setting_checkbox(&mut self, ui: &mut Ui, key: &str, text: &str) {
        let mut value = self.settings().bool(key);
        if widgets::checkbox(ui, &mut value, &tr(text)).changed() {
            let edit = self.doc.edit_mut();
            edit.set(key, value);
            edit.commit("보정 변경");
        }
    }
}

/// A `QComboBox` over `names`; returns the index chosen this frame.
pub fn choice(ui: &mut Ui, id: &str, accessible: &str, names: &[String], index: usize, width: f32) -> Option<usize> {
    let mut chosen = None;
    let selected = names.get(index).cloned().unwrap_or_default();
    ui.scope(|ui| {
        if !t::in_develop_column(ui) {
            ui.spacing_mut().button_padding = Vec2::new(7.0, 7.0);
            ui.spacing_mut().interact_size.y = 30.0;
        }
        let response =
            egui::ComboBox::from_id_salt(id).width(width - 2.0).selected_text(selected.clone()).show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    if ui.selectable_label(i == index, name).clicked() {
                        chosen = Some(i);
                    }
                }
            });
        // The accessible name: the choice's own name where the oracle gives one, else what it shows.
        let label = if accessible.is_empty() { selected } else { accessible.to_owned() };
        response.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &label));
    });
    chosen
}

/// A titled box standing in for an editor that comes in a later work package (curves, mixer, masks, healing).
pub fn placeholder(ui: &mut Ui, title: &str, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::ZERO,
        t::HISTOGRAM_BG,
        Stroke::new(1.0, Color32::from_rgb(0x31, 0x37, 0x3e)),
        StrokeKind::Inside,
    );
    for i in 1..4 {
        let x = rect.left() + rect.width() * i as f32 / 4.0;
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        painter.vline(x, rect.y_range(), Stroke::new(1.0, Color32::from_rgb(0x26, 0x2b, 0x31)));
        painter.hline(rect.x_range(), y, Stroke::new(1.0, Color32::from_rgb(0x26, 0x2b, 0x31)));
    }
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, title, t::font(t::FONT_SIZE), t::MUTED);
}
