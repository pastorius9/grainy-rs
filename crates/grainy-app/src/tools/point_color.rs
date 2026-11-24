//! Point colours (포인트 색상) of the colour mixer: pick a colour on the photo, the list of points, each point's
//! ranges and shifts, and the blue 선택 색상 범위 표시 overlay of what the selected point covers.
//!
//! Ports of luma/studio.py `mode('point_color')`, `stroke` (the pick's click and its `source_points` check),
//! `pick_point_color` (a background job), `load_points`, `load_point`, `point_changed` (a version-1 point becomes
//! version 2 when one of its new ranges is moved), `remove_point`, `point_overlay_target`; of luma/point_color.py
//! `sample`, `preview`, `preview_key` and `_probe`; and of the overlay of luma/app.py `start_preview` (blue
//! `[65, 170, 255]`, alpha `coverage * 150`, on a 720-pixel picture).
//!
//! The oracle reaches the image a point works on through `develop(..., on_point=...)`. The engine crate has no
//! such hook, so [`probe`] runs the same stages up to the colour stage here (RAW colour, negative, retouch, lens
//! shading, geometry, tone with the film look) and takes the image from grainy-colorgrade's point hook.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CursorIcon, Key, Modifiers, Painter, Pos2, Rect};
use grainy_core::{Image, Plane, Settings};
use grainy_engine::files::{load_photo, Photo};
use grainy_engine::{Externals, Source};
use grainy_geometry::UseCrop;
use grainy_i18n::tr;
use ndarray::{Array2, ArrayView3};
use serde_json::{json, Map, Value};

use super::targeted::TargetedAdjust;
use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::{AppContext, Mode};
use crate::imagespace::{CoordinateMap, ViewTransform};
use crate::panels::Tool;
use crate::state::{settings_eq, EditState};

/// The history label of a pick (`'선택 색상 추가'`).
pub const PICK_LABEL: &str = "선택 색상 추가";
/// The note under the pick button before anything happened.
pub const FIRST_NOTE: &str = "사진에서 색을 고른 뒤 범위를 조절하세요.";
/// The overlay colour (`rgba[..., 0..3] = 65, 170, 255`) and its alpha scale (`coverage * 150`).
pub const BLUE: [u8; 3] = [65, 170, 255];
pub const ALPHA: f32 = 150.0;
/// The overlay's long side (`resize_float(..., 720)`).
pub const OVERLAY_SIZE: usize = 720;

/// The six sliders of a point: key, Korean label, low, high, scale, double-click value (`default_value`).
pub const CONTROLS: [(&str, &str, f64, f64, f64, f64); 6] = [
    ("range", "색상 범위", 0.01, 0.5, 100.0, 0.12),
    ("saturation_range", "채도 범위", 0.01, 1.0, 100.0, 0.6),
    ("lightness_range", "명도 범위", 0.01, 1.0, 100.0, 0.5),
    ("hue", "색상", -100.0, 100.0, 1.0, 0.0),
    ("saturation", "채도", -100.0, 100.0, 1.0, 0.0),
    ("lightness", "명도", -100.0, 100.0, 1.0, 0.0),
];

/// What `load_point` shows for a key a point does not have (a version-1 point has no lightness range: 1).
pub fn shown_default(key: &str) -> f64 {
    match key {
        "range" => 0.12,
        "saturation_range" => 0.6,
        "lightness_range" => 1.0,
        _ => 0.0,
    }
}

// ---- the list of points ------------------------------------------------------------------------------------------

/// The photo's point colours (`settings['point_colors']`).
pub fn points(settings: &Settings) -> Vec<Value> {
    settings.value("point_colors").as_array().cloned().unwrap_or_default()
}

/// The panel's state of the point colours, kept in the context (`cx.state::<PointColors>()`).
#[derive(Default)]
pub struct PointColors {
    /// `point_index`: the selected point (−1: none).
    pub index: isize,
    /// `point_note`, Korean source text (or the oracle's error text).
    pub note: Option<String>,
    /// The pick tool is on (`view.tool_mode == 'point_color'`).
    pub picking: bool,
    /// `color_pick_generation`: every tool change makes older picks stale.
    pub generation: u64,
    /// `color_pick_token`: the newest pick started.
    pub token: u64,
    /// `color_pick_running`.
    pub running: bool,
    /// The press of a pick, in frame fractions (`tool_points[0]`).
    pub pressed: Option<[f64; 2]>,
    /// The 선택 색상 범위 표시 check box.
    pub overlay_on: bool,
    pub(crate) overlay: Overlay,
    pub(crate) source: Option<Arc<Loaded>>,
}

