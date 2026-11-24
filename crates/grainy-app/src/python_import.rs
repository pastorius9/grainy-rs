//! 파이썬 Grainy 라이브러리 가져오기: moves a user of the Python Grainy over to this app with their library.
//!
//! The Python Grainy's catalogue has the same format as this app's (`grainy_catalog` is its port), so its photos,
//! edits, history, collections, snapshots and virtual copies come over by replacing the open library's catalogue
//! with a copy of it, the same way as 카탈로그 복원 (the current catalogue is backed up first in its `backups`
//! folder, so the restore command takes it back). Its thumbnails (same `preview_store` version, each checked
//! against its stamp before use) and its camera profiles come over too, so the library does not have to be
//! rebuilt photo by photo.
//!
//! **The Python Grainy's folders are only read.** Its catalogue is copied first into a private snapshot in this
//! app's data folder ([`snapshot_catalog`], the read-only open's copy), and everything this module writes goes
//! into this app's own library and data folder, only after the user said yes.
//!
//! The command is in the 파일 관리 menu; a library that is still empty also gets the question once after the
//! window opens ([`offer`], a step of [`crate::startup`]), when the program's start named the Python Grainy's
//! library ([`PythonLibrary::offer`]; tests and the smoke run leave it out).

use std::path::{Path, PathBuf};

use grainy_platform::json::Value as Pref;

use crate::commands::files::{fail, stop_batch};
use crate::commands::settings::preferences;
use crate::commands::{writable_library, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::about::MessageBox;
use crate::dialogs::confirm::Confirm;
use crate::document::catalog::files::FileOp;
use crate::document::catalog::snapshot_catalog;
use crate::text::{tr, tr_with};

/// The menu command (파일 관리).
pub const COMMAND: Command =
    Command { id: "file.import_python_library", label: LABEL, enabled: can_import, run: import_command };

/// The command's label (the menu).
pub const LABEL: &str = "파이썬 Grainy 라이브러리 가져오기…";
/// The title of its boxes and job.
pub const TITLE: &str = "파이썬 Grainy 라이브러리 가져오기";
/// The preference that remembers that the start-up question was answered (or the library imported).
pub const OFFERED: &str = "python_library_offered";
/// The kind of the preparing job.
const JOB: &str = "python_import";
/// The kind of the job that places the thumbnails and camera profiles after the catalogue was replaced.
pub(crate) const FILES_JOB: &str = "python_import_files";

/// The Python Grainy's library to offer at start (`cx.state::<PythonLibrary>()`).
#[derive(Debug, Default, Clone)]
pub struct PythonLibrary {
    /// Set by the program's start to the Python Grainy's library folder; `None` asks nothing.
    pub offer: Option<PathBuf>,
}

/// What the preparing job copied into the private snapshot folder, waiting for the user's answer. Dropping it
/// removes the snapshot.
pub struct Prepared {
    snapshot: tempfile::TempDir,
    library: PathBuf,
    generation: Option<u64>,
    /// The self-contained copy of the Python Grainy's catalogue.
    catalog: PathBuf,
    /// Photos in it (virtual copies included).
    pub photos: i64,
    /// Copies of its thumbnails (`<id>.jpg`, `<id>.json`).
    thumbnails: PathBuf,
    previews: PathBuf,
    /// Copies of its camera profiles and where they go: (copy, folder in this app).
    profiles: Vec<(PathBuf, PathBuf)>,
}

impl Prepared {
    /// The private folder that holds the copies (tests).
    pub fn folder(&self) -> &Path {
        self.snapshot.path()
    }
}

fn can_import(cx: &AppContext) -> bool {
    writable_library(cx) && !cx.jobs.running(JOB) && !cx.jobs.running(FILES_JOB)
}

pub(crate) fn shutdown(cx: &mut AppContext) {
    cx.jobs.cancel_kind(JOB);
    for kind in [JOB, FILES_JOB] {
        cx.jobs.join_kind(kind);
        drop(cx.jobs.poll_kind(Some(kind)));
    }
}

fn import_command(cx: &mut AppContext, _: &Invocation) {
    let folder = crate::paths::Locations::current().oracle_library();
    start(cx, &folder, true);
}

fn busy(cx: &AppContext) -> bool {
    !cx.jobs.jobs_idle() || !cx.renderer.idle() || crate::ai::running_job(cx).is_some()
}

/// Photos in the open (writable) library, or `None` without one.
fn current_photos(cx: &AppContext) -> Option<i64> {
    cx.doc.library_folder()?;
    let catalog = cx.doc.metadata_catalog()?;
    catalog.connection().query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0)).ok()
}

