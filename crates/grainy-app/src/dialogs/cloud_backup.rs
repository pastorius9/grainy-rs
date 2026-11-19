//! 구글 드라이브 백업… (work package 31): the automatic backup of the catalogue to the "My Drive" folder of Google
//! Drive for desktop, or to any folder (luma/cloud_backup.py: `google_drive`, `snapshot`, `digest`, `upload`,
//! `surplus`, `trim`, `CloudBackup`, `dialog`).
//!
//! Only consistent SQLite snapshots of the catalogue are copied (edits, history, ratings, keywords, collections,
//! presets); original photos and thumbnails stay local. An unchanged catalogue is not copied again, and old
//! backups are thinned to the recycle bin, never deleted for good.
//!
//! Copying into the chosen folder is an ordinary file copy (Google Drive for desktop does the uploading), so no
//! network is involved; it still goes through an [`Uploader`] so that tests record what would have been copied
//! and where the recycle bin is replaced by a fake. A schedule (every [`INTERVAL`], on the window's frames) starts
//! automatic backups; [`shutdown`] makes the final one when the window closes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use eframe::egui;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::maintenance::{PathWait, Pick};
use super::Dialog;
use crate::commands::settings::launch;
use crate::context::AppContext;
use crate::document::catalog::files::FileOp;
use crate::startup::tr;
use crate::text::tr_with;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The library preference holding the settings (`PREF`).
pub const PREF: &str = "cloud_backup";
/// The start of every backup's name (`PREFIX`).
pub const PREFIX: &str = "Grainy-catalog-";
/// The folder made inside "My Drive" (`FOLDER_NAME`).
pub const FOLDER_NAME: &str = "Grainy 백업";
/// Newest backups kept; older ones keep one per month (`KEEP`).
pub const KEEP: usize = 20;
/// Between automatic backups (`INTERVAL_MS`).
pub const INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Seconds the close waits for a backup (`CLOSE_TIMEOUT`).
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(20);

// ---- the functions of the module ---------------------------------------------------------------------------------

/// Port of `google_drive`: "My Drive" of Google Drive for desktop, or none. Windows has no volume-label call
/// without `unsafe`, so every drive that has a `My Drive` (or `내 드라이브`) folder counts.
pub fn google_drive() -> Option<PathBuf> {
    if cfg!(windows) {
        for letter in 'A'..='Z' {
            let root = PathBuf::from(format!("{letter}:\\"));
            for name in ["My Drive", "내 드라이브"] {
                if root.join(name).is_dir() {
                    return Some(root.join(name));
                }
            }
        }
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)?;
    // macOS: Google Drive for desktop mounts each account under ~/Library/CloudStorage/GoogleDrive-<account>.
    let storage = home.join("Library").join("CloudStorage");
    let mut accounts: Vec<PathBuf> = std::fs::read_dir(&storage)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("GoogleDrive-")))
        .collect();
    accounts.sort();
    for account in accounts {
        for name in ["My Drive", "내 드라이브"] {
            if account.join(name).is_dir() {
                return Some(account.join(name));
            }
        }
    }
    [home.join("My Drive"), home.join("Google Drive").join("My Drive"), home.join("Google Drive")]
        .into_iter()
        .find(|p| p.is_dir())
}

/// Port of `default_folder`: the backup folder inside `drive`, or empty.
pub fn default_folder(drive: Option<&Path>) -> String {
    drive.map(|d| d.join(FOLDER_NAME).to_string_lossy().into_owned()).unwrap_or_default()
}

