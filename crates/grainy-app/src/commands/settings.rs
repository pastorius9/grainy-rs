//! 설정 (work package 26): the language, the update items, 이번 버전의 달라진 점…, Grainy 정보…, Grainy 배우기,
//! 사용법 / 구현 범위, lights out (L) and the saved preferences that belong to the window (luma/app.py
//! `set_gpu`-side items, luma/desktop_session.py `choose_language`, `change_language`, `whats_new`;
//! luma/screen_modes.py `toggle_lights`, `leave`; luma/updater.py `UpdateController`; luma/app.py `show_about`,
//! `show_help`, `toggle_drt_advanced`).
//!
//! The application preferences are the Rust app's own `settings.json` in its data folder (never the Python
//! Grainy's file): [`preferences`] opens it through `grainy_platform::Preferences`, the port of luma/preferences.py.
//! The checkable GPU item is `gpu_worker::menu`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use grainy_i18n::Language;
use grainy_platform::json::Value as Pref;
use grainy_platform::preferences::Preferences;
use grainy_platform::{updater, whats_new, APP_VERSION, RS_VERSION};
use serde_json::Value;

use super::{always, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::about::{self, Action, MessageBox};
use crate::dialogs::language::LanguageDialog;
use crate::dialogs::whats_new::WhatsNew;
use crate::text::{tr, tr_with};

/// The interactive tutorial (`LEARN_URL` of luma/app.py).
pub const LEARN_URL: &str = "https://pastorius9.github.io/grainy/learn/";

pub const COMMANDS: &[Command] = &[
    Command { id: "view.lights", label: "조명 끄기", enabled: always, run: lights },
    Command { id: "settings.language", label: "언어 / Language…", enabled: always, run: language },
    Command { id: "settings.update_auto", label: "새 버전 자동 확인", enabled: always, run: update_auto },
    Command { id: "settings.update_check", label: "업데이트 확인…", enabled: always, run: update_check },
    Command {
        id: "settings.whats_new", label: "이번 버전의 달라진 점…", enabled: always, run: show_whats_new
    },
    Command { id: "settings.about", label: "Grainy 정보…", enabled: always, run: show_about },
    Command { id: "settings.learn", label: "Grainy 배우기 (웹 튜토리얼)…", enabled: always, run: learn },
    Command { id: "settings.help", label: "사용법 / 구현 범위", enabled: always, run: help },
];

// ---- the preferences file ---------------------------------------------------------------------------------------

/// The Rust app's settings file (`settings.json` in the data folder), `None` for a session without a data
/// folder (`--memory`, tests).
pub fn preferences(cx: &AppContext) -> Option<Preferences> {
    cx.data_folder.as_ref().map(|folder| Preferences::new(folder.join("settings.json")))
}

/// A stored preference as a `serde_json` value.
pub fn to_json(value: &Pref) -> Value {
    serde_json::from_str(&value.dump(None, false)).unwrap_or(Value::Null)
}

/// A `serde_json` value as a stored preference.
pub fn from_json(value: &Value) -> Pref {
    Pref::from(value)
}

impl AppContext {
    /// Sets the folder the application preferences (`settings.json`) and the snapshots of read-only libraries
    /// live in (`None`: nothing is stored).
    pub fn set_data_folder(&mut self, folder: Option<PathBuf>) {
        self.data_folder =
            folder.filter(|path| !crate::paths::is_oracle_folder(path, &crate::paths::Locations::current()));
    }

    /// The data folder.
    pub fn data_folder(&self) -> Option<&Path> {
        self.data_folder.as_deref()
    }

    /// Whether the window is full screen (F).
    pub fn full_screen(&self) -> bool {
        self.full_screen
    }

    /// Whether the DRT's advanced sliders show.
    pub fn drt_advanced(&self) -> bool {
        self.drt_advanced
    }

    /// Opens exactly these panels of the Develop column (a restored session).
    pub fn set_expanded_panels(&mut self, keys: &[&'static str]) {
        self.expanded = keys.to_vec();
    }
}

/// Once per frame (one hook line in `AppContext::poll_background`): the window state, the DRT's advanced switch.
pub fn poll(cx: &mut AppContext) {
    crate::window_state::poll(cx);
    drt_sync(cx);
}

/// At start of `GrainyApp::from_arguments`, before the window is built: the stored interface language (the
/// oracle's `DesktopSession.start` → `apply_language`). A missing or unknown language leaves the default.
pub fn apply_stored_language(data_folder: &Path) {
    if let Some(language) = Preferences::new(data_folder.join("settings.json")).language() {
        grainy_i18n::set_language(language);
    }
}

/// After the window is built (`DesktopSession.start`): the first-run language question, otherwise what's new;
/// then the stored window layout.
pub fn startup(cx: &mut AppContext) {
    let Some(prefs) = preferences(cx) else { return };
    crate::window_state::restore(cx);
    if prefs.language().is_none() {
        cx.open_dialog(Box::new(LanguageDialog::new(None, true, system_is_korean())));
    } else {
        whats_new_on_start(cx, false, packaged());
    }
}

/// Whether this is the shipped program (the oracle's `sys.frozen`): a release build.
pub fn packaged() -> bool {
    !cfg!(debug_assertions)
}

// ---- language -----------------------------------------------------------------------------------------------------

/// Whether a locale name is Korean (`QLocale.system().language() == Korean`): `ko`, `ko-KR`, `ko_KR.UTF-8`, `Korean`.
pub fn is_korean_locale(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name == "ko" || name.starts_with("ko-") || name.starts_with("ko_") || name.starts_with("korean")
}

/// The system's interface locale, from the environment and, on Windows and macOS, the system settings.
fn system_locale() -> Option<String> {
    for variable in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Some(value) = std::env::var(variable).ok().filter(|v| !v.is_empty() && v != "C" && v != "POSIX") {
            return Some(value);
        }
    }
    let output = if cfg!(windows) {
        std::process::Command::new("reg")
            .args(["query", r"HKCU\Control Panel\International", "/v", "LocaleName"])
            .output()
            .ok()?
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output().ok()?
    } else {
        return None;
    };
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.split_whitespace().last().map(str::to_owned)
}

/// The preselected language of the first-run question.
pub fn system_is_korean() -> bool {
    system_locale().is_some_and(|name| is_korean_locale(&name))
}

fn language(cx: &mut AppContext, _: &Invocation) {
    cx.open_dialog(Box::new(LanguageDialog::new(Some(grainy_i18n::language()), false, false)));
}

/// Whether a job the oracle's `change_language` waits for runs (an import, an export, a catalogue job, a batch
/// command): any job with a name on the status line.
fn busy(cx: &AppContext) -> bool {
    cx.jobs.status().is_some()
}

/// Port of `DesktopSession.change_language`: returns whether the language is the one asked for afterwards. The
/// window is not rebuilt here (everything is drawn from the context every frame); what a new window would start
/// with is reset: the status line, the zoom, the comparison with the original and the clipping overlay. The open
/// photo, the screen and the panels stay, as the oracle restores them.
pub fn change_language(cx: &mut AppContext, selected: Language) -> bool {
    if selected == grainy_i18n::language() {
        return true;
    }
    if busy(cx) {
        cx.set_status(tr("작업이 끝난 뒤 언어를 변경해 주세요."));
        return false;
    }
    if let Some(mut prefs) = preferences(cx) {
        if prefs.save_language(selected.code()).is_err() {
            cx.open_dialog(Box::new(MessageBox::warning(
                "save_language",
                tr("설정"),
                tr("언어 설정을 저장하지 못했습니다."),
            )));
            return false;
        }
    }
    // The window closes first: the open photo's edits are stored (`commit`), the layout is kept.
    cx.doc.edit_mut().commit("보정 변경");
    cx.flush();
    crate::window_state::flush(cx);
    cx.set_language(selected);
    cx.set_status(tr("준비됨  ·  사진을 가져오면 시작할 수 있습니다."));
    cx.auto_note = tr("자동톤: 검정·흰색 양끝 3% 여유\n목표 밝기 약 8–247 / 255");
    cx.set_zoom(crate::context::Zoom::Fit);
    cx.pan = eframe::egui::Vec2::ZERO;
    cx.before = false;
    cx.set_clipping(false);
    true
}

/// The answer of the language box (`DesktopSession.start` for the first run, `choose_language` otherwise).
/// Returns whether the dialog is done; at the first run a language that cannot be saved ends the program, as in
/// the oracle (which returns `False` from `start`).
pub fn language_chosen(cx: &mut AppContext, first_run: bool, selected: Language) {
    if !first_run {
        change_language(cx, selected);
        return;
    }
    if let Some(mut prefs) = preferences(cx) {
        if prefs.save_language(selected.code()).is_err() {
            cx.open_dialog(Box::new(MessageBox::warning(
                "save_language",
                "Grainy".to_owned(),
                "Could not save the language setting.\n언어 설정을 저장하지 못했습니다.".to_owned(),
            )));
            cx.egui.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
            return;
        }
    }
    cx.set_language(selected);
    cx.set_status(tr("준비됨  ·  사진을 가져오면 시작할 수 있습니다."));
    cx.auto_note = tr("자동톤: 검정·흰색 양끝 3% 여유\n목표 밝기 약 8–247 / 255");
    whats_new_on_start(cx, true, packaged());
}

/// A cancelled first-run question closes the program (`DesktopSession.start` returns `False`).
pub fn language_cancelled(cx: &mut AppContext, first_run: bool) {
    if first_run {
        cx.egui.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
    }
}

// ---- what's new, about, learn, help -------------------------------------------------------------------------------

/// The folder with `CHANGELOG.md` and `assets/whats-new-en.md`: next to the program when they ship there, else
/// the copies in the platform crate (development).
pub fn notes_root() -> PathBuf {
    if let Some(folder) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        if folder.join("CHANGELOG.md").is_file() {
            return folder;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("grainy-platform").join("assets").join("root")
}

/// Port of `DesktopSession.whats_new`: once after an update of the shipped program, what changed since the
/// version opened before, and the feedback sent from this computer that the update addressed. Returns whether
/// the window opened. `packaged` is false for a program run from source.
pub fn whats_new_on_start(cx: &mut AppContext, first_run: bool, packaged: bool) -> bool {
    if !packaged {
        return false;
    }
    let Some(mut prefs) = preferences(cx) else { return false };
    let root = notes_root();
    let previous = prefs.values.get(whats_new::KEY).and_then(Pref::as_str).map(str::to_owned);
    let entries = whats_new::pending(&prefs, first_run, APP_VERSION, None, &root);
    let refs = if first_run { Vec::new() } else { whats_new::shipped(&prefs, previous.as_deref(), APP_VERSION, &root) };
    whats_new::remember(&mut prefs, APP_VERSION);
    whats_new::thank(&mut prefs, &refs);
    if entries.is_empty() && refs.is_empty() {
        return false;
    }
    cx.open_dialog(Box::new(WhatsNew::new(entries, refs)));
    true
}

fn show_whats_new(cx: &mut AppContext, _: &Invocation) {
    cx.open_dialog(Box::new(WhatsNew::this_version()));
}

fn show_about(cx: &mut AppContext, _: &Invocation) {
    cx.open_dialog(Box::new(about::about_box()));
}

/// The addresses and files opened while launching is disabled (tests).
pub static LAUNCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Opens a web address or a file in the system's own program (`QDesktopServices.openUrl`).
pub fn launch(target: &str) {
    if super::library_column::LAUNCH_DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
        LAUNCHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(target.to_owned());
        return;
    }
    if crate::network::offline() && target.contains("://") {
        return;
    }
    let result = if cfg!(windows) {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", target]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(target).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(target).spawn()
    };
    if let Err(error) = result {
        log::warn!("cannot open {target}: {error}");
    }
}

fn learn(_: &mut AppContext, _: &Invocation) {
    launch(LEARN_URL);
}

/// Where `show_help` looks for `README.html`: next to the program, and up to four folders above it.
pub fn help_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(mut folder) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        for _ in 0..5 {
            roots.push(folder.clone());
            if !folder.pop() {
                break;
            }
        }
    }
    roots
}

