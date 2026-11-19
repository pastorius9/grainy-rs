//! ICC profiles: the built-in ones of `luma/colorio.py profile` (made with LittleCMS exactly as
//! `imagecodecs.cms_profile` makes them) and profiles from bytes or files.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::sys::{self, CIExyY, CIExyYTriple, CieXyz, ProfileHandle, ToneCurve};
use crate::{CmsError, Result};

/// The built-in profile names of `colorio.profile`.
pub const BUILTIN: [&str; 8] =
    ["sRGB", "Adobe RGB", "Display P3", "ProPhoto RGB", "Linear sRGB", "Linear ProPhoto", "Luma Wide", "Gray sRGB"];

/// ICC profile bytes (shared; cloning is cheap). The oracle passes profiles around as `bytes`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Profile {
    bytes: Arc<[u8]>,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Profile({} bytes)", self.bytes.len())
    }
}

/// What `imagecodecs.cms_info` reports and the port uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileInfo {
    /// `cmsGetColorSpace` as imagecodecs names it (`"rgb"`, `"gray"`, `"lab"`, ...); `None` for others.
    pub colorspace: Option<&'static str>,
    /// `cmsGetDeviceClass` as imagecodecs names it (`"display"`, `"colorspace"`, ...); `None` for others.
    pub deviceclass: Option<&'static str>,
    /// The description (`cmsInfoDescription`), decoded and stripped as imagecodecs does; `None` when empty.
    pub description: Option<String>,
}

impl Profile {
    /// Profile bytes as they are, unchecked (the oracle's `bytes` arguments); conversions check them.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self { bytes: Arc::from(bytes.into()) }
    }

    /// Port of luma/colorio.py `profile` for a built-in name; `None` for other names. Each built-in profile is
    /// made once per process (the oracle's `lru_cache`), so its creation time stays the same.
    pub fn builtin(name: &str) -> Option<Result<Self>> {
        static CACHE: [OnceLock<std::result::Result<Profile, CmsError>>; 8] = [const { OnceLock::new() }; 8];
        let index = BUILTIN.iter().position(|candidate| *candidate == name)?;
        Some(CACHE[index].get_or_init(|| make_builtin(name).map(Self::from_bytes)).clone())
    }

    /// Port of the file branch of luma/colorio.py `profile`: reads the file, checks that LittleCMS opens it
    /// (`imagecodecs.cms_profile_validate`) and that it is an RGB profile.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let data = std::fs::read(path.as_ref()).map_err(|error| CmsError::Io(error.to_string()))?;
        let profile = Self::from_bytes(data);
        if profile.info()?.colorspace != Some("rgb") {
            return Err(CmsError::ChooseRgbProfile);
        }
        Ok(profile)
    }

    /// The ICC bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn shared(&self) -> &Arc<[u8]> {
        &self.bytes
    }

    /// Port of `imagecodecs.cms_profile_validate` and the fields of `imagecodecs.cms_info` the oracle reads.
    /// Fails with "cmsOpenProfileFromMem returned NULL" when LittleCMS rejects the data.
    pub fn info(&self) -> Result<ProfileInfo> {
        let handle = ProfileHandle::open(&self.bytes).ok_or(CmsError::OpenFailed)?;
        let colorspace = match handle.color_space() {
            sys::sig::RGB_DATA => Some("rgb"),
            sys::sig::GRAY_DATA => Some("gray"),
            sys::sig::CMYK_DATA => Some("cmyk"),
            sys::sig::LAB_DATA => Some("lab"),
            sys::sig::XYZ_DATA => Some("xyz"),
            _ => None,
        };
        let deviceclass = match handle.device_class() {
            sys::sig::INPUT_CLASS => Some("input"),
            sys::sig::DISPLAY_CLASS => Some("display"),
            sys::sig::OUTPUT_CLASS => Some("output"),
            sys::sig::LINK_CLASS => Some("link"),
            sys::sig::ABSTRACT_CLASS => Some("abstract"),
            sys::sig::COLORSPACE_CLASS => Some("colorspace"),
            sys::sig::NAMED_COLOR_CLASS => Some("namedcolor"),
            _ => None,
        };
        let description = handle.description_bytes().and_then(|raw| decode_description(&raw));
        Ok(ProfileInfo { colorspace, deviceclass, description })
    }
}

/// `bytes.decode('ascii', errors='replace').rstrip('\x00').strip() or None`.
fn decode_description(raw: &[u8]) -> Option<String> {
    let text: String = raw.iter().map(|&b| if b < 128 { b as char } else { '\u{FFFD}' }).collect();
    // Python's str.strip() whitespace among these characters: \t \n \x0b \x0c \r \x1c-\x1f and space.
    let python_space = |c: char| matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\x1c'..='\x1f' | ' ');
    let text = text.trim_end_matches('\0').trim_matches(python_space);
    (!text.is_empty()).then(|| text.to_string())
}

