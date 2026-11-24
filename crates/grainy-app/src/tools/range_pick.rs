//! Colour and luminance ranges picked on the photo (색상 범위, 광도 범위), their editor's edits (범위 다듬기), the
//! sky's (하늘 다듬기) and the overlay of layers that read the developed image. Port of luma/studio.py
//! `pick_range` (with luma/range_mask.py `sample_selection` on a background job), `range_changed`,
//! `remove_range_sample`, `sky_changed`, `add_luma`, the pick note of `mode`, and luma/widgets.py `PhotoView` in
//! the modes `color_range`, `color_range_add` and `luma_range` (a click or a rectangle; Shift adds colours).
//!
//! The sampling reads the image a layer starts from. The engine does not hand out its layer inputs
//! (`on_layer_input`), so [`layer_input`] develops the settings with the masks before the layer only and the
//! working output: the same pixels, clipped to 0–1 as the develop's output is (the oracle reads them unclipped;
//! an HDR edit reads its limited rendition). See the package report's requests.

use std::path::Path;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Key, Modifiers, Painter, Rect, Stroke, StrokeKind};
use grainy_core::{normalized, Image, Plane, Settings};
use grainy_engine::files::{load_photo, Loaded, Photo};
use grainy_engine::{develop, Crop, Options, Output, Source};
use grainy_geometry::{LensModel, UseCrop};
use grainy_local::RangeSample;
use serde_json::{json, Map, Value};

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::AppContext;
use crate::imagespace::ViewTransform;
use crate::masks::layers::{self, Obj};
use crate::masks::overlay::{self, OverlayInput, OverlayPicture};
use crate::masks::{actions, MaskState, Mode, Operation};
use crate::panels::Tool;
use crate::text::tr_with;

/// The source size `pick_range` samples (the develop view's `source`, `load_image(path, 1800)`).
pub const SOURCE_SIZE: usize = 1800;
/// At most five colours in one colour range.
pub const MAX_COLORS: usize = 5;

/// `range_pick_note` before anything happened.
pub const NOTE_START: &str = "점이나 작은 사각형을 선택하세요. Shift+클릭: 같은 범위에 색 추가(최대 5개) · Esc: 종료";
/// The note of a picking mode (`mode`).
pub const NOTE_MODE: &str = "사진의 점이나 작은 사각형을 선택하세요. Shift+클릭으로 색을 더하고 Esc로 마칩니다.";
const NOTE_NO_COLOR: &str = "목록에서 색상 범위를 먼저 선택하세요.";
const NOTE_FULL: &str = "색은 최대 5개입니다. 고른 색을 제외한 뒤 추가하세요.";
const NOTE_READING: &str = "마스크를 적용하기 전 색과 밝기를 읽는 중…";
const NOTE_CHANGED: &str = "대상이 바뀌어 선택을 취소했습니다. 다시 선택하세요.";
const NOTE_COLOR: &str = "색을 선택했습니다. Shift+클릭으로 추가하거나 목록에서 범위를 다듬으세요.";
const NOTE_LUMA: &str = "밝기 범위를 선택했습니다. 목록에서 범위와 경계 흐림을 다듬으세요.";

/// What the range tools keep between frames.
#[derive(Debug, Clone)]
pub struct RangeState {
    /// `range_pick_note` (Korean source text, or an error's own text).
    pub note: String,
    /// `color_pick_generation`.
    pub generation: u64,
    /// `color_pick_token` while `color_pick_running`.
    pub running: Option<u64>,
    /// `range_samples.currentIndex()`.
    pub sample: usize,
    /// `tool_shift` of the pick being drawn.
    shift: bool,
}

impl Default for RangeState {
    fn default() -> Self {
        Self { note: NOTE_START.to_owned(), generation: 0, running: None, sample: 0, shift: false }
    }
}

fn st(cx: &mut AppContext) -> &mut RangeState {
    cx.state::<RangeState>()
}

