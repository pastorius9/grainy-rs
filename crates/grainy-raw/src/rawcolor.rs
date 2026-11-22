//! Port of `luma/rawcolor.py`, with the oracle's Robertson table and precision.
use crate::{
    dcp::{self, Profile, Weight},
    invalid,
    math::{self, diag, inverse, mm, mv, valid_matrix, Mat, ID},
    tiff, DecodeMode, RawFile, Result,
};
use grainy_color::illuminants;
use grainy_core::{Image, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

const LINES: [[f64; 4]; 31] = [
    [0., 0.18006, 0.26352, -0.24341],
    [10., 0.18066, 0.26589, -0.25479],
    [20., 0.18133, 0.26846, -0.26876],
    [30., 0.18208, 0.27119, -0.28539],
    [40., 0.18293, 0.27407, -0.30470],
    [50., 0.18388, 0.27709, -0.32675],
    [60., 0.18494, 0.28021, -0.35156],
    [70., 0.18611, 0.28342, -0.37915],
    [80., 0.18740, 0.28668, -0.40955],
    [90., 0.18880, 0.28997, -0.44278],
    [100., 0.19032, 0.29326, -0.47888],
    [125., 0.19462, 0.30141, -0.58204],
    [150., 0.19962, 0.30921, -0.70471],
    [175., 0.20525, 0.31647, -0.84901],
    [200., 0.21142, 0.32312, -1.0182],
    [225., 0.21807, 0.32909, -1.2168],
    [250., 0.22511, 0.33439, -1.4512],
    [275., 0.23247, 0.33904, -1.7298],
    [300., 0.24010, 0.34308, -2.0637],
    [325., 0.24792, 0.34655, -2.4681],
    [350., 0.25591, 0.34951, -2.9641],
    [375., 0.26400, 0.35200, -3.5814],
    [400., 0.27218, 0.35407, -4.3633],
    [425., 0.28039, 0.35577, -5.3762],
    [450., 0.28863, 0.35714, -6.7262],
    [475., 0.29685, 0.35823, -8.5955],
    [500., 0.30505, 0.35907, -11.324],
    [525., 0.31320, 0.35968, -15.628],
    [550., 0.32129, 0.36011, -23.325],
    [575., 0.32931, 0.36038, -40.770],
    [600., 0.33724, 0.36051, -116.45],
];
fn direction(i: usize) -> [f64; 2] {
    let s = LINES[i][3];
    let norm = (1. + s * s).sqrt();
    [1. / norm, s / norm]
}
fn blend(i: usize, f: f64) -> ([f64; 2], [f64; 2]) {
    let uv = std::array::from_fn(|j| LINES[i - 1][j + 1] * (1. - f) + LINES[i][j + 1] * f);
    let a = direction(i - 1);
    let b = direction(i);
    let mut d: [f64; 2] = std::array::from_fn(|j| a[j] * (1. - f) + b[j] * f);
    let norm = (d[0] * d[0] + d[1] * d[1]).sqrt();
    d = d.map(|v| v / norm);
    (uv, d)
}
/// Port of rawcolor.CameraSource.raw_info, a JSON-safe sensor calibration record.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraInfo {
    /// rawcolor.decode XYZ to camera matrix.
    pub xyz_to_camera: Mat,
    /// rawcolor.decode camera neutral.
    pub neutral: [f64; 3],
    /// rawcolor.decode daylight neutral.
    pub daylight_neutral: [f64; 3],
    /// rawcolor.read_dng_calibration AnalogBalance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analog: Option<[f64; 3]>,
    /// rawcolor.read_dng_calibration CameraCalibration matrices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<Vec<Mat>>,
    /// rawcolor.read_dng_calibration calibration signature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration_signature: Option<String>,
    /// rawcolor.read_dng_calibration embedded profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_profile: Option<Profile>,
    /// Apple ProRAW / DNG 1.6 `ProfileGainTableMap` (tag 52525) spatially-varying gain map.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain_table_map: Option<crate::proraw::ProfileGainTableMap>,
    /// rawcolor.decode camera metadata (make, camera, format, unique_camera).
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, Value>,
}
impl Default for CameraInfo {
    fn default() -> Self {
        Self {
            xyz_to_camera: ID,
            neutral: [1.; 3],
            daylight_neutral: [1.; 3],
            analog: None,
            calibration: None,
            calibration_signature: None,
            base_profile: None,
            gain_table_map: None,
            metadata: Default::default(),
        }
    }
}
/// Port of rawcolor.CameraSource, keeping owned sensor pixels and calibration together.
#[derive(Clone, Debug)]
pub struct CameraSource {
    pixels: Image,
    info: CameraInfo,
}
impl CameraSource {
    /// Port of rawcolor.CameraSource.__new__; owns an immutable copy of the calibration record.
    pub fn new(pixels: Image, info: CameraInfo) -> Result<Self> {
        math::image(&pixels)?;
        Ok(Self { pixels, info })
    }
    /// Port of numpy.asarray(CameraSource), the camera-space pixels.
    pub fn pixels(&self) -> &Image {
        &self.pixels
    }
    /// Port of CameraSource.raw_info.
    pub fn raw_info(&self) -> &CameraInfo {
        &self.info
    }
}
/// Port of rawcolor.enabled, including negatives that force camera-space decode.
pub fn enabled(s: &Settings) -> bool {
    s.str("raw_mode") != "legacy" || s.bool("dcp_profile") || s.bool("negative")
}
/// Port of rawcolor.xyz_temperature; preserves the oracle's 325-mired .24792 entry.
pub fn xyz_temperature(xyz: [f64; 3]) -> Result<(f64, f64)> {
    let den = xyz[0] + 15. * xyz[1] + 3. * xyz[2];
    if !den.is_finite() || den <= 0. {
        return Err(invalid("cannot calculate camera white point"));
    }
    let uv = [4. * xyz[0] / den, 6. * xyz[1] / den];
    let dist: [f64; 31] = std::array::from_fn(|i| {
        let d = direction(i);
        (uv[1] - LINES[i][2]) * d[0] - (uv[0] - LINES[i][1]) * d[1]
    });
    let i = (1..31).find(|i| dist[*i] <= 0.).unwrap_or(30);
    let f = (dist[i - 1] / (dist[i - 1] - dist[i]).max(1e-15)).clamp(0., 1.);
    let mired = LINES[i - 1][0] * (1. - f) + LINES[i][0] * f;
    let (locus, d) = blend(i, f);
    let tint = -((uv[0] - locus[0]) * d[0] + (uv[1] - locus[1]) * d[1]) * 3000.;
    Ok(((1e6 / mired.max(1e-6)).clamp(1667., 100000.), tint))
}
/// Port of rawcolor.temperature_xyz, including the positive-white clamp at extreme tint.
pub fn temperature_xyz(kelvin: f64, tint: f64) -> Result<[f64; 3]> {
    if !kelvin.is_finite()
        || !tint.is_finite()
        || !(2000. ..=50000.).contains(&kelvin)
        || !(-150. ..=150.).contains(&tint)
    {
        return Err(invalid("RAW temperature/tint outside 2000..50000 K, -150..150"));
    }
    let m = 1e6 / kelvin;
    let i = LINES.partition_point(|l| l[0] < m).clamp(1, 30);
    let f = (m - LINES[i - 1][0]) / (LINES[i][0] - LINES[i - 1][0]);
    let (uv, d) = blend(i, f);
    let u = uv[0] - d[0] * tint / 3000.;
    let v = uv[1] - d[1] * tint / 3000.;
    Ok([1.5 * u / v, 1., (4. - u - 10. * v) / (2. * v)].map(|v| v.max(1e-6)))
}
/// Port of rawcolor.default_profile.
pub fn default_profile(info: &CameraInfo) -> Result<Profile> {
    if let Some(p) = &info.base_profile {
        return Ok(p.clone());
    }
    Ok(Profile { cm1: Some(valid_matrix(info.xyz_to_camera, false)?), t1: Some(6504.), ..Profile::default() })
}
/// Port of rawcolor.individual_matrix.
pub fn individual_matrix(p: &Profile, info: &CameraInfo, w: Weight) -> Result<Mat> {
    let cc = if info.calibration_signature.as_deref().unwrap_or("") == p.signature {
        dcp::calibration_matrix(info.calibration.as_deref().unwrap_or(&[]), w)?
    } else {
        ID
    };
    Ok(mm(diag(info.analog.unwrap_or([1.; 3])), cc))
}
/// Port of rawcolor.neutral_white: XYZ, Kelvin, tint.
pub fn neutral_white(p: &Profile, neutral: [f64; 3], info: &CameraInfo) -> Result<([f64; 3], f64, f64)> {
    if neutral.iter().any(|v| !v.is_finite() || *v <= 0.) {
        return Err(invalid("invalid RAW neutral"));
    }
    if p.cm3.is_some() || p.custom_illuminants {
        let mut last = [0.3457, 0.3585];
        let mut next = last;
        for step in 0..30 {
            let white = illuminants::xy_to_xyz(last)?;
            let w = dcp::weight(p, 6504., Some(white))?;
            let cm = mm(
                individual_matrix(p, info, w)?,
                dcp::interpolate(p, "cm", w)?.ok_or_else(|| invalid("missing ColorMatrix"))?,
            );
            next = illuminants::xyz_to_xy(mv(inverse(cm)?, neutral))?;
            if (next[0] - last[0]).abs() + (next[1] - last[1]).abs() < 1e-7 {
                break;
            }
            if step == 29 {
                next = std::array::from_fn(|i| (next[i] + last[i]) * 0.5);
            }
            last = next;
        }
        let xyz = illuminants::xy_to_xyz(next)?;
        let (t, tint) = xyz_temperature(xyz)?;
        return Ok((xyz, t, tint));
    }
    let mut temperature = 6504.;
    let (mut xyz, mut new, mut tint) = ([0.; 3], 0., 0.);
    for _ in 0..30 {
        let w = dcp::weight(p, temperature, None)?;
        let cm = mm(
            individual_matrix(p, info, w)?,
            dcp::interpolate(p, "cm", w)?.ok_or_else(|| invalid("missing ColorMatrix"))?,
        );
        xyz = mv(inverse(cm)?, neutral);
        (new, tint) = xyz_temperature(xyz)?;
        if (new - temperature).abs() < 0.01 {
            break;
        }
        temperature = (temperature + new) / 2.;
    }
    Ok((xyz, new, tint))
}
/// Port of rawcolor.resolve_white with with_white=True: neutral, Kelvin, tint, XYZ.
pub fn resolve_white(p: &Profile, info: &CameraInfo, s: &Settings) -> Result<([f64; 3], f64, f64, [f64; 3])> {
    let (neutral, temperature, tint, xyz) = if s.str("raw_mode") == "custom" {
        let (t, tint) = (s.f64("raw_kelvin"), s.f64("raw_tint"));
        let xyz = temperature_xyz(t, tint)?;
        let w = dcp::weight(p, t, Some(xyz))?;
        let cm = dcp::interpolate(p, "cm", w)?.ok_or_else(|| invalid("missing ColorMatrix"))?;
        let n = mv(mm(individual_matrix(p, info, w)?, cm), xyz);
        let floor = n.iter().copied().fold(1., f64::max) * 1e-4;
        (n.map(|v| v.max(floor)), t, tint, xyz)
    } else {
        let neutral = match s.str("raw_mode") {
            "sample" => serde_json::from_value(s.value("raw_neutral").clone())
                .map_err(|_| invalid("invalid sampled neutral"))?,
            "daylight" => info.daylight_neutral,
            _ => info.neutral,
        };
        let (xyz, t, tint) = neutral_white(p, neutral, info)?;
        (neutral, t, tint, xyz)
    };
    if neutral.iter().any(|v| *v <= 0. || !v.is_finite()) {
        return Err(invalid("selected RAW white is outside camera gamut"));
    }
    Ok((neutral.map(|v| v / neutral[1]), temperature, tint, xyz))
}
/// Port of rawcolor.develop_camera for CameraSource. Plain arrays use develop_plain.
pub fn develop_camera(source: &CameraSource, s: &Settings) -> Result<Image> {
    if s.bool("negative") {
        return Ok(source.pixels.clone());
    }
    let info = &source.info;
    let record = s.value("dcp_profile");
    let has_record = grainy_core::settings::truthy(record);
    let p = if has_record { dcp::compiled(record)? } else { default_profile(info)? };
    if has_record && !dcp::camera_matches(&p, &Value::Object(info.metadata.clone())) {
        return Err(invalid("DCP camera model does not match RAW"));
    }
    let (neutral, t, _, white) = resolve_white(&p, info, s)?;
    let m = dcp::camera_to_xyz(
        &p,
        neutral,
        t,
        info.calibration.as_deref(),
        info.analog,
        info.calibration_signature.as_deref().unwrap_or(""),
        Some(white),
    )?;
    let transform = mm(dcp::xyz_prophoto()?, m).map(|r| r.map(|v| v as f32));
    let mut a = if s.i64("tone_version") >= 4 {
        let n = neutral.map(|v| v as f32);
        math::map(&source.pixels, |v| {
            let r = reconstruct_camera_highlight(v, n);
            transform.map(|row| r[0] * row[0] + r[1] * row[1] + r[2] * row[2])
        })
    } else {
        math::map(&source.pixels, |v| transform.map(|r| v[0] * r[0] + v[1] * r[1] + v[2] * r[2]))
    };
    if let Some(gain_map) = &info.gain_table_map {
        gain_map.apply(&mut a);
    }
    if has_record {
        let hs = dcp::interpolate_table(&p, dcp::weight(&p, t, Some(white))?)?;
        if s.bool("dcp_huesat") {
            if let Some(hs) = hs {
                a = dcp::table_map(&a, &hs, p.hs_encoding)?;
            }
        }
        if s.bool("dcp_exposure") {
            let exposure = 2f64.powf(p.exposure) as f32;
            a.mapv_inplace(|v| v * exposure);
        }
        if s.bool("dcp_look") {
            if let Some(look) = &p.look {
                a = dcp::table_map(&a, look, p.look_encoding)?;
            }
        }
        if s.bool("dcp_tone") {
            a = dcp::tone_map(&a, &p.curve)?;
        }
    }
    if s.str("working_space") != "ProPhoto" {
        a = crate::sys::prophoto_srgb(&a)?;
    }
    Ok(a)
}
/// Process Version 4 camera-space highlight reconstruction: when 1 or 2 sensor channels saturate
/// before white balance, reconstruct clipped channels from the unclipped channels' white-balanced
/// luminance so blown highlights roll off cleanly to neutral white instead of pink/magenta casts.
/// Pixels with all channels <= 0.90 are returned bit-exact unchanged.
#[inline]
pub(crate) fn reconstruct_camera_highlight(c: [f32; 3], neutral: [f32; 3]) -> [f32; 3] {
    let max_c = c[0].max(c[1]).max(c[2]);
    if max_c <= 0.90 {
        return c;
    }
    let smooth = |x: f32| {
        let t = ((x - 0.90) / 0.095).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let cw = [smooth(c[0]), smooth(c[1]), smooth(c[2])];
    let wb = [c[0] / neutral[0].max(1e-6), c[1] / neutral[1].max(1e-6), c[2] / neutral[2].max(1e-6)];
    let unclipped_w = (1.0 - cw[0]) + (1.0 - cw[1]) + (1.0 - cw[2]);
    let wb_max = wb[0].max(wb[1]).max(wb[2]);
    let est = if unclipped_w > 0.01 {
        ((1.0 - cw[0]) * wb[0] + (1.0 - cw[1]) * wb[1] + (1.0 - cw[2]) * wb[2]) / unclipped_w
    } else {
        wb_max
    };
    let all_clip = cw[0].min(cw[1]).min(cw[2]);
    std::array::from_fn(|i| {
        let lifted = wb[i] + (est.max(wb[i]) - wb[i]) * cw[i];
        let recovered = lifted + (wb_max - lifted) * all_clip;
        recovered * neutral[i]
    })
}

/// Port of rawcolor.develop_camera's non-CameraSource identity branch.
pub fn develop_plain(source: &Image) -> Result<Image> {
    math::image(source)?;
    Ok(source.clone())
}
fn negative(a: Image, s: &Settings) -> Result<Image> {
    if !s.bool("negative") || s.is_none("negative_base") || s.is_none("negative_peak") {
        return Ok(a);
    }
    let base: [f32; 3] = serde_json::from_value(s.value("negative_base").clone())?;
    let peak: [f32; 3] = serde_json::from_value(s.value("negative_peak").clone())?;
    if base.iter().any(|v| !v.is_finite() || *v <= 0.) || peak.iter().any(|v| !v.is_finite()) {
        return Err(invalid("invalid negative film endpoints"));
    }
    let range = s.f64("negative_range");
    let density = s.f64("negative_density");
    if !range.is_finite() || !density.is_finite() {
        return Err(invalid("invalid negative range/density"));
    }
    let span: [f32; 3] = std::array::from_fn(|i| (base[i] / peak[i].max(1e-5).min(base[i] * 0.9)).ln());
    let scale = span.map(|v| (range * std::f64::consts::LN_2) as f32 / v);
    let offset = ((density - range) * std::f64::consts::LN_2) as f32;
    let ceiling = (density * std::f64::consts::LN_2) as f32;
    Ok(math::map(&a, |p| {
        std::array::from_fn(|i| ((base[i].ln() - p[i].max(1e-5).ln()) * scale[i] + offset).min(ceiling).exp())
    }))
}
/// Port of rawcolor.linear_frame; private negative.apply helper until grainy-film is integrated.
pub fn linear_frame(source: &CameraSource, s: &Settings) -> Result<Image> {
    negative(develop_camera(source, s)?, s)
}
/// Port of rawcolor.linear_frame for an ordinary RGB ndarray.
pub fn linear_frame_plain(source: &Image, s: &Settings) -> Result<Image> {
    negative(develop_plain(source)?, s)
}
/// Port of rawcolor.retarget, including sampled-neutral to Kelvin conversion.
pub fn retarget(s: &Settings, metadata: &Value) -> Result<Settings> {
    let mut result = s.clone();
    if !crate::raw_defaults::is_raw(metadata, None) {
        result.set("raw_mode", "legacy");
        result.set("raw_neutral", Value::Null);
        result.set("dcp_profile", Value::Null);
    } else if s.bool("dcp_profile") && !dcp::camera_matches(&dcp::compiled(s.value("dcp_profile"))?, metadata) {
        result.set("dcp_profile", Value::Null);
    }
    if result.str("raw_mode") == "sample" {
        result.set("raw_mode", "custom");
        result.set("raw_neutral", Value::Null);
    }
    Ok(result)
}
/// Port of rawcolor.read_dng_calibration. Reads colour tags only, never image data.
pub fn read_dng_calibration(path: impl AsRef<Path>, info: &mut CameraInfo) -> Result<()> {
    let path = path.as_ref();
    if !path.extension().is_some_and(|s| s.eq_ignore_ascii_case("dng")) {
        return Ok(());
    }
    let wanted = [
        50708,
        50721,
        50722,
        50723,
        50724,
        50727,
        50728,
        50778,
        50779,
        50931,
        50932,
        50964,
        50965,
        crate::proraw::TAG_PROFILE_GAIN_TABLE_MAP,
        52529,
        52530,
        52531,
        52532,
        52533,
        52534,
        52535,
    ];
    let (pages, le) = tiff::dng(path, &wanted, false)?;
    let mut tags = std::collections::BTreeMap::new();
    for page in pages {
        tags.extend(page);
    }
    if let Some(cm) = dcp::matrix_tag(&tags, 50721, false)? {
        info.base_profile = Some(dcp::parse_tags(&tags, le)?);
        info.xyz_to_camera = cm;
    }
    let unique = tags
        .get(&50708)
        .map(tiff::Tag::text)
        .unwrap_or_else(|| info.metadata.get("camera").and_then(Value::as_str).unwrap_or("").into());
    info.metadata.insert("unique_camera".into(), Value::String(unique));
    for tag in [50728, 50727] {
        if let Some(v) = tags.get(&tag) {
            let a = v.numbers()?;
            if a.len() != 3 || a.iter().any(|v| !v.is_finite() || *v <= 0.) {
                return Err(invalid("invalid DNG neutral/analog balance"));
            }
            let value = [a[0], a[1], a[2]];
            if tag == 50728 {
                info.neutral = value;
            } else {
                info.analog = Some(value);
            }
        }
    }
    let calibration = [
        dcp::matrix_tag(&tags, 50723, false)?,
        dcp::matrix_tag(&tags, 50724, false)?,
        dcp::matrix_tag(&tags, 52530, false)?,
    ];
    if calibration.iter().any(Option::is_some) {
        info.calibration = Some(calibration.map(|m| m.unwrap_or(ID)).to_vec());
        info.calibration_signature = Some(tags.get(&50931).map(tiff::Tag::text).unwrap_or_default());
    }
    if let Some(bytes) = tags.get(&crate::proraw::TAG_PROFILE_GAIN_TABLE_MAP).and_then(tiff::Tag::bytes) {
        if let Ok(map) = crate::proraw::ProfileGainTableMap::parse(bytes) {
            info.gain_table_map = Some(map);
        }
    }
    Ok(())
}
/// Port of rawcolor.reduce_false_color: replicated-border 3x3 median of R-G and B-G.
pub fn reduce_false_color(rgb: &Image) -> Result<Image> {
    use rayon::prelude::*;
    let _timing = grainy_core::timing::Span::new("load.reduce_false_color");
    math::image(rgb)?;
    let (h, w, _) = rgb.dim();
    if h < 3 || w < 3 {
        return Ok(rgb.clone());
    }
    let input = rgb.as_standard_layout();
    let input = input.as_slice().ok_or_else(|| invalid("noncontiguous RGB"))?;
    let rows = (grainy_core::parallel::TILE_PIXELS / w).max(1);
    let mut out = vec![0.; rgb.len()];
    out.par_chunks_mut(rows * w * 3).enumerate().for_each(|(band, output)| {
        let first = band * rows;
        let stop = first + output.len() / (w * 3);
        let top = first.saturating_sub(1);
        let bottom = (stop + 1).min(h);
        // Compute each R-G/B-G difference once per tile, including its one-row halo.
        let diff: Vec<[f32; 2]> =
            input[top * w * 3..bottom * w * 3].as_chunks::<3>().0.iter().map(|p| [p[0] - p[1], p[2] - p[1]]).collect();
        for (dy, row) in output.chunks_exact_mut(w * 3).enumerate() {
            let y = first + dy;
            let yy = [y.saturating_sub(1), y, (y + 1).min(h - 1)].map(|v| (v - top) * w);
            for (x, pixel) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let xx = [x.saturating_sub(1), x, (x + 1).min(w - 1)];
                let green = input[(y * w + x) * 3 + 1];
                pixel[1] = green;
                for (channel, c) in [(0, 0), (2, 1)] {
                    let values = std::array::from_fn(|i| diff[yy[i / 3] + xx[i % 3]][c]);
                    pixel[channel] = median9(values) + green;
                }
            }
        }
    });
    Image::from_shape_vec((h, w, 3), out).map_err(|e| invalid(e.to_string()))
}

