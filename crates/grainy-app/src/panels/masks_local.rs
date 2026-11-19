//! The fold-out 커브 · 색상 혼합 · 선택 색상 of the Masks panel: the selected mask's local curve (부분 커브 · RGB / 빨강
//! / 초록 / 파랑), its colour mixer (부분 색상표, eight colours × 색상표 색상 / 채도 / 명도) and its local point colours
//! (부분 선택 색상: pick on the photo inside the mask, the list, six sliders, remove, and the blue 부분 선택 색상 범위
//! 표시 overlay of what the selected point covers inside the mask).
//!
//! Ports of luma/studio.py `build_masks`' fold-out, `load_local_curve`, `local_curve_changed`, `load_local_hsl`,
//! `local_hsl_changed`, `local_point_values`, `load_local_points`, `load_local_point`, `local_point_changed`,
//! `remove_local_point`, `mode('local_point_color')`, `pick_point_color(local=True)` (a background job) and the local
//! part of `point_overlay_target`; of luma/point_color.py `sample`, `preview`, `preview_key` and `_probe` for a
//! mask layer.
//!
//! The oracle reaches the image a local point works on through `develop(..., on_point=...)`. The engine has no such
//! hook, so [`probe`] runs the develop's stages here (as `tools/point_color.rs` does for the photo's points) up to
//! the local adjustments and hands grainy-local's `apply_local` its `on_point` callback.
//!
//! The pick tool on the photo is [`LocalPointTool`] (registered before the Masks tool).

use std::path::PathBuf;
use std::sync::Mutex;

use eframe::egui::{self, Color32, CursorIcon, Key, Modifiers, Painter, Pos2, Rect, Ui};
use grainy_core::{Image, Plane, Settings};
use grainy_engine::{Externals, Source};
use grainy_geometry::{LensModel, UseCrop};
use grainy_local::{LocalContext, LocalHooks, PointInput};
use ndarray::{s, Array2, ArrayView3};
use serde_json::{json, Map, Value};

use super::controls;
use crate::adjustments::{Place, Reset, Spec};
use crate::context::{AppContext, Mode as ViewMode};
use crate::imagespace::ViewTransform;
use crate::masks::layers::{self, Obj};
use crate::masks::{actions, MaskState, Mode};
use crate::panels::Tool;
use crate::state::settings_eq;
use crate::text::{tr, tr_with};
use crate::tools::point_color::{new_point, overlay_picture};
use crate::tools::{CanvasTool, PointerEvent, PointerKind};
use crate::widgets::curve::{self, points_value, CurveEditor, Signal};
use crate::widgets::{self, Button};

