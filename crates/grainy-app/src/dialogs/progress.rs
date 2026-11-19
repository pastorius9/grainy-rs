//! The progress box of the batch workflow (`QProgressDialog` of `WorkflowTools.auto_white` / `border_crop` and of
//! `MainWindow.command_auto_values`): a window-modal box with a label, a bar and 취소, shown while the job of the
//! batch runs. The job reports through a [`Shared`] state and the box closes when the job is finished.

use std::sync::{Arc, Mutex};

use eframe::egui;

use super::Dialog;
use crate::context::AppContext;
use crate::jobs::JobId;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button};

/// What the job tells the box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Photos done.
    pub done: usize,
    /// Photos in all; 0 shows a bar that has no end (`QProgressDialog(…, 0, 0)`).
    pub total: usize,
    /// The job is over (its `done` ran).
    pub finished: bool,
}

/// The state the job and the box share.
#[derive(Debug, Clone, Default)]
pub struct Shared(Arc<Mutex<State>>);

impl Shared {
    /// A new state with `total` photos to do.
    pub fn new(total: usize) -> Self {
        Self(Arc::new(Mutex::new(State { done: 0, total, finished: false })))
    }

    /// `progress.setValue(done)`.
    pub fn set_done(&self, done: usize) {
        if let Ok(mut state) = self.0.lock() {
            state.done = done;
        }
    }

    /// The job is over: the box closes.
    pub fn finish(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.finished = true;
        }
    }

    /// A copy of the state.
    pub fn get(&self) -> State {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// The box.
pub struct ProgressBox {
    job: JobId,
    title: String,
    text: String,
    shared: Shared,
}

impl ProgressBox {
    /// A box for `job`. `title` and `text` are shown as given (already translated).
    pub fn new(job: JobId, title: String, text: String, shared: Shared) -> Self {
        Self { job, title, text, shared }
    }
}

impl Dialog for ProgressBox {
    fn id(&self) -> &'static str {
        "workflow_progress"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let state = self.shared.get();
        if state.finished {
            return false;
        }
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::unique("workflow_progress")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new(&self.title).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            widgets::label(ui, &self.text, t::TEXT, t::FONT_SIZE);
            ui.add_space(8.0);
            let fraction = if state.total == 0 { 0.0 } else { state.done as f32 / state.total as f32 };
            let text = if state.total == 0 { String::new() } else { format!("{}/{}", state.done, state.total) };
            ui.add(egui::ProgressBar::new(fraction).desired_width(ui.available_width()).fill(t::ACCENT).text(text));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::new(&tr("취소")).show(ui).clicked() {
                    cancel = true;
                }
            });
        });
        // Esc cancels a `QProgressDialog` too.
        if cancel || response.should_close() {
            cx.jobs.cancel(self.job);
        }
        // The bar follows the job: repaint while it runs.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        true
    }
}