// Fixed selection network: only the middle value is needed. total_cmp preserves
// the old sort's ordering of signed zero and overflowed differences as well.
fn median9(mut p: [f32; 9]) -> f32 {
    for (a, b) in [
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 1),
        (3, 4),
        (6, 7),
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 3),
        (5, 8),
        (4, 7),
        (3, 6),
        (1, 4),
        (2, 5),
        (4, 7),
        (4, 2),
        (6, 4),
        (4, 2),
    ] {
        if p[a].total_cmp(&p[b]).is_gt() {
            p.swap(a, b);
        }
    }
    p[4]
}

/// Port of rawcolor.decode. Metadata receives raw_info and as-shot temperature/tint.
pub fn decode(
    raw: &mut RawFile,
    metadata: &mut Value,
    max_size: Option<u32>,
    path: Option<&Path>,
) -> Result<CameraSource> {
    let m = raw.metadata();
    if m.num_colors != 3 || !m.color_desc.starts_with("RGB") {
        return Err(invalid("camera RAW correction requires a three-channel RGB sensor"));
    }
    let mut gains = [m.camera_whitebalance[0] as f64, m.camera_whitebalance[1] as f64, m.camera_whitebalance[2] as f64];
    let daylight =
        [m.daylight_whitebalance[0] as f64, m.daylight_whitebalance[1] as f64, m.daylight_whitebalance[2] as f64];
    if gains.iter().any(|v| *v <= 0.) {
        gains = daylight;
    }
    if gains.iter().chain(daylight.iter()).any(|v| *v <= 0. || !v.is_finite()) {
        return Err(invalid("missing camera white balance"));
    }
    let mut info = CameraInfo {
        xyz_to_camera: std::array::from_fn(|i| m.rgb_xyz_matrix[i].map(f64::from)),
        neutral: gains.map(|v| gains[1] / v),
        daylight_neutral: daylight.map(|v| daylight[1] / v),
        ..Default::default()
    };
    for key in ["make", "camera", "format"] {
        if let Some(v) = metadata.get(key) {
            info.metadata.insert(key.into(), v.clone());
        }
    }
    if let Some(path) = path {
        read_dng_calibration(path, &mut info)?;
    }
    let p = default_profile(&info)?;
    let half = max_size.is_some_and(|v| u64::from(m.sizes[2].max(m.sizes[3])) > u64::from(v) * 2);
    let decoded = raw.postprocess(DecodeMode::Camera, half)?;
    let timing = grainy_core::timing::Span::new("load.normalize16");
    let pixels = decoded.mapv(|v| v as f32 / 65535.);
    drop(timing);
    let (_, t, tint) = neutral_white(&p, info.neutral, &info)?;
    let meta = metadata.as_object_mut().ok_or_else(|| invalid("metadata must be a JSON object"))?;
    meta.insert("raw_native".into(), true.into());
    meta.insert("raw_info".into(), serde_json::to_value(&info)?);
    meta.insert("raw_as_shot_kelvin".into(), t.round_ties_even().into());
    meta.insert("raw_as_shot_tint".into(), ((tint * 100.).round_ties_even() / 100.).into());
    CameraSource::new(if half { pixels } else { reduce_false_color(&pixels)? }, info)
}

