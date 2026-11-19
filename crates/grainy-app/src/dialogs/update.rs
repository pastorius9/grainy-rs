//! Updates (work package 31): the rest of luma/updater.py `UpdateController` beside what package 26 has
//! (`commands::settings`: the menu items, `check_for_update`, `checked`, the offer box): the first-run question
//! and the daily check (`startup`), the question after a run that did not close (`after_crash`), the header's
//! update button, and the install (`install`: refusal while a job runs, the progress box, failure, cancel and the
//! hand-over that closes the window).
//!
//! The download, the unpacking and the start of the new version go through a [`Backend`]: the program's start
//! installs `crate::network::NetworkBackend` (unless the network is switched off), which takes only this Rust
//! build's releases (`updater::RUST_MARKER`); without one it is [`Unavailable`], so 지금 업데이트 ends in the
//! oracle's "업데이트를 준비하지 못했습니다" box. Tests put a fake in [`UpdateEnv::backend`].

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;
use grainy_platform::json::Value as Pref;
use grainy_platform::updater::{self, Release};

use super::about::MessageBox;
use super::progress::{ProgressBox, Shared};
use crate::commands::settings::{self, check_for_update, preferences, UpdateState};
use crate::context::AppContext;
use crate::startup::{tr, unavailable};
use crate::text::tr_with;
use crate::widgets::{Button, Kind};

/// The part of an update the program cannot do by itself here: reading the release, unpacking it and starting
/// the new version.
pub trait Backend: Send + Sync {
    /// Port of `download` and `stage`: the release zip into `work`, checked, unpacked; the staged folder, or
    /// none when `cancel` was set. `progress(done, total)` reports the download.
    fn download_and_stage(
        &self,
        release: &Release,
        work: &Path,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<Option<PathBuf>, String>;

    /// Port of `launch`: starts the new version's installer step, which waits for this program to exit.
    fn launch(&self, staged: &Path, app: &Path, arguments: &[String]) -> Result<(), String>;
}

/// The backend of this build.
pub struct Unavailable;

impl Backend for Unavailable {
    fn download_and_stage(
        &self,
        _release: &Release,
        _work: &Path,
        _progress: &mut dyn FnMut(u64, u64),
        _cancel: &AtomicBool,
    ) -> Result<Option<PathBuf>, String> {
        Err(tr("이 빌드에서는 업데이트를 내려받을 수 없습니다."))
    }

    fn launch(&self, _staged: &Path, _app: &Path, _arguments: &[String]) -> Result<(), String> {
        Err(unavailable())
    }
}

/// How an install ended (read by tests; the window shows the matching box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A job was running: nothing started.
    Refused,
    /// Preparing the update failed with this reason.
    Failed(String),
    /// 취소 in the progress box.
    Cancelled,
    /// The new version was started and this window was asked to close.
    Handover,
}

/// What the update dialogs use from outside (`cx.state::<UpdateEnv>()`); tests replace the parts.
#[derive(Default)]
pub struct UpdateEnv {
    /// The download and launch; [`Unavailable`] when none is set.
    pub backend: Option<Arc<dyn Backend>>,
    /// The running program's file (`sys.executable`); the real one when none.
    pub executable: Option<PathBuf>,
    /// The arguments the program was started with (`sys.argv[1:]`); the real ones when none.
    pub arguments: Option<Vec<String>>,
    /// Whether this system has an installer (`SUPPORTED`); the real answer when none.
    pub supported: Option<bool>,
    /// How the last install ended.
    pub outcome: Option<Outcome>,
}

fn supported(cx: &mut AppContext) -> bool {
    cx.state::<UpdateEnv>().supported.unwrap_or_else(|| updater::Layout::current().supported())
}

fn packaged(cx: &mut AppContext) -> bool {
    cx.state::<UpdateState>().packaged.unwrap_or_else(settings::packaged)
}

fn executable(cx: &mut AppContext) -> PathBuf {
    cx.state::<UpdateEnv>().executable.clone().or_else(|| std::env::current_exe().ok()).unwrap_or_default()
}

