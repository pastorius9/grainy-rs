//! Port of `luma/adobe_develop.py`: the explicit, reported translation of Adobe develop values to Grainy
//! settings. It translates intent and values, not Adobe's rendering; fields it cannot translate are reported.
//!
//! XML uses `roxmltree` (MIT/Apache-2.0), a small pure-Rust parser. Like the oracle, a document that declares a
//! `DOCTYPE` or `ENTITY` is refused before parsing.

use std::collections::BTreeSet;
use std::path::Path;

use grainy_core::{normalized, Settings};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::lua::{parse, sequence, Key, Limits, Lua, Table};
use crate::pyutil::{casefold, parse_float};

/// `adobe_develop.VERSION`.
pub const VERSION: i64 = 1;
const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const LIMIT: usize = 16 * 1024 * 1024;

/// `adobe_develop.SCALARS`: Adobe key, Grainy key, lowest and highest accepted value.
pub const SCALARS: [(&str, &str, f64, f64); 26] = [
    ("Exposure2012", "exposure", -5.0, 5.0),
    ("Contrast2012", "contrast", -100.0, 100.0),
    ("Highlights2012", "highlights", -100.0, 100.0),
    ("Shadows2012", "shadows", -100.0, 100.0),
    ("Whites2012", "whites", -100.0, 100.0),
    ("Blacks2012", "blacks", -100.0, 100.0),
    ("Clarity2012", "clarity", -100.0, 100.0),
    ("Texture", "texture", -100.0, 100.0),
    ("Dehaze", "dehaze", -100.0, 100.0),
    ("Saturation", "saturation", -100.0, 100.0),
    ("Vibrance", "vibrance", -100.0, 100.0),
    ("LuminanceSmoothing", "noise_luma", 0.0, 100.0),
    ("ColorNoiseReduction", "noise_color", 0.0, 100.0),
    ("Sharpness", "sharpen", 0.0, 150.0),
    ("SharpenRadius", "sharpen_radius", 0.2, 5.0),
    ("SharpenDetail", "sharpen_detail", 0.0, 100.0),
    ("SharpenEdgeMasking", "sharpen_mask", 0.0, 100.0),
    ("GrainAmount", "grain", 0.0, 100.0),
    ("IncrementalTemperature", "temperature", -100.0, 100.0),
    ("IncrementalTint", "tint", -100.0, 100.0),
    ("GrainSize", "grain_size", 0.0, 100.0),
    ("GrainFrequency", "grain_roughness", 0.0, 100.0),
    ("PostCropVignetteMidpoint", "vignette_midpoint", 0.0, 100.0),
    ("PostCropVignetteRoundness", "vignette_roundness", -100.0, 100.0),
    ("PostCropVignetteFeather", "vignette_feather", 0.0, 100.0),
    ("PostCropVignetteHighlightContrast", "vignette_highlights", 0.0, 100.0),
];

/// `adobe_develop.INFORMATION`: fields that carry no edit and are never reported as unsupported.
pub const INFORMATION: [&str; 18] = [
    "Version",
    "ProcessVersion",
    "HasSettings",
    "RawFileName",
    "UUID",
    "SupportsAmount",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "PresetType",
    "Cluster",
    "Name",
    "Group",
    "Description",
    "SortName",
];

/// `adobe_develop.COLORS`.
pub const COLORS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

const UNSUPPORTED: &str = "현재 변환하지 않는 Adobe 보정 항목입니다.";

/// An insertion-ordered map of key to reason (Python `dict[str, str]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Omitted(pub Vec<(String, String)>);

impl Omitted {
    /// `omitted[key] = reason` (an existing key keeps its position).
    pub fn set(&mut self, key: &str, reason: &str) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = reason.to_owned(),
            None => self.0.push((key.to_owned(), reason.to_owned())),
        }
    }

    /// The reason of a key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The result dict of `convert` (and of `adobe_preset.translate`).
