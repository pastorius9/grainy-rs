//! The Masks panel's parts for the brush, the ranges and the sky (work package 9), drawn by `panels/masks.rs`:
//! the brush's 밝기 제한, 자동 마스크 and 자동 마스크 허용량 and the colour pick's 다듬기 with the pick note in
//! 도구 옵션, then the selected range's editor (범위 다듬기: 광도 최소 / 최대 / 경계 페더 or 다듬기, the picked
//! colours, 선택한 범위에 색 추가, 고른 색 제외) and the selected sky's (하늘 다듬기: 민감도, 경계 흐림). Port of
//! the matching widgets of luma/studio.py `build_masks`, `update_mask_ui` and `load_range_editor`.

use eframe::egui::Ui;
use serde_json::Value;

use super::controls;
use crate::adjustments::{Place, Reset, Spec};
use crate::context::AppContext;
use crate::masks::{actions, Mode, BRUSH_RANGES, SKY_DEFAULTS};
use crate::text::{tr, tr_with};
use crate::tools::{brush, range_pick};
use crate::widgets::{self, Button};

const fn spec(key: &'static str, label: &'static str, low: f64, high: f64, scale: f64, reset: f64) -> Spec {
    Spec { key, label, low, high, scale, reset: Reset::Value(reset), place: Place::Panel("masks") }
}

/// A slider row: the new slider position's value (`position / scale`, as `Adjustment.changed` sends it) and
/// whether it was released.
fn row(ui: &mut Ui, id: &str, spec: &Spec, value: f64) -> (Option<f64>, bool) {
    let event = widgets::adjustment_row(ui, &format!("masks.{id}"), &tr(spec.label), spec, value, true);
    let set = event.set.map(|v| (v * spec.scale).round() / spec.scale);
    (set, event.commit)
}

/// Brush options after the four sliders (`brush_range` row, `auto_mask`, `auto_tolerance`).
pub fn brush_options(ui: &mut Ui, cx: &mut AppContext, mode: Mode) {
    if mode != Mode::Brush {
        return;
    }
    let width = ui.available_width();
    let current = actions::state(cx).options.brush_range;
    let names: Vec<String> = BRUSH_RANGES.iter().map(|(_, title, _)| tr(title)).collect();
    let index = BRUSH_RANGES.iter().position(|(k, _, _)| *k == current).unwrap_or(0);
    let mut chosen = None;
    ui.horizontal(|ui| {
        widgets::label(ui, &tr("밝기 제한"), crate::theme::TEXT, crate::theme::FONT_SIZE)
            .on_hover_text(tr("칠한 곳 가운데 고른 밝기에만 보정이 들어갑니다. 닷지·번에 씁니다."));
        let room = ui.available_width();
        chosen = controls::choice(ui, "mask_brush_range", &tr("밝기 제한"), &names, index, room.min(width));
    });
    if let Some(i) = chosen {
        brush::set_brush_range(cx, BRUSH_RANGES[i].0);
    }
    let mut auto = actions::state(cx).options.auto_mask;
    if widgets::checkbox(ui, &mut auto, &tr("자동 마스크"))
        .on_hover_text(tr("처음 누른 곳과 비슷한 색 안에서만 칠합니다."))
        .changed()
    {
        actions::state(cx).options.auto_mask = auto;
    }
    let tolerance = spec("auto_tolerance", "자동 마스크 허용량", 0.02, 1.0, 100.0, 0.18);
    let position = actions::state(cx).options.auto_tolerance;
    let (set, _) = row(ui, "auto_tolerance", &tolerance, position as f64 / 100.0);
    if let Some(value) = set {
        actions::state(cx).options.auto_tolerance = (value * 100.0).round() as i64;
    }
}

/// The colour pick's 다듬기 and the pick note, after the brush note.
pub fn pick_options(ui: &mut Ui, cx: &mut AppContext, mode: Mode) {
    if mode == Mode::ColorRange {
        let tolerance = spec("tolerance", "다듬기", 0.02, 1.7, 100.0, 0.3);
        let position = actions::state(cx).options.tolerance;
        let (set, _) = row(ui, "pick_tolerance", &tolerance, position as f64 / 100.0);
        if let Some(value) = set {
            actions::state(cx).options.tolerance = (value * 100.0).round() as i64;
        }
    }
    if mode.picking() {
        let note = cx.state::<range_pick::RangeState>().note.clone();
        widgets::muted(ui, &tr(&note));
    }
}

