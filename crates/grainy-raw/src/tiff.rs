//! Bounded TIFF/BigTIFF tag reader. Pixel decoding belongs exclusively to LibRaw.
use crate::{invalid, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// Port of dcp.read_tags values (numeric arrays, UTF-8 text, opaque byte payloads).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Tag {
    /// Port of dcp.read_tags ASCII/UTF-8 tags.
    Text(String),
    /// Port of dcp.read_tags numeric arrays.
    Numbers(Vec<f64>),
    /// Port of dcp.read_tags UNDEFINED payloads.
    Bytes(Vec<u8>),
}
pub(crate) type Tags = BTreeMap<u16, Tag>;
impl Tag {
    pub(crate) fn numbers(&self) -> Result<&[f64]> {
        if let Self::Numbers(v) = self {
            Ok(v)
        } else {
            Err(invalid("numeric TIFF tag required"))
        }
    }
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Text(v) => v.clone(),
            _ => String::new(),
        }
    }
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        if let Self::Bytes(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
pub(crate) fn u16_at(a: &[u8], le: bool) -> u16 {
    if le {
        u16::from_le_bytes([a[0], a[1]])
    } else {
        u16::from_be_bytes([a[0], a[1]])
    }
}
pub(crate) fn u32_at(a: &[u8], le: bool) -> u32 {
    if le {
        u32::from_le_bytes([a[0], a[1], a[2], a[3]])
    } else {
        u32::from_be_bytes([a[0], a[1], a[2], a[3]])
    }
}
fn get<R: Read + Seek>(r: &mut R, pos: u64, n: usize, len: u64) -> Result<Vec<u8>> {
    if pos.checked_add(n as u64).is_none_or(|end| end > len) {
        return Err(invalid("TIFF tag outside file"));
    }
    let mut v = vec![0; n];
    r.seek(SeekFrom::Start(pos))?;
    r.read_exact(&mut v)?;
    Ok(v)
}
fn u64_at(a: &[u8], le: bool) -> u64 {
    let bytes = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
    if le {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    }
}
fn header<R: Read + Seek>(r: &mut R, len: u64, dcp: bool) -> Result<(bool, u64, bool)> {
    let h = get(r, 0, 8, len)?;
    let le = match &h[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(invalid("invalid TIFF byte order")),
    };
    let magic = u16_at(&h[2..], le);
    if magic == 43 && !dcp {
        if u16_at(&h[4..], le) != 8 || u16_at(&h[6..], le) != 0 {
            return Err(invalid("invalid BigTIFF header"));
        }
        return Ok((le, u64_at(&get(r, 8, 8, len)?, le), true));
    }
    if ![42, if dcp { 0x4352 } else { 42 }].contains(&magic) {
        return Err(invalid("not a classic TIFF/DCP"));
    }
    Ok((le, u64::from(u32_at(&h[4..], le)), false))
}
fn directory<R: Read + Seek>(
    r: &mut R,
    len: u64,
    offset: u64,
    le: bool,
    wanted: Option<&[u16]>,
    dcp: bool,
    big: bool,
) -> Result<(Tags, u64)> {
    if offset < 8 {
        return Err(invalid("invalid TIFF directory"));
    }
    let (count_size, entry_size, slot_size, slot_at) = if big { (8, 20, 8, 12) } else { (2, 12, 4, 8) };
    let count_bytes = get(r, offset, count_size, len)?;
    let n = if big { u64_at(&count_bytes, le) } else { u64::from(u16_at(&count_bytes, le)) };
    if n > if dcp { 512 } else { 4096 } {
        return Err(invalid("too many TIFF tags"));
    }
    let n = n as usize;
    let entries = get(r, offset + count_size as u64, n * entry_size + slot_size, len)?;
    let mut tags = Tags::new();
    let mut seen = HashSet::new();
    for i in 0..n {
        let e = &entries[i * entry_size..(i + 1) * entry_size];
        let tag = u16_at(e, le);
        let kind = u16_at(&e[2..], le);
        let count = if big { u64_at(&e[4..], le) } else { u64::from(u32_at(&e[4..], le)) };
        let count = usize::try_from(count).map_err(|_| invalid("TIFF count overflow"))?;
        if !seen.insert(tag) {
            return Err(invalid("duplicate TIFF tag"));
        }
        let selected = wanted.is_none_or(|w| w.contains(&tag));
        if !dcp && !selected {
            continue;
        }
        let width = match kind {
            1 | 2 | 7 => 1,
            3 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            6 if !dcp => 1,
            8 if !dcp => 2,
            13 if !dcp => 4,
            16..=18 if !dcp => 8,
            _ => return Err(invalid("unsupported TIFF field type")),
        };
        let size = count.checked_mul(width).ok_or_else(|| invalid("TIFF tag size overflow"))?;
        let pos = if size <= slot_size {
            offset + count_size as u64 + (i * entry_size + slot_at) as u64
        } else if big {
            u64_at(&e[slot_at..], le)
        } else {
            u64::from(u32_at(&e[slot_at..], le))
        };
        if count == 0 || pos.checked_add(size as u64).is_none_or(|v| v > len) {
            return Err(invalid("invalid TIFF data range"));
        }
        if !selected {
            continue;
        }
        let max_count = if tag == crate::proraw::TAG_PROFILE_GAIN_TABLE_MAP {
            4 * 1024 * 1024
        } else if (52533..=52535).contains(&tag) {
            8022
        } else {
            1024
        };
        if !dcp && count > max_count {
            return Err(invalid("DNG color tag too large"));
        }
        let data = get(r, pos, size, len)?;
        let value = if kind == 7 {
            Tag::Bytes(data)
        } else if kind == 2 || (kind == 1 && [50708, 50932, 50936, 50942, 52552].contains(&tag)) {
            let end = data.iter().rposition(|x| *x != 0).map_or(0, |i| i + 1);
            Tag::Text(String::from_utf8_lossy(&data[..end]).into_owned())
        } else {
            let mut values = Vec::with_capacity(count);
            for chunk in data.chunks_exact(width) {
                let v = match kind {
                    1 => chunk[0] as f64,
                    3 => u16_at(chunk, le) as f64,
                    6 => (chunk[0] as i8) as f64,
                    8 => (u16_at(chunk, le) as i16) as f64,
                    4 | 13 => u32_at(chunk, le) as f64,
                    16 | 18 => u64_at(chunk, le) as f64,
                    17 => (u64_at(chunk, le) as i64) as f64,
                    9 => (u32_at(chunk, le) as i32) as f64,
                    11 => f32::from_bits(u32_at(chunk, le)) as f64,
                    12 => {
                        let mut a = [0; 8];
                        a.copy_from_slice(chunk);
                        f64::from_bits(if le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) })
                    }
                    5 | 10 => {
                        let a = u32_at(chunk, le);
                        let b = u32_at(&chunk[4..], le);
                        if b == 0 {
                            return Err(invalid("zero TIFF rational denominator"));
                        }
                        if kind == 10 {
                            (a as i32) as f64 / (b as i32) as f64
                        } else {
                            a as f64 / b as f64
                        }
                    }
                    _ => return Err(invalid("invalid numeric TIFF type")),
                };
                if !v.is_finite() {
                    return Err(invalid("nonfinite TIFF value"));
                }
                values.push(v);
            }
            Tag::Numbers(values)
        };
        tags.insert(tag, value);
    }
    let next =
        if big { u64_at(&entries[n * entry_size..], le) } else { u64::from(u32_at(&entries[n * entry_size..], le)) };
    Ok((tags, next))
}
pub(crate) fn standalone(data: &[u8], wanted: Option<&[u16]>) -> Result<(Tags, bool)> {
    if !(8..=4 * 1024 * 1024).contains(&data.len()) {
        return Err(invalid("DCP must be between 8 bytes and 4 MB"));
    }
    let mut r = std::io::Cursor::new(data);
    let (le, offset, big) = header(&mut r, data.len() as u64, true)?;
    Ok((directory(&mut r, data.len() as u64, offset, le, wanted, true, big)?.0, le))
}
pub(crate) fn dng(path: &Path, wanted: &[u16], all_roots: bool) -> Result<(Vec<Tags>, bool)> {
    let mut r = std::fs::File::open(path)?;
    let len = r.metadata()?.len();
    let (le, first, big) = header(&mut r, len, false)?;
    let mut request = wanted.to_vec();
    request.push(330);
    let mut todo = vec![(first, 0)];
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some((offset, depth)) = todo.pop() {
        if result.len() > 1024 || !seen.insert(offset) {
            return Err(invalid("cyclic or oversized TIFF directories"));
        }
        let (tags, next) = directory(&mut r, len, offset, le, Some(&request), false, big)?;
        if all_roots && depth == 0 && next != 0 {
            todo.push((next, 0));
        }
        if let Some(Tag::Numbers(sub)) = tags.get(&330).filter(|_| depth == 0) {
            for n in sub.iter().rev() {
                todo.push((*n as u64, 1));
            }
        }
        result.push(tags);
    }
    Ok((result, le))
}
