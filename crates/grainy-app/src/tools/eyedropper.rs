//! The white-balance eyedropper (화이트 밸런스 스포이드, W) and the film-base picker (필름 베이스 찍기): a click on
//! the photo reads around one point, a drag reads the average of its rectangle. Ordinary photos take the shown
//! colour as the neutral and move temperature and tint; RAW photos take the camera's own pixels (through the whole
//! geometry, in source pixels) and set the RAW white balance; a film negative takes the unexposed film base.
//!
//! Ports of luma/eyedropper.py (`mean_colour`, `area_block`), luma/app.py (`toggle_wb`, `wb_shortcut`,
//! `sample_wb`, `sample_wb_area`, `toggle_negative_base`, `sample_negative_base`), luma/studio.py (`area_pixels`) and
//! of the sample mode of luma/widgets.py `PhotoView` (`sample_mode`, `sample_rect`, the mouse handlers, Esc, the
//! eyedropper cursor, the loupe `draw_loupe` and the dashed rectangle `draw_sample_area`).
//!
//! The model (this file's pure functions) works on plain arrays so that the tests can run the oracle's clicks
//! and rectangles without a window; the context part (`toggle_wb`, `sample`, the tool) wires it to the window: the
//! shown frame for ordinary photos, a job that loads the 1800-pixel source for RAW photos and negatives.
//!
//! Deviations from the oracle, none of which changes a setting: the click on a rectangle's few pixels counts as a
//! click when egui reports a click (its own threshold of six points), the negative button shows as pressed only
//! while the picker is on (the oracle leaves it pressed when another tool takes the view), and the ordinary
//! branch reads the 8-bit frame on screen (the oracle's float `on_screen`), so its values are within the
//! quantisation (half a level) of the oracle's.

use eframe::egui::{
    self, epaint, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Painter, Pos2, Rect, Shape, Stroke,
    TextureHandle, TextureOptions, Vec2,
};
use grainy_core::{Image, Settings};
use grainy_engine::files::{load_photo, Photo};
use grainy_raw::rawcolor::{self, CameraSource};
use serde_json::{json, Value};

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::{AppContext, Mode};
use crate::imagespace::{CoordinateMap, ViewTransform};
use crate::panels::Tool;
use crate::render::Preview;
use crate::state::EditState;
use crate::text::{tr, tr_with};

/// A channel this close to full has lost its colour (`CLIPPED`).
pub const CLIPPED: f32 = 0.98;
/// Screen pixels the pointer has to move before a click turns into a rectangle (`SAMPLE_DRAG`).
pub const SAMPLE_DRAG: f32 = 4.0;
/// The loupe's radius in screen pixels (`LOUPE_RADIUS`).
pub const LOUPE_RADIUS: f32 = 72.0;
/// The loupe's magnification over the size the photo has on screen (`LOUPE_ZOOM`).
pub const LOUPE_ZOOM: f32 = 6.0;
/// The long side of the oracle's source (`load_image(path, 1800, …)`).
pub const SOURCE_SIZE: usize = 1800;
/// The history label of a RAW white balance pick.
pub const RAW_LABEL: &str = "RAW 화이트밸런스 스포이드";
/// The history label of a film base pick.
pub const BASE_LABEL: &str = "필름 베이스 지정";

/// The status texts (oracle strings).
pub const HINT_WHITE: &str = "사진에서 중성 회색이나 흰색 부분을 클릭하거나, 드래그해 영역을 고르세요.";
pub const HINT_BASE: &str = "사진 가장자리의 촬영되지 않은 빈 필름을 클릭하세요.";
pub const TOO_DARK_RAW: &str = "너무 어두운 영역입니다. 밝은 중성 회색을 선택하세요.";
pub const NO_COLOUR: &str = "이 영역은 모두 너무 밝거나 어둡습니다. 색이 남아 있는 중성 회색이나 흰색 영역을 고르세요.";
pub const TOO_DARK_BASE: &str = "빈 필름으로 보기에는 너무 어두운 곳입니다. 촬영되지 않은 가장 밝은 부분을 클릭하세요.";

// ---- the pointer: `PhotoView` in the sample mode ----------------------------------------------------------------

/// What the pointer picked: a point (`sampled`) or a rectangle (`sampledArea`), as fractions of the shown frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sample {
    Point([f64; 2]),
    /// x0, y0, x1, y1.
    Area([f64; 4]),
}

impl Sample {
    /// `sample_wb_area`: a rectangle's centre is the point `sample_wb` is given along with it.
    pub fn centre(&self) -> [f64; 2] {
        match self {
            Self::Point(p) => *p,
            Self::Area(a) => [(a[0] + a[2]) / 2.0, (a[1] + a[3]) / 2.0],
        }
    }
}

/// A press in the sample mode (`sample_press`) and the rectangle drawn from it (`sample_area`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gesture {
    start: Pos2,
    origin: [f64; 2],
    area: Option<[f64; 4]>,
}

impl Gesture {
    /// `mousePressEvent`: the press counts when it is on the photo (`free` is the pointer in frame fractions).
    pub fn press(screen: Pos2, free: [f64; 2]) -> Option<Self> {
        let inside = (0.0..=1.0).contains(&free[0]) && (0.0..=1.0).contains(&free[1]);
        inside.then_some(Self { start: screen, origin: free, area: None })
    }

