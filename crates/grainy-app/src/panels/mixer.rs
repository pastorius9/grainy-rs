//! The Colour mixer panel (색상 혼합). Port of the mixer of luma/app.py `build_ui` (the colour box over the eight
//! colours 빨강 … 자홍, the targeted button `TargetedAdjust.toggle('hsl')` with its 사진에서 조정할 항목 box, the
//! three sliders of the chosen colour: `hsl_changed`, `update_hsl`) and of luma/studio.py `build_color` (the mixer
//! mode box inserted on top, `mixer_mode`; the 포인트 색상 fold-out: pick, note, the list of points, six sliders,
//! remove, 선택 색상 범위 표시 and its note). The oracle shows one colour at a time in a combo box, not a grid of
//! bands; so does this panel.

use eframe::egui::Ui;
use grainy_i18n::tr;
use serde_json::Value;

use super::controls::{self, list_spec};
use super::Panel;
use crate::adjustments::{Place, Reset, Spec};
use crate::context::AppContext;
use crate::text::tr_with;
use crate::tools::point_color::{self, PointColors, CONTROLS};
use crate::tools::targeted::{self, Kind, TargetedAdjust};
use crate::widgets::{self, Button};

/// The eight colours of the mixer, in the oracle's order (`hsl[0..8]`).
pub const COLOURS: [&str; 8] = ["빨강", "주황", "노랑", "초록", "청록", "파랑", "보라", "자홍"];
/// The three columns of a colour (`hsl[i][0..3]`), also the items of 사진에서 조정할 항목.
pub const COLUMNS: [&str; 3] = ["색상", "채도", "밝기 (HSV)"];
/// The mixer modes (`mixer_mode`): value and Korean label.
pub const MODES: [(&str, &str); 2] = [("hsl", "HSL · 명도 기반"), ("hsv", "기존 HSV · 밝기 기반")];

/// The mixer panel.
pub struct Mixer;

