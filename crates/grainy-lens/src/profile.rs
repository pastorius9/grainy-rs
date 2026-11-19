use crate::database::{document, parse_lens};
use crate::{invalid, Camera, Database, Lens, LensRecord, Result};
use grainy_core::settings::truthy;
use serde_json::{json, Value};

const CONDITIONS: &str = "센서 크기·초점 거리·조리개·촬영 거리를 확인해 주세요.";
const NO_CAMERA: &str = "카메라를 선택해 주세요.";
const NO_LENS: &str = "사진에 렌즈 이름이 없습니다. 사용한 렌즈를 선택해 주세요.";
const CHECK_LENS: &str = "렌즈 후보를 확인해 주세요.";

/// Port of `optics._key`: decimal-zero, corporate suffix, and punctuation insensitive name key.
pub fn key(text: &str) -> String {
    let lower = text
        .to_lowercase()
        .replace(['ß', 'ẞ'], "ss")
        .replace('ﬀ', "ff")
        .replace('ﬁ', "fi")
        .replace('ﬂ', "fl")
        .replace('ﬃ', "ffi")
        .replace('ﬄ', "ffl")
        .replace(['ﬅ', 'ﬆ'], "st")
        .replace('ǰ', "j")
        .replace('ẖ', "h")
        .replace('ẗ', "t")
        .replace('ẘ', "w")
        .replace('ẙ', "y")
        .replace('ẚ', "a")
        .replace('ſ', "s");
    let mut normalized = String::new();
    let chars: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '.' && i > 0 && chars[i - 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j] == '0' {
                j += 1;
            }
            if j > i + 1 && (j == chars.len() || !chars[j].is_ascii_digit()) {
                i = j;
                continue;
            }
        }
        normalized.push(chars[i]);
        i += 1;
    }
    let mut result = String::new();
    for word in normalized.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if ["corporation", "corp", "inc", "ltd", "limited", "co"].contains(&word) {
            continue;
        }
        result.extend(word.chars().filter(char::is_ascii_alphanumeric));
    }
    result
}

/// Port of `optics._model_key`: remove a normalized maker prefix.
pub fn model_key(model: &str, maker: &str) -> String {
    let m = key(model);
    let p = key(maker);
    m.strip_prefix(&p).unwrap_or(&m).to_owned()
}

/// Port of `optics._saved_lens` (safe XML, exactly one lens).
pub fn saved_lens(xml: &str) -> Result<Lens> {
    if xml.chars().count() > 256000 {
        return Err(invalid("렌즈 보정 데이터가 올바르지 않습니다."));
    }
    let doc = document(xml)?;
    let nodes: Vec<_> = doc.root_element().children().filter(|n| n.has_tag_name("lens")).collect();
    if nodes.len() != 1 {
        return Err(invalid("렌즈 프로파일 하나가 필요합니다."));
    }
    let lens = parse_lens(nodes[0])?;
    if lens.maker.is_empty() || lens.model.is_empty() || lens.mounts.is_empty() {
        return Err(invalid("렌즈 보정 데이터를 읽지 못했습니다."));
    }
    Ok(lens)
}

/// Port of `optics.profile_lens`.
pub fn profile_lens(record: &LensRecord) -> Result<Lens> {
    saved_lens(&record.xml)
}

/// Port of the shooting-condition arguments of `optics.make_profile`.
#[derive(Clone, Debug)]
pub struct Conditions {
    /// Sensor crop factor.
    pub crop: f64,
    /// Focal length in mm.
    pub focal: f64,
    /// Aperture (zero if unknown).
    pub aperture: f64,
    /// Subject distance in meters.
    pub distance: f64,
    /// EXIF sensor orientation 1 through 8.
    pub orientation: i64,
    /// Camera label.
    pub camera: String,
}

impl Default for Conditions {
    /// Port of `optics.make_profile`'s optional defaults; crop/focal must be supplied.
    fn default() -> Self {
        Self { crop: 1., focal: 50., aperture: 0., distance: 1000., orientation: 1, camera: String::new() }
    }
}

/// Port of `optics.make_profile`; retains the oracle's stored library version as provenance.
pub fn make_profile(record: &LensRecord, c: &Conditions) -> Result<Value> {
    if ![c.crop, c.focal, c.aperture, c.distance].iter().all(|v| v.is_finite())
        || !(0.1..=20.).contains(&c.crop)
        || !(0.1..=5000.).contains(&c.focal)
        || !(0.0..=128.).contains(&c.aperture)
        || !(0.01..=100000.).contains(&c.distance)
    {
        return Err(invalid(CONDITIONS));
    }
    let lens = profile_lens(record)?;
    if lens.min_focal != 0. && !(f64::from(lens.min_focal) - 0.01..=f64::from(lens.max_focal) + 0.01).contains(&c.focal)
    {
        return Err(invalid("초점 거리가 선택한 렌즈 범위를 벗어났습니다."));
    }
    if !(1..=8).contains(&c.orientation) {
        return Err(invalid("사진 방향 정보가 올바르지 않습니다."));
    }
    let mut caps = Vec::new();
    if lens.interpolate_distortion(c.focal as f32).is_some() {
        caps.push("distortion");
    }
    if lens.interpolate_tca(c.focal as f32).is_some() {
        caps.push("tca");
    }
    if c.aperture > 0. && lens.interpolate_vignetting(c.focal as f32, c.aperture as f32, c.distance as f32).is_some() {
        caps.push("vignette");
    }
    if caps.is_empty() {
        return Err(invalid("이 촬영 조건에서 사용할 보정 데이터가 없습니다."));
    }
    Ok(
        json!({"xml":record.xml,"name":record.model,"maker":record.maker,"camera":c.camera,"crop":c.crop,"focal":c.focal,"aperture":c.aperture,"distance":c.distance,"orientation":c.orientation,"capabilities":caps,"database_file":record.file,"source":"Lensfun","license":"CC BY-SA 3.0","library_version":[0,3,4,0]}),
    )
}

