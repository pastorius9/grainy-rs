//! The dust and scratch review of one photo (먼지 자동 감지): port of luma/dust_dialog.py `prepare_source`, `DustView`
//! and `DustDialog`. Detection never writes catalogue edits: the dialog answers a `dust` retouch operation (and, in
//! a batch review, the review state) when 선택한 먼지 제거 / 검토 완료 is pressed.
//!
//! [`DustReview`] is the dialog without a window, shared (`Rc<RefCell<…>>`) between the window ([`DustDialog`]) and
//! the jobs' callbacks: every method is the oracle's method of the same name, so a test drives it without a frame.
//! Detection and the repair preview run on the job service ([`scan`], [`start_preview`]); `view` holds the scene
//! state of the picture (`DustView`: fit, 100 %, wheel zoom, hand drag, clicks, manual spots and strokes).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use grainy_core::{Image, Plane, Settings};
use serde_json::{json, Map, Value};

use super::Dialog;
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// `PAGE_SIZE`: candidates added to the list per 후보 더 보기.
pub const PAGE_SIZE: usize = 500;
/// `MAX_CANDIDATES`: the detector's limit.
pub const MAX_CANDIDATES: usize = 5000;
/// The detector's tile size (`detect`'s default).
const TILE_SIZE: usize = 640;
/// The preview timer's delay (`preview_timer.start(120)`).
const PREVIEW_DELAY: Duration = Duration::from_millis(120);

/// The kinds of `찾을 종류`: (label, key).
pub const POLARITIES: [(&str, &str); 3] = [("검은 점 + 흰 먼지", "both"), ("검은 점", "dark"), ("흰 먼지", "bright")];
/// The repair methods of `복구 방식`: (label, key).
pub const METHODS: [(&str, &str); 2] = [("주변 질감 살리기", "texture"), ("부드럽게 연결", "smooth")];

// ---- Python formatting --------------------------------------------------------------------------------------

