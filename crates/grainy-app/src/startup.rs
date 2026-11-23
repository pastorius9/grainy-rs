//! Start-up and shut-down (work package 31): the crash marker and safe mode (luma/safe_start.py,
//! luma/desktop_session.py `DesktopSession.start`), the steps that follow the window (the update question and the
//! daily check, `UpdateController.after_crash`, `file_access.check_folders`) and the folder-permission guide
//! (luma/file_access.py `access`, `unreadable_folders`, `guide`, `check_folders`).
//!
//! The marker `running.json` is written only in the application's own data folder (`cx.data_folder`, never the
//! Python Grainy's settings folder). A run that finds the marker of a run that did not close is in **safe mode**:
//! no photo opens by itself (`MainWindow.restore_folder_selection`), and a newer version is offered first.
//!
//! **Network.** Every network action of the package sits behind a trait: `dialogs::feedback::Transport`,
//! `dialogs::update::Backend` and the `commands::settings::UpdateOpener` of package 26. The program's start gives
//! them the real network (`crate::network::install`) unless it is switched off (`--offline`,
//! `GRAINY_OFFLINE=1`); otherwise, and in tests, they answer that it is not available ([`unavailable`],
//! installed by [`begin`]).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use grainy_i18n::Language;
use grainy_library::file_access::{self, Guide, GuideStep};
use grainy_platform::{safe_start, user_paths::System, APP_VERSION};

use crate::commands::settings::{launch, UpdateOpener, UpdateState};
use crate::context::AppContext;
use crate::dialogs::about::MessageBox;
use crate::dialogs::{cloud_backup, update};

/// Korean source → English of the strings this package adds (the oracle's own are in `grainy_i18n`).
const ENGLISH: [(&str, &str); 4] = [
    ("카탈로그를 찾지 못했습니다.", "The catalog was not found."),
    ("업데이트할 수 있는 설치본이 아닙니다.", "This is not an installation that can update itself."),
    ("이 빌드에서는 네트워크 기능을 쓸 수 없습니다.", "Network access is not available in this build."),
    ("이 빌드에서는 업데이트를 내려받을 수 없습니다.", "Updates cannot be downloaded in this build."),
];

/// [`crate::text::tr`] with this package's table first.
pub fn tr(source: &str) -> String {
    if grainy_i18n::language() == Language::English {
        if let Some((_, english)) = ENGLISH.iter().find(|(k, _)| *k == source) {
            return (*english).to_owned();
        }
    }
    crate::text::tr(source)
}

/// What every network action of this package answers.
pub fn unavailable() -> String {
    tr("이 빌드에서는 네트워크 기능을 쓸 수 없습니다.")
}

/// The reader of the update feed this build has: it always fails with [`unavailable`].
pub fn unavailable_opener() -> UpdateOpener {
    std::sync::Arc::new(|_url: &str| Err(std::io::Error::other(unavailable())))
}

// ---- the steps after the window --------------------------------------------------------------------------------

/// What runs once the window stands (the oracle's `QTimer.singleShot` chain of `DesktopSession.start`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `UpdateController.after_crash` (the run before did not close).
    AfterCrash,
    /// `UpdateController.startup`: the first-run question, the daily check.
    UpdateStartup,
    /// 파이썬 Grainy 라이브러리 가져오기: the question once, for an empty library ([`crate::python_import::offer`]).
    PythonLibrary,
    /// `file_access.check_folders`.
    CheckFolders,
}

/// The state of the start (`cx.state::<Startup>()`).
#[derive(Default)]
pub struct Startup {
    /// [`begin`] ran.
    pub begun: bool,
    /// The run before did not close: no photo opens by itself (`window.safe_start`).
    pub safe: bool,
    /// The folder holding the marker (the data folder).
    pub folder: Option<PathBuf>,
    /// What is still to run, in order; a step runs when no dialog is open.
    pub steps: VecDeque<Step>,
    /// The once-per-run state of the folder guide (`file_access.shown` and `pending`).
    pub guide: Guide,
    /// The place of a guide asked for while another dialog was open, shown when that one is closed.
    pub guide_place: Option<String>,
    checking_folders: bool,
    frames: u32,
}

/// Whether the run before did not close (safe mode).
pub fn safe_mode(cx: &AppContext) -> bool {
    cx.state_ref::<Startup>().is_some_and(|s| s.safe)
}

/// Port of `DesktopSession.start` after the language: leaves the marker in the data folder and plans the steps.
/// Returns whether the run before ended without closing. Does nothing without a data folder (a session).
pub fn begin(cx: &mut AppContext) -> bool {
    let Some(folder) = cx.data_folder.clone() else { return false };
    let crashed = safe_start::begin(&folder, APP_VERSION).is_some();
    plan(cx, &folder, crashed);
    crashed
}