/// Port of `digest`: the SHA-256 of a file, lower-case hex.
pub fn digest(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut hash = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut block = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        hash.update(&block[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Port of `surplus`: backup names to retire, all but the newest `keep` except the newest of each older month.
pub fn surplus(names: &[String], keep: usize) -> Vec<String> {
    let mut names = names.to_vec();
    names.sort_by(|a, b| b.cmp(a));
    let mut months = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for name in names.iter().skip(keep) {
        let month: String = name.chars().skip(PREFIX.chars().count()).take(6).collect();
        if months.contains(&month) {
            out.push(name.clone());
        } else {
            months.insert(month);
        }
    }
    out
}

/// Port of `trim`: retires the surplus backups of `folder` through `recycle` (the recycle bin; a backup that
/// cannot be recycled stays, never deleted for good).
pub fn trim(folder: &Path, keep: usize, recycle: &dyn Fn(&Path) -> bool) {
    let names: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(PREFIX) && n.ends_with(".sqlite"))
        .collect();
    for name in surplus(&names, keep) {
        let _ = recycle(&folder.join(name));
    }
}

fn quick_check(path: &Path) -> Result<(), String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let answer: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0)).map_err(|e| e.to_string())?;
    if answer == "ok" {
        Ok(())
    } else {
        Err(tr("백업 사본 검사에 실패했습니다."))
    }
}

/// Port of `upload`: verifies the snapshot and copies it into `folder` unless it matches `last_hash`. Returns the
/// new file (none when unchanged) and the hash. The staged file is always removed; a partial copy never gets the
/// `.sqlite` name.
pub fn upload(
    staged: &Path,
    folder: &Path,
    last_hash: Option<&str>,
    keep: usize,
    now: NaiveDateTime,
    recycle: &dyn Fn(&Path) -> bool,
) -> Result<(Option<PathBuf>, String), String> {
    let result = (|| -> Result<(Option<PathBuf>, String), String> {
        quick_check(staged)?;
        let value = digest(staged).map_err(|e| e.to_string())?;
        if last_hash == Some(value.as_str()) {
            return Ok((None, value));
        }
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let stamp = now.format("%Y%m%d-%H%M%S").to_string();
        let mut target = folder.join(format!("{PREFIX}{stamp}.sqlite"));
        let mut n = 1;
        while target.exists() {
            target = folder.join(format!("{PREFIX}{stamp}-{n}.sqlite"));
            n += 1;
        }
        let part = PathBuf::from(format!("{}.part", target.display()));
        std::fs::copy(staged, &part).map_err(|e| e.to_string())?;
        if digest(&part).map_err(|e| e.to_string())? != value {
            let _ = std::fs::remove_file(&part);
            return Err(tr("업로드한 사본이 원본과 다릅니다."));
        }
        std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
        trim(folder, keep, recycle);
        Ok((Some(target), value))
    })();
    let _ = std::fs::remove_file(staged);
    result
}

/// Copies a verified snapshot into the backup folder.
pub trait Uploader: Send + Sync {
    /// [`upload`] with this uploader's recycle bin and clock.
    fn upload(
        &self,
        staged: &Path,
        folder: &Path,
        last_hash: Option<&str>,
    ) -> Result<(Option<PathBuf>, String), String>;
}

/// The uploader of this build: a copy into the folder, old backups to the recycle bin.
pub struct FolderUploader;

impl Uploader for FolderUploader {
    fn upload(
        &self,
        staged: &Path,
        folder: &Path,
        last_hash: Option<&str>,
    ) -> Result<(Option<PathBuf>, String), String> {
        upload(staged, folder, last_hash, KEEP, chrono::Local::now().naive_local(), &grainy_platform::updater::recycle)
    }
}

// ---- the settings and the state ----------------------------------------------------------------------------------

/// The settings of the backup (the preference's keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Automatic backups on.
    pub enabled: bool,
    /// The folder the backups go to.
    pub folder: String,
    /// When the last backup was made (`YYYY-MM-DDTHH:MM:SS`), or empty.
    pub last: String,
    /// The hash of the last backup.
    pub hash: String,
    /// The last error, or empty.
    pub error: String,
}

/// A function called when a backup finished, with the message.
pub type Notify = Box<dyn FnOnce(&mut AppContext, String)>;