/// Starts the import of the Python Grainy's library in `source`: the copies are made on a job, then the user is
/// asked (`ask`, or always when the open library is not empty) and [`apply`] replaces the library.
pub fn start(cx: &mut AppContext, source: &Path, ask: bool) {
    if !writable_library(cx) {
        return fail(cx, "Grainy RS 라이브러리를 연 상태에서 가져오세요.");
    }
    if busy(cx) {
        return fail(cx, "가져오기·내보내기 완료 후 다시 시도하세요.");
    }
    if !source.join("catalog.sqlite").is_file() {
        cx.open_dialog(Box::new(MessageBox::information(
            "python_import_missing",
            tr(TITLE),
            tr("파이썬 Grainy 라이브러리를 찾지 못했습니다."),
        )));
        return;
    }
    let Some(data) = cx.data_folder.clone() else { return };
    let Some(library) = cx.doc.library_folder() else { return };
    let source = source.to_path_buf();
    cx.status = tr("파이썬 Grainy 라이브러리를 읽는 중…");
    let generation = cx.document_generation;
    cx.spawn_document_job(
        JOB,
        tr(TITLE),
        move |handle| prepare(&source, &data, &library, &|| handle.cancelled()),
        move |cx: &mut AppContext, result: Result<Prepared, String>| {
            cx.status.clear();
            match result {
                Err(message) => fail(cx, &message),
                Ok(prepared) if prepared.photos == 0 => cx.open_dialog(Box::new(MessageBox::information(
                    "python_import_empty",
                    tr(TITLE),
                    tr("파이썬 Grainy 라이브러리에 가져올 사진이 없습니다."),
                ))),
                Ok(mut prepared) => {
                    prepared.generation = Some(generation);
                    let current = current_photos(cx).unwrap_or(0);
                    if ask || current > 0 {
                        confirm(cx, prepared, current);
                    } else {
                        apply(cx, prepared);
                    }
                }
            }
        },
    );
}

/// The question before the library is replaced.
fn confirm(cx: &mut AppContext, prepared: Prepared, current: i64) {
    let text = if current > 0 {
        tr_with(
            "지금 라이브러리(사진 {1}장)를 백업해 두고 파이썬 Grainy 라이브러리(사진 {0}장)로 바꿀까요?\n\n백업은 파일 관리의 '카탈로그 복원'으로 되돌릴 수 있습니다. 파이썬 Grainy의 라이브러리와 원본 사진은 바뀌지 않습니다.",
            &[&prepared.photos.to_string(), &current.to_string()],
        )
    } else {
        tr_with(
            "파이썬 Grainy 라이브러리의 사진 {0}장을 보정·기록·컬렉션과 함께 가져올까요?\n\n파이썬 Grainy의 라이브러리와 원본 사진은 바뀌지 않습니다.",
            &[&prepared.photos.to_string()],
        )
    };
    cx.open_dialog(Box::new(Confirm::new("python_import", tr(TITLE), text, move |cx: &mut AppContext| {
        apply(cx, prepared)
    })));
}