/// `f'{n:,}'`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `f'{v:g}'`.
pub fn py_g(v: f64) -> String {
    if v == 0.0 {
        return if v.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    if !v.is_finite() {
        return if v.is_nan() {
            "nan".into()
        } else if v > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let scientific = format!("{v:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exponent.abs())
    } else {
        trim(&format!("{v:.*}", (5 - exponent) as usize))
    }
}

/// The text of `Options.validate`'s `ValueError` (the oracle shows it in the status line).
pub fn validate(options: &grainy_dust::Options) -> Result<(), String> {
    if !(1..=100).contains(&options.sensitivity) {
        return Err("민감도는 1–100 사이여야 합니다.".into());
    }
    if !(2 <= options.minimum && options.minimum <= options.maximum && options.maximum <= 160) {
        return Err("먼지 크기는 2–160 px 범위에서 최소 ≤ 최대여야 합니다.".into());
    }
    if !["both", "dark", "bright"].contains(&options.polarity.as_str()) {
        return Err("먼지 종류를 선택하세요.".into());
    }
    if !(2..=40).contains(&options.scratch_width) {
        return Err("흠집 최대 굵기는 2–40 px 사이여야 합니다.".into());
    }
    Ok(())
}

/// `Options(**{k: v for k, v in saved.items() if k in asdict(Options())}); validate()`, the defaults otherwise.
pub fn initial_options(saved: Option<&Value>) -> grainy_dust::Options {
    let known = serde_json::to_value(grainy_dust::Options::default()).unwrap_or(Value::Null);
    let filtered: Map<String, Value> = saved
        .and_then(Value::as_object)
        .map(|m| {
            m.iter().filter(|(k, _)| known.get(k.as_str()).is_some()).map(|(k, v)| (k.clone(), v.clone())).collect()
        })
        .unwrap_or_default();
    match serde_json::from_value::<grainy_dust::Options>(Value::Object(filtered)) {
        Ok(options) if validate(&options).is_ok() => options,
        _ => grainy_dust::Options::default(),
    }
}

/// `asdict(options)`, in the dataclass's field order.
pub fn options_json(options: &grainy_dust::Options) -> Value {
    json!({
        "sensitivity": options.sensitivity, "minimum": options.minimum, "maximum": options.maximum,
        "polarity": options.polarity, "detailed": options.detailed, "reduce_patterns": options.reduce_patterns,
        "soft": options.soft, "suppress_grain": options.suppress_grain, "scratches": options.scratches,
        "scratch_width": options.scratch_width,
    })
}

/// `Options(**stored) == options`: stored options with missing keys at their defaults.
pub fn same_options(stored: &Value, options: &grainy_dust::Options) -> bool {
    serde_json::from_value::<grainy_dust::Options>(stored.clone())
        .is_ok_and(|o| options_json(&o) == options_json(options))
}

/// The detection dictionary (`detect`'s return value) as the dialog keeps it.
pub fn detection_json(detection: &grainy_dust::Detection) -> Value {
    json!({
        "spots": detection.spots, "total": detection.total, "truncated": detection.truncated,
        "width": detection.width, "height": detection.height, "seconds": detection.seconds,
    })
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(grainy_core::settings::truthy)
}

// ---- the source ---------------------------------------------------------------------------------------------

/// What detection and the previews work on: the retouched linear frame and its encoded luminance.
pub struct Prepared {
    /// `apply_retouch(linear_frame(source, settings), settings['retouch'])`.
    pub linear: Image,
    /// `cvtColor(output_rgb(to_srgb(linear), working_space), RGB2GRAY)`.
    pub gray: Plane,
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

/// The marker of a cancelled job (`dust.Cancelled`).
const CANCELLED: &str = "\u{0}cancelled";

/// `rgb[..., 0]*.299 + rgb[..., 1]*.587 + rgb[..., 2]*.114` (`cv2.COLOR_RGB2GRAY` on float32).
fn gray_of(rgb: &Image) -> Result<Plane, String> {
    let gray = grainy_imgproc::cvt_color(rgb, grainy_imgproc::ColorConversion::Rgb2Gray).map_err(|e| e.to_string())?;
    Ok(gray.index_axis_move(ndarray::Axis(2), 0))
}

/// `output_rgb(to_srgb(linear), working_space, destination)`.
fn encoded(linear: &Image, working_space: &str) -> Result<Image, String> {
    grainy_cms::output_rgb(&grainy_color::to_srgb(linear), working_space, grainy_cms::Destination::Named("sRGB"))
        .map_err(|e| e.to_string())
}

/// `cv2` inpainting and distance transform for the retouch stage (grainy-imgproc), as the engine's own.
struct Cv;

impl grainy_local::Inpaint for Cv {
    fn inpaint_ns(&self, plane: &Plane, mask: &ndarray::Array2<u8>, radius: f64) -> grainy_local::Result<Plane> {
        grainy_imgproc::inpaint(plane, mask, radius).map_err(|e| grainy_local::Error::Invalid(e.to_string()))
    }
}

impl grainy_local::DistanceTransform for Cv {
    fn distance_l2(&self, mask: &ndarray::Array2<u8>) -> grainy_local::Result<Plane> {
        grainy_imgproc::distance_transform(mask, 3).map_err(|e| grainy_local::Error::Invalid(e.to_string()))
    }
}

/// Port of luma/dust_dialog.py `prepare_source`: the photo at full resolution, its RAW development or negative
/// conversion (`rawcolor.linear_frame`), the existing retouch operations, and the encoded luminance.
pub fn prepare_source(path: &Path, settings: &Settings, cancel: &AtomicBool) -> Result<Prepared, String> {
    cancelled(cancel)?;
    let loaded = grainy_engine::files::load_photo(path, None, settings).map_err(|e| e.to_string())?;
    cancelled(cancel)?;
    let developed: Image = match loaded.photo.source() {
        grainy_engine::Source::Camera(raw) => {
            grainy_raw::rawcolor::develop_camera(raw, settings).map_err(|e| e.to_string())?
        }
        grainy_engine::Source::Linear(image) => image.clone(),
    };
    drop(loaded);
    let linear = if settings.bool("negative") { grainy_film::negative::apply(&developed, settings) } else { developed };
    let operations = settings.value("retouch").as_array().cloned().unwrap_or_default();
    let linear = if operations.is_empty() {
        linear
    } else {
        let externals = grainy_engine::Externals::default();
        let tools = grainy_local::RetouchTools { inpaint: Some(&Cv), distance: Some(&Cv), dust: externals.dust };
        grainy_local::apply_retouch(&linear, &operations, &tools).map_err(|e| e.to_string())?
    };
    let rgb = encoded(&linear, settings.str("working_space"))?;
    let gray = gray_of(&rgb)?;
    cancelled(cancel)?;
    Ok(Prepared { linear, gray })
}

// ---- the view -----------------------------------------------------------------------------------------------

/// `DustView`'s state: the transform of the scene (the picture's pixels) on the screen and the stroke being drawn.
#[derive(Debug, Clone)]
pub struct ViewState {
    /// `fitted`: the picture follows the view's size.
    pub fitted: bool,
    /// Screen points per scene pixel (`transform().m11()`).
    pub zoom: f32,
    /// The scene point at the centre of the view.
    pub center: Pos2,
    /// `stroke`: the scene points of the stroke being drawn.
    pub stroke: Vec<Pos2>,
    /// `stroke_diameter` (수동 먼지 크기).
    pub stroke_diameter: f32,
    /// The size of the view on screen, when it was last drawn.
    pub viewport: Vec2,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            fitted: true,
            zoom: 1.0,
            center: Pos2::ZERO,
            stroke: Vec::new(),
            stroke_diameter: 12.0,
            viewport: Vec2::new(800.0, 600.0),
        }
    }
}

// ---- the dialog without a window ---------------------------------------------------------------------------

/// What a photo's review starts from in a batch (`record`, `cached`, `review_state`).
#[derive(Debug, Clone, Default)]
pub struct BatchReview {
    /// `capture_record`: `{id, path, settings, fingerprint}`.
    pub record: Value,
    /// `{'result': …, 'options': …}` of the batch's detection.
    pub cached: Option<Value>,
    /// The review kept from an earlier 검토 완료.
    pub review_state: Option<Value>,
}

/// Port of `DustDialog`'s state and methods.
pub struct DustReview {
    pub ident: u64,
    pub path: PathBuf,
    pub settings: Settings,
    pub fingerprint: Value,
    /// `w.load_version` when it opened (the edit state's version of the open photo).
    pub load_version: u64,
    pub batch_review: bool,
    pub cached: Option<Value>,
    pub saved_review: Option<Value>,
    /// What 검토 완료 / 선택한 먼지 제거 answered.
    pub review_state: Option<Value>,
    pub operation: Option<Value>,
    pub closed: bool,
    pub busy: bool,
    /// `accept()` ran.
    pub accepted: bool,
    cancel: Arc<AtomicBool>,
    /// `stop_button.setEnabled(False)` after 검출 취소.
    stop_pressed: bool,
    /// `selection_changed` ran once (before, 후보 더 보기 keeps Qt's default enabled state).
    selection_seen: bool,
    /// `add_mode.isEnabled()`: set by `selection_changed` only (a closed dialog keeps the last state).
    manual_widget: bool,
    pub data: Option<Arc<Prepared>>,
    /// The detection dictionary.
    pub result: Option<Value>,
    pub detect_options: Value,
    pub pending_spots: Vec<Value>,
    /// `view.spots`: the listed candidates, in list order.
    pub spots: Vec<Value>,
    /// The list's check states.
    pub checks: Vec<bool>,
    /// `list.currentRow()`.
    pub current: i64,
    /// `view.current`.
    pub view_current: i64,
    // The controls.
    pub sensitivity: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub polarity: String,
    pub detailed: bool,
    pub soft: bool,
    pub suppress_grain: bool,
    pub reduce_patterns: bool,
    pub scratches: bool,
    pub scratch_width: i32,
    pub show_spots: bool,
    pub emphasis: bool,
    pub strength: i32,
    pub add_mode: bool,
    pub stroke_mode: bool,
    pub manual_size: i32,
    /// 선택 후보 제거 범위.
    pub scale: i32,
    /// 제거 결과 미리보기.
    pub after: bool,
    pub repair_method: String,
    pub status: String,
    pub preview_note: String,
    /// The progress bar: hidden, or (done, total) with total 0 for a busy bar.
    pub progress: Option<(usize, usize)>,
    progress_shared: Arc<Mutex<(usize, usize)>>,
    // The previews.
    preview_running: bool,
    preview_pending: bool,
    preview_generation: u64,
    preview_cancel: Arc<AtomicBool>,
    /// When the preview timer fires.
    pub preview_due: Option<Instant>,
    /// The picture shown (sRGB-encoded, or the emphasis view).
    pub picture: Option<Arc<Image>>,
    /// Grows with every new picture and every new list of spots (the window's caches).
    pub picture_version: u64,
    pub spots_version: u64,
    pub view: ViewState,
}

/// The shared dialog.
pub type Shared = Rc<RefCell<DustReview>>;

impl DustReview {
    /// Port of `DustDialog.__init__` without the window: the open photo, or a batch entry's photo (`record`).
    pub fn new(cx: &AppContext, batch: Option<BatchReview>) -> Self {
        let batch_review = batch.is_some();
        let (ident, path, settings, fingerprint, cached, saved_review) = match batch {
            Some(b) => {
                let record = &b.record;
                let settings = Settings(record["settings"].as_object().cloned().unwrap_or_default());
                (
                    record["id"].as_u64().unwrap_or(0),
                    PathBuf::from(record["path"].as_str().unwrap_or_default()),
                    settings,
                    record.get("fingerprint").cloned().unwrap_or(Value::Null),
                    b.cached,
                    b.review_state,
                )
            }
            None => {
                let path = cx.doc.current().map(|p| p.path.clone()).unwrap_or_default();
                let fingerprint = grainy_catalog::preview::signature(&path).unwrap_or(Value::Null);
                (cx.doc.current_id(), path, cx.settings().clone(), fingerprint, None, None)
            }
        };
        let saved = match &cached {
            Some(c) => Some(c["options"].clone()),
            None => cx.doc.preference("dust_detection_options"),
        };
        let initial = initial_options(saved.as_ref());
        let method = match &saved_review {
            Some(review) => review.get("repair_method").and_then(Value::as_str).unwrap_or("smooth").to_owned(),
            None => cx
                .doc
                .preference("dust_repair_method")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "texture".into()),
        };
        // `findData` misses give -1 and `max(0, …)` the first method.
        let method = if METHODS.iter().any(|(_, k)| *k == method) { method } else { METHODS[0].1.to_owned() };
        Self {
            ident,
            path,
            settings,
            fingerprint,
            load_version: cx.doc.edit().version(),
            batch_review,
            cached,
            saved_review,
            review_state: None,
            operation: None,
            closed: false,
            busy: false,
            accepted: false,
            cancel: Arc::new(AtomicBool::new(false)),
            stop_pressed: false,
            selection_seen: false,
            manual_widget: false,
            data: None,
            result: None,
            detect_options: Value::Null,
            pending_spots: Vec::new(),
            spots: Vec::new(),
            checks: Vec::new(),
            current: -1,
            view_current: -1,
            sensitivity: initial.sensitivity,
            minimum: initial.minimum,
            maximum: initial.maximum,
            polarity: initial.polarity.clone(),
            detailed: initial.detailed,
            soft: initial.soft,
            suppress_grain: initial.suppress_grain,
            reduce_patterns: initial.reduce_patterns,
            scratches: initial.scratches,
            scratch_width: initial.scratch_width,
            show_spots: true,
            emphasis: false,
            strength: 50,
            add_mode: false,
            stroke_mode: false,
            manual_size: 12,
            scale: 100,
            after: false,
            repair_method: method,
            status: String::new(),
            preview_note: String::new(),
            progress: None,
            progress_shared: Arc::new(Mutex::new((0, 0))),
            preview_running: false,
            preview_pending: false,
            preview_generation: 0,
            preview_cancel: Arc::new(AtomicBool::new(false)),
            preview_due: None,
            picture: None,
            picture_version: 0,
            spots_version: 0,
            view: ViewState::default(),
        }
    }

    /// The window title (`먼지 자동 감지`, with the file name in a batch review).
    pub fn title(&self) -> String {
        let mut title = tr("먼지 자동 감지");
        if self.batch_review {
            title.push_str(" · ");
            title.push_str(&self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
        title
    }

    /// `options()`: the detector options of the controls.
    pub fn options(&self) -> grainy_dust::Options {
        grainy_dust::Options {
            sensitivity: self.sensitivity,
            minimum: self.minimum,
            maximum: self.maximum,
            polarity: self.polarity.clone(),
            detailed: self.detailed,
            reduce_patterns: self.reduce_patterns,
            soft: self.soft,
            suppress_grain: self.suppress_grain,
            scratches: self.scratches,
            scratch_width: self.scratch_width,
        }
    }

    /// Port of `valid_source`: the photo, its settings and its file are still the ones detected.
    pub fn valid_source(&self, cx: &AppContext) -> bool {
        let file = grainy_catalog::preview::signature(&self.path)
            .is_some_and(|s| grainy_core::settings::equivalent(&s, &self.fingerprint));
        if self.batch_review {
            let Some(catalog) = cx.doc.metadata_catalog() else { return false };
            let Ok(Some(row)) = catalog.photo(self.ident as i64) else { return false };
            return row["path"].as_str() == Some(&*self.path.to_string_lossy())
                && grainy_core::settings::equivalent(&row["settings"], &Value::Object(self.settings.0.clone()))
                && file;
        }
        cx.doc.current_id() == self.ident
            && cx.doc.edit().version() == self.load_version
            && crate::state::settings_eq(cx.settings(), &self.settings)
            && file
    }

    // ---- what the controls show (the oracle's enabled states, computed as it sets them) ----------------------

    /// `soft.setEnabled(not busy and detailed)`.
    pub fn soft_enabled(&self) -> bool {
        !self.busy && self.detailed
    }

    /// `suppress_grain.setEnabled(not busy and detailed and soft)`.
    pub fn suppress_enabled(&self) -> bool {
        !self.busy && self.detailed && self.soft
    }

    /// `reduce_patterns.setEnabled(not busy and (detailed or scratches))`.
    pub fn reduce_enabled(&self) -> bool {
        !self.busy && (self.detailed || self.scratches)
    }

    /// `scratch_width.setEnabled(not busy and scratches)`.
    pub fn scratch_width_enabled(&self) -> bool {
        !self.busy && self.scratches
    }

    /// `stop_button`: enabled while busy until 검출 취소 is pressed.
    pub fn stop_enabled(&self) -> bool {
        self.busy && !self.stop_pressed
    }

    /// `add_mode.isEnabled()` (and `stroke_mode`).
    pub fn manual_enabled(&self) -> bool {
        self.manual_widget
    }

    /// `scale.isEnabled()`.
    pub fn scale_enabled(&self) -> bool {
        (0..self.spots.len() as i64).contains(&self.current) && !self.busy
    }

    /// `more_button.isEnabled()`.
    pub fn more_enabled(&self) -> bool {
        !self.selection_seen || !self.pending_spots.is_empty() && !self.busy
    }

    /// `apply_button.isEnabled()`.
    pub fn apply_enabled(&self) -> bool {
        (self.checks.iter().any(|c| *c) || self.batch_review) && self.result.is_some() && !self.busy
    }

    /// The text of the list's row `index` (`append_item`).
    pub fn item_text(&self, index: usize) -> String {
        let spot = &self.spots[index];
        let manual = truthy(spot.get("manual"));
        let polarity = spot.get("polarity").and_then(Value::as_str).unwrap_or("");
        let kind = spot.get("detail_kind").and_then(Value::as_str);
        let mut title = (if manual {
            "수동 표시"
        } else if polarity == "dark" {
            "검은 점"
        } else {
            "흰 먼지"
        })
        .to_owned();
        if kind == Some("soft") {
            title = format!("옅은 점 · {title}");
        }
        if kind == Some("scratch") {
            title = if manual {
                "수동 흠집".into()
            } else {
                format!("긴 흠집 · {}", if polarity == "dark" { "어두운 선" } else { "밝은 선" })
            };
        }
        if truthy(spot.get("review")) {
            title = format!("추가 확인 · {title}");
        }
        format!("{}. {title} · {} px", index + 1, py_g(number(spot, "diameter")))
    }

    /// `count.text()`.
    pub fn count_text(&self) -> String {
        tr_with("체크된 후보 {0}개", &[&self.selected().len().to_string()])
    }

    /// `apply_button.text()`.
    pub fn apply_text(&self) -> String {
        if !self.selection_seen {
            return tr("선택한 먼지 제거");
        }
        let n = self.selected().len().to_string();
        if self.batch_review {
            tr_with("검토 완료 · {0}개 선택", &[&n])
        } else {
            tr_with("선택한 먼지 {0}개 제거", &[&n])
        }
    }

    /// Whether 후보 더 보기 shows, and its text.
    pub fn more(&self) -> (bool, String) {
        let pending = self.pending_spots.len();
        if !self.selection_seen {
            return (false, tr("후보 더 보기"));
        }
        (
            pending > 0,
            tr_with("후보 {0}개 더 보기 · 남은 {1}개", &[&PAGE_SIZE.min(pending).to_string(), &thousands(pending)]),
        )
    }

    /// `selected()`: the checked rows.
    pub fn selected(&self) -> Vec<usize> {
        self.checks.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect()
    }

    // ---- the oracle's methods -----------------------------------------------------------------------------

    /// Port of `set_busy`.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if busy {
            self.stop_pressed = false;
        }
        self.selection_changed();
    }

    /// Port of `options_changed`: a changed option drops the candidates and the cached work.
    pub fn options_changed(&mut self) {
        self.cached = None;
        self.saved_review = None;
        self.result = None;
        self.pending_spots.clear();
        self.clear_list();
        self.selection_changed();
        self.status = tr("검출 조건이 바뀌었습니다. 먼지 검출을 다시 누르세요.");
    }

    fn clear_list(&mut self) {
        self.spots.clear();
        self.checks.clear();
        self.current = -1;
        self.spots_version += 1;
    }

    /// Port of `cancel_scan`.
    pub fn cancel_scan(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.stop_pressed = true;
        self.status = tr("검출을 취소하는 중…");
    }

    /// Port of `show_more`: the next page of candidates joins the list (review candidates unchecked).
    pub fn show_more(&mut self) {
        if self.busy || self.result.is_none() {
            return;
        }
        let take = PAGE_SIZE.min(self.pending_spots.len());
        let batch: Vec<Value> = self.pending_spots.drain(..take).collect();
        for spot in batch {
            let checked = !truthy(spot.get("review"));
            self.spots.push(spot);
            self.checks.push(checked);
        }
        self.spots_version += 1;
        self.selection_changed();
        self.result_status();
    }

    /// Port of `result_status`.
    pub fn result_status(&mut self) {
        let Some(result) = &self.result else { return };
        let mut tail = String::new();
        if truthy(result.get("truncated")) {
            tail.push_str(&format!(" · 자동 후보는 최대 {}개까지 확인할 수 있습니다.", thousands(MAX_CANDIDATES)));
        }
        let o = &self.detect_options;
        if truthy(o.get("suppress_grain")) && truthy(o.get("soft")) && truthy(o.get("detailed")) {
            tail.push_str(" · 입자 억제 켜짐: 옅은 먼지가 빠질 수 있습니다.");
        }
        if truthy(o.get("scratches")) && truthy(o.get("reduce_patterns")) {
            tail.push_str(" · 무늬 억제 켜짐: 배경과 붙은 흠집이 빠질 수 있습니다.");
        }
        let extra = self.spots.iter().filter(|s| truthy(s.get("review"))).count();
        let manual = self.spots.iter().filter(|s| truthy(s.get("manual"))).count();
        let shown = self.spots.len() - manual;
        let size = |key: &str| thousands(result.get(key).and_then(Value::as_u64).unwrap_or(0) as usize);
        self.status = tr_with(
            "{0} × {1} · 전체 {2}개 중 목록 {3}개 · 수동 {4}개 · 추가 후보 {5}개는 기본 제외 · 검출 {6}초",
            &[
                &size("width"),
                &size("height"),
                &size("total"),
                &thousands(shown),
                &manual.to_string(),
                &extra.to_string(),
                &format!("{:.2}", number(result, "seconds")),
            ],
        ) + &tail;
    }

    /// Port of `selection_changed` (the texts and enabled states are computed when shown).
    pub fn selection_changed(&mut self) {
        self.selection_seen = true;
        self.manual_widget = self.result.is_some() && self.data.is_some() && !self.busy;
        if !self.manual_enabled() && self.add_mode {
            self.manual_mode(false);
        }
        if !self.manual_enabled() && self.stroke_mode {
            self.manual_stroke_mode(false);
        }
        if self.after {
            self.request_preview();
        }
    }

    /// Port of `select_all`.
    pub fn select_all(&mut self, checked: bool) {
        self.checks.fill(checked);
        self.selection_changed();
    }

    /// A click on a row's check box (`itemChanged` → `selection_changed`).
    pub fn set_checked(&mut self, index: usize, checked: bool) {
        if let Some(c) = self.checks.get_mut(index) {
            if *c != checked {
                *c = checked;
                self.selection_changed();
            }
        }
    }

    /// Port of `toggle_candidate`: a click on a candidate in the picture.
    pub fn toggle_candidate(&mut self, index: usize) {
        if index >= self.checks.len() {
            return;
        }
        self.checks[index] = !self.checks[index];
        self.selection_changed();
        self.current = index as i64;
        self.view_current = index as i64;
        self.scale = scale_of(&self.spots[index]);
    }

    /// `list.setCurrentRow(index)` with its signal: `focus_candidate`.
    pub fn set_current_row(&mut self, index: i64) {
        let index = if (0..self.spots.len() as i64).contains(&index) { index } else { -1 };
        if index != self.current {
            self.current = index;
            self.focus_candidate(index);
        }
    }

    /// Port of `focus_candidate`: the candidate at 100 %.
    pub fn focus_candidate(&mut self, index: i64) {
        self.view_current = index;
        if index >= 0 {
            self.actual(Some(index));
        }
        self.scale = usize::try_from(index).ok().and_then(|i| self.spots.get(i)).map_or(100, scale_of);
    }

    /// Port of `manual_mode` (클릭해서 놓친 먼지 추가).
    pub fn manual_mode(&mut self, enabled: bool) {
        if enabled && self.stroke_mode {
            self.manual_stroke_mode(false);
        }
        self.add_mode = enabled;
    }

    /// Port of `manual_stroke_mode` (드래그해서 흠집 추가).
    pub fn manual_stroke_mode(&mut self, enabled: bool) {
        if enabled && self.add_mode {
            self.manual_mode(false);
        }
        self.stroke_mode = enabled;
        self.view.stroke.clear();
    }

    /// Port of `add_stroke`: a manual scratch along normalized points.
    pub fn add_stroke(&mut self, points: &[[f64; 2]]) {
        let Some(data) = self.data.clone() else { return };
        if self.busy || self.result.is_none() {
            return;
        }
        if self.spots.len() + self.pending_spots.len() >= MAX_CANDIDATES + 1000 {
            self.status = tr("후보가 많습니다. 일부를 먼저 적용한 뒤 다시 표시하세요.");
            return;
        }
        match grainy_dust::shapes::manual(points, data.gray.dim(), f64::from(self.manual_size)) {
            Ok(spot) => self.insert_manual(spot),
            Err(error) => self.status = error.to_string(),
        }
    }

    /// Port of `add_candidate`: a manual spot of 수동 먼지 크기 at a normalized point.
    pub fn add_candidate(&mut self, x: f64, y: f64) {
        let Some(data) = self.data.clone() else { return };
        if self.busy || self.result.is_none() {
            return;
        }
        if self.spots.len() + self.pending_spots.len() >= MAX_CANDIDATES + 1000 {
            self.status = tr("후보가 많습니다. 일부를 먼저 적용한 뒤 다시 검출하세요.");
            return;
        }
        let (h, w) = data.gray.dim();
        let diameter = i64::from(self.manual_size).min(h.min(w) as i64);
        let d = diameter as f64;
        let spot = json!({"x": x, "y": y, "rx": d / (2.0 * w as f64), "ry": d / (2.0 * h as f64), "manual": true,
                          "polarity": "manual", "diameter": diameter, "score": 0});
        self.insert_manual(spot);
    }

    /// Port of `insert_manual`.
    pub fn insert_manual(&mut self, spot: Value) {
        self.spots.push(spot);
        self.checks.push(true);
        self.spots_version += 1;
        let index = self.spots.len() as i64 - 1;
        self.current = index;
        self.view_current = index;
        self.scale = 100;
        self.selection_changed();
        self.result_status();
    }

    /// Port of `resize_candidate` (선택 후보 제거 범위).
    pub fn resize_candidate(&mut self, value: i32) {
        self.scale = value;
        let index = self.current;
        if self.busy || !(0..self.spots.len() as i64).contains(&index) {
            return;
        }
        self.spots[index as usize]["scale"] = json!(value);
        self.spots_version += 1;
        if self.after {
            self.request_preview();
        }
    }

    /// Port of `request_preview`: the preview timer (120 ms) restarts.
    pub fn request_preview(&mut self) {
        if self.closed || self.data.is_none() {
            return;
        }
        self.preview_note =
            if self.after { tr("제거 미리보기 계산 중…") } else { tr("원본 보기 준비 중…") };
        self.preview_generation += 1;
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_due = Some(Instant::now() + PREVIEW_DELAY);
    }

    /// Port of `DustView.fit`.
    pub fn fit(&mut self) {
        self.view.fitted = true;
        let (w, h) = self.scene_size();
        let size = self.view.viewport;
        let zoom = (size.x / (w + 24.0)).min(size.y / (h + 24.0));
        self.view.zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        self.view.center = Pos2::new(w / 2.0, h / 2.0);
    }

    /// Port of `DustView.actual`: 100 %, centred on the candidate `index`.
    pub fn actual(&mut self, index: Option<i64>) {
        self.view.fitted = false;
        self.view.zoom = 1.0;
        if let Some(spot) = index.and_then(|i| usize::try_from(i).ok()).and_then(|i| self.spots.get(i)) {
            let (w, h) = self.scene_size();
            self.view.center = Pos2::new(
                (number(spot, "x") * f64::from(w - 1.0)) as f32,
                (number(spot, "y") * f64::from(h - 1.0)) as f32,
            );
        }
    }

    /// The scene rectangle's size (`sceneRect()`: the picture's size, empty before the first picture).
    pub fn scene_size(&self) -> (f32, f32) {
        match &self.picture {
            Some(p) => (p.dim().1 as f32, p.dim().0 as f32),
            None => (0.0, 0.0),
        }
    }

    /// Port of `DustView.image`: a new picture.
    fn image(&mut self, picture: Arc<Image>) {
        let first = self.picture.is_none();
        self.picture = Some(picture);
        self.picture_version += 1;
        if self.view.fitted {
            self.fit();
        } else if first && self.current >= 0 {
            self.actual(Some(self.current));
        }
    }

    /// Port of `DustView.mousePressEvent`'s candidate hit test: the candidate under the scene point.
    pub fn hit(&self, point: Pos2) -> Option<usize> {
        if !self.show_spots {
            return None;
        }
        let (w, h) = self.scene_size();
        let zoom = f64::from(self.view.zoom.max(0.01));
        let (px, py) = (f64::from(point.x), f64::from(point.y));
        let mut hits: Vec<(f64, usize)> = Vec::new();
        for (i, s) in self.spots.iter().enumerate() {
            if s.get("parts").is_some() {
                let margin = 7.0 / zoom;
                let Ok(parts) = grainy_dust::shapes::ellipses(s, (h as usize, w as usize)) else { continue };
                if parts.iter().any(|[x, y, rx, ry]| {
                    ((px - x) / (rx + margin)).powi(2) + ((py - y) / (ry + margin)).powi(2) <= 1.0
                }) {
                    hits.push((0.0, i));
                }
                continue;
            }
            let dx = px - number(s, "x") * f64::from(w - 1.0);
            let dy = py - number(s, "y") * f64::from(h - 1.0);
            let scale = f64::from(scale_of(s)) / 100.0;
            let rx = (7.0 / zoom).max(number(s, "rx") * f64::from(w) * scale);
            let ry = (7.0 / zoom).max(number(s, "ry") * f64::from(h) * scale);
            if (dx / rx).powi(2) + (dy / ry).powi(2) <= 1.0 {
                hits.push((dx * dx + dy * dy, i));
            }
        }
        hits.into_iter().min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))).map(|(_, i)| i)
    }

    /// Port of `apply_selection`: the `dust` operation and the review state; the dialog accepts.
    pub fn apply_selection(&mut self, cx: &mut AppContext) {
        if self.busy || self.result.is_none() || self.selected().is_empty() && !self.batch_review {
            return;
        }
        if !self.valid_source(cx) {
            self.status = tr("사진이나 보정값·원본이 바뀌었습니다. 현재 사진으로 다시 열어 주세요.");
            return;
        }
        let selected = self.selected();
        let result = self.result.as_ref().map_or(Value::Null, Clone::clone);
        let spots: Vec<Value> = selected.iter().map(|i| self.spots[*i].clone()).collect();
        self.operation = Some(json!({
            "type": "dust", "version": 4, "repair_method": self.repair_method, "spots": spots,
            "detection": self.detect_options, "source_size": [result["width"], result["height"]],
        }));
        self.review_state = Some(json!({
            "spots": self.spots, "pending": self.pending_spots, "checked": selected, "current": self.current,
            "repair_method": self.repair_method,
        }));
        let _ = cx.doc.save_preference("dust_repair_method", &json!(self.repair_method));
        self.accepted = true;
        self.done();
    }

    /// Port of `done`: the jobs stop and the memory is let go.
    pub fn done(&mut self) {
        self.closed = true;
        self.cancel.store(true, Ordering::Relaxed);
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_due = None;
        self.view.stroke.clear();
        self.data = None;
        self.pending_spots.clear();
    }
}