/// The backup's state (`cx.state::<CloudState>()`); tests replace the parts.
pub struct CloudState {
    /// The copier; [`FolderUploader`] when none.
    pub uploader: Option<Arc<dyn Uploader>>,
    /// "My Drive" once looked for (`Some(None)`: not found); `None` while not looked for yet.
    pub drive: Option<Option<PathBuf>>,
    /// Between automatic backups.
    pub interval: Duration,
    /// How long the close waits.
    pub close_timeout: Duration,
    /// The time written as `last` (tests); the local time when none.
    pub now: Option<String>,
    /// A backup is running (`self.busy`).
    pub busy: bool,
    /// The window is closing (`w.closing`).
    pub closing: bool,
    next: Option<Instant>,
    scanning: bool,
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            uploader: None,
            drive: None,
            interval: INTERVAL,
            close_timeout: CLOSE_TIMEOUT,
            now: None,
            busy: false,
            closing: false,
            next: None,
            scanning: false,
        }
    }
}

fn uploader(cx: &mut AppContext) -> Arc<dyn Uploader> {
    cx.state::<CloudState>().uploader.clone().unwrap_or_else(|| Arc::new(FolderUploader))
}

fn stamp(cx: &mut AppContext) -> String {
    cx.state::<CloudState>().now.clone().unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string())
}