/// Port of `show_help`: `README.html` (the oracle opens the file next to its package) found in `roots`; the
/// tutorial page when the program has none.
pub fn help_target(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|root| root.join("README.html"))
        .find(|path| path.is_file())
        .map_or_else(|| LEARN_URL.to_owned(), |path| path.to_string_lossy().into_owned())
}

fn help(_: &mut AppContext, _: &Invocation) {
    launch(&help_target(&help_roots()));
}

// ---- updates ------------------------------------------------------------------------------------------------------

/// How the program reaches an address for reading (the HTTP request with the oracle's headers and timeout):
/// `crate::network::http_opener` when the network is on.
pub type UpdateOpener = Arc<dyn Fn(&str) -> std::io::Result<Box<dyn Read>> + Send + Sync>;

/// Starts the installation of a release ('지금 업데이트' of the offer); the update package sets it.
pub type Installer = fn(&mut AppContext, updater::Release);

/// The update controller's state (`UpdateController`).
#[derive(Default)]
pub struct UpdateState {
    /// The newer release a check found (`self.release`), shown by the header's update button.
    pub release: Option<updater::Release>,
    /// The way to read the release feed; without one a check of the network feed fails.
    pub opener: Option<UpdateOpener>,
    /// A feed address or local file instead of `GRAINY_UPDATE_FEED` and the project's feed (tests).
    pub feed: Option<String>,
    /// Overrides [`packaged`] (tests).
    pub packaged: Option<bool>,
    /// Installs a release.
    pub installer: Option<Installer>,
    checking: bool,
}