/// The local curve's channels (`local_curve_channel`).
pub const CURVE_CHANNELS: [&str; 4] = ["부분 커브 · RGB", "부분 커브 · 빨강", "부분 커브 · 초록", "부분 커브 · 파랑"];
/// The mixer's colours (`local_hsl_channel`).
pub const COLOURS: [&str; 8] = ["빨강", "주황", "노랑", "초록", "청록", "파랑", "보라", "자홍"];
/// The mixer's three sliders (`local_hsl_controls`, keys `'0'`, `'1'`, `'2'`).
pub const HSL_TITLES: [&str; 3] = ["색상표 색상", "색상표 채도", "색상표 명도"];
/// The identity curve (luma/processing.py `IDENTITY`).
pub const IDENTITY: [[f64; 2]; 5] = [[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [0.75, 0.75], [1.0, 1.0]];

/// The six sliders of a local point: key, Korean label, low, high, scale, double-click value (`default_value`).
pub const POINT_CONTROLS: [(&str, &str, f64, f64, f64, f64); 6] = [
    ("range", "선택 색상 범위", 0.01, 0.5, 100.0, 0.12),
    ("saturation_range", "선택 채도 범위", 0.01, 1.0, 100.0, 0.6),
    ("lightness_range", "선택 명도 범위", 0.01, 1.0, 100.0, 0.5),
    ("hue", "선택 색상 이동", -100.0, 100.0, 1.0, 0.0),
    ("saturation", "선택 채도 보정", -100.0, 100.0, 1.0, 0.0),
    ("lightness", "선택 명도 보정", -100.0, 100.0, 1.0, 0.0),
];

/// The history label of a local pick.
pub const PICK_LABEL: &str = "부분 선택 색상 추가";
/// `local_point_note` before anything happened.
pub const FIRST_NOTE: &str = "선택한 마스크 안에서 색을 고르세요.";
const READING: &str = "보정 단계의 색을 읽는 중…";
const CANCELLED: &str = "대상이 바뀌어 색 선택을 취소했습니다. 다시 선택하세요.";
const ADDED_NOTE: &str = "색을 추가했습니다. 범위 표시로 적용될 영역을 확인하세요.";
const ADDED_STATUS: &str = "색을 추가했습니다. 색상·채도·명도 범위를 조절하고 범위 표시로 확인하세요.";
const NO_LAYER: &str = "먼저 마스크와 선택 영역을 추가하세요.";
const PICK_STATUS: &str = "선택한 마스크 안에서 보정할 색을 클릭하세요.";
const OUTSIDE: &str = "사진 안쪽의 색을 골라 주세요.";
const OUTSIDE_MASK: &str = "선택한 마스크의 영역 안쪽을 클릭해 주세요.";
const NOT_REACHED: &str = "마스크를 켜고 선택 영역 안쪽의 색을 골라 주세요.";

/// What the fold-out keeps between frames (`cx.state::<MaskLocal>()`).
#[derive(Default)]
pub struct MaskLocal {
    /// `local_curve_channel.currentIndex()`.
    pub curve_channel: usize,
    /// `local_hsl_channel.currentIndex()`.
    pub hsl_channel: usize,
    /// `local_point_index` (−1: none).
    pub point_index: isize,
    /// `local_point_note`: Korean source text, or an error's own text.
    pub note: Option<String>,
    /// The pick tool is on (`view.tool_mode == 'local_point_color'`).
    pub picking: bool,
    /// `color_pick_generation` of the local picks.
    pub generation: u64,
    /// `color_pick_token`: the newest pick started.
    pub token: u64,
    /// `color_pick_running`.
    pub running: bool,
    /// The press of a pick (`tool_points[0]`).
    pub pressed: Option<[f64; 2]>,
    /// 부분 선택 색상 범위 표시.
    pub preview_on: bool,
    /// The local curve's editor.
    pub curve: CurveEditor,
    overlay: Overlay,
    /// The frame the fold-out was last drawn (leaving the tool ends the pick).
    seen: u64,
}

impl MaskLocal {
    /// The note under the pick button.
    pub fn note(&self) -> &str {
        self.note.as_deref().unwrap_or(FIRST_NOTE)
    }
}

/// The fold-out's state.
pub fn state(cx: &mut AppContext) -> &mut MaskLocal {
    cx.state::<MaskLocal>()
}

// ---- the selected layer's adjustments ---------------------------------------------------------------------------

/// The selected layer's `adjustments` (an empty object when it has none), if a layer is selected.
fn adjustments(cx: &mut AppContext) -> Option<Obj> {
    let layer = actions::current_layer(cx)?;
    Some(layer.get("adjustments").and_then(Value::as_object).cloned().unwrap_or_default())
}

/// `layer_property('adjustments', edit)`.
fn set_adjustments(cx: &mut AppContext, edit: Obj) {
    actions::layer_property(cx, "adjustments", Value::Object(edit));
}

fn identity_curves() -> Value {
    let curve = Value::Array(IDENTITY.iter().map(|p| json!([p[0], p[1]])).collect());
    Value::Array(vec![curve; 4])
}

/// Port of `load_local_curve`: the points the curve shows for the chosen channel (`local_curves[channel]`, the
/// identity without a layer or a stored curve).
pub fn curve_points(cx: &mut AppContext) -> Vec<[f64; 2]> {
    let channel = state(cx).curve_channel;
    let curves = adjustments(cx).and_then(|a| a.get("local_curves").cloned()).unwrap_or_else(identity_curves);
    match curves.get(channel) {
        Some(points) => curve::points_of(points),
        None => IDENTITY.to_vec(),
    }
}

/// Port of `local_curve_changed`: `local_curves[channel] = points` (live; the curve's release commits).
pub fn curve_changed(cx: &mut AppContext, points: &[[f64; 2]]) {
    actions::pause_overlay(cx);
    let Some(mut edit) = adjustments(cx) else { return };
    let channel = state(cx).curve_channel;
    let mut curves = edit.get("local_curves").cloned().unwrap_or_else(identity_curves);
    let Some(slot) = curves.as_array_mut().and_then(|list| list.get_mut(channel)) else { return };
    *slot = points_value(points);
    edit.insert("local_curves".into(), curves);
    set_adjustments(cx, edit);
}

/// Port of `load_local_hsl`: what the three sliders show for the chosen colour, and whether they are enabled
/// (a layer is selected).
pub fn hsl_shown(cx: &mut AppContext) -> ([f64; 3], bool) {
    let channel = state(cx).hsl_channel;
    let Some(edit) = adjustments(cx) else { return ([0.0; 3], false) };
    let group = edit.get("hsl").and_then(|groups| groups.get(channel)).cloned().unwrap_or(json!([0, 0, 0]));
    ([0, 1, 2].map(|i| group.get(i).and_then(Value::as_f64).unwrap_or(0.0)), true)
}

/// Port of `local_hsl_changed`: `hsl[channel][column] = value` (live; the slider's release commits). The eight
/// groups start as integer zeros (`setdefault('hsl', [[0, 0, 0] for _ in range(8)])`).
pub fn hsl_changed(cx: &mut AppContext, column: usize, value: f64) {
    actions::pause_overlay(cx);
    let Some(mut edit) = adjustments(cx) else { return };
    let channel = state(cx).hsl_channel;
    let groups = edit.entry("hsl").or_insert_with(|| Value::Array(vec![json!([0, 0, 0]); 8]));
    let Some(slot) = groups.get_mut(channel).and_then(|group| group.get_mut(column)) else { return };
    *slot = Value::from(value);
    set_adjustments(cx, edit);
}

/// Port of `local_point_values`: the selected layer's `point_colors`.
pub fn point_values(cx: &mut AppContext) -> Vec<Value> {
    adjustments(cx).and_then(|a| a.get("point_colors").and_then(Value::as_array).cloned()).unwrap_or_default()
}

/// Port of `load_local_points` + `load_local_point`: the selection kept within the list (−1 for an empty list).
pub fn load_points(cx: &mut AppContext) {
    let count = point_values(cx).len() as isize;
    let s = state(cx);
    s.point_index = s.point_index.max(0).min(count - 1);
}

/// The selected local point, if any.
fn selected_point(cx: &mut AppContext) -> Option<Obj> {
    let index = state(cx).point_index;
    let list = point_values(cx);
    usize::try_from(index).ok().and_then(|i| list.get(i)).and_then(Value::as_object).cloned()
}

/// What `load_local_point` shows for a key the point does not have.
pub fn shown_default(key: &str) -> f64 {
    match key {
        "range" => 0.12,
        "saturation_range" => 0.6,
        "lightness_range" => 0.5,
        _ => 0.0,
    }
}

/// Port of `load_local_point`'s values: what the six sliders show, and whether they (with 제거 and the range
/// check box) are enabled (`bool(point)`: a selected, non-empty point).
pub fn point_shown(cx: &mut AppContext) -> ([f64; 6], bool) {
    let point = selected_point(cx).unwrap_or_default();
    let values = POINT_CONTROLS.map(|(key, ..)| point.get(key).and_then(Value::as_f64).unwrap_or(shown_default(key)));
    (values, !point.is_empty())
}

/// Port of `local_point_changed`: `key` of the selected point (live; the slider's release commits).
pub fn point_changed(cx: &mut AppContext, key: &str, value: f64) {
    let Some(mut edit) = adjustments(cx) else { return };
    let index = state(cx).point_index;
    let Some(points) = edit.get_mut("point_colors").and_then(Value::as_array_mut) else { return };
    let Some(point) = usize::try_from(index).ok().and_then(|i| points.get_mut(i)).and_then(Value::as_object_mut) else {
        return;
    };
    point.insert(key.to_owned(), Value::from(value));
    set_adjustments(cx, edit);
}

/// Port of `remove_local_point`: the selected point removed, one undo step.
pub fn remove_point(cx: &mut AppContext) {
    let Some(mut edit) = adjustments(cx) else { return };
    let index = state(cx).point_index;
    let Some(points) = edit.get_mut("point_colors").and_then(Value::as_array_mut) else { return };
    let Some(at) = usize::try_from(index).ok().filter(|i| *i < points.len()) else { return };
    points.remove(at);
    set_adjustments(cx, edit);
    cx.doc.edit_mut().commit("보정 변경");
    load_points(cx);
}

// ---- picking -------------------------------------------------------------------------------------------------

/// The view is where the local pick can be: Develop, the Masks tool, no mask tool mode, no white-balance picker.
fn on_screen(cx: &AppContext) -> bool {
    cx.mode() == ViewMode::Develop
        && cx.tool() == Tool::Masks
        && !cx.wb_picker
        && cx.state_ref::<MaskState>().is_none_or(|s| s.mode == Mode::None)
}

/// Another tool took the view (a mask tool mode, another strip tool): the pick ends with a new generation.
pub fn sync(cx: &mut AppContext) {
    if !on_screen(cx) && cx.state_ref::<MaskLocal>().is_some_and(|s| s.picking) {
        let s = state(cx);
        s.picking = false;
        s.pressed = None;
        s.generation += 1;
    }
}

/// Port of `mode('local_point_color')`: the pick tool on, inside the selected mask (refused without one).
pub fn start(cx: &mut AppContext) {
    if cx.doc.current().is_none() {
        return;
    }
    state(cx).generation += 1;
    if actions::current_layer(cx).is_none() {
        cx.set_status(tr(NO_LAYER));
        return;
    }
    cx.set_before(false);
    cx.wb_picker = false;
    // `view.set_tool('local_point_color')`: the mask tool's own mode, stroke and handles end (a fresh mask stays:
    // the mode is not empty).
    actions::cancel_stroke(cx);
    {
        let m = actions::state(cx);
        m.mode = Mode::None;
        m.points.clear();
        m.handles.clear();
        m.handle_drag = None;
    }
    let s = state(cx);
    s.picking = true;
    s.pressed = None;
    cx.set_status(tr(PICK_STATUS));
}

/// Esc (`view.set_tool('')`): the pick ends, and a fresh mask without a component is dropped.
pub fn stop(cx: &mut AppContext) {
    let s = state(cx);
    s.picking = false;
    s.pressed = None;
    s.generation += 1;
    actions::set_tool(cx, Mode::None);
}

/// A pick being read: what `pick_point_color` captured.
#[derive(Clone)]
pub struct Pick {
    pub token: u64,
    pub photo: u64,
    /// The settings at the pick (`deepcopy(self.w.settings)`).
    pub settings: Settings,
    /// `layer_index` at the pick.
    pub layer: isize,
    /// The frame point.
    pub point: [f64; 2],
}

/// Port of `stroke` for the local pick: the clicked frame point must show the photo (`source_points`), then
/// `pick_point_color(points[0], local=True)`.
pub fn clicked(cx: &mut AppContext, point: [f64; 2]) {
    if let Some(map) = actions::coordinate_map(cx) {
        if map.source_points(&[point]).is_empty() {
            cx.set_status(tr("변형 후 생긴 빈 가장자리 대신 사진 안쪽을 선택하세요."));
            return;
        }
    }
    pick(cx, point);
}

/// The first half of `pick_point_color(local=True)`: commits, ends the tool (`mode('')`), captures the settings and
/// the layer, shows the reading note.
pub fn prepare(cx: &mut AppContext, point: [f64; 2]) -> Option<Pick> {
    let photo = cx.doc.current()?.id;
    cx.doc.edit_mut().commit("보정 변경");
    {
        let s = state(cx);
        s.picking = false;
        s.pressed = None;
        s.generation += 1;
    }
    actions::mode(cx, Mode::None);
    let token = state(cx).generation;
    let pick = Pick { token, photo, settings: cx.settings().clone(), layer: actions::state(cx).layer, point };
    let s = state(cx);
    s.note = Some(READING.into());
    s.running = true;
    s.token = token;
    cx.set_status(tr(READING));
    Some(pick)
}

/// The second half (`ready` / `failed`): the colour becomes a new point of the layer when the same photo, settings
/// and layer are still there; otherwise the note says the pick was cancelled. Errors show in the note and the
/// status.
pub fn finish(cx: &mut AppContext, pick: &Pick, result: Result<[f32; 3], String>) {
    if state(cx).token == pick.token {
        state(cx).running = false;
    }
    let valid = state(cx).generation == pick.token
        && cx.doc.current_id() == pick.photo
        && settings_eq(cx.settings(), &pick.settings)
        && actions::state(cx).layer == pick.layer;
    match result {
        Ok(rgb) => {
            if !valid {
                if state(cx).token == pick.token {
                    state(cx).note = Some(CANCELLED.into());
                }
                return;
            }
            let Some(mut edit) = adjustments(cx) else { return };
            let points = edit.entry("point_colors").or_insert_with(|| Value::Array(Vec::new()));
            let Some(list) = points.as_array_mut() else { return };
            list.push(new_point(rgb));
            let index = list.len() as isize - 1;
            state(cx).point_index = index;
            set_adjustments(cx, edit);
            load_points(cx);
            cx.doc.edit_mut().commit(PICK_LABEL);
            state(cx).note = Some(ADDED_NOTE.into());
            cx.set_status(tr(ADDED_STATUS));
        }
        Err(message) => {
            if valid {
                cx.set_status(tr(&message));
                state(cx).note = Some(message);
            }
        }
    }
}

/// `pick_point_color(local=True)` with the reading given (`work`): [`prepare`], the reading, [`finish`] — at once.
pub fn pick_with(cx: &mut AppContext, point: [f64; 2], work: impl FnOnce(&Pick) -> Result<[f32; 3], String>) {
    if let Some(pick) = prepare(cx, point) {
        let result = work(&pick);
        finish(cx, &pick, result);
    }
}

/// A loaded source and its oriented, uncropped size.
struct Loaded {
    photo: grainy_engine::files::Photo,
    original: (f64, f64),
}

fn load(path: &std::path::Path, max_size: Option<usize>, settings: &Settings) -> Result<Loaded, String> {
    let loaded = grainy_engine::files::load_photo(path, max_size, settings).map_err(|e| e.to_string())?;
    let (h, w, _) = loaded.photo.pixels().dim();
    let size = |key: &str, fallback: usize| {
        loaded.metadata.get(key).and_then(Value::as_u64).map_or(fallback as f64, |v| v as f64)
    };
    let original = (size("width", w), size("height", h));
    Ok(Loaded { photo: loaded.photo, original })
}

/// Port of `pick_point_color(local=True)`: the colour is read on a background job from the source on screen (the
/// full-size one at 1:1, the reduced one otherwise).
pub fn pick(cx: &mut AppContext, point: [f64; 2]) {
    let Some(path) = cx.doc.current().map(|p| p.path.clone()) else { return };
    let max = match cx.frame().map(|f| f.resolution) {
        Some(crate::render::Resolution::Full) => None,
        _ => Some(crate::worker::PREVIEW_SIZE),
    };
    let Some(pick) = prepare(cx, point) else { return };
    let job = pick.clone();
    cx.jobs.spawn(
        "local_point_color",
        tr(READING),
        move |_| {
            let loaded = load(&path, max, &job.settings)?;
            let layer = usize::try_from(job.layer).map_err(|_| "list index out of range".to_owned())?;
            sample(loaded.photo.source(), &job.settings.0, job.point, layer, Some(loaded.original))
        },
        move |cx: &mut AppContext, result: Result<[f32; 3], String>| finish(cx, &pick, result),
    );
}

// ---- the image a local point works on ---------------------------------------------------------------------------

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

/// Whether the mask layers read the developed colour source (auto-masked brushes and sky components).
fn needs_colour_source(s: &Settings) -> bool {
    let truthy = grainy_core::settings::truthy;
    s.value("masks").as_array().is_some_and(|list| {
        list.iter().any(|layer| {
            layer.get("components").and_then(Value::as_array).is_some_and(|components| {
                components.iter().any(|c| {
                    c.get("auto_mask").is_some_and(truthy) || c.get("type").and_then(Value::as_str) == Some("sky")
                })
            })
        })
    })
}

/// Port of luma/point_color.py `_probe` for mask layer `layer`: `read` receives what the develop's `on_point`
/// hands point `index` of that layer (`index == len(point_colors)`: the image after the layer's last point). The
/// Masks panel is switched on for the probe (`panels_off` without `masks`). Errors are the oracle's messages or the
/// stage's.
pub fn probe<R>(
    source: Source<'_>,
    settings: &Map<String, Value>,
    layer: usize,
    index: usize,
    original_size: Option<(f64, f64)>,
    mut read: impl FnMut(&PointInput<'_>) -> Result<R, String>,
) -> Result<R, String> {
    let mut map = settings.clone();
    let off: Vec<Value> = map
        .get("panels_off")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter(|p| p.as_str() != Some("masks")).cloned().collect())
        .unwrap_or_default();
    map.insert("panels_off".into(), Value::Array(off));
    let s = grainy_engine::working_settings(&map);
    let profile_lens = if s.bool("lensfun_enabled") && s.bool("lensfun") {
        Some(grainy_lens::ProfileModel::from_settings(&s).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let lens = profile_lens.as_ref().map(|model| model as &dyn LensModel);
    let externals = Externals { lens, ..Externals::default() };
    if let Some(missing) = grainy_engine::pending(&s, &externals) {
        return Err(format!("not ported yet: {missing}"));
    }
    let scale = grainy_engine::pixel_scale(source.pixels(), original_size).map_err(|e| e.to_string())?;
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
    let coloured = grainy_colorgrade::develop_color(&toned, &s).map_err(|e| e.to_string())?;
    let detailed = grainy_detail::develop_detail(&coloured, &s, scale).map_err(|e| e.to_string())?;
    let stored = s.value("masks").as_array().cloned().unwrap_or_default();
    if stored.is_empty() {
        return Err(NOT_REACHED.to_owned());
    }
    // HDR editing: every layer works on extended values.
    let layers: Vec<Value> = if s.bool("hdr") {
        stored
            .into_iter()
            .map(|mut layer| {
                if let Some(map) = layer.as_object_mut() {
                    let adjustments = map.entry("adjustments").or_insert_with(|| Value::Object(Map::new()));
                    if let Some(adjustments) = adjustments.as_object_mut() {
                        adjustments.insert("hdr".into(), Value::Bool(true));
                    }
                }
                layer
            })
            .collect()
    } else {
        stored
    };
    let colour_source = needs_colour_source(&s).then(|| grainy_color::to_srgb(linear));
    let transform = |mask: &Image| -> grainy_local::Result<Image> {
        grainy_geometry::geometry(mask, &s, UseCrop::Yes, externals.lens)
            .map(std::borrow::Cow::into_owned)
            .map_err(|error| grainy_local::Error::Invalid(error.to_string()))
    };
    let (h, w, _) = source.pixels().dim();
    let context = LocalContext {
        source_shape: (h, w),
        transform: &transform,
        source_image: colour_source.as_ref(),
        pixel_scale: scale,
        detail_version: s.i64("detail_version"),
        sky: externals.sky,
    };
    let mut found: Option<Result<R, String>> = None;
    let mut visit = |input: &PointInput<'_>| {
        if input.layer == layer && input.point == index {
            found = Some(read(input));
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    };
    let mut hooks = LocalHooks { on_point: Some(&mut visit), ..LocalHooks::default() };
    let outcome = grainy_local::apply_local(&detailed, &layers, &context, &mut hooks);
    if let Some(found) = found {
        return found;
    }
    match outcome {
        Err(error) if !matches!(error, grainy_local::Error::Interrupted) => Err(error.to_string()),
        _ => Err(NOT_REACHED.to_owned()),
    }
}

/// The layer's `point_colors` in the settings (`settings['masks'][layer]`; an error when the layer is missing).
fn layer_points(settings: &Map<String, Value>, layer: usize) -> Result<Vec<Value>, String> {
    let layers = settings.get("masks").and_then(Value::as_array).cloned().unwrap_or_default();
    let layer = layers.get(layer).ok_or_else(|| "list index out of range".to_owned())?;
    Ok(layer
        .get("adjustments")
        .and_then(|a| a.get("point_colors"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Port of luma/point_color.py `sample(source, settings, point, layer)`: the colour (clipped to 0–1) at the frame
/// point `position` of the image a new point of mask `layer` would work on, inside the mask.
pub fn sample(
    source: Source<'_>,
    settings: &Map<String, Value>,
    position: [f64; 2],
    layer: usize,
    original_size: Option<(f64, f64)>,
) -> Result<[f32; 3], String> {
    if !position.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
        return Err(OUTSIDE.to_owned());
    }
    let count = layer_points(settings, layer)?.len();
    probe(source, settings, layer, count, original_size, |input| {
        let (h, w) = input.frame;
        // x = min(w-1, int(position[0]*w)) - origin[0]
        let x = ((position[0] * w as f64) as i64).min(w as i64 - 1) - input.origin.0 as i64;
        let y = ((position[1] * h as f64) as i64).min(h as i64 - 1) - input.origin.1 as i64;
        let (ih, iw, _) = input.image.dim();
        if !(0..ih as i64).contains(&y) || !(0..iw as i64).contains(&x) {
            return Err(OUTSIDE_MASK.to_owned());
        }
        let (x, y) = (x as usize, y as usize);
        if input.mask[[y, x]] <= 1e-4 {
            return Err(OUTSIDE_MASK.to_owned());
        }
        Ok([0, 1, 2].map(|c| input.image[[y, x, c]].clamp(0.0, 1.0)))
    })
}

/// Port of luma/point_color.py `preview(source, settings, (layer, index))`: the coverage of point `index` of mask
/// `layer` on the frame (its weights times the mask, placed at the layer's region).
pub fn preview(
    source: Source<'_>,
    settings: &Map<String, Value>,
    layer: usize,
    index: usize,
    original_size: Option<(f64, f64)>,
) -> Result<Plane, String> {
    let point = layer_points(settings, layer)?
        .get(index)
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "list index out of range".to_owned())?;
    probe(source, settings, layer, index, original_size, |input| {
        let weight = grainy_colorgrade::point_color::coverage(input.image, &point).map_err(|e| e.to_string())?;
        let weight = weight * input.mask;
        let mut result = Plane::zeros(input.frame);
        let (x, y) = input.origin;
        let (h, w) = weight.dim();
        result.slice_mut(s![y..y + h, x..x + w]).assign(&weight);
        Ok(result)
    })
}

/// Port of `preview_key` for a local point: the point's shifts (and later points) cannot change what it covers.
pub fn preview_key(settings: &Settings, layer: usize, index: usize) -> String {
    let mut value = settings.0.clone();
    if let Some(Value::Object(owner)) =
        value.get_mut("masks").and_then(|m| m.get_mut(layer)).and_then(|l| l.get_mut("adjustments"))
    {
        if let Some(Value::Array(points)) = owner.get_mut("point_colors") {
            points.truncate(index + 1);
            if let Some(Value::Object(point)) = points.get_mut(index) {
                for key in ["hue", "saturation", "lightness"] {
                    point.remove(key);
                }
            }
        }
    }
    format!("{layer}:{index}|{}", Value::Object(value))
}

// ---- the 부분 선택 색상 범위 표시 overlay --------------------------------------------------------------------------

#[derive(Default)]
struct Overlay {
    texture: Option<egui::TextureHandle>,
    shown: Option<String>,
    running: Option<String>,
}

/// Port of `point_overlay_target` for local points: (layer, point) whose coverage shows — the Masks tool open, the
/// check box on, the selected mask switched on, a point selected, not the before view.
pub fn overlay_target(cx: &AppContext) -> Option<(usize, usize)> {
    let local = cx.state_ref::<MaskLocal>()?;
    if !local.preview_on || cx.before() || cx.mode() != ViewMode::Develop || cx.tool() != Tool::Masks {
        return None;
    }
    let masks = cx.state_ref::<MaskState>()?;
    if masks.photo != cx.doc.current_id() {
        return None;
    }
    let list = layers::list(cx.settings());
    let layer = layers::at(&list, masks.layer)?;
    if !layers::truthy(layer.get("enabled"), true) {
        return None;
    }
    let count =
        layer.get("adjustments").and_then(|a| a.get("point_colors")).and_then(Value::as_array).map_or(0, Vec::len);
    let index = usize::try_from(local.point_index).ok().filter(|i| *i < count)?;
    Some((masks.layer as usize, index))
}

/// Brings the overlay up to date: one job at a time; the newest wish is computed when it ends.
pub fn refresh(cx: &mut AppContext) {
    let wanted = overlay_target(cx).zip(cx.doc.current().map(|p| p.path.clone()));
    let Some(((layer, index), path)) = wanted else {
        let s = state(cx);
        s.overlay.texture = None;
        s.overlay.shown = None;
        return;
    };
    let settings = cx.settings().clone();
    let key = format!("{}|{}", path.display(), preview_key(&settings, layer, index));
    {
        let s = state(cx);
        if s.overlay.shown.as_ref() == Some(&key) || s.overlay.running.is_some() {
            return;
        }
        s.overlay.running = Some(key.clone());
    }
    let path: PathBuf = path;
    cx.jobs.spawn(
        "local_point_overlay",
        "",
        move |_| {
            let loaded = load(&path, Some(crate::worker::PREVIEW_SIZE), &settings)?;
            // A point the oracle cannot evaluate shows nothing (`np.zeros((1, 1))`).
            let coverage = preview(loaded.photo.source(), &settings.0, layer, index, Some(loaded.original))
                .unwrap_or_else(|_| Plane::zeros((1, 1)));
            Ok(overlay_picture(&coverage))
        },
        move |cx: &mut AppContext, result| {
            let texture = match result {
                Ok(p) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                    Some(cx.egui.load_texture("local_point_overlay", image, egui::TextureOptions::LINEAR))
                }
                Err(message) => {
                    log::warn!("local point colour overlay: {message}");
                    None
                }
            };
            let s = state(cx);
            s.overlay.running = None;
            s.overlay.texture = texture;
            s.overlay.shown = Some(key);
            cx.egui.request_repaint();
        },
    );
}

/// Whether the overlay picture for the current wish is on screen (tests).
pub fn overlay_ready(cx: &AppContext) -> bool {
    cx.state_ref::<MaskLocal>().is_some_and(|s| s.overlay.texture.is_some() && s.overlay.running.is_none())
}

// ---- the canvas tool -------------------------------------------------------------------------------------------

/// The local pick on the photo (active while picking) and the coverage overlay (drawn whatever tool is active).
pub struct LocalPointTool;

impl CanvasTool for LocalPointTool {
    fn name(&self) -> &'static str {
        "local_point_color"
    }

    fn active(&self, cx: &AppContext) -> bool {
        on_screen(cx) && cx.state_ref::<MaskLocal>().is_some_and(|s| s.picking)
    }

    fn edits(&self) -> &'static [&'static str] {
        &["masks"]
    }

    /// `PhotoView` with the tool mode `local_point_color`: the press must be on the photo; the release ends the
    /// one-point stroke.
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

    /// The coverage overlay of the selected local point.
    fn paint_passive(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        if overlay_target(cx).is_none() {
            return;
        }
        let Some(texture) = cx.state_ref::<MaskLocal>().and_then(|s| s.overlay.texture.as_ref()) else { return };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture.id(), view.rect, uv, Color32::WHITE);
    }
}

// ---- the fold-out --------------------------------------------------------------------------------------------

fn slider_spec(low: f64, high: f64, scale: f64, reset: f64) -> Spec {
    Spec { key: "masks", label: "", low, high, scale, reset: Reset::Value(reset), place: Place::Panel("masks") }
}

/// The fold-out's body; the Masks panel draws it under its adjustments while a mask is selected.
pub fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    // Leaving the tool ends the pick (`tab_changed` → `set_tool('')`): noticed when the fold-out shows again.
    let pass = cx.egui.cumulative_pass_nr();
    let gap = state(cx).seen + 1 < pass;
    state(cx).seen = pass;
    if gap && state(cx).picking {
        let s = state(cx);
        s.picking = false;
        s.pressed = None;
        s.generation += 1;
    }
    sync(cx);
    load_points(cx);
    let has_layer = actions::current_layer(cx).is_some();

    // 부분 커브.
    let names: Vec<String> = CURVE_CHANNELS.iter().map(|c| tr(c)).collect();
    let channel = state(cx).curve_channel.min(3);
    if let Some(chosen) = controls::choice(ui, "local_curve_channel", "", &names, channel, width) {
        state(cx).curve_channel = chosen;
    }
    let points = curve_points(cx);
    let signals = curve::show(ui, &mut state(cx).curve, &points, false);
    for signal in signals {
        match signal {
            Signal::Changed(points) => curve_changed(cx, &points),
            Signal::Committed => {
                cx.doc.edit_mut().commit("보정 변경");
            }
        }
    }

    // 부분 색상표.
    widgets::muted(ui, &tr("부분 색상표 · 선택한 마스크 안에서 색별로 조절합니다."));
    let colours: Vec<String> = COLOURS.iter().map(|c| tr(c)).collect();
    let colour = state(cx).hsl_channel.min(7);
    if let Some(chosen) = controls::choice(ui, "local_hsl_channel", "", &colours, colour, width) {
        state(cx).hsl_channel = chosen;
    }
    let (values, enabled) = hsl_shown(cx);
    let spec = slider_spec(-100.0, 100.0, 1.0, 0.0);
    for (column, title) in HSL_TITLES.iter().enumerate() {
        let event =
            widgets::adjustment_row(ui, &format!("local_hsl.{column}"), &tr(title), &spec, values[column], enabled);
        if let Some(value) = event.set {
            // A slider emits only when its position moves.
            if spec.position(value) != spec.position(values[column]) {
                hsl_changed(cx, column, value);
            }
        }
        if event.commit {
            cx.doc.edit_mut().commit("보정 변경");
        }
    }

    // 부분 선택 색상.
    let picking = state(cx).picking;
    let text = tr("부분 선택 색상 · 사진에서 찍기");
    if Button::new(&text).width(width).enabled(has_layer).checked(picking).show(ui).clicked() {
        start(cx);
    }
    let note = state(cx).note().to_owned();
    widgets::muted(ui, &tr(&note));
    let count = point_values(cx).len();
    let items: Vec<String> = (0..count).map(|i| tr_with("부분 선택 색상 {0}", &[&(i + 1).to_string()])).collect();
    let selected = usize::try_from(state(cx).point_index).unwrap_or(usize::MAX);
    if let Some(chosen) = controls::choice(ui, "local_points", &tr("부분 선택 색상 {0}"), &items, selected, width)
    {
        state(cx).point_index = chosen as isize;
    }
    let (values, enabled) = point_shown(cx);
    for (i, (key, title, low, high, scale, reset)) in POINT_CONTROLS.iter().enumerate() {
        let spec = slider_spec(*low, *high, *scale, *reset);
        let event = widgets::adjustment_row(ui, &format!("local_point.{key}"), &tr(title), &spec, values[i], enabled);
        if let Some(value) = event.set {
            if spec.position(value) != spec.position(values[i]) {
                point_changed(cx, key, value);
            }
        }
        if event.commit {
            cx.doc.edit_mut().commit("보정 변경");
        }
    }
    if Button::new(&tr("부분 선택 색상 제거")).width(width).enabled(enabled).show(ui).clicked() {
        remove_point(cx);
    }
    let mut preview = state(cx).preview_on;
    ui.add_enabled_ui(enabled, |ui| {
        if widgets::checkbox(ui, &mut preview, &tr("부분 선택 색상 범위 표시")).changed() {
            state(cx).preview_on = preview;
        }
    });
    widgets::muted(ui, &tr("마스크 영역 안의 색을 클릭하세요. 파란 표시는 색상 범위와 마스크가 겹치는 부분입니다."));
    refresh(cx);
}
