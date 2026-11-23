//! The window's size, position and layout, kept in the Rust app's own settings file (`settings.json` in the data
//! folder, key `window`; never the Python Grainy's file).
//!
//! The Python Grainy keeps the geometry only while it rebuilds the window for a new language
//! (`DesktopSession.change_language`: `saveGeometry`, `restoreGeometry`); this app also remembers it between runs:
//! the size and position of the normal window, whether it was maximized or full screen, the screen modes of
//! `ScreenModes` (the side columns and the bars: Tab, Shift+Tab) and the widths of the two columns (the oracle's
//! `split.setSizes([270,910,330])` and the widths `ScreenModes.apply` puts back).
//!
//! [`poll`] runs once per frame (one hook line in `AppContext::poll_background`): it compares the window with the
//! last stored value and writes the file once the value has stayed the same for [`DELAY`] (a drag is not written
//! at every frame). [`flush`] writes at once (on exit). [`restore`] reads the file at start, and [`viewport`]
//! gives `main` the size, position and mode to open the window with. The lights-out mode is not stored: a window
//! must not open with nothing on it.

use std::time::{Duration, Instant};

use eframe::egui::{IconData, Rect, ViewportBuilder};
use serde_json::{json, Map, Value};

use crate::context::AppContext;

/// The key of the settings file.
pub const KEY: &str = "window";
/// The smallest window `main` opens (its minimum inner size).
pub const MIN_SIZE: [f32; 2] = [1100.0, 700.0];
/// The widths the column panels allow (`size_range` of the panels).
pub const LIBRARY_RANGE: (f32, f32) = (230.0, 360.0);
/// The widths the Develop column allows.
pub const DEVELOP_RANGE: (f32, f32) = (290.0, 360.0);
/// The widths the columns start with.
pub const DEFAULT_LIBRARY: f32 = 285.0;
/// See [`DEFAULT_LIBRARY`].
pub const DEFAULT_DEVELOP: f32 = 350.0;
/// How long a changed value must stay the same before it is written.
pub const DELAY: Duration = Duration::from_millis(500);

/// What is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    /// The inner size of the normal window (not maximized, not full screen), in points.
    pub size: Option<[f32; 2]>,
    /// The outer position of the normal window, in points.
    pub position: Option<[f32; 2]>,
    /// The window was maximized.
    pub maximized: bool,
    /// The window was full screen (F).
    pub full_screen: bool,
    /// The side columns were shown (Tab).
    pub sides: bool,
    /// The header and the bars were shown (Shift+Tab).
    pub bars: bool,
    /// The library column's width.
    pub library_width: Option<f32>,
    /// The Develop column's width.
    pub develop_width: Option<f32>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            size: None,
            position: None,
            maximized: false,
            full_screen: false,
            sides: true,
            bars: true,
            library_width: None,
            develop_width: None,
        }
    }
}

fn pair(value: Option<&Value>) -> Option<[f32; 2]> {
    let list = value?.as_array()?;
    let (x, y) = (list.first()?.as_f64()?, list.get(1)?.as_f64()?);
    (list.len() == 2 && x.is_finite() && y.is_finite()).then_some([x as f32, y as f32])
}

fn width(value: Option<&Value>, (low, high): (f32, f32)) -> Option<f32> {
    let w = value?.as_f64()? as f32;
    w.is_finite().then(|| w.clamp(low, high))
}

