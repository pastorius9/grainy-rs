//! Native file dialogs (the `rfd` crate) that leave the window drawing while one is open
//! (`QFileDialog.getOpenFileNames` / `getExistingDirectory` of the oracle).
//!
//! **One pattern for every system dialog of the app**: the caller builds the `rfd::AsyncFileDialog` and calls its
//! `pick_*` / `save_file` **on the window's thread**; only the returned future goes to a helper thread
//! ([`await_answer`]), which waits for it and hands the answer on. macOS shows a panel only from the main thread:
//! rfd starts it there as a sheet on the window and answers through the main run loop, so the main thread must
//! never wait for it (a `block_on` there would never return) and a helper thread must never start it. Windows runs
//! the dialog on rfd's own thread from the same call. The blocking `rfd::FileDialog` is not used.

use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::render::Waker;

/// Why a dialog was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Photos to add to the session.
    ImportFiles,
    /// A folder of photos to add (with its subfolders).
    ImportFolder,
    /// The export dialog's folder.
    ExportFolder,
    /// A library folder to open (파일 관리 → 라이브러리 폴더 열기).
    OpenLibrary,
}

/// A dialog's answer: nothing when it was cancelled.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub purpose: Purpose,
    pub paths: Vec<PathBuf>,
}

/// Waits on a helper thread for a dialog the caller started on the window's thread (see the module notes), then
/// runs `then` with the answer there. False when no thread could be started (the dialog's answer is then lost).
pub fn await_answer<T: Send + 'static>(
    dialog: impl Future<Output = T> + Send + 'static,
    then: impl FnOnce(T) + Send + 'static,
) -> bool {
    match std::thread::Builder::new().name("grainy-dialog".into()).spawn(move || then(pollster::block_on(dialog))) {
        Ok(_) => true,
        Err(error) => {
            log::error!("cannot wait for the file dialog: {error}");
            false
        }
    }
}

/// The path of a dialog's chosen file or folder.
pub fn path_of(handle: rfd::FileHandle) -> PathBuf {
    handle.path().to_path_buf()
}

/// Opens the dialog for `purpose` with `title` (call it on the window's thread); the answer goes to `tx` (then
/// `waker` runs).
pub fn open(purpose: Purpose, title: String, start: Option<PathBuf>, tx: Sender<Chosen>, waker: Option<Waker>) {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
    if let Some(start) = start.filter(|s| s.is_dir()) {
        dialog = dialog.set_directory(start);
    }
    let send = move |paths: Vec<PathBuf>| {
        let _ = tx.send(Chosen { purpose, paths });
        if let Some(waker) = waker {
            waker();
        }
    };
    match purpose {
        Purpose::ImportFiles => {
            let extensions: Vec<&str> =
                grainy_service::IMAGE_EXTENSIONS.iter().map(|e| e.trim_start_matches('.')).collect();
            let filter = crate::text::tr("사진");
            let answer = dialog.add_filter(filter, &extensions).pick_files();
            await_answer(answer, move |files| {
                send(files.map(|f| f.into_iter().map(path_of).collect()).unwrap_or_default())
            });
        }
        Purpose::ImportFolder | Purpose::ExportFolder | Purpose::OpenLibrary => {
            let answer = dialog.pick_folder();
            await_answer(answer, move |folder| send(folder.map(path_of).into_iter().collect()));
        }
    }
}
