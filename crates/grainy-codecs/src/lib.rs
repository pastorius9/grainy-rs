//! Ports of `luma/engine.py`, `rasterio.py` and `colorio.py`: files to linear pixels and developed pixels to files.

mod jpeg;
mod metadata;
mod pixels;
mod raster;
mod writer;

pub use metadata::{append_tiff_exif, export_exif, iptc_bytes, jpeg_iptc, read_metadata, xmp_metadata};
pub use pixels::{
    dither_tile, export_longest, jxl_container, jxl_distance, neutral_gray, quantize16, quantize8, quantize8_dithered,
    resize_export, resize_float, SizeLimit,
};
pub use raster::{
    decode_bitmap, load_image, load_image_with_decoder, thumbnail, DecodedBitmap, JpegDecoder, LoadOptions, LoadedImage,
};
pub use writer::{export_image, ExportFormat, ExportOptions};

use grainy_core::Image;
use serde_json::{Map, Value};

/// Dictionary returned by `luma/engine.py read_metadata` and `load_image`.
pub type Metadata = Map<String, Value>;

/// Fallible operations replacing Python exceptions from `engine.py`, `rasterio.py` and `colorio.py`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem failure in `load_image` or `export_image`.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Invalid image or metadata in the corresponding oracle operation.
    #[error("invalid codec input: {0}")]
    Invalid(String),
    /// Codec or colour-engine functionality requiring an external implementation.
    #[error("unsupported codec operation: {0}")]
    Unsupported(String),
    /// A decoder/encoder rejected the file or pixels.
    #[error("codec failed: {0}")]
    Codec(String),
}

/// Result of a port of `luma/engine.py`, `rasterio.py` or `colorio.py`.
pub type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
fn codec(error: impl std::fmt::Display) -> Error {
    Error::Codec(error.to_string())
}

fn validate(a: &Image) -> Result<()> {
    let (h, w, c) = a.dim();
    if h == 0 || w == 0 || !matches!(c, 1..=4) {
        return Err(invalid("empty image or unsupported channel count"));
    }
    if a.iter().any(|x| !x.is_finite()) {
        return Err(invalid("non-finite pixels"));
    }
    Ok(())
}

/// Sample colour model passed by `luma/rasterio.py _linear` to the injected CMS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModel {
    /// RGB samples, normally in 0..1.
    Rgb,
    /// One gray channel, normally in 0..1.
    Gray,
    /// CMYK float samples in percentages, as LittleCMS expects.
    Cmyk,
    /// CIELAB values, L in 0..100 and signed a/b.
    Lab,
}

/// Profile requested by `rasterio._png_profile` / `_tiff_profile` or `colorio.profile`.
#[derive(Clone, Debug, PartialEq)]
pub enum Profile {
    /// Named oracle profile (sRGB, Linear sRGB, Linear ProPhoto, Luma Wide, etc.).
    Named(String),
    /// Embedded ICC bytes passed through unchanged.
    Embedded(Vec<u8>),
    /// Untagged PNG/TIFF colour information used to synthesize an ICC profile.
    Calibrated {
        /// RGB or gray profile.
        model: ColorModel,
        /// xy chromaticity of the white point.
        white: [f64; 2],
        /// xy pairs of the RGB primaries, absent for gray.
        primaries: Option<[f64; 6]>,
        /// Power gamma; absent means sRGB or the supplied transfer table.
        gamma: Option<f64>,
        /// Optional TIFF transfer curves, normalized to 0..1.
        transfer: Vec<f32>,
    },
}

/// Injection boundary for `luma/colorio.py convert/profile` and `rasterio.py _linear`.
/// Implementations must reject profile/channel mismatches and return finite H×W×3 float32 pixels.
pub trait ColorManager {
    /// Port boundary for `colorio.profile` and raster colour-tag profile synthesis.
    fn profile_bytes(&self, profile: &Profile) -> Result<Vec<u8>>;
    /// Port boundary for `colorio.convert` / `imagecodecs.cms_transform`, relative colorimetric intent 1.
    fn convert(&self, pixels: &Image, model: ColorModel, source: &[u8], destination: &[u8]) -> Result<Image>;
    /// Description and model from `imagecodecs.cms_info`, used by `rasterio._linear`.
    fn inspect(&self, profile: &[u8]) -> Result<(ColorModel, String)>;
}
