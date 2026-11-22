//! The Colour grading panel (컬러 그레이딩): interactive 3-way colour wheels + numeric sliders
//! for shadows, midtones and highlights.

use eframe::egui::{self, Color32, Sense, Stroke, Ui, Vec2};
use grainy_i18n::tr;
use serde_json::Value;

use super::controls::list_spec;
use super::Panel;
use crate::adjustments::Place;
use crate::context::AppContext;
use crate::theme as t;
use crate::widgets;

/// The grading panel.
pub struct Grading;

impl Panel for Grading {
    fn key(&self) -> &'static str {
        "grading"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

fn hsv_rgb(hue_deg: f32, sat: f32, val: f32) -> Color32 {
    let h = (hue_deg.rem_euclid(360.0)) / 60.0;
    let c = val * sat;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = val - c;
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color32::from_rgb(((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

fn paint_wheel(ui: &mut Ui, cx: &mut AppContext, index: usize, short_title: &str, diameter: f32) {
    let has_photo = cx.doc.current().is_some();
    let (hue, sat) = {
        let g = cx.settings().value("grading");
        let h = g.get(index).and_then(|r| r.get(0)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let s = g.get(index).and_then(|r| r.get(1)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        (h, s)
    };
    ui.vertical_centered(|ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
        let center = rect.center();
        let radius = diameter * 0.44;
        let painter = ui.painter();
        // Subtle dark disc background
        painter.circle_filled(center, radius, t::INPUT_BG);
        // 36-segment hue ring around perimeter
        let segments = 36usize;
        for seg in 0..segments {
            let a0 = (seg as f32 / segments as f32) * std::f32::consts::TAU;
            let a1 = ((seg + 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let p0 = center + Vec2::new(a0.cos(), a0.sin()) * radius;
            let p1 = center + Vec2::new(a1.cos(), a1.sin()) * radius;
            let deg = seg as f32 * (360.0 / segments as f32);
            painter.line_segment([p0, p1], Stroke::new(3.0, hsv_rgb(deg, 0.82, 0.85)));
        }
        // Subtle center crosshair
        painter.line_segment([center - Vec2::new(4.0, 0.0), center + Vec2::new(4.0, 0.0)], Stroke::new(1.0, t::GROOVE));
        painter.line_segment([center - Vec2::new(0.0, 4.0), center + Vec2::new(0.0, 4.0)], Stroke::new(1.0, t::GROOVE));

        let mut next: Option<(f64, f64)> = None;
        let mut commit = false;
        if has_photo {
            if response.double_clicked() {
                next = Some((0.0, 0.0));
                commit = true;
            } else if response.dragged_by(egui::PointerButton::Primary)
                || response.clicked_by(egui::PointerButton::Primary)
            {
                if let Some(pos) = response.interact_pointer_pos() {
                    let delta = pos - center;
                    let dist = (delta.length() / radius).clamp(0.0, 1.0);
                    let angle = delta.y.atan2(delta.x).to_degrees().rem_euclid(360.0);
                    next = Some((f64::from(angle.round()), f64::from((dist * 100.0).round())));
                }
                if response.clicked_by(egui::PointerButton::Primary) {
                    commit = true;
                }
            }
            if response.drag_stopped() {
                commit = true;
            }
        }
        let (shown_h, shown_s) = match next {
            Some((h, s)) => (h as f32, s as f32),
            None => (hue, sat),
        };
        let rad = shown_h.to_radians();
        let dot = center + Vec2::new(rad.cos(), rad.sin()) * (radius * (shown_s / 100.0).clamp(0.0, 1.0));
        if shown_s > 0.5 {
            painter.line_segment([center, dot], Stroke::new(1.0, Color32::from_white_alpha(120)));
        }
        let tint = if shown_s > 0.5 { hsv_rgb(shown_h, (shown_s / 100.0).clamp(0.2, 1.0), 0.95) } else { t::HANDLE };
        painter.circle(dot, 4.5, tint, Stroke::new(1.2, Color32::WHITE));
        widgets::muted(ui, &crate::text::tr(short_title));

        if let Some((h, s)) = next {
            let mut values = cx.settings().value("grading").clone();
            if let Some(row) = values.get_mut(index).and_then(Value::as_array_mut) {
                if row.len() >= 2 {
                    row[0] = Value::from(h);
                    row[1] = Value::from(s);
                    cx.doc.edit_mut().set("grading", values);
                    cx.before = false;
                }
            }
        }
        if commit && has_photo {
            cx.doc.edit_mut().commit("보정 변경");
        }
    });
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let wheel = ((width - 12.0) / 3.0).clamp(58.0, 80.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, title) in ["그림자", "중간톤", "하이라이트"].iter().enumerate() {
            paint_wheel(ui, cx, i, title, wheel);
        }
    });
    ui.add_space(4.0);
    let ranges = [(0.0, 360.0), (0.0, 100.0), (-100.0, 100.0)];
    for (i, title) in ["그림자 컬러 그레이딩", "중간톤 컬러 그레이딩", "하이라이트 컬러 그레이딩"].iter().enumerate()
    {
        widgets::muted(ui, &tr(title));
        for (j, name) in ["색상", "채도", "명도"].iter().enumerate() {
            let spec = list_spec("grading", ranges[j].0, ranges[j].1);
            cx.list_slider(ui, &spec, name, i, Some(j));
        }
    }
    cx.sliders(ui, Place::Panel("grading"));
}