impl WindowState {
    /// The stored form (`size`, `position`, `maximized`, `full_screen`, `sides`, `bars`, the widths).
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        if let Some([w, h]) = self.size {
            map.insert("size".into(), json!([w.round() as i64, h.round() as i64]));
        }
        if let Some([x, y]) = self.position {
            map.insert("position".into(), json!([x.round() as i64, y.round() as i64]));
        }
        map.insert("maximized".into(), json!(self.maximized));
        map.insert("full_screen".into(), json!(self.full_screen));
        map.insert("sides".into(), json!(self.sides));
        map.insert("bars".into(), json!(self.bars));
        if let Some(w) = self.library_width {
            map.insert("library_width".into(), json!(w.round() as i64));
        }
        if let Some(w) = self.develop_width {
            map.insert("develop_width".into(), json!(w.round() as i64));
        }
        Value::Object(map)
    }

    /// Reads what is stored; anything damaged is left at its default and sizes and widths are brought into the
    /// ranges the window allows (a hand-edited file cannot open a window of no size or at an unreachable place).
    pub fn from_value(value: &Value) -> Self {
        let mut state = Self::default();
        let Some(map) = value.as_object() else { return state };
        state.size =
            pair(map.get("size")).map(|[w, h]| [w.clamp(MIN_SIZE[0], 16_000.0), h.clamp(MIN_SIZE[1], 16_000.0)]);
        state.position = pair(map.get("position"))
            .filter(|[x, y]| (-16_000.0..=32_000.0).contains(x) && (-16_000.0..=32_000.0).contains(y));
        let flag = |key: &str, default: bool| map.get(key).and_then(Value::as_bool).unwrap_or(default);
        state.maximized = flag("maximized", false);
        state.full_screen = flag("full_screen", false);
        state.sides = flag("sides", true);
        state.bars = flag("bars", true);
        state.library_width = width(map.get("library_width"), LIBRARY_RANGE);
        state.develop_width = width(map.get("develop_width"), DEVELOP_RANGE);
        state
    }

    /// Opens `builder` with the stored size, position and mode.
    pub fn apply(&self, mut builder: ViewportBuilder) -> ViewportBuilder {
        if let Some(size) = self.size {
            builder = builder.with_inner_size(size);
        }
        if let Some(position) = self.position {
            builder = builder.with_position(position);
        }
        builder.with_maximized(self.maximized).with_fullscreen(self.full_screen)
    }
}

/// The stored window state of the data folder (the defaults when there is none).
pub fn load(data_folder: &std::path::Path) -> Option<WindowState> {
    let preferences = grainy_platform::Preferences::new(data_folder.join("settings.json"));
    preferences.values.get(KEY).map(|v| WindowState::from_value(&crate::commands::settings::to_json(v)))
}

/// `main`'s base window builder: keeps OS-native icons (`grainy.ico` resource 1 via `winit` on Windows;
/// `GrainyRS.icns` in the `.app` bundle on macOS) by setting `IconData::default()`, so `eframe`'s
/// `AppTitleIconSetter` does not overwrite them on the first frame with `egui`'s default icon.
pub fn default_viewport() -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title("Grainy")
        .with_icon(IconData::default())
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size(MIN_SIZE)
        .with_drag_and_drop(true)
}

/// `main`'s window: `builder` with the stored size, position and mode of the data folder.
pub fn viewport(builder: ViewportBuilder, data_folder: &std::path::Path) -> ViewportBuilder {
    match load(data_folder) {
        Some(state) => state.apply(builder),
        None => builder,
    }
}

/// What the window keeps between frames.
#[derive(Default)]
pub struct Tracker {
    /// The value last written or read.
    saved: Option<Value>,
    /// A changed value and when it first showed.
    pending: Option<(Value, Instant)>,
    normal_size: Option<[f32; 2]>,
    normal_position: Option<[f32; 2]>,
    library_width: Option<f32>,
    develop_width: Option<f32>,
}

/// The library column's width to draw with (the stored one or the default).
pub fn library_width(cx: &mut AppContext) -> f32 {
    cx.state::<Tracker>().library_width.unwrap_or(DEFAULT_LIBRARY)
}

/// The Develop column's width to draw with.
pub fn develop_width(cx: &mut AppContext) -> f32 {
    cx.state::<Tracker>().develop_width.unwrap_or(DEFAULT_DEVELOP)
}

