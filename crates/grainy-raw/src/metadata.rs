//! RAW-only port of `engine.read_metadata` and `_container_tags`.
use crate::Result;
use exif::{Context, Value as ExifValue};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufReader, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

fn accept(result: std::result::Result<exif::Exif, exif::Error>) -> Option<exif::Exif> {
    result.or_else(|e| e.distill_partial_result(|_| {})).ok()
}
fn ascii(v: &ExifValue) -> String {
    if let ExifValue::Ascii(a) = v {
        return a.first().map(|a| String::from_utf8_lossy(a).into_owned()).unwrap_or_default();
    }
    let values: Vec<String> = match v {
        ExifValue::Rational(v) => v.iter().map(|v| rational(i64::from(v.num), i64::from(v.denom))).collect(),
        ExifValue::SRational(v) => v.iter().map(|v| rational(i64::from(v.num), i64::from(v.denom))).collect(),
        ExifValue::Byte(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::Short(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::Long(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::SByte(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::SShort(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::SLong(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::Float(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::Double(v) => v.iter().map(ToString::to_string).collect(),
        ExifValue::Undefined(v, _) => v.iter().map(ToString::to_string).collect(),
        _ => Vec::new(),
    };
    if values.len() == 1 {
        values[0].clone()
    } else {
        format!("[{}]", values.join(", "))
    }
}
fn rational(n: i64, d: i64) -> String {
    if d == 0 {
        return format!("{n}/0");
    }
    let (mut a, mut b) = (n.unsigned_abs(), d.unsigned_abs());
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    let gcd = a.max(1) as i64;
    let (n, d) = (n / gcd, d / gcd);
    if d == 1 {
        n.to_string()
    } else {
        format!("{n}/{d}")
    }
}
fn number(v: &ExifValue, index: usize) -> Option<f64> {
    match v {
        ExifValue::Byte(v) => v.get(index).map(|v| *v as f64),
        ExifValue::Short(v) => v.get(index).map(|v| *v as f64),
        ExifValue::Long(v) => v.get(index).map(|v| *v as f64),
        ExifValue::SByte(v) => v.get(index).map(|v| *v as f64),
        ExifValue::SShort(v) => v.get(index).map(|v| *v as f64),
        ExifValue::SLong(v) => v.get(index).map(|v| *v as f64),
        ExifValue::Float(v) => v.get(index).map(|v| *v as f64),
        ExifValue::Double(v) => v.get(index).copied(),
        ExifValue::Rational(v) => v.get(index).filter(|v| v.denom != 0).map(|v| v.num as f64 / v.denom as f64),
        ExifValue::SRational(v) => v.get(index).filter(|v| v.denom != 0).map(|v| v.num as f64 / v.denom as f64),
        _ => None,
    }
    .filter(|v| v.is_finite())
}
type Tags = BTreeMap<(u8, u16), ExifValue>;
fn collect(exif: &exif::Exif, override_context: Option<u8>, tags: &mut Tags) {
    for f in exif.fields().filter(|f| f.ifd_num == exif::In::PRIMARY) {
        // exifread does not give GPS names to root-IFD tags in a CMT4 block.
        // Preserve that oracle quirk; an actual nested GPS IFD still works.
        if override_context == Some(2) && f.tag.context() == Context::Tiff {
            continue;
        }
        let context = match f.tag.context() {
            Context::Tiff => override_context.unwrap_or(0),
            Context::Exif => 1,
            Context::Gps => 2,
            _ => continue,
        };
        tags.insert((context, f.tag.number()), f.value.clone());
    }
}
fn containers(file: &mut File, extension: &str) -> Result<Tags> {
    let mut tags = Tags::new();
    let mut reader = exif::Reader::new();
    reader.continue_on_error(true);
    if extension == "raf" {
        file.seek(SeekFrom::Start(0))?;
        let mut h = [0; 92];
        if file.read_exact(&mut h).is_err() || !h.starts_with(b"FUJIFILMCCD-RAW") {
            return Ok(tags);
        }
        let offset = u32::from_be_bytes([h[84], h[85], h[86], h[87]]);
        let len = u32::from_be_bytes([h[88], h[89], h[90], h[91]]);
        if len == 0 || len > 64 * 1024 * 1024 {
            return Ok(tags);
        }
        file.seek(SeekFrom::Start(offset.into()))?;
        let mut jpeg = Vec::new();
        file.take(u64::from(len)).read_to_end(&mut jpeg)?;
        if let Some(exif) = accept(reader.read_from_container(&mut Cursor::new(jpeg))) {
            collect(&exif, None, &mut tags);
        }
        return Ok(tags);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut head = Vec::new();
    file.take(4 * 1024 * 1024).read_to_end(&mut head)?;
    for (marker, context) in [(b"CMT1", 0), (b"CMT2", 1), (b"CMT4", 2)] {
        let Some(at) = head.windows(4).position(|w| w == marker) else { continue };
        if at < 4 {
            continue;
        }
        let size = u32::from_be_bytes([head[at - 4], head[at - 3], head[at - 2], head[at - 1]]) as usize;
        let start = at + 4;
        let end = (at - 4).saturating_add(size).min(head.len());
        let Some(block) = head.get(start..end) else { continue };
        if !(block.starts_with(b"II*\0") || block.starts_with(b"MM\0*")) {
            continue;
        }
        if let Some(exif) = accept(reader.read_raw(block.to_vec())) {
            collect(&exif, Some(context), &mut tags);
        }
    }
    Ok(tags)
}
/// Port of engine.read_metadata for RAW inputs, including CR3/RAF EXIF containers.
/// Returns the oracle's JSON keys and rational shutter text, without decoding pixels.
pub fn read_metadata(path: impl AsRef<Path>) -> Result<Value> {
    let path = path.as_ref();
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("").to_lowercase();
    let mut file = File::open(path)?;
    let mut reader = exif::Reader::new();
    reader.continue_on_error(true);
    let mut tags = Tags::new();
    if let Some(exif) = accept(reader.read_from_container(&mut BufReader::new(&mut file))) {
        collect(&exif, None, &mut tags);
    }
    if tags.is_empty() {
        file.seek(SeekFrom::Start(0))?;
        let mut magic = [0; 4];
        if file.read_exact(&mut magic).is_ok() && [*b"IIRO", *b"IIRS", *b"MMOR", *b"IIU\0"].contains(&magic) {
            // Olympus/Panasonic use TIFF IFDs with their own magic word.
            // Adapt the owned parser buffer only; the source file stays read-only.
            file.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            (&mut file).take(32 * 1024 * 1024).read_to_end(&mut bytes)?;
            if let Some(word) = bytes.get_mut(2..4) {
                word.copy_from_slice(if magic.starts_with(b"II") { b"*\0" } else { b"\0*" });
            }
            if let Some(exif) = accept(reader.read_raw(bytes)) {
                collect(&exif, None, &mut tags);
            }
        }
    }
    if tags.is_empty() && ["cr3", "raf"].contains(&extension.as_str()) {
        tags = containers(&mut file, &extension)?;
    }
    let mut result = Map::new();
    for (context, tag, key, numeric) in [
        (0, 271, "make", false),
        (0, 272, "camera", false),
        (1, 33434, "shutter", false),
        (1, 33437, "aperture", true),
        (1, 34855, "iso", true),
        (1, 37386, "focal_length", true),
        (1, 36867, "date", false),
        (1, 42036, "lens", false),
        (1, 37382, "subject_distance", true),
        (0, 315, "creator", false),
        (0, 33432, "copyright", false),
        (1, 42033, "serial", false),
    ] {
        if let Some(v) = tags.get(&(context, tag)) {
            result.insert(
                key.into(),
                if numeric {
                    number(v, 0).map_or_else(|| Value::String(ascii(v)), Value::from)
                } else {
                    Value::String(ascii(v))
                },
            );
        }
    }
    if !result.contains_key("iso") && ["rw2", "rwl"].contains(&extension.as_str()) {
        if let Some(iso) = tags.get(&(0, 23)).and_then(|v| number(v, 0)) {
            result.insert("iso".into(), iso.into());
        }
    }
    for (key, tag, reference) in [("latitude", 2, 1), ("longitude", 4, 3)] {
        if let Some(v) = tags.get(&(2, tag)) {
            if let (Some(d), Some(m), Some(s)) = (number(v, 0), number(v, 1), number(v, 2)) {
                let mut coordinate = d + m / 60. + s / 3600.;
                if tags.get(&(2, reference)).is_some_and(|v| ["S", "W"].contains(&ascii(v).as_str())) {
                    coordinate = -coordinate;
                }
                result.insert(key.into(), coordinate.into());
            }
        }
    }
    Ok(Value::Object(result))
}
