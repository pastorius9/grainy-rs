//! The HDR editing mode (APP_PLAN package 32): the HDR button of the Basic panel, the HDR section (limit,
//! brightness, the HDR range view, the screen's status, the SDR preview and its two sliders), the HDR preview path
//! of the render worker, the HDR histogram and the clipping indicators. Port of luma/app.py `toggle_hdr`,
//! `sync_hdr_controls`, `toggle_hdr_visualize`, the HDR branch of `render`'s `work()` and `done()`, luma/hdr.py
//! `to_srgb_extended`, `histogram`, `visualize`, luma/hdr_display.py `HdrDisplay` (without the native screen, see
//! below) and the HDR parts of luma/widgets.py `Histogram` and `PhotoView.drawForeground`.
//!
//! **Switch.** The oracle 0.6.8 hides the mode (`engine.HDR_FEATURE = False`, since 0.5.73: its ordinary SDR export
//! dulled whites at the default compression and there is no shareable HDR file format besides the 32-bit TIFF). The
//! owner decided to bring it back in the Rust application, so `main.rs` calls [`enable`] (the oracle's tests and
//! self-test assign `engine.HDR_FEATURE = True` the same way). The engine and the codecs already carried the mode
//! (`grainy_tone::hdr`, the HDR branch of `grainy_engine::develop`, the 32-bit TIFF writer). The SDR export keeps the
//! oracle's maths: an HDR photo saved as JPEG/PNG/TIFF goes through `sdr_rendition` with the photo's SDR preview
//! sliders, exactly as the oracle did when the mode was on.
//!
//! **What stays off: the native HDR screen.** The oracle shows HDR pixels above SDR white on an HDR monitor through
//! its own Direct3D 11 swap chain in scRGB (`native_hdr.py`, a separately built `grainy_hdr.dll`; its self-test
//! notes `physical_hdr_verified=False`). The Rust window draws through egui on an 8-bit sRGB surface, and replacing
//! that surface for the photo area is not possible without owning the window's swap chain. So the screen is always
//! the oracle's SDR screen (`native_hdr.Display()`: not active, `stops = 0`): the status reads "SDR 화면 · 흰색까지
//! 표시 · HDR 계조는 유지", values above white are clipped on screen and marked by the clipping overlay, the HDR
//! histogram shows everything above white as beyond the screen. Nothing claims an HDR screen.

use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke, Ui, Vec2};
use grainy_core::{Image, Settings};
use grainy_i18n::tr;
use serde_json::Value;

use crate::commands::{Command, Invocation};
use crate::context::AppContext;
use crate::histogram::{Bins, Channels, HIGHLIGHT, SHADOW};
use crate::theme as t;
use crate::widgets::{self, Button};

/// Switches the HDR editing mode on for this process (port of assigning `engine.HDR_FEATURE = True`).
pub fn enable() {
    grainy_core::settings::set_hdr_feature(true);
}

/// Whether the HDR editing mode is on (`engine.HDR_FEATURE`).
pub fn enabled() -> bool {
    grainy_core::settings::hdr_feature()
}

/// The window's viewing choices for an HDR photo (they are not photo settings).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct View {
    /// 'SDR 디스플레이용 미리보기' (`HdrDisplay.preview`, the library preference `hdr_sdr_preview`).
    pub sdr_preview: bool,
    /// 'HDR 영역 표시' (`hdr_visualize`; not stored).
    pub visualize: bool,
}

/// What the histogram and the clipping button show about an HDR frame (`Histogram.hdr`, `hdr_limit`,
/// `hdr_inside`, `hdr_outside`, `hdr_preview`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameInfo {
    pub limit: f64,
    /// Some pixel is above SDR white but within the screen's range (never on the SDR screen).
    pub inside: bool,
    /// Some pixel is beyond the screen's range.
    pub outside: bool,
    /// The SDR preview was on.
    pub preview: bool,
}

/// The screen's range in stops above SDR white: always 0, the Rust window has no HDR surface (see the module
/// comment; `native_hdr.Display().stops`).
pub const DISPLAY_STOPS: f64 = 0.0;