/// Port of luma/colorio.py `profile(name)`: a built-in profile by name, otherwise an RGB ICC file at that path.
pub fn profile(name: &str) -> Result<Profile> {
    match Profile::builtin(name) {
        Some(result) => result,
        None => Profile::from_path(name),
    }
}

/// `np.linspace(0, 1, n, dtype=np.float32)`: computed in float64 (`arange * step`), last value exactly 1.
fn linspace_f32(n: usize) -> Vec<f32> {
    let step = 1.0 / (n - 1) as f64;
    let mut values: Vec<f32> = (0..n).map(|i| (i as f64 * step) as f32).collect();
    values[n - 1] = 1.0;
    values
}

/// `np.where(x<=.04045, x/12.92, ((x+.055)/1.055)**2.4)` on a float32 `x` (numpy 2 keeps float32: the Python
/// floats are weak scalars). Both branches are float32 maths; `**` is the C runtime's `powf`, as numpy's.
fn srgb_decode_table(n: usize) -> Vec<f32> {
    linspace_f32(n)
        .into_iter()
        .map(|x| if x <= 0.04045f32 { x / 12.92f32 } else { ((x + 0.055f32) / 1.055f32).powf(2.4f32) })
        .collect()
}

/// The curves of `imagecodecs.cms_profile('rgb', ...)`: three separately built curves.
enum Curves<'a> {
    Gamma(f64),
    TableF32(&'a [f32]),
}

fn three_curves(curves: Curves<'_>) -> Result<[ToneCurve; 3]> {
    let one = || match curves {
        Curves::Gamma(gamma) => ToneCurve::gamma(gamma).ok_or(CmsError::ProfileCreation("cmsBuildGamma returned NULL")),
        Curves::TableF32(values) => ToneCurve::table_f32(values)
            .ok_or(CmsError::ProfileCreation("cmsBuildTabulatedToneCurveFloat returned NULL")),
    };
    Ok([one()?, one()?, one()?])
}

/// `imagecodecs.cms_profile('rgb', whitepoint=(x, y), primaries=(6 values), gamma=... | transferfunction=...)`.
fn builtin_rgb(white: [f64; 2], primaries: [f64; 6], curves: Curves<'_>) -> Result<Vec<u8>> {
    let white = CIExyY { x: white[0], y: white[1], Y: 1.0 };
    let xy = |i: usize| CIExyY { x: primaries[2 * i], y: primaries[2 * i + 1], Y: 1.0 };
    let primaries = CIExyYTriple { red: xy(0), green: xy(1), blue: xy(2) };
    let curves = three_curves(curves)?;
    let handle = sys::create_rgb(&white, &primaries, Some([&curves[0], &curves[1], &curves[2]]))
        .ok_or(CmsError::ProfileCreation("cmsCreateRGBProfile returned NULL"))?;
    save(&handle)
}

fn save(handle: &ProfileHandle) -> Result<Vec<u8>> {
    handle.save().ok_or(CmsError::ProfileCreation("cmsSaveProfileToMem returned 0"))
}

/// imagecodecs' `_cms_adobe_rgb_compatible`.
fn adobe_rgb() -> Result<Vec<u8>> {
    use sys::sig::*;
    let failed = CmsError::ProfileCreation("failed to create CMS profile='adobergb'");
    let handle = ProfileHandle::placeholder().ok_or(failed.clone())?;
    handle.set_version(2.1);
    for (tag, text) in [
        (COPYRIGHT_TAG, c"Public Domain"),
        (DESCRIPTION_TAG, c"Adobe RGB (compatible)"),
        (DEVICE_MFG_DESC_TAG, c"Imagecodecs"),
        (DEVICE_MODEL_DESC_TAG, c"Adobe RGB (compatible)"),
    ] {
        if !handle.write_text(tag, text) {
            return Err(failed);
        }
    }
    handle.set_header(DISPLAY_CLASS, RGB_DATA, XYZ_DATA);
    handle.write_xyz(MEDIA_WHITE_POINT_TAG, &CieXyz { X: 0.95045, Y: 1.0, Z: 1.08905 });
    handle.write_xyz(MEDIA_BLACK_POINT_TAG, &CieXyz { X: 0.0, Y: 0.0, Z: 0.0 });
    handle.write_xyz(RED_COLORANT_TAG, &CieXyz { X: 0.609741, Y: 0.311111, Z: 0.019470 });
    handle.write_xyz(GREEN_COLORANT_TAG, &CieXyz { X: 0.205276, Y: 0.625671, Z: 0.060867 });
    handle.write_xyz(BLUE_COLORANT_TAG, &CieXyz { X: 0.149185, Y: 0.063217, Z: 0.744568 });
    let curve = ToneCurve::gamma(2.19921875).ok_or(failed.clone())?;
    handle.write_curve(RED_TRC_TAG, &curve);
    drop(curve);
    handle.link_tag(GREEN_TRC_TAG, RED_TRC_TAG);
    handle.link_tag(BLUE_TRC_TAG, RED_TRC_TAG);
    save(&handle)
}

