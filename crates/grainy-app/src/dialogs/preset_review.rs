//! The boxes of the preset commands: the name box of 현재 보정을 프리셋으로 저장 (`QInputDialog.getText`), the review
//! box of an Adobe preset (`QMessageBox` with 프리셋 추가 / 취소 and the report as detailed text), the choice box of
//! 사용자 프리셋 삭제 (`QInputDialog.getItem`), the error box (`show_error`), and the wait for a native file
//! dialog of the import and export commands.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use eframe::egui;

use super::Dialog;
use crate::commands::presets::{self as presets, Review};
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The buttons of a box: 확인/취소 style. Returns `Some(true)` for the primary button, `Some(false)` for cancel.
fn buttons(ui: &mut egui::Ui, primary: &str, enabled: bool) -> Option<bool> {
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if widgets::button(ui, &tr("취소")).clicked() {
            answer = Some(false);
        }
        ui.add_space(6.0);
        if Button::new(primary).kind(Kind::Primary).enabled(enabled).show(ui).clicked() {
            answer = Some(true);
        }
    });
    answer
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(t::bold(14.0)).color(t::BRAND));
    ui.add_space(6.0);
}

// ---- the name box ---------------------------------------------------------------------------------------------

/// `QInputDialog.getText(self, tr('프리셋 저장'), tr('프리셋 이름'))`.
pub struct NamePrompt {
    pub name: String,
    focused: bool,
}

impl NamePrompt {
    pub fn new() -> Self {
        Self { name: String::new(), focused: false }
    }
}

impl Default for NamePrompt {
    fn default() -> Self {
        Self::new()
    }
}

impl Dialog for NamePrompt {
    fn id(&self) -> &'static str {
        "preset_name"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("preset_name")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(380.0);
            title(ui, &tr("프리셋 저장"));
            widgets::label(ui, &tr("프리셋 이름"), t::TEXT, t::FONT_SIZE);
            ui.add_space(4.0);
            let field = widgets::line_edit(ui, "preset_name_field", &mut self.name, "");
            if !self.focused {
                field.request_focus();
                self.focused = true;
            }
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(10.0);
            answer = buttons(ui, &tr("확인"), true);
            if enter {
                answer = Some(true);
            }
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                presets::save_named(cx, &self.name);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

// ---- the review box ---------------------------------------------------------------------------------------------

/// The review box of an Adobe preset (`import_preset`): what will be added, with the full report on request.
pub struct PresetReview {
    review: Review,
    details: bool,
}

impl PresetReview {
    pub fn new(review: Review) -> Self {
        Self { review, details: false }
    }
}

impl Dialog for PresetReview {
    fn id(&self) -> &'static str {
        "preset_review"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let review = &self.review;
        let response = egui::Modal::new(egui::Id::unique("preset_review")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(520.0);
            title(ui, &tr("Adobe 프리셋 가져오기"));
            let text = tr_with("“{0}” 프리셋을 추가합니다.\n", &[&review.name]) + &review.preview;
            widgets::label(ui, &text, t::TEXT, t::FONT_SIZE);
            if review.preview != review.report {
                ui.add_space(6.0);
                widgets::foldout(ui, &tr("자세히"), &mut self.details);
                if self.details {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        widgets::label(ui, &review.report, t::VALUE, t::FONT_SIZE);
                    });
                }
            }
            ui.add_space(10.0);
            answer = buttons(ui, &tr("프리셋 추가"), true);
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                presets::add_review(cx, self.review.clone());
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

// ---- the delete box ---------------------------------------------------------------------------------------------

/// `QInputDialog.getItem(self, tr('프리셋 삭제'), tr('사용자 프리셋'), names, editable=False)`.
pub struct PresetDelete {
    names: Vec<String>,
    chosen: usize,
}

impl PresetDelete {
    pub fn new(names: Vec<String>) -> Self {
        Self { names, chosen: 0 }
    }
}

impl Dialog for PresetDelete {
    fn id(&self) -> &'static str {
        "preset_delete"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("preset_delete")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(380.0);
            title(ui, &tr("프리셋 삭제"));
            widgets::label(ui, &tr("사용자 프리셋"), t::TEXT, t::FONT_SIZE);
            ui.add_space(4.0);
            egui::ComboBox::from_id_salt("preset_delete_choice")
                .width(ui.available_width())
                .selected_text(self.names.get(self.chosen).cloned().unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (index, name) in self.names.iter().enumerate() {
                        ui.selectable_value(&mut self.chosen, index, name);
                    }
                });
            ui.add_space(10.0);
            answer = buttons(ui, &tr("확인"), true);
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some(name) = self.names.get(self.chosen) {
                    presets::delete_named(cx, name.clone());
                }
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

// ---- the error box ----------------------------------------------------------------------------------------------

/// `show_error(message)`: the status line says it failed; the box shows the last line of the message, with the
/// whole message as details.
pub struct ErrorBox {
    message: String,
    details: bool,
}

/// Opens the error box for `message` (the oracle's `show_error`).
pub fn show_error(cx: &mut AppContext, message: &str) {
    cx.set_status(tr("처리하지 못했습니다. 자세한 내용을 확인해 주세요."));
    cx.open_dialog(Box::new(ErrorBox { message: message.to_owned(), details: false }));
}

impl Dialog for ErrorBox {
    fn id(&self) -> &'static str {
        "preset_error"
    }

