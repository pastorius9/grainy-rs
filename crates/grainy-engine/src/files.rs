//! From a photo file to an exported file: the whole of `luma/engine.py export_image`, with the colour management
//! of `luma/colorio.py` plugged into `grainy-codecs`.
//!
//! - [`Cms`]: `grainy_codecs::ColorManager` over `grainy-cms` (LittleCMS 2.19, as the oracle's imagecodecs).
//! - [`load_photo`]: port of `engine.load_image`: ordinary image files through `grainy-codecs`, RAW files through
//!   `grainy-raw` (LibRaw), natively (`rawcolor.decode`, a camera-space frame) or the legacy way (`postprocess`).
//! - [`export_photo`]: port of `engine.export_image` from the top: JPEG XL colour-space rule, same-file refusal,
//!   normalizing, loading with the working space, the HDR TIFF branch, `develop(output_space=color_space)`,
//!   then `grainy_codecs::export_image` (sizing, grey detection, dither, quantisation, metadata, writing).
//! - [`photo_info`]: the metadata of a photo (`engine.read_metadata` plus what `load_image` reports).
//!
//! Video files are refused, as are the colour models and profile kinds that `grainy-cms` does not expose yet
//! (see [`Cms`]).

mod full_preview;
pub use full_preview::load_full_preview;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grainy_cms::{CmsError, Profile as CmsProfile};
use grainy_codecs::{ColorManager, ColorModel, ExportFormat, ExportOptions, LoadOptions, Metadata, Profile, SizeLimit};
use grainy_core::{normalized, Image};
use serde_json::{Map, Value};

use crate::{develop, CameraSource, Options, Output, Source};

pub use grainy_core::formats::RAW_NAMES as RAW_EXTENSIONS;
/// Video extensions of `luma/engine.py VIDEO_EXTENSIONS`.
pub const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "mov", "avi", "mkv", "m4v"];

/// The oracle's text when a colour conversion of an embedded profile fails (`rasterio._linear`).
const ICC_FAILED: &str = "내장 ICC 프로파일을 작업 색공간으로 변환하지 못했습니다.";

