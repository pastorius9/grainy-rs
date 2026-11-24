//! Export: the options of the export dialog (luma/app.py `ExportDialog`), the saved "previous export" settings
//! (`quick_export.previous` / `PREFERENCE`), the catalogue records an export works on, the new file names
//! (`photo_service.export_plans` through `grainy_service`, `{stem}_grainy` with a number when a name is taken)
//! and the export run (a background job of the window): `photo_service.export_records` through `grainy_service`
//! with `grainy_engine::files::export_photo` for the files and `quick_export.copy_original` for "원본 + 설정".

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};

use grainy_codecs::SizeLimit;
use grainy_core::Settings;
use grainy_engine::files::{export_photo_with_gpu, parse_format, ExportRequest};
use grainy_library::pyformat::{self, Arg};
use grainy_service::export::{ExportJob, Exporter, Longest};
use grainy_service::Failure;
use serde_json::{json, Map, Value};

use crate::render::Waker;

/// Stable format indices; append new formats so existing selections keep their meaning.
/// "Original" copies the source and writes a Grainy XMP.
pub const FORMATS: [&str; 10] = [
    "JPEG",
    "AVIF",
    "JPEG XL",
    "PNG",
    "PNG 16-bit",
    "TIFF 16-bit",
    "TIFF 8-bit",
    "TIFF HDR 32-bit",
    "Original",
    "AVIF HDR 10-bit",
];

/// The indices in [`FORMATS`] the dialog lists, in order (HDR outputs are skipped while
/// `engine.HDR_FEATURE` is off).
pub fn listed_formats() -> Vec<usize> {
    (0..FORMATS.len())
        .filter(|i| !matches!(FORMATS[*i], "TIFF HDR 32-bit" | "AVIF HDR 10-bit") || crate::hdr::enabled())
        .collect()
}
/// The output colour spaces of the dialog.
pub const COLOR_SPACES: [&str; 4] = ["sRGB", "Adobe RGB", "Display P3", "ProPhoto RGB"];
/// The default naming (`ExportDialog.naming`).
pub const NAMING: &str = "{stem}_grainy";

/// The text of a format in the dialog's list (`원본 + 설정` for `Original`, Korean source).
pub fn format_label(format: &str) -> &str {
    if format == "Original" {
        "원본 + 설정"
    } else {
        format
    }
}

/// The export dialog's values.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOptions {
    pub folder: String,
    /// Index in [`FORMATS`].
    pub format: usize,
    /// 40–100.
    pub quality: u32,
    /// Long side in pixels; 0 keeps the original size.
    pub longest: u32,
    /// Index in [`COLOR_SPACES`].
    pub color_space: usize,
    pub keep_metadata: bool,
    pub grayscale: bool,
    pub dither: bool,
    pub naming: String,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            folder: String::new(),
            format: 0,
            quality: 95,
            longest: 0,
            color_space: 0,
            keep_metadata: false,
            grayscale: true,
            dither: true,
            naming: NAMING.to_owned(),
        }
    }
}

/// Which fields apply to the chosen format (`format_changed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fields {
    pub quality: bool,
    pub color_space: bool,
    pub longest: bool,
    pub keep_metadata: bool,
    pub grayscale: bool,
    pub dither: bool,
}

/// The naming check of the dialog and of `quick_export.previous`: the name for `stem='photo', n=1` is not empty,
/// is one file name and has no character Windows refuses.
pub fn naming_ok(naming: &str) -> bool {
    let Ok(name) = pyformat::format(naming, &[("stem", Arg::Str("photo".into())), ("n", Arg::Int(1))]) else {
        return false;
    };
    let platform = grainy_library::Platform::native();
    !name.trim().is_empty()
        && grainy_library::pathx::name(platform, &name) == name
        && !name.chars().any(|c| "<>:\"/\\|?*".contains(c))
}