    /// `sample_rect`: the rectangle from the press to the pointer (its end kept on the photo), `None` while the
    /// pointer is within [`SAMPLE_DRAG`] pixels of the press.
    fn rect(&self, screen: Pos2, frame: [f64; 2]) -> Option<[f64; 4]> {
        if (screen.x - self.start.x).abs().max((screen.y - self.start.y).abs()) < SAMPLE_DRAG {
            return None;
        }
        let end = [frame[0].clamp(0.0, 1.0), frame[1].clamp(0.0, 1.0)];
        Some([
            self.origin[0].min(end[0]),
            self.origin[1].min(end[1]),
            self.origin[0].max(end[0]),
            self.origin[1].max(end[1]),
        ])
    }

    /// `mouseMoveEvent` with the button down: the rectangle so far.
    pub fn moved(&mut self, screen: Pos2, frame: [f64; 2]) {
        self.area = self.rect(screen, frame);
    }

    /// The rectangle being drawn (`sample_area`).
    pub fn area(&self) -> Option<[f64; 4]> {
        self.area
    }

    /// `mouseReleaseEvent`: the click or the rectangle.
    pub fn release(self, screen: Pos2, frame: [f64; 2]) -> Sample {
        match self.rect(screen, frame) {
            None => Sample::Point(self.origin),
            Some(area) => Sample::Area(area),
        }
    }
}

// ---- the display branch: ordinary photos ------------------------------------------------------------------------

/// A picture to read colours from: its size and the colour at a pixel.
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub pixel: &'a dyn Fn(usize, usize) -> [f32; 3],
}

/// The frame on screen as a picture (8-bit RGBA read as `value / 255`).
pub fn preview_pixel(preview: &Preview) -> impl Fn(usize, usize) -> [f32; 3] + '_ {
    move |x, y| {
        let i = (y * preview.width + x) * 4;
        [0, 1, 2].map(|k| f32::from(preview.rgba.get(i + k).copied().unwrap_or(0)) / 255.0)
    }
}

/// Port of `eyedropper.mean_colour`: the average of the pixels without the clipped ones (a channel at
/// [`CLIPPED`] or more) and the black ones (a channel at or under `floor`), or `None` when none is left.
pub fn mean_colour(pixels: impl IntoIterator<Item = [f32; 3]>, floor: f32) -> Option<[f64; 3]> {
    let mut sum = [0f64; 3];
    let mut count = 0usize;
    for p in pixels {
        let max = p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let min = p.iter().copied().fold(f32::INFINITY, f32::min);
        if p.iter().all(|v| v.is_finite()) && max < CLIPPED && min > floor {
            for c in 0..3 {
                sum[c] += f64::from(p[c]);
            }
            count += 1;
        }
    }
    (count > 0).then(|| sum.map(|v| v / count as f64))
}

/// Port of `eyedropper.area_block`: the columns and rows (`left..right`, `top..bottom`) of a `width` × `height`
/// frame inside a rectangle given as fractions of it (x0, y0, x1, y1): at least one pixel.
pub fn area_block(width: usize, height: usize, area: [f64; 4]) -> ([usize; 2], [usize; 2]) {
    let [x0, y0, x1, y1] = area.map(|v| v.clamp(0.0, 1.0));
    let (w, h) = (width as f64, height as f64);
    let left = width.saturating_sub(1).min((x0 * w) as usize);
    let top = height.saturating_sub(1).min((y0 * h) as usize);
    let right = (left + 1).max(width.min((x1 * w).ceil() as usize));
    let bottom = (top + 1).max(height.min((y1 * h).ceil() as usize));
    ([left, right], [top, bottom])
}

/// The index `min(n - 1, int(f * n))` of `sample_wb` for a fraction `f`.
fn index_of(fraction: f64, n: usize) -> usize {
    ((fraction * n as f64) as usize).min(n.saturating_sub(1))
}

/// The mean of the `(2 radius + 1)`² window around pixel (`px`, `py`), cut at the picture's edges.
fn window_mean(
    width: usize,
    height: usize,
    px: usize,
    py: usize,
    radius: usize,
    pixel: &dyn Fn(usize, usize) -> [f32; 3],
) -> [f32; 3] {
    let (x0, x1) = (px.saturating_sub(radius), (px + radius + 1).min(width));
    let (y0, y1) = (py.saturating_sub(radius), (py + radius + 1).min(height));
    // numpy's `.mean(axis=(0, 1))` of a float32 window: float32 sums in row order, divided by the count.
    let mut sum = [0f32; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = pixel(x, y);
            for c in 0..3 {
                sum[c] += p[c];
            }
        }
    }
    let n = ((x1 - x0) * (y1 - y0)).max(1) as f32;
    sum.map(|v| v / n)
}

/// The display branch of `sample_wb`: the colour read from the picture at a click (the 7 × 7 mean around the
/// point) or in a rectangle (`mean_colour(area_block(…))`), plus `.001`; `None` when the rectangle has no colour left.
pub fn display_colour(picture: &Picture, sample: &Sample) -> Option<[f32; 3]> {
    let (w, h) = (picture.width, picture.height);
    if w == 0 || h == 0 {
        return None;
    }
    match sample {
        Sample::Point(p) => {
            let mean = window_mean(w, h, index_of(p[0], w), index_of(p[1], h), 3, picture.pixel);
            Some(mean.map(|v| v + 0.001f32))
        }
        Sample::Area(a) => {
            let ([left, right], [top, bottom]) = area_block(w, h, *a);
            let pixels =
                (top..bottom).flat_map(|y| (left..right).map(move |x| (x, y))).map(|(x, y)| (picture.pixel)(x, y));
            mean_colour(pixels, 0.005).map(|m| m.map(|v| (v + 0.001) as f32))
        }
    }
}