/// Errors of loading a photo and exporting it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read, decoded or written (`grainy-codecs`); an invalid-input message is shown as it is.
    #[error("{}", codec_text(.0))]
    Codecs(#[from] grainy_codecs::Error),
    /// The develop failed.
    #[error(transparent)]
    Develop(#[from] crate::Error),
    /// A colour-management failure.
    #[error(transparent)]
    Cms(#[from] CmsError),
    /// One of the oracle's `ValueError` texts.
    #[error("{0}")]
    Refused(String),
    /// A kind of source this build does not read yet.
    #[error("{0}")]
    Unsupported(String),
    /// A RAW file the decoder cannot read: the oracle's wording of `app.import_reason`, or the decoder's own text.
    #[error("{0}")]
    Raw(String),
    /// A filesystem failure outside the codecs.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn codec_text(error: &grainy_codecs::Error) -> String {
    match error {
        grainy_codecs::Error::Invalid(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Result of this module.
pub type Result<T> = std::result::Result<T, Error>;

/// Port of the colour calls of `luma/colorio.py` (`profile`, `convert`) and of `imagecodecs.cms_info` as the
/// codecs crate makes them.
///
/// Reproduced: named profiles (`colorio.profile`: the eight built-in names, or an RGB ICC file by path, and
/// `Lab4` for `imagecodecs.cms_profile('lab4')`), embedded profiles (bytes as they are), calibrated profiles of
/// `rasterio._png_profile` / `_tiff_profile` (`imagecodecs.cms_profile('rgb' | 'gray', whitepoint=...,
/// primaries=..., gamma=... | transferfunction=...)`), RGB to RGB conversion with the relative colorimetric
/// intent (`colorio.convert`: `grainy_cms::convert`), grey, CMYK (percent) and CIELAB to RGB conversion
/// (`imagecodecs.cms_transform(..., colorspace=space, outcolorspace='rgb', outdtype='float32', intent=1)`:
/// `grainy_cms::cms_transform_image`), and `cms_info` (colour model and description). When loading, any
/// failure of the embedded profile (broken bytes, not openable, wrong colour model, transform not creatable)
/// becomes the oracle's single message `rasterio._linear` raises, so a broken profile fails the load instead
/// of being ignored.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cms {
    /// Errors of conversions and inspections are the loader's message (see above).
    loading: bool,
    /// `convert` returns its input: the pixels were already developed into the output colour space.
    already_converted: bool,
}

impl Cms {
    /// The colour manager for reading source files.
    pub fn for_loading() -> Self {
        Self { loading: true, already_converted: false }
    }

    /// The colour manager for the export step. The pixels given to `grainy_codecs::export_image` were developed
    /// with `output_space=color_space` (the oracle's order), so the codecs crate's own working-space to
    /// output-space conversion must not run a second time: `convert` hands the pixels back unchanged. Profile
    /// lookups are the same as for loading.
    pub fn for_export_of_developed_pixels() -> Self {
        Self { loading: false, already_converted: true }
    }

    fn fail(&self, error: CmsError) -> grainy_codecs::Error {
        if self.loading {
            grainy_codecs::Error::Invalid(ICC_FAILED.into())
        } else {
            grainy_codecs::Error::Invalid(error.to_string())
        }
    }
}

/// Port of `luma/rasterio.py _png_profile` / `_tiff_profile` from the colour tags the codecs crate read: the
/// `imagecodecs.cms_profile` call. `gamma` is already `1 / gAMA`; no gamma and no TIFF `TransferFunction` is
/// `transferfunction=to_linear(np.linspace(0, 1, 65536, dtype=np.float32))`. An RGB `TransferFunction` of
/// `3 * 2**BitsPerSample` values is three curves (`curves.reshape(3, -1)`); the codecs crate does not pass the
/// bit depth, so three curves are recognised by a length of three times a power of two (the only lengths a
/// TIFF of either kind can have: `2**bits` for one curve).
fn calibrated_profile(
    model: ColorModel,
    white: [f64; 2],
    primaries: Option<[f64; 6]>,
    gamma: Option<f64>,
    transfer: &[f32],
) -> grainy_cms::Result<CmsProfile> {
    use grainy_cms::TransferFunction;
    let srgb;
    let function = match gamma {
        Some(gamma) => TransferFunction::Gamma(gamma),
        None if transfer.is_empty() => {
            srgb = grainy_cms::srgb_transfer_table();
            TransferFunction::Table(&srgb)
        }
        None => {
            let third = transfer.len() / 3;
            if model == ColorModel::Rgb && transfer.len().is_multiple_of(3) && third.is_power_of_two() {
                TransferFunction::Tables([&transfer[..third], &transfer[third..2 * third], &transfer[2 * third..]])
            } else {
                TransferFunction::Table(transfer)
            }
        }
    };
    match model {
        ColorModel::Gray => grainy_cms::gray_profile(Some(white), function),
        _ => grainy_cms::rgb_profile(Some(white), primaries, function),
    }
}

/// The `cms_transform` colour space of a codecs colour model.
fn color_space(model: ColorModel) -> grainy_cms::ColorSpace {
    match model {
        ColorModel::Rgb => grainy_cms::ColorSpace::Rgb,
        ColorModel::Gray => grainy_cms::ColorSpace::Gray,
        ColorModel::Cmyk => grainy_cms::ColorSpace::Cmyk,
        ColorModel::Lab => grainy_cms::ColorSpace::Lab,
    }
}

impl ColorManager for Cms {
    fn profile_bytes(&self, profile: &Profile) -> grainy_codecs::Result<Vec<u8>> {
        match profile {
            // rasterio._linear: imagecodecs.cms_profile('lab4') for untagged CIELAB.
            Profile::Named(name) if name == "Lab4" => {
                grainy_cms::lab_profile(true, None).map(|p| p.bytes().to_vec()).map_err(|e| self.fail_named(e))
            }
            Profile::Named(name) => {
                grainy_cms::profile(name).map(|p| p.bytes().to_vec()).map_err(|e| self.fail_named(e))
            }
            Profile::Embedded(bytes) => Ok(bytes.clone()),
            Profile::Calibrated { model, white, primaries, gamma, transfer } => {
                calibrated_profile(*model, *white, *primaries, *gamma, transfer)
                    .map(|p| p.bytes().to_vec())
                    .map_err(|e| self.fail_named(e))
            }
        }
    }

    fn convert(
        &self,
        pixels: &Image,
        model: ColorModel,
        source: &[u8],
        destination: &[u8],
    ) -> grainy_codecs::Result<Image> {
        let (source, destination) = (CmsProfile::from_bytes(source), CmsProfile::from_bytes(destination));
        if model != ColorModel::Rgb {
            // rasterio._linear: imagecodecs.cms_transform(data, icc, destination, colorspace=space,
            // outcolorspace='rgb', outdtype='float32', intent=1) on grey (H×W), CMYK (percent) or CIELAB samples.
            return grainy_cms::cms_transform_image(
                pixels,
                &source,
                color_space(model),
                &destination,
                grainy_cms::ColorSpace::Rgb,
            )
            .map_err(|e| self.fail(e));
        }
        if self.already_converted {
            return Ok(pixels.clone());
        }
        grainy_cms::convert(pixels, &source, &destination).map_err(|e| self.fail(e))
    }

    fn inspect(&self, profile: &[u8]) -> grainy_codecs::Result<(ColorModel, String)> {
        let info = CmsProfile::from_bytes(profile).info().map_err(|e| self.fail(e))?;
        let model = match info.colorspace {
            Some("rgb") => ColorModel::Rgb,
            Some("gray") => ColorModel::Gray,
            Some("cmyk") => ColorModel::Cmyk,
            Some("lab") => ColorModel::Lab,
            _ => return Err(self.fail(CmsError::RgbRequired)),
        };
        Ok((model, info.description.unwrap_or_default()))
    }
}

impl Cms {
    /// `colorio.profile(name)` failing for a name or a file the user chose: its own text, never the loader's.
    fn fail_named(&self, error: CmsError) -> grainy_codecs::Error {
        grainy_codecs::Error::Invalid(error.to_string())
    }
}

/// The arguments of `export_image` after the settings: format, quality, size and metadata options.
#[derive(Clone, Debug)]
pub struct ExportRequest {
    /// The oracle's `format` string.
    pub format: ExportFormat,
    /// `quality` (JPEG and AVIF use the integer part; JPEG XL the distance of `jxl_distance`).
    pub quality: f64,
    /// `longest`: the long side in pixels, or a (width, height, fit) box.
    pub longest: Option<SizeLimit>,
    /// `color_space`: a profile name or an RGB ICC file.
    pub color_space: String,
    /// `keep_metadata`.
    pub keep_metadata: bool,
    /// `user_metadata`.
    pub user_metadata: Metadata,
    /// `keywords`, comma separated.
    pub keywords: String,
    /// `grayscale`.
    pub grayscale: bool,
    /// `dither`.
    pub dither: bool,
    /// The pure-Rust JPEG XL encoder only writes lossless files: the oracle's distance mode needs libjxl.
    /// Without this flag a JPEG XL export fails with a clear message instead of ignoring the quality.
    pub jxl_lossless: bool,
}

impl Default for ExportRequest {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            quality: 95.0,
            longest: None,
            color_space: "sRGB".into(),
            keep_metadata: false,
            user_metadata: Metadata::new(),
            keywords: String::new(),
            grayscale: false,
            dither: false,
            jxl_lossless: false,
        }
    }
}

/// The oracle's format strings (`'JPEG'`, `'PNG'`, `'PNG 16-bit'`, `'TIFF 8-bit'`, `'TIFF 16-bit'`,
/// `'TIFF HDR 32-bit'`, `'AVIF'`, `'JPEG XL'`), compared without regard to case.
pub fn parse_format(name: &str) -> Option<ExportFormat> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => ExportFormat::Jpeg,
        "png" => ExportFormat::Png,
        "png 16-bit" => ExportFormat::Png16,
        "tiff 8-bit" => ExportFormat::Tiff8,
        "tiff 16-bit" => ExportFormat::Tiff16,
        "tiff hdr 32-bit" => ExportFormat::TiffHdr32,
        "avif" => ExportFormat::Avif,
        "avif hdr 10-bit" => ExportFormat::AvifHdr,
        "jpeg xl" | "jxl" => ExportFormat::JpegXl,
        _ => return None,
    })
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The checks of `engine.load_image` that come before decoding: the file exists, it is no video.
fn check_source_kind(path: &Path) -> Result<()> {
    if let Err(error) = std::fs::metadata(path) {
        return Err(Error::Io(std::io::Error::new(error.kind(), format!("cannot open {}: {error}", path.display()))));
    }
    let ext = extension(path);
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return Err(Error::Refused("동영상 파일은 지원하지 않습니다.".into()));
    }
    Ok(())
}