impl ExportOptions {
    /// The format string.
    pub fn format_name(&self) -> &'static str {
        FORMATS[self.format.min(FORMATS.len() - 1)]
    }

    /// Whether the format writes the XMP sidecar and copies the original (`원본 + 설정`).
    pub fn is_original(&self) -> bool {
        self.format_name() == "Original"
    }

    /// Port of `format_changed`: which fields apply to the chosen format.
    pub fn fields(&self) -> Fields {
        let f = self.format_name();
        Fields {
            quality: matches!(f, "JPEG" | "AVIF" | "AVIF HDR 10-bit"),
            color_space: !matches!(f, "AVIF HDR 10-bit" | "TIFF HDR 32-bit" | "Original" | "JPEG XL"),
            longest: f != "Original",
            keep_metadata: f != "Original",
            grayscale: matches!(f, "JPEG" | "PNG" | "TIFF 8-bit" | "TIFF 16-bit"),
            dither: matches!(f, "JPEG" | "AVIF" | "PNG" | "TIFF 8-bit"),
        }
    }

    /// `(quality, colour space, grayscale, dither)` of [`ExportOptions::fields`].
    pub fn enabled(&self) -> (bool, bool, bool, bool) {
        let f = self.fields();
        (f.quality, f.color_space, f.grayscale, f.dither)
    }

    /// Port of the dialog's `format_changed` side effect: JPEG XL is written in sRGB only.
    pub fn set_format(&mut self, index: usize) {
        self.format = index.min(FORMATS.len() - 1);
        if self.format_name() == "JPEG XL" {
            self.color_space = 0;
        }
    }

    /// Port of `ExportDialog.validate`: the Korean source of the message when the values cannot be used.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.folder.trim().is_empty() || !Path::new(&self.folder).is_dir() {
            return Err("저장할 폴더를 선택해 주세요.");
        }
        if !naming_ok(&self.naming) {
            return Err("{stem}, {n:03d}와 유효한 파일 이름만 사용하세요.");
        }
        Ok(())
    }

    /// The values `start_export` keeps as the "previous export" (`options` of `start_export`, in its order).
    pub fn saved(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("folder".into(), self.folder.clone().into());
        map.insert("format".into(), self.format_name().into());
        map.insert("quality".into(), self.quality.into());
        map.insert("longest".into(), self.longest.into());
        map.insert("color_space".into(), COLOR_SPACES[self.color_space.min(3)].into());
        map.insert("keep_metadata".into(), self.keep_metadata.into());
        map.insert("naming".into(), self.naming.clone().into());
        map.insert("grayscale".into(), self.grayscale.into());
        map.insert("dither".into(), self.dither.into());
        map
    }

    /// The dialog's values for a saved export (`quick_export.previous`'s result, or a preset with a folder).
    pub fn from_saved(saved: &Map<String, Value>) -> Self {
        let text = |key: &str| saved.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        let flag = |key: &str| saved.get(key).and_then(Value::as_bool).unwrap_or(true);
        let format = text("format");
        let space = text("color_space");
        Self {
            folder: text("folder"),
            format: FORMATS.iter().position(|f| *f == format).unwrap_or(0),
            quality: saved.get("quality").and_then(Value::as_u64).unwrap_or(95) as u32,
            longest: saved.get("longest").and_then(Value::as_u64).unwrap_or(0) as u32,
            color_space: COLOR_SPACES.iter().position(|c| *c == space).unwrap_or(0),
            keep_metadata: saved.get("keep_metadata").and_then(Value::as_bool).unwrap_or(false),
            grayscale: flag("grayscale"),
            dither: flag("dither"),
            naming: text("naming"),
        }
    }

    /// The options of `photo_service.export_records` (the values go through as the oracle passes them).
    pub fn service(&self) -> grainy_service::export::ExportOptions {
        grainy_service::export::ExportOptions {
            format: self.format_name().to_owned(),
            quality: i64::from(self.quality),
            longest: Longest::Side(i64::from(self.longest)),
            color_space: COLOR_SPACES[self.color_space.min(3)].to_owned(),
            keep_metadata: self.keep_metadata,
            naming: self.naming.clone(),
            grayscale: self.grayscale,
            dither: self.dither,
        }
    }

    /// The engine's request for these values (a plain photo without catalogue metadata).
    pub fn request(&self) -> Result<ExportRequest, String> {
        let format =
            parse_format(self.format_name()).ok_or_else(|| format!("unknown format {}", self.format_name()))?;
        Ok(ExportRequest {
            format,
            quality: f64::from(self.quality.clamp(40, 100)),
            longest: (self.longest > 0).then_some(SizeLimit::Longest(self.longest as usize)),
            color_space: COLOR_SPACES[self.color_space.min(3)].to_owned(),
            keep_metadata: self.keep_metadata,
            grayscale: self.grayscale,
            dither: self.dither,
            ..ExportRequest::default()
        })
    }
}