/// `to_linear` of luma/engine.py for one value (float32).
fn decode(a: f32) -> f32 {
    use grainy_core::numpy::{f, maximum};
    if a <= f(0.04045) {
        a / f(12.92)
    } else {
        (maximum(a + f(0.055), 0.0) / f(1.055)).powf(f(2.4))
    }
}

/// The settings the display branch of `sample_wb` writes: temperature and tint moved by the light ratios of the
/// colour (the gains of `processing.white_balance` are `2 ** (±T / 180 + t / 400)` and `2 ** (-t / 250)`; the
/// shown pixels are display-encoded, so they are linearised first). float32 as the oracle's numpy scalars.
pub fn temperature_tint(rgb: [f32; 3], temperature: f64, tint: f64) -> [f64; 2] {
    let lin = rgb.map(|v| decode(v).max(1e-4f32));
    let t = (temperature as f32 + 90.0f32 * (lin[2] / lin[0]).log2()).clamp(-100.0, 100.0);
    let scale = (1.0 / 400.0 + 1.0 / 250.0) as f32;
    let n = (tint as f32 + (lin[1] / (lin[0] * lin[2]).sqrt()).log2() / scale).clamp(-100.0, 100.0);
    [f64::from(t), f64::from(n)]
}

/// Port of the display branch of `sample_wb` on an edit: temperature and tint set, then one plain commit. Returns
/// `Err(text)` (a status text) when a rectangle has no colour left.
pub fn apply_display(edit: &mut EditState, picture: &Picture, sample: &Sample) -> Result<(), &'static str> {
    let rgb = display_colour(picture, sample).ok_or(NO_COLOUR)?;
    let [t, n] = temperature_tint(rgb, edit.settings().f64("temperature"), edit.settings().f64("tint"));
    edit.set("temperature", t);
    edit.set("tint", n);
    edit.commit("보정 변경");
    Ok(())
}

// ---- the source branch: RAW photos and negatives ----------------------------------------------------------------

/// What a pick is on the source: the frame point carried through the geometry (`source_point`: `None` where the
/// frame shows empty canvas) or the source points under a rectangle (`area_pixels`' grid).
#[derive(Debug, Clone, PartialEq)]
pub enum SourcePick {
    Point(Option<[f64; 2]>),
    Area(Vec<[f64; 2]>),
}

/// Why a pick did nothing, or what to tell about it.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// Nothing happens (the point is off the photo, the rectangle empty).
    Silent,
    /// A status text of the oracle.
    Message(&'static str),
    /// The processing failed.
    Error(String),
}

/// Port of `numpy.linspace(start, stop, n)` for the grid of `area_pixels`.
fn linspace(start: f64, stop: f64, n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![start];
    }
    let step = (stop - start) / (n - 1) as f64;
    (0..n).map(|i| if i == n - 1 { stop } else { start + i as f64 * step }).collect()
}

/// The source points of `area_pixels` for a rectangle of the frame (fractions x0, y0, x1, y1): up to 80 × 80
/// positions inside it are carried to the source one by one through the geometry.
pub fn area_points(map: &CoordinateMap, area: [f64; 4]) -> Vec<[f64; 2]> {
    let [x0, y0, x1, y1] = area;
    let (w, h) = map.frame_size();
    let across = ((x1 - x0) * (w as f64 - 1.0) + 1.0).clamp(1.0, 80.0) as usize;
    let down = ((y1 - y0) * (h as f64 - 1.0) + 1.0).clamp(1.0, 80.0) as usize;
    let xs = linspace(x0, x1, across);
    let ys = linspace(y0, y1, down);
    let grid: Vec<[f64; 2]> = ys.iter().flat_map(|y| xs.iter().map(move |x| [*x, *y])).collect();
    map.source_points(&grid)
}

/// The pick on the source for a sample of the frame (`source_point` of the centre / `area_points`).
pub fn source_pick(map: &CoordinateMap, sample: &Sample) -> SourcePick {
    match sample {
        Sample::Point(p) => SourcePick::Point(map.source_point(*p)),
        Sample::Area(a) => SourcePick::Area(area_points(map, *a)),
    }
}

/// `StudioTools.area_pixels` after the grid: the source pixels at the given source points.
pub fn area_pixels(image: &Image, found: &[[f64; 2]]) -> Vec<[f32; 3]> {
    let (h, w, _) = image.dim();
    found
        .iter()
        .map(|p| {
            let x = (p[0] * (w as f64 - 1.0)).round_ties_even() as usize;
            let y = (p[1] * (h as f64 - 1.0)).round_ties_even() as usize;
            let (x, y) = (x.min(w - 1), y.min(h - 1));
            [image[[y, x, 0]], image[[y, x, 1]], image[[y, x, 2]]]
        })
        .collect()
}

/// The `(2 radius + 1)`² window of `image` around the source point, as the oracle slices it.
fn window(image: &Image, point: [f64; 2], radius: usize) -> Image {
    let (h, w, _) = image.dim();
    let (px, py) = (index_of(point[0], w), index_of(point[1], h));
    let (x0, x1) = (px.saturating_sub(radius), (px + radius + 1).min(w));
    let (y0, y1) = (py.saturating_sub(radius), (py + radius + 1).min(h));
    image.slice(ndarray::s![y0..y1, x0..x1, ..]).to_owned()
}