/// The widths the two panels have now (called after they are drawn).
pub fn note_widths(cx: &mut AppContext, library: Option<f32>, develop: Option<f32>) {
    let tracker = cx.state::<Tracker>();
    if let Some(w) = library {
        tracker.library_width = Some(w.clamp(LIBRARY_RANGE.0, LIBRARY_RANGE.1));
    }
    if let Some(w) = develop {
        tracker.develop_width = Some(w.clamp(DEVELOP_RANGE.0, DEVELOP_RANGE.1));
    }
}

/// Reads the stored state into the context (the screen modes, the widths) at start.
pub fn restore(cx: &mut AppContext) {
    let Some(folder) = cx.data_folder.clone() else { return };
    let Some(state) = load(&folder) else { return };
    cx.sides = state.sides;
    cx.bars = state.bars;
    cx.full_screen = state.full_screen;
    let tracker = cx.state::<Tracker>();
    tracker.saved = Some(state.to_value());
    tracker.normal_size = state.size;
    tracker.normal_position = state.position;
    tracker.library_width = state.library_width;
    tracker.develop_width = state.develop_width;
}

/// The state of the window now: the viewport's size and position while it is a normal window, the last normal
/// ones while it is maximized or full screen.
pub fn current(cx: &mut AppContext) -> WindowState {
    let (inner, outer, maximized, fullscreen) = cx.egui.input(|i| {
        let v = i.viewport();
        (v.inner_rect, v.outer_rect, v.maximized.unwrap_or(false), v.fullscreen.unwrap_or(false))
    });
    let (sides, bars, full_screen_flag) = (cx.sides, cx.bars, cx.full_screen);
    let tracker = cx.state::<Tracker>();
    let normal = !maximized && !fullscreen;
    if normal {
        let known = |r: Rect| r.is_finite() && r.width() > 0.0 && r.height() > 0.0;
        if let Some(rect) = inner.filter(|r| known(*r)) {
            tracker.normal_size = Some([rect.width(), rect.height()]);
        }
        if let Some(rect) = outer.filter(|r| known(*r)) {
            tracker.normal_position = Some([rect.min.x, rect.min.y]);
        }
    }
    WindowState {
        size: tracker.normal_size,
        position: tracker.normal_position,
        maximized,
        full_screen: fullscreen || full_screen_flag,
        sides,
        bars,
        library_width: tracker.library_width,
        develop_width: tracker.develop_width,
    }
}

fn write(cx: &mut AppContext, value: &Value) {
    let Some(mut preferences) = crate::commands::settings::preferences(cx) else { return };
    match preferences.save(KEY, crate::commands::settings::from_json(value)) {
        Ok(()) => {
            let tracker = cx.state::<Tracker>();
            tracker.saved = Some(value.clone());
            tracker.pending = None;
        }
        Err(error) => log::warn!("window state not saved: {error}"),
    }
}

/// Once per frame: writes the window state when it changed and stayed the same for [`DELAY`].
pub fn poll(cx: &mut AppContext) {
    if cx.data_folder.is_none() {
        return;
    }
    let value = current(cx).to_value();
    let tracker = cx.state::<Tracker>();
    if tracker.saved.as_ref() == Some(&value) {
        tracker.pending = None;
        return;
    }
    let now = Instant::now();
    match &tracker.pending {
        Some((pending, since)) if *pending == value => {
            if now.duration_since(*since) >= DELAY {
                write(cx, &value);
            } else {
                cx.egui.request_repaint_after(DELAY);
            }
        }
        _ => {
            tracker.pending = Some((value, now));
            cx.egui.request_repaint_after(DELAY);
        }
    }
}

/// Writes the window state now if it differs from the stored one (on exit, before the language changes).
pub fn flush(cx: &mut AppContext) {
    if cx.data_folder.is_none() {
        return;
    }
    let value = current(cx).to_value();
    if cx.state::<Tracker>().saved.as_ref() != Some(&value) {
        write(cx, &value);
    }
}