impl PointColors {
    /// The note shown under the pick button.
    pub fn note(&self) -> &str {
        self.note.as_deref().unwrap_or(FIRST_NOTE)
    }

    /// Port of `load_points` + `load_point`: the selection kept within the list (−1 for an empty list).
    pub fn load_points(&mut self, settings: &Settings) {
        let count = points(settings).len() as isize;
        self.index = self.index.max(0).min(count - 1);
    }

    /// Port of `load_point` after the combo box chose `index`.
    pub fn select(&mut self, index: isize) {
        self.index = index;
    }

    /// The selected point, if any.
    pub fn selected(&self, settings: &Settings) -> Option<Map<String, Value>> {
        let list = points(settings);
        usize::try_from(self.index).ok().and_then(|i| list.get(i)).and_then(Value::as_object).cloned()
    }

    /// Port of `load_point`'s values: what the six sliders show, and whether they are enabled.
    pub fn shown(&self, settings: &Settings) -> ([f64; 6], bool) {
        let point = self.selected(settings);
        let values = CONTROLS.map(|(key, ..)| {
            point.as_ref().and_then(|p| p.get(key)).and_then(Value::as_f64).unwrap_or_else(|| shown_default(key))
        });
        (values, point.is_some())
    }

    /// Port of `point_changed`: sets `key` of the selected point (live, no commit). Moving a new range of a
    /// version-1 point first makes it version 2 with the old fixed widths (`saturation_range .6`,
    /// `lightness_range 1`).
    pub fn changed(&self, edit: &mut EditState, key: &str, value: f64) {
        let mut list = points(edit.settings());
        let Some(point) = usize::try_from(self.index).ok().and_then(|i| list.get_mut(i)).and_then(Value::as_object_mut)
        else {
            return;
        };
        let version = point.get("version").and_then(Value::as_f64).unwrap_or(1.0);
        if (key == "saturation_range" || key == "lightness_range") && version < 2.0 {
            point.insert("version".into(), json!(2));
            point.insert("saturation_range".into(), json!(0.6));
            point.insert("lightness_range".into(), json!(1));
        }
        point.insert(key.to_owned(), json!(value));
        edit.set("point_colors", Value::Array(list));
    }

    /// Port of `remove_point` (and the commit its `set_setting` starts): the selected point removed.
    pub fn remove(&mut self, edit: &mut EditState) {
        let mut list = points(edit.settings());
        let Some(index) = usize::try_from(self.index).ok().filter(|i| *i < list.len()) else { return };
        list.remove(index);
        edit.set("point_colors", Value::Array(list));
        edit.commit("보정 변경");
        self.load_points(edit.settings());
    }

    /// Port of `pick_point_color`'s `ready` once the colour is read: a new version-2 point, selected, committed.
    pub fn added(&mut self, edit: &mut EditState, rgb: [f32; 3]) {
        let mut list = points(edit.settings());
        list.push(new_point(rgb));
        self.index = list.len() as isize - 1;
        edit.set("point_colors", Value::Array(list));
        self.load_points(edit.settings());
        edit.commit(PICK_LABEL);
        self.note = Some("색을 추가했습니다. 범위 표시로 적용될 영역을 확인하세요.".into());
    }
}

/// The point a pick adds (`dict(version=2, rgb=rgb, range=.12, saturation_range=.6, lightness_range=.5, hue=0,
/// saturation=0, lightness=0)`).
pub fn new_point(rgb: [f32; 3]) -> Value {
    json!({
        "version": 2,
        "rgb": rgb.map(f64::from),
        "range": 0.12,
        "saturation_range": 0.6,
        "lightness_range": 0.5,
        "hue": 0,
        "saturation": 0,
        "lightness": 0,
    })
}

// ---- the image a point works on ------------------------------------------------------------------------------

const OUTSIDE: &str = "사진 안쪽의 색을 골라 주세요.";
const NOT_REACHED: &str = "마스크를 켜고 선택 영역 안쪽의 색을 골라 주세요.";

/// `cv2` inpainting and distance transform for the retouch stage (grainy-imgproc), as the engine's own.
struct Cv;