/// `Path.resolve()` (not strict): the real path of a file that exists, otherwise its folder's real path plus the
/// file name.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (absolute.parent().and_then(|p| std::fs::canonicalize(p).ok()), absolute.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => absolute,
    }
}

/// The pixels of a loaded photo: a scene-linear frame in the working space, or a RAW file's camera frame.
#[derive(Clone, Debug)]
pub enum Photo {
    /// Scene-linear working-space pixels (ordinary image files, RAW files developed the old way).
    Linear(Image),
    /// Camera-space pixels with the camera's calibration (`rawcolor.CameraSource`).
    Camera(Box<CameraSource>),
}

impl Photo {
    /// The photo as a develop source.
    pub fn source(&self) -> Source<'_> {
        match self {
            Photo::Linear(image) => Source::Linear(image),
            Photo::Camera(camera) => Source::Camera(camera.as_ref()),
        }
    }

    /// The pixels (camera-space for a camera frame).
    pub fn pixels(&self) -> &Image {
        self.source().pixels()
    }
}

/// What `engine.load_image` returns: the pixels and the metadata dictionary.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The pixels.
    pub photo: Photo,
    /// The oracle's metadata dictionary (file facts, EXIF values, size, orientation, colour profile).
    pub metadata: Metadata,
}

/// Port of luma/engine.py `load_image(path, max_size, working_space=settings['working_space'],
/// raw_options=settings)`: the photo as a develop source. `max_size` reduces the longest side for previews;
/// exports load at full size. `settings` are normalized settings (the working space and the RAW options).
pub fn load_photo(path: impl AsRef<Path>, max_size: Option<usize>, settings: &grainy_core::Settings) -> Result<Loaded> {
    let _timing = grainy_core::timing::Span::new("load_photo");
    let path = path.as_ref();
    check_source_kind(path)?;
    if RAW_EXTENSIONS.contains(&extension(path).as_str()) {
        return load_raw(path, max_size, settings, None);
    }
    let options =
        LoadOptions { max_size, working_space: settings.str("working_space").to_owned(), ..LoadOptions::default() };
    let loaded = grainy_codecs::load_image(path, &options, &Cms::for_loading())?;
    Ok(Loaded { photo: Photo::Linear(loaded.pixels), metadata: loaded.metadata })
}