// ---- a window of any size --------------------------------------------------------------------------------------
//
// The window asks for a minimum size ([`MIN_SIZE`]), but a program can still give it any size (`SetWindowPos`,
// `MoveWindow`): 1 × 1, 0 × 0, or larger than the GPU can draw. Every layout of the window copes with the small
// sizes (tests/window_size.rs draws it down to 0 × 0). A size larger than the GPU's largest texture
// (`max_texture_dimension_2d`, 8192 to 32768 pixels) makes `wgpu` refuse to set up the window's surface and to
// draw a frame at that size, errors `wgpu` turns into a panic by default, so the release program, which has no
// console, quit without a message; and a size within that limit can still be more than the GPU's memory holds
// (30000 × 30000 pixels is 3.6 GB a picture), where `egui_wgpu` itself gives up. Now the surface's errors are only
// logged ([`guard_surface_errors`]), a surface whose setup was refused is set up again ([`on_surface_status`]), and
// a window larger than the GPU draws, or larger than all the monitors together (no one can drag a window that
// large: only a program can make it), goes back to the size of its monitor ([`keep_drawable`]).

/// Whether `error` concerns the window's surface (the picture the window shows): its setup refused
/// (`Surface::configure`: too large for the GPU or its memory, a format or mode the window no longer offers) or no
/// picture to draw into (`Surface::get_current_texture_view`: the surface is not set up). `wgpu` names the call in
/// the error's source (`In Surface::configure`).
pub fn is_surface_error(error: &eframe::wgpu::Error) -> bool {
    std::error::Error::source(error).is_some_and(|source| source.to_string().starts_with("In Surface::"))
}

/// Whether `error` ends the program: every error but a surface's ([`is_surface_error`]) and, while the window is
/// larger than the GPU or the desktop allows (`too_large`, [`drawable_size`]), the errors of drawing the frame at
/// that size.
pub fn is_fatal(error: &eframe::wgpu::Error, too_large: bool) -> bool {
    !too_large && !is_surface_error(error)
}

/// Whether the window was larger than the GPU or the desktop allows at its last frame ([`keep_drawable`]), one per
/// window.
#[derive(Clone, Default)]
struct TooLarge(std::sync::Arc<std::sync::atomic::AtomicBool>);

fn too_large_id() -> eframe::egui::Id {
    eframe::egui::Id::unique("grainy_window_too_large")
}

/// Replaces `wgpu`'s fatal handler of the errors no error scope caught on the window's device (`ctx`'s window):
/// the surface's errors, and any error while the window is larger than the GPU or the desktop allows, are logged
/// and the frame is skipped; every other error stays fatal, as before (`wgpu`'s own handler).
pub fn guard_surface_errors(ctx: &eframe::egui::Context, device: &eframe::wgpu::Device) {
    let flag = TooLarge::default();
    ctx.data_mut(|data| data.insert_temp(too_large_id(), flag.clone()));
    device.on_uncaptured_error(std::sync::Arc::new(move |error| {
        if is_fatal(&error, flag.0.load(std::sync::atomic::Ordering::Relaxed)) {
            log::error!("Handling wgpu errors as fatal by default");
            panic!("wgpu error: {error}\n");
        }
        log::warn!("the window cannot be drawn at this size: {error}");
    }));
}

/// How much larger than the desktop (the box around every monitor) the window may be: the system lets a window be
/// dragged to the desktop's size and its frame, a little more.
pub const DESKTOP_SLACK: f32 = 1.1;

/// The size, in pixels, of the box around every monitor (`position` and `size` in pixels); `None` without monitors.
pub fn desktop_size(monitors: impl IntoIterator<Item = ([i32; 2], [u32; 2])>) -> Option<eframe::egui::Vec2> {
    let mut bounds: Option<eframe::egui::Rect> = None;
    for ([x, y], [w, h]) in monitors {
        let monitor = eframe::egui::Rect::from_min_size(
            eframe::egui::pos2(x as f32, y as f32),
            eframe::egui::vec2(w as f32, h as f32),
        );
        bounds = Some(bounds.map_or(monitor, |b| b.union(monitor)));
    }
    bounds.map(|b| b.size()).filter(|s| s.x >= 1.0 && s.y >= 1.0)
}