/// The part of `StudioTools.mode` that belongs to the brush and the ranges, run first by [`actions::mode`]: the pick
/// generation moves on (a pick still running is dropped), 색상 범위 추가 needs a selected colour range, a picking
/// mode shows its note and the brush takes the slider's size again. Returns whether the mode may start.
pub fn mode_starts(cx: &mut AppContext, kind: Mode) -> bool {
    st(cx).generation += 1;
    if kind == Mode::ColorRangeAdd && component_type(cx).as_deref() != Some("color") {
        st(cx).note = NOTE_NO_COLOR.to_owned();
        return false;
    }
    if kind.picking() {
        st(cx).note = NOTE_MODE.to_owned();
    }
    if kind == Mode::Brush {
        super::brush::forget_size(cx);
    }
    true
}

fn component_type(cx: &mut AppContext) -> Option<String> {
    actions::current_component(cx).and_then(|c| c.get("type").and_then(Value::as_str).map(str::to_owned))
}

/// 선택한 범위에 색 추가 (`mode('color_range_add')`).
pub fn range_add_mode(cx: &mut AppContext) {
    actions::mode(cx, Mode::ColorRangeAdd);
}

/// A pick on its way: what `pick_range` captured before its job (the `valid` check and `ready` read these).
#[derive(Debug, Clone)]
pub struct Pick {
    document: u64,
    pub token: u64,
    pub photo: u64,
    pub settings: Settings,
    /// The mask sampled (`None`: a new one after the last).
    pub layer: Option<usize>,
    pub selection: isize,
    pub mode: Mode,
    pub color: bool,
    pub append: bool,
    pub component: Obj,
    pub samples: Vec<Value>,
    pub operation: Operation,
    pub tolerance: f64,
    pub points: Vec<[f64; 2]>,
}

/// The first part of `pick_range`: the checks, the commit and the captured state; `None` when nothing is picked
/// (five colours already).
pub fn prepare(cx: &mut AppContext, points: &[[f64; 2]], mode: Mode, add: bool) -> Option<Pick> {
    cx.doc.current()?;
    let color = mode != Mode::LumaRange;
    let component = actions::current_component(cx).unwrap_or_default();
    let append = color && add && component.get("type").and_then(Value::as_str) == Some("color");
    let samples = if append {
        match component.get("samples") {
            Some(Value::Array(list)) => list.clone(),
            Some(other) => vec![other.clone()],
            None => vec![component.get("rgb").cloned().unwrap_or_else(|| json!([0.5, 0.5, 0.5]))],
        }
    } else {
        Vec::new()
    };
    if samples.len() >= MAX_COLORS {
        st(cx).note = NOTE_FULL.to_owned();
        return None;
    }
    cx.doc.edit_mut().commit("보정 변경");
    let state = st(cx);
    state.generation += 1;
    let token = state.generation;
    state.running = Some(token);
    state.note = NOTE_READING.to_owned();
    let layer = actions::current_layer(cx).map(|_| actions::state(cx).layer as usize);
    let photo = cx.doc.current_id();
    let settings = cx.doc.edit().settings().clone();
    let document = cx.document_generation;
    let s = actions::state(cx);
    Some(Pick {
        document,
        token,
        photo,
        settings,
        layer,
        selection: s.component,
        mode,
        color,
        append,
        component,
        samples,
        operation: s.operation,
        tolerance: s.options.tolerance as f64 / 100.0,
        points: points.to_vec(),
    })
}

/// `valid()` of `pick_range`: the same photo, settings, selection and tool, and no newer pick.
fn valid(cx: &mut AppContext, pick: &Pick) -> bool {
    let generation = st(cx).generation;
    let layer = actions::current_layer(cx).map(|_| actions::state(cx).layer as usize);
    let (component, mode) = {
        let s = actions::state(cx);
        (s.component, s.mode)
    };
    pick.document == cx.document_generation
        && generation == pick.token
        && cx.doc.current_id() == pick.photo
        && component == pick.selection
        && layer == pick.layer
        && mode == pick.mode
        && cx.doc.edit().settings() == &pick.settings
}

fn close(a: &Value, b: &Value) -> bool {
    // np.allclose(rgb, old, atol=1e-6, rtol=0)
    match (a.as_array(), b.as_array()) {
        (Some(a), Some(b)) if a.len() == b.len() => a.iter().zip(b).all(|(x, y)| match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-6,
            _ => false,
        }),
        _ => false,
    }
}