/// `preferences.values.get('update_check') is True`.
pub fn update_check_enabled(cx: &AppContext) -> bool {
    preferences(cx).is_some_and(|p| matches!(p.values.get("update_check"), Some(Pref::Bool(true))))
}

fn update_auto(cx: &mut AppContext, _: &Invocation) {
    let value = !update_check_enabled(cx);
    if let Some(mut prefs) = preferences(cx) {
        // `save` ignores an `OSError`, as the controller does.
        if let Err(error) = prefs.save("update_check", Pref::Bool(value)) {
            log::warn!("update_check not saved: {error}");
        }
    }
}

fn update_check(cx: &mut AppContext, _: &Invocation) {
    check_for_update(cx, true);
}

/// Port of `UpdateController.check`: asks for the newest release on a job; `manual` is the menu command (it
/// says so on the status line and reports every result in a box).
pub fn check_for_update(cx: &mut AppContext, manual: bool) {
    let state = cx.state::<UpdateState>();
    if state.checking {
        return;
    }
    state.checking = true;
    let opener = state.opener.clone();
    let feed = state.feed.clone().or_else(|| std::env::var("GRAINY_UPDATE_FEED").ok());
    if manual {
        cx.set_status(tr("새 버전을 확인하는 중…"));
    }
    cx.jobs.spawn(
        "update_check",
        "",
        move |_| {
            let open = |url: &str| -> std::io::Result<Box<dyn Read>> {
                match &opener {
                    Some(opener) => opener(url),
                    None => Err(std::io::Error::other("no network connection is available")),
                }
            };
            // The Rust build's rule: only a release file marked `GrainyRS-` qualifies (`updater::RUST_MARKER`).
            updater::latest_rust(&open, feed.as_deref(), &updater::Layout::current()).map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| checked(cx, manual, result),
    );
}

/// Port of `UpdateController.checked`.
pub fn checked(cx: &mut AppContext, manual: bool, result: Result<Option<updater::Release>, String>) {
    cx.state::<UpdateState>().checking = false;
    let release = match result {
        Ok(release) => release,
        Err(error) => {
            if manual {
                cx.open_dialog(Box::new(error_box(&error)));
            }
            return;
        }
    };
    if let Some(mut prefs) = preferences(cx) {
        let seconds =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let _ = prefs.save("update_checked", Pref::Float(seconds));
    }
    match release.filter(|r| updater::newer(&r.version, RS_VERSION)) {
        Some(release) => {
            cx.state::<UpdateState>().release = Some(release.clone());
            if manual {
                offer(cx, release);
            } else {
                cx.set_status(tr_with(
                    "새 버전 {0}이 있습니다. 위쪽의 버튼을 누르면 업데이트합니다.",
                    &[&release.version],
                ));
            }
        }
        None => {
            cx.state::<UpdateState>().release = None;
            if manual {
                cx.open_dialog(Box::new(latest_box()));
            }
        }
    }
}

/// The box of a check that failed (`QMessageBox.information(.., tr('업데이트'), ..)`).
pub fn error_box(error: &str) -> MessageBox {
    let text = tr_with("새 버전을 확인하지 못했습니다 ({0}). 인터넷 연결을 확인해 주세요.", &[error]);
    MessageBox::information("update", tr("업데이트"), text)
}

/// The box of a check that found nothing newer.
pub fn latest_box() -> MessageBox {
    MessageBox::information("update", tr("업데이트"), tr_with("최신 버전을 쓰고 있습니다 ({0}).", &[RS_VERSION]))
}

/// Port of `UpdateController.offer`: the box of a newer release. `packaged` says whether the program can
/// update itself; `installer` is what 지금 업데이트 runs (the button is left out without one).
pub fn offer_box(release: &updater::Release, packaged: bool, installer: Option<Installer>) -> MessageBox {
    let layout = updater::Layout::current();
    let executable = std::env::current_exe().unwrap_or_default();
    let app = if packaged { updater::app_folder(&executable, &layout) } else { None };
    let reason = updater::blocked(app.as_deref(), release.size, &layout, &updater::free_space);
    let mut message = MessageBox::information(
        "update_offer",
        tr("업데이트"),
        tr_with("새 버전 {0}이 있습니다. (지금 버전 {1})", &[&release.version, RS_VERSION]),
    );
    message.informative = if reason.is_empty() {
        tr("업데이트하면 Grainy가 닫혔다가 새 버전으로 다시 열립니다. 사진과 보정 기록은 그대로입니다.")
    } else {
        format!(
            "{reason}
"
        )
    };
    message.detail = release.notes.trim().chars().take(4000).collect();
    message.buttons.clear();
    if let (true, Some(install)) = (reason.is_empty(), installer) {
        let release = release.clone();
        let action: Action = Box::new(move |cx| install(cx, release));
        message.buttons.push((tr("지금 업데이트"), Some(action)));
    }
    message.buttons.push((tr("내려받기 페이지 열기"), Some(Box::new(|_| launch(updater::RS_PAGE)))));
    message.buttons.push((tr("나중에"), None));
    message
}

/// Opens the offer of `release`.
pub fn offer(cx: &mut AppContext, release: updater::Release) {
    let state = cx.state::<UpdateState>();
    let (packaged, installer) = (state.packaged.unwrap_or_else(packaged), state.installer);
    cx.open_dialog(Box::new(offer_box(&release, packaged, installer)));
}

/// Runs what the jobs that finished have to do (the window does this every frame; tests and scripts call it).
pub fn pump(cx: &mut AppContext) {
    for finished in cx.jobs.poll() {
        finished(cx);
    }
}

// ---- lights out and the screen modes ------------------------------------------------------------------------------

/// Lights out (L): only the photo is shown (`ScreenModes.lights`).
#[derive(Default)]
struct Lights(bool);

/// Whether lights out is on.
pub fn lights_on(cx: &AppContext) -> bool {
    cx.state_ref::<Lights>().is_some_and(|l| l.0)
}

/// Port of `ScreenModes.toggle_lights`. The photo's background is black while it lasts (the view's own).
pub fn toggle_lights(cx: &mut AppContext) {
    let state = cx.state::<Lights>();
    state.0 = !state.0;
}

fn lights(cx: &mut AppContext, _: &Invocation) {
    toggle_lights(cx);
}

/// Port of `ScreenModes.leave` (Esc): out of lights out, else out of full screen. False when it was neither, so
/// Esc does what it does elsewhere.
pub fn leave(cx: &mut AppContext) -> bool {
    if lights_on(cx) {
        toggle_lights(cx);
    } else if cx.full_screen {
        cx.run("view.full_screen");
    } else {
        return false;
    }
    true
}

/// What the window shows (`ScreenModes.apply`): the visible parts for the modes and the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible {
    pub library_column: bool,
    pub edit_column: bool,
    pub header_bar: bool,
    pub view_bar: bool,
    pub filmstrip: bool,
    pub library_toolbar: bool,
    pub compare_bar: bool,
    pub menu: bool,
    pub status: bool,
}

