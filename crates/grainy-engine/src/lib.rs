//! Port of luma/engine.py `develop`: the stages of the develop pipeline in the oracle's order, on the CPU
//! reference path (the oracle with `GRAINY_GPU=0`). Preview and export use the same function.
//!
//! Order: RAW camera colour (grainy-raw) → film negative → retouch → lens shading → geometry → tone → colour → detail →
//! local adjustments → HDR limit or clip → output colour space. The stages live in their own crates; this
//! crate only decides what runs, in which order and with which arguments.

use std::sync::{Arc, Mutex};

use grainy_core::{effective, normalized, Image, Settings};
use ndarray::ArrayView3;
use serde_json::{Map, Value};

pub mod auto;
pub mod cache;
pub mod files;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod merge;
pub mod stages;

pub use cache::{settings_key, DevelopmentCache, COLOR_KEYS, DETAIL_KEYS, GEOMETRY_KEYS, RAW_KEYS, TONE_KEYS};
pub use grainy_raw::rawcolor::CameraSource;
pub use stages::{pending, Externals};

/// Errors of a develop.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The oracle's `ValueError` for a bad `original_size`.
    #[error("Original image size must contain positive width and height")]
    OriginalSize,
    /// The source is not an H×W×3 frame.
    #[error("image shape {0:?}: an HxWx3 frame is needed")]
    Shape(Vec<usize>),
    /// The RAW camera colour stage failed.
    #[error(transparent)]
    Raw(#[from] grainy_raw::Error),
    /// The stored lens profile cannot be used (the oracle's messages).
    #[error(transparent)]
    Lens(#[from] grainy_lens::Error),
    /// Geometry failed (the oracle's messages for a bad stored matrix, lens conditions…).
    #[error(transparent)]
    Geometry(#[from] grainy_geometry::Error),
    /// Retouching or a local adjustment failed.
    #[error(transparent)]
    Local(#[from] grainy_local::Error),
    /// The tone stage failed.
    #[error(transparent)]
    Tone(#[from] grainy_tone::Error),
    /// The film look failed.
    #[error(transparent)]
    Film(#[from] grainy_film::Error),
    /// The colour stage failed.
    #[error(transparent)]
    Color(#[from] grainy_colorgrade::Error),
    /// The detail stage failed.
    #[error(transparent)]
    Detail(#[from] grainy_detail::Error),
    /// The output colour conversion failed.
    #[error(transparent)]
    Cms(#[from] grainy_cms::CmsError),
    /// The settings need a piece the caller's [`Externals`] left out.
    #[error("not ported yet: {0}")]
    Pending(&'static str),
}

/// Result of this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// The oracle's `use_crop` argument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Crop {
    /// `True`: the crop is applied.
    #[default]
    Cropped,
    /// `False`: the whole frame.
    Uncropped,
    /// `'all'`: the crop tool's view (uncropped, on a canvas enlarged for a straightened photo).
    All,
}

/// The oracle's `output_space` argument.
#[derive(Clone, Copy, Debug)]
pub enum Output<'a> {
    /// `None`: the developed working pixels, for independent display, histogram and thumbnail transforms.
    Working,
    /// A profile name (`'sRGB'`, `'Display P3'`, …) or a profile file path.
    Named(&'a str),
    /// ICC profile bytes.
    Icc(&'a grainy_cms::Profile),
}

/// The arguments of `develop` besides the source and the settings.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// `use_crop`.
    pub use_crop: Crop,
    /// `output_space`.
    pub output: Output<'a>,
    /// `original_size`: the oriented, uncropped (width, height) of a reduced source. Spatial filter radii then
    /// follow original pixels.
    pub original_size: Option<(f64, f64)>,
    /// Implementations from outside the pipeline crates (lens model, dust repair, sky detection).
    pub externals: Externals<'a>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            use_crop: Crop::Cropped,
            output: Output::Named("sRGB"),
            original_size: None,
            externals: Externals::default(),
        }
    }
}

/// What a develop starts from: a scene-linear RGB frame in the working space (ordinary image files, and RAW files
/// developed the old way), or the camera-space frame of a RAW file with its calibration (`rawcolor.CameraSource`).
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// Scene-linear working-space pixels.
    Linear(&'a Image),
    /// Camera-space pixels and the camera's calibration.
    Camera(&'a CameraSource),
}

impl<'a> Source<'a> {
    /// The pixels of the source (camera-space for a camera source).
    pub fn pixels(&self) -> &'a Image {
        match *self {
            Source::Linear(image) => image,
            Source::Camera(camera) => camera.pixels(),
        }
    }
}

impl<'a> From<&'a Image> for Source<'a> {
    fn from(image: &'a Image) -> Self {
        Source::Linear(image)
    }
}

impl<'a> From<&'a CameraSource> for Source<'a> {
    fn from(camera: &'a CameraSource) -> Self {
        Source::Camera(camera)
    }
}

/// The settings a develop works with: `effective(normalized(settings))`.
pub fn working_settings(settings: &Map<String, Value>) -> Settings {
    effective(&normalized(settings))
}

/// `pixel_scale` of a develop: 1 for a full-size source, the reduction factor for a reduced one.
pub fn pixel_scale(source: &Image, original_size: Option<(f64, f64)>) -> Result<f64> {
    let Some((width, height)) = original_size else { return Ok(1.0) };
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(Error::OriginalSize);
    }
    let (h, w, _) = source.dim();
    Ok((h.max(w) as f64 / width.max(height)).min(1.0))
}

/// A frame handed from stage to stage: the caller's source, or a stage result (shared with the cache when there
/// is one).
pub(crate) enum Frame<'a> {
    Source(&'a Image),
    Stage(Arc<Image>),
}

impl std::ops::Deref for Frame<'_> {
    type Target = Image;
    fn deref(&self) -> &Image {
        match self {
            Frame::Source(image) => image,
            Frame::Stage(image) => image,
        }
    }
}

/// Port of luma/engine.py `develop(source, settings, use_crop, output_space, original_size=...)`, without a
/// cache and without the picker callbacks: the path of exports. `source` is `&Image` (scene-linear) or
/// `&CameraSource` (a RAW file's camera frame).
pub fn develop<'a>(
    source: impl Into<Source<'a>>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
) -> Result<Image> {
    run(source.into(), settings, options, None, false).map(Arc::unwrap_or_clone)
}

/// [`develop`] with the worker-owned cache of previews (`develop(..., cache=cache)`): stages whose settings did
/// not change are reused, with the same pixels. `token` identifies the source (the oracle compares source
/// objects): give every loaded source its own number.
pub fn develop_cached<'a>(
    source: impl Into<Source<'a>>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
    cache: &mut DevelopmentCache,
    token: u64,
) -> Result<Image> {
    cached(source.into(), settings, options, cache, token, false).map(Arc::unwrap_or_clone)
}

