//! The analysis behind the editor's automatic buttons: auto tone and the automatic value of one Basic slider
//! (luma/engine.py), auto colour (luma/auto_color.py), automatic white balance (luma/workflow_data.py), the film
//! negative measurement (luma/negative.py), automatic alignment (luma/upright.py `estimate`), the scan-border crop
//! (luma/scan_border.py `crop_for`) and the AI batch's auto level / auto crop (luma/photo_edits.py `automatic`).
//!
//! Every function works on a develop [`Source`] as loaded (any size: each reduces it as the oracle does) and
//! returns settings, which are stored in the catalogue as edits, so the values are the oracle's. The frame they
//! share is the oracle's `geometry(lens_shading(linear_frame(resize_float(source, N), s), s), s)`: Pillow's
//! bilinear reduction (a camera source keeps its calibration), the camera colour stage and the film negative
//! conversion (`grainy_film::negative::apply`), the lens shading and the geometry of the settings.
//!
//! Reports are the oracle's dictionaries as JSON objects (the UI words its messages from them).

use std::borrow::Cow;
use std::sync::Mutex;

use grainy_core::{normalized, Image, Settings};
use grainy_geometry::upright::Estimate;
use grainy_geometry::UseCrop;
use ndarray::{s, Array2, ArrayView3, Axis};
use serde_json::{json, Map, Value};

use crate::{CameraSource, Externals, Options, Source};

/// Errors of the automatic analyses.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A stage of the frame preparation or of a develop failed (an alignment that finds no evidence is
    /// `Develop(Geometry(Alignment(message)))`, with the oracle's message).
    #[error(transparent)]
    Develop(#[from] crate::Error),
    /// Pillow's reduction refused the frame.
    #[error(transparent)]
    Resize(#[from] grainy_codecs::Error),
    /// The oracle's `ValueError` with its message (white balance without usable colour, an unknown alignment
    /// mode…).
    #[error("{0}")]
    Value(String),
}

impl Error {
    /// Whether this is the oracle's `upright.AlignmentError` (automatic alignment found no usable lines).
    pub fn is_alignment(&self) -> bool {
        matches!(self, Error::Develop(crate::Error::Geometry(grainy_geometry::Error::Alignment(_))))
    }
}

macro_rules! via_develop {
    ($($t:ty),*) => {$(
        impl From<$t> for Error {
            fn from(error: $t) -> Self {
                Error::Develop(error.into())
            }
        }
    )*};
}
via_develop!(
    grainy_geometry::Error,
    grainy_raw::Error,
    grainy_tone::Error,
    grainy_colorgrade::Error,
    grainy_film::Error
);

impl From<grainy_imgproc::Error> for Error {
    fn from(error: grainy_imgproc::Error) -> Self {
        Error::Develop(crate::Error::Geometry(error.into()))
    }
}

/// Result of this module.
pub type Result<T> = std::result::Result<T, Error>;

/// A report: the oracle's dictionary.
pub type Report = Map<String, Value>;

/// `np.array([.2126, .7152, .0722], np.float32)`.
const WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

// ---- numpy helpers -----------------------------------------------------------------------------------------

/// `pixel @ weights` (numpy's float32 matmul with a 3-vector: sequential, unfused).
#[inline]
fn dot3(p: &[f32]) -> f32 {
    p[0] * WEIGHTS[0] + p[1] * WEIGHTS[1] + p[2] * WEIGHTS[2]
}

/// `image @ weights` of an H×W×3 frame, in row-major order.
fn luminance(image: &Image) -> Vec<f32> {
    image.lanes(Axis(2)).into_iter().map(|p| p[0] * WEIGHTS[0] + p[1] * WEIGHTS[1] + p[2] * WEIGHTS[2]).collect()
}

use grainy_core::numpy::percentiles;

use grainy_core::numpy::percentile;

/// `np.clip` of a float64 (NaN stays NaN).
fn clip(v: f64, low: f64, high: f64) -> f64 {
    if v.is_nan() {
        v
    } else {
        v.max(low).min(high)
    }
}

/// Python's `round(x, digits)`: the exact binary value rounded half to even in decimal.
fn round_digits(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// Python's `max(a, b)` of floats (the first unless the second is larger).
fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

// ---- the shared frame --------------------------------------------------------------------------------------

/// A source as `resize_float` returns it: the caller's own (not reduced) or a reduced copy.
enum Reduced<'a> {
    Same(Source<'a>),
    Linear(Image),
    Camera(Box<CameraSource>),
}

impl Reduced<'_> {
    fn source(&self) -> Source<'_> {
        match self {
            Reduced::Same(source) => *source,
            Reduced::Linear(image) => Source::Linear(image),
            Reduced::Camera(camera) => Source::Camera(camera),
        }
    }
}