/// The window's HDR state (`cx.state::<HdrState>()`).
#[derive(Debug, Default)]
pub struct HdrState {
    pub view: View,
    /// Changes whenever [`View`] changes: part of the preview's key, so the picture is made again.
    pub revision: u64,
    /// The library whose preference was read (`None`: not read yet).
    loaded: Option<Option<std::path::PathBuf>>,
}

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn bump(state: &mut HdrState) {
    state.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Once per frame (one hook line in `AppContext::poll_background`): reads the library's `hdr_sdr_preview` when the
/// library changes (`HdrDisplay.__init__`) and hands the view to the renderer.
pub fn poll(cx: &mut AppContext) {
    if !enabled() {
        return;
    }
    let folder = cx.doc.library_folder();
    if cx.state::<HdrState>().loaded.as_ref() != Some(&folder) {
        let stored = cx.doc.preference("hdr_sdr_preview").and_then(|v| v.as_bool()).unwrap_or(false);
        let state = cx.state::<HdrState>();
        state.loaded = Some(folder);
        if state.view.sdr_preview != stored {
            state.view.sdr_preview = stored;
            bump(state);
        }
    }
    let view = cx.state::<HdrState>().view;
    cx.renderer.set_hdr_view(view);
}

/// The view's revision for the preview's key (0 while the mode is off).
pub fn revision(cx: &AppContext) -> u64 {
    if !enabled() {
        return 0;
    }
    cx.state_ref::<HdrState>().map_or(0, |s| s.revision)
}

/// The commands of this package.
pub const COMMANDS: &[Command] = &[Command { id: "develop.hdr", label: "HDR", enabled: can_toggle, run: toggle }];

fn can_toggle(cx: &AppContext) -> bool {
    enabled() && cx.doc.current().is_some()
}

/// Port of `toggle_hdr`: the photo's `hdr` setting on or off, one undo step ('보정 변경').
fn toggle(cx: &mut AppContext, _: &Invocation) {
    if !can_toggle(cx) {
        return;
    }
    let on = !cx.settings().bool("hdr");
    cx.doc.edit_mut().set("hdr", on);
    cx.doc.edit_mut().commit("보정 변경");
}

/// The HDR button of the automatic row (`hdr_button`, checkable, text 'HDR'), `width` wide. Shown only while the
/// mode is on.
pub fn button(ui: &mut Ui, cx: &mut AppContext, width: f32) {
    if !enabled() {
        return;
    }
    let on = cx.settings().bool("hdr");
    if Button::new("HDR").checked(on).width(width).show(ui).clicked() {
        cx.run("develop.hdr");
    }
}

/// The HDR section of the Basic panel (`hdr_panel`, visible while the photo is in HDR mode: `sync_hdr_controls`).
pub fn panel(ui: &mut Ui, cx: &mut AppContext) {
    if !enabled() || !cx.settings().bool("hdr") {
        return;
    }
    widgets::section(ui, "HDR");
    for key in ["hdr_limit", "hdr_brightness"] {
        if let Some(spec) = crate::adjustments::spec(key) {
            cx.slider(ui, spec, true);
        }
    }
    let mut view = cx.state::<HdrState>().view;
    if widgets::checkbox(ui, &mut view.visualize, &tr("HDR 영역 표시")).changed() {
        set_view(cx, view);
    }
    // `HdrDisplay.update_ui`: the screen is never an active HDR screen here (see the module comment).
    let label = if view.sdr_preview {
        tr("SDR 미리보기 · 하이라이트 압축 적용")
    } else {
        tr("SDR 화면 · 흰색까지 표시 · HDR 계조는 유지")
    };
    widgets::muted(ui, &label).on_hover_text(tr(
        "HDR 미리보기에서는 SDR 압축을 적용하지 않습니다. SDR 화면의 흰색을 넘는 밝기는 화면에서 구분되지 않습니다.",
    ));
    if widgets::checkbox(ui, &mut view.sdr_preview, &tr("SDR 디스플레이용 미리보기")).changed() {
        // `set_preview`: stored in the library.
        if let Err(message) = cx.doc.save_preference("hdr_sdr_preview", &Value::Bool(view.sdr_preview)) {
            log::warn!("hdr_sdr_preview not stored: {message}");
        }
        set_view(cx, view);
    }
    if view.sdr_preview {
        for key in ["sdr_exposure", "sdr_compression"] {
            if let Some(spec) = crate::adjustments::spec(key) {
                cx.slider(ui, spec, true);
            }
        }
    }
}

/// Changes the viewing choices (the picture is made again).
pub fn set_view(cx: &mut AppContext, view: View) {
    let state = cx.state::<HdrState>();
    if state.view != view {
        state.view = view;
        bump(state);
    }
    cx.renderer.set_hdr_view(view);
}

// ---- pixels ------------------------------------------------------------------------------------------------------

/// numpy's `float(x)` comparison constant of a Python float against float32 pixels (weak scalar).
fn f(v: f64) -> f32 {
    v as f32
}

/// Port of luma/hdr.py `to_srgb_extended`: sRGB-encoded extended pixels (unchanged for the sRGB working space).
pub fn to_srgb_extended(pixels: &Image, working_space: &str) -> Result<Image, String> {
    if working_space != "ProPhoto" {
        return Ok(pixels.clone());
    }
    let source = grainy_cms::profile("Linear ProPhoto").map_err(|e| e.to_string())?;
    let target = grainy_cms::profile("Linear sRGB").map_err(|e| e.to_string())?;
    let linear = grainy_color::to_linear(pixels);
    let converted = grainy_cms::convert(&linear, &source, &target).map_err(|e| e.to_string())?;
    Ok(grainy_color::to_srgb(&converted))
}

/// Port of luma/hdr.py `histogram`: 128 bins per channel, SDR in the left half, one stop per sixteenth above white
/// (every `max(1, height // 300)`-th row and column).
pub fn histogram(pixels: &Image) -> Bins {
    let mut bins = [[0u32; 128]; 3];
    let (height, width, _) = pixels.dim();
    let step = (height / 300).max(1);
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            for (c, channel) in bins.iter_mut().enumerate() {
                let a = pixels[[y, x, c]];
                let coordinate = if a <= 1.0 {
                    a.clamp(0.0, 1.0) * 0.5
                } else {
                    0.5 + grainy_color::srgb_decode(a).max(1.0).log2() / 8.0
                };
                let scaled = coordinate.clamp(0.0, 1.0) * 128.0;
                // astype(int32) truncates; NaN (no oracle value) counts as 0.
                let bin = if scaled.is_nan() { 0 } else { (scaled as i32).min(127) as usize };
                channel[bin] += 1;
            }
        }
    }
    bins
}