/// The window size (in points) to go back to when the window, `size` points at `pixels_per_point`, is larger than
/// the GPU's largest texture (`max_side` pixels) or than the desktop (`desktop`, pixels, [`DESKTOP_SLACK`]) in
/// either direction: the size of its monitor (`monitor`, points) within both; `None` when it can be drawn (any
/// smaller size, 0 × 0 included).
pub fn drawable_size(
    size: eframe::egui::Vec2,
    pixels_per_point: f32,
    max_side: u32,
    desktop: Option<eframe::egui::Vec2>,
    monitor: Option<eframe::egui::Vec2>,
) -> Option<eframe::egui::Vec2> {
    use eframe::egui::Vec2;
    if !size.is_finite() || !pixels_per_point.is_finite() || pixels_per_point <= 0.0 || max_side == 0 {
        return None;
    }
    let mut largest = Vec2::splat(max_side as f32);
    if let Some(desktop) = desktop.filter(|d| d.is_finite() && d.x >= 1.0 && d.y >= 1.0) {
        largest = largest.min(desktop * DESKTOP_SLACK);
    }
    let pixels = size * pixels_per_point;
    if pixels.x <= largest.x && pixels.y <= largest.y {
        return None;
    }
    // Whole points: the system rounds points × scale to pixels, and a whole number of points within the limit
    // rounds to a whole number of pixels within it.
    let mut limit = (largest / pixels_per_point).floor();
    if let Some(monitor) = monitor.filter(|m| m.is_finite() && m.x >= 1.0 && m.y >= 1.0) {
        limit = limit.min(monitor.floor());
    }
    Some(size.min(limit))
}

/// The type of `egui_wgpu`'s answer to a frame without a picture (`WgpuConfiguration::on_surface_status`).
pub type SurfaceStatusHandler =
    std::sync::Arc<dyn Fn(&eframe::wgpu::CurrentSurfaceTexture) -> eframe::egui_wgpu::SurfaceErrorAction + Send + Sync>;

/// `egui_wgpu`'s answer to a frame without a picture, but a surface left without a setup (its last setup refused,
/// [`guard_surface_errors`]: `CurrentSurfaceTexture::Validation`) is set up again at the window's size, where
/// `egui_wgpu` only skips the frame and the window stayed dark for good.
pub fn on_surface_status() -> SurfaceStatusHandler {
    let default = eframe::egui_wgpu::WgpuConfiguration::default().on_surface_status;
    std::sync::Arc::new(move |status| match status {
        eframe::wgpu::CurrentSurfaceTexture::Validation => eframe::egui_wgpu::SurfaceErrorAction::Reconfigure,
        other => default(other),
    })
}

/// The desktop of `frame`'s window ([`desktop_size`]); `None` where the window toolkit gives no monitors (tests,
/// other systems, where the system keeps a window within the screen).
fn desktop_of(frame: &eframe::Frame) -> Option<eframe::egui::Vec2> {
    #[cfg(windows)]
    {
        let window = frame.winit_window()?;
        desktop_size(window.available_monitors().map(|monitor| {
            let (position, size) = (monitor.position(), monitor.size());
            ([position.x, position.y], [size.width, size.height])
        }))
    }
    #[cfg(not(windows))]
    {
        let _ = frame;
        None
    }
}

