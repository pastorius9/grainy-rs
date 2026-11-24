#[allow(unsafe_code)]
mod modern;

use crate::metadata::{extract_exif, jpeg_segments, parse_exif, png_chunks, u16_at, u32_at, webp_chunks, Ifd};
use crate::{codec, invalid, validate, ColorManager, ColorModel, Error, Metadata, Profile, Result};
use grainy_core::Image;
use serde_json::json;
use std::{
    io::{Cursor, Read},
    path::Path,
    time::UNIX_EPOCH,
};

/// Pillow-compatible JPEG backend used by the `luma/engine.py load_image` port.
#[derive(Clone, Copy, Debug, Default)]
pub enum JpegDecoder {
    /// Shared libjpeg-turbo with Pillow's colour conversion and draft scaling.
    #[default]
    LibjpegTurbo,
}
/// Arguments to the non-RAW `luma/engine.py load_image` port.
#[derive(Clone, Debug)]
pub struct LoadOptions {
    /// Optional longest-side preview reduction.
    pub max_size: Option<usize>,
    /// The oracle recognizes exactly `ProPhoto` as wide; other names follow sRGB.
    pub working_space: String,
    /// JPEG implementation, shared with LibRaw's lossy DNG decoder.
    pub jpeg_decoder: JpegDecoder,
}
impl Default for LoadOptions {
    fn default() -> Self {
        Self { max_size: None, working_space: "sRGB".into(), jpeg_decoder: JpegDecoder::default() }
    }
}
/// Result of `luma/engine.py load_image`.
pub struct LoadedImage {
    /// Scene-linear working pixels, oriented and optionally reduced.
    pub pixels: Image,
    /// Oracle-compatible metadata dictionary, including the live file fingerprint.
    pub metadata: Metadata,
}
/// Intermediate samples of `luma/rasterio.py read_bitmap`, before normalization or orientation.
pub struct DecodedBitmap {
    /// Native sample values represented as f32 (8/16-bit integers are exactly representable).
    pub samples: Image,
    /// Native samples retained before palette/YCbCr/Lab TIFF interpretation, for decoder verification.
    pub native_samples: Option<Image>,
    /// Native channel model; an optional alpha channel follows the colour channels.
    pub model: ColorModel,
    /// Normalization divisor; floating-point TIFFs use 1.
    pub divisor: f32,
    /// Source bit depth as reported by the oracle.
    pub bit_depth: u8,
    /// ICC source or synthesized profile request.
    pub profile: Option<Profile>,
    /// EXIF orientation, not yet applied.
    pub orientation: u16,
    /// Whether alpha is premultiplied and must first be unassociated.
    pub associated: bool,
    /// Floating TIFFs are already linear except Lab.
    pub linear: bool,
    /// Original dimensions, before JPEG draft and orientation.
    pub full_size: (usize, usize),
    /// Decoder-specific oracle metadata.
    pub info: Metadata,
    /// Whether `rasterio.read_tiff` inverts the normalized gray channel.
    pub white_is_zero: bool,
}

fn shape(h: usize, w: usize, c: usize, data: Vec<f32>) -> Result<Image> {
    if h == 0 || w == 0 || c == 0 || c > 5 || h.checked_mul(w).and_then(|n| n.checked_mul(c)) != Some(data.len()) {
        return Err(invalid("invalid image dimensions"));
    }
    Image::from_shape_vec((h, w, c), data).map_err(codec)
}
fn from_bytes(h: usize, w: usize, c: usize, data: Vec<u8>) -> Result<Image> {
    shape(h, w, c, data.into_iter().map(f32::from).collect())
}
fn source_orientation(data: &[u8]) -> u16 {
    extract_exif(data)
        .ok()
        .flatten()
        .and_then(|b| parse_exif(&b).ok())
        .and_then(|ex| ex.root.get(&274).map(|v| v.number() as u16))
        .filter(|n| *n != 0)
        .unwrap_or(1)
}
fn base(samples: Image, model: ColorModel, format: &str, bits: u8) -> DecodedBitmap {
    let (h, w, _) = samples.dim();
    DecodedBitmap {
        samples,
        native_samples: None,
        model,
        divisor: if bits == 16 { 65535. } else { 255. },
        bit_depth: bits,
        profile: None,
        orientation: 1,
        associated: false,
        linear: false,
        full_size: (w, h),
        info: Metadata::from_iter([("decoder".into(), json!(format)), ("bit_depth".into(), json!(bits))]),
        white_is_zero: false,
    }
}
fn unzlib(data: &[u8]) -> Result<Vec<u8>> {
    let decoder = flate2::read::ZlibDecoder::new(data);
    let mut out = vec![];
    decoder.take(32 * 1024 * 1024 + 1).read_to_end(&mut out)?;
    if out.len() > 32 * 1024 * 1024 {
        return Err(invalid("ICC profile too large"));
    }
    Ok(out)
}