/// imagecodecs' `_cms_linear_rgb`: Rec. 709 primaries, D65, one shared gamma-1 curve (linked TRC tags).
fn linear_rgb() -> Result<Vec<u8>> {
    let failed = CmsError::ProfileCreation("failed to create CMS profile='linearrgb'");
    let white = CIExyY { x: 0.3127, y: 0.3290, Y: 1.0 };
    let primaries = CIExyYTriple {
        red: CIExyY { x: 0.6400, y: 0.3300, Y: 1.0 },
        green: CIExyY { x: 0.3000, y: 0.6000, Y: 1.0 },
        blue: CIExyY { x: 0.1500, y: 0.0600, Y: 1.0 },
    };
    let curve = ToneCurve::gamma(1.0).ok_or(failed.clone())?;
    let handle = sys::create_rgb_thr(&white, &primaries, [&curve, &curve, &curve]).ok_or(failed.clone())?;
    drop(curve);
    if !handle.write_text(sys::sig::DESCRIPTION_TAG, c"Linear RGB") {
        return Err(failed);
    }
    save(&handle)
}

const PROPHOTO_WHITE: [f64; 2] = [0.3457, 0.3585];
const PROPHOTO_PRIMARIES: [f64; 6] = [0.7347, 0.2653, 0.1596, 0.8404, 0.0366, 0.0001];

/// The bytes of a built-in profile, made as `colorio.profile` makes them.
fn make_builtin(name: &str) -> Result<Vec<u8>> {
    match name {
        "Linear sRGB" => linear_rgb(),
        "Linear ProPhoto" => builtin_rgb(PROPHOTO_WHITE, PROPHOTO_PRIMARIES, Curves::Gamma(1.0)),
        "Luma Wide" => builtin_rgb(PROPHOTO_WHITE, PROPHOTO_PRIMARIES, Curves::TableF32(&srgb_decode_table(65536))),
        "sRGB" => {
            let handle = sys::create_srgb().ok_or(CmsError::ProfileCreation("cmsCreate_sRGBProfile returned NULL"))?;
            save(&handle)
        }
        "Adobe RGB" => adobe_rgb(),
        "Display P3" => builtin_rgb(
            [0.3127, 0.329],
            [0.68, 0.32, 0.265, 0.69, 0.15, 0.06],
            Curves::TableF32(&srgb_decode_table(4096)),
        ),
        "ProPhoto RGB" => builtin_rgb(PROPHOTO_WHITE, PROPHOTO_PRIMARIES, Curves::Gamma(1.8)),
        "Gray sRGB" => {
            let white = CIExyY { x: 0.3127, y: 0.329, Y: 1.0 };
            let curve = ToneCurve::table_f32(&srgb_decode_table(65536))
                .ok_or(CmsError::ProfileCreation("cmsBuildTabulatedToneCurveFloat returned NULL"))?;
            let handle = sys::create_gray(&white, &curve)
                .ok_or(CmsError::ProfileCreation("cmsCreateGrayProfile returned NULL"))?;
            save(&handle)
        }
        _ => Err(CmsError::ProfileCreation("unknown built-in profile")),
    }
}