/// The RAW white balance a pick gives.
#[derive(Debug, Clone, PartialEq)]
pub struct RawWhite {
    /// `raw_neutral`: the neutral over its green.
    pub neutral: [f64; 3],
    /// `raw_kelvin` (2000 to 50000).
    pub kelvin: f64,
    /// `raw_tint` (-150 to 150).
    pub tint: f64,
}

/// The RAW branch of `sample_wb`: the camera-space neutral under the pick (the 7 × 7 mean of a click, the
/// `mean_colour` of a rectangle with floor 1e-4), then `rawcolor.neutral_white` with the photo's profile.
pub fn raw_white(source: &CameraSource, settings: &Settings, pick: &SourcePick) -> Result<RawWhite, Failure> {
    let pixels = source.pixels();
    let (neutral, scaled) = match pick {
        SourcePick::Point(None) => return Err(Failure::Silent),
        SourcePick::Point(Some(point)) => {
            let (h, w, _) = pixels.dim();
            let pixel = |x: usize, y: usize| [pixels[[y, x, 0]], pixels[[y, x, 1]], pixels[[y, x, 2]]];
            let mean = window_mean(w, h, index_of(point[0], w), index_of(point[1], h), 3, &pixel);
            if mean.iter().copied().fold(f32::INFINITY, f32::min) < 1e-5f32 {
                return Err(Failure::Message(TOO_DARK_RAW));
            }
            (mean.map(f64::from), mean.map(|v| f64::from(v / mean[1])))
        }
        SourcePick::Area(found) => {
            let mean = mean_colour(area_pixels(pixels, found), 1e-4).ok_or(Failure::Message(NO_COLOUR))?;
            (mean, mean.map(|v| v / mean[1]))
        }
    };
    let record = settings.value("dcp_profile");
    let profile = if grainy_core::settings::truthy(record) {
        grainy_raw::dcp::compiled(record)
    } else {
        rawcolor::default_profile(source.raw_info())
    }
    .map_err(|e| Failure::Error(e.to_string()))?;
    let (_, kelvin, tint) =
        rawcolor::neutral_white(&profile, neutral, source.raw_info()).map_err(|e| Failure::Error(e.to_string()))?;
    Ok(RawWhite { neutral: scaled, kelvin: kelvin.clamp(2000.0, 50000.0), tint: tint.clamp(-150.0, 150.0) })
}

/// The settings the RAW branch writes: one commit of what is pending, the white, the label
/// [`RAW_LABEL`].
pub fn apply_raw_white(edit: &mut EditState, white: &RawWhite) {
    edit.commit("보정 변경");
    edit.set("raw_neutral", json!(white.neutral));
    edit.set("raw_mode", "sample");
    edit.set("raw_kelvin", white.kelvin);
    edit.set("raw_tint", white.tint);
    edit.set("temperature", 0.0);
    edit.set("tint", 0.0);
    edit.set("kelvin_enabled", false);
    edit.commit(RAW_LABEL);
}

/// `negative_peak` as three numbers, when set.
fn peak_of(settings: &Settings) -> Option<[f64; 3]> {
    let list = settings.value("negative_peak").as_array()?;
    let at = |i: usize| list.get(i).and_then(Value::as_f64);
    Some([at(0)?, at(1)?, at(2)?])
}

/// The scan of `photo` as `sample_negative_base` reads it: `rawcolor.develop_camera` (a negative stays in the
/// camera's colours).
pub fn scan_of(photo: &Photo, settings: &Settings) -> Result<Image, String> {
    match photo {
        Photo::Camera(source) => rawcolor::develop_camera(source, settings).map_err(|e| e.to_string()),
        Photo::Linear(pixels) => rawcolor::develop_plain(pixels).map_err(|e| e.to_string()),
    }
}

/// Port of `sample_negative_base` after the point is on the source: the median of the 9 × 9 pixels around a click or
/// of the rectangle (`negative.base_from`), refused when it is not brighter than the densest part by 10 %.
pub fn negative_base(scan: &Image, pick: &SourcePick, peak: Option<[f64; 3]>) -> Result<[f64; 3], Failure> {
    let base = match pick {
        SourcePick::Point(None) => return Err(Failure::Silent),
        SourcePick::Point(Some(point)) => grainy_film::negative::base_from(&window(scan, *point, 4)),
        SourcePick::Area(found) => {
            let pixels = area_pixels(scan, found);
            if pixels.is_empty() {
                return Err(Failure::Silent);
            }
            let column = Image::from_shape_fn((pixels.len(), 1, 3), |(i, _, c)| pixels[i][c]);
            grainy_film::negative::base_from(&column)
        }
    };
    match peak {
        Some(peak) if base.iter().zip(&peak).all(|(b, p)| *b > p * 1.1) => Ok(base),
        _ => Err(Failure::Message(TOO_DARK_BASE)),
    }
}

/// The settings a film base pick writes: pending edits committed, `negative_base` set, [`BASE_LABEL`].
pub fn apply_negative_base(edit: &mut EditState, base: [f64; 3]) {
    edit.commit("보정 변경");
    let mut settings = edit.settings().clone();
    settings.set("negative_base", json!(base));
    edit.set_all(settings);
    edit.commit(BASE_LABEL);
}

// ---- the window --------------------------------------------------------------------------------------------------