fn decode_png(data: &[u8]) -> Result<DecodedBitmap> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(codec)?;
    let len = reader.output_buffer_size().ok_or_else(|| invalid("PNG too large"))?;
    if len > 512 * 1024 * 1024 {
        return Err(invalid("PNG exceeds decode allocation limit"));
    }
    let mut bytes = vec![0; len];
    let frame = reader.next_frame(&mut bytes).map_err(codec)?;
    bytes.truncate(frame.buffer_size());
    let channels = frame.color_type.samples();
    let model = if channels <= 2 { ColorModel::Gray } else { ColorModel::Rgb };
    let bits = *data.get(24).ok_or_else(|| invalid("PNG header"))?;
    let samples = if frame.bit_depth == png::BitDepth::Sixteen {
        shape(
            frame.height as usize,
            frame.width as usize,
            channels,
            bytes.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p) as f32).collect(),
        )?
    } else {
        from_bytes(frame.height as usize, frame.width as usize, channels, bytes)?
    };
    let mut out = base(samples, model, &format!("PNG {bits}-bit"), bits);
    out.divisor = if frame.bit_depth == png::BitDepth::Sixteen { 65535. } else { 255. };
    out.orientation = source_orientation(data);
    let chunks = png_chunks(data)?;
    let find = |name: &[u8; 4]| chunks.iter().find(|(k, _)| k == name).map(|(_, v)| *v);
    if let Some(cicp) = find(b"cICP") {
        out.profile = Some(Profile::Named(
            match cicp {
                [1, 13, 0, 1] => "sRGB",
                [12, 13, 0, 1] => "Display P3",
                _ => return Err(Error::Unsupported("PNG cICP HDR/video colour encoding".into())),
            }
            .into(),
        ));
        if model == ColorModel::Gray && cicp == [1, 13, 0, 1] {
            out.profile = None;
        }
    } else if let Some(icc) = find(b"iCCP") {
        let n = icc.iter().position(|b| *b == 0).ok_or_else(|| invalid("bad iCCP name"))?;
        if icc.get(n + 1) != Some(&0) {
            return Err(invalid("bad iCCP compression"));
        }
        out.profile = Some(Profile::Embedded(unzlib(icc.get(n + 2..).ok_or_else(|| invalid("bad iCCP"))?)?));
    } else if find(b"sRGB").is_some() {
        if model == ColorModel::Rgb {
            out.profile = Some(Profile::Named("sRGB".into()));
        }
    } else if find(b"gAMA").is_some() || find(b"cHRM").is_some() {
        let gamma = find(b"gAMA").map(|p| u32_at(p, 0, false).map(|v| v as f64 / 100000.)).transpose()?;
        if gamma.is_some_and(|v| v <= 0.) {
            return Err(invalid("invalid PNG gamma"));
        }
        let values = if let Some(p) = find(b"cHRM") {
            let mut v = [0.; 8];
            for (i, x) in v.iter_mut().enumerate() {
                *x = u32_at(p, i * 4, false)? as f64 / 100000.;
            }
            v
        } else {
            [0.3127, 0.329, 0.64, 0.33, 0.30, 0.60, 0.15, 0.06]
        };
        out.profile = Some(Profile::Calibrated {
            model,
            white: [values[0], values[1]],
            primaries: if model == ColorModel::Rgb {
                Some([values[2], values[3], values[4], values[5], values[6], values[7]])
            } else {
                None
            },
            gamma: gamma.map(|g| 1. / g),
            transfer: vec![],
        });
    }
    Ok(out)
}