/// Port of `ScreenModes.apply`'s visibility rules for the modes `sides` and `bars` (Tab, Shift+Tab: the columns
/// and the bars *hidden* when true in the oracle; here `sides_shown` and `bars_shown` are what the context holds),
/// lights out, the screen (`develop`) and whether the comparison is on.
pub fn visible(sides_shown: bool, bars_shown: bool, lights: bool, develop: bool, compare: bool) -> Visible {
    let sides = sides_shown && !lights;
    let bars = bars_shown && !lights;
    Visible {
        library_column: sides,
        edit_column: sides,
        header_bar: bars,
        view_bar: bars,
        filmstrip: bars && develop,
        library_toolbar: bars && !develop,
        compare_bar: bars && compare,
        menu: !lights,
        status: !lights,
    }
}

/// What the window shows now.
pub fn visible_now(cx: &AppContext) -> Visible {
    let compare = cx.state_ref::<crate::views::compare::Compare>().is_some_and(|c| c.on);
    visible(cx.sides, cx.bars, lights_on(cx), cx.mode == crate::context::Mode::Develop, compare)
}

/// At the start of a frame: while the lights are out the columns and the bars are drawn as hidden. Returns what
/// to put back with [`end_frame`] (the keys already ran, nothing changes them while the menus are hidden).
pub fn begin_frame(cx: &mut AppContext) -> Option<(bool, bool)> {
    if !lights_on(cx) {
        return None;
    }
    let saved = (cx.sides, cx.bars);
    cx.sides = false;
    cx.bars = false;
    Some(saved)
}