#[derive(Clone, Debug, PartialEq)]
pub struct Converted {
    /// The Grainy settings (normalized, with the translated values applied).
    pub settings: Settings,
    /// Adobe keys that were translated, sorted.
    pub mapped: Vec<String>,
    /// Adobe keys that were not, with the reason.
    pub omitted: Omitted,
}

/// Formats a bound as Python prints the oracle's integer or `.2` literals.
fn bound(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// Port of `adobe_develop.number`: a finite number within `[low, high]`, as `f64`.
pub fn number(value: &Lua, low: f64, high: f64) -> Result<f64> {
    let result = match value {
        Lua::Bool(_) => return Err(Error::value("숫자 대신 논리값이 저장되어 있습니다.")),
        Lua::Int(i) => *i as f64,
        Lua::Huge(f) | Lua::Float(f) => {
            if f.is_infinite() && matches!(value, Lua::Huge(_)) {
                return Err(Error::value("숫자 형식이 아닙니다."));
            }
            *f
        }
        Lua::Str(s) => match parse_float(s) {
            Some(f) => f,
            None => return Err(Error::value("숫자 형식이 아닙니다.")),
        },
        _ => return Err(Error::value("숫자 형식이 아닙니다.")),
    };
    if !result.is_finite() || !(low <= result && result <= high) {
        return Err(Error::value(format!("지원 범위 {}–{} 밖의 값입니다.", bound(low), bound(high))));
    }
    Ok(result)
}

/// Port of `adobe_develop.boolean`.
pub fn boolean(value: &Lua) -> Result<bool> {
    match value {
        Lua::Bool(b) => Ok(*b),
        Lua::Str(s) => match casefold(s).as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(Error::value("논리값 형식이 아닙니다.")),
        },
        _ => Err(Error::value("논리값 형식이 아닙니다.")),
    }
}

/// The points of a curve as it is being decoded: either a list of items or a Python `str`/`list` pair source.
enum Pair {
    Items(Vec<Lua>),
    Text(Vec<char>),
    Other(String),
}

/// Port of `adobe_develop.curve`: points as `[x, y]` pairs in 0..1, ascending in x.
pub fn curve(value: &Lua) -> Result<Vec<[f64; 2]>> {
    let list: Vec<Lua> = match value {
        Lua::Table(_) => sequence(value)?,
        Lua::List(l) => l.clone(),
        _ => return Err(Error::value("커브 점 배열이 아닙니다.")),
    };
    let points: Vec<Pair> = match list.first() {
        Some(Lua::Str(_)) => {
            let mut out = Vec::new();
            for v in &list {
                match v {
                    Lua::Str(s) => out.push(Pair::Items(s.split(',').map(|p| Lua::Str(p.to_owned())).collect())),
                    other => {
                        return Err(Error::attribute(format!(
                            "'{}' object has no attribute 'split'",
                            other.type_name()
                        )))
                    }
                }
            }
            out
        }
        Some(Lua::Table(_)) | Some(Lua::List(_)) => {
            let mut out = Vec::new();
            for v in &list {
                out.push(match v {
                    Lua::Table(_) => Pair::Items(sequence(v)?),
                    Lua::List(l) => Pair::Items(l.clone()),
                    Lua::Str(s) => Pair::Text(s.chars().collect()),
                    other => Pair::Other(other.type_name().to_owned()),
                });
            }
            out
        }
        _ => {
            if list.len() % 2 == 1 {
                return Err(Error::value("커브 좌표가 짝을 이루지 않습니다."));
            }
            list.chunks(2).map(|c| Pair::Items(c.to_vec())).collect()
        }
    };
    if !(2..=256).contains(&points.len()) {
        return Err(Error::value("커브 점의 수가 올바르지 않습니다."));
    }
    let mut pairs: Vec<[Lua; 2]> = Vec::new();
    for p in &points {
        let len = match p {
            Pair::Items(items) => items.len(),
            Pair::Text(chars) => chars.len(),
            Pair::Other(name) => return Err(Error::type_error(format!("object of type '{name}' has no len()"))),
        };
        if len != 2 {
            return Err(Error::value("커브 점의 수가 올바르지 않습니다."));
        }
        pairs.push(match p {
            Pair::Items(items) => [items[0].clone(), items[1].clone()],
            Pair::Text(chars) => [Lua::Str(chars[0].to_string()), Lua::Str(chars[1].to_string())],
            Pair::Other(_) => unreachable!("rejected above"),
        });
    }
    let mut result = Vec::with_capacity(pairs.len());
    for [x, y] in &pairs {
        result.push([number(x, 0.0, 255.0)? / 255.0, number(y, 0.0, 255.0)? / 255.0]);
    }
    if result.windows(2).any(|w| w[0][0] >= w[1][0]) {
        return Err(Error::value("커브 입력 좌표가 오름차순이 아닙니다."));
    }
    Ok(result)
}