/// `app.import_reason` for a RAW error: the decoder's "unsupported file" becomes advice for the person (Nikon's
/// High Efficiency NEF has its own sentence); every other error keeps its own text.
fn raw_error(path: &Path, error: grainy_raw::Error) -> Error {
    match error {
        grainy_raw::Error::UnsupportedRaw => Error::Raw(
            if matches!(extension(path).as_str(), "nef" | "nrw") {
                "이 압축 방식은 지원하지 않습니다. 니콘의 고효율(HE) RAW라면 카메라에서 \"무손실 압축\"으로 찍거나 DNG로 변환해 주세요."
            } else {
                "지원하지 않는 RAW 형식입니다."
            }
            .into(),
        ),
        grainy_raw::Error::Io(error) => Error::Io(error),
        other => Error::Raw(other.to_string()),
    }
}

/// `preview_store.signature(path)` as `load_image` stores it: resolved path, size and modification time (ns).
fn fingerprint(path: &Path, size: u64, modified: std::time::SystemTime) -> Result<Value> {
    let raw_canon = std::fs::canonicalize(path)?.to_string_lossy().into_owned();
    let canonical = if let Some(unc) = raw_canon.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        raw_canon.trim_start_matches(r"\\?\").to_string()
    };
    let nanos = modified.duration_since(std::time::UNIX_EPOCH).map_err(|e| Error::Raw(e.to_string()))?.as_nanos();
    let nanos = u64::try_from(nanos).map_err(|e| Error::Raw(e.to_string()))?;
    Ok(serde_json::json!({"path": canonical, "size": size, "mtime_ns": nanos}))
}

