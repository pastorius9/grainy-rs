//! Port of `luma/dcp.py`: bounded profile parsing and DNG 1.7.1 colour transforms.
pub use crate::tiff::Tag;
use crate::{
    invalid,
    math::{self, diag, inverse, mm, mv, valid_matrix, Mat, ID},
    tiff, Result,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use grainy_color::illuminants;
use grainy_core::Image;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

/// Port of dcp.MAX_PROFILE_BYTES.
pub const MAX_PROFILE_BYTES: usize = 4 * 1024 * 1024;
/// Port of dcp.D50.
pub use grainy_color::primaries::D50;
const BRADFORD: Mat = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// Port of the colour tables returned by dcp.parse_tags. Storage order is V,H,S,channel.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Table {
    /// dcp.parse_tags table shape, excluding the final three channels.
    pub dims: [usize; 3],
    /// dcp.parse_tags values (float32 exactly represented in f64).
    pub data: Vec<f64>,
    /// dcp.interpolate promotes triple-light tables to float64, as NumPy does.
    #[serde(default)]
    pub double: bool,
}
impl Table {
    fn validate(&self) -> Result<()> {
        let [v, h, s] = self.dims;
        if !(1..=256).contains(&v)
            || !(1..=360).contains(&h)
            || !(2..=256).contains(&s)
            || v * h * s > 250000
            || self.data.len() != v * h * s * 3
            || self.data.iter().any(|x| !x.is_finite())
        {
            return Err(invalid("invalid DCP table dimensions/data"));
        }
        Ok(())
    }
}
/// Port of the complete dictionary returned by dcp.parse/parse_tags.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    /// dcp.parse_tags name.
    pub name: String,
    /// dcp.parse_tags camera model restriction.
    pub camera: String,
    /// dcp.parse_tags copyright notice.
    pub copyright: String,
    /// dcp.parse_tags calibration signature.
    pub signature: String,
    /// dcp.parse_tags embedding policy.
    pub policy: i64,
    /// dcp.parse_tags baseline exposure offset in stops.
    pub exposure: f64,
    /// dcp.parse_tags ColorMatrix1.
    pub cm1: Option<Mat>,
    /// dcp.parse_tags ColorMatrix2.
    pub cm2: Option<Mat>,
    /// dcp.parse_tags ColorMatrix3.
    pub cm3: Option<Mat>,
    /// dcp.parse_tags ForwardMatrix1.
    pub fm1: Option<Mat>,
    /// dcp.parse_tags ForwardMatrix2.
    pub fm2: Option<Mat>,
    /// dcp.parse_tags ForwardMatrix3.
    pub fm3: Option<Mat>,
    /// dcp.parse_tags illuminant 1 xy.
    pub xy1: Option<[f64; 2]>,
    /// dcp.parse_tags illuminant 2 xy.
    pub xy2: Option<[f64; 2]>,
    /// dcp.parse_tags illuminant 3 xy.
    pub xy3: Option<[f64; 2]>,
    /// dcp.parse_tags illuminant 1 temperature.
    pub t1: Option<f64>,
    /// dcp.parse_tags illuminant 2 temperature.
    pub t2: Option<f64>,
    /// dcp.parse_tags illuminant 3 temperature.
    pub t3: Option<f64>,
    /// dcp.parse_tags custom illuminant flag.
    pub custom_illuminants: bool,
    /// dcp.parse_tags hue/saturation table 1.
    pub hs1: Option<Table>,
    /// dcp.parse_tags hue/saturation table 2.
    pub hs2: Option<Table>,
    /// dcp.parse_tags hue/saturation table 3.
    pub hs3: Option<Table>,
    /// dcp.parse_tags look table.
    pub look: Option<Table>,
    /// dcp.parse_tags hue/saturation encoding.
    pub hs_encoding: i32,
    /// dcp.parse_tags look encoding.
    pub look_encoding: i32,
    /// dcp.parse_tags tone curve.
    pub curve: Vec<[f64; 2]>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "DCP".into(),
            camera: String::new(),
            copyright: String::new(),
            signature: String::new(),
            policy: 0,
            exposure: 0.,
            cm1: None,
            cm2: None,
            cm3: None,
            fm1: None,
            fm2: None,
            fm3: None,
            xy1: None,
            xy2: None,
            xy3: None,
            t1: None,
            t2: None,
            t3: None,
            custom_illuminants: false,
            hs1: None,
            hs2: None,
            hs3: None,
            look: None,
            hs_encoding: 0,
            look_encoding: 0,
            curve: vec![[0., 0.], [1., 1.]],
        }
    }
}
/// Port of dcp.read_tags; unknown tags are validated even when filtered out.
pub fn read_tags(data: &[u8], wanted: Option<&[u16]>) -> Result<BTreeMap<u16, Tag>> {
    Ok(tiff::standalone(data, wanted)?.0)
}
/// Port of dcp.parse.
pub fn parse(data: &[u8]) -> Result<Profile> {
    let (tags, le) = tiff::standalone(data, None)?;
    parse_tags(&tags, le)
}
fn number(tags: &BTreeMap<u16, Tag>, tag: u16, default: f64) -> Result<f64> {
    match tags.get(&tag) {
        None => Ok(default),
        Some(v) => {
            let a = v.numbers()?;
            if a.len() != 1 || !a[0].is_finite() {
                Err(invalid("invalid scalar DCP tag"))
            } else {
                Ok(a[0])
            }
        }
    }
}
pub(crate) fn matrix_tag(tags: &BTreeMap<u16, Tag>, tag: u16, bounded: bool) -> Result<Option<Mat>> {
    let Some(v) = tags.get(&tag) else { return Ok(None) };
    let a = v.numbers()?;
    if a.len() != 9 {
        return Err(invalid("DCP matrix is not 3 by 3"));
    }
    Ok(Some(valid_matrix(std::array::from_fn(|i| std::array::from_fn(|j| a[i * 3 + j])), bounded)?))
}
fn table(tags: &BTreeMap<u16, Tag>, dims: u16, data: u16) -> Result<Option<Table>> {
    let Some(a) = tags.get(&data) else { return Ok(None) };
    let d = tags.get(&dims).ok_or_else(|| invalid("missing DCP table dimensions"))?.numbers()?;
    if d.len() != 3 || d.iter().any(|v| *v < 1. || *v != v.floor() || *v > 360.) {
        return Err(invalid("invalid table dimensions"));
    }
    let t = Table {
        dims: [d[2] as usize, d[0] as usize, d[1] as usize],
        data: a.numbers()?.iter().map(|v| (*v as f32) as f64).collect(),
        double: false,
    };
    t.validate()?;
    for (v, i) in t.data.as_chunks::<3>().0.iter().zip(0..) {
        if v[0].abs() > 360.
            || v[1..].iter().any(|v| *v < 0. || *v > 100.)
            || i % t.dims[2] == 0 && (v[2] - 1.).abs() > 2e-5
        {
            return Err(invalid("DCP table values outside bounds"));
        }
    }
    Ok(Some(t))
}
fn temperature(code: i64) -> Option<f64> {
    Some(match code {
        1 | 4 | 9 => 5500.,
        2 => 4200.,
        3 | 17 => 2856.,
        10 => 6500.,
        11 => 7500.,
        12 => 6430.,
        13 => 5000.,
        14 => 4150.,
        15 => 3450.,
        16 => 2940.,
        18 => 4874.,
        19 => 6774.,
        20 => 5503.,
        21 => 6504.,
        22 => 7504.,
        23 => 5003.,
        24 => 3200.,
        _ => return None,
    })
}
fn curve_valid(curve: &[[f64; 2]]) -> Result<()> {
    if !(2..=4096).contains(&curve.len())
        || curve.iter().flatten().any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
        || curve.windows(2).any(|p| p[0][0] >= p[1][0])
    {
        return Err(invalid("invalid DCP tone curve"));
    }
    Ok(())
}
/// Port of dcp.parse_tags, shared with rawcolor.read_dng_calibration.
pub fn parse_tags(tags: &BTreeMap<u16, Tag>, little_endian: bool) -> Result<Profile> {
    if [52525, 52543, 52544].iter().any(|t| tags.contains_key(t)) {
        return Err(invalid("RGB/spatial gain tables are unsupported"));
    }
    if let Some(t) = tags.get(&52551) {
        let b = t.bytes().ok_or_else(|| invalid("invalid output range tag"))?;
        if b.len() != 8 || tiff::u16_at(b, little_endian) != 1 || tiff::u16_at(&b[2..], little_endian) != 0 {
            return Err(invalid("HDR DCP profiles are unsupported"));
        }
    }
    let text = |tag, default: &str| tags.get(&tag).map_or_else(|| default.into(), Tag::text);
    let mut p = Profile {
        name: text(50936, "DCP"),
        camera: text(50708, ""),
        copyright: text(50942, ""),
        signature: text(50932, ""),
        policy: number(tags, 50941, 0.)? as i64,
        exposure: number(tags, 51109, 0.)?,
        cm1: matrix_tag(tags, 50721, true)?,
        cm2: matrix_tag(tags, 50722, true)?,
        cm3: matrix_tag(tags, 52531, true)?,
        fm1: matrix_tag(tags, 50964, true)?,
        fm2: matrix_tag(tags, 50965, true)?,
        fm3: matrix_tag(tags, 52532, true)?,
        ..Profile::default()
    };
    if p.cm1.is_none() || p.exposure.abs() > 10. {
        return Err(invalid("missing ColorMatrix1 or invalid exposure"));
    }
    let triple = p.cm3.is_some();
    if triple && (p.cm2.is_none() || [50778, 50779, 52529].iter().any(|t| !tags.contains_key(t))) {
        return Err(invalid("incomplete triple illuminants"));
    }
    if !triple && [52529, 52532, 52535, 52537, 52538].iter().any(|t| tags.contains_key(t)) {
        return Err(invalid("missing third ColorMatrix"));
    }
    let mut xy = [None; 3];
    let mut ts = [None; 3];
    for (i, tag) in [50778, 50779, 52529].iter().enumerate() {
        let n = number(tags, *tag, 0.)?;
        if n != n.trunc() {
            return Err(invalid("invalid illuminant code"));
        }
        let code = n as i64;
        xy[i] = illuminants::light_xy(code, tags.get(&(52533 + i as u16)).and_then(Tag::bytes), little_endian)?;
        ts[i] = if code == 255 {
            p.custom_illuminants = true;
            Some(illuminants::temperature_tint(xy[i].ok_or_else(|| invalid("missing custom white"))?)?.0)
        } else {
            temperature(code)
        };
    }
    [p.xy1, p.xy2, p.xy3] = xy;
    [p.t1, p.t2, p.t3] = ts;
    if triple {
        if xy.iter().any(Option::is_none) {
            return Err(invalid("unknown triple white point"));
        }
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            if let (Some(a), Some(b)) = (xy[i], xy[j]) {
                if (a[0] - b[0]).abs().max((a[1] - b[1]).abs()) < 1e-12 {
                    return Err(invalid("duplicate triple white points"));
                }
            }
        }
    }
    if !triple && p.cm2.is_some() && (p.t1.is_none() || p.t2.is_none() || p.t1 == p.t2) {
        return Err(invalid("invalid dual illuminant temperatures"));
    }
    if p.cm2.is_some() && (p.fm1.is_none() != p.fm2.is_none())
        || p.cm2.is_none() && (p.fm2.is_some() || tags.contains_key(&50939))
        || triple && (p.fm1.is_none() != p.fm3.is_none())
    {
        return Err(invalid("incomplete ForwardMatrix set"));
    }
    p.hs1 = table(tags, 50937, 50938)?;
    p.hs2 = table(tags, 50937, 50939)?;
    p.hs3 = table(tags, 50937, 52537)?;
    p.look = table(tags, 50981, 50982)?;
    if p.hs2.is_some() && p.hs1.is_none()
        || triple && (p.hs1.is_none() != p.hs2.is_none() || p.hs1.is_none() != p.hs3.is_none())
    {
        return Err(invalid("incomplete hue/saturation tables"));
    }
    p.hs_encoding = number(tags, 51107, 0.)? as i32;
    p.look_encoding = number(tags, 51108, 0.)? as i32;
    if ![0, 1].contains(&p.hs_encoding) || ![0, 1].contains(&p.look_encoding) {
        return Err(invalid("unsupported table encoding"));
    }
    if let Some(v) = tags.get(&50940) {
        let a = v.numbers()?;
        if a.len() % 2 != 0 {
            return Err(invalid("odd tone curve length"));
        }
        p.curve = a.as_chunks::<2>().0.to_vec();
    }
    curve_valid(&p.curve)?;
    Ok(p)
}
/// Port of dcp.load_profile: JSON-safe pinned profile with original bytes and digest.
pub fn load_profile(path: impl AsRef<Path>) -> Result<Value> {
    let path = path.as_ref();
    if std::fs::metadata(path)?.len() > MAX_PROFILE_BYTES as u64 {
        return Err(invalid("DCP exceeds 4 MB"));
    }
    let data = std::fs::read(path)?;
    let p = parse(&data)?;
    Ok(
        serde_json::json!({"name":p.name,"camera":p.camera,"copyright":p.copyright,"sha256":format!("{:x}",Sha256::digest(&data)),"data":STANDARD.encode(&data)}),
    )
}
/// Port of dcp.compiled/_compiled (validated digest; caller may cache the owned Profile).
pub fn compiled(record: &Value) -> Result<Profile> {
    let encoded = record.get("data").and_then(Value::as_str).ok_or_else(|| invalid("invalid stored DCP record"))?;
    if encoded.len() > MAX_PROFILE_BYTES.div_ceil(3) * 4 {
        return Err(invalid("stored DCP too large"));
    }
    let data = STANDARD.decode(encoded).map_err(|_| invalid("corrupt stored DCP data"))?;
    if record.get("sha256").and_then(Value::as_str) != Some(format!("{:x}", Sha256::digest(&data)).as_str()) {
        return Err(invalid("DCP digest mismatch"));
    }
    parse(&data)
}
pub(crate) fn camera_key(s: &str) -> String {
    crate::unicode_data::fold_alnum(s)
}
/// Port of dcp.camera_matches (Unicode casefold, alphanumeric matching).
pub fn camera_matches(profile: &Profile, metadata: &Value) -> bool {
    let camera = camera_key(&profile.camera);
    if camera.is_empty() {
        return true;
    }
    let get = |k| metadata.get(k).and_then(Value::as_str).unwrap_or("");
    [
        camera_key(get("camera")),
        camera_key(&format!("{} {}", get("make"), get("camera"))),
        camera_key(get("unique_camera")),
    ]
    .contains(&camera)
}
/// Port of dcp.weight's scalar or triple-light result.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Weight {
    /// Dual-light weight of the first illuminant.
    Dual(f64),
    /// Three-light barycentric weights.
    Triple([f64; 3]),
}
/// Port of dcp.weight.
pub fn weight(p: &Profile, temperature: f64, white: Option<[f64; 3]>) -> Result<Weight> {
    if !temperature.is_finite() || temperature <= 0. {
        return Err(invalid("invalid temperature"));
    }
    if p.cm3.is_some() {
        let whites = [p.xy1, p.xy2, p.xy3];
        let mut xy = [[0.; 2]; 3];
        for i in 0..3 {
            xy[i] = whites[i].ok_or_else(|| invalid("missing triple white point"))?;
        }
        return Ok(Weight::Triple(illuminants::triple_weights(
            &xy,
            match white {
                Some(w) => w,
                None => crate::rawcolor::temperature_xyz(temperature, 0.)?,
            },
        )?));
    }
    if p.cm2.is_none() {
        return Ok(Weight::Dual(1.));
    }
    let t = if p.custom_illuminants && white.is_some() {
        illuminants::temperature_tint(illuminants::xyz_to_xy(white.unwrap_or(D50))?)?.0
    } else {
        temperature
    };
    let (t1, t2) = (p.t1.ok_or_else(|| invalid("missing t1"))?, p.t2.ok_or_else(|| invalid("missing t2"))?);
    if t1 <= 0. || t2 <= 0. || t1 == t2 {
        return Err(invalid("invalid dual temperatures"));
    }
    Ok(Weight::Dual(((1. / t - 1. / t2) / (1. / t1 - 1. / t2)).clamp(0., 1.)))
}
fn weights(w: Weight) -> [f64; 3] {
    match w {
        Weight::Dual(x) => [x, 1. - x, 0.],
        Weight::Triple(a) => a,
    }
}
/// Port of dcp.interpolate for ColorMatrix/ForwardMatrix.
pub fn interpolate(p: &Profile, name: &str, w: Weight) -> Result<Option<Mat>> {
    let a = match name {
        "cm" => [p.cm1, p.cm2, p.cm3],
        "fm" => [p.fm1, p.fm2, p.fm3],
        _ => return Err(invalid("matrix name must be cm or fm")),
    };
    let Some(first) = a[0] else { return Ok(None) };
    if matches!(w, Weight::Dual(_)) && a[1].is_none() {
        return Ok(Some(first));
    }
    let ws = weights(w);
    let mut out = [[0.; 3]; 3];
    for i in 0..if matches!(w, Weight::Triple(_)) { 3 } else { 2 } {
        let m = a[i].ok_or_else(|| invalid("missing interpolated matrix"))?;
        for y in 0..3 {
            for x in 0..3 {
                out[y][x] += m[y][x] * ws[i];
            }
        }
    }
    Ok(Some(out))
}
/// Port of dcp.interpolate for hue/saturation tables, retaining NumPy promotion rules.
pub fn interpolate_table(p: &Profile, w: Weight) -> Result<Option<Table>> {
    let Some(a) = &p.hs1 else { return Ok(None) };
    if matches!(w, Weight::Dual(_)) && p.hs2.is_none() {
        return Ok(Some(a.clone()));
    }
    let ws = weights(w);
    let tables = [Some(a), p.hs2.as_ref(), p.hs3.as_ref()];
    let double = matches!(w, Weight::Triple(_));
    let mut out = Table { dims: a.dims, data: vec![0.; a.data.len()], double };
    for k in 0..if double { 3 } else { 2 } {
        let t = tables[k].ok_or_else(|| invalid("missing interpolated table"))?;
        t.validate()?;
        if t.dims != a.dims {
            return Err(invalid("table shapes differ"));
        }
        for (i, v) in out.data.iter_mut().enumerate() {
            if double {
                *v += t.data[i] * ws[k];
            } else {
                *v = ((*v as f32) + (t.data[i] as f32) * (ws[k] as f32)) as f64;
            }
        }
    }
    Ok(Some(out))
}
/// Port of dcp.calibration_matrix.
pub fn calibration_matrix(calibration: &[Mat], w: Weight) -> Result<Mat> {
    if calibration.is_empty() {
        return Ok(ID);
    }
    let ws = weights(w);
    let mut out = [[0.; 3]; 3];
    for (i, ws) in ws.iter().enumerate().take(if matches!(w, Weight::Triple(_)) { 3 } else { 2 }) {
        let m = if matches!(w, Weight::Triple(_)) {
            calibration.get(i).copied().unwrap_or(ID)
        } else {
            calibration[i.min(calibration.len() - 1)]
        };
        for y in 0..3 {
            for x in 0..3 {
                out[y][x] += m[y][x] * ws;
            }
        }
    }
    valid_matrix(out, false)
}
/// Port of dcp.adapt_white (Bradford, destination defaults to D50 at call sites).
pub fn adapt_white(source: [f64; 3], destination: [f64; 3]) -> Result<Mat> {
    if source[1] == 0. {
        return Err(invalid("zero white luminance"));
    }
    let src = mv(BRADFORD, source.map(|v| v / source[1]));
    let dst = mv(BRADFORD, destination);
    if src.iter().any(|v| !v.is_finite() || v.abs() < 1e-300) {
        return Err(invalid("invalid adaptation white"));
    }
    Ok(mm(inverse(BRADFORD)?, mm(diag(std::array::from_fn(|i| dst[i] / src[i])), BRADFORD)))
}
/// Port of dcp.camera_to_xyz. Calibration is applied only for matching signatures.
pub fn camera_to_xyz(
    p: &Profile,
    neutral: [f64; 3],
    temperature: f64,
    calibration: Option<&[Mat]>,
    analog: Option<[f64; 3]>,
    signature: &str,
    white: Option<[f64; 3]>,
) -> Result<Mat> {
    let white = if (p.cm3.is_some() || p.custom_illuminants) && white.is_none() {
        let info = crate::rawcolor::CameraInfo {
            calibration: calibration.map(<[Mat]>::to_vec),
            analog,
            calibration_signature: Some(signature.into()),
            ..Default::default()
        };
        Some(crate::rawcolor::neutral_white(p, neutral, &info)?.0)
    } else {
        white
    };
    let w = weight(p, temperature, white)?;
    let cc = if signature == p.signature { calibration_matrix(calibration.unwrap_or(&[]), w)? } else { ID };
    let individual = mm(diag(analog.unwrap_or([1.; 3])), cc);
    let cm = interpolate(p, "cm", w)?.ok_or_else(|| invalid("missing ColorMatrix"))?;
    if let Some(fm) = interpolate(p, "fm", w)? {
        let inv = inverse(individual)?;
        let reference = mv(inv, neutral);
        if reference.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return Err(invalid("invalid camera neutral"));
        }
        let sum = mv(fm, [1.; 3]);
        if sum.contains(&0.) {
            return Err(invalid("invalid ForwardMatrix white"));
        }
        let fm = mm(diag(std::array::from_fn(|i| D50[i] / sum[i])), fm);
        return Ok(mm(mm(fm, diag(reference.map(|v| 1. / v))), inv));
    }
    let inv = inverse(mm(individual, cm))?;
    let white = mv(inv, neutral);
    let a = mm(adapt_white(white, D50)?, inv);
    Ok(a.map(|r| r.map(|v| v / white[1])))
}
pub(crate) fn xyz_prophoto() -> Result<Mat> {
    let p = grainy_color::primaries::PROPHOTO_PRIMARIES;
    inverse(mm(p, diag(mv(inverse(p)?, D50))))
}