impl grainy_local::Inpaint for Cv {
    fn inpaint_ns(&self, plane: &Plane, mask: &Array2<u8>, radius: f64) -> grainy_local::Result<Plane> {
        grainy_imgproc::inpaint(plane, mask, radius).map_err(|e| grainy_local::Error::Invalid(e.to_string()))
    }
}

impl grainy_local::DistanceTransform for Cv {
    fn distance_l2(&self, mask: &Array2<u8>) -> grainy_local::Result<Plane> {
        grainy_imgproc::distance_transform(mask, 3).map_err(|e| grainy_local::Error::Invalid(e.to_string()))
    }
}

/// The tone stage with the film look (the engine's private `tone`).
fn tone(frame: &Image, s: &Settings) -> Result<Image, String> {
    if !grainy_film::active(s) {
        return grainy_tone::develop_tone(frame, s, None).map_err(|e| e.to_string());
    }
    let name = s.str("film_look");
    let space = grainy_film::WorkingSpace::from_setting(s.str("working_space"));
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let render = |pixels: ArrayView3<'_, f32>| -> Image {
        match grainy_film::render(&pixels.to_owned(), name, space) {
            Ok(print) => print,
            Err(error) => {
                if let Ok(mut slot) = failure.lock() {
                    slot.get_or_insert(error.to_string());
                }
                Image::zeros(pixels.raw_dim())
            }
        }
    };
    let toned = grainy_tone::develop_tone(frame, s, Some(&render)).map_err(|e| e.to_string())?;
    match failure.into_inner() {
        Ok(Some(error)) => Err(error),
        _ => Ok(toned),
    }
}

/// Port of luma/point_color.py `_probe` for the photo's own point colours (`layer=None`): the image point
/// `index` of `point_colors` works on (`index == len(point_colors)`: the colour stage's image after the last
/// point), cropped. The colour mixer is switched on for the probe (`panels_off` without `mixer`). Errors are the
/// oracle's messages or the stage's.
pub fn probe(
    source: Source<'_>,
    settings: &Map<String, Value>,
    original_size: Option<(f64, f64)>,
    index: usize,
) -> Result<Image, String> {
    let mut map = settings.clone();
    let off: Vec<Value> = map
        .get("panels_off")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter(|p| p.as_str() != Some("mixer")).cloned().collect())
        .unwrap_or_default();
    map.insert("panels_off".into(), Value::Array(off));
    let s = grainy_engine::working_settings(&map);
    let profile_lens = if s.bool("lensfun_enabled") && s.bool("lensfun") {
        Some(grainy_lens::ProfileModel::from_settings(&s).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let lens = profile_lens.as_ref().map(|model| model as &dyn grainy_geometry::LensModel);
    let defaults = Externals::default();
    let externals = Externals { lens, ..defaults };
    if let Some(missing) = grainy_engine::pending(&s, &externals) {
        return Err(format!("not ported yet: {missing}"));
    }
    grainy_engine::pixel_scale(source.pixels(), original_size).map_err(|e| e.to_string())?;
    let camera;
    let linear: &Image = match source {
        Source::Camera(raw) => {
            camera = grainy_raw::rawcolor::develop_camera(raw, &s).map_err(|e| e.to_string())?;
            &camera
        }
        Source::Linear(image) => image,
    };
    let negative;
    let linear: &Image = if s.bool("negative") {
        negative = grainy_film::negative::apply(linear, &s);
        &negative
    } else {
        linear
    };
    // geometry(lens_shading(apply_retouch(linear, s['retouch']), s), s, use_crop)
    let operations = s.value("retouch").as_array().cloned().unwrap_or_default();
    let retouched;
    let base: &Image = if operations.is_empty() {
        linear
    } else {
        let tools = grainy_local::RetouchTools { inpaint: Some(&Cv), distance: Some(&Cv), dust: externals.dust };
        retouched = grainy_local::apply_retouch(linear, &operations, &tools).map_err(|e| e.to_string())?;
        &retouched
    };
    let shaded = grainy_geometry::lens::lens_shading(base, &s, externals.lens).map_err(|e| e.to_string())?;
    let frame = grainy_geometry::geometry(&shaded, &s, UseCrop::Yes, externals.lens).map_err(|e| e.to_string())?;
    let toned = tone(&frame, &s)?;
    let mut found: Option<Image> = None;
    let mut hook = |i: usize, image: &Image| {
        if i == index && found.is_none() {
            found = Some(image.clone());
        }
    };
    grainy_colorgrade::develop_color_with(&toned, &s, Some(&mut hook)).map_err(|e| e.to_string())?;
    found.ok_or_else(|| NOT_REACHED.to_owned())
}

/// Port of luma/point_color.py `sample` for the photo's point colours: the colour (clipped to 0–1) at the frame
/// point `position` of the image a new point would work on.
pub fn sample(
    source: Source<'_>,
    settings: &Map<String, Value>,
    position: [f64; 2],
    original_size: Option<(f64, f64)>,
) -> Result<[f32; 3], String> {
    if !position.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
        return Err(OUTSIDE.to_owned());
    }
    let count = settings.get("point_colors").and_then(Value::as_array).map_or(0, Vec::len);
    let image = probe(source, settings, original_size, count)?;
    let (h, w, _) = image.dim();
    // x = min(w-1, int(position[0]*w)) - origin[0]
    let x = ((position[0] * w as f64) as usize).min(w.saturating_sub(1));
    let y = ((position[1] * h as f64) as usize).min(h.saturating_sub(1));
    if x >= w || y >= h {
        return Err("선택한 마스크의 영역 안쪽을 클릭해 주세요.".to_owned());
    }
    Ok([0, 1, 2].map(|c| image[[y, x, c]].clamp(0.0, 1.0)))
}