fn decode_jpeg(data: &[u8], options: &LoadOptions, ext: &str) -> Result<DecodedBitmap> {
    let decoded = grainy_jpeg_sys::decode(data, options.max_size).map_err(codec)?;
    let model = match decoded.channels {
        1 => ColorModel::Gray,
        3 => ColorModel::Rgb,
        4 => ColorModel::Cmyk,
        _ => return Err(invalid("unsupported JPEG channel count")),
    };
    let full = (decoded.full_width as usize, decoded.full_height as usize);
    let samples =
        from_bytes(decoded.height as usize, decoded.width as usize, decoded.channels as usize, decoded.pixels)?;
    let name = if model == ColorModel::Rgb { ext.to_uppercase() } else { "JPEG".into() };
    let mut out = base(samples, model, &format!("{name} · 8-bit"), 8);
    out.full_size = full;
    let mut parts = Vec::new();
    let mut exif = Vec::new();
    let mut before_sof = true;
    for (m, p) in jpeg_segments(data)? {
        if matches!(m, 0xc0..=0xcf) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
            before_sof = false;
        }
        if m == 0xe1 && p.starts_with(b"Exif\0\0") {
            exif.extend_from_slice(if exif.is_empty() { p } else { &p[6..] });
        }
        if before_sof && m == 0xe2 && p.starts_with(b"ICC_PROFILE\0") {
            if p.len() < 14 {
                return Err(invalid("short JPEG ICC marker"));
            }
            parts.push(p);
        }
    }
    out.orientation = parse_exif(&exif).ok().and_then(|ex| ex.root.get(&274).map(|v| v.number() as u16)).unwrap_or(1);
    if !parts.is_empty() {
        // Pillow sorts the entire APP2 payload, checks only the first count byte, and retains
        // duplicate sequence numbers. Incomplete profiles are ignored, not a decode error.
        parts.sort();
        if parts[0][13] as usize == parts.len() {
            out.profile = Some(Profile::Embedded(parts.into_iter().flat_map(|p| p[14..].iter().copied()).collect()));
        }
    }
    Ok(out)
}

fn tag_num(tags: &Ifd, tag: u16, default: f64) -> f64 {
    tags.get(&tag).map(|e| e.number()).unwrap_or(default)
}
fn decode_lzw_bounded(compressed: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut decoder = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
    let mut out = Vec::new();
    let mut input = compressed;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let res = decoder.decode_bytes(input, &mut buf);
        input = &input[res.consumed_in..];
        if out.len().checked_add(res.consumed_out).is_none_or(|n| n > limit) {
            return Err(invalid("LZW allocation limit"));
        }
        out.extend_from_slice(&buf[..res.consumed_out]);
        match res.status {
            Ok(weezl::LzwStatus::Done) | Ok(weezl::LzwStatus::NoProgress) => break,
            Ok(weezl::LzwStatus::Ok) if input.is_empty() && res.consumed_out == 0 => break,
            Ok(weezl::LzwStatus::Ok) => {}
            Err(e) => return Err(codec(e)),
        }
    }
    Ok(out)
}

