//! The language box (luma/language_dialog.py `LanguageDialog`): asked at the first run (`Continue / 시작`) and from
//! 설정 → 언어 / Language… (`Apply / 적용`). Its texts are written in both languages, as in the oracle.

use eframe::egui;
use grainy_i18n::Language;

use super::Dialog;
use crate::commands::settings;
use crate::context::AppContext;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The two languages of the box, each written in its own language, with their codes (`addItem(text, data)`).
pub const LANGUAGES: [(&str, Language); 2] = [("한국어", Language::Korean), ("English", Language::English)];

/// The language box.
pub struct LanguageDialog {
    first_run: bool,
    selected: Language,
}

impl LanguageDialog {
    /// `LanguageDialog(current, parent, first_run)`: `current`, or Korean when the system language is Korean and
    /// English otherwise.
    pub fn new(current: Option<Language>, first_run: bool, system_korean: bool) -> Self {
        let selected = current.unwrap_or(if system_korean { Language::Korean } else { Language::English });
        Self { first_run, selected }
    }

    /// `selected_language`.
    pub fn selected(&self) -> Language {
        self.selected
    }

    /// Chooses an item of the box (`setCurrentIndex`).
    pub fn select(&mut self, language: Language) {
        self.selected = language;
    }

    /// The window title.
    pub fn title() -> &'static str {
        "Grainy · Language / 언어"
    }

    /// The label above the box.
    pub fn heading(&self) -> &'static str {
        if self.first_run {
            "Welcome to grainy / 그레이니에 오신 것을 환영합니다"
        } else {
            "Interface language / 화면 언어"
        }
    }

    /// The note under the box.
    pub fn note(&self) -> &'static str {
        if self.first_run {
            "You can change this in Settings → Language.\n설정 → 언어에서 언제든 변경할 수 있습니다."
        } else {
            "Your edits will be saved and the workspace will reopen.\n보정을 저장한 뒤 선택한 언어로 작업 화면을 다시 엽니다."
        }
    }

    /// The text of the OK button.
    pub fn ok_text(&self) -> &'static str {
        if self.first_run {
            "Continue / 시작"
        } else {
            "Apply / 적용"
        }
    }

    /// The text of the Cancel button.
    pub fn cancel_text() -> &'static str {
        "Cancel / 취소"
    }

    /// Whether this is the first-run question.
    pub fn first_run(&self) -> bool {
        self.first_run
    }
}

impl Dialog for LanguageDialog {
    fn id(&self) -> &'static str {
        "language"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("language_box")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(egui::RichText::new(Self::title()).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            widgets::label(ui, self.heading(), t::TEXT, t::FONT_SIZE);
            ui.add_space(8.0);
            let name = LANGUAGES.iter().find(|(_, l)| *l == self.selected).map_or("", |(n, _)| *n);
            egui::ComboBox::from_id_salt("interface_language").width(ui.available_width()).selected_text(name).show_ui(
                ui,
                |ui| {
                    for (text, language) in LANGUAGES {
                        if ui.selectable_label(self.selected == language, text).clicked() {
                            self.selected = language;
                        }
                    }
                },
            );
            ui.add_space(8.0);
            widgets::label(ui, self.note(), t::TEXT, t::FONT_SIZE);
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, Self::cancel_text()).clicked() {
                    answer = Some(false);
                }
                ui.add_space(6.0);
                if Button::new(self.ok_text()).kind(Kind::Primary).show(ui).clicked() {
                    answer = Some(true);
                }
            });
        });
        if response.should_close() && answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                settings::language_chosen(cx, self.first_run, self.selected);
                false
            }
            Some(false) => {
                settings::language_cancelled(cx, self.first_run);
                false
            }
            None => true,
        }
    }
}
