//! The engine behind the tools: loading photos (`engine.load_image`, `preview_store.load_offline`), the preview
//! render of `photo_images.render_preview`, the library's cached thumbnails, the automatic edits of the auto
//! tools (`photo_service.auto_job`), the auto level / auto crop of apply_edits (`photo_edits.automatic`) and the
//! export of `photo_service.export_records`. Errors carry the Python exception class the oracle would raise, so
//! `photo_service.reason` and `english_error` word them the same.

use std::path::Path;

use grainy_core::{Image, Settings};
use grainy_engine::files::{load_photo, Photo};
use grainy_engine::{CameraSource, Externals, Options, Output};
use grainy_service::auto::{auto_values, AutoKind, AutoSource};
use grainy_service::targets::{access, Access};
use grainy_service::{Error, Failure};
use ndarray::{Array2, Array3};
use serde_json::{Map, Value};

use crate::service::Job;
use crate::util::{get, id_of, truthy};

/// A record's settings as engine settings.
pub(crate) fn settings_of(record: &Value) -> Map<String, Value> {
    get(record, "settings").as_object().cloned().unwrap_or_default()
}

fn failed(kind: &str, message: impl ToString) -> Failure {
    Failure::new(kind, message.to_string())
}

/// The Python class of a load or export failure.
pub(crate) fn files_failure(error: &grainy_engine::files::Error) -> Failure {
    use grainy_engine::files::Error as E;
    match error {
        E::Io(e) | E::Codecs(grainy_codecs::Error::Io(e)) => Failure::io(e),
        E::Develop(grainy_engine::Error::Pending(_)) => failed("NotImplementedError", error),
        _ => failed("ValueError", error),
    }
}

fn develop_failure(error: &grainy_engine::Error) -> Failure {
    match error {
        grainy_engine::Error::Pending(_) => failed("NotImplementedError", error),
        _ => failed("ValueError", error),
    }
}

fn auto_failure(error: &grainy_engine::auto::Error) -> Failure {
    if error.is_alignment() {
        return failed("AlignmentError", error);
    }
    match error {
        grainy_engine::auto::Error::Develop(e) => develop_failure(e),
        _ => failed("ValueError", error),
    }
}

/// `preview_store.load_offline(directory, id, settings)`: the photo's offline preview as a develop source.
pub(crate) fn offline(
    directory: &Path,
    ident: i64,
    settings: &Settings,
) -> Result<(Photo, Map<String, Value>), Failure> {
    let convert = |pixels: grainy_catalog::preview::Pixels, from: &str, to: &str| {
        let image = Array3::from_shape_vec(pixels.shape, pixels.values.clone())
            .map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        let source = grainy_cms::profile(from).map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        let target = grainy_cms::profile(to).map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        let converted =
            grainy_cms::convert(&image, &source, &target).map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        Ok(grainy_catalog::preview::Pixels {
            shape: pixels.shape,
            values: converted.into_raw_vec_and_offset().0,
            half: false,
        })
    };
    let (pixels, info) =
        grainy_catalog::preview::load_offline(directory, ident, settings, convert).map_err(|e| match e {
            grainy_catalog::Error::Io(e) => Failure::io(&e),
            other => failed("ValueError", other),
        })?;
    let image = Array3::from_shape_vec(pixels.shape, pixels.values).map_err(|e| failed("ValueError", e))?;
    let info = info.as_object().cloned().unwrap_or_default();
    let photo = if truthy(info.get("raw_native").unwrap_or(&Value::Null)) {
        let raw: grainy_raw::rawcolor::CameraInfo =
            serde_json::from_value(info.get("raw_info").cloned().unwrap_or_default())
                .map_err(|e| failed("ValueError", e))?;
        Photo::Camera(Box::new(CameraSource::new(image, raw).map_err(|e| failed("ValueError", e))?))
    } else {
        Photo::Linear(image)
    };
    Ok((photo, info))
}

/// The photo of a record as `load_image(path, max_size, settings['working_space'], raw_options=settings)` loads it,
/// or its offline preview when the original is not there.
pub(crate) fn source(
    record: &Value,
    settings: &Settings,
    max_size: usize,
    directory: &Path,
) -> Result<(Photo, Map<String, Value>), Failure> {
    let path = get(record, "path").as_str().unwrap_or("");
    if access(path) == Access::Present {
        let loaded = load_photo(path, Some(max_size), settings).map_err(|e| files_failure(&e))?;
        return Ok((loaded.photo, loaded.metadata));
    }
    offline(directory, id_of(record), settings)
}

