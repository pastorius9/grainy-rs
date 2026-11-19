//! A yes/no question (`QMessageBox.question`): runs its action on 예.

use eframe::egui;

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// What 예 does.
pub type Action = Box<dyn FnOnce(&mut AppContext)>;

/// A question with 예 and 아니요.
pub struct Confirm {
    pub id: &'static str,
    /// Title and text, already translated.
    pub title: String,
    pub text: String,
    pub action: Option<Action>,
}

impl Confirm {
    /// A question; `action` runs on 예.
    pub fn new(id: &'static str, title: String, text: String, action: impl FnOnce(&mut AppContext) + 'static) -> Self {
        Self { id, title, text, action: Some(Box::new(action)) }
    }
}

impl Dialog for Confirm {
    fn id(&self) -> &'static str {
        self.id
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique(("confirm", self.id))).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(egui::RichText::new(&self.title).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            widgets::label(ui, &self.text, t::TEXT, t::FONT_SIZE);
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, &tr("아니요")).clicked() {
                    answer = Some(false);
                }
                ui.add_space(6.0);
                if Button::new(&tr("예")).kind(Kind::Primary).show(ui).clicked() {
                    answer = Some(true);
                }
            });
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some(action) = self.action.take() {
                    action(cx);
                }
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}