/// Copies what the import needs into a new private folder under `<data>/python import`: the catalogue (a verified
/// snapshot), its thumbnails and its camera profiles. Nothing outside that folder is written. `cancelled` is asked
/// between files.
pub fn prepare(source: &Path, data: &Path, library: &Path, cancelled: &dyn Fn() -> bool) -> Result<Prepared, String> {
    let (snapshot, catalog) = snapshot_catalog(source, &data.join("python import")).map_err(|message| {
        if message.contains("changed while") || message.contains("rollback journal") {
            "파이썬 Grainy가 라이브러리를 쓰는 중입니다. 파이썬 Grainy를 닫고 다시 시도하세요.".to_owned()
        } else {
            message
        }
    })?;
    let photos: i64 = grainy_catalog::rusqlite::Connection::open_with_flags(
        &catalog,
        grainy_catalog::rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|db| db.query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0)))
    .map_err(|_| "파이썬 Grainy 라이브러리를 읽지 못했습니다.".to_owned())?;
    let stage = snapshot.path().join("stage");
    let previews = stage.join("previews");
    let source_previews = source.join("previews");
    if source_previews.try_exists().map_err(|e| e.to_string())? {
        let entries = std::fs::read_dir(source_previews).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&previews).map_err(|e| e.to_string())?;
        for entry in entries {
            if cancelled() {
                return Err("cancelled".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let p = entry.path();
            if entry.file_type().map_err(|e| e.to_string())?.is_file()
                && p.extension().is_some_and(|x| x == "npz")
                && p.file_stem().and_then(|x| x.to_str()).and_then(|x| x.parse::<i64>().ok()).is_some()
            {
                std::fs::copy(&p, previews.join(entry.file_name())).map_err(|e| e.to_string())?;
            }
        }
    }
    let thumbnails = stage.join("thumbnails");
    std::fs::create_dir_all(&thumbnails).map_err(|e| e.to_string())?;
    copy_tree(&source.join("thumbnails"), &thumbnails, false, true, cancelled)?;
    // Camera profiles: the library's own folder and the Python Grainy's `CameraProfiles` beside its library.
    let mut profiles = Vec::new();
    let own = source.parent().map(|luma| luma.join("CameraProfiles"));
    for (n, (from, to)) in
        [(Some(source.join("camera-profiles")), library.join("camera-profiles")), (own, data.join("CameraProfiles"))]
            .into_iter()
            .enumerate()
    {
        let Some(from) = from else { continue };
        let copy = stage.join(format!("profiles-{n}"));
        copy_tree(&from, &copy, true, false, cancelled)?;
        if copy.try_exists().map_err(|e| e.to_string())? {
            profiles.push((copy, to));
        }
    }
    Ok(Prepared {
        snapshot,
        catalog,
        photos,
        thumbnails,
        previews,
        profiles,
        library: library.to_owned(),
        generation: None,
    })
}

/// Copies the regular files of `from` into `to` (created), keeping files already there; `deep` follows
/// subfolders, `thumbnails_only` takes only `.jpg` and `.json`. Links are skipped. A missing `from` copies nothing.
fn copy_tree(
    from: &Path,
    to: &Path,
    deep: bool,
    thumbnails_only: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<usize, String> {
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.to_string()),
    };
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let mut copied = 0;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if cancelled() {
            return Err("cancelled".into());
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let target = to.join(&name);
        if kind.is_dir() && deep {
            copied += copy_tree(&entry.path(), &target, deep, thumbnails_only, cancelled)?;
        } else if kind.is_file() {
            let wanted = !thumbnails_only
                || Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("json"));
            // Preserve a locally installed profile. A truncated prefix left by an older
            // import is repairable without replacing an unrelated profile with the same name.
            let repair = !thumbnails_only
                && target.is_file()
                && truncated_copy(&entry.path(), &target).map_err(|e| e.to_string())?;
            if wanted && (!target.exists() || repair) {
                publish_copy(&entry.path(), &target).map_err(|e| e.to_string())?;
                copied += 1;
            }
        }
    }
    Ok(copied)
}

