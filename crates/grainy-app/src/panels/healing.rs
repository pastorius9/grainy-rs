//! The Healing tool's panel (복구): port of luma/studio.py `build_retouch` and `update_healing_controls`. The
//! dust detection buttons run `heal.detect_dust` and `heal.detect_dust_batch` (work package 25).

use eframe::egui::Ui;

use super::controls::hint;
use super::Panel;
use crate::adjustments::{Place, Reset, Spec};
use crate::commands::Invocation;
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::tools::healing::{self, HealingState};
use crate::widgets::{self, Button, Kind};

/// `Adjustment('radius', '복구 브러시 크기', .002, .12, 1000)`.
pub const RADIUS: Spec = Spec {
    key: "retouch_radius",
    label: "복구 브러시 크기",
    low: 0.002,
    high: 0.12,
    scale: 1000.0,
    reset: Reset::Zero,
    place: Place::Panel("healing"),
};

/// `Adjustment('feather', '복구 경계 흐림', 0, 100)`.
pub const FEATHER: Spec = Spec {
    key: "retouch_feather",
    label: "복구 경계 흐림",
    low: 0.0,
    high: 100.0,
    scale: 1.0,
    reset: Reset::Zero,
    place: Place::Panel("healing"),
};

/// The kind buttons: (title, kind).
pub const TOOLS: [(&str, &str); 4] =
    [("복제 도장", "clone"), ("힐링 브러시", "heal"), ("먼지·흠집 제거", "inpaint"), ("적목 보정", "red_eye")];

/// The healing panel.
pub struct Healing;

impl Panel for Healing {
    fn key(&self) -> &'static str {
        "healing"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        healing::sync(cx);
        body(ui, cx);
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.y = 6.0;
    widgets::muted(ui, &tr("원본을 보존하며 복구 동작을 보정 기록에 저장합니다."));
    let dust = crate::commands::enabled(cx, "heal.detect_dust");
    if Button::new(&tr("먼지 자동 감지 · 후보 확인…")).width(width).enabled(dust).show(ui).clicked() {
        cx.run("heal.detect_dust");
    }
    let batch = crate::commands::enabled(cx, "heal.detect_dust_batch");
    if Button::new(&tr("여러 사진의 먼지 감지…")).width(width).enabled(batch).show(ui).clicked() {
        cx.run("heal.detect_dust_batch");
    }
    widgets::muted(ui, &tr("검은 점과 흰 먼지를 찾아, 확인한 후보만 한 번에 제거합니다."));
    for (title, kind) in TOOLS {
        let text = tr(title);
        let response = Button::new(&text).width(width).show(ui);
        let response =
            if kind == healing::HEAL { response.on_hover_text(format!("{text} ({})", hint("Q"))) } else { response };
        if response.clicked() {
            cx.run_with("heal.mode", &Invocation::value(kind));
        }
    }
    widgets::muted(ui, &tr("힐링: 지울 곳을 여러 번 칠한 뒤 실행하세요. 붉은 표시는 아직 적용되지 않은 영역입니다. Alt+클릭으로 원본을 직접 지정할 수 있습니다."));
    let state = cx.state::<HealingState>();
    let (note, radius, feather, waiting, can_clear) =
        (state.source_note(), state.radius_value(), state.feather, state.staged.len(), state.can_clear());
    widgets::muted(ui, &tr(note));
    if Button::new(&tr("힐링 원본 자동 선택으로 복귀")).width(width).show(ui).clicked() {
        cx.run("heal.automatic_source");
    }
    let event = widgets::adjustment_row(ui, RADIUS.key, &tr(RADIUS.label), &RADIUS, radius, true);
    if let Some(value) = event.set {
        healing::set_radius_position(cx, RADIUS.position(value));
    }
    let event = widgets::adjustment_row(ui, FEATHER.key, &tr(FEATHER.label), &FEATHER, feather as f64, true);
    if let Some(value) = event.set {
        healing::set_feather(cx, FEATHER.position(value));
    }
    widgets::muted(ui, &tr_with("실행 대기: {0}개", &[&waiting.to_string()]));
    let text = tr("실행 · 표시한 곳 제거");
    let kind = if waiting > 0 { Kind::Primary } else { Kind::Normal };
    let apply = Button::new(&text).kind(kind).width(width).enabled(waiting > 0);
    if apply.show(ui).clicked() {
        cx.run("heal.apply");
    }
    let labels = [tr("마지막 표시 취소"), tr("표시 모두 취소")];
    let row = [Button::new(&labels[0]).enabled(waiting > 0), Button::new(&labels[1]).enabled(can_clear)];
    match widgets::button_row(ui, &row, width, 4.0) {
        Some(0) => {
            cx.run("heal.remove_mark");
        }
        Some(_) => {
            cx.run("heal.clear_marks");
        }
        None => {}
    }
    widgets::muted(
        ui,
        &tr("Enter: 실행 · Esc: 현재 획 / 대기 표시 취소. 다른 사진으로 이동하면 실행 전 표시는 취소됩니다."),
    );
    let count = cx.settings().value("retouch").as_array().map_or(0, Vec::len);
    widgets::muted(ui, &tr_with("복구 동작 {0}개", &[&count.to_string()]));
    if Button::new(&tr("마지막 복구 제거")).width(width).show(ui).clicked() {
        cx.run("heal.remove_last");
    }
    if Button::new(&tr("도구 끝내기")).width(width).show(ui).clicked() {
        cx.run("heal.finish");
    }
}