/// `resize_float(source, longest)` of a develop source (a camera source keeps its calibration).
fn reduced(photo: Photo, longest: usize) -> Result<Photo, Failure> {
    let fail = |e: grainy_codecs::Error| failed("ValueError", e);
    Ok(match photo {
        Photo::Linear(image) => Photo::Linear(grainy_codecs::resize_float(&image, longest).map_err(fail)?),
        Photo::Camera(camera) => {
            let (h, w, _) = camera.pixels().dim();
            if h.max(w) <= longest {
                Photo::Camera(camera)
            } else {
                let pixels = grainy_codecs::resize_float(camera.pixels(), longest).map_err(fail)?;
                let info = camera.raw_info().clone();
                Photo::Camera(Box::new(CameraSource::new(pixels, info).map_err(|e| failed("ValueError", e))?))
            }
        }
    })
}

fn u8_pixels(rgb: &Image) -> Array3<u8> {
    rgb.mapv(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// Port of `photo_images.render_preview`: H×W×3 sRGB bytes of the photo with its current edits, long side at
/// most `longest` (as `reference_view.render_photo`).
pub fn render_preview(record: &Value, longest: usize, directory: &Path) -> Result<Array3<u8>, Error> {
    let map = settings_of(record);
    let settings = Settings(map.clone());
    let size = 4096.min(1024.max(2 * longest));
    let (photo, info) = source(record, &settings, size, directory).map_err(Error::Failed)?;
    let saved = get(record, "info");
    let (h, w, _) = photo.pixels().dim();
    let pick = |key: &str, fallback: usize| {
        let given = info.get(key).filter(|v| truthy(v)).or_else(|| Some(get(saved, key)).filter(|v| truthy(v)));
        given.and_then(Value::as_f64).unwrap_or(fallback as f64)
    };
    let (width, height) = (pick("width", w), pick("height", h));
    let options = Options {
        output: Output::Working,
        original_size: Some((width, height)),
        externals: Externals::default(),
        ..Options::default()
    };
    let working =
        grainy_engine::develop(photo.source(), &map, &options).map_err(|e| Error::Failed(develop_failure(&e)))?;
    let space = settings.str("working_space").to_owned();
    let mut rgb = grainy_cms::output_rgb(&working, &space, grainy_cms::Destination::Named("sRGB"))
        .map_err(|e| Error::Failed(failed("ValueError", e)))?;
    let (h, w, _) = rgb.dim();
    if h.max(w) > longest {
        rgb = grainy_codecs::resize_float(&rgb, longest).map_err(|e| Error::Failed(failed("ValueError", e)))?;
    }
    Ok(u8_pixels(&rgb))
}

/// Port of `photo_images.cached_thumbnail`: the library's own thumbnail as sRGB bytes when it shows the current
/// edits, else `None`.
pub fn cached_thumbnail(directory: &Path, ident: i64) -> Result<Option<Array3<u8>>, Error> {
    let inspection = grainy_catalog::preview::inspect(directory, ident)?;
    let Some(image) = inspection.image else { return Ok(None) };
    let state = &inspection.state;
    let status = get(state, "status").as_str().unwrap_or("");
    if truthy(get(state, "rebuild")) || !["", "오프라인", "접근 권한 필요"].contains(&status) {
        return Ok(None);
    }
    let decoded = grainy_jpeg_sys::decode(&image, None).map_err(|e| Error::Failed(failed("OSError", e)))?;
    let (h, w) = (decoded.height as usize, decoded.width as usize);
    let channels = decoded.channels as usize;
    let mut pixels = Array3::<u8>::zeros((h, w, 3));
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * channels;
            for c in 0..3 {
                pixels[[y, x, c]] = decoded.pixels[at + if channels == 1 { 0 } else { c.min(channels - 1) }];
            }
        }
    }
    if get(state, "color_space").as_str() == Some("ProPhoto RGB") {
        let image = pixels.mapv(|v| f32::from(v) / 255.0);
        let source = grainy_cms::profile("ProPhoto RGB").map_err(|e| Error::Failed(failed("ValueError", e)))?;
        let target = grainy_cms::profile("sRGB").map_err(|e| Error::Failed(failed("ValueError", e)))?;
        let converted =
            grainy_cms::convert(&image, &source, &target).map_err(|e| Error::Failed(failed("ValueError", e)))?;
        pixels = u8_pixels(&converted);
    }
    Ok(Some(pixels))
}

