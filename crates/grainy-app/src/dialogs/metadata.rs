//! The metadata box (`LibraryManager.metadata`: ten fields and the keywords, 저장 / 취소) and the box that asks how
//! many minutes to move the capture times (`QInputDialog.getInt`, `shift_date`).

use eframe::egui;

use super::Dialog;
use crate::commands::metadata::{self as commands, Form, FIELDS};
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The buttons of the boxes: the primary one and 취소. `Some(true)` for the primary, `Some(false)` for cancel.
fn buttons(ui: &mut egui::Ui, primary: &str) -> Option<bool> {
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if widgets::button(ui, &tr("취소")).clicked() {
            answer = Some(false);
        }
        ui.add_space(6.0);
        if Button::new(primary).kind(Kind::Primary).show(ui).clicked() {
            answer = Some(true);
        }
    });
    answer
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(t::bold(14.0)).color(t::BRAND));
    ui.add_space(8.0);
}

/// 메타데이터 · N장.
pub struct MetadataDialog {
    ids: Vec<i64>,
    /// The values being edited (the fields' text); public so tests can fill the box.
    pub form: Form,
}

impl MetadataDialog {
    /// The box for photos `ids`, with the first photo's values in it.
    pub fn new(ids: Vec<i64>, form: Form) -> Self {
        Self { ids, form }
    }
}

impl Dialog for MetadataDialog {
    fn id(&self) -> &'static str {
        "metadata"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let count = self.ids.len().to_string();
        let response = egui::Modal::new(egui::Id::unique("metadata_dialog")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(520.0);
            title(ui, &tr_with("메타데이터 · {0}장", &[&count]));
            egui::Grid::new("metadata_form").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                let rows =
                    FIELDS.iter().map(|(key, label)| (*key, *label)).chain([("keywords", "키워드 · 여행|일본, 필름")]);
                for (index, (key, label)) in rows.enumerate() {
                    ui.allocate_ui_with_layout(
                        egui::vec2(220.0, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new(tr(label)).font(t::font(t::FONT_SIZE)).color(t::TEXT));
                        },
                    );
                    ui.allocate_ui(egui::vec2(280.0, 28.0), |ui| {
                        let text =
                            if index < FIELDS.len() { &mut self.form.values[index] } else { &mut self.form.keywords };
                        widgets::line_edit(ui, &format!("metadata_{key}"), text, "");
                    });
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            widgets::muted(ui, &tr("변경한 항목만 선택한 사진에 적용합니다. 원본 파일은 수정하지 않습니다."));
            ui.add_space(10.0);
            answer = buttons(ui, &tr("저장"));
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                commands::save_form(cx, &self.ids, &self.form);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

/// 촬영 시각 이동: the minutes to move by (−525600 … 525600).
pub struct ShiftDialog {
    /// The number in the box; public so tests can fill it.
    pub minutes: i64,
}

impl ShiftDialog {
    pub fn new() -> Self {
        Self { minutes: 0 }
    }
}

impl Default for ShiftDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl Dialog for ShiftDialog {
    fn id(&self) -> &'static str {
        "metadata_shift"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("metadata_shift")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(380.0);
            title(ui, &tr("촬영 시각 이동"));
            widgets::label(ui, &tr("이동할 분 (예: +540 / -60)"), t::TEXT, t::FONT_SIZE);
            ui.add_space(4.0);
            ui.add(egui::DragValue::new(&mut self.minutes).range(-525_600..=525_600).speed(1.0));
            ui.add_space(10.0);
            answer = buttons(ui, &tr("확인"));
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                commands::shift_now(cx, self.minutes);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}