/// [`begin`] when the marker was read already (tests give `crashed` and the folder).
pub fn plan(cx: &mut AppContext, folder: &Path, crashed: bool) {
    let state = cx.state::<UpdateState>();
    if state.opener.is_none() {
        state.opener = Some(unavailable_opener());
    }
    if state.installer.is_none() {
        state.installer = Some(update::install);
    }
    let steps: VecDeque<Step> = if crashed {
        [Step::AfterCrash, Step::CheckFolders].into()
    } else {
        [Step::UpdateStartup, Step::PythonLibrary, Step::CheckFolders].into()
    };
    let startup = cx.state::<Startup>();
    startup.begun = true;
    startup.safe = crashed;
    startup.folder = Some(folder.to_path_buf());
    startup.steps = steps;
    startup.frames = 0;
    if crashed {
        cx.doc.start_without_photo();
    }
}

/// Frames to wait before the first step (the oracle waits 300 ms to 1.5 s).
const FIRST_STEP_AFTER: u32 = 3;

/// Once per frame: the timer of the cloud backup, a folder guide that waited for another dialog, and the next
/// step after the window (each when no dialog is open, so a question does not cover another).
pub fn frame(cx: &mut AppContext) {
    cloud_backup::frame(cx);
    if !cx.state::<Startup>().begun || cx.modal_open() {
        return;
    }
    if let Some(place) = cx.state::<Startup>().guide_place.take() {
        let startup = cx.state::<Startup>();
        startup.guide.retry_fired();
        guide(cx, &place);
        return;
    }
    let startup = cx.state::<Startup>();
    if startup.steps.is_empty() {
        return;
    }
    startup.frames += 1;
    if startup.frames < FIRST_STEP_AFTER {
        cx.egui.request_repaint();
        return;
    }
    if let Some(step) = cx.state::<Startup>().steps.pop_front() {
        run_step(cx, step);
        cx.egui.request_repaint();
    }
}

/// Runs one step now.
pub fn run_step(cx: &mut AppContext, step: Step) {
    match step {
        Step::AfterCrash => update::after_crash(cx),
        Step::UpdateStartup => update::startup(cx),
        Step::PythonLibrary => crate::python_import::offer(cx),
        Step::CheckFolders => check_folders(cx),
    }
}

/// Port of the close of the window: a normal close removes this run's marker, then the final cloud backup
/// (`CloudBackup.shutdown`, "catalog still open").
pub fn shutdown(cx: &mut AppContext) {
    cloud_backup::shutdown(cx);
    if let Some(folder) = cx.state::<Startup>().folder.take() {
        safe_start::end(&folder);
    }
}

// ---- file_access -----------------------------------------------------------------------------------------------
// `access`, `present`, `denied`, `unreadable_folders` and the texts of `guide` are grainy-library's port
// (`grainy_library::file_access`); this is the window's part: the box, once per run, and the folder check.

/// The box of `guide` for a system (`mac`) and a place.
pub fn guide_box(place: &str, mac: bool) -> MessageBox {
    let text = file_access::guide_text(place, mac);
    let mut message = MessageBox::warning("file_access", text.title, text.text);
    message.informative = text.informative;
    message.buttons.clear();
    if let Some(name) = text.settings_button {
        message.buttons.push((name, Some(Box::new(|_| launch(file_access::SETTINGS_URL)))));
    }
    message.buttons.push((text.close_button, None));
    message
}

/// Port of `guide`: explains how to let Grainy read the folder again, once per run. Returns whether it was shown
/// now; with another dialog open it waits for that one to close (the oracle asks again a second later).
pub fn guide(cx: &mut AppContext, place: &str) -> bool {
    let modal = cx.modal_open();
    let startup = cx.state::<Startup>();
    match startup.guide.request(modal) {
        GuideStep::Skip => false,
        GuideStep::RetryLater => {
            startup.guide_place = Some(place.to_owned());
            false
        }
        GuideStep::Show => {
            cx.open_dialog(Box::new(guide_box(place, System::current() == System::MacOs)));
            true
        }
    }
}

/// Port of `check_folders`: once after start-up and off the window's thread, each registered folder root is
/// listed (never the photos in it); a folder the system refuses shows [`guide`]. A volume that does not answer
/// only keeps the job waiting.
pub fn check_folders(cx: &mut AppContext) {
    if cx.state::<Startup>().guide.shown || cx.state::<Startup>().checking_folders {
        return;
    }
    let roots = cx.doc.metadata_catalog().and_then(|catalog| catalog.folder_roots().ok()).unwrap_or_default();
    if roots.is_empty() {
        return;
    }
    cx.state::<Startup>().checking_folders = true;
    cx.jobs.spawn(
        "folder_access",
        "",
        move |_| Ok(file_access::unreadable_folders(&roots)),
        |cx: &mut AppContext, result: Result<Vec<String>, String>| {
            cx.state::<Startup>().checking_folders = false;
            if let Some(first) = result.ok().and_then(|found| found.into_iter().next()) {
                guide(cx, &first);
            }
        },
    );
}
