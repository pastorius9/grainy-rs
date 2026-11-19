//! The cancelable catalogue-maintenance window (luma/maintenance_dialog.py `MaintenanceDialog`) and the waits for
//! the native file dialogs of file management (`QFileDialog.getOpenFileName`, `getSaveFileName`,
//! `getExistingDirectory` in luma/manager.py and luma/folder_panel.py).
//!
//! The maintenance window shows the newest progress the job reported (`refresh` every 100 ms), a busy bar while no
//! total is known, and 취소, which asks the job to stop at its next safe point (`request_cancel`). It cannot be
//! closed while the job runs; the job's end closes it (`finish`).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use grainy_library::Cancel;

use super::{native, Dialog};
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button};

/// One progress report (`dict(stage, examined, total)` or `dict(stage, done, total)`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    /// The stage text, worded for the person.
    pub stage: String,
    pub done: u64,
    pub total: u64,
}

/// What the job and the window share: the newest update (`updates` queue, only the last one is shown), the cancel
/// flag and whether the job ended.
#[derive(Debug, Clone, Default)]
pub struct Shared {
    pub cancel: Cancel,
    pub latest: Arc<Mutex<Option<Update>>>,
    pub finished: Arc<std::sync::atomic::AtomicBool>,
}

impl Shared {
    /// The job reports (`dialog.updates.put`).
    pub fn put(&self, update: Update) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(update);
        }
    }

    /// The job ended (`dialog.finish()`).
    pub fn finish(&self) {
        self.finished.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Port of `MaintenanceDialog`.
pub struct MaintenanceDialog {
    /// The window title (already translated).
    pub title: String,
    /// The label (`안전 백업을 준비하고 있습니다…`, then the stages).
    pub label: String,
    /// `(done, total)` of the bar, `None` for the busy bar.
    pub bar: Option<(u64, u64)>,
    /// The note under the bar.
    pub note: String,
    /// 취소 was pressed.
    pub cancelling: bool,
    pub shared: Shared,
}

impl MaintenanceDialog {
    /// `MaintenanceDialog(parent, title='카탈로그 최적화', note=None)`.
    pub fn new(shared: Shared) -> Self {
        Self {
            title: tr("카탈로그 최적화"),
            label: tr("안전 백업을 준비하고 있습니다…"),
            bar: None,
            note: crate::commands::files::tr_files(
                "프로파일 중복과 빈 공간을 정리합니다.\n원본 사진은 그대로 유지합니다. 취소 후에도 다시 시작할 수 있습니다.",
            ),
            cancelling: false,
            shared,
        }
    }

    /// Port of `refresh`: the newest update replaces the label and the bar (not after 취소).
    pub fn refresh(&mut self) {
        let latest = self.shared.latest.lock().ok().and_then(|mut l| l.take());
        if let Some(update) = latest {
            if !self.shared.cancel.is_set() {
                self.label = tr(&update.stage);
                self.bar = (update.total > 0).then_some((update.done, update.total.max(1)));
            }
        }
    }

    /// Port of `request_cancel`.
    pub fn request_cancel(&mut self) {
        self.shared.cancel.set();
        self.label = tr("현재 작업을 안전하게 마무리하는 중…");
        self.cancelling = true;
    }
}

impl Dialog for MaintenanceDialog {
    fn id(&self) -> &'static str {
        "maintenance"
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        if self.shared.finished.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        self.refresh();
        let response = egui::Modal::new(egui::Id::unique("maintenance")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(egui::RichText::new(&self.title).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            widgets::label(ui, &self.label, t::TEXT, t::FONT_SIZE);
            ui.add_space(6.0);
            let bar = match self.bar {
                Some((done, total)) => egui::ProgressBar::new(done as f32 / total as f32),
                // A busy bar that does not move: the picture stays the same from frame to frame.
                None => egui::ProgressBar::new(0.0),
            };
            ui.add(bar.desired_height(8.0).desired_width(ui.available_width()));
            ui.add_space(6.0);
            widgets::label(ui, &self.note, t::MUTED, t::FONT_SIZE);
            ui.add_space(10.0);
            if Button::new(&tr("취소")).enabled(!self.cancelling).show(ui).clicked() {
                self.request_cancel();
            }
        });
        // `reject` and `closeEvent` while running: the same as 취소.
        if response.should_close() && !self.cancelling {
            self.request_cancel();
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        true
    }
}

// ---- waiting for a native file dialog ------------------------------------------------------------------------

/// What to ask the system's file dialog for.
#[derive(Debug, Clone)]
pub enum Pick {
    /// One file to open, with a filter (label, extensions); no filter: every file.
    Open(Option<(String, Vec<&'static str>)>),
    /// A file name to save, proposing a name, with a filter.
    Save(String, (String, Vec<&'static str>)),
    /// A folder.
    Folder,
}

type Then = Box<dyn FnOnce(&mut AppContext, PathBuf)>;

/// A native file dialog on its own thread; this invisible dialog waits for the answer, runs `then` with the chosen
/// path and goes away. Nothing runs when the dialog is cancelled.
pub struct PathWait {
    receiver: Receiver<Option<PathBuf>>,
    then: Option<Then>,
}

impl PathWait {
    /// Opens the system dialog `pick` with `title`, starting in `start`.
    pub fn open(
        title: String,
        pick: Pick,
        start: Option<PathBuf>,
        then: impl FnOnce(&mut AppContext, PathBuf) + 'static,
    ) -> Self {
        let (sender, receiver) = channel();
        // Started here, on the window's thread; only the answer is awaited elsewhere (`native`).
        let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
        if let Some(start) = start.filter(|s| s.is_dir()) {
            dialog = dialog.set_directory(start);
        }
        let send = move |chosen: Option<rfd::FileHandle>| {
            let _ = sender.send(chosen.map(native::path_of));
        };
        match pick {
            Pick::Open(filter) => {
                if let Some((label, extensions)) = filter {
                    dialog = dialog.add_filter(label, &extensions);
                }
                native::await_answer(dialog.pick_file(), send);
            }
            Pick::Save(name, (label, extensions)) => {
                native::await_answer(dialog.add_filter(label, &extensions).set_file_name(name).save_file(), send);
            }
            Pick::Folder => {
                native::await_answer(dialog.pick_folder(), send);
            }
        }
        Self { receiver, then: Some(Box::new(then)) }
    }

    /// A wait answered by the caller (tests): the answer is sent through the returned sender.
    pub fn answered(
        then: impl FnOnce(&mut AppContext, PathBuf) + 'static,
    ) -> (Self, std::sync::mpsc::Sender<Option<PathBuf>>) {
        let (sender, receiver) = channel();
        (Self { receiver, then: Some(Box::new(then)) }, sender)
    }
}

impl Dialog for PathWait {
    fn id(&self) -> &'static str {
        "files_path"
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
                ctx.request_repaint_after(Duration::from_millis(100));
                true
            }
        }
    }
}