/// `ready` and `failed` of `pick_range`: the sampled colours or brightness span become the range component (a new
/// one, or the selected colour range with the colours added), committed as 색상 범위 선택 / 밝기 범위 선택.
pub fn finish(cx: &mut AppContext, pick: &Pick, result: Result<RangeSample, String>) {
    let state = st(cx);
    let latest = state.running == Some(pick.token);
    if latest {
        state.running = None;
    }
    if !valid(cx, pick) {
        if latest {
            st(cx).note = NOTE_CHANGED.to_owned();
        }
        return;
    }
    let sample = match result {
        Ok(sample) => sample,
        Err(message) => {
            st(cx).note = message;
            return;
        }
    };
    let mut list = layers::list(&pick.settings);
    let target = match pick.layer {
        Some(index) => index,
        None => {
            let index = list.len();
            list.push(layers::new_layer(&tr_with("마스크 {0}", &[&(index + 1).to_string()])));
            index
        }
    };
    let value = if pick.color {
        let mut colors = pick.samples.clone();
        for rgb in &sample.colors {
            let rgb = Value::Array(rgb.iter().map(|v| Value::from(f64::from(*v))).collect());
            if colors.len() < MAX_COLORS && !colors.iter().any(|old| close(&rgb, old)) {
                colors.push(rgb);
            }
        }
        let first = colors.first().cloned().unwrap_or(Value::Null);
        if pick.append {
            let mut value = pick.component.clone();
            value.insert("samples".into(), Value::Array(colors));
            value.insert("rgb".into(), first);
            value
        } else {
            let mut value = Map::new();
            value.insert("type".into(), json!("color"));
            value.insert("samples".into(), Value::Array(colors));
            value.insert("rgb".into(), first);
            value.insert("tolerance".into(), json!(pick.tolerance));
            value.insert("operation".into(), json!(pick.operation.as_str()));
            value
        }
    } else {
        let mut value = Map::new();
        value.insert("type".into(), json!("luma"));
        value.insert("range".into(), json!([sample.range[0], sample.range[1]]));
        value.insert("softness".into(), json!(0.1));
        value.insert("operation".into(), json!(pick.operation.as_str()));
        value
    };
    let Some(layer) = layers::at_mut(&mut list, target as isize) else { return };
    let components = layers::components_mut(layer);
    let index = if pick.append {
        let at = pick.selection as usize;
        if at >= components.len() {
            return;
        }
        components[at] = Value::Object(value);
        at
    } else {
        components.push(Value::Object(value));
        components.len() - 1
    };
    actions::set_masks(cx, list);
    let s = actions::state(cx);
    s.layer = target as isize;
    s.component = index as isize;
    actions::sync(cx);
    super::brush::load_brush_range(cx);
    cx.doc.edit_mut().commit(if pick.color { "색상 범위 선택" } else { "밝기 범위 선택" });
    st(cx).note = (if pick.color { NOTE_COLOR } else { NOTE_LUMA }).to_owned();
}

/// `pick_range` with the sampling given (`work`): [`prepare`], the sampling, [`finish`] — at once.
pub fn pick_with(
    cx: &mut AppContext,
    points: &[[f64; 2]],
    mode: Mode,
    add: bool,
    work: impl FnOnce(&Pick) -> Result<RangeSample, String>,
) {
    if let Some(pick) = prepare(cx, points, mode, add) {
        let result = work(&pick);
        finish(cx, &pick, result);
    }
}

/// Port of `pick_range`: the sampling runs on a background job on the develop view's source.
pub fn pick_range(cx: &mut AppContext, points: &[[f64; 2]], mode: Mode, add: bool) {
    let Some((path, original)) = cx.doc.current().map(|p| (p.path.clone(), p.original)) else { return };
    let Some(pick) = prepare(cx, points, mode, add) else { return };
    let job = pick.clone();
    cx.jobs.spawn(
        "range_pick",
        "",
        move |_| {
            let loaded = load(&path, SOURCE_SIZE, &job.settings)?;
            let original = original.map(|(w, h)| (f64::from(w), f64::from(h)));
            sample(loaded.photo.source(), &job.settings, &job.points, job.layer, original)
        },
        move |cx: &mut AppContext, result| finish(cx, &pick, result),
    );
}