/// The end of a frame drawn by [`begin_frame`].
pub fn end_frame(cx: &mut AppContext, saved: Option<(bool, bool)>) {
    if let Some((sides, bars)) = saved {
        cx.sides = sides;
        cx.bars = bars;
    }
}

// ---- the DRT's advanced switch -------------------------------------------------------------------------------------

/// The library preference `drt_advanced` (`toggle_drt_advanced`: `catalog.save_preference`; read when the window
/// is built: `catalog.preference('drt_advanced', False)`).
pub const DRT_ADVANCED: &str = "drt_advanced";

#[derive(Default)]
struct DrtState {
    /// The library the switch was read for.
    key: Option<String>,
    /// The value last read or written.
    value: bool,
}

fn drt_key(cx: &AppContext) -> Option<String> {
    match cx.doc.kind() {
        crate::document::Kind::Session => None,
        crate::document::Kind::Catalog { folder, read_only } => Some(format!("{}|{read_only}", folder.display())),
    }
}

/// Reads the switch of a newly opened library and stores a change of it.
fn drt_sync(cx: &mut AppContext) {
    let key = drt_key(cx);
    if cx.state::<DrtState>().key != key {
        let value = cx.doc.preference(DRT_ADVANCED).and_then(|v| v.as_bool()).unwrap_or(false);
        let state = cx.state::<DrtState>();
        state.key = key;
        state.value = value;
        cx.drt_advanced = value;
        return;
    }
    if key.is_none() || cx.state::<DrtState>().value == cx.drt_advanced {
        return;
    }
    let value = cx.drt_advanced;
    match cx.doc.save_preference(DRT_ADVANCED, &Value::Bool(value)) {
        Ok(()) => cx.state::<DrtState>().value = value,
        Err(error) => log::warn!("drt_advanced not saved: {error}"),
    }
}

