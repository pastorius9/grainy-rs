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
            .filter(|[x, y]| (-16_000.0..=32_000.0).contains(x) && (-100.0..=32_000.0).contains(y));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_viewport_preserves_os_native_icon() {
        let builder = default_viewport();
        assert_eq!(builder.icon.as_deref(), Some(&IconData::default()));
        let restored = WindowState { maximized: true, ..WindowState::default() }.apply(builder);
        assert_eq!(restored.icon.as_deref(), Some(&IconData::default()));
    }
}
