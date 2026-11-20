//! A message box (`QMessageBox`) with an icon, a text, an informative text, optional details and buttons, and
//! Grainy 정보… (luma/app.py `show_about`). The update boxes, the language-save warning and the about box are
//! made from it.

use eframe::egui;
use grainy_platform::RS_VERSION;

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// What a button does when it is pressed.
pub type Action = Box<dyn FnOnce(&mut AppContext)>;

/// The box's icon (`QMessageBox.Icon`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Information,
    Warning,
}

/// A message box. The first button is the default (the accept button), a button without an action just closes it.
pub struct MessageBox {
    pub id: &'static str,
    pub icon: Icon,
    /// The window title, already translated.
    pub title: String,
    /// `setText`.
    pub text: String,
    /// `setInformativeText`.
    pub informative: String,
    /// `setDetailedText` (shown in a box that scrolls).
    pub detail: String,
    /// The buttons' names with their actions.
    pub buttons: Vec<(String, Option<Action>)>,
}

impl MessageBox {
    fn make(id: &'static str, icon: Icon, title: String, text: String) -> Self {
        Self {
            id,
            icon,
            title,
            text,
            informative: String::new(),
            detail: String::new(),
            buttons: vec![(tr("확인"), None)],
        }
    }

    /// `QMessageBox.information` with the OK button.
    pub fn information(id: &'static str, title: String, text: String) -> Self {
        Self::make(id, Icon::Information, title, text)
    }

    /// `QMessageBox.warning` with the OK button.
    pub fn warning(id: &'static str, title: String, text: String) -> Self {
        Self::make(id, Icon::Warning, title, text)
    }
}

/// The attribution of the embedded Lensfun database (also in `THIRD_PARTY_NOTICES.md`).
pub const LENSFUN_NOTICE: &str = "렌즈 보정 데이터베이스는 Lensfun(https://lensfun.github.io/)의 것으로, CC BY-SA 3.0 라이선스(https://creativecommons.org/licenses/by-sa/3.0/)에 따라 수정 없이 들어 있습니다. 저작권은 Lensfun 기여자들에게 있습니다.";

/// Port of `show_about`: the version and the notice of the licences and trademarks. The version line names the Rust
/// build and its own version (`Grainy RS 0.2.1`), not the oracle's.
pub fn about_box() -> MessageBox {
    let mut message = MessageBox::information("about", tr("Grainy 정보"), format!("Grainy RS {RS_VERSION}"));
    let oracle = tr("원본을 건드리지 않는 사진 보정·관리 프로그램입니다.\n\nGrainy의 코드는 MIT 라이선스입니다. 함께 들어 있는 외부 부품의 라이선스 고지는 배포본의 THIRD_PARTY_LICENSES 폴더에 있습니다.\n\nAdobe, Lightroom, Photoshop은 Adobe Inc.의 상표입니다. Grainy는 Adobe가 만들거나 후원·보증한 프로그램이 아니며 Adobe와 관련이 없습니다. 그 밖의 상표는 각 소유자의 것입니다.");
    // The notice the Rust app adds: the built-in lens database (Lensfun, CC BY-SA 3.0) is third-party data.
    message.informative = format!("{oracle}\n\n{}", tr(LENSFUN_NOTICE));
    message.buttons.clear();
    message.buttons.push((tr("확인"), None));
    message
}

fn paint_icon(ui: &mut egui::Ui, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
    let (fill, letter) = match icon {
        Icon::Information => (egui::Color32::from_rgb(0x3f, 0x7f, 0x86), "i"),
        Icon::Warning => (egui::Color32::from_rgb(0xb8, 0x9c, 0x76), "!"),
    };
    ui.painter().circle_filled(rect.center(), 15.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, letter, t::bold(18.0), t::PRIMARY_TEXT);
}

impl Dialog for MessageBox {
    fn id(&self) -> &'static str {
        self.id
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut clicked = None;
        let response =
            egui::Modal::new(egui::Id::unique(("message_box", self.id))).frame(super::frame()).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(egui::RichText::new(&self.title).font(t::bold(14.0)).color(t::BRAND));
                ui.add_space(10.0);
                ui.horizontal_top(|ui| {
                    paint_icon(ui, self.icon);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        let width = ui.available_width();
                        let text = widgets::wrapped(ui, self.text.clone(), t::bold(13.0), t::TEXT, width);
                        let (rect, _) = ui.allocate_exact_size(text.size(), egui::Sense::hover());
                        ui.painter().galley(rect.min, text, t::TEXT);
                        if !self.informative.is_empty() {
                            ui.add_space(8.0);
                            widgets::label(ui, &self.informative, t::TEXT, t::FONT_SIZE);
                        }
                    });
                });
                if !self.detail.is_empty() {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(t::INPUT_BG)
                        .stroke(egui::Stroke::new(1.0, t::INPUT_BORDER))
                        .inner_margin(6)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(150.0).id_salt("detail").show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                widgets::label(ui, &self.detail, t::TEXT, t::FONT_SIZE);
                            });
                        });
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    for (index, (name, _)) in self.buttons.iter().enumerate().rev() {
                        let kind = if index == 0 { Kind::Primary } else { Kind::Normal };
                        if Button::new(name).kind(kind).show(ui).clicked() {
                            clicked = Some(index);
                        }
                        ui.add_space(6.0);
                    }
                });
            });
        if let Some(index) = clicked {
            if let Some(action) = self.buttons.get_mut(index).and_then(|(_, action)| action.take()) {
                action(cx);
            }
            return false;
        }
        !response.should_close()
    }
}