// ---- the empty view -------------------------------------------------------------------------------------------------

/// What the view shows with no photo (`PhotoView.paintEvent`): the small spaced line, the headline and the
/// hint, centred in `area` at the oracle's heights.
pub(crate) fn paint_empty(painter: &eframe::egui::Painter, area: eframe::egui::Rect) {
    use eframe::egui::{text::LayoutJob, Align2, Color32, Pos2, TextFormat};
    let centre = area.center();
    let line = |text: &str, font: eframe::egui::FontId, color: Color32, spacing: f32, offset: f32| {
        let mut job = LayoutJob::default();
        job.append(text, 0.0, TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
        let galley = painter.layout_job(job);
        let size = galley.size();
        let rect = Align2::CENTER_CENTER.anchor_size(Pos2::new(centre.x, centre.y + offset), size);
        painter.galley(rect.min, galley, color);
    };
    line("ROOM FOR YOUR PHOTOGRAPHS", crate::theme::font(13.0), Color32::from_rgb(0xdf, 0xc2, 0x9b), 5.0, -63.0);
    line(&tr("사진 한 장에서 시작하세요"), crate::theme::font(27.0), Color32::from_rgb(0xf0, 0xeb, 0xe4), 0.0, -7.0);
    line(
        &tr("사진이나 폴더를 끌어 놓거나, 사진 가져오기를 누르세요."),
        crate::theme::font(13.0),
        Color32::from_rgb(0x8c, 0x93, 0x9b),
        0.0,
        43.0,
    );
}