/// `spot.get('scale', 100)`.
pub fn scale_of(spot: &Value) -> i32 {
    spot.get("scale").and_then(Value::as_i64).map_or(100, |v| v as i32)
}

// ---- the jobs -----------------------------------------------------------------------------------------------

type ScanResult = Option<(Arc<Prepared>, Value, Value)>;

/// Port of `scan` (먼지 검출): detection at full resolution on a job, with progress and cancel.
pub fn scan(shared: &Shared, cx: &mut AppContext) {
    let mut d = shared.borrow_mut();
    if d.closed || d.busy {
        return;
    }
    let options = d.options();
    if let Err(message) = validate(&options) {
        d.status = message;
        return;
    }
    if d.fingerprint.is_null() || !d.valid_source(cx) {
        d.status = tr("원본을 연결하고 현재 사진으로 다시 열어 주세요.");
        return;
    }
    d.cancel = Arc::new(AtomicBool::new(false));
    let cancel = d.cancel.clone();
    let data = d.data.clone();
    d.result = None;
    d.pending_spots.clear();
    d.clear_list();
    d.set_busy(true);
    d.progress = Some((0, 0));
    *d.progress_shared.lock().unwrap_or_else(|e| e.into_inner()) = (0, 0);
    d.status = tr("원본 해상도로 먼지를 찾고 있습니다…");
    let path = d.path.clone();
    let settings = d.settings.clone();
    let fingerprint = d.fingerprint.clone();
    let cached = d.cached.take();
    let saved_review = d.saved_review.take();
    let progress = d.progress_shared.clone();
    drop(d);
    let label = tr("원본 해상도로 먼지를 찾고 있습니다…");
    let work = move |handle: &crate::jobs::JobHandle| -> Result<ScanResult, String> {
        let run = || -> Result<(Arc<Prepared>, Value, Value), String> {
            let prepared = match data {
                Some(d) => d,
                None => Arc::new(prepare_source(&path, &settings, &cancel)?),
            };
            cancelled(&cancel)?;
            let result = match cached.as_ref().filter(|c| same_options(&c["options"], &options)) {
                Some(c) => c["result"].clone(),
                None => {
                    let check = || cancel.load(Ordering::Relaxed);
                    let mut report = |done: usize, total: usize| {
                        *progress.lock().unwrap_or_else(|e| e.into_inner()) = (done, total);
                        handle.progress(done, total, "");
                    };
                    let detection = grainy_dust::detect(
                        &prepared.gray,
                        &options,
                        Some(&check),
                        Some(&mut report),
                        TILE_SIZE,
                        MAX_CANDIDATES,
                    )
                    .map_err(|e| match e {
                        grainy_dust::Error::Cancelled => CANCELLED.to_owned(),
                        other => other.to_string(),
                    })?;
                    detection_json(&detection)
                }
            };
            let now = grainy_catalog::preview::signature(&path);
            if !now.is_some_and(|s| grainy_core::settings::equivalent(&s, &fingerprint)) {
                return Err("분석 중 원본 파일이 바뀌었습니다. 사진을 다시 열어 주세요.".into());
            }
            Ok((prepared, result, options_json(&options)))
        };
        match run() {
            Ok(r) => Ok(Some(r)),
            Err(e) if e == CANCELLED => Ok(None),
            Err(e) => Err(e),
        }
    };
    let shared = shared.clone();
    cx.jobs.spawn("dust", label, work, move |cx: &mut AppContext, result: Result<ScanResult, String>| {
        let mut d = shared.borrow_mut();
        match result {
            Ok(result) => ready(&mut d, cx, result, saved_review),
            Err(error) => {
                if !d.closed {
                    d.set_busy(false);
                    d.progress = None;
                    d.status = error;
                }
            }
        }
    });
}