/// Port of `photo_edits.automatic` (the `automatic` callback of `grainy_service::photo_edits::compose`): the crop
/// tool's auto level and Workflow's scan-border crop on the photo loaded at 1400 px with `settings`.
pub fn automatic(
    record: &Value,
    settings: &Value,
    straighten: bool,
    crop: bool,
    directory: &Path,
) -> Result<Value, Error> {
    let map = settings.as_object().cloned().unwrap_or_default();
    let s = Settings(map);
    let (photo, _) = source(record, &s, 1400, directory).map_err(Error::Failed)?;
    let result = grainy_engine::auto::automatic(photo.source(), &s, straighten, crop, &Externals::default())
        .map_err(|e| Error::Failed(auto_failure(&e)))?;
    Ok(Value::Object(result.0))
}

/// What one photo's automatic computation gives.
pub(crate) enum AutoValue {
    /// New settings and the report's status (`applied`, `flat`, `neutral`, `monochrome`).
    Edit(Value, String),
    /// A negative's sample for a roll measured together.
    Sample(Array2<f32>),
}

struct AutoEngine<'a> {
    kind: AutoKind,
    amount: f64,
    directory: &'a Path,
}

impl AutoSource for AutoEngine<'_> {
    type Image = Photo;
    type Value = AutoValue;

    fn load(&mut self, record: &Value, path: &Path) -> Result<Photo, Failure> {
        let settings = Settings(settings_of(record));
        let loaded = load_photo(path, Some(720), &settings).map_err(|e| files_failure(&e))?;
        Ok(loaded.photo)
    }

    fn offline(&mut self, record: &Value) -> Result<Photo, Failure> {
        offline(self.directory, id_of(record), &Settings(settings_of(record))).map(|(p, _)| p)
    }

    fn compute(&mut self, image: Photo, settings: &Value) -> Result<AutoValue, Failure> {
        let photo = reduced(image, 720)?;
        let s = Settings(settings.as_object().cloned().unwrap_or_default());
        let externals = Externals::default();
        let edit = |result: (Settings, Map<String, Value>)| {
            let status = result.1.get("status").and_then(Value::as_str).unwrap_or("applied").to_owned();
            AutoValue::Edit(Value::Object(result.0 .0), status)
        };
        Ok(match self.kind {
            AutoKind::Tone => edit(
                grainy_engine::auto::auto_tone_settings(photo.source(), &s, &externals)
                    .map_err(|e| auto_failure(&e))?,
            ),
            AutoKind::Color => edit(
                grainy_engine::auto::auto_color_settings(photo.source(), &s, self.amount, &externals)
                    .map_err(|e| auto_failure(&e))?,
            ),
            AutoKind::Negative => {
                let converted = grainy_engine::auto::negative_converted(photo.source(), &s, &externals)
                    .map_err(|e| auto_failure(&e))?;
                AutoValue::Edit(Value::Object(converted.0), "applied".into())
            }
            AutoKind::Roll => AutoValue::Sample(
                grainy_engine::auto::negative_sample(photo.source(), &s, &externals).map_err(|e| auto_failure(&e))?,
            ),
        })
    }
}

/// What the work of an auto job hands to its finish on the GUI thread.
pub(crate) enum AutoOutcome {
    /// `values is None`: cancelled while computing.
    Cancelled,
    /// An exception in the work (`failed(error)`): the job's error text.
    Failed(String),
    /// `{'error', 'errors'}`: a roll without 2 readable photos.
    Error {
        /// The job's error.
        error: String,
    },
    /// `{'results': [(record, settings, status)], 'errors'}`.
    Results {
        /// Per photo: the frozen record, its new settings and the report's status.
        results: Vec<(Value, Value, String)>,
        /// `{id, error}` of photos that could not be read or computed.
        errors: Vec<Value>,
    },
}

