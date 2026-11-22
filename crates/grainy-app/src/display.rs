//! Display colour management and the photo background (APP_PLAN package 23): the monitor profile the preview, the
//! thumbnails and the compare view are shown in, the status button 색상 · 자동/수동/해제/확인 필요, and 사진 배경.
//!
//! Ports luma/display_color.py (`DisplayTarget`: the state of one window's monitor, resolved on a job, polled every
//! 15 s, `revision` counting profile changes; `DisplayColor`: the saved choice `display_color` and the legacy
//! `monitor_icc`, `configure`, the button text and tooltip), the thumbnail conversion of luma/photo_browser.py
//! `PhotoModel.thumbnail` and luma/app.py `set_view_background` / `_build_bg_context_menu` with the preference
//! `view_background`. The profile itself comes from `grainy_cms::display::resolve`; the operating-system query is
//! behind [`SystemProfiles`] ([`OsProfiles`] is the real one, tests inject fixed profiles).
//!
//! The state lives in the context (`cx.state::<DisplayState>()`); [`poll`] runs once per frame (one hook line in
//! `AppContext::poll_background`). A new profile is handed to the renderer with `Renderer::set_monitor` and
//! [`revision`] goes into the preview's and the thumbnails' keys, so they are made again.
//!
//! The monitor the window is on (`DisplayTarget.screen_name` and Qt's `screenChanged`): [`watch_window`] runs before
//! every frame (one hook line in `GrainyApp`'s `eframe::App::ui`); when the window moved, changed size or scale, or
//! [`SCREEN_CHECK`] passed, it asks grainy-platform for the name of the window's monitor, and a different name asks
//! for the profile again. Windows: the device name of the monitor of the window handle (a cheap system call on the
//! handle, which egui hands over without `unsafe`). macOS: `'Name (id)'` of the CoreGraphics display under the
//! window, found from the window's place and its monitor's size (egui's points times the zoom factor are the
//! system's points, in CoreGraphics' global display space), as the oracle names a macOS screen for
//! `mac_profile_data`. The profile query itself runs on the job, as before.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::Color32;
use grainy_cms::display::{resolve, DisplayProfile, SystemProfiles};
use grainy_cms::Profile;
use serde_json::{json, Value};

use crate::context::AppContext;
use crate::document::Kind;
use crate::render::Preview;
use crate::text::tr;

/// The three modes of the choice (`DisplayColor.mode`).
pub const MODES: [&str; 3] = ["auto", "manual", "off"];

/// The library preference of the choice (`{'mode', 'path'}`).
pub const DISPLAY_COLOR: &str = "display_color";
/// The legacy preference (the manual profile's path; read only when `display_color` is missing).
pub const MONITOR_ICC: &str = "monitor_icc";
/// The library preference of the photo background.
pub const VIEW_BACKGROUND: &str = "view_background";

/// `DisplayTarget.poll`: how often the profile is asked for again.
pub const POLL: Duration = Duration::from_secs(15);

/// The photo backgrounds of the 사진 배경 menu: (Korean label, key, colour) (`BACKGROUNDS` of luma/app.py).
pub const BACKGROUNDS: [(&str, &str, Color32); 4] = [
    ("검정", "black", Color32::from_rgb(0x10, 0x12, 0x15)),
    ("어두운 회색", "dark_grey", Color32::from_rgb(0x2b, 0x2e, 0x33)),
    ("중간 회색", "mid_grey", Color32::from_rgb(0x80, 0x80, 0x80)),
    ("흰색", "white", Color32::from_rgb(0xff, 0xff, 0xff)),
];

/// What `BACKGROUNDS.get(key, '#101215')` falls back to.
const UNKNOWN_BACKGROUND: Color32 = Color32::from_rgb(0x10, 0x12, 0x15);
/// Lights out (`toggle_lights` paints the views `#000000`): the window's own black (`#101215`) is kept, as the
/// committed snapshot of lights out (`settings-lights-*`) shows it; one constant to change, with that snapshot.
const LIGHTS_OUT: Color32 = UNKNOWN_BACKGROUND;

/// The shared system lookups.
pub type System = Arc<dyn SystemProfiles + Send + Sync>;