fn save(cx: &mut AppContext, key: &str, value: Pref) {
    if let Some(mut prefs) = preferences(cx) {
        // `save` ignores an `OSError`, as the controller does.
        if let Err(error) = prefs.save(key, value) {
            log::warn!("{key} not saved: {error}");
        }
    }
}

fn cleanup(cx: &mut AppContext) {
    let layout = updater::Layout::current();
    let exe = executable(cx);
    if let Some(app) = updater::app_folder(&exe, &layout) {
        updater::cleanup_for(&app, &layout, &exe, &mut |path| updater::recycle(path), &updater::RS_PACKAGE);
    }
}

/// Seconds since 1970 (`time.time()`).
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The check the daily rule asks for: automatic checks on, and none made in the last [`updater::INTERVAL`].
pub fn check_if_due(cx: &mut AppContext) {
    let Some(prefs) = preferences(cx) else { return };
    let on = matches!(prefs.values.get("update_check"), Some(Pref::Bool(true)));
    let last = match prefs.values.get("update_checked") {
        Some(Pref::Int(n)) => Some(*n as f64),
        Some(Pref::Float(x)) => Some(*x),
        _ => None,
    };
    if on && last.is_none_or(|last| now() - last >= updater::INTERVAL as f64) {
        check_for_update(cx, false);
    }
}

/// The question of the first start: 새 버전이 나오면 알려 드릴까요? (`QMessageBox` of `startup`).
pub fn first_run_box() -> MessageBox {
    let mut message =
        MessageBox::information("update_first_run", tr("업데이트"), tr("새 버전이 나오면 알려 드릴까요?"));
    message.informative = tr("켜 두면 Grainy를 시작할 때 하루에 한 번 GitHub에서 최신 버전 번호만 확인합니다. 사진이나 사용 정보는 보내지 않으며, 설정 메뉴에서 언제든 끌 수 있습니다.");
    message.buttons = vec![
        (
            tr("자동 확인 켜기"),
            Some(Box::new(|cx: &mut AppContext| {
                save(cx, "update_check", Pref::Bool(true));
                check_if_due(cx);
            })),
        ),
        (tr("끄기"), Some(Box::new(|cx: &mut AppContext| save(cx, "update_check", Pref::Bool(false))))),
    ];
    message
}

/// Port of `UpdateController.startup`: once per start of the shipped program, tidies what an update left, asks
/// about automatic checks the first time, then checks when one is due.
pub fn startup(cx: &mut AppContext) {
    if !packaged(cx) || !supported(cx) {
        return;
    }
    cleanup(cx);
    let asked = preferences(cx).is_some_and(|p| !matches!(p.values.get("update_check"), None | Some(Pref::Null)));
    if !asked && preferences(cx).is_some() {
        cx.open_dialog(Box::new(first_run_box()));
        return;
    }
    check_if_due(cx);
}

/// The question after a run that did not close (`QMessageBox` of `after_crash`).
pub fn crash_box() -> MessageBox {
    let mut message =
        MessageBox::information("update_crash", tr("업데이트"), tr("지난번에 Grainy가 정상적으로 닫히지 않았습니다."));
    message.informative = tr("새 버전에서 고쳐졌을 수 있습니다. GitHub에서 최신 버전 번호를 확인할까요? 사진이나 사용 정보는 보내지 않습니다.");
    message.buttons = vec![
        (tr("새 버전 확인"), Some(Box::new(|cx: &mut AppContext| check_for_update(cx, true)))),
        (tr("계속"), None),
    ];
    message
}

/// Port of `UpdateController.after_crash`: the run before ended without closing and no photo was opened; a newer
/// version is offered first, as it may fix what ended that run. With automatic checks on it checks at once,
/// otherwise it asks, since checking reaches the network.
pub fn after_crash(cx: &mut AppContext) {
    cx.set_status(tr("지난번에 Grainy가 정상적으로 닫히지 않아 사진을 자동으로 열지 않았습니다."));
    if !packaged(cx) || !supported(cx) {
        return;
    }
    cleanup(cx);
    if crate::commands::settings::update_check_enabled(cx) {
        check_for_update(cx, true);
    } else {
        cx.open_dialog(Box::new(crash_box()));
    }
}