/// Port of `adobe_develop.from_lua`: the develop table of a `.lrtemplate` or a saved catalog table.
pub fn from_lua(text: &str) -> Result<Lua> {
    let values = parse(text, Limits::default())?;
    if let Some(Lua::Table(value)) = values.get_str("value") {
        if let Some(settings @ Lua::Table(_)) = value.get_str("settings") {
            return Ok(settings.clone());
        }
    }
    if let Some(settings @ Lua::Table(_)) = values.get_str("settings") {
        return Ok(settings.clone());
    }
    Ok(Lua::Table(values))
}

/// Port of `adobe_develop.from_xmp`: reads an XMP file (at most 16 MiB).
pub fn from_xmp(path: &Path) -> Result<Lua> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| Error::io(e.to_string()))?;
    let mut data = Vec::new();
    file.take(LIMIT as u64 + 1).read_to_end(&mut data).map_err(|e| Error::io(e.to_string()))?;
    from_xmp_bytes(&data)
}

fn decode_xmp(data: &[u8]) -> Result<String> {
    if data.len() > LIMIT {
        return Err(Error::data("XMP 파일이 지원 크기를 넘었습니다."));
    }
    if data.starts_with(&[0xff, 0xfe]) || data.starts_with(&[0xfe, 0xff]) {
        let little = data[0] == 0xff;
        let units: Vec<u16> = data[2..]
            .chunks(2)
            .map(|c| {
                if c.len() == 2 {
                    Some(if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
                } else {
                    None
                }
            })
            .collect::<Option<Vec<u16>>>()
            .ok_or_else(|| Error::value("utf-16 codec can't decode byte: truncated data"))?;
        return String::from_utf16(&units).map_err(|e| Error::value(e.to_string()));
    }
    let body = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    String::from_utf8(body.to_vec()).map_err(|e| Error::value(format!("'utf-8' codec can't decode: {e}")))
}

fn has_doctype(text: &str) -> bool {
    // re.search(r'<!\s*(DOCTYPE|ENTITY)\b', text, re.I)
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '<' && chars[i + 1] == '!' {
            let mut j = i + 2;
            while j < chars.len() && crate::pyutil::is_space(chars[j]) {
                j += 1;
            }
            for word in ["DOCTYPE", "ENTITY"] {
                let w: Vec<char> = word.chars().collect();
                if j + w.len() <= chars.len()
                    && chars[j..j + w.len()].iter().zip(&w).all(|(a, b)| a.to_ascii_uppercase() == *b)
                {
                    let after = chars.get(j + w.len());
                    if !after.is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

/// Port of `adobe_develop.xmp_root`'s checks: size, encoding, no `DOCTYPE`/`ENTITY`, well-formed XML. Returns
/// the decoded text (the parsed document is built by the caller from it).
fn xmp_text(data: &[u8]) -> Result<String> {
    let text = decode_xmp(data)?;
    if has_doctype(&text) {
        return Err(Error::data("외부 정의가 포함된 XMP는 읽지 않습니다."));
    }
    Ok(text)
}

/// Port of `adobe_develop.xmp_root`: checks that the data is a readable XMP document without external
/// definitions.
pub fn xmp_root(data: &[u8]) -> Result<()> {
    let text = xmp_text(data)?;
    roxmltree::Document::parse(&text).map(|_| ()).map_err(|_| Error::data("XMP 문서 구문을 읽지 못했습니다."))
}

fn child_text(node: roxmltree::Node) -> String {
    // ElementTree's `.text`: the text before the first child element (comments and instructions dropped).
    let mut out = String::new();
    for c in node.children() {
        if c.is_element() {
            break;
        }
        if c.is_text() {
            out.push_str(c.text().unwrap_or(""));
        }
    }
    out
}

fn tail_text(node: roxmltree::Node) -> String {
    let mut out = String::new();
    let mut next = node.next_sibling();
    while let Some(n) = next {
        if n.is_element() {
            break;
        }
        if n.is_text() {
            out.push_str(n.text().unwrap_or(""));
        }
        next = n.next_sibling();
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn serialize(node: roxmltree::Node, out: &mut String) {
    // A stable text for the element and its tail, like `ET.tostring(child)`; used to tell different complex
    // values apart (the namespace prefixes are not ElementTree's `ns0` naming).
    let name = node.tag_name();
    out.push('<');
    out.push('{');
    out.push_str(name.namespace().unwrap_or(""));
    out.push('}');
    out.push_str(name.name());
    for a in node.attributes() {
        out.push_str(&format!(" {{{}}}{}=\"{}\"", a.namespace().unwrap_or(""), a.name(), escape(a.value())));
    }
    out.push('>');
    for c in node.children() {
        if c.is_element() {
            serialize(c, out);
        } else if c.is_text() {
            out.push_str(&escape(c.text().unwrap_or("")));
        }
    }
    out.push_str("</>");
    out.push_str(&escape(&tail_text(node)));
}

fn in_ns(node: roxmltree::Node, namespace: &str, name: Option<&str>) -> bool {
    node.is_element()
        && node.tag_name().namespace() == Some(namespace)
        && name.is_none_or(|n| node.tag_name().name() == n)
}

/// Port of `adobe_develop.from_xmp_bytes`: the `crs:` fields of an XMP packet (attributes and child elements).
/// A complex field is kept as `{'xml': text}`, never as a misleading empty string.
pub fn from_xmp_bytes(data: &[u8]) -> Result<Lua> {
    let text = xmp_text(data)?;
    let doc = roxmltree::Document::parse(&text).map_err(|_| Error::data("XMP 문서 구문을 읽지 못했습니다."))?;
    let mut values = Table::new();
    let mut put = |key: &str, value: Lua| -> Result<()> {
        match values.get_str(key) {
            Some(old) if *old != value => Err(Error::data(format!("서로 다른 값이 중복된 XMP 항목입니다: {key}"))),
            Some(_) => Ok(()),
            None => {
                values.set(Key::Str(key.to_owned()), value);
                Ok(())
            }
        }
    };
    for desc in doc.root().descendants().filter(|n| in_ns(*n, RDF, Some("Description"))) {
        for attribute in desc.attributes() {
            if attribute.namespace() == Some(CRS) {
                put(attribute.name(), Lua::Str(attribute.value().to_owned()))?;
            }
        }
        for child in desc.children().filter(|c| c.is_element()) {
            if child.tag_name().namespace() != Some(CRS) {
                continue;
            }
            let key = child.tag_name().name();
            let seq = child.children().find(|c| in_ns(*c, RDF, Some("Seq")));
            if let Some(seq) = seq {
                let items =
                    seq.children().filter(|c| in_ns(*c, RDF, Some("li"))).map(|li| Lua::Str(child_text(li))).collect();
                put(key, Lua::List(items))?;
            } else if child.children().any(|c| c.is_element()) || child.attributes().count() > 0 {
                let mut xml = String::new();
                serialize(child, &mut xml);
                put(key, Lua::table_of(vec![("xml", Lua::Str(xml))]))?;
            } else {
                put(key, Lua::Str(child_text(child)))?;
            }
        }
    }
    if values.is_empty() {
        return Err(Error::data("Adobe 보정 항목이 없는 XMP입니다."));
    }
    Ok(Lua::Table(values))
}

fn shape_error(what: &str) -> Error {
    Error::type_error(format!("'{what}' object is not subscriptable"))
}

/// Sets `settings[key][i]` (and `[i][j]`): the assignments of the HSL, calibration, curve, parametric and
/// grading controls. A missing or malformed container reports as Python's `IndexError` / `TypeError`.
fn set_item(settings: &mut Map<String, Value>, key: &str, path: &[usize], value: Value) -> Result<()> {
    let mut slot = settings.get_mut(key).ok_or_else(|| Error::key(key))?;
    for (n, &index) in path.iter().enumerate() {
        match slot {
            Value::Array(items) => {
                let last = n + 1 == path.len();
                match items.get_mut(index) {
                    Some(next) => slot = next,
                    None => {
                        return Err(Error::index(if last {
                            "list assignment index out of range"
                        } else {
                            "list index out of range"
                        }))
                    }
                }
            }
            other => return Err(shape_error(type_of_json(other))),
        }
    }
    *slot = value;
    Ok(())
}

fn type_of_json(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn float(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

fn curve_value(points: Vec<[f64; 2]>) -> Value {
    Value::Array(points.into_iter().map(|[x, y]| Value::Array(vec![float(x), float(y)])).collect())
}

/// Whether a value stringifies to one of the default split positions (`str(values[k]) in (str(v), str(float(v)))`).
fn is_default_split(value: &Lua, default: i128) -> bool {
    match value {
        Lua::Str(s) => *s == default.to_string() || *s == format!("{default}.0"),
        Lua::Int(i) => *i == default,
        Lua::Float(f) => *f == default as f64,
        _ => false,
    }
}

/// `str(values.get('WhiteBalance', ''))` as far as the comparison with `asshot` can tell.
fn white_balance_text(value: Option<&Lua>) -> String {
    match value {
        None => String::new(),
        Some(Lua::Str(s)) => s.clone(),
        Some(Lua::Int(i)) => i.to_string(),
        Some(Lua::Float(f)) => crate::pyutil::float_repr(*f),
        Some(Lua::Bool(true)) => "True".into(),
        Some(Lua::Bool(false)) => "False".into(),
        Some(Lua::Nil) => "None".into(),
        Some(_) => "<container>".into(),
    }
}

/// The orientation values the oracle treats as unrotated: `None`, `'AB'`, `1`, `'1'` (and what equals 1).
fn upright(orientation: Option<&Lua>) -> bool {
    match orientation {
        None | Some(Lua::Nil) => true,
        Some(Lua::Str(s)) => s == "AB" || s == "1",
        Some(Lua::Int(i)) => *i == 1,
        Some(Lua::Float(f)) => *f == 1.0,
        Some(Lua::Bool(b)) => *b,
        Some(_) => false,
    }
}

struct Run<'a> {
    values: &'a Table,
    settings: Map<String, Value>,
    mapped: Vec<String>,
    omitted: Omitted,
    used: BTreeSet<String>,
}

impl Run<'_> {
    /// The oracle's inner `apply`: runs `action` on the value of `key` when present; the exceptions it catches
    /// (`ValueError`, `TypeError`, `KeyError`, `IndexError`) become an omitted entry, anything else propagates.
    fn apply(&mut self, key: &str, action: impl FnOnce(&mut Map<String, Value>, &Lua) -> Result<()>) -> Result<()> {
        let Some(value) = self.values.get_str(key) else {
            return Ok(());
        };
        self.used.insert(key.to_owned());
        match action(&mut self.settings, value) {
            Ok(()) => self.mapped.push(key.to_owned()),
            Err(e)
                if matches!(
                    e.kind,
                    crate::error::ErrorKind::Value
                        | crate::error::ErrorKind::Data
                        | crate::error::ErrorKind::Type
                        | crate::error::ErrorKind::Key
                        | crate::error::ErrorKind::Index
                ) =>
            {
                self.omitted.set(key, &e.message)
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    fn use_all(&mut self, keys: &[&str]) {
        self.used.extend(keys.iter().map(|k| (*k).to_owned()));
    }
}

fn scalar(settings: &mut Map<String, Value>, key: &str, value: &Lua, low: f64, high: f64) -> Result<()> {
    settings.insert(key.to_owned(), float(number(value, low, high)?));
    Ok(())
}

/// Port of `adobe_develop.convert(values, *, base=None, is_raw=False, orientation=None)`.
///
/// `values` is the develop table (from [`from_lua`] or [`from_xmp_bytes`]); `base` the settings the values are
/// applied to (normalized first); `orientation` the photo's orientation tag when known.
pub fn convert(
    values: &Lua,
    base: Option<&Map<String, Value>>,
    is_raw: bool,
    orientation: Option<&Lua>,
) -> Result<Converted> {
    let table = match values {
        Lua::Table(t) if t.keys().all(|k| matches!(k, Key::Str(_))) => t,
        _ => return Err(Error::data("이름이 있는 Adobe 보정 표가 아닙니다.")),
    };
    let empty = Map::new();
    let settings = normalized(base.unwrap_or(&empty)).0;
    let mut run =
        Run { values: table, settings, mapped: Vec::new(), omitted: Omitted::default(), used: BTreeSet::new() };
    if let Some(applied) = table.get_str("AlreadyApplied") {
        run.used.insert("AlreadyApplied".into());
        if boolean(applied)? {
            let mut omitted = Omitted::default();
            omitted.set("AlreadyApplied", "픽셀에 이미 반영된 보정을 중복 적용하지 않습니다.");
            return Ok(Converted { settings: Settings(run.settings), mapped: Vec::new(), omitted });
        }
    }
    for (key, target, low, high) in SCALARS {
        run.apply(key, |s, v| scalar(s, target, v, low, high))?;
    }
    run.apply("PostCropVignetteAmount", |s, v| {
        // Lightroom darkens with negative amounts; Grainy's post-crop vignette only darkens (0-100).
        let amount = number(v, -100.0, 100.0)?;
        if amount > 0.0 {
            return Err(Error::value("밝게 하는 비네팅은 지원하지 않습니다."));
        }
        s.insert("vignette".into(), float(-amount));
        Ok(())
    })?;
    for (old, new, target) in [
        ("Exposure", "Exposure2012", "exposure"),
        ("Contrast", "Contrast2012", "contrast"),
        ("Clarity", "Clarity2012", "clarity"),
    ] {
        if !table.contains_str(new) {
            let (low, high) = if old == "Exposure" { (-4.0, 4.0) } else { (-100.0, 100.0) };
            run.apply(old, |s, v| scalar(s, target, v, low, high))?;
        } else if table.contains_str(old) {
            run.used.insert(old.to_owned());
        }
    }
    run.apply("ConvertToGrayscale", |s, v| {
        s.insert("monochrome".into(), Value::Bool(boolean(v)?));
        Ok(())
    })?;
    for (prefix, channel) in [("HueAdjustment", 0usize), ("SaturationAdjustment", 1), ("LuminanceAdjustment", 2)] {
        for (index, color) in COLORS.iter().enumerate() {
            run.apply(&format!("{prefix}{color}"), |s, v| {
                set_item(s, "hsl", &[index, channel], float(number(v, -100.0, 100.0)?))?;
                s.insert("mixer_mode".into(), Value::String("hsl".into()));
                Ok(())
            })?;
        }
    }
    for (index, color) in ["Red", "Green", "Blue"].iter().enumerate() {
        for (channel, suffix) in ["Hue", "Saturation"].iter().enumerate() {
            run.apply(&format!("{color}{suffix}"), |s, v| {
                set_item(s, "calibration", &[index, channel], float(number(v, -100.0, 100.0)?))
            })?;
        }
    }
    for (suffix, target) in [("", None), ("Red", Some(0usize)), ("Green", Some(1)), ("Blue", Some(2))] {
        let key = if table.contains_str(&format!("ToneCurvePV2012{suffix}")) {
            format!("ToneCurvePV2012{suffix}")
        } else {
            format!("ToneCurve{suffix}")
        };
        run.apply(&key, |s, v| {
            let points = curve_value(curve(v)?);
            match target {
                None => {
                    s.insert("curve".into(), points);
                    Ok(())
                }
                Some(t) => set_item(s, "rgb_curves", &[t], points),
            }
        })?;
        // A display name is not a curve definition; only consume it when coordinates were validated.
        if run.mapped.contains(&key) {
            run.use_all(&["ToneCurveName", "ToneCurveName2012"]);
        }
    }
    let splits = [("ParametricShadowSplit", 25), ("ParametricMidtoneSplit", 50), ("ParametricHighlightSplit", 75)];
    let custom_splits = splits.iter().any(|(k, v)| table.get_str(k).is_some_and(|value| !is_default_split(value, *v)));
    if !custom_splits {
        run.use_all(&splits.map(|(k, _)| k));
        for (i, key) in
            ["ParametricShadows", "ParametricDarks", "ParametricLights", "ParametricHighlights"].iter().enumerate()
        {
            run.apply(key, |s, v| set_item(s, "parametric", &[i], float(number(v, -100.0, 100.0)?)))?;
        }
    }
    for (index, tone) in [(0usize, "Shadow"), (2, "Highlight")] {
        for (component, suffix, limit) in [(0usize, "Hue", 360.0), (1, "Saturation", 100.0)] {
            run.apply(&format!("SplitToning{tone}{suffix}"), |s, v| {
                set_item(s, "grading", &[index, component], float(number(v, 0.0, limit)?))
            })?;
        }
    }
    for (index, tone) in ["Shadow", "Midtone", "Highlight"].iter().enumerate() {
        for (component, suffix, low, high) in
            [(0usize, "Hue", 0.0, 360.0), (1, "Sat", 0.0, 100.0), (2, "Lum", -100.0, 100.0)]
        {
            run.apply(&format!("ColorGrade{tone}{suffix}"), |s, v| {
                set_item(s, "grading", &[index, component], float(number(v, low, high)?))
            })?;
        }
    }
    run.apply("SplitToningBalance", |s, v| scalar(s, "grading_balance", v, -100.0, 100.0))?;
    run.apply("ColorGradeBalance", |s, v| scalar(s, "grading_balance", v, -100.0, 100.0))?;
    // Absolute Kelvin is only meaningful before RAW demosaiced RGB is balanced.
    let wb = casefold(&white_balance_text(table.get_str("WhiteBalance")).replace(' ', ""));
    if is_raw && wb == "asshot" {
        run.settings.insert("raw_mode".into(), Value::String("as_shot".into()));
        run.settings.insert("raw_neutral".into(), Value::Null);
        run.settings.insert("temperature".into(), float(0.0));
        run.settings.insert("tint".into(), float(0.0));
        run.settings.insert("kelvin_enabled".into(), Value::Bool(false));
        for k in ["Temperature", "Tint", "WhiteBalance"] {
            if table.contains_str(k) {
                run.mapped.push(k.to_owned());
            }
        }
        run.use_all(&["Temperature", "Tint", "WhiteBalance"]);
    } else if is_raw && table.contains_str("Temperature") {
        let outcome = (|| -> Result<(f64, f64)> {
            let temperature = number(table.get_str("Temperature").unwrap_or(&Lua::Nil), 2000.0, 50000.0)?;
            let tint = number(table.get_str("Tint").unwrap_or(&Lua::Int(0)), -150.0, 150.0)?;
            Ok((temperature, tint))
        })();
        match outcome {
            Err(e) if e.is_value_error() => run.omitted.set("WhiteBalance", &e.message),
            Err(e) => return Err(e),
            Ok((temperature, tint)) => {
                run.settings.insert("raw_mode".into(), Value::String("custom".into()));
                run.settings.insert("raw_kelvin".into(), float(temperature));
                run.settings.insert("raw_tint".into(), float(tint));
                run.settings.insert("raw_neutral".into(), Value::Null);
                run.settings.insert("temperature".into(), float(0.0));
                run.settings.insert("tint".into(), float(0.0));
                run.settings.insert("kelvin_enabled".into(), Value::Bool(false));
                for k in ["Temperature", "Tint", "WhiteBalance"] {
                    if table.contains_str(k) {
                        run.mapped.push(k.to_owned());
                    }
                }
            }
        }
        run.use_all(&["Temperature", "Tint", "WhiteBalance"]);
    }
    // Non-rotated normalized crop is supported; rotation needs a separately verified transform.
    if let Some(has_crop) = table.get_str("HasCrop") {
        run.used.insert("HasCrop".into());
        let outcome = (|| -> Result<Option<Vec<f64>>> {
            if !boolean(has_crop)? {
                return Ok(None);
            }
            let turned = !upright(orientation)
                || number(table.get_str("CropAngle").unwrap_or(&Lua::Int(0)), -180.0, 180.0)? != 0.0;
            if turned {
                return Err(Error::value("회전된 Adobe 크롭 좌표는 변환하지 않습니다."));
            }
            let mut rect = Vec::new();
            for k in ["CropLeft", "CropTop", "CropRight", "CropBottom"] {
                let v = table.get_str(k).ok_or_else(|| Error::key(k))?;
                rect.push(number(v, 0.0, 1.0)?);
            }
            if rect[0] >= rect[2] || rect[1] >= rect[3] {
                return Err(Error::value("크롭 영역의 순서가 잘못되었습니다."));
            }
            Ok(Some(rect))
        })();
        match outcome {
            Ok(None) => {
                run.settings.insert("crop".into(), Value::Null);
                run.mapped.push("HasCrop".into());
                run.use_all(&["CropLeft", "CropRight", "CropTop", "CropBottom", "CropAngle", "CropConstrainToWarp"]);
            }
            Ok(Some(rect)) => {
                run.settings.insert("crop".into(), Value::Array(rect.into_iter().map(float).collect()));
                for k in ["HasCrop", "CropLeft", "CropTop", "CropRight", "CropBottom"] {
                    run.mapped.push(k.to_owned());
                }
                run.use_all(&["CropLeft", "CropTop", "CropRight", "CropBottom", "CropAngle"]);
            }
            Err(e)
                if matches!(
                    e.kind,
                    crate::error::ErrorKind::Value | crate::error::ErrorKind::Data | crate::error::ErrorKind::Key
                ) =>
            {
                run.omitted.set("HasCrop", &e.message)
            }
            Err(e) => return Err(e),
        }
    }
    for key in table.keys() {
        if let Key::Str(key) = key {
            if !run.used.contains(key) && !INFORMATION.contains(&key.as_str()) {
                run.omitted.set(key, UNSUPPORTED);
            }
        }
    }
    let mapped: Vec<String> = run.mapped.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();
    let mut omitted = run.omitted;
    omitted.0.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Converted { settings: Settings(run.settings), mapped, omitted })
}

/// Port of `adobe_develop.summary(result, limit=None)`: the report shown to the person.
pub fn summary(result: &Converted, limit: Option<usize>) -> String {
    let mut text = format!("변환한 보정 항목: {}개\n", result.mapped.len());
    text.push_str("Luma의 현상 방식으로 적용하므로 Lightroom과 색·명암·효과 강도가 달라질 수 있습니다.");
    if !result.omitted.is_empty() {
        let items = &result.omitted.0;
        let shown = match limit {
            None => items.len(),
            Some(n) => n.min(items.len()),
        };
        text.push_str(&format!("\n\n변환하지 않은 항목 {}개:\n", items.len()));
        let lines: Vec<String> = items[..shown].iter().map(|(k, r)| format!("• {k}: {r}")).collect();
        text.push_str(&lines.join("\n"));
        if shown < items.len() {
            text.push_str(&format!("\n외 {}개 · 자세한 내용에서 전체 항목을 확인하세요.", items.len() - shown));
        }
    }
    text
}