/// Port of luma/engine.py `resize_float(source, longest)`: Pillow's bilinear reduction to `longest` pixels on the
/// long side (`grainy_codecs::resize_float`); a camera source keeps its calibration. A source that is small
/// enough is used as it is.
fn resize_float(source: Source<'_>, longest: usize) -> Result<Reduced<'_>> {
    let (h, w, _) = source.pixels().dim();
    if h.max(w) <= longest {
        return Ok(Reduced::Same(source));
    }
    let pixels = grainy_codecs::resize_float(source.pixels(), longest)?;
    Ok(match source {
        Source::Linear(_) => Reduced::Linear(pixels),
        Source::Camera(camera) => {
            Reduced::Camera(Box::new(CameraSource::new(pixels, camera.raw_info().clone()).map_err(crate::Error::from)?))
        }
    })
}

/// Port of luma/rawcolor.py `linear_frame`: the camera colour stage (a camera source), then the film negative
/// conversion of the settings (`grainy_film::negative::apply`, when it changes pixels).
fn linear_frame<'a>(source: Source<'a>, s: &Settings) -> Result<Cow<'a, Image>> {
    let developed = match source {
        Source::Linear(image) => Cow::Borrowed(image),
        Source::Camera(camera) => Cow::Owned(grainy_raw::rawcolor::develop_camera(camera, s)?),
    };
    Ok(if grainy_film::negative::enabled(s) {
        Cow::Owned(grainy_film::negative::apply(&developed, s))
    } else {
        developed
    })
}

/// `lens_shading(frame, s)` then `geometry(…, s, use_crop)` (engine.geometry with a copy: always owned).
fn shaded_geometry(frame: &Image, s: &Settings, use_crop: UseCrop, externals: &Externals<'_>) -> Result<Image> {
    let shaded = grainy_geometry::lens::lens_shading(frame, s, externals.lens)?;
    let moved = grainy_geometry::geometry(&shaded, s, use_crop, externals.lens)?;
    Ok(moved.into_owned())
}

/// `geometry(lens_shading(linear_frame(resize_float(source, longest), s), s), s)`: the analysis frame of auto
/// tone and auto colour.
fn analysis_frame(source: Source<'_>, s: &Settings, longest: usize, externals: &Externals<'_>) -> Result<Image> {
    let reduced = resize_float(source, longest)?;
    let linear = linear_frame(reduced.source(), s)?;
    shaded_geometry(&linear, s, UseCrop::Yes, externals)
}

/// The tone stage's film render (`film_look.render`) for the settings, with its first error kept.
fn with_film<T>(s: &Settings, run: impl FnOnce(Option<grainy_tone::FilmRender<'_>>) -> Result<T>) -> Result<T> {
    if !grainy_film::active(s) {
        return run(None);
    }
    let name = s.str("film_look");
    let space = grainy_film::WorkingSpace::from_setting(s.str("working_space"));
    let failure: Mutex<Option<grainy_film::Error>> = Mutex::new(None);
    let render = |pixels: ArrayView3<'_, f32>| -> Image {
        match grainy_film::render(&pixels.to_owned(), name, space) {
            Ok(print) => print,
            Err(error) => {
                if let Ok(mut slot) = failure.lock() {
                    slot.get_or_insert(error);
                }
                Image::zeros(pixels.raw_dim())
            }
        }
    };
    let result = run(Some(&render))?;
    match failure.into_inner() {
        Ok(Some(error)) => Err(error.into()),
        _ => Ok(result),
    }
}