/// The `work()` of `McpService.start_auto`: `photo_service.auto_values` with the engine, and for a roll
/// `negative.converted_together` of the samples.
pub(crate) fn auto_outcome(
    kind: AutoKind,
    amount: f64,
    load: &[Value],
    records: &[Value],
    directory: &Path,
    job: &Job,
) -> AutoOutcome {
    let mut engine = AutoEngine { kind, amount, directory };
    let mut progress = |n: usize| job.set_done(n as i64);
    let (values, errors) = match auto_values(load, &mut engine, Some(&job.cancel), Some(&mut progress), false) {
        Err(error) => {
            return AutoOutcome::Failed(format!("could not read or compute: {}", crate::util::english(&error)))
        }
        Ok(pair) => pair,
    };
    let Some(values) = values else { return AutoOutcome::Cancelled };
    let normal = |value: Value| grainy_service::normalized(&value).unwrap_or(value);
    if kind == AutoKind::Roll {
        let mut good: Vec<(&Value, Array2<f32>)> = Vec::new();
        for (record, value) in records.iter().zip(values) {
            if let Some(AutoValue::Sample(sample)) = value {
                good.push((record, sample));
            }
        }
        if good.len() < 2 {
            let _ = errors;
            return AutoOutcome::Error {
                error: "a roll needs at least 2 photos that Grainy can read; nothing was changed".into(),
            };
        }
        let samples: Vec<Array2<f32>> = good.iter().map(|(_, s)| s.clone()).collect();
        let settings: Vec<Settings> = good.iter().map(|(r, _)| Settings(settings_of(r))).collect();
        let converted = grainy_engine::auto::negative_converted_together(&samples, &settings);
        let results = good
            .iter()
            .zip(converted)
            .map(|((r, _), s)| ((*r).clone(), normal(Value::Object(s.0)), "applied".to_owned()))
            .collect();
        return AutoOutcome::Results { results, errors };
    }
    let results = records
        .iter()
        .zip(values)
        .filter_map(|(r, v)| match v {
            Some(AutoValue::Edit(settings, status)) => Some((r.clone(), normal(settings), status)),
            _ => None,
        })
        .collect();
    AutoOutcome::Results { results, errors }
}

/// `photo_service.planned_size`'s probe: `(width, height)` from `load_image(path, 32, ...)`.
pub(crate) fn probe(record: &Value) -> Result<(i64, i64), Failure> {
    let path = get(record, "path").as_str().unwrap_or("");
    let loaded = load_photo(path, Some(32), &Settings(settings_of(record))).map_err(|e| files_failure(&e))?;
    let side = |key: &str| loaded.metadata.get(key).and_then(Value::as_i64).unwrap_or(0);
    Ok((side("width"), side("height")))
}

/// `engine.export_image` and `quick_export.copy_original` for `photo_service.export_records`.
pub struct EngineExporter;

impl grainy_service::export::Exporter for EngineExporter {
    fn export_image(&self, job: &grainy_service::export::ExportJob<'_>) -> Result<Value, Failure> {
        use grainy_service::export::Longest;
        let options = job.options;
        let format = grainy_engine::files::parse_format(&options.format)
            .ok_or_else(|| failed("KeyError", format!("'{}'", options.format)))?;
        let longest = job.longest.map(|l| match l {
            Longest::Side(n) => grainy_codecs::SizeLimit::Longest((*n).max(0) as usize),
            Longest::Box(w, h, fit) => grainy_codecs::SizeLimit::Box {
                width: (*w).max(0) as usize,
                height: (*h).max(0) as usize,
                cover: fit == "cover",
            },
        });
        let request = grainy_engine::files::ExportRequest {
            format,
            quality: options.quality as f64,
            longest,
            color_space: options.color_space.clone(),
            keep_metadata: options.keep_metadata,
            user_metadata: get(job.record, "user_metadata").as_object().cloned().unwrap_or_default(),
            keywords: get(job.record, "keywords").as_str().unwrap_or("").to_owned(),
            grayscale: options.grayscale,
            dither: options.dither,
            jxl_lossless: format == grainy_codecs::ExportFormat::JpegXl,
        };
        let path = get(job.record, "path").as_str().unwrap_or("");
        let (w, h) = grainy_engine::files::export_photo(path, job.destination, &settings_of(job.record), &request)
            .map_err(|e| files_failure(&e))?;
        Ok(serde_json::json!([w, h]))
    }

    fn copy_original(&self, record: &Value, destination: &Path) -> Result<Value, Failure> {
        let (w, h) =
            grainy_library::quick_export::copy_original(record, destination).map_err(|e| failed("OSError", e))?;
        Ok(serde_json::json!([w, h]))
    }
}