/// The RAW branch of `engine.load_image`. The metadata dictionary starts with the file facts and the EXIF values
/// (`read_metadata`), then `rawcolor.decode` (or the legacy path) adds its keys, then the size, decoder and
/// orientation. Half-size development is used for previews only (the longest side is more than twice `max_size`).
fn load_raw(
    path: &Path,
    max_size: Option<usize>,
    settings: &grainy_core::Settings,
    secondary: Option<(usize, &mut Option<Photo>)>,
) -> Result<Loaded> {
    let stat = std::fs::metadata(path)?;
    let mut metadata = Metadata::new();
    metadata.insert("filename".into(), Value::String(path.file_name().unwrap_or_default().to_string_lossy().into()));
    metadata.insert("format".into(), Value::String(extension(path).to_uppercase()));
    metadata.insert("size_mb".into(), Value::from((stat.len() as f64 / 1048576. * 100.).round_ties_even() / 100.));
    metadata.insert("file_fingerprint".into(), fingerprint(path, stat.len(), stat.modified()?)?);
    metadata.insert("working_space".into(), Value::String(settings.str("working_space").to_owned()));
    let timing = grainy_core::timing::Span::new("load.metadata");
    if let Value::Object(exif) = grainy_raw::metadata::read_metadata(path).map_err(|e| raw_error(path, e))? {
        metadata.extend(exif);
    }
    drop(timing);
    let mut raw = grainy_raw::RawFile::open(path).map_err(|e| raw_error(path, e))?;
    // Fallback to LibRaw MakerNote lens name when standard EXIF tag 42036 is missing.
    let lens_missing = metadata.get("lens").and_then(Value::as_str).is_none_or(|s| s.trim().is_empty());
    if lens_missing && !raw.metadata().lens.trim().is_empty() {
        metadata.insert("lens".into(), Value::String(raw.metadata().lens.trim().to_owned()));
    }
    let sizes = raw.metadata().sizes;
    let flip = raw.metadata().flip;
    let num_colors = raw.metadata().num_colors;
    let is_rgb_sensor = raw.metadata().color_desc.starts_with("RGB");
    let (width, height) = (sizes[3], sizes[2]);
    let (full_width, full_height) = if (flip & 4) != 0 { (height, width) } else { (width, height) };
    let photo = if grainy_raw::rawcolor::enabled(settings) && num_colors == 3 && is_rgb_sensor {
        let mut dictionary = Value::Object(std::mem::take(&mut metadata));
        let source = grainy_raw::rawcolor::decode(&mut raw, &mut dictionary, max_size.map(|v| v as u32), Some(path))
            .map_err(|e| raw_error(path, e));
        if let Value::Object(map) = dictionary {
            metadata = map;
        }

        Photo::Camera(Box::new(source?))
    } else {
        let half = max_size.is_some_and(|m| width.max(height) as usize > m.saturating_mul(2));
        let mode = if settings.str("working_space") == "ProPhoto" {
            grainy_raw::DecodeMode::ProPhoto
        } else {
            grainy_raw::DecodeMode::Srgb
        };
        let decoded = raw.postprocess(mode, half).map_err(|e| raw_error(path, e))?;
        let timing = grainy_core::timing::Span::new("load.normalize16");
        let mut rgb = if decoded.dim().2 == 1 {
            // 1-channel monochrome sensor RAW (e.g. Leica M Monochrom / Pentax Monochrome):
            // replicate neutral luminance cleanly into 3-channel linear RGB.
            let (dh, dw, _) = decoded.dim();
            grainy_core::Image::from_shape_fn((dh, dw, 3), |(y, x, _)| decoded[[y, x, 0]] as f32 / 65535.)
        } else {
            decoded.mapv(|v| v as f32 / 65535.)
        };
        drop(timing);
        if !half && num_colors == 3 {
            rgb = grainy_raw::rawcolor::reduce_false_color(&rgb).map_err(|e| raw_error(path, e))?;
        }
        metadata.insert("raw_native".into(), Value::Bool(false));
        Photo::Linear(rgb)
    };
    metadata.insert("width".into(), Value::from(full_width));
    metadata.insert("height".into(), Value::from(full_height));
    metadata.insert("decoder".into(), Value::String("LibRaw".into()));
    let orientation = grainy_raw::exif_orientation(flip);
    metadata.insert("source_orientation".into(), Value::from(orientation));
    let photo = match max_size.filter(|m| *m > 0) {
        Some(longest) => {
            let visible = resize_photo(&photo, longest, path)?;
            if let Some((size, slot)) = secondary {
                // Both sizes use the identical half-size LibRaw decode. Move its pre-resize
                // pixels to the analysis job; a second resize of the visible image is different.
                if width.max(height) as usize > longest.max(size).saturating_mul(2) {
                    *slot = Some(photo);
                }
            }
            visible
        }
        None => photo,
    };
    Ok(Loaded { photo, metadata })
}