#[cfg(test)]
mod performance_tests {
    use super::*;

    #[test]
    fn median_network_covers_every_ordering() {
        fn permutations(a: &mut [f32; 9], from: usize, expected: u32) {
            if from == a.len() {
                assert_eq!(median9(*a).to_bits(), expected);
                return;
            }
            for i in from..a.len() {
                a.swap(from, i);
                permutations(a, from + 1, expected);
                a.swap(from, i);
            }
        }
        let mut values = [f32::NEG_INFINITY, -2., -1., -0., 0., 1., 2., 3., f32::INFINITY];
        permutations(&mut values, 0, 0f32.to_bits());
    }

    fn original(rgb: &Image) -> Image {
        let (h, w, _) = rgb.dim();
        if h < 3 || w < 3 {
            return rgb.clone();
        }
        Image::from_shape_fn(rgb.dim(), |(y, x, c)| {
            let green = rgb[[y, x, 1]];
            if c == 1 {
                return green;
            }
            let mut values = [0.; 9];
            for (i, value) in values.iter_mut().enumerate() {
                let yy = (y as isize + i as isize / 3 - 1).clamp(0, h as isize - 1) as usize;
                let xx = (x as isize + i as isize % 3 - 1).clamp(0, w as isize - 1) as usize;
                *value = rgb[[yy, xx, c]] - rgb[[yy, xx, 1]];
            }
            values.sort_by(f32::total_cmp);
            values[4] + green
        })
    }