/// `scan`'s `ready`.
fn ready(d: &mut DustReview, cx: &mut AppContext, result: ScanResult, saved_review: Option<Value>) {
    if d.closed {
        return;
    }
    d.set_busy(false);
    d.progress = None;
    let Some((data, result, options)) = result.filter(|_| !d.cancel.load(Ordering::Relaxed)) else {
        d.status = tr("검출을 취소했습니다. 사진은 변경되지 않았습니다.");
        return;
    };
    if !d.valid_source(cx) {
        d.status = tr("사진이나 보정값이 바뀌었습니다. 현재 사진으로 다시 열어 주세요.");
        return;
    }
    d.data = Some(data);
    d.detect_options = options;
    let _ = cx.doc.save_preference("dust_detection_options", &d.detect_options);
    d.pending_spots = result["spots"].as_array().cloned().unwrap_or_default();
    d.result = Some(result);
    d.clear_list();
    if truthy(d.detect_options.get("scratches")) {
        d.pending_spots.sort_by_key(|spot| spot.get("detail_kind").and_then(Value::as_str) != Some("scratch"));
    }
    match saved_review {
        Some(saved) => {
            d.spots = saved["spots"].as_array().cloned().unwrap_or_default();
            d.pending_spots = saved["pending"].as_array().cloned().unwrap_or_default();
            let checked: Vec<i64> =
                saved["checked"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            d.checks = (0..d.spots.len() as i64).map(|i| checked.contains(&i)).collect();
            d.spots_version += 1;
            d.set_current_row(saved["current"].as_i64().unwrap_or(-1));
            d.selection_changed();
            d.result_status();
        }
        None => d.show_more(),
    }
    d.request_preview();
}

/// Port of `scan_progress`: the job's progress for the dialog's bar.
pub fn poll_progress(d: &mut DustReview) {
    if !d.closed && d.busy {
        let (done, total) = *d.progress_shared.lock().unwrap_or_else(|e| e.into_inner());
        if total > 0 {
            d.progress = Some((done, total));
        }
    }
}

/// Port of `start_preview`: the picture of the source, with the checked candidates repaired (제거 결과 미리보기)
/// and as the emphasis view (먼지 강조 보기), on a job.
pub fn start_preview(shared: &Shared, cx: &mut AppContext) {
    let mut d = shared.borrow_mut();
    d.preview_due = None;
    if d.closed {
        return;
    }
    let Some(data) = d.data.clone() else { return };
    if d.preview_running {
        d.preview_pending = true;
        return;
    }
    d.preview_running = true;
    d.preview_pending = false;
    let generation = d.preview_generation;
    d.preview_cancel = Arc::new(AtomicBool::new(false));
    let cancel = d.preview_cancel.clone();
    let spots: Vec<Value> =
        if d.after { d.selected().iter().map(|i| d.spots[*i].clone()).collect() } else { Vec::new() };
    let space = d.settings.str("working_space").to_owned();
    let (emphasis, strength, method) = (d.emphasis, d.strength, d.repair_method.clone());
    drop(d);
    let work = move |_: &crate::jobs::JobHandle| -> Result<Option<Image>, String> {
        let run = || -> Result<Image, String> {
            cancelled(&cancel)?;
            let check = || cancel.load(Ordering::Relaxed);
            let repaired;
            let image: &Image = if spots.is_empty() {
                &data.linear
            } else {
                repaired = grainy_dust::repair(&data.linear, &spots, Some(&check), &method).map_err(|e| match e {
                    grainy_dust::Error::Cancelled => CANCELLED.to_owned(),
                    other => other.to_string(),
                })?;
                &repaired
            };
            let rgb = if emphasis {
                let rgb = encoded(image, &space)?;
                let gray = grainy_dust::visualize(&gray_of(&rgb)?, f64::from(strength)).map_err(|e| e.to_string())?;
                let (h, w) = gray.dim();
                Image::from_shape_fn((h, w, 3), |(y, x, _)| gray[[y, x]])
            } else {
                encoded(image, &space)?
            };
            cancelled(&cancel)?;
            Ok(rgb)
        };
        match run() {
            Ok(rgb) => Ok(Some(rgb)),
            Err(e) if e == CANCELLED => Ok(None),
            Err(e) => Err(e),
        }
    };
    let shared = shared.clone();
    cx.jobs.spawn("dust_preview", "", work, move |cx: &mut AppContext, result: Result<Option<Image>, String>| {
        let pending = {
            let mut d = shared.borrow_mut();
            d.preview_running = false;
            if d.closed {
                return;
            }
            match result {
                Ok(image) => {
                    if let Some(image) = image.filter(|_| generation == d.preview_generation) {
                        d.image(Arc::new(image));
                        let mut note = if emphasis { tr("먼지 강조 · ") } else { String::new() };
                        note.push_str(&if d.after {
                            tr("체크된 후보 제거 미리보기")
                        } else {
                            tr("제거 전 보기")
                        });
                        d.preview_note = note;
                    }
                }
                Err(error) => d.status = error,
            }
            d.preview_pending
        };
        if pending {
            start_preview(&shared, cx);
        }
    });
}

/// The preview timer: starts the preview once its delay passed (`preview_timer.timeout`).
pub fn tick(shared: &Shared, cx: &mut AppContext, now: Instant) {
    let due = shared.borrow().preview_due.is_some_and(|due| now >= due);
    if due {
        start_preview(shared, cx);
    }
}

// ---- the window ---------------------------------------------------------------------------------------------

/// What happens when the dialog accepts (선택한 먼지 제거 / 검토 완료).
pub type OnAccept = Box<dyn FnOnce(&mut AppContext, &DustReview)>;

/// The dialog window.
pub struct DustDialog {
    pub shared: Shared,
    on_accept: Option<OnAccept>,
    texture: Option<(u64, egui::TextureHandle, f32)>,
    outlines: std::collections::HashMap<(usize, i32, u64), Vec<[Pos2; 2]>>,
    outline_version: u64,
    started: bool,
}

impl DustDialog {
    /// The dialog on `shared`; `on_accept` runs when it accepts. The first detection starts when it first shows
    /// (`QTimer.singleShot(0, self.scan)`).
    pub fn new(shared: Shared, on_accept: impl FnOnce(&mut AppContext, &DustReview) + 'static) -> Self {
        Self {
            shared,
            on_accept: Some(Box::new(on_accept)),
            texture: None,
            outlines: Default::default(),
            outline_version: 0,
            started: false,
        }
    }
}

const SPIN_WIDTH: f32 = 96.0;

fn spin(ui: &mut egui::Ui, value: &mut i32, range: std::ops::RangeInclusive<i32>, suffix: &str, enabled: bool) -> bool {
    let response =
        ui.add_enabled(enabled, egui::DragValue::new(value).range(range).suffix(suffix).speed(0.2).max_decimals(0));
    response.changed()
}

fn form_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 22.0], egui::Label::new(egui::RichText::new(label).color(t::TEXT)));
        changed = add(ui);
    });
    changed
}

