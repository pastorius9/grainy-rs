//! Native file dialogs (the `rfd` crate) on their own threads, so the window keeps drawing while one is open
//! (`QFileDialog.getOpenFileNames` / `getExistingDirectory` of the oracle).

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

/// Opens the dialog for `purpose` with `title`; the answer goes to `tx` (then `waker` runs).
pub fn open(purpose: Purpose, title: String, start: Option<PathBuf>, tx: Sender<Chosen>, waker: Option<Waker>) {
    let work = move || {
        let mut dialog = rfd::AsyncFileDialog::new().set_title(&title);
        if let Some(start) = start.filter(|s| s.is_dir()) {
            dialog = dialog.set_directory(start);
        }
        let paths: Vec<PathBuf> = match purpose {
            Purpose::ImportFiles => {
                let extensions: Vec<&str> =
                    grainy_service::IMAGE_EXTENSIONS.iter().map(|e| e.trim_start_matches('.')).collect();
                let filter = crate::text::tr("사진");
                pollster::block_on(dialog.add_filter(filter, &extensions).pick_files())
                    .map(|files| files.into_iter().map(|f| f.path().to_path_buf()).collect())
                    .unwrap_or_default()
            }
            Purpose::ImportFolder | Purpose::ExportFolder | Purpose::OpenLibrary => {
                pollster::block_on(dialog.pick_folder())
                    .map(|folder| vec![folder.path().to_path_buf()])
                    .unwrap_or_default()
            }
        };
        let _ = tx.send(Chosen { purpose, paths });
        if let Some(waker) = waker {
            waker();
        }
    };
    if let Err(error) = std::thread::Builder::new().name("grainy-dialog".into()).spawn(work) {
        log::error!("cannot open the file dialog: {error}");
    }
}