/// Whether a job the install must not interrupt is running (`export_running`, `import_busy`, `import_scans`,
/// `maintenance_running`, `command_running`).
fn busy(cx: &AppContext) -> bool {
    ["import", "export", "maintenance", "files", "auto", "previews"].iter().any(|kind| cx.jobs.running(kind))
        || cx.dialog_open("maintenance")
}

/// The box shown when a job is running (`QMessageBox.information` of `install`).
pub fn busy_box() -> MessageBox {
    MessageBox::information(
        "update_busy",
        tr("업데이트"),
        tr("가져오기·내보내기 같은 작업이 끝난 뒤 다시 시도해 주세요."),
    )
}

/// The box shown when the update could not be prepared (`QMessageBox.warning` of `install`).
pub fn failed_box(reason: &str) -> MessageBox {
    MessageBox::warning("update_failed", tr("업데이트"), tr_with("업데이트를 준비하지 못했습니다.\n{0}", &[reason]))
}

/// Port of `UpdateController.install`: the progress box while the release downloads, then the hand-over to the
/// new version (which closes this window), or the box of what went wrong.
pub fn install(cx: &mut AppContext, release: Release) {
    if busy(cx) {
        cx.state::<UpdateEnv>().outcome = Some(Outcome::Refused);
        cx.open_dialog(Box::new(busy_box()));
        return;
    }
    let layout = updater::Layout::current();
    let exe = executable(cx);
    let Some(app) = updater::app_folder(&exe, &layout) else {
        fail(cx, tr("업데이트할 수 있는 설치본이 아닙니다."));
        return;
    };
    let work = updater::work_folder_for(&app, &layout, &updater::RS_PACKAGE);
    let backend: Arc<dyn Backend> = cx.state::<UpdateEnv>().backend.clone().unwrap_or_else(|| Arc::new(Unavailable));
    let arguments = cx
        .state::<UpdateEnv>()
        .arguments
        .clone()
        .unwrap_or_else(|| updater::restart_arguments(&std::env::args().skip(1).collect::<Vec<_>>()));
    let shared = Shared::new(100);
    let (worker_shared, done_shared) = (shared.clone(), shared.clone());
    let launcher = backend.clone();
    let job = cx.jobs.spawn(
        "update_install",
        tr("새 버전을 내려받는 중…"),
        move |handle| {
            let mut report = |done: u64, total: u64| {
                let percent = (done * 100 / total.max(1)).min(99) as usize;
                worker_shared.set_done(percent);
                handle.progress(percent, 100, "");
            };
            backend.download_and_stage(&release, &work, &mut report, handle.cancel_flag())
        },
        move |cx: &mut AppContext, result: Result<Option<PathBuf>, String>| {
            done_shared.finish();
            match result {
                Err(reason) => fail(cx, reason),
                Ok(None) => cx.state::<UpdateEnv>().outcome = Some(Outcome::Cancelled),
                Ok(Some(staged)) => match launcher.launch(&staged, &app, &arguments) {
                    Ok(()) => {
                        cx.state::<UpdateEnv>().outcome = Some(Outcome::Handover);
                        cx.egui.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(reason) => fail(cx, reason),
                },
            }
        },
    );
    cx.open_dialog(Box::new(ProgressBox::new(job, String::new(), tr("새 버전을 내려받는 중…"), shared)));
}

fn fail(cx: &mut AppContext, reason: String) {
    cx.state::<UpdateEnv>().outcome = Some(Outcome::Failed(reason.clone()));
    cx.open_dialog(Box::new(failed_box(&reason)));
}

/// The header's update button (`self.update_button`): shown while a newer release is known; it opens the offer.
pub fn header_button(ui: &mut egui::Ui, cx: &mut AppContext) {
    let Some(release) = cx.state_ref::<UpdateState>().and_then(|s| s.release.clone()) else { return };
    ui.add_space(12.0);
    let label = tr_with("새 버전 {0}", &[&release.version]);
    if Button::new(&label).kind(Kind::Primary).show(ui).clicked() {
        settings::offer(cx, release);
    }
}