/// The real system lookups: on Windows `GetICMProfileW` of the screen's display context
/// (`grainy_platform::monitor::windows_profile_path`, the oracle's `windows_profile_path`); on macOS the ICC data
/// ColorSync gives the display (`grainy_platform::monitor::mac_profile_data`: `CGDisplayCopyColorSpace`,
/// `CGColorSpaceCopyICCData`); elsewhere the system default (sRGB). A manual profile works everywhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsProfiles;

impl SystemProfiles for OsProfiles {
    fn is_mac(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn windows_profile_path(&self, screen: &str) -> Result<Option<String>, String> {
        grainy_platform::monitor::windows_profile_path(screen)
    }

    fn mac_profile_data(&self, screen: &str) -> Result<Option<Vec<u8>>, String> {
        grainy_platform::monitor::mac_profile_data(screen)
    }
}

/// The window's display state: the choice, the resolved monitor, the background.
pub struct DisplayState {
    system: System,
    /// The screen's name (`DisplayTarget.screen_name`: empty where the toolkit gives none, as `offscreen`).
    screen: String,
    /// `DisplayColor.mode`: `auto`, `manual` or `off`.
    pub mode: String,
    /// `DisplayColor.path`.
    pub path: String,
    /// The library the choice was read for (`None`: not read yet).
    loaded: Option<Option<String>>,
    /// `DisplayTarget.state`.
    state: DisplayProfile,
    /// `DisplayTarget.revision`: counts changes of the profile's digest.
    revision: u64,
    /// `DisplayTarget.generation`: a result of an older generation is dropped.
    generation: u64,
    running: bool,
    /// A refresh is wanted (`schedule`, the poll).
    wanted: bool,
    next_poll: Option<Instant>,
    /// How many times the state changed (`DisplayTarget.changed` emitted).
    changes: u64,
    /// How many times the choice was configured (`configurationChanged`).
    configurations: u64,
    /// A profile file chosen in a file dialog for the open display dialog.
    pub(crate) chosen: Option<std::path::PathBuf>,
    /// `view_background`: the key.
    background: String,
}

impl Default for DisplayState {
    fn default() -> Self {
        Self {
            system: Arc::new(OsProfiles),
            screen: String::new(),
            mode: "auto".into(),
            path: String::new(),
            loaded: None,
            state: DisplayProfile::default(),
            revision: 0,
            generation: 0,
            running: false,
            wanted: false,
            next_poll: None,
            changes: 0,
            configurations: 0,
            chosen: None,
            background: "black".into(),
        }
    }
}

impl DisplayState {
    /// `DisplayTarget.state`: what the monitor resolved to.
    pub fn target(&self) -> &DisplayProfile {
        &self.state
    }

    /// `DisplayTarget.revision`.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many times the state changed.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// How many times the choice was configured.
    pub fn configurations(&self) -> u64 {
        self.configurations
    }

    /// Whether a display profile check is queued or running.
    pub(crate) fn busy(&self) -> bool {
        self.running || self.wanted
    }

    /// `DisplayTarget.schedule`.
    fn schedule(&mut self) {
        self.generation += 1;
        self.wanted = true;
    }

    /// The button text (`DisplayColor.__init__` before the first answer, `main_changed` after it):
    /// 색상 · 자동 / 수동 / 해제, 확인 필요 for an error.
    pub fn button_text(&self) -> String {
        if self.changes == 0 {
            return match self.mode.as_str() {
                "auto" => tr("색상 · 자동"),
                "manual" => tr("색상 · 수동"),
                _ => tr("색상 · 해제"),
            };
        }
        let label = if self.state.error {
            "확인 필요"
        } else {
            match self.state.mode.as_str() {
                "auto" => "자동",
                "manual" => "수동",
                _ => "해제",
            }
        };
        tr("색상 · ") + &tr(label)
    }