    fn show(&mut self, ctx: &egui::Context, _: &mut AppContext) -> bool {
        let mut close = false;
        let last = self.message.trim().lines().last().unwrap_or("").to_owned();
        let response = egui::Modal::new(egui::Id::unique("preset_error")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(460.0);
            title(ui, &tr("처리 오류"));
            widgets::label(ui, &last, t::TEXT, t::FONT_SIZE);
            if self.message.trim() != last {
                ui.add_space(6.0);
                widgets::foldout(ui, &tr("자세히"), &mut self.details);
                if self.details {
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        widgets::label(ui, &self.message, t::VALUE, t::FONT_SIZE);
                    });
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                close = Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked();
            });
        });
        !(close || response.should_close())
    }
}

// ---- waiting for a native file dialog ------------------------------------------------------------------------

/// A native file dialog on its own thread (`QFileDialog.getOpenFileName` / `getSaveFileName`); this invisible
/// dialog waits for the answer, runs `then` with the chosen path, and goes away. Nothing waits when the dialog
/// is cancelled.
pub struct FileWait {
    receiver: Receiver<Option<PathBuf>>,
    then: Option<fn(&mut AppContext, PathBuf)>,
}

/// A file filter: its label and extensions.
pub type Filter = (String, Vec<&'static str>);

impl FileWait {
    /// A dialog for one file to open.
    pub fn open(title: String, filter: Filter, start: Option<PathBuf>, then: fn(&mut AppContext, PathBuf)) -> Self {
        Self::spawn(title, filter, start, None, then)
    }

    /// A dialog for a file name to save, proposing `name`.
    pub fn save(
        title: String,
        name: &str,
        filter: Filter,
        start: Option<PathBuf>,
        then: fn(&mut AppContext, PathBuf),
    ) -> Self {
        Self::spawn(title, filter, start, Some(name.to_owned()), then)
    }

    fn spawn(
        title: String,
        filter: Filter,
        start: Option<PathBuf>,
        save: Option<String>,
        then: fn(&mut AppContext, PathBuf),
    ) -> Self {
        let (sender, receiver) = channel();
        let work = move || {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(&title).add_filter(filter.0, &filter.1);
            if let Some(start) = start.filter(|s| s.is_dir()) {
                dialog = dialog.set_directory(start);
            }
            let chosen = match save {
                Some(name) => pollster::block_on(dialog.set_file_name(name).save_file()),
                None => pollster::block_on(dialog.pick_file()),
            };
            let _ = sender.send(chosen.map(|f| f.path().to_path_buf()));
        };
        if let Err(error) = std::thread::Builder::new().name("grainy-dialog".into()).spawn(work) {
            log::error!("cannot open the file dialog: {error}");
        }
        Self { receiver, then: Some(then) }
    }

    /// A wait that is answered by the caller (tests): the answer is sent through the returned sender.
    pub fn answered(then: fn(&mut AppContext, PathBuf)) -> (Self, std::sync::mpsc::Sender<Option<PathBuf>>) {
        let (sender, receiver) = channel();
        (Self { receiver, then: Some(then) }, sender)
    }
}

impl Dialog for FileWait {
    fn id(&self) -> &'static str {
        "preset_file"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        match self.receiver.try_recv() {
            Ok(Some(path)) => {
                if let Some(then) = self.then.take() {
                    then(cx, path);
                }
                false
            }
            Ok(None) | Err(TryRecvError::Disconnected) => false,
            Err(TryRecvError::Empty) => {
                // The thread cannot wake the window: look again soon.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                true
            }
        }
    }
}