/// Working pixels for the oracle's SDR preview and histogram. A neutral suffix can share the
/// cached, already clipped tone result with the display until the next edit.
pub fn preview_working<'a>(
    source: impl Into<Source<'a>>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
    cache: &mut DevelopmentCache,
    token: u64,
) -> Result<Arc<Image>> {
    cached(source.into(), settings, &Options { output: Output::Working, ..*options }, cache, token, true)
}

fn cached(
    source: Source<'_>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
    cache: &mut DevelopmentCache,
    token: u64,
    preview: bool,
) -> Result<Arc<Image>> {
    let bytes = source.pixels().len() * std::mem::size_of::<f32>();
    cache.bind(token, bytes);
    // Avoid extra full-resolution copies when a useful prefix cannot fit.
    let usable = bytes * 2 <= cache.max_bytes();
    run(source, settings, options, usable.then_some(cache), preview)
}

fn stage(
    cache: &mut Option<&mut DevelopmentCache>,
    name: &str,
    key: &str,
    calculate: impl FnOnce() -> Result<Image>,
) -> Result<Arc<Image>> {
    let _timing = grainy_core::timing::Span::new(name);
    match cache {
        Some(cache) => cache.evaluate(name, key, calculate),
        None => Ok(Arc::new(calculate()?)),
    }
}