fn truncated_copy(source: &Path, target: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let len = std::fs::metadata(target)?.len();
    if len >= std::fs::metadata(source)?.len() {
        return Ok(false);
    }
    let mut src = std::fs::File::open(source)?;
    let mut dst = std::fs::File::open(target)?;
    let (mut a, mut b) = ([0u8; 8192], [0u8; 8192]);
    loop {
        let n = dst.read(&mut a)?;
        if n == 0 {
            return Ok(true);
        }
        src.read_exact(&mut b[..n])?;
        if a[..n] != b[..n] {
            return Ok(false);
        }
    }
}

/// Publish through a new inode on the destination volume, even when the old name
/// is a hard link to an original photo.
fn publish_copy(source: &Path, target: &Path) -> std::io::Result<()> {
    let mut staged = tempfile::NamedTempFile::new_in(target.parent().unwrap_or(Path::new(".")))?;
    std::io::copy(&mut std::fs::File::open(source)?, staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    staged.persist(target).map_err(|e| e.error)?;
    Ok(())
}

fn place_thumbnails(staged: &Path, folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(staged).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = folder.join(entry.file_name());
        if std::fs::rename(entry.path(), &target).is_err() {
            publish_copy(&entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn restore_caches(library: &Path, backup: &Path) -> std::io::Result<()> {
    for name in ["previews", "thumbnails"] {
        let saved = backup.join(name);
        if saved.try_exists()? {
            std::fs::rename(saved, library.join(name))?;
        }
    }
    Ok(())
}

fn detach_caches(library: &Path) -> Result<PathBuf, String> {
    let backups = library.join("backups");
    std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
    // Persist before moving the first directory. Even if rollback itself fails, a
    // temporary-directory destructor must never remove the user's previous files.
    let backup =
        tempfile::Builder::new().prefix("before-python-import-").tempdir_in(backups).map_err(|e| e.to_string())?.keep();
    for name in ["previews", "thumbnails"] {
        let old = library.join(name);
        match std::fs::rename(&old, backup.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                if let Err(rollback) = restore_caches(library, &backup) {
                    return Err(format!("{error} ({rollback})"));
                }
                return Err(error.to_string());
            }
        }
    }
    Ok(backup)
}

/// Replaces the open library's catalogue with the prepared copy (after backing it up, as 카탈로그 복원 does), then
/// places the thumbnails and the camera profiles that are not there yet, and says what came over.
pub fn apply(cx: &mut AppContext, prepared: Prepared) {
    if busy(cx) {
        return fail(cx, "가져오기·내보내기 완료 후 다시 시도하세요.");
    }
    let Some(library) = cx.doc.library_folder() else {
        return fail(cx, "Grainy RS 라이브러리를 연 상태에서 가져오세요.");
    };
    if prepared.library != library || prepared.generation.is_some_and(|g| g != cx.document_generation) {
        return;
    }
    stop_batch(cx);
    // Detach the previous preview namespace before any replacement rows become visible.
    // Keep its contents alongside the safety backup, including files of unknown ownership.
    let previous = library.join("previews");
    let backup = match detach_caches(&library) {
        Ok(backup) => backup,
        Err(error) => return fail(cx, &error),
    };
    if let Err(message) = cx.doc.file_op(FileOp::Restore { path: prepared.catalog.clone() }) {
        if let Err(error) = restore_caches(&library, &backup) {
            return fail(cx, &format!("{message} ({error})"));
        }
        cx.photo_changed();
        return fail(cx, &message);
    }
    remember(cx);
    cx.photo_changed();
    cx.state::<crate::session_state::ColumnState>().rescan();
    // Thousands of thumbnails take seconds to move on Windows: off the window's thread. Until one is in place its
    // photo shows the thumbnail made meanwhile; both are checked against their stamps before use.
    let Prepared { snapshot, thumbnails, previews, profiles, photos, .. } = prepared;
    let target = library.join("thumbnails");
    cx.spawn_document_job(
        FILES_JOB,
        "",
        move |_| {
            place_thumbnails(&thumbnails, &target)?;
            if previews.is_dir() {
                place_thumbnails(&previews, &previous)?;
            }
            for (copy, folder) in &profiles {
                copy_tree(copy, folder, true, false, &|| false)?;
            }
            drop(snapshot);
            Ok(())
        },
        move |cx: &mut AppContext, result: Result<(), String>| match result {
            Ok(()) => cx.open_dialog(Box::new(done_box(photos, python_mcp_used()))),
            Err(message) => fail(cx, &message),
        },
    );
}

/// Whether the Python Grainy's AI connection was set up on this computer: its bridge is in its settings folder.
/// AI programs set up for it still start that bridge, which reaches the Python Grainy, not this app.
fn python_mcp_used() -> bool {
    let folder = crate::paths::Locations::current().oracle_settings().join("mcp");
    ["grainy-mcp-bridge.ps1", "grainy-mcp-bridge.sh"].iter().any(|name| folder.join(name).is_file())
}

/// What the import says at the end; with the Python Grainy's AI connection in use, how to move it too.
pub fn done_box(photos: i64, mcp: bool) -> MessageBox {
    let mut message = MessageBox::information(
        "python_import_done",
        tr(TITLE),
        tr_with("파이썬 Grainy 라이브러리의 사진 {0}장을 가져왔습니다.", &[&photos.to_string()]),
    );
    message.informative = tr("이제부터 파이썬 Grainy에서 고친 내용은 옮겨지지 않습니다. 파이썬 Grainy의 라이브러리와 원본 사진은 그대로입니다.");
    if mcp {
        message.informative += "\n\n";
        message.informative += &tr("AI 프로그램(Claude, Codex 등)을 파이썬 Grainy에 연결해 두셨다면, AI 연결 창의 설치 명령을 한 번 다시 실행해야 Grainy RS에 연결됩니다.");
        message.buttons = vec![
            (
                tr("AI 연결 열기"),
                Some(Box::new(|cx: &mut AppContext| {
                    cx.open_dialog(Box::new(crate::dialogs::mcp::McpDialog::default()))
                })),
            ),
            (tr("닫기"), None),
        ];
    }
    message
}

fn remember(cx: &mut AppContext) {
    if let Some(mut prefs) = preferences(cx) {
        if let Err(error) = prefs.save(OFFERED, Pref::Bool(true)) {
            log::warn!("{OFFERED} not saved: {error}");
        }
    }
}

/// The question once after the window opens: the open library is empty, the Python Grainy's library exists and
/// the question was not answered before.
pub fn offer(cx: &mut AppContext) {
    let Some(source) = cx.state::<PythonLibrary>().offer.clone() else { return };
    if !source.join("catalog.sqlite").is_file() || current_photos(cx) != Some(0) {
        return;
    }
    let Some(prefs) = preferences(cx) else { return };
    if prefs.values.get(OFFERED).is_some_and(|v| !matches!(v, Pref::Null)) {
        return;
    }
    let mut message = MessageBox::information(
        "python_import_offer",
        tr(TITLE),
        tr("파이썬 Grainy에서 쓰던 라이브러리를 가져올까요?"),
    );
    message.informative = tr("사진·보정·기록·컬렉션을 그대로 옮겨 옵니다. 파이썬 Grainy의 라이브러리와 원본 사진은 바뀌지 않습니다. 나중에 파일 관리 메뉴에서도 가져올 수 있습니다.");
    message.buttons = vec![
        (
            tr("가져오기"),
            Some(Box::new(move |cx: &mut AppContext| {
                remember(cx);
                start(cx, &source, false);
            })),
        ),
        (tr("나중에"), Some(Box::new(remember))),
    ];
    cx.open_dialog(Box::new(message));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_volume_copy_replaces_hard_link_without_changing_original() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.jpg");
        let target = dir.path().join("1.jpg");
        let source = dir.path().join("import.jpg");
        std::fs::write(&original, b"original").unwrap();
        std::fs::hard_link(&original, &target).unwrap();
        std::fs::write(&source, b"thumbnail").unwrap();
        publish_copy(&source, &target).unwrap();
        assert_eq!(std::fs::read(original).unwrap(), b"original");
        assert_eq!(std::fs::read(target).unwrap(), b"thumbnail");
    }
}