/// Port of luma/point_color.py `preview` for the photo's point `index`: its coverage on the frame
/// (grainy-colorgrade `point_color::coverage` of the image it works on).
pub fn preview(
    source: Source<'_>,
    settings: &Map<String, Value>,
    index: usize,
    original_size: Option<(f64, f64)>,
) -> Result<Plane, String> {
    let point = settings
        .get("point_colors")
        .and_then(Value::as_array)
        .and_then(|list| list.get(index))
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "list index out of range".to_owned())?;
    let image = probe(source, settings, original_size, index)?;
    grainy_colorgrade::point_color::coverage(&image, &point).map_err(|e| e.to_string())
}

/// Port of `preview_key`: the selected point's shifts cannot change what it covers, so they (and the later
/// points) are left out of the key.
pub fn preview_key(settings: &Settings, index: usize) -> String {
    let mut value = settings.0.clone();
    let mut list = points(settings);
    list.truncate(index + 1);
    if let Some(Value::Object(point)) = list.get_mut(index) {
        for key in ["hue", "saturation", "lightness"] {
            point.remove(key);
        }
    }
    value.insert("point_colors".into(), Value::Array(list));
    format!("{index}|{}", Value::Object(value))
}

// ---- the photo's source ------------------------------------------------------------------------------------------

/// A loaded source of the open photo and its oriented, uncropped size.
pub struct Loaded {
    path: PathBuf,
    raw_key: String,
    max_size: Option<usize>,
    photo: Photo,
    original: (f64, f64),
}

fn load(path: &std::path::Path, max_size: Option<usize>, settings: &Settings) -> Result<Loaded, String> {
    let loaded = load_photo(path, max_size, settings).map_err(|e| e.to_string())?;
    let (h, w, _) = loaded.photo.pixels().dim();
    let size = |key: &str, fallback: usize| {
        loaded.metadata.get(key).and_then(Value::as_u64).map_or(fallback as f64, |v| v as f64)
    };
    let original = (size("width", w), size("height", h));
    Ok(Loaded { path: path.to_path_buf(), raw_key: raw_key(settings), max_size, photo: loaded.photo, original })
}

fn raw_key(settings: &Settings) -> String {
    format!("{}|{}", grainy_engine::settings_key(settings, &grainy_engine::RAW_KEYS), settings.value("camera_matrix"))
}

/// The cached source when it is the one asked for.
fn cached(
    state: &PointColors,
    path: &std::path::Path,
    max_size: Option<usize>,
    settings: &Settings,
) -> Option<Arc<Loaded>> {
    state
        .source
        .as_ref()
        .filter(|s| s.path == path && s.max_size == max_size && s.raw_key == raw_key(settings))
        .cloned()
}

fn source_for(
    have: Option<Arc<Loaded>>,
    path: &std::path::Path,
    max: Option<usize>,
    s: &Settings,
) -> Result<Arc<Loaded>, String> {
    match have {
        Some(source) => Ok(source),
        None => load(path, max, s).map(Arc::new),
    }
}

// ---- picking -----------------------------------------------------------------------------------------------------

fn state(cx: &mut AppContext) -> &mut PointColors {
    cx.state::<PointColors>()
}