/// Port of luma/hdr.py `visualize`: pixels above SDR white coloured by their stop (blue, green, yellow, red), the
/// rest dimmed grey.
pub fn visualize(pixels: &Image) -> Image {
    const COLORS: [[f32; 3]; 4] = [[0.25, 0.55, 1.0], [0.2, 1.0, 0.45], [1.0, 0.9, 0.1], [1.0, 0.25, 0.15]];
    let (height, width, _) = pixels.dim();
    Image::from_shape_fn((height, width, 3), |(y, x, c)| {
        let p = [pixels[[y, x, 0]], pixels[[y, x, 1]], pixels[[y, x, 2]]];
        let peak = p.iter().map(|v| grainy_color::srgb_decode(*v)).fold(f32::NEG_INFINITY, f32::max);
        let stops = peak.max(1.0).log2();
        if stops > f(1e-5) {
            let index = ((stops.ceil() as i64) - 1).clamp(0, 3) as usize;
            COLORS[index][c]
        } else {
            // pixels.mean(axis=-1): float32 sum in order, divided by 3.
            let mean = (p[0] + p[1] + p[2]) / 3.0;
            mean.clamp(0.0, 1.0) * f(0.35)
        }
    })
}

/// Port of luma/widgets.py `clipping_channels(image, ceiling)`: per channel, whether any value is below 1/510
/// (shadows) or at least `ceiling - 1/510` (highlights). NaN is skipped (`fmin`/`fmax`).
pub fn clipping_channels(image: &Image, ceiling: f64) -> Channels {
    let low_limit = f(1.0 / 510.0);
    let high_limit = f(ceiling - 1.0 / 510.0);
    let mut channels = ([false; 3], [false; 3]);
    for p in image.as_standard_layout().as_slice().unwrap_or(&[]).as_chunks::<3>().0.iter() {
        for c in 0..3 {
            if p[c] < low_limit {
                channels.0[c] = true;
            }
            if p[c] >= high_limit {
                channels.1[c] = true;
            }
        }
    }
    channels
}

/// The oracle's `float(to_srgb(np.float32(2**stops)))`.
pub fn encoded_ceiling(stops: f64) -> f64 {
    f64::from(grainy_color::srgb_encode(2f64.powf(stops) as f32))
}

/// A presented HDR frame: what the worker hands to the window.
pub struct Presented {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub bins: Bins,
    pub channels: Channels,
    /// One byte per pixel: [`HIGHLIGHT`] (above what the screen shows) and [`SHADOW`].
    pub mask: Vec<u8>,
    pub info: FrameInfo,
}

