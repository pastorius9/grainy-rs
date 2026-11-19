//! 내보내기 기록: the export records of the selected photos (luma/workflow_ui.py `show_exports`): the photo, when
//! the file was saved, whether the photo was edited since, and the file. Newest first.

use eframe::egui;

use super::Dialog;
use crate::commands::export::HistoryRow;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{Button, Kind};

/// The box listing the records (read when the box opens).
pub struct ExportHistory {
    rows: Vec<HistoryRow>,
}

impl ExportHistory {
    /// A box with `rows`.
    pub fn new(rows: Vec<HistoryRow>) -> Self {
        Self { rows }
    }

    /// The lines shown.
    pub fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }
}

impl Dialog for ExportHistory {
    fn id(&self) -> &'static str {
        "export_history"
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        let mut closed = false;
        let response = egui::Modal::new(egui::Id::unique("export_history")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(940.0);
            ui.label(egui::RichText::new(tr("내보내기 기록")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            egui::ScrollArea::both().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("export_history_rows").num_columns(4).spacing([18.0, 6.0]).striped(true).show(
                    ui,
                    |ui| {
                        for title in ["사진", "저장 시각", "상태", "파일"] {
                            // The oracle's own wording of the column titles (en.json: 사진 → Photo).
                            ui.label(
                                egui::RichText::new(grainy_i18n::tr(title)).font(t::bold(t::FONT_SIZE)).color(t::MUTED),
                            );
                        }
                        ui.end_row();
                        for row in &self.rows {
                            // One line per cell (the oracle's tree scrolls sideways instead of wrapping).
                            for text in [&row.name, &row.created, &row.status, &row.path] {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(text).font(t::font(t::FONT_SIZE)).color(t::TEXT),
                                    )
                                    .extend(),
                                );
                            }
                            ui.end_row();
                        }
                    },
                );
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::new(&tr("닫기")).kind(Kind::Primary).show(ui).clicked() {
                    closed = true;
                }
            });
        });
        !(closed || response.should_close())
    }
}