/// What the pickers keep between frames (`cx.state::<Pickers>()`). The eyedropper's own switch (`sample_mode`)
/// is the context's `wb_picker`, which the other tools already reset when they take the view.
#[derive(Default)]
pub struct Pickers {
    /// The film base button is the one pressed (`negative_base_button.isChecked()`).
    base: bool,
    /// The photo the picker was switched on for (a new photo ends the mode, as `activate` does).
    photo: u64,
    /// The press in progress.
    gesture: Option<Gesture>,
    /// The coordinate map of the open photo's geometry (`source_point`'s `coordinates`).
    map: Option<(String, CoordinateMap)>,
}

fn state(cx: &mut AppContext) -> &mut Pickers {
    cx.state::<Pickers>()
}

/// The picker is on for the open photo.
fn on(cx: &AppContext) -> bool {
    cx.wb_picker && cx.state_ref::<Pickers>().is_some_and(|s| s.photo == cx.doc.current_id())
}

/// The white balance button is pressed.
pub fn wb_checked(cx: &AppContext) -> bool {
    on(cx) && !cx.state_ref::<Pickers>().is_some_and(|s| s.base)
}

/// The film base button is pressed.
pub fn base_checked(cx: &AppContext) -> bool {
    on(cx) && cx.state_ref::<Pickers>().is_some_and(|s| s.base)
}

/// Ends the mode (`sample_mode = False`, both buttons up).
fn off(cx: &mut AppContext) {
    cx.wb_picker = false;
    let s = state(cx);
    s.base = false;
    s.gesture = None;
}

/// A new photo ends the mode (`activate`); the Basic panel and the commands call it before they act.
pub fn sync(cx: &mut AppContext) {
    if cx.wb_picker && cx.state_ref::<Pickers>().is_none_or(|s| s.photo != cx.doc.current_id()) {
        off(cx);
    }
    if !cx.wb_picker && cx.state_ref::<Pickers>().is_some_and(|s| s.base || s.gesture.is_some()) {
        off(cx);
    }
}

/// What `toggle_wb` and `toggle_negative_base` do first: the Develop screen, the crop frame cancelled, the tools
/// that keep the pointer left (`view.set_tool('')`).
fn take_the_view(cx: &mut AppContext) {
    if cx.mode() != Mode::Develop {
        cx.set_mode(Mode::Develop);
    }
    if super::crop::is_open(cx) {
        super::crop::cancel(cx);
    }
    if cx.tool() != Tool::Edit {
        cx.select_tool(Tool::Edit);
    }
}

/// Port of `toggle_wb`.
pub fn toggle_wb(cx: &mut AppContext, checked: bool) {
    if cx.doc.current().is_none() {
        off(cx);
        return;
    }
    if checked {
        take_the_view(cx);
        let photo = cx.doc.current_id();
        let s = state(cx);
        s.base = false;
        s.gesture = None;
        s.photo = photo;
        cx.set_status(tr(HINT_WHITE));
    } else {
        state(cx).gesture = None;
    }
    cx.wb_picker = checked;
}

/// Port of `wb_shortcut` (the button runs the same): the picker on or off; from the library or another tool the
/// window goes to the Basic panel first.
pub fn shortcut(cx: &mut AppContext) {
    sync(cx);
    if cx.doc.current().is_none() {
        return;
    }
    let checked = wb_checked(cx);
    toggle_wb(cx, !checked);
}

/// Port of `toggle_negative_base`.
pub fn toggle_base(cx: &mut AppContext, checked: bool) {
    sync(cx);
    if cx.doc.current().is_none() || !cx.settings().bool("negative") {
        off(cx);
        return;
    }
    if checked {
        take_the_view(cx);
        let photo = cx.doc.current_id();
        let s = state(cx);
        s.base = true;
        s.gesture = None;
        s.photo = photo;
        cx.set_status(tr(HINT_BASE));
    } else {
        let s = state(cx);
        s.base = false;
        s.gesture = None;
    }
    cx.wb_picker = checked;
}

/// The geometry's coordinate map for the open photo on its live source, rebuilt when the geometry changes
/// (`source_point`'s cache).
fn map_for(cx: &mut AppContext) -> Option<&CoordinateMap> {
    let photo = cx.doc.current()?;
    let (w, h) = photo.original?;
    let (id, (w, h)) = (photo.id, super::healing::live_size(w as usize, h as usize));
    let effective = grainy_core::effective(cx.settings());
    let key = format!("{id}:{w}x{h}:{}", grainy_engine::settings_key(&effective, &grainy_engine::GEOMETRY_KEYS));
    if state(cx).map.as_ref().is_none_or(|(k, _)| *k != key) {
        let map = CoordinateMap::new(w, h, cx.settings()).ok()?;
        state(cx).map = Some((key, map));
    }
    state(cx).map.as_ref().map(|(_, m)| m)
}