fn check(ui: &mut egui::Ui, value: &mut bool, text: &str, enabled: bool, tip: Option<&str>) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        let response = widgets::checkbox(ui, value, text);
        let response = match tip {
            Some(tip) => response.on_hover_text(tip),
            None => response,
        };
        changed = response.changed();
    });
    changed
}

fn combo(ui: &mut egui::Ui, id: &str, value: &mut String, items: &[(&str, &str)], enabled: bool) -> bool {
    let before = value.clone();
    ui.add_enabled_ui(enabled, |ui| {
        let shown = items.iter().find(|(_, k)| *k == value.as_str()).map_or(String::new(), |(l, _)| tr(l));
        egui::ComboBox::from_id_salt(id).width(SPIN_WIDTH * 1.8).selected_text(shown).show_ui(ui, |ui| {
            for (label, key) in items {
                ui.selectable_value(value, (*key).to_owned(), tr(label));
            }
        });
    });
    *value != before
}

impl Dialog for DustDialog {
    fn id(&self) -> &'static str {
        "dust"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        if !self.started {
            self.started = true;
            scan(&self.shared, cx);
        }
        tick(&self.shared, cx, Instant::now());
        poll_progress(&mut self.shared.borrow_mut());
        let mut close = false;
        let screen = ctx.content_rect();
        let size = Vec2::new(1180.0f32.min(screen.width() - 20.0), 850.0f32.min(screen.height() - 20.0));
        let modal = egui::Modal::new(egui::Id::unique("dust_dialog")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(size.x - 32.0);
            ui.set_height(size.y - 32.0);
            close = self.body(ui, cx);
        });
        if modal.should_close() {
            close = true;
        }
        let busy = {
            let d = self.shared.borrow();
            d.busy || d.preview_running || d.preview_due.is_some()
        };
        if busy {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        let accepted = self.shared.borrow().accepted;
        if accepted {
            if let Some(action) = self.on_accept.take() {
                let d = self.shared.borrow();
                action(cx, &d);
            }
            return false;
        }
        if close {
            self.shared.borrow_mut().done();
            return false;
        }
        true
    }
}

