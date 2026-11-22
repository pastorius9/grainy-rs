//! The Detail panel (디테일): standard sharpening & noise reduction sliders plus the local
//! GPU-accelerated AI RAW Denoise Engine (`ai_denoise`).

use eframe::egui::Ui;

use super::Panel;
use crate::adjustments::{Place, Reset, Spec};
use crate::context::AppContext;
use crate::text::tr;
use crate::widgets;

const AI_DENOISE_SPEC: Spec = Spec {
    key: "ai_denoise",
    label: "AI 노이즈 제거",
    low: 0.0,
    high: 100.0,
    scale: 1.0,
    reset: Reset::Zero,
    place: Place::Panel("detail"),
};

/// The Detail panel.
pub struct Detail;

impl Panel for Detail {
    fn key(&self) -> &'static str {
        "detail"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        cx.sliders(ui, Place::Panel("detail"));
        ui.add_space(4.0);
        let value = cx.settings().f64(AI_DENOISE_SPEC.key);
        let event =
            widgets::adjustment_row(ui, AI_DENOISE_SPEC.key, &tr(AI_DENOISE_SPEC.label), &AI_DENOISE_SPEC, value, true);
        cx.apply_row(AI_DENOISE_SPEC.key, event);
        widgets::muted(ui, &tr("GPU 협업 패치 AI 디노이즈 · 로컬 오프라인 처리"));
    }
}