/// Port of `quick_export.previous` on the stored preference: the saved export settings when they are valid.
pub fn previous(stored: Option<&Value>) -> Option<Map<String, Value>> {
    grainy_library::quick_export::validate_previous(stored?, crate::hdr::enabled())
}

/// The record of a photo that is not in a catalogue (a session): what `export_records` reads of a
/// `Catalog.photo` row.
pub fn session_record(id: u64, path: &Path, settings: &Settings, info: &Value) -> Value {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    json!({
        "id": id,
        "name": name,
        "path": path.to_string_lossy(),
        "settings": Value::Object(settings.0.clone()),
        "info": if info.is_object() { info.clone() } else { json!({}) },
        "user_metadata": {},
        "keywords": "",
        "rating": 0,
        "label": "",
    })
}

/// Port of `[self.catalog.photo(i) for i in ids]` of `start_export`: the catalogue rows of `ids` with the settings
/// of the window (`settings[i]`, which equal the stored ones after the commit). A photo that is gone from the
/// catalogue is an error (the oracle's `TypeError` on `None`).
pub fn catalog_records(
    catalog: &grainy_catalog::Catalog,
    ids: &[u64],
    settings: &[Settings],
) -> Result<Vec<Value>, String> {
    let mut records = Vec::with_capacity(ids.len());
    for (id, settings) in ids.iter().zip(settings) {
        let mut row = catalog
            .photo(*id as i64)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("photo {id} is not in the catalogue"))?;
        row["settings"] = Value::Object(settings.0.clone());
        records.push(row);
    }
    Ok(records)
}

/// The keys of the oracle's `engine.DEFAULTS` in its insertion order. `normalized()` returns the settings in this
/// order and `json.dumps` keeps it, so the XMP sidecar of "원본 + 설정" (`luma:Settings`) has the keys in this
/// order; the Rust defaults are sorted alphabetically. A test compares the list with the oracle's.
pub const DEFAULTS_ORDER: &[&str] = &[
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "temperature",
    "tint",
    "saturation",
    "vibrance",
    "clarity",
    "sharpen",
    "vignette",
    "grain",
    "vignette_midpoint",
    "vignette_roundness",
    "vignette_feather",
    "vignette_highlights",
    "grain_size",
    "grain_roughness",
    "rotation",
    "straighten",
    "flip",
    "crop",
    "curve",
    "hsl",
    "monochrome",
    "video_color",
    "video_time",
    "hdr",
    "hdr_limit",
    "hdr_brightness",
    "sdr_exposure",
    "sdr_compression",
    "photo_filter",
    "photo_filter_enabled",
    "photo_filter_density",
    "photo_filter_luminosity",
    "tone_version",
    "detail_version",
    "rgb_curves",
    "parametric",
    "mixer_mode",
    "kelvin",
    "wb_reference",
    "kelvin_enabled",
    "grading",
    "grading_balance",
    "calibration",
    "point_colors",
    "texture",
    "dehaze",
    "noise_luma",
    "noise_color",
    "sharpen_radius",
    "sharpen_detail",
    "sharpen_mask",
    "distortion",
    "distortion_k2",
    "distortion_k3",
    "lens_vignette",
    "ca_red",
    "ca_blue",
    "defringe",
    "perspective_v",
    "perspective_h",
    "aspect_scale",
    "shift_x",
    "shift_y",
    "transform_scale",
    "guides",
    "lens_profile",
    "camera_matrix",
    "lensfun",
    "lensfun_enabled",
    "lensfun_distortion",
    "lensfun_tca",
    "lensfun_vignette",
    "lensfun_scale",
    "upright",
    "upright_crop",
    "masks",
    "retouch",
    "working_space",
    "raw_mode",
    "raw_kelvin",
    "raw_tint",
    "raw_neutral",
    "dcp_profile",
    "dcp_huesat",
    "dcp_look",
    "dcp_tone",
    "dcp_exposure",
    "negative",
    "negative_base",
    "negative_peak",
    "negative_range",
    "negative_density",
    "film_look",
    "film_strength",
    "tone_mapping",
    "drt_contrast",
    "drt_toe",
    "drt_shoulder",
    "drt_white",
    "panels_off",
];

/// The settings with their keys in the oracle's order ([`DEFAULTS_ORDER`]; other keys follow as they were).
pub fn python_order(settings: &Map<String, Value>) -> Map<String, Value> {
    let mut ordered = Map::new();
    for key in DEFAULTS_ORDER {
        if let Some(value) = settings.get(*key) {
            ordered.insert((*key).to_owned(), value.clone());
        }
    }
    for (key, value) in settings {
        if !ordered.contains_key(key) {
            ordered.insert(key.clone(), value.clone());
        }
    }
    ordered
}