    #[test]
    fn tiled_false_color_matches_original_bits_and_borders() {
        let values = [-f32::MAX, -1., -0., 0., 0.5, 1., f32::MAX];
        for (h, w) in [(1, 2), (2, 7), (3, 3), (7, 11), (513, 321)] {
            let a =
                Image::from_shape_fn((w, h, 3), |(x, y, c)| values[(x * 13 + y * 7 + c * 3 + x * y) % values.len()])
                    .permuted_axes([1, 0, 2]);
            let expected = original(&a).mapv(f32::to_bits);
            for threads in [1, 2, 4] {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
                let actual = pool.install(|| reduce_false_color(&a)).unwrap();
                assert_eq!(actual.mapv(f32::to_bits), expected, "{h}x{w}, {threads} threads");
            }
        }
    }

    #[test]
    fn pv4_camera_highlight_reconstruction_eliminates_magenta_cast() {
        let neutral = [0.5_f32, 1.0, 0.59];
        // Unclipped midtone pixel is bit-exact untouched.
        let mid = [0.3, 0.5, 0.25];
        assert_eq!(reconstruct_camera_highlight(mid, neutral), mid);

        // Blown cloud where green saturated at 1.0 while red/blue still carry real data ->
        // without reconstruction wb = [1.70, 1.00, 1.61] (strong magenta); after reconstruction
        // green recovers cleanly near 1.67 so wb is neutral white > 1.0!
        let blown_green = [0.85_f32, 1.0, 0.95];
        let rec = reconstruct_camera_highlight(blown_green, neutral);
        let wb = [rec[0] / neutral[0], rec[1] / neutral[1], rec[2] / neutral[2]];
        assert!(wb[1] > 1.55, "reconstructed green WB should lift above sensor clip, got {wb:?}");
        assert!((wb[0] - wb[1]).abs() < 0.10 && (wb[2] - wb[1]).abs() < 0.10, "should be neutral, got {wb:?}");

        // All 3 channels clipped -> neutral white at max WB headroom instead of magenta.
        let all_blown = [1.0_f32, 1.0, 1.0];
        let rec_all = reconstruct_camera_highlight(all_blown, neutral);
        let wb_all = [rec_all[0] / neutral[0], rec_all[1] / neutral[1], rec_all[2] / neutral[2]];
        assert!((wb_all[0] - wb_all[1]).abs() < 1e-4 && (wb_all[2] - wb_all[1]).abs() < 1e-4);
    }
}
