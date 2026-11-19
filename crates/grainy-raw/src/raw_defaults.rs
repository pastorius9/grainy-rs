//! Port of the pure metadata/rule logic in `luma/raw_defaults.py`.
use crate::{dcp, invalid, tiff, Result};
use grainy_core::{settings::normalized, Settings};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

/// Port of raw_defaults.ISO_KEYS (interpolation order is stable).
pub const ISO_KEYS: [&str; 10] = [
    "noise_luma",
    "noise_color",
    "sharpen",
    "sharpen_radius",
    "sharpen_detail",
    "sharpen_mask",
    "clarity",
    "texture",
    "dehaze",
    "blacks",
];
/// Port of raw_defaults.GROUPS (the serialized Korean group names are preserved).
pub const GROUPS: [(&str, &[&str]); 6] = [
    ("RAW 프로파일", &["dcp_profile", "dcp_huesat", "dcp_look", "dcp_tone", "dcp_exposure", "working_space"]),
    (
        "화이트 밸런스",
        &[
            "raw_mode",
            "raw_kelvin",
            "raw_tint",
            "raw_neutral",
            "temperature",
            "tint",
            "kelvin",
            "wb_reference",
            "kelvin_enabled",
        ],
    ),
    ("빛", &["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]),
    (
        "색상·커브",
        &[
            "saturation",
            "vibrance",
            "monochrome",
            "mixer_mode",
            "hsl",
            "grading",
            "grading_balance",
            "calibration",
            "curve",
            "rgb_curves",
            "parametric",
            "camera_matrix",
        ],
    ),
    (
        "디테일",
        &[
            "clarity",
            "texture",
            "dehaze",
            "sharpen",
            "sharpen_radius",
            "sharpen_detail",
            "sharpen_mask",
            "noise_luma",
            "noise_color",
        ],
    ),
    ("효과", &["grain", "vignette"]),
];
const EXTENSIONS: [&str; 15] =
    ["arw", "cr2", "cr3", "nef", "nrw", "raf", "rw2", "orf", "pef", "dng", "srw", "3fr", "iiq", "kdc", "mos"];
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => v.to_string(),
    }
}
fn field(info: &Value, k: &str) -> String {
    info.get(k).map(text).unwrap_or_default()
}
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(v) => Some(if *v { 1. } else { 0. }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
fn normalized_settings(s: &Settings) -> Settings {
    normalized(&s.0)
}
fn by_iso(table: &[(f64, f64)], iso: f64) -> f64 {
    if iso <= table[0].0 {
        return table[0].1;
    }
    for p in table.windows(2) {
        let (lo, a) = p[0];
        let (hi, b) = p[1];
        if iso <= hi {
            return (a + (b - a) * (iso / lo).log2() / (hi / lo).log2()).round_ties_even();
        }
    }
    table[table.len() - 1].1
}
/// Port of raw_defaults.builtin_luma_nr.
pub fn builtin_luma_nr(iso: Option<f64>) -> f64 {
    iso.map_or(0., |v| by_iso(&[(400., 0.), (3200., 12.), (6400., 25.), (32000., 30.)], v))
}
/// Port of raw_defaults.builtin_iso_detail.
pub fn builtin_iso_detail(iso: Option<f64>) -> Value {
    json!({"noise_luma":builtin_luma_nr(iso),"noise_color":iso.map_or(25.,|v|by_iso(&[(6400.,25.),(32000.,50.)],v)),"sharpen":60.})
}
/// Port of raw_defaults.linear_raw; unreadable files return false, as in the oracle.
pub fn linear_raw(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    if !path.extension().is_some_and(|s| s.eq_ignore_ascii_case("dng")) {
        return false;
    }
    let Ok((pages, _)) = tiff::dng(path, &[262], true) else { return false };
    let kinds: Vec<f64> =
        pages.iter().filter_map(|p| p.get(&262)).filter_map(|t| t.numbers().ok()).flatten().copied().collect();
    kinds.contains(&34892.) && !kinds.contains(&32803.)
}
/// Port of raw_defaults.key: NFKC, full Unicode casefold, alphanumeric filtering.
pub fn key(value: &str) -> String {
    dcp::camera_key(&value.nfkc().collect::<String>())
}
/// Port of raw_defaults.camera_key, including sequential manufacturer suffix removal.
pub fn camera_key(info: &Value) -> String {
    let mut make = key(&field(info, "make"));
    for suffix in ["corporation", "company", "incorporated", "inc", "co", "ltd"] {
        if make.ends_with(suffix) {
            make.truncate(make.len() - suffix.len());
        }
    }
    let mut model = key(&field(info, "camera"));
    if !make.is_empty() && model.starts_with(&make) {
        model = model[make.len()..].into();
    }
    if model.is_empty() {
        String::new()
    } else {
        format!("{make}:{model}")
    }
}
/// Port of raw_defaults.serial (whitespace then NUL trimming, in that order).
pub fn serial(info: &Value) -> String {
    field(info, "serial").trim().trim_matches('\0').into()
}
/// Port of raw_defaults.is_raw; a supplied path takes precedence over metadata.
pub fn is_raw(info: &Value, path: Option<&Path>) -> bool {
    let format = if let Some(path) = path.filter(|p| !p.as_os_str().is_empty()) {
        path.extension().and_then(|v| v.to_str()).unwrap_or("").to_lowercase()
    } else {
        field(info, "format").to_lowercase()
    };
    EXTENSIONS.contains(&format.as_str())
}
/// Port of raw_defaults.iso_value, including numeric strings and finite range checks.
pub fn iso_value(info: &Value) -> Option<f64> {
    number(info.get("iso")?).filter(|v| v.is_finite() && *v > 0. && *v <= 1e8)
}
/// Port of raw_defaults.iso_anchors; photos contain info/path/settings objects.
pub fn iso_anchors(photos: &[Value]) -> Result<Vec<Value>> {
    if photos.len() < 2 {
        return Err(invalid("select at least two RAW photos with different ISO"));
    }
    let camera = camera_key(&photos[0]["info"]);
    if camera.is_empty() || photos.iter().any(|p| camera_key(&p["info"]) != camera) {
        return Err(invalid("ISO anchors must use the same camera"));
    }
    let mut anchors: Vec<(f64, Value)> = Vec::new();
    for p in photos {
        let info = &p["info"];
        let iso = iso_value(info).ok_or_else(|| invalid("invalid ISO anchor"))?;
        if !is_raw(info, p["path"].as_str().map(Path::new)) {
            return Err(invalid("ISO anchors must be RAW"));
        }
        let mut values = serde_json::Map::new();
        for k in ISO_KEYS {
            let v =
                number(&p["settings"][k]).filter(|v| v.is_finite()).ok_or_else(|| invalid("invalid ISO adjustment"))?;
            values.insert(k.into(), v.into());
        }
        let values = Value::Object(values);
        if let Some((_, old)) = anchors.iter().find(|(i, _)| *i == iso) {
            if *old != values {
                return Err(invalid("conflicting adjustments at the same ISO"));
            }
        } else {
            anchors.push((iso, values));
        }
    }
    if anchors.len() < 2 {
        return Err(invalid("at least two distinct ISO values required"));
    }
    anchors.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(anchors.into_iter().map(|(iso, values)| json!({"iso":iso,"values":values})).collect())
}
fn json_python_string(s: &str) -> String {
    // json.dumps defaults to ensure_ascii=True. Escape non-BMP scalars as surrogate pairs.
    let quoted = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut out = String::new();
    for c in quoted.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out
}
/// Port of raw_defaults.make_rule. A missing group selection means all groups.
pub fn make_rule(
    info: &Value,
    settings: &Settings,
    scope: &str,
    groups: Option<&[String]>,
    anchors: &[Value],
    name: &str,
) -> Result<Value> {
    if !["master", "model", "serial"].contains(&scope) {
        return Err(invalid("invalid rule scope"));
    }
    let camera = camera_key(info);
    let serial = serial(info);
    if scope != "master" && camera.is_empty() || scope == "serial" && serial.is_empty() {
        return Err(invalid("missing camera model or serial"));
    }
    let groups: Vec<String> =
        groups.map(<[String]>::to_vec).unwrap_or_else(|| GROUPS.iter().map(|(n, _)| (*n).into()).collect());
    if groups.is_empty() || groups.iter().any(|g| !GROUPS.iter().any(|(n, _)| n == g)) {
        return Err(invalid("invalid rule groups"));
    }
    let s = normalized_settings(settings);
    let mut values = serde_json::Map::new();
    for group in &groups {
        if let Some((_, keys)) = GROUPS.iter().find(|(n, _)| n == group) {
            for k in *keys {
                values.insert((*k).into(), s.value(k).clone());
            }
        }
    }
    if values.get("raw_mode").and_then(Value::as_str) == Some("sample") {
        values.insert("raw_mode".into(), "custom".into());
        values.insert("raw_neutral".into(), Value::Null);
    }
    if values.get("dcp_profile").is_some_and(grainy_core::settings::truthy) {
        let p = dcp::compiled(&values["dcp_profile"])?;
        if scope != "master" && !dcp::camera_matches(&p, info) {
            return Err(invalid("DCP camera mismatch"));
        }
        if values.get("raw_mode").and_then(Value::as_str).unwrap_or("legacy") == "legacy" {
            values.insert("raw_mode".into(), "as_shot".into());
        }
    }
    let target_camera = if scope == "master" { "" } else { &camera };
    let target_serial = if scope == "serial" { &serial } else { "" };
    let target = format!(
        "[{}, {}, {}]",
        json_python_string(scope),
        json_python_string(target_camera),
        json_python_string(target_serial)
    );
    let digest = format!("{:x}", Sha256::digest(target.as_bytes()));
    let title = if !name.is_empty() {
        name.into()
    } else if scope == "master" {
        "모든 RAW".into()
    } else {
        format!(
            "{}{}",
            field(info, "camera"),
            if target_serial.is_empty() { String::new() } else { format!(" · {target_serial}") }
        )
    };
    Ok(
        json!({"id":&digest[..24],"scope":scope,"camera":target_camera,"serial":target_serial,"enabled":true,"name":title,"values":values,"groups":groups,"iso":anchors}),
    )
}
fn priority(scope: &str) -> Option<u8> {
    match scope {
        "master" => Some(0),
        "model" => Some(1),
        "serial" => Some(2),
        _ => None,
    }
}
/// Port of raw_defaults.resolve. None base means a new import with built-in development.
pub fn resolve(
    info: &Value,
    rules: &[Value],
    path: Option<&Path>,
    base: Option<&Settings>,
) -> Result<(Settings, Vec<String>)> {
    let mut s = normalized_settings(&base.cloned().unwrap_or_else(|| Settings(Default::default())));
    let mut notes = Vec::new();
    if !is_raw(info, path) {
        return Ok((s, notes));
    }
    if base.is_none() {
        for (k, v) in [
            ("exposure", if info.get("linear_raw").is_some_and(grainy_core::settings::truthy) { 0.75 } else { 1.5 }),
            ("blacks", -30.),
            ("sharpen", 60.),
            ("sharpen_radius", 1.),
            ("sharpen_detail", 25.),
            ("noise_color", 25.),
        ] {
            s.set(k, v);
        }
        s.set("tone_version", 3);
        if let Some(detail) = builtin_iso_detail(iso_value(info)).as_object() {
            for (k, v) in detail {
                s.set(k, v.clone());
            }
        }
        notes.push("RAW 기본 현상".into());
    }
    let camera = camera_key(info);
    let serial = serial(info);
    let mut selected: Option<&Value> = None;
    let mut rank = 0;
    for r in rules {
        let scope = r["scope"].as_str().ok_or_else(|| invalid("rule missing scope"))?;
        let prio = priority(scope).ok_or_else(|| invalid("unknown rule scope"))?;
        let enabled = r.get("enabled").is_none_or(grainy_core::settings::truthy);
        let matches = scope == "master"
            || r["camera"].as_str() == Some(&camera)
                && (scope == "model" || r["serial"].as_str() == Some(&serial) && !serial.is_empty());
        if enabled && matches && (selected.is_none() || prio > rank) {
            selected = Some(r);
            rank = prio;
        }
    }
    let Some(rule) = selected else { return Ok((s, notes)) };
    let values = rule["values"].as_object().ok_or_else(|| invalid("rule values must be an object"))?;
    for (k, v) in values {
        s.set(k, v.clone());
    }
    notes.push(format!("RAW 기본값: {}", field(rule, "name")));
    if s.bool("dcp_profile") && !dcp::camera_matches(&dcp::compiled(s.value("dcp_profile"))?, info) {
        s.set("dcp_profile", Value::Null);
        notes.push("다른 카메라용 DCP는 제외했습니다.".into());
    }
    let mut points: Vec<&Value> =
        rule.get("iso").and_then(Value::as_array).map(|v| v.iter().collect()).unwrap_or_default();
    if !points.is_empty() {
        if let Some(iso) = iso_value(info) {
            for p in &points {
                if number(&p["iso"]).is_none_or(|v| !v.is_finite() || v <= 0.) {
                    return Err(invalid("invalid rule ISO anchor"));
                }
            }
            points.sort_by(|a, b| number(&a["iso"]).unwrap_or(0.).total_cmp(&number(&b["iso"]).unwrap_or(0.)));
            let mut lo = points[0];
            let mut hi = points[points.len() - 1];
            for p in &points {
                let v = number(&p["iso"]).unwrap_or(0.);
                if v <= iso {
                    lo = p;
                }
                if v >= iso {
                    hi = p;
                    break;
                }
            }
            if iso <= number(&points[0]["iso"]).unwrap_or(0.) {
                lo = points[0];
                hi = lo;
            }
            if iso >= number(&points[points.len() - 1]["iso"]).unwrap_or(0.) {
                lo = points[points.len() - 1];
                hi = lo;
            }
            let a = number(&lo["iso"]).unwrap_or(1.);
            let b = number(&hi["iso"]).unwrap_or(1.);
            let f = if a == b { 0. } else { (iso / a).log2() / (b / a).log2() };
            for k in ISO_KEYS {
                let a = number(&lo["values"][k]).ok_or_else(|| invalid("missing ISO adjustment"))?;
                let b = number(&hi["values"][k]).ok_or_else(|| invalid("missing ISO adjustment"))?;
                s.set(k, a * (1. - f) + b * f);
            }
            notes.push(format!("ISO {} 디테일·검정 보정", format_iso(iso)));
        } else {
            notes.push("ISO가 없어 저장된 기본 보정값을 사용했습니다.".into());
        }
    }
    Ok((normalized_settings(&s), notes))
}
fn format_iso(v: f64) -> String {
    let scientific = format!("{v:.5e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else { return v.to_string() };
    let exp = exponent.parse::<i32>().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        format!(
            "{}e{}{:02}",
            mantissa.trim_end_matches('0').trim_end_matches('.'),
            if exp < 0 { "-" } else { "+" },
            exp.unsigned_abs()
        )
    } else {
        let decimals = (5 - exp).max(0) as usize;
        let result = format!("{v:.decimals$}");
        if result.contains('.') {
            result.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            result
        }
    }
}