fn tiff_lzw_uncompressed(data: &mut Vec<u8>, tags: &Ifd, le: bool, root: usize) -> Result<()> {
    // tiff 0.11's streaming LZW reader loses buffered output for some small edge tiles.
    // Decode each chunk completely with a hard 1 GiB cap and replace only in-memory fields.
    const MAX_TIFF_BYTES: usize = 1024 * 1024 * 1024;
    let (offset_tag, count_tag) = if tags.contains_key(&324) { (324, 325) } else { (273, 279) };
    let offsets = tags.get(&offset_tag).ok_or_else(|| invalid("missing LZW offsets"))?.numbers();
    let lengths = tags.get(&count_tag).ok_or_else(|| invalid("missing LZW lengths"))?.numbers();
    if offsets.len() != lengths.len() {
        return Err(invalid("LZW chunk counts"));
    }
    let mut new_offsets = vec![];
    let mut new_lengths = vec![];
    for (offset, n) in offsets.iter().zip(lengths) {
        let compressed = crate::metadata::get(data, *offset as usize, n as usize)?;
        let remaining = MAX_TIFF_BYTES.checked_sub(data.len()).ok_or_else(|| invalid("LZW allocation limit"))?;
        let decoded = decode_lzw_bounded(compressed, remaining)?;
        new_offsets.push(u32::try_from(data.len()).map_err(codec)?);
        new_lengths.push(u32::try_from(decoded.len()).map_err(codec)?);
        data.extend(decoded);
    }
    let count = u16_at(data, root, le)? as usize;
    for i in 0..count {
        let at = root.checked_add(2).and_then(|v| v.checked_add(i * 12)).ok_or_else(|| invalid("TIFF IFD overflow"))?;
        if at.checked_add(12).is_none_or(|end| end > data.len()) {
            return Err(invalid("truncated TIFF IFD"));
        }
        let tag = u16_at(data, at, le)?;
        if tag == 259 {
            data[at + 8..at + 10].copy_from_slice(&if le { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() });
        }
        let values = if tag == offset_tag {
            Some(&new_offsets)
        } else if tag == count_tag {
            Some(&new_lengths)
        } else {
            None
        };
        if let Some(values) = values {
            data[at + 2..at + 4].copy_from_slice(&if le { 4u16.to_le_bytes() } else { 4u16.to_be_bytes() });
            let pointer = if values.len() == 1 {
                values[0]
            } else {
                let pointer = u32::try_from(data.len()).map_err(codec)?;
                for v in values {
                    data.extend(if le { v.to_le_bytes() } else { v.to_be_bytes() });
                }
                pointer
            };
            data[at + 8..at + 12].copy_from_slice(&if le { pointer.to_le_bytes() } else { pointer.to_be_bytes() });
        }
    }
    Ok(())
}
fn decode_tiff(data: &[u8]) -> Result<DecodedBitmap> {
    let exif = parse_exif(data)?;
    let tags = &exif.root;
    let photo = tag_num(tags, 262, 1.) as u16;
    let bits = tag_num(tags, 258, 8.) as u8;
    let planar = tag_num(tags, 284, 1.) == 2.;
    let floating = tag_num(tags, 339, 1.) == 3.;
    let channels = tag_num(tags, 277, 1.) as usize;
    if channels > 5 {
        return Err(Error::Unsupported("multispectral TIFF".into()));
    }
    // Disable decoder-side WhiteIsZero/palette/Lab transforms; the oracle works on native samples.
    let mut source = data.to_vec();
    let le = data.starts_with(b"II");
    let root = u32_at(data, 4, le)? as usize;
    if tag_num(tags, 259, 1.) == 5. {
        tiff_lzw_uncompressed(&mut source, tags, le, root)?;
    }
    if matches!(photo, 0 | 3 | 8 | 9) {
        let n = u16_at(data, root, le)? as usize;
        for i in 0..n {
            let at =
                root.checked_add(2).and_then(|v| v.checked_add(i * 12)).ok_or_else(|| invalid("TIFF IFD overflow"))?;
            if at.checked_add(12).is_none_or(|end| end > source.len()) {
                return Err(invalid("truncated TIFF IFD"));
            }
            if u16_at(data, at, le)? == 262 {
                source[at + 8..at + 10].copy_from_slice(&if le { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() });
            }
        }
    }
    let mut decoder = tiff::decoder::Decoder::new(Cursor::new(source)).map_err(codec)?;
    let (w, h) = decoder.dimensions().map_err(codec)?;
    let mut decoded = tiff::decoder::DecodingResult::U8(vec![]);
    decoder.read_image_to_buffer(&mut decoded).map_err(codec)?;
    use tiff::decoder::DecodingResult as D;
    let values: Vec<f32> = match decoded {
        D::U8(v) => v.into_iter().map(f32::from).collect(),
        D::U16(v) => v.into_iter().map(f32::from).collect(),
        D::U32(v) => v.into_iter().map(|v| v as f32).collect(),
        D::U64(v) => v.into_iter().map(|v| v as f32).collect(),
        D::F32(v) => v,
        D::F64(v) => v.into_iter().map(|v| v as f32).collect(),
        D::F16(v) => v.into_iter().map(|v| v.to_f32()).collect(),
        _ => return Err(Error::Unsupported("signed TIFF samples".into())),
    };
    let mut samples = shape(h as usize, w as usize, channels, values)?;
    if planar {
        let data = samples.into_raw_vec_and_offset().0;
        samples = Image::from_shape_fn((h as usize, w as usize, channels), |(y, x, c)| {
            data[c * w as usize * h as usize + y * w as usize + x]
        });
    }
    let model = match photo {
        0 | 1 => ColorModel::Gray,
        2 | 3 | 6 => ColorModel::Rgb,
        5 => ColorModel::Cmyk,
        8 | 9 => ColorModel::Lab,
        _ => return Err(Error::Unsupported(format!("TIFF photometric {photo}"))),
    };
    let mut out = base(samples, model, &format!("TIFF {bits}-bit{}", if floating { " float" } else { "" }), bits);
    out.divisor = if floating { 1. } else { ((2f64).powi(bits as i32) - 1.) as f32 };
    out.linear = floating && model != ColorModel::Lab;
    out.orientation = tag_num(tags, 274, 1.) as u16;
    let extras = tags.get(&338).map(|e| e.numbers()).unwrap_or_default();
    let base_channels = channels.saturating_sub(extras.len());
    if let Some((i, kind)) = extras.iter().enumerate().find(|(_, v)| **v == 1. || **v == 2.) {
        out.associated = *kind == 1.;
        if out.associated && !matches!(photo, 1 | 2) {
            return Err(Error::Unsupported("associated alpha in non-RGB/gray TIFF".into()));
        }
        let count = match model {
            ColorModel::Gray => 1,
            ColorModel::Cmyk => 4,
            _ => 3,
        };
        if base_channels < count {
            return Err(invalid("TIFF missing channels"));
        }
        out.samples = Image::from_shape_fn((h as usize, w as usize, count + 1), |(y, x, c)| {
            out.samples[[y, x, if c == count { base_channels + i } else { c }]]
        });
    } else {
        let count = match model {
            ColorModel::Gray => 1,
            ColorModel::Cmyk => 4,
            _ => 3,
        };
        if photo != 3 && channels < count {
            return Err(invalid("TIFF missing channels"));
        }
        if channels > count {
            out.samples = out.samples.slice(ndarray::s![.., .., ..count]).to_owned();
        }
    }
    out.white_is_zero = photo == 0;
    if matches!(photo, 3 | 6 | 8 | 9) {
        out.native_samples = Some(out.samples.clone());
    }
    if photo == 3 {
        let palette = tags.get(&320).ok_or_else(|| invalid("TIFF missing palette"))?.numbers();
        let len = palette.len() / 3;
        if len == 0 {
            return Err(invalid("empty TIFF palette"));
        }
        if out.samples.iter().any(|v| *v < 0. || *v as usize >= len) {
            return Err(invalid("TIFF palette index"));
        }
        out.samples = Image::from_shape_fn((h as usize, w as usize, 3), |(y, x, c)| {
            palette[c * len + out.samples[[y, x, 0]] as usize] as f32
        });
        out.divisor = 65535.;
        out.info.insert("bit_depth".into(), json!(16));
    }
    if photo == 5 && tag_num(tags, 332, 1.) != 1. {
        return Err(Error::Unsupported("non-CMYK TIFF ink set".into()));
    }
    if matches!(photo, 8 | 9) && !floating {
        let div = out.divisor;
        for mut row in out.samples.outer_iter_mut() {
            for mut p in row.outer_iter_mut() {
                p[0] = p[0] / div * 100.;
                for c in 1..3 {
                    p[c] = if photo == 8 {
                        let signed =
                            if p[c] >= 2f32.powi(bits as i32 - 1) { p[c] - 2f32.powi(bits as i32) } else { p[c] };
                        signed / 2f32.powi((bits as i32 - 8).max(0))
                    } else {
                        p[c] / div * 255. - 128.
                    };
                }
            }
        }
        out.divisor = 1.;
    }
    if photo == 6 && !matches!(tag_num(tags, 259, 1.) as u16, 6 | 7 | 33007 | 34892) {
        let coef = tags.get(&529).map(|e| e.numbers()).unwrap_or(vec![0.299, 0.587, 0.114]);
        let max = out.divisor;
        let refs = tags.get(&532).map(|e| e.numbers()).unwrap_or(vec![
            0.,
            max as f64,
            2f64.powi(bits as i32 - 1),
            max as f64,
            2f64.powi(bits as i32 - 1),
            max as f64,
        ]);
        if coef.len() != 3 || refs.len() != 6 || coef[1] <= 0. {
            return Err(invalid("TIFF YCbCr tags"));
        }
        for mut row in out.samples.outer_iter_mut() {
            for mut p in row.outer_iter_mut() {
                // Preserve the oracle's normalize-then-rescale float32 rounding before YCbCr conversion.
                for c in 0..3 {
                    p[c] = p[c] / max * max;
                }
                let y = (p[0] - refs[0] as f32) / (refs[1] - refs[0]).max(1.) as f32;
                let cb = (p[1] - refs[2] as f32) / (refs[3] - refs[2]).max(1.) as f32 * 0.5;
                let cr = (p[2] - refs[4] as f32) / (refs[5] - refs[4]).max(1.) as f32 * 0.5;
                let r = y + cr * (2. - 2. * coef[0]) as f32;
                let b = y + cb * (2. - 2. * coef[2]) as f32;
                let g = (y - coef[0] as f32 * r - coef[2] as f32 * b) / coef[1] as f32;
                p[0] = r;
                p[1] = g;
                p[2] = b;
            }
        }
        out.divisor = 1.;
    }
    if let Some(e) = tags.get(&34675) {
        out.profile = Some(Profile::Embedded(e.data.clone()));
    } else if matches!(model, ColorModel::Gray | ColorModel::Rgb)
        && [318, 319, 301].iter().any(|t| tags.contains_key(t))
    {
        let white = tags.get(&318).map(|e| e.numbers()).unwrap_or(vec![0.3127, 0.329]);
        let prim = tags.get(&319).map(|e| e.numbers()).unwrap_or(vec![0.64, 0.33, 0.30, 0.60, 0.15, 0.06]);
        if white.len() != 2 || prim.len() != 6 || white.iter().chain(&prim).any(|v| !v.is_finite()) {
            return Err(invalid("TIFF chromaticities"));
        }
        out.profile = Some(Profile::Calibrated {
            model,
            white: [white[0], white[1]],
            primaries: if model == ColorModel::Rgb {
                Some([prim[0], prim[1], prim[2], prim[3], prim[4], prim[5]])
            } else {
                None
            },
            gamma: None,
            transfer: tags
                .get(&301)
                .map(|e| e.numbers().iter().map(|v| *v as f32 / 65535.).collect())
                .unwrap_or_default(),
        });
    }
    let mut pages = 1;
    while decoder.more_images() {
        decoder.next_image().map_err(codec)?;
        pages += 1;
        if pages > 65535 {
            return Err(invalid("too many TIFF pages"));
        }
    }
    if pages > 1 {
        out.info.insert("pages".into(), json!(pages));
    }
    Ok(out)
}