// ---- auto tone ---------------------------------------------------------------------------------------------

/// Port of luma/engine.py `auto_tone_settings(source, settings)`: maps the 0.1–99.9 percentile luminance interval
/// to 3–97 % brightness with exposure, Blacks, Whites and a tone curve (contrast, highlights and shadows reset);
/// geometry, white balance, colour and effects stay. Returns the normalized settings with the result and the
/// report (`status` `applied` with the measured values, or `flat` with the oracle's message and the settings
/// unchanged).
pub fn auto_tone_settings(
    source: Source<'_>,
    settings: &Settings,
    externals: &Externals<'_>,
) -> Result<(Settings, Report)> {
    let mut s = normalized(&settings.0);
    let mut image = analysis_frame(source, &s, 720, externals)?;
    let gain = grainy_core::white_balance(&s);
    grainy_core::parallel::for_each_pixel(&mut image, |p| {
        for c in 0..3 {
            p[c] *= gain[c];
        }
    });
    let linear = percentiles(&mut luminance(&image), &[0.1, 99.9]);
    let (low_linear, high_linear) = (linear[0], linear[1]);
    if high_linear - low_linear < 1e-6 {
        let mut report = Report::new();
        report.insert("status".into(), "flat".into());
        report.insert("message".into(), "밝기 차이가 거의 없어 자동톤을 적용하지 않았습니다.".into());
        return Ok((s, report));
    }
    let original = percentiles(&mut luminance(&grainy_color::to_srgb(&image)), &[0.1, 99.9]);
    let exposure = clip((0.9 / py_max(high_linear, 1e-8)).log2(), -5.0, 5.0);
    // Auto tone always produces the current process; measure through its highlight roll-off.
    s.set("tone_version", 3);
    let factor = 2f64.powf(exposure) as f32;
    let mut exposed = image;
    exposed.mapv_inplace(|v| v * factor);
    if !s.bool("hdr") {
        let mut with_exposure = s.clone();
        with_exposure.set("exposure", exposure);
        let white = grainy_tone::rolloff_white(&with_exposure);
        exposed = grainy_tone::highlight_rolloff(&exposed, white, grainy_tone::SHOULDER_KNEE);
    }
    let ends = percentiles(&mut luminance(&grainy_color::to_srgb(&exposed)), &[0.1, 99.9]);
    let (low, high) = (ends[0], ends[1]);
    let black_point = clip(low, 0.0, 0.8);
    let white_gain = (1.0 - black_point) / py_max(high - black_point, 1e-6);
    let whites = clip(100.0 * white_gain.log2(), -200.0, 200.0);
    s.set("exposure", exposure);
    s.set("blacks", -250.0 * black_point);
    s.set("whites", whites);
    s.set("contrast", 0.0);
    s.set("highlights", 0.0);
    s.set("shadows", 0.0);
    let gain = 2f64.powf(whites / 100.0);
    let mapped = |v: f64| clip((v - black_point) / (1.0 - black_point) * gain, 0.0, 1.0);
    let (mapped_low, mapped_high) = (mapped(low), mapped(high));
    let mut curve: Vec<(f64, f64)> = grainy_core::defaults()
        .value("curve")
        .as_array()
        .map(|points| {
            points.iter().map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0))).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if mapped_high - mapped_low > 1e-6 && (mapped_low > 1e-6 || mapped_high < 1.0 - 1e-6) {
        // A dict keyed by x: an equal key keeps its place and takes the later value; then sorted by x.
        let mut points: Vec<(f64, f64)> = Vec::new();
        for (x, y) in
            [(0.0, 0.0), (mapped_low, 0.0), ((mapped_low + mapped_high) / 2.0, 0.5), (mapped_high, 1.0), (1.0, 1.0)]
        {
            match points.iter_mut().find(|(key, _)| *key == x) {
                Some(point) => point.1 = y,
                None => points.push((x, y)),
            }
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        curve = points;
    }
    let margin = 0.03;
    let curve: Vec<Value> = curve.iter().map(|&(x, y)| json!([x, margin + (1.0 - 2.0 * margin) * y])).collect();
    s.set("curve", curve);
    let mut report = Report::new();
    report.insert("status".into(), "applied".into());
    report.insert("low_percentile".into(), json!(0.1));
    report.insert("high_percentile".into(), json!(99.9));
    report.insert("input_black".into(), json!(round_digits(original[0] * 255.0, 1)));
    report.insert("input_white".into(), json!(round_digits(original[1] * 255.0, 1)));
    report.insert("output_black".into(), json!(margin * 255.0));
    report.insert("output_white".into(), json!((1.0 - margin) * 255.0));
    report.insert("margin_percent".into(), json!(margin * 100.0));
    report.insert("exposure".into(), json!(exposure));
    report.insert("black_point".into(), json!(black_point));
    report.insert("white_gain".into(), json!(gain));
    Ok((s, report))
}