/// Port of `stroke` for the picking modes: the pick of the stroke's points (`add` for 색상 범위 추가 or Shift).
pub fn stroke(cx: &mut AppContext, points: &[[f64; 2]], shift: bool) {
    let mode = actions::state(cx).mode;
    if cx.doc.current().is_none() || !mode.picking() {
        return;
    }
    actions::cancel_stroke(cx);
    pick_range(cx, points, mode, mode == Mode::ColorRangeAdd || shift);
}

// ---- the develop the sampling and the overlay read -----------------------------------------------------------

/// Loaded sources, newest first (the develop view keeps its source; a job loads it once per photo and size).
static SOURCES: Mutex<Vec<(String, Arc<Loaded>)>> = Mutex::new(Vec::new());

/// The photo at `path` reduced to `size` on its long side (`load_image(path, size, …)`), loaded once.
pub fn load(path: &Path, size: usize, settings: &Settings) -> Result<Arc<Loaded>, String> {
    let s = normalized(&settings.0);
    let key = format!(
        "{}|{size}|{}|{}",
        path.display(),
        s.str("working_space"),
        grainy_engine::settings_key(&s, &grainy_engine::RAW_KEYS)
    );
    if let Ok(cache) = SOURCES.lock() {
        if let Some((_, loaded)) = cache.iter().find(|(k, _)| *k == key) {
            return Ok(loaded.clone());
        }
    }
    let loaded = Arc::new(load_photo(path, Some(size), &s).map_err(|e| e.to_string())?);
    if let Ok(mut cache) = SOURCES.lock() {
        cache.retain(|(k, _)| *k != key);
        cache.insert(0, (key, loaded.clone()));
        cache.truncate(3);
    }
    Ok(loaded)
}

/// The lens model of the settings' lens profile (as the develop builds it), if one is on.
fn lens(s: &Settings) -> Result<Option<grainy_lens::ProfileModel>, String> {
    if s.bool("lensfun_enabled") && s.bool("lensfun") {
        return grainy_lens::ProfileModel::from_settings(s).map(Some).map_err(|e| e.to_string());
    }
    Ok(None)
}

/// The frame geometry of an H×W×1 array (`engine.geometry(a, s, use_crop=True)`).
fn geometry(a: &Image, s: &Settings) -> Result<Image, String> {
    let model = lens(s)?;
    grainy_geometry::geometry(a, s, UseCrop::Yes, model.as_ref().map(|m| m as &dyn LensModel))
        .map(std::borrow::Cow::into_owned)
        .map_err(|e| e.to_string())
}

/// The image mask layer `layer` starts from (`on_layer_input`): the develop of `settings` with the masks before
/// it only, in the working space. Clipped to 0–1 like the develop's output.
pub fn layer_input(
    source: Source<'_>,
    settings: &Settings,
    layer: usize,
    original_size: Option<(f64, f64)>,
) -> Result<Image, String> {
    let mut s = settings.clone();
    let mut masks = layers::list(settings);
    masks.truncate(layer);
    s.0.insert("masks".into(), Value::Array(masks));
    let options = Options { use_crop: Crop::Cropped, output: Output::Working, original_size, ..Options::default() };
    develop(source, &s.0, &options).map_err(|e| e.to_string())
}

/// Port of `sample_selection(source, settings, points, layer, original_size=...)` through the app's develop
/// ([`layer_input`]): up to five colours and the brightness span of a point or rectangle.
pub fn sample(
    source: Source<'_>,
    settings: &Settings,
    points: &[[f64; 2]],
    layer: Option<usize>,
    original_size: Option<(f64, f64)>,
) -> Result<RangeSample, String> {
    let (h, w, _) = source.pixels().dim();
    let mut develop = |s: &Settings, read: &mut dyn FnMut(usize, &Image) -> std::ops::ControlFlow<()>| {
        let index = layer.unwrap_or_else(|| layers::list(s).len().saturating_sub(1));
        let image = layer_input(source, s, index, original_size).map_err(grainy_local::Error::Invalid)?;
        let _ = read(index, &image);
        Ok(())
    };
    let shape = |a: &Image, s: &Settings| geometry(a, s).map_err(grainy_local::Error::Invalid);
    grainy_local::sample_selection((h, w), settings, points, layer.map(|l| l as i64), &mut develop, &shape)
        .map_err(|e| e.to_string())
}