/// Once per frame, before the frame is drawn: asks for a smaller window when the window is larger than the GPU or
/// the desktop allows ([`drawable_size`]), and tells [`guard_surface_errors`] whether this frame is drawn at such a
/// size.
pub fn keep_drawable(ctx: &eframe::egui::Context, frame: &eframe::Frame) {
    let Some(max_side) = frame.wgpu_render_state().map(|state| state.device.limits().max_texture_dimension_2d) else {
        return;
    };
    let (size, pixels_per_point, monitor) =
        ctx.input(|i| (i.viewport_rect().size(), i.pixels_per_point, i.viewport().monitor_size));
    let drawable = drawable_size(size, pixels_per_point, max_side, desktop_of(frame), monitor);
    if let Some(flag) = ctx.data(|data| data.get_temp::<TooLarge>(too_large_id())) {
        flag.0.store(drawable.is_some(), std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(size) = drawable {
        log::warn!(
            "the window is larger than the GPU or the desktop allows ({max_side} pixels); asking for {size:?} points"
        );
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::InnerSize(size));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Source(&'static str);

    impl std::fmt::Display for Source {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Source {}

    fn validation(source: &'static str) -> eframe::wgpu::Error {
        eframe::wgpu::Error::Validation { source: Box::new(Source(source)), description: format!("{source}\n") }
    }

    #[test]
    fn only_the_window_surface_errors_are_survived() {
        // The source `wgpu` gives (wgpu-core's `ContextError`), as the release program reported it before it quit:
        // "In Surface::configure", then "`Surface` width and height must be within the maximum supported texture
        // size. Requested was (32984, 8961), maximum extent for either dimension is 32768."
        assert!(is_surface_error(&validation("In Surface::configure")));
        // A size within the limit but too large for the GPU's memory leaves the surface without a picture.
        assert!(is_surface_error(&eframe::wgpu::Error::OutOfMemory {
            source: Box::new(Source("In Surface::configure"))
        }));
        assert!(is_surface_error(&validation("In Surface::get_current_texture_view")));
        // Every other error stays fatal.
        assert!(!is_surface_error(&validation("In Device::create_texture, label = 'preview'")));
        assert!(!is_surface_error(&eframe::wgpu::Error::Internal {
            source: Box::new(Source("In Queue::submit")),
            description: String::new(),
        }));
        // Drawing a frame larger than the GPU can draw fails too (the frame of the size above: "In a set_viewport
        // command: Viewport size { w: 32984, h: 8961 } greater than device's requested `max_texture_dimension_2d`
        // limit 32768"); that is survived only while the window is that large.
        let drawing = validation("In a CommandEncoder, label = 'encoder'");
        assert!(!is_fatal(&drawing, true));
        assert!(is_fatal(&drawing, false));
        assert!(!is_fatal(&validation("In Surface::configure"), false));
    }

    #[test]
    fn a_surface_left_without_a_setup_is_set_up_again() {
        use eframe::egui_wgpu::SurfaceErrorAction as Action;
        use eframe::wgpu::CurrentSurfaceTexture as Status;
        let answer = on_surface_status();
        // The window stayed dark after a 40000 × 40000 window: "Surface is not configured for presentation".
        assert!(matches!(answer(&Status::Validation), Action::Reconfigure));
        // The rest is `egui_wgpu`'s own answer.
        assert!(matches!(answer(&Status::Outdated), Action::Reconfigure));
        assert!(matches!(answer(&Status::Lost), Action::RecreateSurface));
        assert!(matches!(answer(&Status::Occluded), Action::SkipFrame));
        assert!(matches!(answer(&Status::Timeout), Action::SkipFrame));
    }

    #[test]
    fn the_desktop_is_the_box_around_every_monitor() {
        use eframe::egui::vec2;
        assert_eq!(desktop_size([([0, 0], [1920, 1080])]), Some(vec2(1920.0, 1080.0)));
        // A 4K monitor left of the main one and 200 pixels higher (down to 1960), a portrait one on the right.
        let monitors = [([0, 0], [2560, 1440]), ([-3840, -200], [3840, 2160]), ([2560, 0], [1080, 1920])];
        assert_eq!(desktop_size(monitors), Some(vec2(3840.0 + 2560.0 + 1080.0, 200.0 + 1960.0)));
        assert_eq!(desktop_size([]), None);
        assert_eq!(desktop_size([([0, 0], [0, 0])]), None);
    }

    #[test]
    fn a_window_too_large_goes_back_to_its_monitor() {
        use eframe::egui::vec2;
        let (desktop, monitor) = (Some(vec2(1920.0, 1080.0)), Some(vec2(1920.0, 1080.0)));
        // The sizes the program quit at on a GPU of 32 768 and a 1920 × 1080 monitor: back to the monitor.
        for size in [vec2(32984.0, 8961.0), vec2(39984.0, 39961.0), vec2(29984.0, 29961.0)] {
            assert_eq!(drawable_size(size, 1.0, 32768, desktop, monitor), Some(vec2(1920.0, 1080.0)), "{size:?}");
        }
        // A side within the desktop is kept.
        assert_eq!(drawable_size(vec2(9000.0, 600.0), 1.0, 32768, desktop, monitor), Some(vec2(1920.0, 600.0)));
        // At 150 % a 4K monitor is 2560 × 1440 points.
        let (desktop_4k, monitor_4k) = (Some(vec2(3840.0, 2160.0)), Some(vec2(2560.0, 1440.0)));
        assert_eq!(drawable_size(vec2(30000.0, 500.0), 1.5, 32768, desktop_4k, monitor_4k), Some(vec2(2560.0, 500.0)));
        // Without monitors: the largest whole number of points the GPU draws. At 150 % 21 845 points are 32 767.5
        // pixels, which the system rounds to 32 768.
        assert_eq!(drawable_size(vec2(32984.0, 8961.0), 1.0, 32768, None, None), Some(vec2(32768.0, 8961.0)));
        assert_eq!(drawable_size(vec2(30000.0, 500.0), 1.5, 32768, None, None), Some(vec2(21845.0, 500.0)));
        assert_eq!(drawable_size(vec2(9000.0, 9000.0), 2.0, 16384, None, None), Some(vec2(8192.0, 8192.0)));
        // A desktop known without its monitor: within the desktop and its slack.
        assert_eq!(drawable_size(vec2(5000.0, 500.0), 1.0, 32768, desktop, None), Some(vec2(2112.0, 500.0)));
        // A desktop larger than the GPU draws does not raise the limit; nor does a monitor of no size.
        let wall = Some(vec2(20000.0, 20000.0));
        assert_eq!(drawable_size(vec2(9000.0, 9000.0), 1.0, 8192, wall, wall), Some(vec2(8192.0, 8192.0)));
        let none = Some(vec2(0.0, 0.0));
        assert_eq!(drawable_size(vec2(9000.0, 9000.0), 1.0, 8192, none, none), Some(vec2(8192.0, 8192.0)));
        // Every size a person can give the window is left alone: the usual ones, across every monitor (the desktop
        // and its frame), tiny, none (minimised).
        for size in [vec2(1600.0, 1000.0), vec2(2100.0, 1180.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(0.0, 500.0)] {
            assert_eq!(drawable_size(size, 1.0, 32768, desktop, monitor), None, "{size:?}");
        }
        let three = Some(vec2(3.0 * 3840.0, 2160.0));
        assert_eq!(drawable_size(vec2(11520.0, 2160.0), 1.0, 16384, three, Some(vec2(3840.0, 2160.0))), None);
        assert_eq!(drawable_size(vec2(16384.0, 100.0), 2.0, 32768, None, None), None);
        // Nothing sensible to ask for: unknown scale, no limit, a size that is not a number.
        assert_eq!(drawable_size(vec2(40000.0, 40000.0), 0.0, 32768, desktop, monitor), None);
        assert_eq!(drawable_size(vec2(40000.0, 40000.0), 1.0, 0, desktop, monitor), None);
        assert_eq!(drawable_size(vec2(f32::NAN, 40000.0), 1.0, 32768, desktop, monitor), None);
    }

    #[test]
    fn default_viewport_preserves_os_native_icon() {
        let builder = default_viewport();
        assert_eq!(builder.icon.as_deref(), Some(&IconData::default()));
        let restored = WindowState { maximized: true, ..WindowState::default() }.apply(builder);
        assert_eq!(restored.icon.as_deref(), Some(&IconData::default()));
    }
}