/// `engine.export_image` and `quick_export.copy_original` for `photo_service.export_records`.
/// Lazily initializes one hardware GPU context per batch (bounding full-res VRAM peak via a
/// mutex during the brief GPU compute pass while all worker threads decode RAW & encode files
/// in parallel).
pub struct AppExporter {
    gpu: std::sync::Mutex<Option<grainy_gpu::Gpu>>,
}

impl Default for AppExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl AppExporter {
    pub fn new() -> Self {
        Self::with_gpu(true)
    }

    fn with_gpu(enabled: bool) -> Self {
        let use_gpu = enabled
            && std::env::var("GRAINY_GPU").as_deref() != Ok("0")
            && !crate::context::FROZEN_TIMING.load(std::sync::atomic::Ordering::Relaxed);
        let gpu = if use_gpu {
            grainy_gpu::Gpu::with_adapter(grainy_gpu::AdapterKind::Hardware).ok().flatten()
        } else {
            None
        };
        Self { gpu: std::sync::Mutex::new(gpu) }
    }
}

fn failure_of(error: &grainy_engine::files::Error) -> Failure {
    use grainy_engine::files::Error as E;
    match error {
        E::Io(e) | E::Codecs(grainy_codecs::Error::Io(e)) => Failure::io(e),
        _ => Failure::new("ValueError", error.to_string()),
    }
}

impl Exporter for AppExporter {
    fn export_image(&self, job: &ExportJob<'_>) -> Result<Value, Failure> {
        // `folder_sync.note_output`: a file Grainy writes is not a new photo for the automatic folder import.
        grainy_library::folder_sync::note_output(
            grainy_library::Platform::native(),
            &grainy_library::pathx::text(job.destination),
        );
        let options = job.options;
        let format =
            parse_format(&options.format).ok_or_else(|| Failure::new("KeyError", format!("'{}'", options.format)))?;
        let longest = job.longest.map(|l| match l {
            Longest::Side(n) => SizeLimit::Longest((*n).max(0) as usize),
            Longest::Box(w, h, fit) => {
                SizeLimit::Box { width: (*w).max(0) as usize, height: (*h).max(0) as usize, cover: fit == "cover" }
            }
        });
        let get = |key: &str| job.record.get(key).cloned().unwrap_or(Value::Null);
        let request = ExportRequest {
            format,
            quality: options.quality as f64,
            longest,
            color_space: options.color_space.clone(),
            keep_metadata: options.keep_metadata,
            user_metadata: get("user_metadata").as_object().cloned().unwrap_or_default(),
            keywords: get("keywords").as_str().unwrap_or_default().to_owned(),
            grayscale: options.grayscale,
            dither: options.dither,
            jxl_lossless: format == grainy_codecs::ExportFormat::JpegXl,
        };
        let path = get("path");
        let settings = get("settings").as_object().cloned().unwrap_or_default();
        let (w, h) = export_photo_with_gpu(
            path.as_str().unwrap_or_default(),
            job.destination,
            &settings,
            &request,
            Some(&self.gpu),
        )
        .map_err(|e| failure_of(&e))?;
        Ok(json!([w, h]))
    }

    fn copy_original(&self, record: &Value, destination: &Path) -> Result<Value, Failure> {
        // The sidecar writes the settings as they are given: in the oracle's key order, as `normalized()` has them.
        let mut record = record.clone();
        if let Some(settings) = record.get("settings").and_then(Value::as_object) {
            record["settings"] = Value::Object(python_order(settings));
        }
        let (w, h) = grainy_library::quick_export::copy_original(&record, destination)
            .map_err(|e| Failure::new("OSError", e.to_string()))?;
        Ok(json!([w, h]))
    }
}

/// What an export run reports (`photo_service.export_records`' result).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// `{path, size, bytes, photo_id, edit_signature}` of each file written, in the order of the photos.
    pub outputs: Vec<Value>,
    /// `name: message` per photo that failed.
    pub failures: Vec<String>,
    /// Photos not started because the run was cancelled.
    pub skipped: usize,
}

impl Outcome {
    /// The files written.
    pub fn written(&self) -> Vec<PathBuf> {
        self.outputs.iter().filter_map(|o| o["path"].as_str().map(PathBuf::from)).collect()
    }

