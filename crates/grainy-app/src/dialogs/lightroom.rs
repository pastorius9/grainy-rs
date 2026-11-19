//! The review box of the Lightroom import (luma/lightroom_import_dialog.py `reviewed`'s `QMessageBox`): Lightroom
//! 가져오기 범위, what the import brings in, the check box 지원하는 보정값·이력·스냅샷도 변환 (off at first) and the buttons
//! 가져오기 / 취소 (취소 is the default; Esc answers 취소). The question lives in
//! [`crate::commands::lightroom::LightroomState`]; the answer goes to
//! [`crate::commands::lightroom::answer_review`].

use eframe::egui;

use super::Dialog;
use crate::commands::lightroom::{answer_review, LightroomState};
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The review box.
pub struct ReviewDialog;

impl Dialog for ReviewDialog {
    fn id(&self) -> &'static str {
        "lightroom_review"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let Some(review) = cx.state::<LightroomState>().review.clone() else { return false };
        let mut convert = review.convert;
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("lightroom_review")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(egui::RichText::new(tr("Lightroom 가져오기 범위")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            let width = ui.available_width();
            let text = widgets::wrapped(ui, review.text.clone(), t::font(t::FONT_SIZE), t::TEXT, width);
            let (rect, _) = ui.allocate_exact_size(text.size(), egui::Sense::hover());
            ui.painter().galley(rect.min, text, t::TEXT);
            ui.add_space(10.0);
            widgets::checkbox(
                ui,
                &mut convert,
                &tr("지원하는 보정값·이력·스냅샷도 변환 (Adobe와 결과가 다를 수 있음)"),
            );
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::new(&tr("취소")).kind(Kind::Primary).show(ui).clicked() {
                    answer = Some(false);
                }
                ui.add_space(6.0);
                if widgets::button(ui, &tr("가져오기")).clicked() {
                    answer = Some(true);
                }
            });
        });
        if let Some(open) = cx.state::<LightroomState>().review.as_mut() {
            open.convert = convert;
        }
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(import) = answer {
            answer_review(cx, import, convert);
            return false;
        }
        true
    }
}
