//! The pixel path around lens profiles (luma/optics.py `_sensor_view`, `_modifier`, `lens_shading`,
//! `lens_geometry`). The lens maths itself (lensfun through lensfunpy in the oracle) stays behind
//! [`LensModel`]: this crate never reads lens databases.

use std::borrow::Cow;

use grainy_core::Settings;
use grainy_imgproc::{remap, remap_interleaved, Border, Interpolation};
use ndarray::{s, Array3, Array4, ArrayView3, Axis};
use serde_json::Value;

use crate::orient::orient_pixels;
use crate::{Error, Result};

/// `optics.MAP_BUDGET`: above this many bytes of coordinate maps (24 per pixel with lateral CA, 8 without)
/// the geometry is corrected in strips of [`STRIP_ROWS`] rows.
pub const MAP_BUDGET: usize = 32 * 1024 * 1024;
/// Rows per strip above the map budget.
pub const STRIP_ROWS: usize = 128;

/// The lensfun `ModifyFlags` the oracle requests (`lf.ModifyFlags`: TCA 1, VIGNETTING 2, DISTORTION 8, SCALE 32).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LensFlags {
    /// `VIGNETTING` (colour modification only).
    pub vignetting: bool,
    /// `DISTORTION`.
    pub distortion: bool,
    /// `TCA` (lateral chromatic aberration).
    pub tca: bool,
    /// `SCALE` (automatic scale so no empty edges show).
    pub scale: bool,
}

impl LensFlags {
    /// The lensfun bit mask (`int(flags)` in the oracle).
    pub fn bits(&self) -> u32 {
        u32::from(self.tca)
            | u32::from(self.vignetting) << 1
            | u32::from(self.distortion) << 3
            | u32::from(self.scale) << 5
    }
}

/// What the oracle's `_modifier` builds: `lf.Modifier(lens, crop, width, height)` then
/// `initialize(focal, aperture, distance, scale=scale, pixel_format=np.float32, flags=flags)`.
#[derive(Clone, Debug, PartialEq)]
pub struct LensRequest {
    /// `settings['lensfun']` as stored (holds the profile `xml`, the conditions and `orientation`).
    pub profile: Value,
    /// Width of the frame in the sensor's orientation (`_sensor_view`).
    pub width: usize,
    /// Height of the frame in the sensor's orientation.
    pub height: usize,
    /// Sensor crop factor.
    pub crop: f64,
    /// Focal length in mm.
    pub focal: f64,
    /// Aperture (0 when unknown).
    pub aperture: f64,
    /// Subject distance in m.
    pub distance: f64,
    /// `scale` argument of `initialize`: 0 (automatic) when `lensfun_scale`, else 1.
    pub scale: f64,
    /// Requested corrections.
    pub flags: LensFlags,
}

/// A lens model: the three `lensfunpy.Modifier` calls the oracle makes, for the modifier described by a
/// [`LensRequest`]. `Ok(None)` is lensfunpy's `None` (nothing to correct); `Err` is a failure of the model
/// (for example a profile it cannot read), reported as [`Error::Lens`].
pub trait LensModel {
    /// `apply_geometry_distortion(0, y, width, rows)`: source coordinates for `rows` output rows from row `y`,
    /// shape rows × width × 2 (x, y).
    fn geometry_coordinates(
        &self,
        request: &LensRequest,
        y: usize,
        rows: usize,
    ) -> std::result::Result<Option<Array3<f32>>, String>;
    /// `apply_subpixel_geometry_distortion(0, y, width, rows)`: per-channel source coordinates,
    /// shape rows × width × 3 × 2.
    fn subpixel_coordinates(
        &self,
        request: &LensRequest,
        y: usize,
        rows: usize,
    ) -> std::result::Result<Option<Array4<f32>>, String>;
    /// `apply_color_modification(pixels)`: vignetting correction in place on a C-contiguous float32
    /// H×W×C frame; `false` when nothing was changed.
    fn apply_color(&self, request: &LensRequest, pixels: &mut Array3<f32>) -> std::result::Result<bool, String>;
}