/// Port of `sample_wb`: what a click or rectangle on the photo does.
pub fn sample(cx: &mut AppContext, sample: Sample) {
    if base_checked(cx) {
        return sample_negative_base(cx, sample);
    }
    let Some(photo) = cx.doc.current() else { return };
    let settings = cx.settings();
    if photo.is_raw() && rawcolor::enabled(settings) && !settings.bool("negative") {
        return sample_raw(cx, sample);
    }
    // The display branch: the colours as shown (lazily reads back zero-copy GPU textures on demand).
    let Some(frame) = cx.frame().filter(|f| f.photo == cx.doc.current_id()).cloned() else { return };
    let rgba = frame.rgba();
    let w = frame.preview.width;
    let h = frame.preview.height;
    let pixel = move |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        [0, 1, 2].map(|k| f32::from(rgba.get(i + k).copied().unwrap_or(0)) / 255.0)
    };
    let sample = match (frame.tile_uv, sample) {
        (Some([u0, v0, u1, v1]), Sample::Point([x, y])) => Sample::Point([
            ((x - f64::from(u0)) / f64::from((u1 - u0).max(1e-6))).clamp(0.0, 1.0),
            ((y - f64::from(v0)) / f64::from((v1 - v0).max(1e-6))).clamp(0.0, 1.0),
        ]),
        (Some([u0, v0, u1, v1]), Sample::Area([x0, y0, x1, y1])) => Sample::Area([
            ((x0 - f64::from(u0)) / f64::from((u1 - u0).max(1e-6))).clamp(0.0, 1.0),
            ((y0 - f64::from(v0)) / f64::from((v1 - v0).max(1e-6))).clamp(0.0, 1.0),
            ((x1 - f64::from(u0)) / f64::from((u1 - u0).max(1e-6))).clamp(0.0, 1.0),
            ((y1 - f64::from(v0)) / f64::from((v1 - v0).max(1e-6))).clamp(0.0, 1.0),
        ]),
        (_, s) => s,
    };
    let picture = Picture { width: w, height: h, pixel: &pixel };
    match apply_display(cx.doc.edit_mut(), &picture, &sample) {
        Ok(()) => {
            cx.before = false;
            off(cx);
        }
        Err(text) => cx.set_status(tr(text)),
    }
}

/// The open photo for a job: id, path, settings, and the edit version they belong to.
fn target(cx: &AppContext) -> Option<(u64, std::path::PathBuf, Settings, u64, u64)> {
    let photo = cx.doc.current()?;
    Some((photo.id, photo.path.clone(), cx.settings().clone(), cx.doc.edit().version(), cx.document_generation))
}

fn still(cx: &AppContext, id: u64, version: u64, generation: u64) -> bool {
    cx.document_generation == generation && cx.doc.current_id() == id && cx.doc.edit().version() == version
}

/// The RAW branch of `sample_wb`: the pick is carried to the source here, the camera source (the oracle's 1800
/// pixel `source`) is loaded and read on a job.
fn sample_raw(cx: &mut AppContext, sample: Sample) {
    if cx.jobs.running("picker") {
        return;
    }
    let Some(pick) = map_for(cx).map(|m| source_pick(m, &sample)) else { return };
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    cx.jobs.spawn(
        "picker",
        tr("화이트 밸런스 스포이드"),
        move |_| {
            let loaded = load_photo(&path, Some(SOURCE_SIZE), &settings).map_err(|e| e.to_string())?;
            match &loaded.photo {
                Photo::Camera(source) => Ok(raw_white(source, &settings, &pick)),
                Photo::Linear(_) => Err("the photo was not read in the camera's colours".to_owned()),
            }
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, version, generation) {
                return;
            }
            match result {
                Ok(Ok(white)) => {
                    apply_raw_white(cx.doc.edit_mut(), &white);
                    cx.before = false;
                    off(cx);
                }
                Ok(Err(failure)) => report(cx, failure),
                Err(message) => cx.set_status(tr_with("처리하지 못했습니다: {0}", &[&message])),
            }
        },
    );
}

/// A failed pick's status text.
fn report(cx: &mut AppContext, failure: Failure) {
    match failure {
        Failure::Silent => {}
        Failure::Message(text) => cx.set_status(tr(text)),
        Failure::Error(message) => cx.set_status(tr_with("처리하지 못했습니다: {0}", &[&message])),
    }
}

/// Port of `sample_negative_base`: the picker goes off at once, the scan is read on a job.
fn sample_negative_base(cx: &mut AppContext, sample: Sample) {
    off(cx);
    if cx.jobs.running("picker") {
        return;
    }
    let Some(pick) = map_for(cx).map(|m| source_pick(m, &sample)) else { return };
    if pick == SourcePick::Point(None) {
        return;
    }
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    cx.jobs.spawn(
        "picker",
        tr("필름 베이스 찍기"),
        move |_| {
            let loaded = load_photo(&path, Some(SOURCE_SIZE), &settings).map_err(|e| e.to_string())?;
            let scan = scan_of(&loaded.photo, &settings)?;
            Ok(negative_base(&scan, &pick, peak_of(&settings)))
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, version, generation) {
                return;
            }
            match result {
                Ok(Ok(base)) => {
                    apply_negative_base(cx.doc.edit_mut(), base);
                    cx.before = false;
                }
                Ok(Err(failure)) => report(cx, failure),
                Err(message) => cx.set_status(tr_with("처리하지 못했습니다: {0}", &[&message])),
            }
        },
    );
}

// ---- the tool ----------------------------------------------------------------------------------------------------

/// The canvas tool: active while the picker is on.
pub struct Eyedropper;