// ---- the editors' edits ----------------------------------------------------------------------------------------

fn replace_live(cx: &mut AppContext, component: Obj) {
    let (layer, index) = {
        let s = actions::state(cx);
        (s.layer, s.component)
    };
    let mut list = actions::masks(cx);
    let Some(target) = layers::at_mut(&mut list, layer) else { return };
    let Some(slot) = usize::try_from(index).ok().and_then(|i| layers::components_mut(target).get_mut(i)) else {
        return;
    };
    *slot = Value::Object(component);
    actions::set_masks(cx, list);
}

/// Python's `max(a, b)` / `min(a, b)` on stored numbers: the first of equal values.
fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// Port of `range_changed`: a slider of 범위 다듬기 moved (`low`, `high`, `softness` of a luminance range,
/// `tolerance` of a colour range), live; the slider's release commits.
pub fn range_changed(cx: &mut AppContext, key: &str, value: f64) {
    let Some(mut component) = actions::current_component(cx) else { return };
    let kind = component.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
    match (kind.as_str(), key) {
        ("color", "tolerance") => {
            component.insert("tolerance".into(), json!(value));
        }
        ("luma", "softness") => {
            component.insert("softness".into(), json!(value / 100.0));
        }
        ("luma", "low" | "high") => {
            let range = component.get("range").and_then(Value::as_array).cloned();
            let (mut low, mut high) = match range.as_deref() {
                Some([a, b]) => (a.clone(), b.clone()),
                _ => (json!(0.25), json!(0.75)),
            };
            if key == "low" {
                low = json!(value / 100.0);
                // max(low, high): `high` only when it is greater.
                let greater = number(&high) > number(&low);
                if !greater {
                    high = low.clone();
                }
            } else {
                high = json!(value / 100.0);
                if number(&low) > number(&high) {
                    low = high.clone();
                }
            }
            component.insert("range".into(), json!([low, high]));
        }
        _ => return,
    }
    replace_live(cx, component);
}

/// Port of `sky_changed`: 민감도 or 경계 흐림 of the selected sky, live.
pub fn sky_changed(cx: &mut AppContext, key: &str, value: f64) {
    let Some(mut component) = actions::current_component(cx) else { return };
    if component.get("type").and_then(Value::as_str) != Some("sky") {
        return;
    }
    component.insert(key.into(), json!(value));
    replace_live(cx, component);
}

/// Port of `remove_range_sample` (고른 색 제외): the colour chosen in the list leaves the selected colour range.
pub fn remove_range_sample(cx: &mut AppContext) {
    let index = st(cx).sample;
    let Some(mut component) = actions::current_component(cx) else { return };
    let mut samples = component.get("samples").and_then(Value::as_array).cloned().unwrap_or_default();
    if component.get("type").and_then(Value::as_str) != Some("color") || samples.len() <= 1 || index >= samples.len() {
        return;
    }
    samples.remove(index);
    component.insert("rgb".into(), samples[0].clone());
    component.insert("samples".into(), Value::Array(samples));
    actions::replace_component(cx, component);
    st(cx).sample = 0;
    super::brush::load_brush_range(cx);
}

/// Port of `add_luma`: a luminance range between two 0–100 slider positions (unused in the oracle's panel: its
/// 최소 밝기 / 최대 밝기 sliders are never shown; kept for the same behaviour).
pub fn add_luma(cx: &mut AppContext, low: i64, high: i64) {
    let (a, b) = (low as f64 / 100.0, high as f64 / 100.0);
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut component = Map::new();
    component.insert("type".into(), json!("luma"));
    component.insert("range".into(), json!([lo, hi]));
    component.insert("softness".into(), json!(0.1));
    actions::add_component(cx, component);
    super::brush::load_brush_range(cx);
    cx.doc.edit_mut().commit("보정 변경");
}

// ---- the overlay of layers that read the developed image ---------------------------------------------------