impl Panel for Mixer {
    fn key(&self) -> &'static str {
        "mixer"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

/// Port of `hsl_changed`: `hsl[color][column] = value` (live; the slider's release commits).
pub fn hsl_changed(settings: &grainy_core::Settings, color: usize, column: usize, value: f64) -> Value {
    let mut values = settings.value("hsl").clone();
    if let Some(slot) = values.get_mut(color).and_then(|group| group.get_mut(column)) {
        *slot = Value::from(value);
    }
    values
}

/// Port of `update_hsl`: what the three sliders show for `color`.
pub fn shown_hsl(settings: &grainy_core::Settings, color: usize) -> [f64; 3] {
    let group = &settings.value("hsl")[color];
    [0, 1, 2].map(|i| group[i].as_f64().unwrap_or(0.0))
}

/// The point slider of `CONTROLS[i]`.
fn point_spec(i: usize) -> Spec {
    let (_, _, low, high, scale, reset) = CONTROLS[i];
    Spec { key: "point_colors", label: "", low, high, scale, reset: Reset::Value(reset), place: Place::Panel("") }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    targeted::sync(cx);
    point_color::sync(cx);

    // luma/studio.py: the mode box, inserted above the mixer.
    let current = cx.settings().str("mixer_mode").to_owned();
    let index = MODES.iter().position(|(v, _)| *v == current).unwrap_or(0);
    let names: Vec<String> = MODES.iter().map(|(_, n)| tr(n)).collect();
    if let Some(chosen) = controls::choice(ui, "mixer_mode", "", &names, index, width) {
        if MODES[chosen].0 != current && cx.doc.current().is_some() {
            cx.doc.edit_mut().set("mixer_mode", MODES[chosen].0);
            cx.doc.edit_mut().commit("보정 변경");
        }
    }

    // The colour box (`hsl_color`).
    let color = cx.state::<TargetedAdjust>().hsl_color.min(7);
    let colours: Vec<String> = COLOURS.iter().map(|c| tr(c)).collect();
    if let Some(chosen) = controls::choice(ui, "hsl_color", "", &colours, color, width) {
        cx.state::<TargetedAdjust>().hsl_color = chosen;
    }

    // The targeted button and 사진에서 조정할 항목.
    let on = targeted::checked(cx, Kind::Hsl);
    let target = cx.state::<TargetedAdjust>().hsl_target.min(2);
    let columns: Vec<String> = COLUMNS.iter().map(|c| tr(c)).collect();
    let mut clicked = false;
    let mut chosen_target = None;
    ui.horizontal(|ui| {
        let spacing = ui.spacing().item_spacing.x;
        let box_width = (width * 0.38).max(80.0);
        let response = Button::new(&tr("사진에서 드래그로 조정"))
            .width(width - box_width - spacing)
            .checked(on)
            .show(ui)
            .on_hover_text(tr("사진의 한 곳을 누른 채 위아래로 끌면 그 색에 해당하는 색상 혼합 값이 바뀝니다."));
        clicked = response.clicked();
        chosen_target = controls::choice(ui, "hsl_target", &tr("사진에서 조정할 항목"), &columns, target, box_width);
    });
    if clicked {
        targeted::toggle(cx, Kind::Hsl, !on);
    }
    if let Some(chosen) = chosen_target {
        cx.state::<TargetedAdjust>().hsl_target = chosen;
    }

    // The three sliders of the chosen colour (`hsl_widgets`).
    let color = cx.state::<TargetedAdjust>().hsl_color.min(7);
    let spec = list_spec("hsl", -100.0, 100.0);
    for (column, label) in COLUMNS.iter().enumerate() {
        cx.list_slider(ui, &spec, label, color, Some(column));
    }

    // 포인트 색상.
    let mut open = cx.foldouts.contains("point_color");
    widgets::foldout(ui, &tr("포인트 색상"), &mut open);
    cx.set_foldout("point_color", open);
    if open {
        point_colors(ui, cx, width);
    }
    point_color::refresh(cx);
}

/// The 포인트 색상 fold-out.
fn point_colors(ui: &mut Ui, cx: &mut AppContext, width: f32) {
    let has_photo = cx.doc.current().is_some();
    let picking = cx.state::<PointColors>().picking;
    if Button::new(&tr("선택 색상 · 사진에서 찍기")).width(width).checked(picking).show(ui).clicked() {
        point_color::start(cx);
    }
    let note = cx.state::<PointColors>().note().to_owned();
    widgets::muted(ui, &tr(&note));

    // The list of points (`points`); its accessible name is the fold-out's (the box is empty without points).
    let settings = cx.settings().clone();
    let count = point_color::points(&settings).len();
    {
        let state = cx.state::<PointColors>();
        if state.index >= count as isize || (state.index < 0 && count > 0) {
            state.load_points(&settings);
        }
    }
    let names: Vec<String> = (0..count).map(|i| tr_with("선택 색상 {0}", &[&(i + 1).to_string()])).collect();
    let index = cx.state::<PointColors>().index;
    let selected = usize::try_from(index).unwrap_or(usize::MAX);
    if let Some(chosen) = controls::choice(ui, "point_colors", &tr("포인트 색상"), &names, selected, width) {
        cx.state::<PointColors>().select(chosen as isize);
    }

    // The six sliders (`point_controls`).
    let (values, enabled) = cx.state::<PointColors>().shown(&settings);
    for (i, (key, label, ..)) in CONTROLS.iter().enumerate() {
        let spec = point_spec(i);
        let event = widgets::adjustment_row(ui, &format!("point.{key}"), &tr(label), &spec, values[i], enabled);
        if let Some(value) = event.set {
            // A slider emits only when its position moves.
            if has_photo && spec.position(value) != spec.position(values[i]) {
                let state = std::mem::take(cx.state::<PointColors>());
                state.changed(cx.doc.edit_mut(), key, value);
                *cx.state::<PointColors>() = state;
                cx.set_before(false);
            }
        }
        if event.commit && has_photo {
            cx.doc.edit_mut().commit("보정 변경");
        }
    }
    if Button::new(&tr("선택 색상 제거")).width(width).show(ui).clicked() && has_photo {
        let mut state = std::mem::take(cx.state::<PointColors>());
        state.remove(cx.doc.edit_mut());
        *cx.state::<PointColors>() = state;
    }
    let mut overlay = cx.state::<PointColors>().overlay_on;
    ui.add_enabled_ui(enabled, |ui| {
        if widgets::checkbox(ui, &mut overlay, &tr("선택 색상 범위 표시")).changed() {
            cx.state::<PointColors>().overlay_on = overlay;
        }
    });
    widgets::muted(ui, &tr("파란 표시가 진할수록 보정이 강하게 적용됩니다. 범위 표시는 사진에 저장되지 않습니다."));
}