/// The pre-resize half-size RAW of `engine.load_image`, reusable for a second preview size.
pub struct AnalysisSeed {
    photo: Photo,
    size: usize,
    path: PathBuf,
}

impl AnalysisSeed {
    /// Complete the deferred resize of `engine.load_image` on the analysis thread.
    pub fn finish(self) -> Result<Photo> {
        resize_photo(&self.photo, self.size, &self.path)
    }
}

fn resize_photo(photo: &Photo, longest: usize, path: &Path) -> Result<Photo> {
    Ok(match photo {
        Photo::Linear(rgb) => Photo::Linear(grainy_codecs::resize_float(rgb, longest)?),
        Photo::Camera(camera) => {
            let pixels = grainy_codecs::resize_float(camera.pixels(), longest)?;
            Photo::Camera(Box::new(
                CameraSource::new(pixels, camera.raw_info().clone()).map_err(|e| raw_error(path, e))?,
            ))
        }
    })
}

/// Port of the two preview sizes of `engine.load_image`: hand off a compatible decoded RAW
/// without delaying the visible frame for the analysis resize. Other files use the usual loader.
pub fn load_preview(
    path: &Path,
    size: usize,
    analysis_size: usize,
    settings: &grainy_core::Settings,
) -> Result<(Loaded, Option<AnalysisSeed>)> {
    if !RAW_EXTENSIONS.contains(&extension(path).as_str()) {
        return Ok((load_photo(path, Some(size), settings)?, None));
    }
    let _span = grainy_core::timing::Span::new("load_photo");
    check_source_kind(path)?;
    let mut seed = None;
    let loaded = load_raw(path, Some(size), settings, Some((analysis_size, &mut seed)))?;
    Ok((loaded, seed.map(|photo| AnalysisSeed { photo, size: analysis_size, path: path.to_owned() })))
}

/// Port of luma/engine.py `export_image(source_path, output_path, settings, format, quality, longest,
/// color_space, keep_metadata, user_metadata, keywords, grayscale, dither)` for every source (RAW files included). Returns the
/// exported (width, height).
///
/// The source is never written and the output is created exclusively (an existing file is an error, as the
/// oracle's `open('x+b')`). Nothing is created when loading or developing fails, for example with
/// an unusable lens profile.
pub fn export_photo(
    source_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    settings: &Map<String, Value>,
    request: &ExportRequest,
) -> Result<(usize, usize)> {
    export_photo_impl(source_path.as_ref(), output_path.as_ref(), settings, request, |source, s, opts| {
        develop(source, s, opts)
    })
}

/// GPU-accelerated full-resolution export: develops SDR exports on the hardware GPU (with automatic
/// CPU fallback on any unsupported path or device error), while keeping HDR exports on the canonical
/// HDR pipeline. Takes a mutex-guarded GPU so batch worker threads decode RAW and encode output
/// files in full parallel while bounding peak GPU VRAM during the brief compute pass.
#[cfg(feature = "gpu")]
pub fn export_photo_with_gpu(
    source_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    settings: &Map<String, Value>,
    request: &ExportRequest,
    gpu: Option<&std::sync::Mutex<Option<grainy_gpu::Gpu>>>,
) -> Result<(usize, usize)> {
    export_photo_impl(source_path.as_ref(), output_path.as_ref(), settings, request, |source, s, opts| {
        let ws = crate::working_settings(s);
        let wants_gpu = !ws.bool("hdr") && ws.str("working_space") != "ProPhoto" && request.color_space == "sRGB";
        if wants_gpu {
            if let Some(mutex) = gpu {
                let mut guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(dev) = guard.as_ref() {
                    let mut cache = crate::gpu::GpuCache::default();
                    match crate::gpu::develop_gpu_cached(source, s, opts, dev, &mut cache, 0) {
                        Ok(img) => return Ok(img),
                        Err(crate::gpu::Error::Engine(e)) => return Err(e),
                        Err(crate::gpu::Error::Gpu(grainy_gpu::Error::Device(_))) => {
                            *guard = None;
                        }
                        Err(crate::gpu::Error::Gpu(_)) => {}
                    }
                }
            }
        }
        develop(source, s, opts)
    })
}