impl CanvasTool for Eyedropper {
    fn name(&self) -> &'static str {
        "white_balance"
    }

    fn active(&self, cx: &AppContext) -> bool {
        on(cx)
    }

    fn edits(&self) -> &'static [&'static str] {
        &["temperature", "tint"]
    }

    /// `PhotoView`'s mouse handlers in the sample mode: a press on the photo, the rectangle while dragging, and on
    /// release the click or the rectangle goes to `sample_wb` (the view never pans in this mode).
    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        match event.kind {
            PointerKind::Hover => {}
            PointerKind::Press => state(cx).gesture = Gesture::press(event.screen, event.free),
            PointerKind::Click => {
                if let Some(gesture) = Gesture::press(event.screen, event.free) {
                    let picked = gesture.release(event.screen, event.frame);
                    sample(cx, picked);
                }
            }
            PointerKind::Drag => {
                if let Some(gesture) = &mut state(cx).gesture {
                    gesture.moved(event.screen, event.frame);
                }
            }
            PointerKind::Release => {
                if let Some(gesture) = state(cx).gesture.take() {
                    let picked = gesture.release(event.screen, event.frame);
                    sample(cx, picked);
                }
            }
        }
        true
    }

    /// Esc ends the mode (`keyPressEvent`, `sampleCancelled`).
    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        if key == Key::Escape {
            off(cx);
            return true;
        }
        false
    }

    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        let hover = cx.egui.pointer_hover_pos().filter(|p| painter.clip_rect().contains(*p));
        let area = cx.state_ref::<Pickers>().and_then(|s| s.gesture).and_then(|g| g.area());
        if let Some(area) = area {
            draw_area(painter, view, area);
        }
        let Some(pointer) = hover else { return };
        if area.is_some() {
            cx.egui.set_cursor_icon(CursorIcon::Crosshair);
            return;
        }
        // The pointer is hidden: over the photo the loupe marks the spot, elsewhere the eyedropper is drawn.
        cx.egui.set_cursor_icon(CursorIcon::None);
        if view.rect.contains(pointer) {
            draw_loupe(painter, view, cx, pointer);
        } else {
            draw_eyedropper(painter, pointer);
        }
    }
}

/// `draw_sample_area`: the rectangle that will be averaged, a thin dashed light line over a dark one.
fn draw_area(painter: &Painter, view: &ViewTransform, area: [f64; 4]) {
    let a = view.to_screen([area[0], area[1]]);
    let b = view.to_screen([area[2], area[3]]);
    let rect = Rect::from_two_pos(a, b);
    let path = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    for (colour, offset) in
        [(Color32::from_rgba_unmultiplied(0, 0, 0, 200), 4.0), (Color32::from_rgb(0xf2, 0xf2, 0xf2), 0.0)]
    {
        painter.extend(Shape::dashed_line_with_offset(&path, Stroke::new(1.0, colour), &[4.0], &[4.0], offset));
    }
}

/// The eyedropper cursor of `sync_cursor` (a 32 × 32 picture with its hot spot at 4, 28), drawn at the pointer.
fn draw_eyedropper(painter: &Painter, pointer: Pos2) {
    let at = |x: f32, y: f32| pointer + Vec2::new(x - 4.0, y - 28.0);
    let tube: Vec<Pos2> =
        [(4.0, 28.0), (7.0, 21.0), (18.0, 10.0), (23.0, 15.0), (12.0, 26.0)].iter().map(|p| at(p.0, p.1)).collect();
    painter.add(Shape::convex_polygon(tube.clone(), Color32::WHITE, Stroke::new(3.0, Color32::BLACK)));
    painter.add(Shape::convex_polygon(tube, Color32::WHITE, Stroke::new(1.0, Color32::WHITE)));
    let cap: Vec<Pos2> = [(15.0, 9.0), (20.0, 4.0), (24.0, 4.0), (28.0, 8.0), (28.0, 12.0), (23.0, 17.0)]
        .iter()
        .map(|p| at(p.0, p.1))
        .collect();
    painter.add(Shape::convex_polygon(cap, Color32::from_rgb(0x20, 0x24, 0x2a), Stroke::new(1.5, Color32::WHITE)));
}