/// `sky_mask.sky_mask(source, component, transform)` from grainy-sky (as the engine's develop uses it).
struct Sky;

impl grainy_local::SkyMask for Sky {
    fn sky_mask(
        &self,
        source: &Image,
        component: &Map<String, Value>,
        transform: Option<grainy_local::Transform<'_>>,
    ) -> grainy_local::Result<Plane> {
        let component = Value::Object(component.clone());
        let moved = transform.map(|transform| {
            move |mask: &Image| transform(mask).map_err(|error| grainy_sky::Error::Input(error.to_string()))
        });
        let moved = moved.as_ref().map(|transform| transform as grainy_sky::Transform<'_>);
        grainy_sky::sky_mask(source, &component, moved).map_err(|error| grainy_local::Error::Invalid(error.to_string()))
    }
}

fn truthy(value: Option<&Value>) -> bool {
    layers::truthy(value, false)
}

/// Whether a layer's overlay reads the developed image: a range or a sky ([`overlay::needs_image`]) or an
/// automatic brush mask.
pub fn needs_image(layer: &Obj) -> bool {
    overlay::needs_image(layer)
        || layers::components(layer).iter().any(|c| {
            layers::truthy(c.get("enabled"), true)
                && c.get("type").and_then(Value::as_str) == Some("brush")
                && truthy(c.get("auto_mask"))
        })
}

fn needs_colour(layer: &Obj) -> bool {
    layers::components(layer)
        .iter()
        .any(|c| c.get("type").and_then(Value::as_str) == Some("sky") || truthy(c.get("auto_mask")))
}

/// The developed colour source of the masks (`to_srgb(linear)`).
fn colour_source(photo: &Photo, s: &Settings) -> Result<Image, String> {
    let linear = match photo {
        Photo::Linear(image) => image.clone(),
        Photo::Camera(camera) => grainy_raw::rawcolor::develop_camera(camera, s).map_err(|e| e.to_string())?,
    };
    let linear = if s.bool("negative") { grainy_film::negative::apply(&linear, s) } else { linear };
    Ok(grainy_color::to_srgb(&linear))
}

/// The overlay of a layer that reads the developed image: its mask on the image it starts from (live size).
pub fn compute_image_overlay(
    source: &Photo,
    settings: &Settings,
    layer: usize,
    original_size: Option<(f64, f64)>,
) -> Result<Option<OverlayPicture>, String> {
    let s = grainy_engine::working_settings(&settings.0);
    let masks = layers::list(&s);
    let Some(object) = layers::at(&masks, layer as isize) else { return Ok(None) };
    let (h, w, _) = source.pixels().dim();
    let image = layer_input(source.source(), settings, layer, original_size)?;
    let colour = if needs_colour(object) { Some(colour_source(source, &s)?) } else { None };
    let transform =
        |mask: &Image| -> grainy_local::Result<Image> { geometry(mask, &s).map_err(grainy_local::Error::Invalid) };
    let mask = grainy_local::layer_mask(&image, (h, w), object, &transform, colour.as_ref(), Some(&Sky))
        .map_err(|e| e.to_string())?;
    let (height, width) = mask.dim();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for &m in &mask {
        rgba.extend_from_slice(&overlay::RED);
        rgba.push((m * overlay::ALPHA) as u8);
    }
    Ok(Some(OverlayPicture { width, height, rgba }))
}

/// The overlay hook of [`overlay::refresh`]: a layer that reads the developed image is computed here (its job
/// develops the photo); returns whether it was (otherwise the masks package computes a shape layer's).
pub(crate) fn image_overlay(cx: &mut AppContext, input: &OverlayInput) -> bool {
    let s = grainy_core::effective(&input.settings);
    let masks = layers::list(&s);
    let Some(layer) = layers::at(&masks, input.layer as isize) else { return false };
    if !needs_image(layer) {
        return false;
    }
    let Some((path, original)) = cx.doc.current().map(|p| (p.path.clone(), p.original)) else { return false };
    let key = format!("image|{}|{}|{}", path.display(), input.layer, Value::Object(input.settings.0.clone()));
    let state = actions::state(cx);
    if state.overlay.shown.as_ref() == Some(&key) || state.overlay.running.is_some() {
        return true;
    }
    state.overlay.running = Some(key.clone());
    let (settings, index) = (input.settings.clone(), input.layer);
    let photo = cx.doc.current_id();
    cx.spawn_document_job(
        "mask_overlay",
        "",
        move |_| overlay_job(&path, original, &settings, index),
        move |cx: &mut AppContext, result| {
            if cx.doc.current_id() != photo {
                return;
            }
            if actions::state(cx).overlay.running.as_ref() != Some(&key) {
                return;
            }
            let picture = result.unwrap_or_else(|message| {
                log::warn!("mask overlay: {message}");
                None
            });
            let texture = picture.map(|p| {
                let image = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                cx.egui.load_texture("mask_overlay", image, egui::TextureOptions::LINEAR)
            });
            let state = actions::state(cx);
            state.overlay.running = None;
            state.overlay.texture = texture;
            state.overlay.shown = Some(key);
            cx.egui.request_repaint();
        },
    );
    true
}

fn overlay_job(
    path: &Path,
    original: Option<(u32, u32)>,
    settings: &Settings,
    layer: usize,
) -> Result<Option<OverlayPicture>, String> {
    let loaded = load(path, overlay::LIVE_SIZE, settings)?;
    let original = original.map(|(w, h)| (f64::from(w), f64::from(h)));
    compute_image_overlay(&loaded.photo, settings, layer, original)
}

// ---- the canvas tool -------------------------------------------------------------------------------------------

/// The range picker on the photo view (a click or a rectangle).
#[derive(Default)]
pub struct RangePick;

fn inside(p: [f64; 2]) -> bool {
    (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1])
}