fn export_photo_impl<F>(
    source_path: &Path,
    output_path: &Path,
    settings: &Map<String, Value>,
    request: &ExportRequest,
    develop_sdr: F,
) -> Result<(usize, usize)>
where
    F: FnOnce(crate::Source<'_>, &Map<String, Value>, &Options<'_>) -> crate::Result<grainy_core::Image>,
{
    let _timing = grainy_core::timing::Span::new("export_photo");
    if request.format == ExportFormat::JpegXl && request.color_space != "sRGB" {
        return Err(Error::Refused("JPEG XL은 sRGB로만 저장합니다. 출력 색공간을 sRGB로 바꿔 주세요.".into()));
    }
    if resolve(source_path) == resolve(output_path) {
        return Err(Error::Refused("원본 파일에는 덮어쓸 수 없습니다.".into()));
    }
    if output_path.exists() {
        return Err(Error::Io(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "the output file exists")));
    }
    let settings = normalized(settings).0;
    let s = grainy_core::Settings(settings.clone());
    let working_space = s.str("working_space").to_owned();
    let photo = match full_preview::for_export(source_path, &s) {
        Some(photo) => {
            let _hit = grainy_core::timing::Span::new("export.reuse_full_raw");
            photo
        }
        None => Arc::new(load_photo(source_path, None, &s)?.photo),
    };
    let hdr_export = matches!(request.format, ExportFormat::TiffHdr32 | ExportFormat::AvifHdr);
    let (developed, cms) = if hdr_export {
        if !s.bool("hdr") {
            let message = if request.format == ExportFormat::AvifHdr {
                "HDR AVIF를 저장하려면 사진의 HDR 모드를 켜 주세요."
            } else {
                "HDR TIFF를 저장하려면 사진의 HDR 모드를 켜 주세요."
            };
            return Err(Error::Refused(message.into()));
        }
        let working = develop(photo.source(), &settings, &Options { output: Output::Working, ..Options::default() })?;
        (grainy_color::to_linear(&working), Cms::for_export_of_developed_pixels())
    } else {
        let options = Options { output: Output::Named(&request.color_space), ..Options::default() };
        (develop_sdr(photo.source(), &settings, &options)?, Cms::for_export_of_developed_pixels())
    };
    let options = ExportOptions {
        format: request.format,
        quality: request.quality,
        longest: request.longest,
        working_space,
        color_space: request.color_space.clone(),
        keep_metadata: request.keep_metadata,
        user_metadata: request.user_metadata.clone(),
        keywords: request.keywords.clone(),
        grayscale: request.grayscale,
        dither: request.dither,
        hdr_enabled: s.bool("hdr"),
        jxl_lossless: request.jxl_lossless,
    };
    Ok(grainy_codecs::export_image(&developed, source_path, output_path, &options, &cms)?)
}

/// What `grainy-cli info` shows: `engine.read_metadata` and the facts of `engine.load_image` (size, format,
/// colour profile, orientation). A source whose pixels cannot be decoded here still gives its file metadata,
/// with the reason under `"load_error"`.
pub fn photo_info(path: impl AsRef<Path>) -> Result<Metadata> {
    let path = path.as_ref();
    check_source_kind(path)?;
    let raw = RAW_EXTENSIONS.contains(&extension(path).as_str());
    let loaded = if raw {
        let settings = grainy_core::Settings(normalized(&Map::new()).0);
        load_photo(path, Some(64), &settings).map(|loaded| loaded.metadata)
    } else {
        let options = LoadOptions { max_size: Some(64), ..LoadOptions::default() };
        grainy_codecs::load_image(path, &options, &Cms::for_loading())
            .map(|loaded| loaded.metadata)
            .map_err(Error::from)
    };
    match loaded {
        Ok(metadata) => Ok(metadata),
        Err(error) => {
            let mut metadata = if raw {
                match grainy_raw::metadata::read_metadata(path).map_err(|e| raw_error(path, e))? {
                    Value::Object(map) => map,
                    _ => Metadata::new(),
                }
            } else {
                grainy_codecs::read_metadata(path)?
            };
            metadata.insert("load_error".into(), Value::String(error.to_string()));
            Ok(metadata)
        }
    }
}
