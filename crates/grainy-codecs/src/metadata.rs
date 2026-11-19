use crate::{invalid, Metadata, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub typ: u16,
    pub count: u32,
    pub data: Vec<u8>,
}
pub(crate) type Ifd = BTreeMap<u16, Entry>;
#[derive(Clone, Default)]
pub(crate) struct Exif {
    pub root: Ifd,
    pub exif: Ifd,
    pub gps: Ifd,
}

impl Entry {
    pub fn uint(typ: u16, n: u32) -> Self {
        Self {
            typ,
            count: 1,
            data: if typ == 3 { (n as u16).to_be_bytes().to_vec() } else { n.to_be_bytes().to_vec() },
        }
    }
    fn ascii(text: &str) -> Self {
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        Self { typ: 2, count: data.len() as u32, data }
    }
    pub fn numbers(&self) -> Vec<f64> {
        let unit = unit(self.typ).unwrap_or(1);
        self.data
            .chunks_exact(unit)
            .map(|p| match self.typ {
                1 | 7 => p[0] as f64,
                6 => (p[0] as i8) as f64,
                3 => u16::from_be_bytes([p[0], p[1]]) as f64,
                8 => i16::from_be_bytes([p[0], p[1]]) as f64,
                4 => u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as f64,
                9 => i32::from_be_bytes([p[0], p[1], p[2], p[3]]) as f64,
                5 | 10 => {
                    let n = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
                    let d = u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
                    if self.typ == 10 {
                        (n as i32) as f64 / (d as i32) as f64
                    } else {
                        n as f64 / d as f64
                    }
                }
                11 => f32::from_be_bytes([p[0], p[1], p[2], p[3]]) as f64,
                12 => f64::from_be_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]]),
                _ => 0.,
            })
            .collect()
    }
    pub fn number(&self) -> f64 {
        self.numbers().first().copied().unwrap_or(0.)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).trim_end_matches('\0').to_string()
    }
    fn rational_text(&self, reduce: bool) -> String {
        if self.data.len() < 8 {
            return String::new();
        }
        let n = u32::from_be_bytes(self.data[..4].try_into().unwrap_or([0; 4]));
        let d = u32::from_be_bytes(self.data[4..8].try_into().unwrap_or([0; 4]));
        let (n, d) = if reduce {
            let g = gcd(n, d).max(1);
            (n / g, d / g)
        } else {
            (n, d)
        };
        if reduce && d == 1 {
            n.to_string()
        } else {
            format!("{n}/{d}")
        }
    }
}
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn unit(t: u16) -> Option<usize> {
    Some(match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}
pub(crate) fn get(data: &[u8], offset: usize, n: usize) -> Result<&[u8]> {
    data.get(offset..offset.checked_add(n).ok_or_else(|| invalid("metadata offset overflow"))?)
        .ok_or_else(|| invalid("truncated metadata"))
}
pub(crate) fn u16_at(data: &[u8], offset: usize, le: bool) -> Result<u16> {
    let p = get(data, offset, 2)?;
    Ok(if le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) })
}
pub(crate) fn u32_at(data: &[u8], offset: usize, le: bool) -> Result<u32> {
    let p = get(data, offset, 4)?;
    Ok(if le { u32::from_le_bytes([p[0], p[1], p[2], p[3]]) } else { u32::from_be_bytes([p[0], p[1], p[2], p[3]]) })
}
pub(crate) fn parse_ifd(data: &[u8], offset: usize, le: bool) -> Result<Ifd> {
    let count = u16_at(data, offset, le)? as usize;
    get(data, offset + 2, count * 12)?;
    let mut map = Ifd::new();
    for i in 0..count {
        let at = offset + 2 + i * 12;
        let tag = u16_at(data, at, le)?;
        let typ = u16_at(data, at + 2, le)?;
        let count = u32_at(data, at + 4, le)?;
        let Some(size) = unit(typ) else { continue };
        let n = (count as usize).checked_mul(size).ok_or_else(|| invalid("tag size overflow"))?;
        let from = if n <= 4 { at + 8 } else { u32_at(data, at + 8, le)? as usize };
        let mut bytes = get(data, from, n)?.to_vec();
        if le {
            let step = if typ == 5 || typ == 10 { 4 } else { size };
            for chunk in bytes.chunks_exact_mut(step) {
                chunk.reverse();
            }
        }
        map.insert(tag, Entry { typ, count, data: bytes });
    }
    Ok(map)
}
pub(crate) fn parse_exif(data: &[u8]) -> Result<Exif> {
    let data = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
    let le = data.starts_with(b"II");
    if !(le || data.starts_with(b"MM")) || u16_at(data, 2, le)? != 42 {
        return Err(invalid("not classic TIFF/EXIF"));
    }
    let root = parse_ifd(data, u32_at(data, 4, le)? as usize, le)?;
    let sub = |tag| -> Result<Ifd> {
        if let Some(e) = root.get(&tag) {
            parse_ifd(data, e.number() as usize, le)
        } else {
            Ok(Ifd::new())
        }
    };
    Ok(Exif { exif: sub(34665)?, gps: sub(34853)?, root })
}