impl DustDialog {
    /// The dialog's contents; returns whether 닫기 was pressed.
    fn body(&mut self, ui: &mut egui::Ui, cx: &mut AppContext) -> bool {
        let shared = self.shared.clone();
        let mut close = false;
        let title = shared.borrow().title();
        ui.label(egui::RichText::new(title).font(t::bold(14.0)).color(t::BRAND));
        ui.add_space(4.0);
        widgets::label(ui, &tr("표시된 후보를 클릭해 제외·포함하고 목록에서 100%로 확인하세요. 추가 후보는 기본 제외되며 확인한 것만 체크하세요.\n자르기·톤 보정 전 전체 사진입니다. 무늬·별도 후보가 될 수 있습니다. 재검출하면 후보 선택과 수동 표시가 초기화됩니다."), t::TEXT, t::FONT_SIZE);
        if shared.borrow().batch_review {
            widgets::label(
                ui,
                &tr("검토 완료를 누르면 목록에 보관합니다. 여러 사진 창의 적용 버튼을 눌러야 사진에 반영됩니다."),
                t::TEXT,
                t::FONT_SIZE,
            );
        }
        ui.add_space(6.0);
        let bottom = 70.0;
        let body_height = (ui.available_height() - bottom).max(200.0);
        let side_width = 346.0;
        ui.horizontal_top(|ui| {
            let view_size = Vec2::new((ui.available_width() - side_width - 12.0).max(200.0), body_height);
            self.view(ui, cx, view_size);
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(
                Vec2::new(side_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical().id_salt("dust_side").max_height(body_height).show(ui, |ui| {
                        ui.set_width(side_width - 14.0);
                        self.side(ui, cx);
                    });
                },
            );
        });
        ui.add_space(6.0);
        // 화면 맞춤, 100%, the count, 선택한 먼지 제거, 닫기.
        ui.horizontal(|ui| {
            if widgets::button(ui, &tr("화면 맞춤")).clicked() {
                shared.borrow_mut().fit();
            }
            if widgets::button(ui, "100%").clicked() {
                let mut d = shared.borrow_mut();
                let current = d.current;
                d.actual(Some(current));
            }
            let (count, apply, enabled) = {
                let d = shared.borrow();
                (d.count_text(), d.apply_text(), d.apply_enabled())
            };
            widgets::label(ui, &count, t::TEXT, t::FONT_SIZE);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::button(ui, &tr("닫기")).clicked() {
                    close = true;
                }
                ui.add_space(6.0);
                if Button::new(&apply).kind(Kind::Primary).enabled(enabled).show(ui).clicked() {
                    shared.borrow_mut().apply_selection(cx);
                }
            });
        });
        let (progress, status) = {
            let d = shared.borrow();
            (d.progress, d.status.clone())
        };
        if let Some((done, total)) = progress {
            let bar = if total == 0 {
                egui::ProgressBar::new(0.0).animate(true)
            } else {
                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
            };
            ui.add(bar.desired_height(6.0));
        }
        widgets::label(ui, &status, t::TEXT, t::FONT_SIZE);
        close
    }

    /// The controls on the right.
    fn side(&mut self, ui: &mut egui::Ui, cx: &mut AppContext) {
        let shared = self.shared.clone();
        ui.spacing_mut().item_spacing.y = 5.0;
        let mut changed = false;
        {
            let mut d = shared.borrow_mut();
            let idle = !d.busy;
            let d = &mut *d;
            changed |= form_row(ui, &tr("검출 민감도"), |ui| spin(ui, &mut d.sensitivity, 1..=100, "", idle));
            changed |= form_row(ui, &tr("최소 점 크기"), |ui| spin(ui, &mut d.minimum, 2..=160, " px", idle));
            changed |= form_row(ui, &tr("최대 점 크기"), |ui| spin(ui, &mut d.maximum, 2..=160, " px", idle));
            changed |=
                form_row(ui, &tr("찾을 종류"), |ui| combo(ui, "dust_polarity", &mut d.polarity, &POLARITIES, idle));
            changed |= check(ui, &mut d.detailed, &tr("경계·미세 먼지까지 정밀 검색"), idle, Some(&tr("작은 크기를 추가 탐색합니다. 무늬 근처의 추가 후보는 자동으로 체크하지 않습니다. 처리 시간이 더 걸립니다.")));
            let enabled = d.soft_enabled();
            changed |= check(ui, &mut d.soft, &tr("입자 속 옅은 먼지 추가 검색"), enabled, Some(&tr("입자가 많은 사진의 옅은 점을 추가로 찾습니다. 시간이 더 걸리고 무늬도 후보에 포함될 수 있습니다. 추가 후보는 확인한 뒤 체크하세요.")));
            let enabled = d.suppress_enabled();
            changed |= check(ui, &mut d.suppress_grain, &tr("입자 후보 억제"), enabled, Some(&tr("옅은 점의 검출 기준을 높여 검토 후보를 줄입니다. 옅은 먼지도 놓칠 수 있습니다. 후보가 너무 많을 때 켜고 필요하면 끄고 다시 검색하세요.")));
            let enabled = d.reduce_enabled();
            changed |= check(ui, &mut d.reduce_patterns, &tr("무늬 후보 줄이기"), enabled, Some(&tr("정밀 후보는 반복 무늬와 비교하고, 긴 흠집은 넓은 물체와 붙은 부분을 추가로 제외합니다. 실제 먼지나 흠집도 놓칠 수 있으므로 필요에 따라 끄고 다시 검출하세요.")));
            changed |= check(ui, &mut d.scratches, &tr("긴 흠집·머리카락 후보 찾기"), idle, Some(&tr("설정한 최대 굵기보다 충분히 긴 검은 선·흰 흠집을 추가로 찾습니다. 짧은 점은 위 점 검색을 사용하세요. 전선·가지·글자도 잡힐 수 있어 확인한 것만 체크하세요. 시간은 더 걸립니다.")));
            let enabled = d.scratch_width_enabled();
            changed |=
                form_row(ui, &tr("흠집 최대 굵기"), |ui| spin(ui, &mut d.scratch_width, 2..=40, " px", enabled));
            if changed {
                d.options_changed();
            }
        }
        let (scan_enabled, stop_enabled) = {
            let d = shared.borrow();
            (!d.busy, d.stop_enabled())
        };
        let labels = [tr("먼지 검출"), tr("검출 취소")];
        let width = ui.available_width();
        match widgets::button_row(
            ui,
            &[Button::new(&labels[0]).enabled(scan_enabled), Button::new(&labels[1]).enabled(stop_enabled)],
            width,
            4.0,
        ) {
            Some(0) => scan(&shared, cx),
            Some(_) => shared.borrow_mut().cancel_scan(),
            None => {}
        }
        self.list(ui);
        let (more, more_text, more_enabled) = {
            let d = shared.borrow();
            let (visible, text) = d.more();
            (visible, text, d.more_enabled())
        };
        if more && Button::new(&more_text).width(width).enabled(more_enabled).show(ui).clicked() {
            shared.borrow_mut().show_more();
        }
        let labels = [tr("목록 모두 포함"), tr("목록 모두 제외")];
        match widgets::button_row(ui, &[Button::new(&labels[0]), Button::new(&labels[1])], width, 4.0) {
            Some(0) => shared.borrow_mut().select_all(true),
            Some(_) => shared.borrow_mut().select_all(false),
            None => {}
        }
        let mut d = shared.borrow_mut();
        let d = &mut *d;
        let mut show = d.show_spots;
        if check(ui, &mut show, &tr("먼지 후보 표시"), true, None) {
            d.show_spots = show;
        }
        let mut emphasis = d.emphasis;
        if check(ui, &mut emphasis, &tr("먼지 강조 보기"), true, None) {
            d.emphasis = emphasis;
            d.request_preview();
        }
        if form_row(ui, &tr("강조 강도"), |ui| spin(ui, &mut d.strength, 1..=100, "", true)) {
            d.request_preview();
        }
        let manual = d.manual_enabled();
        let mut add = d.add_mode;
        if check(ui, &mut add, &tr("클릭해서 놓친 먼지 추가"), manual, None) {
            d.manual_mode(add);
        }
        let mut stroke = d.stroke_mode;
        if check(ui, &mut stroke, &tr("드래그해서 흠집 추가"), manual, None) {
            d.manual_stroke_mode(stroke);
        }
        if form_row(ui, &tr("수동 먼지 크기"), |ui| spin(ui, &mut d.manual_size, 2..=160, " px", true)) {
            d.view.stroke_diameter = d.manual_size as f32;
        }
        let scale_enabled = d.scale_enabled();
        let mut scale = d.scale;
        if form_row(ui, &tr("선택 후보 제거 범위"), |ui| spin(ui, &mut scale, 25..=250, " %", scale_enabled)) {
            d.resize_candidate(scale);
        }
        let mut after = d.after;
        if check(ui, &mut after, &tr("제거 결과 미리보기"), true, None) {
            d.after = after;
            d.request_preview();
        }
        let tip = tr("주변 질감 살리기는 가까운 깨끗한 영역의 입자를 가져옵니다. 무늬가 어색하면 부드럽게 연결로 바꾸고 제거 미리보기에서 비교하세요. 이전에 적용한 보정은 바뀌지 않습니다.");
        if form_row(ui, &tr("복구 방식"), |ui| {
            let changed = combo(ui, "dust_method", &mut d.repair_method, &METHODS, true);
            ui.response().on_hover_text(tip);
            changed
        }) {
            d.request_preview();
        }
        if !d.preview_note.is_empty() {
            widgets::label(ui, &d.preview_note, t::MUTED, t::FONT_SIZE);
        }
    }

    /// The candidate list (`QListWidget` with check boxes).
    fn list(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let rows = shared.borrow().spots.len();
        let height = 220.0;
        egui::Frame::new().fill(t::LIST_BG).stroke(Stroke::new(1.0, t::INPUT_BORDER)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("dust_list").max_height(height).min_scrolled_height(height).show_rows(
                ui,
                20.0,
                rows,
                |ui, range| {
                    for index in range {
                        let (text, mut checked, current) = {
                            let d = shared.borrow();
                            (d.item_text(index), d.checks[index], d.current == index as i64)
                        };
                        ui.horizontal(|ui| {
                            let box_changed = widgets::checkbox(ui, &mut checked, "").changed();
                            let label = ui.selectable_label(current, egui::RichText::new(&text).color(t::TEXT));
                            if box_changed {
                                shared.borrow_mut().set_checked(index, checked);
                            }
                            if label.clicked() {
                                shared.borrow_mut().set_current_row(index as i64);
                            }
                        });
                    }
                },
            );
        });
    }

    /// The picture with the candidates (`DustView`).
    fn view(&mut self, ui: &mut egui::Ui, cx: &mut AppContext, size: Vec2) {
        let shared = self.shared.clone();
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x10, 0x12, 0x15));
        {
            let mut d = shared.borrow_mut();
            if d.view.viewport != size {
                d.view.viewport = size;
                if d.view.fitted {
                    d.fit();
                }
            }
        }
        let (zoom, center, scene) = {
            let d = shared.borrow();
            (d.view.zoom, d.view.center, d.scene_size())
        };
        let to_screen = |p: Pos2| rect.center() + (p - center) * zoom;
        let to_scene = |p: Pos2| center + (p - rect.center()) / zoom;
        // The picture.
        let picture = {
            let d = shared.borrow();
            d.picture.clone().map(|p| (d.picture_version, p))
        };
        if let Some((version, picture)) = picture {
            if self.texture.as_ref().is_none_or(|(v, _, _)| *v != version) {
                let max = ui.ctx().input(|i| i.max_texture_side).max(1);
                let (h, w, _) = picture.dim();
                let step = w.max(h).div_ceil(max).max(1);
                let (tw, th) = (w.div_ceil(step), h.div_ceil(step));
                let mut rgba = Vec::with_capacity(tw * th * 4);
                for y in 0..th {
                    for x in 0..tw {
                        for c in 0..3 {
                            rgba.push((picture[[y * step, x * step, c]].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                        }
                        rgba.push(255);
                    }
                }
                let image = egui::ColorImage::from_rgba_unmultiplied([tw, th], &rgba);
                let texture = ui.ctx().load_texture("dust_picture", image, egui::TextureOptions::LINEAR);
                self.texture = Some((version, texture, step as f32));
            }
        }
        if let Some((_, texture, _)) = &self.texture {
            let r = Rect::from_min_max(to_screen(Pos2::ZERO), to_screen(Pos2::new(scene.0, scene.1)));
            painter.image(texture.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        // Wheel zoom (`wheelEvent`).
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let mut d = shared.borrow_mut();
                let factor = if scroll > 0.0 { 1.2 } else { 1.0 / 1.2 };
                d.view.fitted = false;
                if (0.015..16.0).contains(&(d.view.zoom * factor)) {
                    // AnchorUnderMouse: the scene point under the pointer stays.
                    if let Some(pointer) = response.hover_pos() {
                        let anchor = to_scene(pointer);
                        d.view.zoom *= factor;
                        d.view.center = anchor - (pointer - rect.center()) / d.view.zoom;
                    } else {
                        d.view.zoom *= factor;
                    }
                }
            }
        }
        let clamp =
            |p: Pos2| Pos2::new(p.x.clamp(0.0, (scene.0 - 1.0).max(0.0)), p.y.clamp(0.0, (scene.1 - 1.0).max(0.0)));
        let inside = |p: Pos2| p.x >= 0.0 && p.y >= 0.0 && p.x <= scene.0 && p.y <= scene.1;
        let (stroke_mode, add_mode, has_picture) = {
            let d = shared.borrow();
            (d.stroke_mode, d.add_mode, d.picture.is_some())
        };
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(start) = response.interact_pointer_pos() {
                let point = to_scene(start);
                if stroke_mode && has_picture && inside(point) {
                    shared.borrow_mut().view.stroke = vec![point];
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            let mut d = shared.borrow_mut();
            if !d.view.stroke.is_empty() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let point = clamp(to_scene(pointer));
                    let last = *d.view.stroke.last().unwrap_or(&point);
                    let step = (0.5f32).max(d.view.stroke_diameter * 0.2);
                    if (point - last).length_sq() >= step * step {
                        d.view.stroke.push(point);
                        if d.view.stroke.len() > 8192 {
                            let last = *d.view.stroke.last().unwrap_or(&point);
                            let mut kept: Vec<Pos2> = d.view.stroke.iter().step_by(2).copied().collect();
                            kept.push(last);
                            d.view.stroke = kept;
                        }
                    }
                }
            } else if !stroke_mode && !add_mode {
                // ScrollHandDrag.
                d.view.fitted = false;
                let delta = response.drag_delta() / d.view.zoom;
                d.view.center -= delta;
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            let mut d = shared.borrow_mut();
            if !d.view.stroke.is_empty() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let point = clamp(to_scene(pointer));
                    d.view.stroke.push(point);
                }
                let (w, h) = (f64::from((scene.0 - 1.0).max(1.0)), f64::from((scene.1 - 1.0).max(1.0)));
                let points: Vec<[f64; 2]> =
                    d.view.stroke.iter().map(|p| [f64::from(p.x) / w, f64::from(p.y) / h]).collect();
                d.view.stroke.clear();
                d.add_stroke(&points);
            }
        }
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(pointer) = response.interact_pointer_pos() {
                let point = to_scene(pointer);
                let mut d = shared.borrow_mut();
                if stroke_mode && has_picture && inside(point) {
                    // A click without a move is a stroke of one point.
                    let (w, h) = (f64::from((scene.0 - 1.0).max(1.0)), f64::from((scene.1 - 1.0).max(1.0)));
                    let p = clamp(point);
                    let q = [f64::from(p.x) / w, f64::from(p.y) / h];
                    d.add_stroke(&[q, q]);
                } else if add_mode && has_picture && inside(point) {
                    let (w, h) = (f64::from((scene.0 - 1.0).max(1.0)), f64::from((scene.1 - 1.0).max(1.0)));
                    d.add_candidate((f64::from(point.x) / w).clamp(0.0, 1.0), (f64::from(point.y) / h).clamp(0.0, 1.0));
                } else if let Some(index) = d.hit(point) {
                    d.toggle_candidate(index);
                }
            }
        }
        // The candidates (`drawForeground`).
        let d = shared.borrow();
        if d.spots_version != self.outline_version {
            self.outlines.clear();
            self.outline_version = d.spots_version;
        }
        if !d.view.stroke.is_empty() {
            let points: Vec<Pos2> = d.view.stroke.iter().map(|p| to_screen(*p)).collect();
            let width = d.view.stroke_diameter * zoom;
            let colour = Color32::from_rgb(0xff, 0xe2, 0xa1);
            painter.add(egui::Shape::line(points.clone(), Stroke::new(width, colour)));
            for p in [points[0], *points.last().unwrap_or(&points[0])] {
                painter.circle_filled(p, width / 2.0, colour);
            }
        }
        if d.show_spots {
            let (w, h) = scene;
            for (i, s) in d.spots.iter().enumerate() {
                let checked = d.checks.get(i).copied().unwrap_or(false);
                let colour = if i as i64 == d.view_current {
                    Color32::from_rgb(0xff, 0xe2, 0xa1)
                } else if checked {
                    Color32::from_rgb(0xff, 0x6d, 0x52)
                } else if truthy(s.get("review")) {
                    Color32::from_rgb(0xd9, 0xb1, 0x63)
                } else {
                    Color32::from_rgb(0x8e, 0x95, 0x9b)
                };
                let width = if i as i64 == d.view_current { 2.5 } else { 1.3 };
                let stroke = Stroke::new(width, colour);
                if s.get("parts").is_some() {
                    let key = (i, scale_of(s), d.spots_version);
                    let segments = self.outlines.entry(key).or_insert_with(|| outline(s, (h as usize, w as usize)));
                    for (n, [a, b]) in segments.iter().enumerate() {
                        if !checked && n % 2 == 1 {
                            continue;
                        }
                        let (a, b) = (to_screen(*a), to_screen(*b));
                        if rect.expand(4.0).contains(a) || rect.expand(4.0).contains(b) {
                            painter.line_segment([a, b], stroke);
                        }
                    }
                    continue;
                }
                let scale = scale_of(s) as f32 / 100.0;
                let c = to_screen(Pos2::new(
                    (number(s, "x") * f64::from(w - 1.0)) as f32,
                    (number(s, "y") * f64::from(h - 1.0)) as f32,
                ));
                let rx = (5.0f32).max(number(s, "rx") as f32 * w * scale * zoom);
                let ry = (5.0f32).max(number(s, "ry") as f32 * h * scale * zoom);
                if !rect.expand2(Vec2::new(rx + 3.0, ry + 3.0)).contains(c) {
                    continue;
                }
                if checked {
                    painter.add(egui::Shape::ellipse_stroke(c, Vec2::new(rx, ry), stroke));
                } else {
                    let points: Vec<Pos2> = (0..=48)
                        .map(|k| {
                            let a = k as f32 / 48.0 * std::f32::consts::TAU;
                            c + Vec2::new(rx * a.cos(), ry * a.sin())
                        })
                        .collect();
                    painter.extend(egui::Shape::dashed_line(&points, stroke, 4.0, 3.0));
                }
            }
        }
        drop(d);
        let _ = cx;
    }
}