/// The editor of the selected range or sky (`range_editor`, `sky_editor`).
pub fn editors(ui: &mut Ui, cx: &mut AppContext) {
    brush::sync_brush_range(cx);
    let Some(component) = actions::current_component(cx) else { return };
    let kind = component.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
    let width = ui.available_width();
    match kind.as_str() {
        "color" | "luma" => {
            widgets::section(ui, &tr("범위 다듬기"));
            let number = |key: &str, default: f64| component.get(key).and_then(Value::as_f64).unwrap_or(default);
            if kind == "luma" {
                let range = component.get("range").and_then(Value::as_array);
                let at =
                    |i: usize, default: f64| range.and_then(|r| r.get(i)).and_then(Value::as_f64).unwrap_or(default);
                let rows = [
                    (spec("low", "광도 최소", 0.0, 100.0, 1.0, 25.0), at(0, 0.25) * 100.0),
                    (spec("high", "광도 최대", 0.0, 100.0, 1.0, 75.0), at(1, 0.75) * 100.0),
                    (spec("softness", "광도 경계 페더", 0.1, 100.0, 10.0, 10.0), number("softness", 0.1) * 100.0),
                ];
                for (spec, value) in rows {
                    let (set, commit) = row(ui, spec.key, &spec, value);
                    if let Some(value) = set {
                        range_pick::range_changed(cx, spec.key, value);
                    }
                    if commit {
                        cx.doc.edit_mut().commit("보정 변경");
                    }
                }
            } else {
                let tolerance = spec("tolerance", "다듬기", 0.02, 1.7, 100.0, 0.3);
                let (set, commit) = row(ui, "range_tolerance", &tolerance, number("tolerance", 0.3));
                if let Some(value) = set {
                    range_pick::range_changed(cx, "tolerance", value);
                }
                if commit {
                    cx.doc.edit_mut().commit("보정 변경");
                }
                let count = match component.get("samples") {
                    Some(Value::Array(list)) => list.len(),
                    _ => 1,
                };
                let names: Vec<String> =
                    (0..count).map(|i| tr_with("색 {0} / {1}", &[&(i + 1).to_string(), &count.to_string()])).collect();
                let state = cx.state::<range_pick::RangeState>();
                state.sample = state.sample.min(count.saturating_sub(1));
                let index = state.sample;
                if let Some(i) = controls::choice(ui, "mask_range_samples", "", &names, index, width) {
                    cx.state::<range_pick::RangeState>().sample = i;
                }
                if Button::new(&tr("선택한 범위에 색 추가")).enabled(count < 5).width(width).show(ui).clicked()
                {
                    range_pick::range_add_mode(cx);
                }
                if Button::new(&tr("고른 색 제외")).enabled(count > 1).width(width).show(ui).clicked() {
                    range_pick::remove_range_sample(cx);
                }
            }
            widgets::muted(
                ui,
                &tr(if kind == "color" {
                    "같은 범위 안의 색들을 함께 선택합니다. 허용량을 낮추면 선택이 좁아집니다."
                } else {
                    "범위와 경계 흐림을 조절하며 사진과 선택 영역을 확인하세요."
                }),
            );
        }
        "sky" => {
            widgets::section(ui, &tr("하늘 다듬기"));
            for (key, title) in [("sensitivity", "민감도"), ("feather", "경계 흐림")] {
                let default = SKY_DEFAULTS.iter().find(|(k, _)| *k == key).map_or(0.0, |(_, v)| *v);
                let value = component.get(key).and_then(Value::as_f64).unwrap_or(default);
                let sky = spec(key, title, 0.0, 100.0, 1.0, default);
                let (set, commit) = row(ui, &format!("sky_{key}"), &sky, value);
                if let Some(value) = set {
                    range_pick::sky_changed(cx, key, value);
                }
                if commit {
                    cx.doc.edit_mut().commit("보정 변경");
                }
            }
            widgets::muted(
                ui,
                &tr("민감도를 올리면 옅은 구름과 안개까지 넓어집니다. 경계 흐림은 나뭇가지와 지붕선 경계의 부드러움입니다."),
            );
        }
        _ => {}
    }
}