// OpenCV float RGB/HSV, including FLT_EPSILON denominators. Grainy-color's
// generic HSV uses different denominators and cannot replace this primitive.
fn rgb_hsv(a: [f32; 3]) -> [f32; 3] {
    let v = a[0].max(a[1]).max(a[2]);
    let min = a[0].min(a[1]).min(a[2]);
    let diff = v - min;
    let s = diff / (v.abs() + f32::EPSILON);
    let scale = 60. / (diff + f32::EPSILON);
    let mut h = if v == a[0] {
        (a[1] - a[2]) * scale
    } else if v == a[1] {
        (a[2] - a[0]) * scale + 120.
    } else {
        (a[0] - a[1]) * scale + 240.
    };
    if h < 0. {
        h += 360.;
    }
    [h, s, v]
}
fn hsv_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    if s == 0. {
        return [v; 3];
    }
    let h = h * (6. / 360.);
    let sector = h.floor() as usize;
    let f = h - h.floor();
    let tab = [v, v * (1. - s), v * (1. - s * f), v * (1. - s * (1. - f))];
    let idx = [[0, 3, 1], [2, 0, 1], [1, 0, 3], [1, 2, 0], [3, 1, 0], [0, 1, 2]][sector % 6];
    idx.map(|i| tab[i])
}
fn lerp_table(t: &Table, v: f32, h: f32, s: f32) -> [f64; 3] {
    let [vd, hd, sd] = t.dims;
    let (v0, h0, s0) = (v.floor() as usize, h.floor() as usize, s.floor() as usize);
    let (vf, hf, sf) = (v - v.floor(), h - h.floor(), s - s.floor());
    let (v1, h1, s1) = ((v0 + 1).min(vd - 1), (h0 + 1) % hd, (s0 + 1).min(sd - 1));
    let h0 = h0 % hd;
    let round = |v: f64| if t.double { v } else { (v as f32) as f64 };
    let mix = |a: f64, b: f64, f: f32| round(round(a * ((1. - f) as f64)) + round(b * (f as f64)));
    std::array::from_fn(|c| {
        let at = |v, h, s| t.data[((v * hd + h) * sd + s) * 3 + c];
        let plane = |v| mix(mix(at(v, h0, s0), at(v, h0, s1), sf), mix(at(v, h1, s0), at(v, h1, s1), sf), hf);
        if vd > 1 {
            mix(plane(v0), plane(v1), vf)
        } else {
            plane(v0)
        }
    })
}
/// Port of dcp.table_map and native_dcp.interpolate (one implementation).
pub fn table_map(rgb: &Image, table: &Table, encoding: i32) -> Result<Image> {
    math::image(rgb)?;
    table.validate()?;
    if ![0, 1].contains(&encoding) {
        return Err(invalid("unsupported encoding"));
    }
    let [vd, hd, sd] = table.dims;
    Ok(math::map(rgb, |a| {
        let hsv = rgb_hsv(a.map(|v| v.max(0.)));
        let h = hsv[0] / 360.;
        let s = hsv[1];
        let v = hsv[2];
        let encoded = if encoding != 0 && vd > 1 { grainy_color::srgb_encode(v) } else { v };
        let values = lerp_table(
            table,
            encoded.clamp(0., 1.) * (vd - 1) as f32,
            h * hd as f32,
            s.clamp(0., 1.) * (sd - 1) as f32,
        );
        let calc = |x: f32, y: f64| {
            if table.double {
                (x as f64 * y).clamp(0., 1.) as f32
            } else {
                (x * y as f32).clamp(0., 1.)
            }
        };
        let mut v = calc(encoded, values[2]);
        if encoding != 0 && vd > 1 {
            v = grainy_color::srgb_decode(v);
        }
        let hue = if table.double {
            ((h * 360.) as f64 + values[0]).rem_euclid(360.) as f32
        } else {
            (h * 360. + values[0] as f32).rem_euclid(360.)
        };
        hsv_rgb(hue, calc(s, values[1]), v)
    }))
}
fn curve_lut(p: &[[f64; 2]]) -> Vec<f32> {
    let n = p.len();
    let h: Vec<_> = p.windows(2).map(|p| p[1][0] - p[0][0]).collect();
    let mut diag = vec![1.; n];
    let mut upper = vec![0.; n - 1];
    let mut lower = vec![0.; n - 1];
    let mut rhs = vec![0.; n];
    for i in 1..n - 1 {
        lower[i - 1] = h[i - 1];
        diag[i] = 2. * (h[i - 1] + h[i]);
        upper[i] = h[i];
        rhs[i] = 6. * ((p[i + 1][1] - p[i][1]) / h[i] - (p[i][1] - p[i - 1][1]) / h[i - 1]);
    }
    for i in 1..n {
        let f = lower[i - 1] / diag[i - 1];
        diag[i] -= f * upper[i - 1];
        rhs[i] -= f * rhs[i - 1];
    }
    let mut second = vec![0.; n];
    second[n - 1] = rhs[n - 1] / diag[n - 1];
    for i in (0..n - 1).rev() {
        second[i] = (rhs[i] - upper[i] * second[i + 1]) / diag[i];
    }
    (0..65536)
        .map(|j| {
            let v = (j as f64 / 65535.).clamp(p[0][0], p[n - 1][0]);
            let i = p.partition_point(|p| p[0] < v).saturating_sub(1).min(n - 2);
            let width = h[i];
            let a = (p[i + 1][0] - v) / width;
            let b = (v - p[i][0]) / width;
            (a * p[i][1]
                + b * p[i + 1][1]
                + ((a * a * a - a) * second[i] + (b * b * b - b) * second[i + 1]) * width * width / 6.)
                .clamp(0., 1.) as f32
        })
        .collect()
}
/// Port of dcp.tone_map, including identity no-op and constant spline extensions.
pub fn tone_map(rgb: &Image, curve: &[[f64; 2]]) -> Result<Image> {
    math::image(rgb)?;
    curve_valid(curve)?;
    if curve[0][0] == 0. && curve[curve.len() - 1][0] == 1. && curve.iter().all(|p| p[0] == p[1]) {
        return Ok(rgb.clone());
    }
    let lut = curve_lut(curve);
    let lookup = |v: f32| {
        let p = v * 65535.;
        let i = (p as usize).min(65534);
        let f = p as f64 - i as f64;
        (lut[i] as f64) * (1. - f) + (lut[i + 1] as f64) * f
    };
    Ok(math::map(rgb, |p| {
        let a = p.map(|v| v.clamp(0., 1.));
        let lo = a[0].min(a[1]).min(a[2]);
        let hi = a[0].max(a[1]).max(a[2]);
        let l = lookup(lo);
        let h = lookup(hi);
        a.map(|v| (l + ((v - lo) / (hi - lo).max(1e-10)) as f64 * (h - l)) as f32)
    }))
}