/// The tone curves of `imagecodecs.cms_profile('rgb' | 'gray', gamma=... | transferfunction=...)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransferFunction<'a> {
    /// Neither argument: no curves (`cmsCreateRGBProfile(..., NULL)`, `cmsCreateGrayProfile(..., NULL)`).
    None,
    /// `gamma=g`: one `cmsBuildGamma(NULL, g)` per channel.
    Gamma(f64),
    /// A 1-D float32 `transferfunction`: one `cmsBuildTabulatedToneCurveFloat` per channel, all from this table.
    Table(&'a [f32]),
    /// A 3×N float32 `transferfunction` (RGB only): one table per channel.
    Tables([&'a [f32]; 3]),
}

fn build_curve(transfer: TransferFunction<'_>, channel: usize) -> Result<Option<ToneCurve>> {
    let table = match transfer {
        TransferFunction::None => return Ok(None),
        TransferFunction::Gamma(gamma) => {
            return ToneCurve::gamma(gamma).map(Some).ok_or(CmsError::ProfileCreation("cmsBuildGamma returned NULL"))
        }
        TransferFunction::Table(table) => table,
        TransferFunction::Tables(tables) => tables[channel],
    };
    ToneCurve::table_f32(table)
        .map(Some)
        .ok_or(CmsError::ProfileCreation("cmsBuildTabulatedToneCurveFloat returned NULL"))
}

fn xy(white: Option<[f64; 2]>) -> Option<CIExyY> {
    white.map(|[x, y]| CIExyY { x, y, Y: 1.0 })
}

/// Port of `imagecodecs.cms_profile('rgb', whitepoint=(x, y), primaries=(6 values), gamma=... |
/// transferfunction=...)`, the calibrated profiles of `luma/rasterio.py _png_profile` and `_tiff_profile`:
/// `cmsCreateRGBProfile` with three separately built curves (or none). `None` arguments are the oracle's
/// `None` (NULL pointers). The bytes equal the oracle's apart from the creation time (header bytes 24..36).
pub fn rgb_profile(
    whitepoint: Option<[f64; 2]>,
    primaries: Option<[f64; 6]>,
    transfer: TransferFunction<'_>,
) -> Result<Profile> {
    let white = xy(whitepoint);
    let primaries = primaries.map(|p| CIExyYTriple {
        red: CIExyY { x: p[0], y: p[1], Y: 1.0 },
        green: CIExyY { x: p[2], y: p[3], Y: 1.0 },
        blue: CIExyY { x: p[4], y: p[5], Y: 1.0 },
    });
    let curves = [build_curve(transfer, 0)?, build_curve(transfer, 1)?, build_curve(transfer, 2)?];
    let curves = match &curves {
        [Some(r), Some(g), Some(b)] => Some([r, g, b]),
        _ => None,
    };
    let handle = sys::create_rgb_opt(white.as_ref(), primaries.as_ref(), curves)
        .ok_or(CmsError::ProfileCreation("cmsCreateRGBProfile returned NULL"))?;
    save(&handle).map(Profile::from_bytes)
}

/// Port of `imagecodecs.cms_profile('gray', whitepoint=(x, y), gamma=... | transferfunction=...)`:
/// `cmsCreateGrayProfile` with one curve (or none). A 3×N table is the oracle's `ValueError`.
pub fn gray_profile(whitepoint: Option<[f64; 2]>, transfer: TransferFunction<'_>) -> Result<Profile> {
    if matches!(transfer, TransferFunction::Tables(_)) {
        return Err(CmsError::ProfileCreation("invalid transferfunction shape or dtype"));
    }
    let white = xy(whitepoint);
    let curve = build_curve(transfer, 0)?;
    let handle = sys::create_gray_opt(white.as_ref(), curve.as_ref())
        .ok_or(CmsError::ProfileCreation("cmsCreateGrayProfile returned NULL"))?;
    save(&handle).map(Profile::from_bytes)
}

/// Port of `imagecodecs.cms_profile('lab4')` (`version4`) and `cms_profile('lab2')`, with an optional
/// `whitepoint=(x, y)`: `cmsCreateLab4Profile` / `cmsCreateLab2Profile` (NULL is D50). `rasterio._linear`
/// converts untagged CIELAB TIFFs from `cms_profile('lab4')`.
pub fn lab_profile(version4: bool, whitepoint: Option<[f64; 2]>) -> Result<Profile> {
    let white = xy(whitepoint);
    let handle = sys::create_lab(version4, white.as_ref()).ok_or(CmsError::ProfileCreation(if version4 {
        "cmsCreateLab4Profile returned NULL"
    } else {
        "cmsCreateLab2Profile returned NULL"
    }))?;
    save(&handle).map(Profile::from_bytes)
}

/// `engine.to_linear(np.linspace(0, 1, 65536, dtype=np.float32))`: the sRGB decoding table `rasterio` passes as
/// `transferfunction` when a PNG has `cHRM` without `gAMA`, or a TIFF has chromaticities without a
/// `TransferFunction` (on 0..1 the `np.maximum(a + .055, 0)` of `to_linear` changes nothing).
pub fn srgb_transfer_table() -> Vec<f32> {
    srgb_decode_table(65536)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linspace_matches_numpy_endpoints() {
        let x = linspace_f32(4096);
        assert_eq!(x[0], 0.0);
        assert_eq!(x[4095], 1.0);
        assert_eq!(x[1], (1.0f64 / 4095.0) as f32);
    }

    #[test]
    fn description_strip() {
        assert_eq!(decode_description(b"  sRGB built-in\0"), Some("sRGB built-in".into()));
        assert_eq!(decode_description(b"\0"), None);
        assert_eq!(decode_description(b"a\xffb\0"), Some("a\u{FFFD}b".into()));
    }
}