/// The view is where the pick tool can be: Develop, the panels, no white-balance picker, no targeted adjustment.
fn on_screen(cx: &AppContext) -> bool {
    cx.mode() == Mode::Develop
        && cx.tool() == Tool::Edit
        && !cx.wb_picker
        && cx.state_ref::<TargetedAdjust>().is_none_or(|t| t.kind.is_none())
}

/// Another tool took the view (`set_tool` to another mode): the pick ends. The mixer panel calls it every frame.
pub fn sync(cx: &mut AppContext) {
    if !on_screen(cx) && cx.state_ref::<PointColors>().is_some_and(|s| s.picking) {
        let state = state(cx);
        state.picking = false;
        state.pressed = None;
        state.generation += 1;
    }
}

/// Port of `mode('point_color')`: the pick tool on (the before view, a crop, the white-balance picker and the
/// targeted adjustment end).
pub fn start(cx: &mut AppContext) {
    if cx.doc.current().is_none() {
        return;
    }
    cx.set_before(false);
    if cx.mode() != Mode::Develop {
        cx.set_mode(Mode::Develop);
    }
    if cx.tool() != Tool::Edit {
        cx.select_tool(Tool::Edit);
    }
    cx.wb_picker = false;
    cx.state::<TargetedAdjust>().stop();
    crate::tools::guides::stop(cx);
    let state = state(cx);
    state.generation += 1;
    state.picking = true;
    state.pressed = None;
    cx.set_status(tr("보정할 색을 사진에서 클릭하세요."));
}

/// `set_tool('')` (Esc): the pick tool off.
pub fn stop(cx: &mut AppContext) {
    let state = state(cx);
    if state.picking {
        state.picking = false;
        state.pressed = None;
        state.generation += 1;
    }
}

/// The live source's size (the coordinate map's), from the photo's original size.
fn live_shape(cx: &AppContext) -> Option<(usize, usize)> {
    let (w, h) = cx.doc.current()?.original?;
    Some(super::healing::live_size(w as usize, h as usize))
}

/// Port of `stroke` for the pick: the clicked frame point must show the photo (`source_points`), then
/// `pick_point_color(points[0])`.
pub fn clicked(cx: &mut AppContext, point: [f64; 2]) {
    if let Some((w, h)) = live_shape(cx) {
        let inside = CoordinateMap::new(w, h, cx.settings()).map(|map| !map.source_points(&[point]).is_empty());
        if matches!(inside, Ok(false)) {
            cx.set_status(tr("변형 후 생긴 빈 가장자리 대신 사진 안쪽을 선택하세요."));
            return;
        }
    }
    pick(cx, point);
}

/// Port of `pick_point_color` (photo point colours): commits the pending edit, ends the pick tool and reads the
/// colour on a background job; the point is added when the same photo is still open with the same settings.
pub fn pick(cx: &mut AppContext, point: [f64; 2]) {
    let Some(photo) = cx.doc.current() else { return };
    let (id, path) = (photo.id, photo.path.clone());
    cx.doc.edit_mut().commit("보정 변경");
    // mode(''): a new generation, the tool off.
    {
        let state = state(cx);
        state.picking = false;
        state.pressed = None;
        state.generation += 1;
    }
    let settings = cx.settings().clone();
    // The source on screen: the full-size one at 1:1, the reduced one otherwise.
    let max = match cx.frame().map(|f| f.resolution) {
        Some(crate::render::Resolution::Full) => None,
        _ => Some(crate::worker::PREVIEW_SIZE),
    };
    let reading = "보정 단계의 색을 읽는 중…";
    let have = {
        let state = state(cx);
        state.token = state.generation;
        state.running = true;
        state.note = Some(reading.into());
        cached(state, &path, max, &settings)
    };
    let token = cx.state::<PointColors>().token;
    let generation = cx.document_generation;
    cx.set_status(tr(reading));
    let input = settings.clone();
    cx.jobs.spawn(
        "point_color",
        tr(reading),
        move |_| {
            let source = source_for(have, &path, max, &input)?;
            let rgb = sample(source.photo.source(), &input.0, point, Some(source.original));
            Ok((source, rgb))
        },
        move |cx: &mut AppContext, result| {
            let valid = |cx: &AppContext| {
                cx.state_ref::<PointColors>().is_some_and(|s| s.generation == token)
                    && cx.document_generation == generation
                    && cx.doc.current_id() == id
                    && settings_eq(cx.settings(), &settings)
            };
            if cx.state::<PointColors>().token == token {
                cx.state::<PointColors>().running = false;
            }
            let outcome = match result {
                Ok((source, rgb)) => {
                    if source.max_size == Some(crate::worker::PREVIEW_SIZE) {
                        cx.state::<PointColors>().source = Some(source);
                    }
                    rgb
                }
                Err(message) => Err(message),
            };
            match outcome {
                Ok(rgb) => {
                    if !valid(cx) {
                        if cx.state::<PointColors>().token == token {
                            cx.state::<PointColors>().note =
                                Some("대상이 바뀌어 색 선택을 취소했습니다. 다시 선택하세요.".into());
                        }
                        return;
                    }
                    let mut state = std::mem::take(cx.state::<PointColors>());
                    state.added(cx.doc.edit_mut(), rgb);
                    *cx.state::<PointColors>() = state;
                    cx.set_before(false);
                    cx.set_status(tr("색을 추가했습니다. 색상·채도·명도 범위를 조절하고 범위 표시로 확인하세요."));
                }
                Err(message) => {
                    if valid(cx) {
                        cx.set_status(tr(&message));
                        cx.state::<PointColors>().note = Some(message);
                    }
                }
            }
        },
    );
}