/// Port of the HDR branch of `render`'s `work()` and `done()` (luma/app.py) on the SDR screen: `working` is
/// `develop(..., output_space=None)` of an HDR photo (the limited, sRGB-encoded extended working pixels).
pub fn present(
    working: &Image,
    settings: &Settings,
    view: View,
    monitor: Option<&grainy_cms::Profile>,
) -> Result<Presented, String> {
    let space = settings.str("working_space");
    let hdr_pixels = to_srgb_extended(working, space)?;
    let rendition;
    let shown = if view.sdr_preview {
        rendition = grainy_tone::hdr::sdr_rendition(working, settings);
        &rendition
    } else {
        working
    };
    let display = if view.visualize {
        grainy_cms::display_rgb(&visualize(&hdr_pixels), monitor).map_err(|e| e.to_string())?
    } else {
        let destination = match monitor {
            Some(profile) => grainy_cms::Destination::Icc(profile),
            None => grainy_cms::Destination::Named("sRGB"),
        };
        grainy_cms::output_rgb(shown, space, destination).map_err(|e| e.to_string())?
    };
    let (height, width, _) = display.dim();
    let mut rgba = vec![255u8; width * height * 4];
    let standard = display.as_standard_layout();
    for (pixel, p) in
        rgba.as_chunks_mut::<4>().0.iter_mut().zip(standard.as_slice().unwrap_or(&[]).as_chunks::<3>().0.iter())
    {
        for c in 0..3 {
            pixel[c] = crate::histogram::to_byte(p[c]);
        }
    }
    let limit = settings.f64("hdr_limit");
    let channels = clipping_channels(&hdr_pixels, encoded_ceiling(limit));
    // `set_hdr_ranges` and the overlay of `drawForeground` against the screen's ceiling.
    let screen = f(encoded_ceiling(DISPLAY_STOPS) + 1e-5);
    let white = f(1.0 + 1e-5);
    let shadow = f(1.0 / 510.0);
    let mut mask = vec![0u8; width * height];
    let (mut inside, mut outside) = (false, false);
    let pixels = hdr_pixels.as_standard_layout();
    for (bits, p) in mask.iter_mut().zip(pixels.as_slice().unwrap_or(&[]).as_chunks::<3>().0.iter()) {
        let peak = p[0].max(p[1]).max(p[2]);
        if peak > white && peak <= screen {
            inside = true;
        }
        if peak > screen {
            outside = true;
            *bits |= HIGHLIGHT;
        }
        if p.iter().any(|v| *v < shadow) {
            *bits |= SHADOW;
        }
    }
    Ok(Presented {
        width,
        height,
        rgba,
        bins: histogram(&hdr_pixels),
        channels,
        mask,
        info: FrameInfo { limit, inside, outside, preview: view.sdr_preview },
    })
}

// ---- the histogram -----------------------------------------------------------------------------------------------

fn grey() -> Color32 {
    Color32::from_rgb(0xc4, 0xc9, 0xd0)
}

/// The HDR marks of `Histogram.paintEvent` over the bins: SDR / HDR captions, the white line and four stop lines,
/// and the screen's range bar (all of it beyond the SDR screen).
pub fn paint_histogram(painter: &Painter, rect: Rect, info: &FrameInfo) {
    let font = t::font(t::FONT_SIZE);
    let at = |x: f32, baseline: f32| Pos2::new(rect.left() + x, rect.top() + baseline);
    painter.text(at(30.0, 17.0), egui::Align2::LEFT_BOTTOM, "SDR", font.clone(), grey());
    let caption = if info.preview { "HDR / SDR" } else { "HDR" };
    painter.text(
        at((rect.width() * 0.5).floor() + 4.0, 17.0),
        egui::Align2::LEFT_BOTTOM,
        caption,
        font.clone(),
        grey(),
    );
    for stop in 0..5 {
        let x = (rect.width() * (0.5 + stop as f32 / 8.0)).floor();
        let top = at(x, 25.0);
        let bottom = at(x, rect.height() - 13.0);
        if stop == 0 {
            painter.line_segment([top, bottom], Stroke::new(1.0, grey()));
        } else {
            painter.extend(egui::Shape::dashed_line(&[top, bottom], Stroke::new(1.0, grey()), 4.0, 2.0));
        }
        let text = if stop == 0 { "W".to_owned() } else { format!("+{stop}") };
        painter.text(at(x - 13.0, rect.height() - 1.0), egui::Align2::LEFT_BOTTOM, text, font.clone(), grey());
    }
    let half = (rect.width() * 0.5).floor();
    let edge = (rect.width() * (0.5 + DISPLAY_STOPS.min(4.0) as f32 / 8.0)).floor();
    let bar = |from: f32, to: f32, color: Color32| {
        if to > from {
            painter.rect_filled(
                Rect::from_min_size(at(from, rect.height() - 14.0), Vec2::new(to - from, 3.0)),
                0.0,
                color,
            );
        }
    };
    bar(half, edge, Color32::from_rgb(0xdd, 0xd4, 0xb6));
    bar(edge, rect.width(), Color32::from_rgb(0xe9, 0x69, 0x5a));
}

/// `refresh_indicators` of the highlight button for an HDR frame: red when some pixel is beyond the screen, yellow
/// when some is above white within it, grey otherwise; and its title.
pub fn indicator(info: &FrameInfo) -> (Color32, String) {
    let color = if info.outside {
        Color32::from_rgb(0xe9, 0x69, 0x5a)
    } else if info.inside {
        Color32::from_rgb(0xf3, 0xd1, 0x5d)
    } else {
        Color32::from_rgb(0x59, 0x63, 0x6f)
    };
    (color, tr("HDR 화면 범위 · 노랑: 표시 가능 · 빨강: 화면 한계 초과"))
}