/// The outline of the rectangle being drawn (`QColor(245, 100, 70, 150)`, 2 px).
const OUTLINE: Color32 = Color32::from_rgba_unmultiplied_const(245, 100, 70, 150);

impl CanvasTool for RangePick {
    fn name(&self) -> &'static str {
        "mask_range"
    }

    fn tool(&self) -> Option<Tool> {
        Some(Tool::Masks)
    }

    fn active(&self, cx: &AppContext) -> bool {
        cx.tool() == Tool::Masks
            && cx.state_ref::<MaskState>().is_some_and(|s| s.mode.picking() && s.photo == cx.doc.current_id())
    }

    fn edits(&self) -> &'static [&'static str] {
        &["masks"]
    }

    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        if cx.doc.current().is_none() {
            return false;
        }
        actions::sync(cx);
        match event.kind {
            PointerKind::Press => {
                if !inside(event.free) {
                    return false;
                }
                actions::state(cx).points = vec![event.frame];
                st(cx).shift = event.modifiers.shift;
            }
            PointerKind::Drag => {
                let s = actions::state(cx);
                if !s.points.is_empty() && s.points.last() != Some(&event.frame) {
                    s.points.push(event.frame);
                }
            }
            PointerKind::Release => {
                let s = actions::state(cx);
                if s.points.is_empty() {
                    return true;
                }
                if s.points.last() != Some(&event.frame) {
                    s.points.push(event.frame);
                }
                let points = std::mem::take(&mut s.points);
                let shift = st(cx).shift;
                stroke(cx, &points, shift);
            }
            PointerKind::Click => {
                if !inside(event.free) {
                    return false;
                }
                actions::state(cx).points.clear();
                stroke(cx, &[event.frame], event.modifiers.shift);
            }
            PointerKind::Hover => return false,
        }
        true
    }

    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        if key != Key::Escape || cx.doc.current().is_none() {
            return false;
        }
        if actions::state(cx).points.is_empty() {
            actions::set_tool(cx, Mode::None);
        } else {
            actions::state(cx).points.clear();
        }
        true
    }

    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        overlay::paint(painter, view, cx);
        let Some(state) = cx.state_ref::<MaskState>() else { return };
        if let (Some(first), Some(last)) = (state.points.first(), state.points.last()) {
            let rect = Rect::from_two_pos(view.to_screen(*first), view.to_screen(*last));
            painter.rect_stroke(rect, 0.0, Stroke::new(2.0, OUTLINE), StrokeKind::Middle);
        }
    }
}