fn number(data: &Value, key: &str, default: Option<f64>) -> f64 {
    match data.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        None => default.unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// Port of `optics._sensor_view`: the frame in the sensor's own orientation, and the stored orientation.
pub fn sensor_view(pixels: ArrayView3<'_, f32>, profile: &Value) -> Result<(Array3<f32>, i64)> {
    let orientation = match profile.get("orientation") {
        None => 1,
        Some(Value::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::Bool(b)) => i64::from(*b),
        Some(_) => 0,
    };
    let inverse = match orientation {
        6 => 8,
        8 => 6,
        o => o,
    };
    if !(1..=8).contains(&orientation) {
        return Err(Error::Value("사진 방향 정보가 올바르지 않습니다.".into()));
    }
    Ok((orient_pixels(pixels, inverse), orientation))
}

/// Port of `optics._modifier` for a frame of `height`×`width` (sensor orientation): `Ok(None)` when lens
/// correction is off or no correction is requested. The stored conditions are validated first (the oracle's
/// error), then the lens model is required ([`Error::LensModelMissing`]) where the oracle loads the profile.
/// Reading the profile XML itself is the model's job.
pub fn modifier_request(
    settings: &Settings,
    height: usize,
    width: usize,
    color: bool,
    lens: Option<&dyn LensModel>,
) -> Result<Option<LensRequest>> {
    let data = settings.value("lensfun");
    if !grainy_core::settings::truthy(data) || !settings.bool("lensfun_enabled") {
        return Ok(None);
    }
    let crop = number(data, "crop", None);
    let focal = number(data, "focal", None);
    let aperture = number(data, "aperture", Some(0.0));
    let distance = number(data, "distance", Some(1000.0));
    let values = [crop, focal, aperture, distance];
    if !values.iter().all(|v| v.is_finite())
        || !((0.1..=20.0).contains(&crop)
            && (0.1..=5000.0).contains(&focal)
            && (0.0..=128.0).contains(&aperture)
            && (0.01..=100000.0).contains(&distance))
    {
        return Err(Error::Value("렌즈 프로파일의 촬영 조건이 올바르지 않습니다.".into()));
    }
    if lens.is_none() {
        return Err(Error::LensModelMissing);
    }
    let mut flags = LensFlags::default();
    if color {
        if settings.bool("lensfun_vignette") && aperture > 0.0 {
            flags.vignetting = true;
        }
    } else {
        flags.distortion = settings.bool("lensfun_distortion");
        flags.tca = settings.bool("lensfun_tca");
        flags.scale = (flags.distortion || flags.tca) && settings.bool("lensfun_scale");
    }
    if flags == LensFlags::default() {
        return Ok(None);
    }
    Ok(Some(LensRequest {
        profile: data.clone(),
        width,
        height,
        crop,
        focal,
        aperture,
        distance,
        scale: if settings.bool("lensfun_scale") { 0.0 } else { 1.0 },
        flags,
    }))
}

/// Port of `optics.lens_shading`: lens vignetting correction (borrowed input when it does not apply).
pub fn lens_shading<'a>(
    pixels: &'a Array3<f32>,
    settings: &Settings,
    lens: Option<&dyn LensModel>,
) -> Result<Cow<'a, Array3<f32>>> {
    if !settings.bool("lensfun_enabled")
        || !grainy_core::settings::truthy(settings.value("lensfun"))
        || !settings.bool("lensfun_vignette")
    {
        return Ok(Cow::Borrowed(pixels));
    }
    let (mut sensor, orientation) = sensor_view(pixels.view(), settings.value("lensfun"))?;
    let (h, w, _) = sensor.dim();
    let Some(request) = modifier_request(settings, h, w, true, lens)? else {
        return Ok(Cow::Borrowed(pixels));
    };
    let model = lens.ok_or(Error::LensModelMissing)?;
    if !model.apply_color(&request, &mut sensor).map_err(Error::Lens)? {
        return Ok(Cow::Borrowed(pixels));
    }
    Ok(Cow::Owned(orient_pixels(sensor.view(), orientation)))
}

/// Port of `optics.lens_geometry`: distortion and lateral CA correction (borrowed input when it does not apply).
pub fn lens_geometry<'a>(
    pixels: &'a Array3<f32>,
    settings: &Settings,
    lens: Option<&dyn LensModel>,
) -> Result<Cow<'a, Array3<f32>>> {
    lens_geometry_with_budget(pixels, settings, lens, MAP_BUDGET)
}