fn webp_buffer_len(len: Option<usize>) -> Result<usize> {
    len.filter(|&n| n <= 512 * 1024 * 1024).ok_or_else(|| invalid("WebP too large"))
}

#[test]
fn webp_allocation_is_bounded() {
    assert!(webp_buffer_len(Some(512 * 1024 * 1024 + 1)).is_err());
    assert_eq!(webp_buffer_len(Some(240 * 160 * 3)).unwrap(), 240 * 160 * 3);
}

/// Port of `luma/rasterio.py read_bitmap`'s decode stage, including Pillow-compatible JPEG draft scaling.
pub fn decode_bitmap(path: impl AsRef<Path>, options: &LoadOptions) -> Result<DecodedBitmap> {
    let path = path.as_ref();
    let data = std::fs::read(path)?;
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    if ext == "avif" {
        return modern::avif(&data);
    }
    if ext == "jxl" {
        return modern::jxl(&data);
    }
    if ext == "png" {
        return decode_png(&data);
    }
    if matches!(ext.as_str(), "tiff" | "tif") {
        return decode_tiff(&data);
    }
    if data.starts_with(b"\xff\xd8") {
        return decode_jpeg(&data, options, &ext);
    }
    if data.starts_with(b"RIFF") {
        let mut d = image_webp::WebPDecoder::new(Cursor::new(&data)).map_err(codec)?;
        let (w, h) = d.dimensions();
        let c = if d.has_alpha() { 4 } else { 3 };
        let len = webp_buffer_len(d.output_buffer_size())?;
        let mut bytes = vec![0; len];
        d.read_image(&mut bytes).map_err(codec)?;
        let mut out = base(from_bytes(h as usize, w as usize, c, bytes)?, ColorModel::Rgb, "WEBP · 8-bit", 8);
        out.orientation = source_orientation(&data);
        out.profile =
            webp_chunks(&data)?.into_iter().find(|(k, _)| k == b"ICCP").map(|(_, p)| Profile::Embedded(p.to_vec()));
        return Ok(out);
    }
    if data.starts_with(b"BM") {
        let im = image::load_from_memory_with_format(&data, image::ImageFormat::Bmp).map_err(codec)?.into_rgb8();
        let (w, h) = im.dimensions();
        let dib = u32_at(&data, 14, true)? as usize;
        let bits = u16_at(&data, if dib == 12 { 24 } else { 28 }, true)?;
        let mut gray = false;
        let mut boolean = false;
        if bits <= 8 {
            let count = if dib == 12 {
                1usize << bits
            } else {
                let n = u32_at(&data, 46, true)? as usize;
                if n == 0 {
                    1usize << bits
                } else {
                    n
                }
            };
            let stride = if dib == 12 { 3 } else { 4 };
            let palette = crate::metadata::get(&data, 14 + dib, count * stride)?;
            gray = (0..count).all(|i| {
                palette[i * stride..i * stride + 3] == [if count == 2 { (i * 255) as u8 } else { i as u8 }; 3]
            });
            boolean = gray && count == 2;
        }
        let mut out = if gray {
            let samples = shape(
                h as usize,
                w as usize,
                1,
                im.pixels().map(|p| if boolean { f32::from(p[0] != 0) } else { p[0] as f32 }).collect(),
            )?;
            base(samples, ColorModel::Gray, "BMP · 8-bit", 8)
        } else {
            base(from_bytes(h as usize, w as usize, 3, im.into_raw())?, ColorModel::Rgb, "BMP · 8-bit", 8)
        };
        if boolean {
            out.divisor = 1.;
        }
        return Ok(out);
    }
    Err(Error::Unsupported(format!("non-bitmap format: {ext}")))
}
fn orient(a: &Image, n: u16) -> Image {
    let (h, w, c) = a.dim();
    let turned = (5..=8).contains(&n);
    Image::from_shape_fn((if turned { w } else { h }, if turned { h } else { w }, c), |(y, x, k)| {
        let (y, x) = match n {
            2 => (y, w - 1 - x),
            3 => (h - 1 - y, w - 1 - x),
            4 => (h - 1 - y, x),
            5 => (x, y),
            6 => (h - 1 - x, y),
            7 => (h - 1 - x, w - 1 - y),
            8 => (x, w - 1 - y),
            _ => (y, x),
        };
        a[[y, x, k]]
    })
}
/// Port of `luma/engine.py load_image` for non-RAW files, including `rasterio._finish/_linear`.
pub fn load_image(path: impl AsRef<Path>, options: &LoadOptions, cms: &impl ColorManager) -> Result<LoadedImage> {
    load_image_with_decoder(path, options, cms, |path, options| decode_bitmap(path, options))
}