fn run(
    source: Source<'_>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
    mut cache: Option<&mut DevelopmentCache>,
    preview: bool,
) -> Result<Arc<Image>> {
    let _timing = grainy_core::timing::Span::new("develop");
    if source.pixels().dim().2 != 3 {
        return Err(Error::Shape(source.pixels().shape().to_vec()));
    }
    let s = working_settings(settings);
    // The lens model of the stored lens profile, unless the caller supplies one.
    let profile_lens = if options.externals.lens.is_none() && s.bool("lensfun_enabled") && s.bool("lensfun") {
        Some(grainy_lens::ProfileModel::from_settings(&s)?)
    } else {
        None
    };
    let externals = Externals {
        lens: options.externals.lens.or(profile_lens.as_ref().map(|model| model as &dyn grainy_geometry::LensModel)),
        ..options.externals
    };
    if let Some(missing) = pending(&s, &externals) {
        return Err(Error::Pending(missing));
    }
    let scale = pixel_scale(source.pixels(), options.original_size)?;
    let caching = cache.is_some();
    let key_of = |keys: &[&str]| if caching { settings_key(&s, keys) } else { String::new() };

    // RAW: camera colour (white balance, the camera profile) into the scene-linear working space.
    let (raw_key, camera_linear) = match source {
        Source::Camera(camera) => {
            let mut key = key_of(&RAW_KEYS);
            if s.i64("tone_version") >= 4 {
                key.push_str("|pv4");
            }
            let linear = stage(&mut cache, "raw", &key, || Ok(grainy_raw::rawcolor::develop_camera(camera, &s)?))?;
            (key, Frame::Stage(linear))
        }
        Source::Linear(image) => ("-".to_owned(), Frame::Source(image)),
    };
    // Film negative: turned into a positive before anything else, so every later stage is unchanged.
    let negative_key = s.bool("negative").then(|| format!("{raw_key}|{}", key_of(&grainy_film::negative::KEYS)));
    let linear = match &negative_key {
        Some(key) => {
            Frame::Stage(stage(&mut cache, "negative", key, || Ok(grainy_film::negative::apply(&camera_linear, &s)))?)
        }
        None => camera_linear,
    };
    let geometry_key = format!(
        "{raw_key}|{}|{:?}|{}",
        negative_key.as_deref().unwrap_or("-"),
        options.use_crop,
        key_of(&GEOMETRY_KEYS)
    );
    // Retouch, lens shading and geometry. When none applies the frame is the linear one itself (not cached).
    let timing = grainy_core::timing::Span::new("geometry");
    let moved = match cache.as_deref_mut().and_then(|cache| cache.lookup("geometry", &geometry_key)) {
        Some(cached) => Some(cached),
        None => stages::geometry(&linear, &s, options.use_crop, &externals)?.map(|image| match cache.as_deref_mut() {
            Some(cache) => cache.store("geometry", &geometry_key, image),
            None => Arc::new(image),
        }),
    };
    drop(timing);
    let frame: &Image = moved.as_deref().unwrap_or(&linear);

    let tone_key = format!("{geometry_key}|{}", key_of(&TONE_KEYS));
    let toned = stage(&mut cache, "tone", &tone_key, || tone(frame, &s))?;
    if preview && !s.bool("hdr") && !s.bool("masks") {
        static DEFAULTS: std::sync::LazyLock<Settings> = std::sync::LazyLock::new(|| working_settings(&Map::new()));
        if COLOR_KEYS.iter().chain(&DETAIL_KEYS).all(|k| s.value(k) == DEFAULTS.value(k)) {
            return Ok(toned);
        }
    }
    let color_key = format!("{tone_key}|{}", key_of(&COLOR_KEYS));
    let coloured = stage(&mut cache, "color", &color_key, || Ok(grainy_colorgrade::develop_color(&toned, &s)?))?;
    let detail_key = format!("{color_key}|{scale}|{}", key_of(&DETAIL_KEYS));
    let detailed =
        stage(&mut cache, "detail", &detail_key, || Ok(grainy_detail::develop_detail(&coloured, &s, scale)?))?;
    // Cached arrays are read-only: what follows works on its own copy (the oracle's clip allocates one too).
    let a = Arc::try_unwrap(detailed).unwrap_or_else(|shared| (*shared).clone());
    // Local adjustments: masks are drawn in source pixels and moved with the same geometry.
    let colour_source = if stages::needs_colour_source(&s) {
        let key = format!("{raw_key}|{}", negative_key.as_deref().unwrap_or("-"));
        Some(stage(&mut cache, "mask-source", &key, || Ok(grainy_color::to_srgb(&linear)))?)
    } else {
        None
    };
    let (height, width, _) = source.pixels().dim();
    let timing = grainy_core::timing::Span::new("local");
    let a = stages::local(a, (height, width), colour_source.as_deref(), &s, options.use_crop, scale, &externals)?;
    drop(timing);

    let _timing = grainy_core::timing::Span::new("output_color");
    let a = if s.bool("hdr") {
        let limited = grainy_tone::hdr::limit(&a, &s);
        if matches!(options.output, Output::Working) {
            return Ok(Arc::new(limited));
        }
        grainy_tone::hdr::sdr_rendition(&limited, &s)
    } else {
        let mut a = a;
        grainy_color::clip01_in_place(&mut a);
        // output_rgb would only clip again.
        if matches!(options.output, Output::Named("sRGB")) && s.str("working_space") != "ProPhoto" {
            return Ok(Arc::new(a));
        }
        a
    };
    let destination = match options.output {
        Output::Working => return Ok(Arc::new(a)),
        Output::Named(name) => grainy_cms::Destination::Named(name),
        Output::Icc(profile) => grainy_cms::Destination::Icc(profile),
    };
    Ok(Arc::new(grainy_cms::output_rgb(&a, s.str("working_space"), destination)?))
}

/// The tone stage with the film look of the settings.
fn tone(frame: &Image, s: &Settings) -> Result<Image> {
    if !grainy_film::active(s) {
        return Ok(grainy_tone::develop_tone(frame, s, None)?);
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
    let toned = grainy_tone::develop_tone(frame, s, Some(&render))?;
    match failure.into_inner() {
        Ok(Some(error)) => Err(error.into()),
        _ => Ok(toned),
    }
}