fn check_geometry(coords: &Array3<f32>, rows: usize, width: usize) -> Result<()> {
    if coords.dim() != (rows, width, 2) {
        return Err(Error::Lens(format!(
            "geometry coordinates have shape {:?}, expected ({rows}, {width}, 2)",
            coords.dim()
        )));
    }
    Ok(())
}

fn check_subpixel(coords: &Array4<f32>, rows: usize, width: usize) -> Result<()> {
    if coords.dim() != (rows, width, 3, 2) {
        return Err(Error::Lens(format!(
            "subpixel coordinates have shape {:?}, expected ({rows}, {width}, 3, 2)",
            coords.dim()
        )));
    }
    Ok(())
}

fn remap_planes(sensor: &Array3<f32>, coords: &Array4<f32>, output: &mut Array3<f32>, y: usize) -> Result<()> {
    let rows = coords.dim().0;
    for c in 0..3 {
        let plane = sensor.index_axis(Axis(2), c).to_owned();
        let mx = coords.slice(s![.., .., c, 0]).to_owned();
        let my = coords.slice(s![.., .., c, 1]).to_owned();
        let out = remap(&plane, &mx, &my, Interpolation::Linear, Border::Constant(0.0))?;
        output.slice_mut(s![y..y + rows, .., c]).assign(&out);
    }
    Ok(())
}

/// [`lens_geometry`] with an explicit map budget in bytes (the oracle's `MAP_BUDGET`), so the 128-row strip
/// path can be exercised on small frames.
pub fn lens_geometry_with_budget<'a>(
    pixels: &'a Array3<f32>,
    settings: &Settings,
    lens: Option<&dyn LensModel>,
    budget: usize,
) -> Result<Cow<'a, Array3<f32>>> {
    if !settings.bool("lensfun_enabled") || !grainy_core::settings::truthy(settings.value("lensfun")) {
        return Ok(Cow::Borrowed(pixels));
    }
    let (sensor, orientation) = sensor_view(pixels.view(), settings.value("lensfun"))?;
    let (h, w, channels) = sensor.dim();
    let Some(request) = modifier_request(settings, h, w, false, lens)? else {
        return Ok(Cow::Borrowed(pixels));
    };
    let model = lens.ok_or(Error::LensModelMissing)?;
    let chromatic = channels == 3 && settings.bool("lensfun_tca");
    let mut output = Array3::<f32>::zeros((h, w, channels));
    let map_bytes = h * w * if chromatic { 24 } else { 8 };
    if map_bytes <= budget {
        if chromatic {
            let Some(coords) = model.subpixel_coordinates(&request, 0, h).map_err(Error::Lens)? else {
                return Ok(Cow::Borrowed(pixels));
            };
            check_subpixel(&coords, h, w)?;
            remap_planes(&sensor, &coords, &mut output, 0)?;
        } else {
            let Some(coords) = model.geometry_coordinates(&request, 0, h).map_err(Error::Lens)? else {
                return Ok(Cow::Borrowed(pixels));
            };
            check_geometry(&coords, h, w)?;
            output = remap_interleaved(&sensor, &coords, Interpolation::Linear, Border::Constant(0.0))?;
        }
        return Ok(Cow::Owned(orient_pixels(output.view(), orientation)));
    }
    for y in (0..h).step_by(STRIP_ROWS) {
        let count = STRIP_ROWS.min(h - y);
        if chromatic {
            let Some(coords) = model.subpixel_coordinates(&request, y, count).map_err(Error::Lens)? else {
                return Ok(Cow::Borrowed(pixels));
            };
            check_subpixel(&coords, count, w)?;
            remap_planes(&sensor, &coords, &mut output, y)?;
        } else {
            let Some(coords) = model.geometry_coordinates(&request, y, count).map_err(Error::Lens)? else {
                return Ok(Cow::Borrowed(pixels));
            };
            check_geometry(&coords, count, w)?;
            let strip = remap_interleaved(&sensor, &coords, Interpolation::Linear, Border::Constant(0.0))?;
            output.slice_mut(s![y..y + count, .., ..]).assign(&strip);
        }
    }
    Ok(Cow::Owned(orient_pixels(output.view(), orientation)))
}