/// Port of `luma/engine.py load_image` with an injected bitmap decoder, allowing external
/// backends without changing normalization, metadata, orientation, colour management or resize semantics.
pub fn load_image_with_decoder(
    path: impl AsRef<Path>,
    options: &LoadOptions,
    cms: &impl ColorManager,
    decode: impl FnOnce(&Path, &LoadOptions) -> Result<DecodedBitmap>,
) -> Result<LoadedImage> {
    let path = path.as_ref();
    let decoded = decode(path, options)?;
    let (h, w, c) = decoded.samples.dim();
    let count = match decoded.model {
        ColorModel::Gray => 1,
        ColorModel::Cmyk => 4,
        _ => 3,
    };
    if c < count {
        return Err(invalid("missing colour channels"));
    }
    let mut samples = decoded.samples.slice(ndarray::s![.., .., ..count]).mapv(|v| v / decoded.divisor);
    if decoded.white_is_zero {
        for mut row in samples.outer_iter_mut() {
            for mut p in row.outer_iter_mut() {
                p[0] = 1. - p[0];
            }
        }
    }
    let alpha = if c == count + 1 {
        Some(decoded.samples.slice(ndarray::s![.., .., count]).mapv(|v| v / decoded.divisor))
    } else {
        None
    };
    if samples.iter().any(|v| !v.is_finite()) || alpha.as_ref().is_some_and(|a| a.iter().any(|v| !v.is_finite())) {
        return Err(invalid("non-finite source samples"));
    }
    if decoded.associated {
        if let Some(a) = &alpha {
            for ((y, x, _), v) in samples.indexed_iter_mut() {
                *v = if a[[y, x]] > 0. { *v / a[[y, x]] } else { 0. };
            }
        }
    }
    let wide = options.working_space == "ProPhoto";
    let destination = Profile::Named(if wide { "Linear ProPhoto" } else { "Linear sRGB" }.into());
    let mut info = decoded.info;
    let mut rgb = if let Some(source) = &decoded.profile {
        let source = cms.profile_bytes(source)?;
        let (model, description) = cms.inspect(&source)?;
        if model != decoded.model {
            return Err(invalid("ICC profile/channel mismatch"));
        }
        info.insert(
            "input_profile".into(),
            json!(if description.is_empty() { format!("{:?}", model).to_uppercase() } else { description }),
        );
        if model == ColorModel::Cmyk {
            samples *= 100.;
        }
        cms.convert(&samples, model, &source, &cms.profile_bytes(&destination)?)?
    } else if decoded.model == ColorModel::Lab {
        cms.convert(
            &samples,
            ColorModel::Lab,
            &cms.profile_bytes(&Profile::Named("Lab4".into()))?,
            &cms.profile_bytes(&destination)?,
        )?
    } else {
        let values = match decoded.model {
            ColorModel::Cmyk => {
                info.insert("input_profile".into(), json!("태그 없는 CMYK · 근사 RGB 변환"));
                Image::from_shape_fn((h, w, 3), |(y, x, c)| (1. - samples[[y, x, c]]) * (1. - samples[[y, x, 3]]))
            }
            ColorModel::Gray => Image::from_shape_fn((h, w, 3), |(y, x, _)| samples[[y, x, 0]]),
            _ => samples,
        };
        info.entry("input_profile").or_insert(json!(if decoded.linear {
            "태그 없는 선형 RGB"
        } else {
            "태그 없는 sRGB"
        }));
        let values = if decoded.linear { values } else { grainy_color::to_linear(&values) };
        if wide {
            cms.convert(
                &values,
                ColorModel::Rgb,
                &cms.profile_bytes(&Profile::Named("Linear sRGB".into()))?,
                &cms.profile_bytes(&destination)?,
            )?
        } else {
            values
        }
    };
    if rgb.dim() != (h, w, 3) {
        return Err(invalid("CMS returned wrong image shape"));
    }
    validate(&rgb)?;
    if let Some(alpha) = alpha {
        for ((y, x, _), v) in rgb.indexed_iter_mut() {
            let a = alpha[[y, x]].clamp(0., 1.);
            *v = *v * a + (1. - a);
        }
        info.insert("alpha".into(), json!("흰색 배경 · 선형광 합성"));
    }
    rgb = orient(&rgb, decoded.orientation);
    let (fw, fh) = decoded.full_size;
    let (fw, fh) = if (5..=8).contains(&decoded.orientation) { (fh, fw) } else { (fw, fh) };
    info.insert("width".into(), json!(fw));
    info.insert("height".into(), json!(fh));
    info.insert("source_orientation".into(), json!(decoded.orientation));
    if let Some(max) = options.max_size {
        rgb = crate::resize_float(&rgb, max)?;
    }
    let stat = std::fs::metadata(path)?;
    let raw_canon = std::fs::canonicalize(path)?.to_string_lossy().into_owned();
    let canonical = if let Some(unc) = raw_canon.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        raw_canon.trim_start_matches(r"\\?\").to_string()
    };
    let mtime = stat.modified()?.duration_since(UNIX_EPOCH).map_err(codec)?.as_nanos();
    let mut metadata = crate::read_metadata(path)?;
    metadata.extend(info);
    metadata.insert("filename".into(), json!(path.file_name().unwrap_or_default().to_string_lossy()));
    metadata.insert("format".into(), json!(path.extension().unwrap_or_default().to_string_lossy().to_uppercase()));
    metadata.insert("size_mb".into(), json!((stat.len() as f64 / 1048576. * 100.).round_ties_even() / 100.));
    metadata.insert(
        "file_fingerprint".into(),
        json!({"path":canonical,"size":stat.len(),"mtime_ns":u64::try_from(mtime).map_err(codec)?}),
    );
    metadata.insert("working_space".into(), json!(options.working_space));
    Ok(LoadedImage { pixels: rgb, metadata })
}
/// Port of `luma/engine.py thumbnail`: linear load, sRGB encode, clip and truncate (no +0.5).
pub fn thumbnail(
    path: impl AsRef<Path>,
    size: usize,
    cms: &impl ColorManager,
) -> Result<(ndarray::Array3<u8>, Metadata)> {
    let image = load_image(path, &LoadOptions { max_size: Some(size), ..Default::default() }, cms)?;
    let encoded = grainy_color::to_srgb(&image.pixels);
    Ok((encoded.mapv(|v| (v.clamp(0., 1.) * 255.) as u8), image.metadata))
}