/// The outline of a spot made of parts (`candidate_path`'s simplified union), as segments in scene pixels.
fn outline(spot: &Value, shape: (usize, usize)) -> Vec<[Pos2; 2]> {
    let Ok(parts) = grainy_dust::shapes::ellipses(spot, shape) else { return Vec::new() };
    let inside = |x: f64, y: f64, skip: usize| {
        parts
            .iter()
            .enumerate()
            .any(|(j, [cx, cy, rx, ry])| j != skip && ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) < 0.999)
    };
    let mut segments = Vec::new();
    const SAMPLES: usize = 24;
    for (i, [cx, cy, rx, ry]) in parts.iter().enumerate() {
        let point = |k: usize| {
            let a = k as f64 / SAMPLES as f64 * std::f64::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        };
        for k in 0..SAMPLES {
            let (a, b) = (point(k), point(k + 1));
            if !inside(a.0, a.1, i) && !inside(b.0, b.1, i) {
                segments.push([Pos2::new(a.0 as f32, a.1 as f32), Pos2::new(b.0 as f32, b.1 as f32)]);
            }
        }
    }
    segments
}

thread_local! {
    static LAST: RefCell<std::rc::Weak<RefCell<DustReview>>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// Opens the dialog of the open photo (`StudioTools.detect_dust` up to `exec`); `on_accept` gets the review.
pub fn open(cx: &mut AppContext, on_accept: impl FnOnce(&mut AppContext, &DustReview) + 'static) -> Shared {
    let shared = Rc::new(RefCell::new(DustReview::new(cx, None)));
    LAST.with(|last| *last.borrow_mut() = Rc::downgrade(&shared));
    cx.open_dialog(Box::new(DustDialog::new(shared.clone(), on_accept)));
    shared
}

/// The review [`open`] opened last on this thread, while its window lives (tests drive the shown dialog).
pub fn last_opened() -> Option<Shared> {
    LAST.with(|last| last.borrow().upgrade())
}