    /// What `start_export` hands `record_exports`: `path`, `photo_id`, `edit_signature` of each output.
    pub fn history_items(&self) -> Vec<Value> {
        self.outputs
            .iter()
            .map(|o| json!({"path": o["path"], "photo_id": o["photo_id"], "edit_signature": o["edit_signature"]}))
            .collect()
    }
}

/// Port of `photo_service.export_records` for the dialog's values: `progress(done, total)` after each photo; a
/// cancel skips the photos not started. An existing file is never overwritten (names are planned around the
/// files in the folder and the writers create files exclusively).
pub fn run_records(
    records: &[Value],
    options: &ExportOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> Result<Outcome, String> {
    run_records_with_gpu(records, options, progress, cancel, true)
}

pub(crate) fn run_records_with_gpu(
    records: &[Value],
    options: &ExportOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
    enabled: bool,
) -> Result<Outcome, String> {
    crate::paths::writable(Path::new(&options.folder)).map_err(|e| e.to_string())?;
    let total = records.len();
    let workers = grainy_library::quick_export::rs_export_concurrency_here(records).map_err(|e| e.to_string())?;
    let report = |done: usize| progress(done, total);
    let exporter = AppExporter::with_gpu(enabled);
    let result = grainy_service::export::export_records(
        records,
        Path::new(&options.folder),
        &options.service(),
        Some(cancel),
        Some(&report),
        &exporter,
        false,
        workers,
    )
    .map_err(|e| e.to_string())?;
    let list = |key: &str| result[key].as_array().cloned().unwrap_or_default();
    Ok(Outcome {
        outputs: list("outputs"),
        failures: list("failures").iter().filter_map(|f| f.as_str().map(str::to_owned)).collect(),
        skipped: result["skipped"].as_u64().unwrap_or(0) as usize,
    })
}

/// Port of `photo_service.export_plans`: the new file of each source in `folder` (names already in the folder
/// or planned before are skipped: `{stem}_grainy.jpg`, then `{stem}_grainy_2.jpg`…).
pub fn plans(items: &[(PathBuf, Settings)], folder: &Path, format: &str, naming: &str) -> Result<Vec<PathBuf>, String> {
    let records: Vec<Value> = items
        .iter()
        .map(|(path, settings)| json!({"path": path.to_string_lossy(), "settings": Value::Object(settings.0.clone())}))
        .collect();
    let plans = grainy_service::export::export_plans(&records, folder, format, naming).map_err(|e| e.to_string())?;
    Ok(plans.into_iter().map(|(_, path)| PathBuf::from(path)).collect())
}

/// What the export job reports.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportEvent {
    /// `done` of `total` photos handled.
    Progress { done: usize, total: usize },
    /// All handled: the files written and a message per failure.
    Finished { written: Vec<PathBuf>, failures: Vec<String> },
}

/// Exports `items` (source path and settings, photos outside a catalogue) with `options`: the files written and a
/// message per failure. `progress(done, total)` after each photo; a cancel skips the photos not started.
pub fn run(
    items: &[(PathBuf, Settings)],
    options: &ExportOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> (Vec<PathBuf>, Vec<String>) {
    let records: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(i, (path, settings))| session_record(i as u64 + 1, path, settings, &Value::Null))
        .collect();
    match run_records(&records, options, progress, cancel) {
        Ok(outcome) => (outcome.written(), outcome.failures),
        Err(message) => (Vec::new(), vec![message]),
    }
}

/// [`run`] on a thread of its own: events arrive on the receiver and `waker` is called after each.
pub fn spawn(items: Vec<(PathBuf, Settings)>, options: ExportOptions, waker: Option<Waker>) -> Receiver<ExportEvent> {
    let (tx, rx) = channel();
    let work = move || {
        let send = |event: ExportEvent| {
            let _ = tx.send(event);
            if let Some(waker) = &waker {
                waker();
            }
        };
        let progress = |done: usize, total: usize| send(ExportEvent::Progress { done, total });
        let (written, failures) = run(&items, &options, &progress, &AtomicBool::new(false));
        send(ExportEvent::Finished { written, failures });
    };
    if let Err(error) = std::thread::Builder::new().name("grainy-export".into()).spawn(work) {
        log::error!("cannot start the export thread: {error}");
    }
    rx
}

#[test]
fn disabled_export_gpu_opens_no_adapter() {
    assert!(AppExporter::with_gpu(false).gpu.lock().unwrap().is_none());
}