pub(crate) fn jpeg_segments(data: &[u8]) -> Result<Vec<(u8, &[u8])>> {
    if !data.starts_with(b"\xff\xd8") {
        return Err(invalid("not JPEG"));
    }
    let mut at = 2;
    let mut result = vec![];
    while at < data.len() {
        if data[at] != 255 {
            return Err(invalid("bad JPEG marker"));
        }
        while at < data.len() && data[at] == 255 {
            at += 1;
        }
        let marker = *data.get(at).ok_or_else(|| invalid("truncated JPEG marker"))?;
        at += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 1 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let len = u16_at(data, at, false)? as usize;
        if len < 2 {
            return Err(invalid("short JPEG segment"));
        }
        result.push((marker, get(data, at + 2, len - 2)?));
        at += len;
    }
    Ok(result)
}
pub(crate) fn png_chunks(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid("not PNG"));
    }
    let mut at = 8;
    let mut result = vec![];
    while at < data.len() {
        let n = u32_at(data, at, false)? as usize;
        let kind = get(data, at + 4, 4)?.try_into().map_err(crate::codec)?;
        let payload = get(data, at + 8, n)?;
        get(data, at + 8 + n, 4)?;
        result.push((kind, payload));
        at += 12 + n;
    }
    Ok(result)
}
pub(crate) fn webp_chunks(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    if !data.starts_with(b"RIFF") || get(data, 8, 4)? != b"WEBP" {
        return Err(invalid("not WebP"));
    }
    let mut at = 12;
    let mut out = vec![];
    while at + 8 <= data.len() {
        let n = u32_at(data, at + 4, true)? as usize;
        out.push((get(data, at, 4)?.try_into().map_err(crate::codec)?, get(data, at + 8, n)?));
        at += 8 + n + (n & 1);
    }
    Ok(out)
}
pub(crate) fn extract_exif(data: &[u8]) -> Result<Option<Vec<u8>>> {
    if data.starts_with(b"II") || data.starts_with(b"MM") {
        return Ok(Some(data.to_vec()));
    }
    if data.starts_with(b"\xff\xd8") {
        return Ok(jpeg_segments(data)?
            .into_iter()
            .find(|(m, p)| *m == 0xe1 && p.starts_with(b"Exif\0\0"))
            .map(|(_, p)| p.to_vec()));
    }
    if data.starts_with(b"\x89PNG") {
        return Ok(png_chunks(data)?.into_iter().find(|(k, _)| k == b"eXIf").map(|(_, p)| p.to_vec()));
    }
    if data.starts_with(b"RIFF") {
        return Ok(webp_chunks(data)?.into_iter().find(|(k, _)| k == b"EXIF").map(|(_, p)| p.to_vec()));
    }
    Ok(None)
}
fn container_exif(data: &[u8], ext: &str) -> Result<Exif> {
    if ext == "raf" {
        if !data.starts_with(b"FUJIFILMCCD-RAW") {
            return Ok(Exif::default());
        }
        let off = u32_at(data, 84, false)? as usize;
        let len = u32_at(data, 88, false)? as usize;
        if len == 0 || len > 64 * 1024 * 1024 {
            return Ok(Exif::default());
        }
        return extract_exif(get(data, off, len)?)?.map(|p| parse_exif(&p)).unwrap_or(Ok(Exif::default()));
    }
    let data = &data[..data.len().min(4 * 1024 * 1024)];
    let mut out = Exif::default();
    for (name, which) in [(b"CMT1", 0), (b"CMT2", 1), (b"CMT4", 2)] {
        if let Some(at) = data.windows(4).position(|p| p == name) {
            if at < 4 {
                continue;
            }
            let size = u32_at(data, at - 4, false)? as usize;
            if size < 8 {
                continue;
            }
            if let Ok(block) = get(data, at + 4, size - 8) {
                if let Ok(parsed) = parse_exif(block) {
                    match which {
                        0 => {
                            out.root.extend(parsed.root);
                            out.exif.extend(parsed.exif);
                            out.gps.extend(parsed.gps);
                        }
                        1 => {
                            out.exif.extend(parsed.root);
                            out.exif.extend(parsed.exif);
                            out.gps.extend(parsed.gps);
                        }
                        _ => {
                            // exifread names CMT4's root entries "Image Tag 0x0002", etc.;
                            // prefix rewriting never turns them into "GPS GPSLatitude".
                            out.exif.extend(parsed.exif);
                            out.gps.extend(parsed.gps);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Port of `luma/engine.py read_metadata` and `_container_tags`, including PNG's decimal shutter strings.
pub fn read_metadata(path: impl AsRef<Path>) -> Result<Metadata> {
    let path = path.as_ref();
    let data = std::fs::read(path)?;
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    let ex = if ext == "webp" {
        // Port exifread 3.5.1's bugs: inspect the VP8X length byte instead of flags,
        // skip no odd chunk padding, and unconditionally skip six EXIF prefix bytes.
        let mut found = None;
        if data.get(12..16) == Some(b"VP8X") && data.get(16).is_some_and(|v| v & 8 != 0) {
            let mut at = 30;
            while at + 8 <= data.len() {
                let n = u32_at(&data, at + 4, true)? as usize;
                if data.get(at..at + 4) == Some(b"EXIF") {
                    found = data.get(at + 14..).and_then(|v| parse_exif(v).ok());
                    break;
                }
                let Some(next) = at.checked_add(8).and_then(|v| v.checked_add(n)) else { break };
                at = next;
            }
        }
        found.unwrap_or_default()
    } else if ext == "cr3" || ext == "raf" {
        container_exif(&data, &ext).unwrap_or_default()
    } else {
        extract_exif(&data)?.and_then(|d| parse_exif(&d).ok()).unwrap_or_default()
    };
    let mut out = Metadata::new();
    for (tag, name, numeric, root) in [
        (271, "make", false, true),
        (272, "camera", false, true),
        (315, "creator", false, true),
        (33432, "copyright", false, true),
        (33434, "shutter", false, false),
        (33437, "aperture", true, false),
        (34855, "iso", true, false),
        (37386, "focal_length", true, false),
        (36867, "date", false, false),
        (42036, "lens", false, false),
        (37382, "subject_distance", true, false),
        (42033, "serial", false, false),
    ] {
        let value = if ext == "png" {
            ex.exif.get(&tag).or(ex.root.get(&tag))
        } else if root {
            ex.root.get(&tag)
        } else {
            ex.exif.get(&tag)
        };
        if let Some(v) = value {
            let value = if numeric {
                json!(v.number())
            } else if v.typ == 2 {
                json!(v.text())
            } else if v.typ == 5 || v.typ == 10 {
                json!(if ext == "png" { python_float(v.number()) } else { v.rational_text(true) })
            } else {
                json!(v.number().to_string())
            };
            out.insert(name.into(), value);
        }
    }
    if !out.contains_key("iso") && matches!(ext.as_str(), "rw2" | "rwl") {
        if let Some(v) = ex.root.get(&23) {
            out.insert("iso".into(), json!(v.number()));
        }
    }
    for (name, tag, r) in [("latitude", 2, 1), ("longitude", 4, 3)] {
        if let Some(v) = ex.gps.get(&tag) {
            let n = v.numbers();
            if n.len() >= 3 && n.iter().all(|v| v.is_finite()) {
                let mut p = n[0] + n[1] / 60. + n[2] / 3600.;
                if ex.gps.get(&r).is_some_and(|s| s.text() == "S" || s.text() == "W") {
                    p = -p;
                }
                out.insert(name.into(), json!(p));
            }
        }
    }
    Ok(out)
}
fn python_float(n: f64) -> String {
    if n.fract() == 0. {
        format!("{n:.1}")
    } else {
        n.to_string()
    }
}
pub(crate) fn python_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

fn fraction(v: f64) -> (u32, u32) {
    // Continued-fraction limit_denominator(100000), matching Fraction(float).
    let mut x = v;
    let (mut p0, mut q0, mut p1, mut q1) = (0u64, 1u64, 1u64, 0u64);
    loop {
        let a = x.floor() as u64;
        let Some(q2) = a.checked_mul(q1).and_then(|v| v.checked_add(q0)) else { break };
        if q2 > 100000 {
            break;
        }
        let p2 = p0 + a * p1;
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
        let f = x - a as f64;
        if f == 0. {
            return (p1 as u32, q1 as u32);
        }
        x = 1. / f;
    }
    if q1 == 0 {
        return (v as u32, 1);
    }
    let k = (100000 - q0) / q1;
    let a = (p0 + k * p1, q0 + k * q1);
    let b = (p1, q1);
    let r = if (b.0 as f64 / b.1 as f64 - v).abs() <= (a.0 as f64 / a.1 as f64 - v).abs() { b } else { a };
    (r.0 as u32, r.1 as u32)
}
fn coordinate(v: f64) -> Result<Entry> {
    if !v.is_finite() || v.abs() > u32::MAX as f64 {
        return Err(invalid("invalid GPS coordinate"));
    }
    let v = v.abs();
    let d = v as u32;
    let m = ((v - d as f64) * 60.) as u32;
    let s = fraction((v - d as f64 - m as f64 / 60.) * 3600.);
    let data = [(d, 1), (m, 1), s].into_iter().flat_map(|(n, d)| [n.to_be_bytes(), d.to_be_bytes()].concat()).collect();
    Ok(Entry { typ: 5, count: 3, data })
}

fn piexif_ifd(map: &Ifd, offset: usize, next: bool, pointers: &[(u16, u32)]) -> Result<Vec<u8>> {
    let count = map.len() + pointers.len();
    let count16 = u16::try_from(count).map_err(crate::codec)?;
    let head = 2 + 12 * count + if next { 4 } else { 0 };
    let mut out = count16.to_be_bytes().to_vec();
    let mut extra: Vec<u8> = vec![];
    for (&tag, e) in map {
        out.extend(tag.to_be_bytes());
        out.extend(e.typ.to_be_bytes());
        out.extend(e.count.to_be_bytes());
        if e.data.len() <= 4 {
            out.extend(&e.data);
            out.resize(out.len() + 4 - e.data.len(), 0);
        } else {
            let at = u32::try_from(offset + head + extra.len()).map_err(crate::codec)?;
            out.extend(at.to_be_bytes());
            extra.extend(&e.data);
        }
    }
    for &(tag, at) in pointers {
        out.extend(tag.to_be_bytes());
        out.extend(4u16.to_be_bytes());
        out.extend(1u32.to_be_bytes());
        out.extend(at.to_be_bytes());
    }
    if next {
        out.extend([0; 4]);
    }
    out.extend(extra);
    Ok(out)
}
/// Port of `luma/colorio.py export_exif`, including piexif ordering, GPS and the final four zero bytes.
pub fn export_exif(
    source: impl AsRef<Path>,
    metadata: &Metadata,
    size: Option<(u32, u32)>,
    color_space: &str,
) -> Result<Vec<u8>> {
    let mut data = std::fs::read(source)
        .ok()
        .and_then(|d| extract_exif(&d).ok().flatten())
        .and_then(|d| {
            let parsed = parse_exif(&d).ok()?;
            let tiff = d.strip_prefix(b"Exif\0\0").unwrap_or(&d);
            let le = tiff.starts_with(b"II");
            // piexif loads Interop before export_exif removes it. Bad pointers discard all source EXIF.
            if let Some(e) = parsed.exif.get(&40965) {
                let at = e.number() as usize;
                let count = u16_at(tiff, at, le).ok()? as usize;
                for i in 0..count {
                    unit(u16_at(tiff, at + 2 + i * 12 + 2, le).ok()?)?;
                }
                parse_ifd(tiff, at, le).ok()?;
            }
            Some(parsed)
        })
        .unwrap_or_default();
    // piexif.load drops unknown tags; dump chooses schema types rather than copying wire types.
    data.exif.remove(&37500);
    let schema: Value = serde_json::from_str(include_str!("../assets/exif-types.json")).map_err(crate::codec)?;
    for (name, table) in [("0th", &mut data.root), ("Exif", &mut data.exif), ("GPS", &mut data.gps)] {
        table.retain(|tag, _| schema[name].get(tag.to_string()).is_some());
        for (tag, e) in table {
            let typ = schema[name][tag.to_string()].as_u64().unwrap_or(e.typ as u64) as u16;
            if typ != e.typ {
                let numbers = e.numbers();
                e.data = match typ {
                    1 | 7 if matches!(e.typ, 1 | 7) => e.data.clone(),
                    3 => numbers.iter().flat_map(|v| (*v as u16).to_be_bytes()).collect(),
                    4 => numbers.iter().flat_map(|v| (*v as u32).to_be_bytes()).collect(),
                    _ => return Err(invalid(format!("unsupported EXIF type conversion for {tag}"))),
                };
                e.typ = typ;
            }
        }
    }
    data.root.remove(&34665);
    data.root.remove(&34853);
    data.exif.remove(&40965);
    data.exif.remove(&37500);
    data.root.insert(274, Entry::uint(3, 1));
    data.exif.insert(40961, Entry::uint(3, if color_space == "sRGB" { 1 } else { 65535 }));
    if let Some((w, h)) = size {
        for (tag, v) in [(256, w), (257, h)] {
            data.root.insert(tag, Entry::uint(4, v));
        }
        for (tag, v) in [(40962, w), (40963, h)] {
            data.exif.insert(tag, Entry::uint(4, v));
        }
    }
    for (key, tag) in [("creator", 315), ("copyright", 33432), ("caption", 270)] {
        if let Some(v) = metadata.get(key) {
            data.root.insert(tag, Entry::ascii(&python_str(v)));
        }
    }
    if let Some(Value::String(v)) = metadata.get("DateTimeOriginal").filter(|v| truthy(v)) {
        if !v.is_ascii() {
            return Err(invalid("EXIF date must be ASCII"));
        }
        for tag in [36867, 36868] {
            data.exif.insert(tag, Entry::ascii(v));
        }
    }
    for (key, r, tag, pos, neg) in [("latitude", 1, 2, "N", "S"), ("longitude", 3, 4, "E", "W")] {
        if let Some(v) = metadata.get(key) {
            if v.is_null() || python_str(v).trim().is_empty() {
                data.gps.remove(&r);
                data.gps.remove(&tag);
            } else {
                let n = v.as_f64().or_else(|| v.as_str()?.parse().ok()).ok_or_else(|| invalid("invalid GPS value"))?;
                data.gps.insert(r, Entry::ascii(if n >= 0. { pos } else { neg }));
                data.gps.insert(tag, coordinate(n)?);
            }
        }
    }
    let np = 1 + usize::from(!data.gps.is_empty());
    let root_size = piexif_ifd(&data.root, 8, true, &vec![(34665, 0); np])?.len();
    let exif = piexif_ifd(&data.exif, 8 + root_size, false, &[])?;
    let mut pointers = vec![(34665, (8 + root_size) as u32)];
    if !data.gps.is_empty() {
        pointers.push((34853, (8 + root_size + exif.len()) as u32));
    }
    let mut out = b"Exif\0\0MM\0*\0\0\0\x08".to_vec();
    out.extend(piexif_ifd(&data.root, 8, true, &pointers)?);
    out.extend(&exif);
    if !data.gps.is_empty() {
        out.extend(piexif_ifd(&data.gps, 8 + root_size + exif.len(), false, &[])?);
    }
    out.extend([0; 4]);
    Ok(out)
}

/// Port of `luma/colorio.py iptc_bytes`: UTF-8 IIM, byte truncation and comma-separated keyword records.
pub fn iptc_bytes(metadata: &Metadata, keywords: &str) -> Vec<u8> {
    let mut out = b"\x1c\x01\x5a\0\x03\x1b%G".to_vec();
    let mut record = |tag: u8, text: String| {
        let bytes = text.as_bytes();
        let bytes = &bytes[..bytes.len().min(32700)];
        if !bytes.is_empty() {
            out.extend([0x1c, 2, tag]);
            out.extend((bytes.len() as u16).to_be_bytes());
            out.extend(bytes);
        }
    };
    for (key, tag) in [
        ("title", 5),
        ("creator", 80),
        ("city", 90),
        ("location", 92),
        ("country", 101),
        ("copyright", 116),
        ("caption", 120),
    ] {
        record(tag, metadata.get(key).map(python_str).unwrap_or_default());
    }
    for keyword in keywords.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        record(25, keyword.into());
    }
    out
}
pub(crate) fn inject_iptc(jpeg: &[u8], metadata: &Metadata, keywords: &str) -> Result<Vec<u8>> {
    if !jpeg.starts_with(b"\xff\xd8") {
        return Err(invalid("not JPEG"));
    }
    let iim = iptc_bytes(metadata, keywords);
    let mut payload = b"Photoshop 3.0\0".to_vec();
    payload.extend(b"8BIM\x04\x04\0\0");
    payload.extend((iim.len() as u32).to_be_bytes());
    payload.extend(&iim);
    if !iim.len().is_multiple_of(2) {
        payload.push(0);
    }
    if payload.len() > 65530 {
        return Err(invalid("IPTC metadata too large"));
    }
    let mut out = b"\xff\xd8\xff\xed".to_vec();
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.extend(payload);
    out.extend(&jpeg[2..]);
    Ok(out)
}
/// Port of `luma/colorio.py jpeg_iptc`. Caller owns the newly created export file.
pub fn jpeg_iptc(path: impl AsRef<Path>, metadata: &Metadata, keywords: &str) -> Result<()> {
    let data = std::fs::read(&path)?;
    std::fs::write(path, inject_iptc(&data, metadata, keywords)?)?;
    Ok(())
}
fn escape(s: &str, attr: bool) -> String {
    let mut s = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    if attr {
        s = s.replace('"', "&quot;").replace('\t', "&#09;").replace('\n', "&#10;").replace('\r', "&#13;");
    }
    s
}
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::String(v) => !v.is_empty(),
        Value::Number(v) => v.as_f64() != Some(0.),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
/// Port of `luma/colorio.py xmp_metadata`, including ElementTree namespace and attribute ordering.
pub fn xmp_metadata(metadata: &Metadata, keywords: &str, exif: Option<&[u8]>) -> Result<Vec<u8>> {
    let ns = BTreeMap::from([
        ("x", "adobe:ns:meta/"),
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("dc", "http://purl.org/dc/elements/1.1/"),
        ("exif", "http://ns.adobe.com/exif/1.0/"),
        ("photoshop", "http://ns.adobe.com/photoshop/1.0/"),
        ("tiff", "http://ns.adobe.com/tiff/1.0/"),
    ]);
    let mut used = std::collections::BTreeSet::from(["x", "rdf"]);
    let mut children = String::new();
    let mut attrs: Vec<(String, String)> = vec![];
    for (key, tag) in [("title", "title"), ("caption", "description"), ("creator", "creator"), ("copyright", "rights")]
    {
        if let Some(v) = metadata.get(key).filter(|v| truthy(v)) {
            used.insert("dc");
            let kind = if key == "creator" { "Seq" } else { "Alt" };
            let lang = if kind == "Alt" { " xml:lang=\"x-default\"" } else { "" };
            children += &format!(
                "<dc:{tag}><rdf:{kind}><rdf:li{lang}>{}</rdf:li></rdf:{kind}></dc:{tag}>",
                escape(&python_str(v), false)
            );
        }
    }
    for (key, prefix, tag) in [
        ("city", "photoshop", "City"),
        ("country", "photoshop", "Country"),
        ("location", "photoshop", "Location"),
        ("DateTimeOriginal", "exif", "DateTimeOriginal"),
        ("latitude", "exif", "GPSLatitude"),
        ("longitude", "exif", "GPSLongitude"),
    ] {
        if let Some(v) = metadata.get(key).filter(|v| !v.is_null() && !python_str(v).trim().is_empty()) {
            used.insert(prefix);
            attrs.push((format!("{prefix}:{tag}"), python_str(v)));
        }
    }
    if !keywords.is_empty() {
        used.insert("dc");
        let items: Vec<_> = keywords.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        children += "<dc:subject>";
        if items.is_empty() {
            children += "<rdf:Bag />";
        } else {
            children += "<rdf:Bag>";
            for item in items {
                children += &format!("<rdf:li>{}</rdf:li>", escape(item, false));
            }
            children += "</rdf:Bag>";
        }
        children += "</dc:subject>";
    }
    if let Some(bytes) = exif.filter(|v| !v.is_empty()) {
        let data = parse_exif(bytes)?;
        for (table, prefix, entries) in [
            (&data.root, "tiff", vec![(271, "Make"), (272, "Model"), (315, "Artist"), (33432, "Copyright")]),
            (
                &data.exif,
                "exif",
                vec![
                    (33434, "ExposureTime"),
                    (33437, "FNumber"),
                    (34855, "ISOSpeedRatings"),
                    (37386, "FocalLength"),
                    (36867, "DateTimeOriginal"),
                ],
            ),
        ] {
            for (id, name) in entries {
                if let Some(v) = table.get(&id) {
                    used.insert(prefix);
                    let text = if v.typ == 2 || v.typ == 7 {
                        v.text()
                    } else if v.typ == 5 || v.typ == 10 {
                        v.rational_text(false)
                    } else {
                        v.number().to_string()
                    };
                    let key = format!("{prefix}:{name}");
                    if let Some(a) = attrs.iter_mut().find(|a| a.0 == key) {
                        a.1 = text;
                    } else {
                        attrs.push((key, text));
                    }
                }
            }
        }
    }
    let mut out = "<x:xmpmeta".to_string();
    for prefix in used {
        out += &format!(" xmlns:{prefix}=\"{}\"", ns[prefix]);
    }
    out += "><rdf:RDF><rdf:Description";
    for (k, v) in attrs {
        out += &format!(" {k}=\"{}\"", escape(&v, true));
    }
    if children.is_empty() {
        out += " />";
    } else {
        out += ">";
        out += &children;
        out += "</rdf:Description>";
    }
    out += "</rdf:RDF></x:xmpmeta>";
    Ok(out.into_bytes())
}

fn endian_data(e: &Entry, le: bool) -> Vec<u8> {
    let mut d = e.data.clone();
    if le {
        let step = if e.typ == 5 || e.typ == 10 { 4 } else { unit(e.typ).unwrap_or(1) };
        for c in d.chunks_exact_mut(step) {
            c.reverse();
        }
    }
    d
}
fn put16(out: &mut Vec<u8>, n: u16, le: bool) {
    out.extend(if le { n.to_le_bytes() } else { n.to_be_bytes() });
}
fn put32(out: &mut Vec<u8>, n: u32, le: bool) {
    out.extend(if le { n.to_le_bytes() } else { n.to_be_bytes() });
}
fn pillow_ifd(map: &Ifd, children: &[(u16, &Ifd)], offset: usize, le: bool) -> Result<Vec<u8>> {
    let mut all = map.clone();
    for (tag, _) in children {
        all.insert(*tag, Entry::uint(4, 0));
    }
    let count = u16::try_from(all.len()).map_err(crate::codec)?;
    let mut out = vec![];
    put16(&mut out, count, le);
    let mut extra = vec![];
    let head = 2 + all.len() * 12 + 4;
    for (&tag, e) in &all {
        let child = children.iter().find(|(t, _)| *t == tag);
        let mut e = e.clone();
        // Pillow infers known SHORT tags from its tag registry; arbitrary EXIF integer values use the smallest type.
        if child.is_none() && e.typ == 4 && e.number() <= 65535. && matches!(tag, 40962 | 40963) {
            e = Entry::uint(3, e.number() as u32);
        }
        if matches!(e.typ, 5 | 10) {
            for pair in e.data.as_chunks_mut::<8>().0 {
                let n = u32::from_be_bytes(pair[..4].try_into().unwrap_or([0; 4]));
                let d = u32::from_be_bytes(pair[4..].try_into().unwrap_or([0; 4]));
                let g = gcd(n, d).max(1);
                pair[..4].copy_from_slice(&(n / g).to_be_bytes());
                pair[4..].copy_from_slice(&(d / g).to_be_bytes());
            }
        }
        // Pillow reads TIFF ASCII bytes as Latin-1 and writes them as ASCII with replacement.
        // Preserve its lossy UTF-8 round trip, even though EXIF and XMP retain the original bytes.
        if e.typ == 2 {
            for byte in &mut e.data {
                if *byte >= 128 {
                    *byte = b'?';
                }
            }
        }
        let bytes = if let Some((_, c)) = child {
            pillow_ifd(c, &[], offset + head + extra.len(), le)?
        } else {
            endian_data(&e, le)
        };
        put16(&mut out, tag, le);
        put16(&mut out, e.typ, le);
        put32(&mut out, if child.is_some() { 1 } else { e.count }, le);
        if bytes.len() <= 4 {
            out.extend(&bytes);
            out.resize(out.len() + 4 - bytes.len(), 0);
        } else {
            put32(&mut out, u32::try_from(offset + head + extra.len()).map_err(crate::codec)?, le);
            extra.extend(&bytes);
            if bytes.len() % 2 != 0 {
                extra.push(0);
            }
        }
    }
    out.extend([0; 4]);
    out.extend(extra);
    Ok(out)
}
/// Port of `luma/colorio.py append_tiff_exif`: retain pixel offsets and append Pillow-compatible IFDs.
pub fn append_tiff_exif(handle: &mut (impl Read + Write + Seek), exif: &[u8]) -> Result<()> {
    handle.flush()?;
    handle.seek(SeekFrom::Start(0))?;
    let mut data = vec![];
    handle.read_to_end(&mut data)?;
    let le = data.starts_with(b"II");
    if u16_at(&data, 2, le)? != 42 {
        return Err(invalid("BigTIFF EXIF append is unsupported"));
    }
    let root = u32_at(&data, 4, le)? as usize;
    let n = u16_at(&data, root, le)? as usize;
    if u32_at(&data, root + 2 + n * 12, le)? != 0 {
        return Err(invalid("cannot append EXIF to multipage TIFF"));
    }
    let mut entries: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    for i in 0..n {
        let at = root + 2 + i * 12;
        entries.insert(u16_at(&data, at, le)?, get(&data, at, 12)?.to_vec());
    }
    let parsed = parse_exif(exif)?;
    let mut map = Ifd::new();
    for tag in [269, 270, 271, 272, 274, 282, 283, 296, 306, 315, 33432, 40091, 40092, 40093, 40094, 40095] {
        if let Some(e) = parsed.root.get(&tag) {
            map.insert(tag, e.clone());
        }
    }
    let mut children = vec![];
    if !parsed.exif.is_empty() {
        children.push((34665, &parsed.exif));
    }
    if !parsed.gps.is_empty() {
        children.push((34853, &parsed.gps));
    }
    if data.len() % 2 != 0 {
        data.push(0);
    }
    let offset = data.len();
    let block = pillow_ifd(&map, &children, offset, le)?;
    let count = u16_at(&block, 0, le)? as usize;
    for i in 0..count {
        let at = 2 + i * 12;
        entries.insert(u16_at(&block, at, le)?, get(&block, at, 12)?.to_vec());
    }
    data.extend(block);
    if data.len() % 2 != 0 {
        data.push(0);
    }
    let root = u32::try_from(data.len()).map_err(crate::codec)?;
    put16(&mut data, u16::try_from(entries.len()).map_err(crate::codec)?, le);
    for e in entries.values() {
        data.extend(e);
    }
    data.extend([0; 4]);
    data[4..8].copy_from_slice(&if le { root.to_le_bytes() } else { root.to_be_bytes() });
    handle.seek(SeekFrom::Start(0))?;
    handle.write_all(&data)?;
    handle.seek(SeekFrom::End(0))?;
    Ok(())
}
