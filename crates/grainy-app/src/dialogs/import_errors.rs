//! Message boxes of the library column: the "photos that could not be read" box at the end of an import
//! (`QMessageBox.information(self, '읽을 수 없는 사진', '\n'.join(self.import_errors[:10]))`) and the
//! information boxes of folder removal (`FolderPanel.remove`, when an import or export is running).

use eframe::egui;

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// An information box with one 확인 button.
pub struct Notice {
    pub id: &'static str,
    /// Title and text, already translated.
    pub title: String,
    pub text: String,
}

impl Notice {
    /// The box listing the photos an import could not read: the first ten reasons, one per line.
    pub fn import_errors(errors: &[String]) -> Self {
        Self {
            id: "import_errors",
            title: tr("읽을 수 없는 사진"),
            text: errors.iter().take(10).cloned().collect::<Vec<_>>().join("\n"),
        }
    }

    /// The box of a folder removal that cannot start now.
    pub fn folder_busy() -> Self {
        Self {
            id: "folder_busy",
            title: tr("폴더 제거"),
            text: tr("사진 가져오기·내보내기와 카탈로그 작업을 마친 뒤 다시 시도하세요."),
        }
    }
}

impl Dialog for Notice {
    fn id(&self) -> &'static str {
        self.id
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        let mut closed = false;
        let response = egui::Modal::new(egui::Id::unique(("notice", self.id))).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(egui::RichText::new(&self.title).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            widgets::label(ui, &self.text, t::TEXT, t::FONT_SIZE);
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                    closed = true;
                }
            });
        });
        !(closed || response.should_close())
    }
}
