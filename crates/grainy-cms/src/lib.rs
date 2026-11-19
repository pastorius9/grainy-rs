//! Grainy's colour management with ICC profiles, on the same LittleCMS 2.19 as the Python Grainy (vendored,
//! built by `build.rs`), so conversions give the same pixels bit for bit.
//!
//! Ports `luma/colorio.py` (`profile`, `convert`, `display_rgb`), the `imagecodecs.cms_transform` /
//! `cms_profile` / `cms_info` calls of `luma/rasterio.py` (grey, CMYK and CIELAB sources, calibrated PNG and TIFF
//! profiles, `lab4`), `luma/native_color.py` (`Transform`, the
//! transform cache, `convert`, `convert8_planar`), the ICC part of `luma/engine.py output_rgb` (here the
//! complete function: [`output_rgb`]) and the pure parts of `luma/display_profiles.py` ([`display`]).
//! Metadata (EXIF/IPTC/XMP) is not here.

mod display_impl;
mod profiles;
#[allow(unsafe_code)]
mod sys;
mod transform;

use grainy_core::Image;

pub use profiles::{
    gray_profile, lab_profile, profile, rgb_profile, srgb_transfer_table, Profile, ProfileInfo, TransferFunction,
    BUILTIN,
};
pub use sys::Sample;
pub use transform::{
    cache, cache_info, clear_cache, cms_transform, cms_transform_image, convert, convert8_planar, convert_samples,
    native_convert, CacheInfo, ColorSpace, Transform, TransformCache, MAX_PROFILE_BYTES, MAX_PROFILE_SIZE,
    MAX_TRANSFORMS, PARALLEL_PIXELS,
};

/// Display profiles: the pure parts of luma/display_profiles.py.
pub mod display {
    pub use crate::display_impl::{
        checked, read_profile, resolve, CheckedProfile, DisplayProfile, SystemProfiles, MAX_PROFILE_BYTES,
    };
}

/// The encoded version of the linked LittleCMS (`cmsGetEncodedCMMversion`): 2190.
pub fn lcms_version() -> i32 {
    sys::version()
}

/// Errors, with the oracle's messages (`ValueError`/`CmsError`/`TypeError` texts).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CmsError {
    /// native_color: a profile LittleCMS cannot open.
    #[error("Invalid ICC profile")]
    InvalidProfile,
    /// native_color: profile shorter than 128 bytes or longer than 16 MB.
    #[error("Invalid ICC profile size")]
    InvalidProfileSize,
    /// native_color: a profile that is not RGB.
    #[error("RGB ICC profiles are required")]
    RgbRequired,
    /// native_color: `cmsCreateTransformTHR` failed.
    #[error("Cannot create ICC color transform")]
    TransformFailed,
    /// native_color: `cmsCreateContext` failed.
    #[error("Cannot create color context")]
    Context,
    /// native_color.convert: not a float32 RGB buffer.
    #[error("Float32 RGB buffer required")]
    Float32RgbRequired,
    /// native_color.convert8_planar: not an 8-bit RGB buffer.
    #[error("8-bit RGB buffer required")]
    Rgb8Required,
    /// native_color.convert8_planar: a plane larger than 4 GB.
    #[error("Image is too large for one plane stride")]
    PlaneStrideTooLarge,
    /// A sample slice whose length is not a multiple of 3 (numpy shape checks in the oracle).
    #[error("RGB samples required")]
    RgbSamplesRequired,
    /// imagecodecs `_cms_format`: samples that do not match the colour space's channel count.
    #[error("shape does not match colorspace")]
    SamplesShape,
    /// imagecodecs: `cmsOpenProfileFromMem` failed (`CmsError`).
    #[error("cmsOpenProfileFromMem returned NULL")]
    OpenFailed,
    /// imagecodecs: `cmsCreateTransform` failed (`CmsError`).
    #[error("cmsCreateTransform returned NULL")]
    CreateTransformFailed,
    /// imagecodecs: making a built-in profile failed.
    #[error("{0}")]
    ProfileCreation(&'static str),
    /// colorio.profile(path): not an RGB profile.
    #[error("RGB ICC 프로파일을 선택하세요.")]
    ChooseRgbProfile,
    /// display_profiles: not an RGB display/colour-space profile.
    #[error("화면용 RGB ICC 프로파일을 선택하세요.")]
    ChooseDisplayProfile,
    /// display_profiles: the test conversion gave non-finite values.
    #[error("ICC 색상 변환 결과가 올바르지 않습니다.")]
    NonFiniteResult,
    /// display_profiles: a profile file above 16 MB.
    #[error("16MB 이하의 모니터 ICC 프로파일을 선택하세요.")]
    DisplayProfileTooLarge,
    /// display_profiles.resolve: an unknown mode.
    #[error("화면 색상 모드를 확인하세요.")]
    UnknownDisplayMode,
    /// display_profiles.resolve: manual mode without a file.
    #[error("ICC 프로파일 파일을 선택하세요.")]
    ManualProfileMissing,
    /// An error of an injected system lookup (its message).
    #[error("{0}")]
    System(String),
    /// A file error (Rust's message; the oracle shows Python's `OSError` text).
    #[error("{0}")]
    Io(String),
}

/// Result with [`CmsError`].
pub type Result<T> = std::result::Result<T, CmsError>;

/// The destination of [`output_rgb`]: a profile name (`colorio.profile`: built-in name or file path) or ICC
/// bytes (the oracle's `bytes` destination).
#[derive(Clone, Copy, Debug)]
pub enum Destination<'a> {
    /// A built-in profile name or a path.
    Named(&'a str),
    /// ICC profile bytes.
    Icc(&'a Profile),
}

/// Port of luma/engine.py `output_rgb(a, working_space, destination)`, complete: the developed working pixels
/// converted to the output profile, then clipped to [0, 1] (NaN stays NaN, as `np.clip`).
///
/// With the sRGB working space and the `"sRGB"` destination the oracle only clips. Otherwise it converts with
/// LittleCMS from `'Luma Wide'` (ProPhoto working space) or `'sRGB'` to the destination profile; so does this,
/// on the same LittleCMS, so the pixels are the oracle's exactly (`grainy_color::output_rgb` is the pure-maths
/// approximation of the ProPhoto → sRGB case and errors for other destinations).
pub fn output_rgb(a: &Image, working_space: &str, destination: Destination<'_>) -> Result<Image> {
    let plain_srgb = matches!(destination, Destination::Named("sRGB"));
    if working_space != "ProPhoto" && plain_srgb {
        return Ok(grainy_color::clip01(a));
    }
    let source = profile(if working_space == "ProPhoto" { "Luma Wide" } else { "sRGB" })?;
    let target = match destination {
        Destination::Named(name) => profile(name)?,
        Destination::Icc(bytes) => bytes.clone(),
    };
    let mut converted = convert(a, &source, &target)?;
    grainy_color::clip01_in_place(&mut converted);
    Ok(converted)
}

/// Port of luma/colorio.py `display_rgb`: no destination returns the pixels unchanged; otherwise sRGB pixels
/// are converted to the display profile and clipped to [0, 1].
pub fn display_rgb(rgb: &Image, destination: Option<&Profile>) -> Result<Image> {
    let Some(destination) = destination else {
        return Ok(rgb.clone());
    };
    let srgb = profile("sRGB")?;
    let mut converted = convert(rgb, &srgb, destination)?;
    grainy_color::clip01_in_place(&mut converted);
    Ok(converted)
}