    /// The button's tooltip: screen, description, path and message, the empty ones left out.
    pub fn tooltip(&self) -> String {
        if self.changes == 0 {
            // Set by `main_changed`: the first answer.
            return String::new();
        }
        let s = &self.state;
        [s.screen.as_str(), s.description.as_str(), s.path.as_str(), s.message.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The text of the dialog's status label (`DisplayDialog.update_status`).
    pub fn status_text(&self) -> String {
        let s = &self.state;
        let screen = if s.screen.is_empty() { "기본 화면" } else { s.screen.as_str() };
        [format!("모니터: {screen}"), format!("적용 프로파일: {}", s.description), s.path.clone(), s.message.clone()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `display_profiles.SYSTEM`.
    pub fn system_name(&self) -> &'static str {
        if self.system.is_mac() {
            "macOS"
        } else {
            "Windows"
        }
    }

    /// The photo background's key (`view_background`).
    pub fn background_key(&self) -> &str {
        &self.background
    }

    /// Replaces the system lookups (tests), and asks for the profile again.
    pub fn set_system(&mut self, system: System) {
        self.system = system;
        self.schedule();
    }

    /// Sets the screen name the profile is asked for (`DisplayTarget.screen_name`).
    pub fn set_screen(&mut self, screen: &str) {
        self.screen = screen.to_owned();
        self.schedule();
    }

    /// The screen name the profile is asked for.
    pub fn screen(&self) -> &str {
        &self.screen
    }

    /// The window's monitor as just looked up (`screenChanged`): a different name is kept and the profile asked for
    /// again; the same name changes nothing. Returns whether it changed.
    pub fn screen_seen(&mut self, screen: &str) -> bool {
        if self.screen == screen {
            return false;
        }
        self.set_screen(screen);
        true
    }

    /// Asks for the profile again (`DisplayTarget.schedule`: the dialog's 다시 확인, a changed screen).
    pub fn ask_again(&mut self) {
        self.schedule();
    }
}

// ---- the monitor the window is on -------------------------------------------------------------------------------------

/// How often the window's monitor is looked up again while the window stays where it is (a monitor arrangement
/// changed in the system's settings moves no window).
pub const SCREEN_CHECK: Duration = Duration::from_secs(2);

/// When to look up the window's monitor: the window's place (outer rectangle, the monitor's size and the scale, as
/// raw bits) changed, or [`SCREEN_CHECK`] passed since the last look.
#[derive(Debug, Default, Clone)]
pub struct ScreenWatch {
    place: Option<[u32; 7]>,
    checked: Option<Instant>,
}

impl ScreenWatch {
    /// Whether to look now, for the window at `place` at `now`; a yes is remembered as the last look.
    pub fn due(&mut self, place: [u32; 7], now: Instant) -> bool {
        let moved = self.place != Some(place);
        let stale = self.checked.is_none_or(|t| now.saturating_duration_since(t) >= SCREEN_CHECK);
        if moved || stale {
            self.place = Some(place);
            self.checked = Some(now);
        }
        moved || stale
    }
}

/// Where the window is, as raw bits: the outer rectangle, the monitor's size and the points-per-pixel.
fn window_place(ctx: &eframe::egui::Context) -> [u32; 7] {
    ctx.input(|i| {
        let viewport = i.viewport();
        let rect = viewport.outer_rect.unwrap_or(eframe::egui::Rect::NOTHING);
        let monitor = viewport.monitor_size.unwrap_or_default();
        [rect.min.x, rect.min.y, rect.max.x, rect.max.y, monitor.x, monitor.y, i.pixels_per_point].map(f32::to_bits)
    })
}

/// The window toolkit's native window, as far as the monitor lookup needs it.
enum Native {
    /// A Windows window: its `HWND` as a number.
    Windows(isize),
    /// A macOS window: its `NSView*` pointer (monitor is looked up by its place).
    Mac(isize),
}

/// The window's native window, `None` where the toolkit gives none (tests) or on other systems.
fn native_window(frame: &eframe::Frame) -> Option<Native> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(Native::Windows(handle.hwnd.get())),
        RawWindowHandle::AppKit(handle) => Some(Native::Mac(handle.ns_view.as_ptr() as isize)),
        _ => None,
    }
}

/// The window's outer rectangle `[left, top, right, bottom]` and its monitor's size in the system's points: egui's
/// points times the zoom factor (egui divides the system's pixels by the scale and the zoom).
pub fn system_place(ctx: &eframe::egui::Context) -> Option<([f64; 4], Option<[f64; 2]>)> {
    let zoom = f64::from(ctx.zoom_factor());
    ctx.input(|i| {
        let viewport = i.viewport();
        let rect = viewport.outer_rect?;
        let window = [rect.min.x, rect.min.y, rect.max.x, rect.max.y].map(|v| f64::from(v) * zoom);
        let monitor = viewport.monitor_size.map(|size| [f64::from(size.x) * zoom, f64::from(size.y) * zoom]);
        Some((window, monitor))
    })
}

/// Before every frame: the monitor the window is on, looked up when the window moved (or every [`SCREEN_CHECK`]);
/// a different monitor asks for its profile (`DisplayTarget.bind` / `screenChanged` -> `schedule`). Without a
/// native window (tests, other systems) the screen keeps its name (empty: the system default).
pub fn watch_window(cx: &mut AppContext, frame: &eframe::Frame) {
    let place = window_place(&cx.egui);
    if !cx.state::<ScreenWatch>().due(place, Instant::now()) {
        return;
    }
    let screen = match native_window(frame) {
        Some(Native::Windows(handle)) => {
            #[cfg(windows)]
            crate::native_hdr::bind(cx, handle);
            grainy_platform::monitor::screen_of_window(handle).unwrap_or_default()
        }
        Some(Native::Mac(_handle)) => {
            #[cfg(target_os = "macos")]
            crate::native_hdr::bind(cx, _handle);
            match system_place(&cx.egui) {
                Some((window, monitor)) => {
                    grainy_platform::monitor::mac_screen_of_window(window, monitor).unwrap_or_default()
                }
                // Not placed yet (or minimised): asked again when the window moves.
                None => return,
            }
        }
        None => return,
    };
    cx.state::<DisplayState>().screen_seen(&screen);
}

fn library_key(cx: &AppContext) -> Option<String> {
    match cx.doc.kind() {
        Kind::Session => None,
        Kind::Catalog { folder, read_only } => Some(format!("{}|{read_only}", folder.display())),
    }
}

/// `DisplayColor.__init__`'s choice from the library: `display_color`, else the legacy `monitor_icc` (a manual
/// path; an empty string was "off"; unset is auto: the preference store cannot tell a stored `null`, also "off",
/// from an unset one).
pub fn stored_choice(saved: Option<Value>, legacy: Option<Value>) -> (String, String) {
    let saved = saved.filter(|v| v.as_object().is_some_and(|o| !o.is_empty()));
    let (mode, path) = match saved {
        Some(saved) => (
            saved.get("mode").and_then(Value::as_str).unwrap_or("auto").to_owned(),
            saved.get("path").and_then(Value::as_str).unwrap_or("").to_owned(),
        ),
        None => match legacy.as_ref().and_then(Value::as_str) {
            None => ("auto".to_owned(), String::new()),
            Some("") => ("off".to_owned(), String::new()),
            Some(path) => ("manual".to_owned(), path.to_owned()),
        },
    };
    let mode = if MODES.contains(&mode.as_str()) { mode } else { "auto".to_owned() };
    (mode, path)
}

/// Reads the choice and the background of a newly opened library.
fn load(cx: &mut AppContext) {
    let key = library_key(cx);
    if cx.state::<DisplayState>().loaded.as_ref() == Some(&key) {
        return;
    }
    let (mode, path) = stored_choice(cx.doc.preference(DISPLAY_COLOR), cx.doc.preference(MONITOR_ICC));
    let background = cx.doc.preference(VIEW_BACKGROUND).and_then(|v| v.as_str().map(str::to_owned));
    let state = cx.state::<DisplayState>();
    state.loaded = Some(key);
    state.mode = mode;
    state.path = path;
    state.background = background.unwrap_or_else(|| "black".into());
    state.schedule();
}

/// Once per frame (a hook in `AppContext::poll_background`): reads the choice of a new library, asks for the
/// profile when it is wanted or the poll is due, and applies what the job found.
pub fn poll(cx: &mut AppContext) {
    load(cx);
    let now = Instant::now();
    let state = cx.state::<DisplayState>();
    let due = *state.next_poll.get_or_insert(now + POLL);
    if now >= due {
        state.next_poll = Some(now + POLL);
        state.wanted = true;
    }
    let again = state.next_poll.map_or(POLL, |t| t.saturating_duration_since(now));
    let start = state.wanted && !state.running;
    cx.egui.request_repaint_after(again);
    if start {
        refresh(cx);
    }
}

/// `DisplayTarget.refresh`: resolves the profile on a job.
fn refresh(cx: &mut AppContext) {
    let state = cx.state::<DisplayState>();
    state.wanted = false;
    state.running = true;
    let generation = state.generation;
    let (mode, path, screen, system) =
        (state.mode.clone(), state.path.clone(), state.screen.clone(), state.system.clone());
    cx.jobs.spawn(
        "display",
        "",
        move |_| {
            let (mode, screen, path) = (mode, screen, path);
            Ok(resolve(&mode, &screen, &path, system.as_ref()))
        },
        move |cx: &mut AppContext, result| {
            let resolved =
                result.unwrap_or_else(|message| DisplayProfile { error: true, message, ..DisplayProfile::default() });
            ready(cx, generation, resolved);
        },
    );
}

/// `DisplayTarget.refresh.ready`: a result that still belongs to the current generation replaces the state; a
/// changed digest counts a new revision and hands the profile to the renderer.
fn ready(cx: &mut AppContext, generation: u64, resolved: DisplayProfile) {
    let state = cx.state::<DisplayState>();
    state.running = false;
    if generation != state.generation {
        // Asked again while this ran: `schedule` already wants the next run.
        return;
    }
    let digest_changed = resolved.digest != state.state.digest;
    let different = resolved != state.state;
    state.state = resolved;
    if digest_changed {
        state.revision += 1;
    }
    if different {
        state.changes += 1;
    }
    if digest_changed {
        let profile = state.state.data.clone();
        cx.renderer.set_monitor(profile);
        reload_compare(cx);
    }
}

/// The compare view's second picture is made again in the new colours (`refresh_view`).
fn reload_compare(cx: &mut AppContext) {
    if let Some(compare) = cx.state_ref::<crate::views::compare::Compare>() {
        if compare.on {
            cx.state::<crate::views::compare::Compare>().shown = None;
        }
    }
}

/// The monitor profile of the window (`display_color.main.profile`), `None` for sRGB.
pub fn monitor(cx: &AppContext) -> Option<Profile> {
    cx.state_ref::<DisplayState>().and_then(|s| s.state.data.clone())
}

/// `display_color.main.revision`: part of the preview's and the thumbnails' keys.
pub fn revision(cx: &AppContext) -> u64 {
    cx.state_ref::<DisplayState>().map_or(0, |s| s.revision)
}

/// Runs the jobs that finished and the refresh they asked for until the profile is settled (tests and scripts;
/// the window does this every frame).
pub fn settle(cx: &mut AppContext) {
    let started = Instant::now();
    loop {
        poll(cx);
        for finished in cx.jobs.poll() {
            finished(cx);
        }
        let state = cx.state::<DisplayState>();
        if !state.running && !state.wanted {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the display profile did not resolve");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// `DisplayColor.configure`: the choice, saved as `display_color` (and `monitor_icc`), and the profile asked for.
pub fn configure(cx: &mut AppContext, mode: &str, path: &str) -> Result<(), String> {
    if !MODES.contains(&mode) {
        return Err("지원하지 않는 화면 색상 모드입니다.".into());
    }
    load(cx);
    let state = cx.state::<DisplayState>();
    state.mode = mode.to_owned();
    state.path = path.to_owned();
    state.configurations += 1;
    state.schedule();
    let legacy = if mode == "manual" { json!(path) } else { Value::Null };
    for (key, value) in [(DISPLAY_COLOR, json!({"mode": mode, "path": path})), (MONITOR_ICC, legacy)] {
        if let Err(error) = cx.doc.save_preference(key, &value) {
            log::warn!("{key} not saved: {error}");
        }
    }
    Ok(())
}

/// Port of the status bar's colour button, drawn in `app.rs`: runs the dialog.
pub fn show_dialog(cx: &mut AppContext) {
    let dialog = crate::dialogs::display::DisplayDialog::new(cx);
    cx.open_dialog(Box::new(dialog));
    cx.state::<DisplayState>().ask_again();
}

// ---- the thumbnails -------------------------------------------------------------------------------------------------

/// Port of the conversion in `PhotoModel.thumbnail`: the 8-bit sRGB thumbnail through the monitor profile
/// (`convert(pixels / 255, sRGB, monitor)`, then `qimage`'s clip and rounding).
pub fn monitor_preview(preview: Preview, profile: &Profile) -> Result<Preview, String> {
    let (width, height) = (preview.width, preview.height);
    let values: Vec<f32> =
        preview.rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).map(|v| f32::from(v) / 255.0).collect();
    let image = grainy_core::Image::from_shape_vec((height, width, 3), values).map_err(|e| e.to_string())?;
    let converted = grainy_cms::display_rgb(&image, Some(profile)).map_err(|e| e.to_string())?;
    let mut rgba = vec![255u8; width * height * 4];
    let converted = converted.as_standard_layout();
    let values = converted.as_slice().unwrap_or(&[]).as_chunks::<3>().0;
    for (out, rgb) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        for c in 0..3 {
            out[c] = crate::histogram::to_byte(rgb[c]);
        }
    }
    Ok(Preview { width, height, rgba })
}

// ---- the photo background -------------------------------------------------------------------------------------------

/// `BACKGROUNDS.get(key, '#101215')`.
pub fn colour_of(key: &str) -> Color32 {
    BACKGROUNDS.iter().find(|(_, k, _)| *k == key).map_or(UNKNOWN_BACKGROUND, |(_, _, colour)| *colour)
}

/// The colour behind the photo: the chosen background, black while the lights are out.
pub fn background(cx: &AppContext) -> Color32 {
    if crate::commands::settings::lights_on(cx) {
        return LIGHTS_OUT;
    }
    cx.state_ref::<DisplayState>().map_or(UNKNOWN_BACKGROUND, |s| colour_of(&s.background))
}

/// The chosen background's key.
pub fn background_key(cx: &AppContext) -> String {
    cx.state_ref::<DisplayState>().map_or_else(|| "black".to_owned(), |s| s.background.clone())
}

/// Port of `MainWindow.set_view_background`: the background, saved when it differs from the stored one.
pub fn set_background(cx: &mut AppContext, key: &str) {
    load(cx);
    cx.state::<DisplayState>().background = key.to_owned();
    let stored = cx.doc.preference(VIEW_BACKGROUND).and_then(|v| v.as_str().map(str::to_owned));
    if stored.as_deref().unwrap_or("black") != key {
        if let Err(error) = cx.doc.save_preference(VIEW_BACKGROUND, &json!(key)) {
            log::warn!("view_background not saved: {error}");
        }
    }
}

/// Port of `_build_bg_context_menu`: the four checkable items (the right-click menu of the view and the 사진 배경
/// submenu); returns the key that was chosen.
pub fn background_menu(ui: &mut eframe::egui::Ui, cx: &AppContext) -> Option<&'static str> {
    let current = background_key(cx);
    let mut chosen = None;
    for (label, key, _) in BACKGROUNDS {
        if ui.add(eframe::egui::Button::new(tr(label)).selected(*key == *current)).clicked() {
            chosen = Some(key);
            ui.close();
        }
    }
    chosen
}

/// The display revision as a number mixed into a thumbnail's key (0 until the monitor profile changes, so the keys
/// stay what they were without one).
pub fn thumb_salt(cx: &AppContext) -> u64 {
    revision(cx).wrapping_mul(0x517c_c1b7_2722_0a95)
}

/// The right-click menu of the photo view (`_view_background_context_menu`).
pub fn background_context_menu(ui: &mut eframe::egui::Ui, cx: &mut AppContext) {
    if let Some(key) = background_menu(ui, cx) {
        set_background(cx, key);
    }
}

/// The status bar's colour button (`DisplayColor.button`): its text follows the monitor's state, the tooltip names
/// the screen, the profile, its file and the message; a click opens the display box.
pub fn status_button(ui: &mut eframe::egui::Ui, cx: &mut AppContext) {
    let (text, tooltip) = {
        let state = cx.state::<DisplayState>();
        (state.button_text(), state.tooltip())
    };
    let mut response = crate::widgets::button(ui, &text);
    if !tooltip.is_empty() {
        response = response.on_hover_text(tooltip);
    }
    if response.clicked() {
        cx.run("display.dialog");
    }
}