/// Port of `CloudBackup.settings`: the stored values over the defaults.
pub fn settings(cx: &AppContext) -> Settings {
    let saved = cx.doc.preference(PREF).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let text = |key: &str| saved.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    Settings {
        enabled: saved.get("enabled").is_some_and(truthy),
        folder: text("folder"),
        last: text("last"),
        hash: text("hash"),
        error: text("error"),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

/// Port of `CloudBackup.save`: the changes over the current settings, stored as the preference.
pub fn save(cx: &mut AppContext, changes: &[(&str, Value)]) {
    let s = settings(cx);
    let mut map = Map::new();
    map.insert("enabled".into(), json!(s.enabled));
    map.insert("folder".into(), json!(s.folder));
    map.insert("last".into(), json!(s.last));
    map.insert("hash".into(), json!(s.hash));
    map.insert("error".into(), json!(s.error));
    for (key, value) in changes {
        map.insert((*key).to_owned(), value.clone());
    }
    if let Err(error) = cx.doc.save_preference(PREF, &Value::Object(map)) {
        log::warn!("cloud backup settings not saved: {error}");
    }
}

/// Port of `snapshot`: a consistent copy of the catalogue in its backups folder, one self-contained file.
pub fn snapshot(cx: &mut AppContext) -> Result<PathBuf, String> {
    let library = cx.doc.library_folder().ok_or_else(|| tr("카탈로그를 찾지 못했습니다."))?;
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let staged = library.join("backups").join("cloud-staging").join(format!("{unique:032x}.sqlite"));
    cx.doc.file_op(FileOp::Backup { path: staged.clone() })?;
    let db = Connection::open(&staged).map_err(|e| e.to_string())?;
    db.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
    drop(db);
    Ok(staged)
}

/// Port of `CloudBackup.start`: backs up now (`manual`) or when enabled (automatic). `notify` gets the message.
/// Returns whether a backup started.
pub fn start(cx: &mut AppContext, manual: bool, notify: Option<Notify>) -> bool {
    let s = settings(cx);
    let state = cx.state::<CloudState>();
    if state.busy || state.closing || s.folder.is_empty() || !(manual || s.enabled) {
        return false;
    }
    let staged = (|| {
        if manual {
            cx.doc.edit_mut().commit("보정 변경");
            cx.doc.flush()?;
        }
        snapshot(cx)
    })();
    let staged = match staged {
        Ok(path) => path,
        Err(error) => {
            finish(cx, Err(error), manual, notify);
            return false;
        }
    };
    cx.state::<CloudState>().busy = true;
    let uploader = uploader(cx);
    let hash = (!manual).then_some(s.hash.clone()).filter(|h| !h.is_empty());
    let folder = PathBuf::from(&s.folder);
    cx.jobs.spawn(
        "cloud_backup",
        "",
        move |_| uploader.upload(&staged, &folder, hash.as_deref()),
        move |cx: &mut AppContext, result: Result<(Option<PathBuf>, String), String>| {
            cx.state::<CloudState>().busy = false;
            finish(cx, result, manual, notify);
        },
    );
    true
}

/// The changes a finished backup makes to the settings.
fn success_changes(cx: &mut AppContext, target: &Option<PathBuf>, value: &str) -> Vec<(&'static str, Value)> {
    let mut changes = vec![("error", json!(""))];
    if !value.is_empty() {
        changes.push(("hash", json!(value)));
    }
    if target.is_some() {
        changes.push(("last", json!(stamp(cx))));
    }
    changes
}

/// Port of `CloudBackup.finish`: the settings, the message (to `notify`, or the status line after a manual
/// backup or an error).
pub fn finish(
    cx: &mut AppContext,
    result: Result<(Option<PathBuf>, String), String>,
    manual: bool,
    notify: Option<Notify>,
) {
    let (message, error) = match result {
        Err(error) => {
            save(cx, &[("error", json!(error))]);
            (tr_with("구글 드라이브 백업 실패: {0}", &[&error]), true)
        }
        Ok((target, value)) => {
            let changes = success_changes(cx, &target, &value);
            save(cx, &changes);
            let message = match &target {
                Some(path) => tr_with("백업했습니다: {0}", &[&path.to_string_lossy()]),
                None => tr("바뀐 내용이 없어 백업을 건너뛰었습니다."),
            };
            (message, false)
        }
    };
    if let Some(notify) = notify {
        notify(cx, message);
    } else if manual || error {
        cx.set_status(message);
    }
}

/// Port of the timer: starts an automatic backup every [`CloudState::interval`] while a library is open.
pub fn tick(cx: &mut AppContext, now: Instant) -> bool {
    if cx.doc.library_folder().is_none() {
        cx.state::<CloudState>().next = None;
        return false;
    }
    let state = cx.state::<CloudState>();
    let interval = state.interval;
    match state.next {
        None => {
            state.next = Some(now + interval);
            false
        }
        Some(due) if now >= due => {
            state.next = Some(now + interval);
            start(cx, false, None)
        }
        Some(_) => false,
    }
}

/// Once per frame (the timer of `CloudBackup`).
pub fn frame(cx: &mut AppContext) {
    if tick(cx, Instant::now()) {
        cx.egui.request_repaint();
    }
    if cx.state_ref::<CloudState>().is_some_and(|s| s.next.is_some()) {
        cx.egui.request_repaint_after(Duration::from_secs(30));
    }
}

/// Port of `CloudBackup.shutdown`: on close (the catalogue still open) waits for a running backup, then backs up
/// the final state when automatic backups are on.
pub fn shutdown(cx: &mut AppContext) {
    let timeout = {
        let state = cx.state::<CloudState>();
        state.closing = true;
        state.next = None;
        state.close_timeout
    };
    let started = Instant::now();
    while cx.state::<CloudState>().busy && started.elapsed() < timeout {
        for finished in cx.jobs.poll() {
            finished(cx);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = settings(cx);
    if !s.enabled || s.folder.is_empty() || cx.state::<CloudState>().busy {
        return;
    }
    let staged = match snapshot(cx) {
        Ok(path) => path,
        Err(error) => {
            save(cx, &[("error", json!(error))]);
            return;
        }
    };
    let uploader = uploader(cx);
    let hash = Some(s.hash.clone()).filter(|h| !h.is_empty());
    let folder = PathBuf::from(&s.folder);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(uploader.upload(&staged, &folder, hash.as_deref()));
    });
    match receiver.recv_timeout(timeout) {
        Err(_) => save(cx, &[("error", json!(tr("종료할 때 백업이 시간 안에 끝나지 않았습니다.")))]),
        Ok(Err(error)) => save(cx, &[("error", json!(error))]),
        Ok(Ok((target, value))) => {
            let changes = success_changes(cx, &target, &value);
            save(cx, &changes);
        }
    }
}

// ---- the dialog --------------------------------------------------------------------------------------------------

/// The line with the last backup and error (`status_text`).
pub fn status_text(cx: &AppContext) -> String {
    let now = settings(cx);
    let last = if now.last.is_empty() { tr("없음") } else { now.last.replace('T', " ") };
    let text = tr_with("마지막 백업: {0}", &[&last]);
    if now.error.is_empty() {
        text
    } else {
        format!("{text}\n{}", tr_with("최근 오류: {0}", &[&now.error]))
    }
}

#[derive(Default)]
struct Pending {
    folder: Option<String>,
    message: Option<String>,
}

/// The dialog's model (`dialog` without the widgets).
pub struct CloudBackupModel {
    /// The check box 자동 백업.
    pub enabled: bool,
    /// The folder field.
    pub folder: String,
    /// The line with the status.
    pub status: String,
    /// 지금 백업 is off while one runs.
    pub running: bool,
    touched: bool,
    pending: Arc<Mutex<Pending>>,
}

impl CloudBackupModel {
    /// The fields as `dialog` fills them: the stored settings, the default folder when there is none.
    pub fn new(cx: &mut AppContext) -> Self {
        let s = settings(cx);
        let drive = cx.state::<CloudState>().drive.clone().flatten();
        Self {
            enabled: s.enabled,
            folder: if s.folder.is_empty() { default_folder(drive.as_deref()) } else { s.folder },
            status: status_text(cx),
            running: false,
            touched: false,
            pending: Arc::default(),
        }
    }

    /// Whether Google Drive was looked for and not found (the label that says so).
    pub fn drive_missing(cx: &mut AppContext) -> bool {
        matches!(cx.state::<CloudState>().drive, Some(None))
    }

    /// Port of `store`.
    pub fn store(&self, cx: &mut AppContext) {
        let folder = self.folder.trim().to_owned();
        save(cx, &[("enabled", json!(self.enabled && !folder.is_empty())), ("folder", json!(folder))]);
    }

    /// Port of `run_now` (지금 백업).
    pub fn run_now(&mut self, cx: &mut AppContext) {
        if self.folder.trim().is_empty() {
            self.status = tr("백업 폴더를 선택하세요.");
            return;
        }
        self.store(cx);
        self.running = true;
        self.status = tr("백업하는 중…");
        let pending = self.pending.clone();
        let notify: Notify = Box::new(move |_, message| {
            if let Ok(mut pending) = pending.lock() {
                pending.message = Some(message);
            }
        });
        if !start(cx, true, Some(notify)) {
            self.running = false;
            self.status = status_text(cx);
        }
    }

    /// Port of the OK button: stores, and starts the first automatic backup when none was made yet.
    pub fn accept(&self, cx: &mut AppContext) {
        self.store(cx);
        if self.enabled && !self.folder.trim().is_empty() && settings(cx).last.is_empty() {
            start(cx, false, None);
        }
    }

    /// Takes what the folder dialog and the backup answered, and the drive scan.
    pub fn poll(&mut self, cx: &mut AppContext) {
        let (folder, message) = match self.pending.lock() {
            Ok(mut p) => (p.folder.take(), p.message.take()),
            Err(_) => (None, None),
        };
        if let Some(folder) = folder {
            self.folder = folder;
            self.touched = true;
        }
        if let Some(message) = message {
            self.running = false;
            self.status = format!("{message}\n{}", status_text(cx));
        }
        if !self.touched && self.folder.is_empty() {
            let drive = cx.state::<CloudState>().drive.clone().flatten();
            self.folder = default_folder(drive.as_deref());
        }
    }

    /// Port of the 폴더 열기 button: opens the folder when it exists.
    pub fn open_folder(&self) {
        if Path::new(&self.folder).is_dir() {
            launch(&self.folder);
        }
    }
}

/// Looks for Google Drive once, on a job (a drive that does not answer must not stop the window).
pub fn detect_drive(cx: &mut AppContext) {
    let state = cx.state::<CloudState>();
    if state.drive.is_some() || state.scanning {
        return;
    }
    state.scanning = true;
    cx.jobs.spawn(
        "drive_scan",
        "",
        |_| Ok(google_drive()),
        |cx: &mut AppContext, result: Result<Option<PathBuf>, String>| {
            let state = cx.state::<CloudState>();
            state.scanning = false;
            state.drive = Some(result.ok().flatten());
        },
    );
}

/// The window.
pub struct CloudBackupDialog {
    /// The model.
    pub model: CloudBackupModel,
}

impl CloudBackupDialog {
    /// A window for the open library.
    pub fn new(cx: &mut AppContext) -> Self {
        detect_drive(cx);
        Self { model: CloudBackupModel::new(cx) }
    }
}

impl Dialog for CloudBackupDialog {
    fn id(&self) -> &'static str {
        "cloud_backup"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        self.model.poll(cx);
        let missing = CloudBackupModel::drive_missing(cx);
        let (mut ok, mut cancel, mut now, mut open, mut choose) = (false, false, false, false, false);
        let response = egui::Modal::new(egui::Id::unique("cloud_backup")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(488.0);
            ui.label(egui::RichText::new(tr("구글 드라이브 백업")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            widgets::label(
                ui,
                &tr("보정값·편집 이력·별점·키워드·컬렉션·프리셋이 담긴 카탈로그를 백업합니다. 원본 사진은 올리지 않습니다."),
                t::TEXT,
                t::FONT_SIZE,
            );
            if missing {
                ui.add_space(6.0);
                widgets::label(
                    ui,
                    &tr("Google Drive 데스크톱 앱을 찾지 못했습니다. 설치하고 로그인하면 내 드라이브 폴더를 고를 수 있습니다."),
                    t::TEXT,
                    t::FONT_SIZE,
                );
            }
            ui.add_space(8.0);
            widgets::checkbox(ui, &mut self.model.enabled, &tr("자동 백업 (30분마다 바뀐 내용이 있으면, 그리고 종료할 때)"));
            ui.add_space(6.0);
            widgets::label(ui, &tr("백업 폴더"), t::TEXT, t::FONT_SIZE);
            let before = self.model.folder.clone();
            widgets::line_edit(ui, "cloud_backup_folder", &mut self.model.folder, "");
            if self.model.folder != before {
                self.model.touched = true;
            }
            ui.add_space(4.0);
            if widgets::button(ui, &tr("폴더 선택")).clicked() {
                choose = true;
            }
            ui.add_space(6.0);
            widgets::label(ui, &self.model.status, t::TEXT, t::FONT_SIZE);
            ui.add_space(6.0);
            widgets::label(
                ui,
                &tr_with(
                    "최근 {0}개는 모두, 그보다 오래된 것은 달마다 1개씩 남기고 나머지는 휴지통으로 옮깁니다. 복원: 파일 관리 › 카탈로그 복원",
                    &[&KEEP.to_string()],
                ),
                t::MUTED,
                t::FONT_SIZE,
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if Button::new(&tr("지금 백업")).enabled(!self.model.running).show(ui).clicked() {
                    now = true;
                }
                if widgets::button(ui, &tr("폴더 열기")).clicked() {
                    open = true;
                }
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, "Cancel").clicked() {
                    cancel = true;
                }
                ui.add_space(6.0);
                if Button::new("OK").kind(Kind::Primary).show(ui).clicked() {
                    ok = true;
                }
            });
        });
        if choose {
            let pending = self.model.pending.clone();
            let start = PathBuf::from(&self.model.folder);
            let picked = PathWait::open(tr("백업 폴더"), Pick::Folder, Some(start), move |_, path| {
                if let Ok(mut pending) = pending.lock() {
                    pending.folder = Some(path.to_string_lossy().into_owned());
                }
            });
            cx.open_dialog(Box::new(picked));
        }
        if now {
            self.model.run_now(cx);
        }
        if open {
            self.model.open_folder();
        }
        if ok {
            self.model.accept(cx);
        }
        if self.model.running {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        !(ok || cancel || response.should_close())
    }
}