/// The keys [`auto_slider_value`] knows besides `exposure`, with their percentile and target
/// (`engine.AUTO_SLIDER_TARGETS`).
const SLIDER_TARGETS: [(&str, f64, f64); 2] = [("whites", 99.9, 0.99), ("blacks", 0.1, 0.01)];

/// Port of luma/engine.py `auto_slider_value(source, settings, key)` (Shift+double-click on a Basic slider): that
/// slider's automatic value with the others kept. `exposure`: auto tone's exposure rounded to 0.01; `whites` /
/// `blacks`: the whole value that brings the brightest (99.9th percentile of each pixel's highest channel) /
/// darkest (0.1th percentile of the lowest channel) tones just inside clipping, searched by bisection on a
/// 480-pixel develop. `None` for another key, a photo without tonal range or a target out of reach.
pub fn auto_slider_value(
    source: Source<'_>,
    settings: &Settings,
    key: &str,
    externals: &Externals<'_>,
) -> Result<Option<f64>> {
    let s = normalized(&settings.0);
    if key == "exposure" {
        let (adjusted, report) = auto_tone_settings(source, &s, externals)?;
        if report.get("status").and_then(Value::as_str) != Some("applied") {
            return Ok(None);
        }
        return Ok(Some(round_digits(adjusted.f64("exposure"), 2)));
    }
    let Some(&(_, quantile, target)) = SLIDER_TARGETS.iter().find(|(name, _, _)| *name == key) else {
        return Ok(None);
    };
    let small = resize_float(source, 480)?;
    let options = Options { output: crate::Output::Named("sRGB"), externals: *externals, ..Options::default() };
    let mut base = s.0.clone();
    base.insert("masks".into(), json!([]));
    base.insert("grain".into(), json!(0.0));
    base.insert("vignette".into(), json!(0.0));
    let develop = |values: &Map<String, Value>| crate::develop(small.source(), values, &options);
    let measure = |value: f64| -> Result<f64> {
        let mut values = base.clone();
        values.insert(key.into(), json!(value));
        let a = develop(&values)?;
        let mut channel: Vec<f32> = a
            .lanes(Axis(2))
            .into_iter()
            .map(|p| if key == "whites" { p[0].max(p[1]).max(p[2]) } else { p[0].min(p[1]).min(p[2]) })
            .collect();
        Ok(f64::from(percentile(&mut channel, quantile)))
    };
    let mut current = luminance(&develop(&base)?);
    let range = f64::from(percentile(&mut current, 99.9) - percentile(&mut current, 0.1));
    if range < 1e-3 {
        return Ok(None); // no tonal range
    }
    let (mut low, mut high) = (-200.0, 200.0);
    let reachable = if key == "whites" {
        // Brighter with higher values; the brightest tones below target at +200 cannot reach it.
        !(measure(high)? < target || measure(low)? > target)
    } else {
        !(measure(low)? > target || measure(high)? < target)
    };
    if !reachable {
        return Ok(None);
    }
    for _ in 0..14 {
        let middle = (low + high) / 2.0;
        if measure(middle)? < target {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok(Some(((low + high) / 2.0).round_ties_even() + 0.0))
}

// ---- auto colour -------------------------------------------------------------------------------------------

/// Port of luma/auto_color.py `auto_color_settings(source, settings, amount)`: the normalized settings with
/// automatic `rgb_curves` (existing channel curves replaced) and the report of
/// `grainy_colorgrade::auto_color::channel_points` (`status` `applied`, `neutral`, `flat`, or `monochrome` for a
/// black-and-white photo). The analysis frame is the photo inside its crop after the tone stage and the colour
/// mixer, as it enters the channel curves. `amount` (0-1) scales every shift.
pub fn auto_color_settings(
    source: Source<'_>,
    settings: &Settings,
    amount: f64,
    externals: &Externals<'_>,
) -> Result<(Settings, Report)> {
    let mut s = normalized(&settings.0);
    if s.bool("monochrome") {
        let mut report = Report::new();
        report.insert("status".into(), "monochrome".into());
        return Ok((s, report));
    }
    let image = analysis_frame(source, &s, 720, externals)?;
    let toned = with_film(&s, |film| Ok(grainy_tone::develop_tone_pixels(image.view(), &s, None, 0, film)?))?;
    let clipped = grainy_color::clip01(&toned);
    let params = grainy_colorgrade::ColorParams::from_settings(&s)?;
    let mixed = grainy_colorgrade::develop_color_mix(&clipped, &params);
    let (curves, report) = grainy_colorgrade::auto_color::channel_points(&mixed, amount);
    let mut out = Report::new();
    out.insert("status".into(), report.status.into());
    if let Some(amount) = report.amount {
        out.insert("amount".into(), json!(amount));
    }
    if let Some(casts) = &report.casts {
        out.insert("casts".into(), json!(casts));
    }
    if let Some(curves) = curves {
        s.set("rgb_curves", json!(curves));
    }
    Ok((s, out))
}

// ---- automatic white balance -------------------------------------------------------------------------------

/// Port of luma/workflow_data.py `automatic_white_balance(source, settings)` (the workflow's batch button): the
/// settings with `kelvin_enabled` off and the `temperature` and `tint` that neutralise the low-chroma surfaces of
/// `geometry(linear_frame(resize_float(source, 640), settings), settings)` (no lens shading, as the oracle). The
/// settings are used as given (the oracle does not normalize them). An error with the oracle's message when the
/// photo has too few usable colours.
pub fn automatic_white_balance(source: Source<'_>, settings: &Settings, externals: &Externals<'_>) -> Result<Settings> {
    let reduced = resize_float(source, 640)?;
    let linear = linear_frame(reduced.source(), settings)?;
    let frame = grainy_geometry::geometry(&linear, settings, UseCrop::Yes, externals.lens)?;
    let (temperature, tint) = grainy_core::analysis::white_balance_statistics(&frame).map_err(Error::Value)?;
    let mut values = settings.0.clone();
    values.insert("kelvin_enabled".into(), Value::Bool(false));
    values.insert("temperature".into(), json!(temperature));
    values.insert("tint".into(), json!(tint));
    Ok(Settings(values))
}

// ---- film negatives ----------------------------------------------------------------------------------------

/// Port of luma/negative.py `measured(source, settings)`: the part of `source` (the frame as loaded, any size) a
/// measurement is made from: the scan values (a camera source stays in the camera's colours) through lens
/// shading and the whole geometry of the normalized settings (crop, turns, lens and perspective corrections,
/// straighten, upright), without the border (`negative.inside`).
pub fn negative_measured(source: Source<'_>, settings: &Settings, externals: &Externals<'_>) -> Result<Image> {
    let mut plain = normalized(&settings.0);
    plain.set("negative", true);
    // develop_camera leaves a camera source in the camera's colours while the conversion is on.
    let frame = shaded_geometry(source.pixels(), &plain, UseCrop::Yes, externals)?;
    Ok(grainy_film::negative::inside(&frame).to_owned())
}

/// `frame.reshape(-1, 3)` of an H×W×3 view.
fn rows(view: ArrayView3<'_, f32>) -> Array2<f32> {
    let (h, w, _) = view.dim();
    let mut out = Array2::zeros((h * w, 3));
    for (mut row, pixel) in out.outer_iter_mut().zip(view.lanes(Axis(2))) {
        for c in 0..3 {
            row[c] = pixel[c];
        }
    }
    out
}

/// Port of luma/negative.py `sample(source, settings)`: [`negative_measured`] as at most `SAMPLE` rows of pixels
/// spread evenly over the picture, what one photo adds to a roll measured together.
pub fn negative_sample(source: Source<'_>, settings: &Settings, externals: &Externals<'_>) -> Result<Array2<f32>> {
    let part = negative_measured(source, settings, externals)?;
    let (h, w, _) = part.dim();
    let samples = grainy_film::negative::SAMPLE as f64;
    let step = ((((h * w) as f64 / samples).sqrt()).ceil() as usize).max(1);
    Ok(rows(part.slice(s![..;step, ..;step, ..])))
}

/// Port of luma/negative.py `converted(source, settings)`: `settings` with the conversion switched on, the clear
/// film and the densest part measured from `source` inside the crop, and a first brightness.
pub fn negative_converted(source: Source<'_>, settings: &Settings, externals: &Externals<'_>) -> Result<Settings> {
    let pixels = rows(negative_measured(source, settings, externals)?.view());
    let (base, peak) = grainy_film::negative::ends(pixels.view());
    Ok(grainy_film::negative::placed(settings, pixels.view(), base, peak))
}

/// Port of luma/negative.py `converted_together(samples, settings)`: [`negative_converted`] for the photos of one
/// roll from their [`negative_sample`]s, measured as one (shared clear film and densest part, brightness per
/// photo).
pub fn negative_converted_together(samples: &[Array2<f32>], settings: &[Settings]) -> Vec<Settings> {
    grainy_film::negative::converted_together(samples, settings)
}

/// The develop panel's "measure the negative" button (luma/app.py `measure_negative`):
/// `converted(resize_float(source, 1200), settings)`. The UI copies `negative_base`, `negative_peak`,
/// `negative_density`, `exposure` and `blacks` from the result and switches on the Basic panel.
pub fn measure_negative(source: Source<'_>, settings: &Settings, externals: &Externals<'_>) -> Result<Settings> {
    let reduced = resize_float(source, 1200)?;
    negative_converted(reduced.source(), settings, externals)
}

// ---- automatic alignment -----------------------------------------------------------------------------------

/// Port of luma/upright.py `estimate(image, mode)`: the alignment of a linear frame (`level`, `vertical`, `full`
/// or `auto`) from the line segments of its display-encoded luminance (8-bit grey, OpenCV's LSD detector in
/// `grainy_imgproc`), then `grainy_geometry::upright::estimate_from_lines`. The result is stored as
/// `settings['upright']` ([`Estimate::to_json`]); the oracle's `ValueError` / `AlignmentError` messages otherwise.
pub fn upright_estimate(image: &Image, mode: &str) -> Result<Estimate> {
    let (h, w, _) = image.dim();
    if !matches!(mode, "level" | "vertical" | "full" | "auto") || h.min(w) < 40 {
        // The oracle checks the mode and the size before looking at the pixels.
        return Ok(grainy_geometry::upright::estimate_from_lines(w, h, None, mode)?);
    }
    // Display-encoded luminance exposes structural edges in shadow regions.
    let encoded = grainy_color::to_srgb(image);
    let mut grey = Array2::<u8>::zeros((h, w));
    for (g, p) in grey.iter_mut().zip(encoded.lanes(Axis(2))) {
        let rgb = [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0)];
        *g = (dot3(&rgb).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    let found = grainy_imgproc::line_segment_detector(&grey)?;
    let lines = Array2::from_shape_vec((found.len(), 4), found.iter().flatten().copied().collect())
        .map_err(|error| Error::Value(error.to_string()))?;
    let lines = (!found.is_empty()).then(|| lines.view());
    Ok(grainy_geometry::upright::estimate_from_lines(w, h, lines, mode)?)
}

/// The crop tool's Auto level and the Transform panel's Upright buttons (luma/app.py, luma/studio.py):
/// [`upright_estimate`] of `geometry(lens_shading(resize_float(source, 1400), s), s, use_crop=False)` with the
/// settings' own upright, straighten and crop left out. Auto level asks for `level` and turns the photo by
/// `-rotation` (rounded to 0.1°, within ±45°) on the straighten slider; Upright stores the estimate.
pub fn upright_for(source: Source<'_>, settings: &Settings, mode: &str, externals: &Externals<'_>) -> Result<Estimate> {
    let mut analysis = settings.clone();
    analysis.set("upright", Value::Null);
    analysis.set("straighten", 0);
    analysis.set("crop", Value::Null);
    let reduced = resize_float(source, 1400)?;
    let image = shaded_geometry(reduced.source().pixels(), &analysis, UseCrop::No, externals)?;
    upright_estimate(&image, mode)
}

// ---- scan borders ------------------------------------------------------------------------------------------

/// Port of luma/scan_border.py `crop_for(source, settings, longest)` (Workflow's scan-border crop; the oracle's
/// default `longest` is 1200): the unexposed film edge or scanner border found on the photo's uncropped geometry
/// frame, as a crop for `settings['crop']`, or `None`.
pub fn scan_border_crop(
    source: Source<'_>,
    settings: &Settings,
    longest: usize,
    externals: &Externals<'_>,
) -> Result<Option<[f64; 4]>> {
    let reduced = resize_float(source, longest)?;
    let frame = linear_frame(reduced.source(), settings)?;
    Ok(grainy_geometry::scan_border::crop_for_frame(&frame, settings, externals.lens)?)
}

// ---- the AI batch's automatic edits ------------------------------------------------------------------------

use grainy_core::panel_keys::switch_on;

use grainy_core::panel_keys::changed_keys;

/// Port of luma/photo_edits.py `automatic(record, settings, entry, directory)` after its loader: the crop tool's
/// auto level (`auto_straighten`: straighten 0 and `upright` = the `level` estimate of the uncropped frame) and
/// Workflow's scan-border crop (`auto_crop`), on `source`, the photo as `load_image(path, 1400, …)` gives it.
/// Returns `normalized(switch_on(result, changed_keys(settings, result)))`. This is the computation of the
/// `automatic` callback of `grainy_service::photo_edits::compose`; the caller loads the photo (or its offline
/// preview) and passes `(auto_straighten, auto_crop)` from the entry.
pub fn automatic(
    source: Source<'_>,
    settings: &Settings,
    auto_straighten: bool,
    auto_crop: bool,
    externals: &Externals<'_>,
) -> Result<Settings> {
    let mut result = settings.0.clone();
    if auto_straighten {
        let estimate = upright_for(source, settings, "level", externals)?;
        result.insert("straighten".into(), json!(0));
        result.insert("upright".into(), estimate.to_json());
    }
    if auto_crop {
        if let Some(crop) = scan_border_crop(source, &Settings(result.clone()), 1200, externals)? {
            result.insert("crop".into(), json!(crop));
        }
    }
    let changed = changed_keys(&settings.0, &result);
    switch_on(&mut result, &changed);
    Ok(normalized(&result))
}