/// The frame as a texture that magnifies without smoothing, kept for the frame it was made from.
fn loupe_texture(cx: &AppContext, frame: &crate::render::Frame) -> Option<TextureHandle> {
    static ID: std::sync::OnceLock<egui::Id> = std::sync::OnceLock::new();
    let id = *ID.get_or_init(|| egui::Id::unique("grainy-loupe"));
    if let Some((frame_id, texture)) = cx.egui.data(|d| d.get_temp::<(u64, TextureHandle)>(id)) {
        if frame_id == frame.id {
            return Some(texture);
        }
    }
    let rgba = frame.rgba();
    let expected = frame.preview.width.checked_mul(frame.preview.height)?.checked_mul(4)?;
    if rgba.len() != expected || expected == 0 {
        return None;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([frame.preview.width, frame.preview.height], &rgba);
    let texture = cx.egui.load_texture("loupe", image, TextureOptions::NEAREST);
    cx.egui.data_mut(|d| d.insert_temp(id, (frame.id, texture.clone())));
    Some(texture)
}

/// Sutherland-Hodgman: the convex polygon clipped to a rectangle.
fn clip_to(polygon: &[Pos2], rect: Rect) -> Vec<Pos2> {
    type Inside = fn(Pos2, Rect) -> bool;
    type Cross = fn(Pos2, Pos2, Rect) -> Pos2;
    let edges: [(Inside, Cross); 4] = [
        (
            |p, r| p.x >= r.left(),
            |a, b, r| {
                let t = (r.left() - a.x) / (b.x - a.x);
                Pos2::new(r.left(), a.y + t * (b.y - a.y))
            },
        ),
        (
            |p, r| p.x <= r.right(),
            |a, b, r| {
                let t = (r.right() - a.x) / (b.x - a.x);
                Pos2::new(r.right(), a.y + t * (b.y - a.y))
            },
        ),
        (
            |p, r| p.y >= r.top(),
            |a, b, r| {
                let t = (r.top() - a.y) / (b.y - a.y);
                Pos2::new(a.x + t * (b.x - a.x), r.top())
            },
        ),
        (
            |p, r| p.y <= r.bottom(),
            |a, b, r| {
                let t = (r.bottom() - a.y) / (b.y - a.y);
                Pos2::new(a.x + t * (b.x - a.x), r.bottom())
            },
        ),
    ];
    let mut output = polygon.to_vec();
    for (inside, cross) in edges {
        let input = std::mem::take(&mut output);
        for (i, &current) in input.iter().enumerate() {
            let previous = input[(i + input.len() - 1) % input.len()];
            match (inside(previous, rect), inside(current, rect)) {
                (true, true) => output.push(current),
                (true, false) => output.push(cross(previous, current, rect)),
                (false, true) => {
                    output.push(cross(previous, current, rect));
                    output.push(current);
                }
                (false, false) => {}
            }
        }
        if output.is_empty() {
            break;
        }
    }
    output
}

/// `draw_loupe`: the surroundings of the pointer enlarged [`LOUPE_ZOOM`] times in a circle, the picked spot
/// marked in its middle, and the colour there as it is shown.
fn draw_loupe(painter: &Painter, view: &ViewTransform, cx: &AppContext, pointer: Pos2) {
    let Some(frame) = cx.frame() else { return };
    let (w, h) = (frame.preview.width, frame.preview.height);
    if w == 0 || h == 0 || view.rect.width() <= 0.0 {
        return;
    }
    let radius = LOUPE_RADIUS;
    let dark = Color32::from_rgb(0x10, 0x12, 0x15);
    painter.circle_filled(pointer, radius, dark);
    // Remap through `tile_uv` when the frame holds a 1:1 viewport tile rather than the full image.
    let tile_rect = match frame.tile_uv {
        Some([u0, v0, u1, v1]) => Rect::from_min_max(
            Pos2::new(view.rect.left() + u0 * view.rect.width(), view.rect.top() + v0 * view.rect.height()),
            Pos2::new(view.rect.left() + u1 * view.rect.width(), view.rect.top() + v1 * view.rect.height()),
        ),
        None => view.rect,
    };
    if tile_rect.width() <= 0.0 || tile_rect.height() <= 0.0 {
        return;
    }
    // Screen points of the circle's edge and the texels they show: the spot is at the circle's centre.
    let across = (tile_rect.width() / w as f32) * LOUPE_ZOOM;
    let spot = Vec2::new(
        (pointer.x - tile_rect.left()) / tile_rect.width() * w as f32,
        (pointer.y - tile_rect.top()) / tile_rect.height() * h as f32,
    );
    let to_uv = |p: Pos2| {
        let texel = spot + (p - pointer) / across;
        Pos2::new(texel.x / w as f32, texel.y / h as f32)
    };
    let picture = Rect::from_min_max(
        pointer + (Vec2::ZERO - spot) * across,
        pointer + (Vec2::new(w as f32, h as f32) - spot) * across,
    );
    let circle: Vec<Pos2> = (0..96)
        .map(|i| {
            let a = i as f32 / 96.0 * std::f32::consts::TAU;
            pointer + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    let shown = clip_to(&circle, picture);
    if shown.len() >= 3 {
        if let Some(texture) = loupe_texture(cx, frame) {
            let mut mesh = epaint::Mesh::with_texture(texture.id());
            for p in &shown {
                mesh.vertices.push(epaint::Vertex { pos: *p, uv: to_uv(*p), color: Color32::WHITE });
            }
            for i in 1..shown.len() as u32 - 1 {
                mesh.indices.extend([0, i, i + 1]);
            }
            painter.add(Shape::mesh(mesh));
        }
    }
    painter.circle_stroke(pointer, radius, Stroke::new(4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 150)));
    painter.circle_stroke(pointer, radius, Stroke::new(2.0, Color32::from_rgb(0xf2, 0xf2, 0xf2)));
    let mark = Rect::from_center_size(pointer, Vec2::splat(9.0));
    painter.rect_stroke(
        mark,
        0.0,
        Stroke::new(3.0, Color32::from_rgba_unmultiplied(0, 0, 0, 200)),
        egui::StrokeKind::Middle,
    );
    painter.rect_stroke(mark, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
    // The colour under the spot, as shown.
    let rgba = frame.rgba();
    let x = (spot.x.max(0.0) as usize).min(w - 1);
    let y = (spot.y.max(0.0) as usize).min(h - 1);
    let at = (y * w + x) * 4;
    let byte = |k: usize| rgba.get(at + k).copied().unwrap_or(0);
    let text = format!("R {}   G {}   B {}", byte(0), byte(1), byte(2));
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), Color32::from_rgb(0xf2, 0xf2, 0xf2));
    let wide = galley.size().x + 18.0;
    let clip = painter.clip_rect();
    let below = pointer.y - clip.top() + radius + 30.0 < clip.height();
    let top = if below { pointer.y + radius + 8.0 } else { pointer.y - radius - 30.0 };
    let label = Rect::from_min_size(Pos2::new(pointer.x - wide / 2.0, top), Vec2::new(wide, 22.0));
    painter.rect_filled(label, 0.0, Color32::from_rgba_unmultiplied(16, 18, 21, 220));
    painter.galley(Align2::CENTER_CENTER.align_size_within_rect(galley.size(), label).min, galley, Color32::WHITE);
}