// ---- the 선택 색상 범위 표시 overlay ----------------------------------------------------------------------------

/// The overlay's picture and what is being computed.
#[derive(Default)]
pub(crate) struct Overlay {
    texture: Option<egui::TextureHandle>,
    shown: Option<String>,
    running: Option<String>,
}

/// The overlay picture: RGBA rows, width, height.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayPicture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The overlay picture of a coverage: reduced to [`OVERLAY_SIZE`] (`resize_float`), blue with alpha
/// `uint8(coverage * 150)`.
pub fn overlay_picture(coverage: &Plane) -> OverlayPicture {
    let (h, w) = coverage.dim();
    let stacked = coverage.clone().into_shape_with_order((h, w, 1)).unwrap_or_else(|_| Image::zeros((1, 1, 1)));
    let small = grainy_codecs::resize_float(&stacked, OVERLAY_SIZE).unwrap_or(stacked);
    let (height, width, _) = small.dim();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for &m in &small {
        rgba.extend_from_slice(&BLUE);
        rgba.push((m * ALPHA) as u8);
    }
    OverlayPicture { width, height, rgba }
}

/// Port of `point_overlay_target` (photo points): the point whose coverage shows, if any — the mixer panel open,
/// the check box on, a point selected, not the before view.
pub fn overlay_target(cx: &AppContext) -> Option<usize> {
    let state = cx.state_ref::<PointColors>()?;
    if !state.overlay_on
        || cx.before()
        || cx.mode() != Mode::Develop
        || cx.tool() != Tool::Edit
        || !cx.expanded_panels().contains(&"mixer")
    {
        return None;
    }
    let index = usize::try_from(state.index).ok()?;
    (index < points(cx.settings()).len()).then_some(index)
}

/// Brings the overlay up to date: one job at a time; the newest wish is computed when it ends. Called every frame
/// the mixer panel is drawn.
pub fn refresh(cx: &mut AppContext) {
    let wanted = overlay_target(cx).zip(cx.doc.current().map(|p| p.path.clone()));
    let Some((index, path)) = wanted else {
        let state = state(cx);
        state.overlay.texture = None;
        state.overlay.shown = None;
        return;
    };
    let settings = cx.settings().clone();
    let key = format!("{}|{}|{}", path.display(), raw_key(&settings), preview_key(&settings, index));
    let max = Some(crate::worker::PREVIEW_SIZE);
    let have = {
        let state = state(cx);
        if state.overlay.shown.as_ref() == Some(&key) || state.overlay.running.is_some() {
            return;
        }
        state.overlay.running = Some(key.clone());
        cached(state, &path, max, &settings)
    };
    let photo = cx.doc.current_id();
    cx.spawn_document_job(
        "point_overlay",
        "",
        move |_| {
            let source = source_for(have, &path, max, &settings)?;
            // A point the oracle cannot evaluate shows nothing (`np.zeros((1, 1))`).
            let coverage = preview(source.photo.source(), &settings.0, index, Some(source.original))
                .unwrap_or_else(|_| Plane::zeros((1, 1)));
            Ok((source, overlay_picture(&coverage)))
        },
        move |cx: &mut AppContext, result| {
            if cx.doc.current_id() != photo {
                return;
            }
            if state(cx).overlay.running.as_ref() != Some(&key) {
                return;
            }
            let picture = match result {
                Ok((source, picture)) => {
                    cx.state::<PointColors>().source = Some(source);
                    Some(picture)
                }
                Err(message) => {
                    log::warn!("point colour overlay: {message}");
                    None
                }
            };
            let texture = picture.map(|p| {
                let image = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                cx.egui.load_texture("point_overlay", image, egui::TextureOptions::LINEAR)
            });
            let state = state(cx);
            state.overlay.running = None;
            state.overlay.texture = texture;
            state.overlay.shown = Some(key);
            cx.egui.request_repaint();
        },
    );
}