/// Port of `optics.match_metadata`'s `(camera, candidates, reason)`.
#[derive(Debug)]
pub struct Match<'a> {
    /// Unambiguous camera, or none.
    pub camera: Option<&'a Camera>,
    /// Compatible lenses with an exact normalized model key.
    pub candidates: Vec<&'a LensRecord>,
    /// Oracle UI explanation; empty on success.
    pub reason: String,
}

fn string<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
fn present(v: &Value, k: &str) -> bool {
    v.get(k).is_some_and(truthy)
}
fn numeric(v: &Value, k: &str, default: f64) -> Result<f64> {
    let Some(value) = v.get(k).filter(|v| truthy(v)) else { return Ok(default) };
    if let Some(n) = value.as_f64() {
        return Ok(n);
    };
    if let Some(s) = value.as_str() {
        return s.parse().map_err(|_| invalid(&format!("could not convert string to float: '{s}'")));
    }
    if let Some(b) = value.as_bool() {
        return Ok(f64::from(b));
    }
    Err(invalid(CONDITIONS))
}

impl Database {
    /// Port of `optics.match_metadata`; the oracle uses exact normalized keys, not fuzzy EXIF matching.
    pub fn match_metadata(&self, metadata: &Value) -> Match<'_> {
        let maker = string(metadata, "make");
        let model = string(metadata, "camera");
        let cameras: Vec<_> = self
            .cameras()
            .iter()
            .filter(|c| {
                !maker.is_empty()
                    && !model.is_empty()
                    && key(&c.maker) == key(maker)
                    && model_key(&c.model, &c.maker) == model_key(model, maker)
            })
            .collect();
        let camera = if cameras.len() == 1 { Some(cameras[0]) } else { None };
        let name = string(metadata, "lens");
        let mut candidates = Vec::new();
        if let Some(c) = camera {
            if !name.is_empty() {
                candidates = self
                    .compatible_lenses(Some(c))
                    .into_iter()
                    .filter(|r| model_key(&r.model, &r.maker) == model_key(name, &r.maker))
                    .collect();
            }
            if candidates.len() > 1 && candidates.iter().all(|r| key(&r.model) == key(&candidates[0].model)) {
                candidates.sort_by(|a, b| {
                    (a.crop - f64::from(c.crop_factor)).abs().total_cmp(&(b.crop - f64::from(c.crop_factor)).abs())
                });
                if (candidates[0].crop - f64::from(c.crop_factor)).abs() + 0.001
                    < (candidates[1].crop - f64::from(c.crop_factor)).abs()
                {
                    candidates.truncate(1);
                }
            }
        }
        let reason = if camera.is_none() {
            NO_CAMERA
        } else if name.is_empty() {
            NO_LENS
        } else if candidates.len() != 1 {
            CHECK_LENS
        } else {
            ""
        };
        Match { camera, candidates, reason: reason.into() }
    }

    /// Port of `optics.automatic_profile` (same profile dictionary and UI reason).
    pub fn automatic_profile(&self, metadata: &Value) -> (Option<Value>, String) {
        let m = self.match_metadata(metadata);
        if !m.reason.is_empty() {
            return (None, m.reason);
        }
        if !present(metadata, "focal_length") {
            return (None, "초점 거리 정보가 없습니다. 촬영 조건을 입력해 주세요.".into());
        }
        let result = (|| {
            let c = m.camera.ok_or_else(|| invalid(NO_CAMERA))?;
            let r = m.candidates.first().ok_or_else(|| invalid(CHECK_LENS))?;
            make_profile(
                r,
                &Conditions {
                    crop: f64::from(c.crop_factor),
                    focal: numeric(metadata, "focal_length", 0.)?,
                    aperture: numeric(metadata, "aperture", 0.)?,
                    distance: numeric(metadata, "subject_distance", 1000.)?,
                    orientation: numeric(metadata, "source_orientation", 1.)? as i64,
                    camera: c.model.clone(),
                },
            )
        })();
        match result {
            Ok(v) => (Some(v), String::new()),
            Err(e) => (None, e.to_string()),
        }
    }

    /// Port of `optics.retarget`: copy conditions to another generated photo's metadata.
    pub fn retarget(&self, settings: &Value, metadata: &Value) -> Result<Value> {
        let mut result = settings.clone();
        if !settings.is_object() {
            return Err(invalid("settings must be an object"));
        }
        let Some(data) = settings.get("lensfun").filter(|v| truthy(v)) else { return Ok(result) };
        let attempt = (|| {
            let camera = self.match_metadata(metadata).camera;
            let name = string(metadata, "lens");
            if !name.is_empty()
                && model_key(name, string(data, "maker")) != model_key(string(data, "name"), string(data, "maker"))
            {
                let (profile, reason) = self.automatic_profile(metadata);
                return profile
                    .ok_or_else(|| invalid(&format!("대상 렌즈가 달라 프로파일을 적용하지 않았습니다. {reason}")));
            }
            let mut crop = numeric(data, "crop", f64::NAN)?;
            if let Some(camera) = camera {
                if !self.compatible_lenses(Some(camera)).iter().any(|r| key(&r.model) == key(string(data, "name"))) {
                    return Err(invalid("대상 카메라와 렌즈의 호환 정보를 확인해 주세요."));
                }
                crop = f64::from(camera.crop_factor);
            } else if present(metadata, "camera")
                && present(data, "camera")
                && key(string(metadata, "camera")) != key(string(data, "camera"))
            {
                return Err(invalid("대상 카메라의 센서 크기를 확인해 주세요."));
            }
            let record = LensRecord {
                id: String::new(),
                maker: string(data, "maker").into(),
                model: string(data, "name").into(),
                xml: string(data, "xml").into(),
                file: string(data, "database_file").into(),
                crop,
                mounts: Vec::new(),
            };
            let mut profile = make_profile(
                &record,
                &Conditions {
                    crop,
                    focal: numeric(metadata, "focal_length", numeric(data, "focal", f64::NAN)?)?,
                    aperture: numeric(
                        metadata,
                        "aperture",
                        if present(metadata, "camera") { 0. } else { numeric(data, "aperture", f64::NAN)? },
                    )?,
                    distance: numeric(metadata, "subject_distance", 1000.)?,
                    orientation: numeric(metadata, "source_orientation", 1.)? as i64,
                    camera: camera
                        .map(|c| c.model.as_str())
                        .unwrap_or_else(|| {
                            if present(metadata, "camera") {
                                string(metadata, "camera")
                            } else {
                                string(data, "camera")
                            }
                        })
                        .into(),
                },
            )?;
            if !present(metadata, "focal_length") {
                profile["note"] = json!("초점 거리 정보가 없어 복사한 촬영 조건을 사용합니다.");
            }
            Ok(profile)
        })();
        match attempt {
            Ok(p) => result["lensfun"] = p,
            Err(e) => {
                result["lensfun_enabled"] = json!(false);
                if result["lensfun"].is_object() {
                    result["lensfun"]["note"] = json!(e.to_string());
                }
            }
        }
        Ok(result)
    }
}

fn general(value: f64, precision: usize) -> String {
    if value == 0. {
        return "0".into();
    }
    // Python's general format selects notation using the exponent after rounding.
    let scientific = format!("{:.*e}", precision.saturating_sub(1), value);
    let exponent = scientific.split_once('e').and_then(|(_, e)| e.parse::<i32>().ok()).unwrap_or(0);
    if exponent < -4 || exponent >= precision as i32 {
        let s = scientific;
        if let Some((mantissa, exp)) = s.split_once('e') {
            let e = exp.parse::<i32>().unwrap_or(0);
            return format!("{}e{:+03}", mantissa.trim_end_matches('0').trim_end_matches('.'), e);
        }
        s
    } else {
        let places = (precision as i32 - 1 - exponent).max(0) as usize;
        let s = format!("{value:.places$}");
        if places > 0 {
            s.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            s
        }
    }
}

/// Port of `optics.description`, including Korean labels and Python's general numeric formatting.
pub fn description(data: &Value) -> Result<String> {
    if !truthy(data) {
        return Ok("자동 렌즈 프로파일 없음".into());
    }
    let name = data.get("name").and_then(Value::as_str).ok_or_else(|| invalid("profile has no name"))?;
    let names = data
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| match v.as_str() {
            Some("distortion") => Some("왜곡"),
            Some("tca") => Some("색수차"),
            Some("vignette") => Some("주변광량"),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let focal = numeric(data, "focal", f64::NAN)?;
    let aperture = numeric(data, "aperture", 0.)?;
    let crop = numeric(data, "crop", f64::NAN)?;
    if ![focal, aperture, crop].iter().all(|v| v.is_finite()) {
        return Err(invalid(CONDITIONS));
    }
    let mut text = format!(
        "{name}\n{}mm · f/{} · 센서 배율 {}\n{names}",
        general(focal, 6),
        general(aperture, 6),
        general(crop, 3)
    );
    if aperture == 0. {
        text.push_str("\n조리개 정보 없음: 주변광량 보정 제외");
    }
    if present(data, "note") {
        text.push('\n');
        text.push_str(string(data, "note"));
    }
    Ok(text)
}