/// Whether the overlay picture for the current wish is on screen (tests).
pub fn overlay_ready(cx: &AppContext) -> bool {
    cx.state_ref::<PointColors>().is_some_and(|s| s.overlay.texture.is_some() && s.overlay.running.is_none())
}

// ---- the canvas tool ---------------------------------------------------------------------------------------------

/// The pick tool (active while picking) and the coverage overlay (drawn whatever tool is active).
pub struct PointColor;

impl CanvasTool for PointColor {
    fn name(&self) -> &'static str {
        "point_color"
    }

    fn active(&self, cx: &AppContext) -> bool {
        on_screen(cx) && cx.state_ref::<PointColors>().is_some_and(|s| s.picking)
    }

    fn edits(&self) -> &'static [&'static str] {
        &["point_colors"]
    }

    /// `PhotoView.mousePressEvent` / `mouseReleaseEvent` with a tool mode: the press must be on the photo; the
    /// release ends the one-point stroke.
    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        let inside = |p: [f64; 2]| (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]);
        match event.kind {
            PointerKind::Hover => false,
            PointerKind::Press => {
                if inside(event.free) {
                    state(cx).pressed = Some(event.frame);
                }
                true
            }
            PointerKind::Drag => true,
            PointerKind::Release => {
                if let Some(point) = state(cx).pressed.take() {
                    clicked(cx, point);
                }
                true
            }
            PointerKind::Click => {
                state(cx).pressed = None;
                if inside(event.free) {
                    clicked(cx, event.frame);
                }
                true
            }
        }
    }

    /// Esc leaves the tool.
    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        if key == Key::Escape {
            stop(cx);
            return true;
        }
        false
    }

    fn paint(&self, _painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        if cx.egui.pointer_hover_pos().is_some_and(|p| view.rect.contains(p)) {
            cx.egui.set_cursor_icon(CursorIcon::Crosshair);
        }
    }

    /// The coverage overlay of the selected point (`painter.drawImage(self.image_rect, self.tool_overlay)`).
    fn paint_passive(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        if overlay_target(cx).is_none() {
            return;
        }
        let Some(texture) = cx.state_ref::<PointColors>().and_then(|s| s.overlay.texture.as_ref()) else { return };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture.id(), view.rect, uv, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(points: Value) -> Settings {
        let mut s = grainy_core::defaults().clone();
        s.set("point_colors", points);
        s
    }

    #[test]
    fn selection_stays_in_the_list() {
        let mut state = PointColors { index: 5, ..PointColors::default() };
        state.load_points(&settings(json!([{}, {}])));
        assert_eq!(state.index, 1);
        state.load_points(&settings(json!([])));
        assert_eq!(state.index, -1);
        state.load_points(&settings(json!([{}])));
        assert_eq!(state.index, 0);
    }

    #[test]
    fn preview_key_ignores_the_shifts_and_later_points() {
        let a = settings(json!([{"rgb": [0.1, 0.2, 0.3], "hue": 10}, {"rgb": [0.5, 0.5, 0.5]}]));
        let b = settings(json!([{"rgb": [0.1, 0.2, 0.3], "hue": -40, "lightness": 3}]));
        assert_eq!(preview_key(&a, 0), preview_key(&b, 0));
        let c = settings(json!([{"rgb": [0.1, 0.2, 0.3], "range": 0.3}]));
        assert_ne!(preview_key(&a, 0), preview_key(&c, 0));
    }
}

#[test]
fn point_color_source_changes_with_raw_development() {
    let mut s = grainy_core::defaults();
    let key = raw_key(&s);
    s.set("raw_kelvin", 3150);
    assert_ne!(raw_key(&s), key);
}
