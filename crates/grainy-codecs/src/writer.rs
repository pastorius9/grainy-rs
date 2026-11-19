use crate::metadata::{get, u32_at};
use crate::pixels::box_bytes;
use crate::{codec, invalid, validate, ColorManager, ColorModel, Error, Metadata, Profile, Result, SizeLimit};
use grainy_core::Image;
use ndarray::Array3;
use std::{
    io::{Cursor, Write},
    path::Path,
};

/// Formats accepted by the developed-pixel port of `luma/engine.py export_image`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// JPEG, quality converted to integer, 4:4:4 subsampling.
    Jpeg,
    /// 8-bit PNG.
    Png,
    /// 16-bit RGB PNG.
    Png16,
    /// Uncompressed 8-bit TIFF.
    Tiff8,
    /// Uncompressed 16-bit TIFF.
    Tiff16,
    /// Scene-linear float32 TIFF; requires hdr_enabled and a linear input buffer.
    TiffHdr32,
    /// AVIF, 8-bit 4:4:4, speed 6.
    Avif,
    /// JPEG XL; built-in encoding requires explicit lossless mode (see jxl_lossless).
    JpegXl,
}
/// Options for the file-writing portion of `luma/engine.py export_image`.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Encoder and output bit depth.
    pub format: ExportFormat,
    /// Oracle quality; JPEG and AVIF use its integer part.
    pub quality: f64,
    /// Optional downscaling box or longest edge.
    pub longest: Option<SizeLimit>,
    /// Working space of the supplied developed, sRGB-curve-encoded pixels.
    pub working_space: String,
    /// Destination oracle ICC name, or a filename supported by the injected CMS.
    pub color_space: String,
    /// Copy source EXIF and write user metadata blocks.
    pub keep_metadata: bool,
    /// User metadata overrides.
    pub user_metadata: Metadata,
    /// Comma-separated IPTC/XMP keywords.
    pub keywords: String,
    /// Detect a neutral sRGB output and use one channel for supported formats.
    pub grayscale: bool,
    /// Fixed TPDF noise for 8-bit outputs only.
    pub dither: bool,
    /// HDR opt-in; input pixels for TiffHdr32 must already be scene-linear.
    pub hdr_enabled: bool,
    /// Explicit alternative to libjxl's unsupported distance mode; never silently ignores quality.
    pub jxl_lossless: bool,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            quality: 95.,
            longest: None,
            working_space: "sRGB".into(),
            color_space: "sRGB".into(),
            keep_metadata: false,
            user_metadata: Metadata::new(),
            keywords: String::new(),
            grayscale: false,
            dither: false,
            hdr_enabled: false,
            jxl_lossless: false,
        }
    }
}

#[allow(clippy::too_many_arguments)] // The codec's dimensions, mode and three independent metadata blocks.
fn encode_png(
    pixels: &[u8],
    w: u32,
    h: u32,
    gray: bool,
    sixteen: bool,
    icc: &[u8],
    exif: Option<&[u8]>,
    xmp: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let _timing = grainy_core::timing::Span::new("export.png_encode");
    let mut data = vec![];
    {
        let mut encoder = png::Encoder::new(&mut data, w, h);
        encoder.set_color(if gray { png::ColorType::Grayscale } else { png::ColorType::Rgb });
        encoder.set_depth(if sixteen { png::BitDepth::Sixteen } else { png::BitDepth::Eight });
        // PNG16 export is a lossless pixel contract, not a compressed-byte contract.
        // fdeflate avoids the dominant balanced-DEFLATE cost on camera exports.
        if sixteen {
            encoder.set_compression(png::Compression::Fast);
        }
        let mut writer = encoder.write_header().map_err(codec)?;
        let mut zip = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::new(6));
        zip.write_all(icc)?;
        let mut payload = b"Luma color\0\0".to_vec();
        payload.extend(zip.finish()?);
        writer.write_chunk(png::chunk::ChunkType(*b"iCCP"), &payload).map_err(codec)?;
        if let Some(exif) = exif {
            writer
                .write_chunk(png::chunk::ChunkType(*b"eXIf"), exif.strip_prefix(b"Exif\0\0").unwrap_or(exif))
                .map_err(codec)?;
        }
        if let Some(xmp) = xmp {
            let mut payload = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
            payload.extend(xmp);
            writer.write_chunk(png::chunk::ChunkType(*b"iTXt"), &payload).map_err(codec)?;
        }
        writer.write_image_data(pixels).map_err(codec)?;
        writer.finish().map_err(codec)?;
    }
    Ok(data)
}
fn encode_jpeg(
    pixels: &[u8],
    w: u32,
    h: u32,
    gray: bool,
    quality: f64,
    icc: &[u8],
    exif: Option<&[u8]>,
) -> Result<Vec<u8>> {
    crate::jpeg::encode(pixels, w, h, gray, quality, icc, exif)
}
struct Blocks<'a> {
    icc: &'a [u8],
    exif: Option<&'a [u8]>,
    xmp: Option<&'a [u8]>,
    iptc: Option<&'a [u8]>,
}
fn encode_tiff<T: tiff::encoder::colortype::ColorType>(
    pixels: &[T::Inner],
    w: u32,
    h: u32,
    b: &Blocks<'_>,
) -> Result<Vec<u8>>
where
    [T::Inner]: tiff::encoder::TiffValue,
{
    let mut cursor = Cursor::new(vec![]);
    {
        let mut tiff = tiff::encoder::TiffEncoder::new(&mut cursor).map_err(codec)?;
        let mut image = tiff.new_image::<T>(w, h).map_err(codec)?;
        image.encoder().write_tag(tiff::tags::Tag::Unknown(34675), b.icc).map_err(codec)?;
        if let Some(xmp) = b.xmp {
            image.encoder().write_tag(tiff::tags::Tag::Unknown(700), xmp).map_err(codec)?;
        }
        if let Some(iptc) = b.iptc {
            image.encoder().write_tag(tiff::tags::Tag::Unknown(33723), iptc).map_err(codec)?;
        }
        image.write_data(pixels).map_err(codec)?;
    }
    if let Some(exif) = b.exif {
        crate::append_tiff_exif(&mut cursor, exif)?;
    }
    Ok(cursor.into_inner())
}

fn boxes(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    let mut pos = 0;
    let mut out = vec![];
    while pos < data.len() {
        let n = u32_at(data, pos, false)? as usize;
        if n < 8 {
            return Err(invalid("unsupported BMFF box length"));
        }
        let k = get(data, pos + 4, 4)?.try_into().map_err(codec)?;
        out.push((k, get(data, pos + 8, n - 8)?));
        pos += n;
    }
    Ok(out)
}
fn fullbox(kind: &[u8; 4], version: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let mut p = vec![version, 0, 0, 0];
    p.extend(payload);
    box_bytes(kind, &p)
}
fn infe(id: u16, kind: &[u8; 4], content: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut p = id.to_be_bytes().to_vec();
    p.extend([0, 0]);
    p.extend(kind);
    p.push(0);
    if let Some(v) = content {
        p.extend(v);
        p.extend([0, 0]);
    }
    fullbox(b"infe", 2, &p)
}

fn encode_avif(pixels: &Array3<u8>, quality: f64, b: &Blocks<'_>) -> Result<Vec<u8>> {
    let (h, w, _) = pixels.dim();
    let rgb: Vec<ravif::RGB8> = pixels
        .outer_iter()
        .flat_map(|r| r.outer_iter().map(|p| ravif::RGB8::new(p[0], p[1], p[2])).collect::<Vec<_>>())
        .collect();
    let encoded = ravif::Encoder::new()
        .with_quality(quality as f32)
        .with_speed(6)
        .with_bit_depth(ravif::BitDepth::Eight)
        .encode_rgb(ravif::Img::new(&rgb, w, h))
        .map_err(codec)?;
    let top = boxes(&encoded.avif_file)?;
    let av1 = top.iter().find(|(k, _)| k == b"mdat").map(|(_, p)| *p).ok_or_else(|| invalid("ravif missing mdat"))?;
    let meta = top.iter().find(|(k, _)| k == b"meta").map(|(_, p)| *p).ok_or_else(|| invalid("ravif missing meta"))?;
    let meta = boxes(get(meta, 4, meta.len().saturating_sub(4))?)?;
    let iprp = meta.iter().find(|(k, _)| k == b"iprp").map(|(_, p)| *p).ok_or_else(|| invalid("ravif missing iprp"))?;
    let iprp = boxes(iprp)?;
    let ipco = iprp.iter().find(|(k, _)| k == b"ipco").map(|(_, p)| *p).ok_or_else(|| invalid("ravif missing ipco"))?;
    let props = boxes(ipco)?;
    let mut properties = vec![];
    for (k, p) in props {
        if matches!(&k, b"ispe" | b"pixi" | b"av1C" | b"colr") {
            properties.push(box_bytes(&k, p)?);
        }
    }
    let mut icc = b"prof".to_vec();
    icc.extend(b.icc);
    properties.push(box_bytes(b"colr", &icc)?);
    // Rebuild a simple one-image HEIF container so ICC, EXIF and XMP are all first-class items.
    let mut items = vec![(1u16, *b"av01", av1.to_vec())];
    if let Some(exif) = b.exif {
        let mut p = vec![0; 4];
        p.extend(exif.strip_prefix(b"Exif\0\0").unwrap_or(exif));
        items.push((2, *b"Exif", p));
    }
    if let Some(xmp) = b.xmp {
        items.push((3, *b"mime", xmp.to_vec()));
    }
    let ftyp = box_bytes(b"ftyp", b"avif\0\0\0\0avifmif1miafMA1A")?;
    let build_meta = |start: u32| -> Result<Vec<u8>> {
        let mut body = vec![];
        let mut hdlr = vec![0; 4];
        hdlr.extend(b"pict");
        hdlr.extend([0; 12]);
        hdlr.push(0);
        body.extend(fullbox(b"hdlr", 0, &hdlr)?);
        body.extend(fullbox(b"pitm", 0, &1u16.to_be_bytes())?);
        let mut loc = vec![0x44, 0];
        loc.extend((items.len() as u16).to_be_bytes());
        let mut offset = start;
        for (id, _, p) in &items {
            loc.extend(id.to_be_bytes());
            loc.extend([0, 0]);
            loc.extend(1u16.to_be_bytes());
            loc.extend(offset.to_be_bytes());
            loc.extend(u32::try_from(p.len()).map_err(codec)?.to_be_bytes());
            offset = offset.checked_add(p.len() as u32).ok_or_else(|| invalid("AVIF exceeds 4 GiB"))?;
        }
        body.extend(fullbox(b"iloc", 0, &loc)?);
        let mut iinf = (items.len() as u16).to_be_bytes().to_vec();
        for (id, k, _) in &items {
            iinf.extend(infe(*id, k, if k == b"mime" { Some(b"application/rdf+xml") } else { None })?);
        }
        body.extend(fullbox(b"iinf", 0, &iinf)?);
        let mut rp = box_bytes(b"ipco", &properties.concat())?;
        let mut assoc = 1u32.to_be_bytes().to_vec();
        assoc.extend(1u16.to_be_bytes());
        assoc.push(properties.len() as u8);
        for (i, p) in properties.iter().enumerate() {
            assoc.push((i + 1) as u8 | if &p[4..8] == b"av1C" { 128 } else { 0 });
        }
        rp.extend(fullbox(b"ipma", 0, &assoc)?);
        body.extend(box_bytes(b"iprp", &rp)?);
        if items.len() > 1 {
            let mut refs = vec![];
            for (id, _, _) in &items[1..] {
                let mut p = id.to_be_bytes().to_vec();
                p.extend(1u16.to_be_bytes());
                p.extend(1u16.to_be_bytes());
                refs.extend(box_bytes(b"cdsc", &p)?);
            }
            body.extend(fullbox(b"iref", 0, &refs)?);
        }
        fullbox(b"meta", 0, &body)
    };
    let meta = build_meta(0)?;
    let start = u32::try_from(ftyp.len() + meta.len() + 8).map_err(codec)?;
    let mut out = ftyp;
    out.extend(build_meta(start)?);
    out.extend(box_bytes(b"mdat", &items.into_iter().flat_map(|(_, _, v)| v).collect::<Vec<_>>())?);
    Ok(out)
}

/// Port of `luma/engine.py export_image` after development: convert, size, quantize, exclusive-create and write.
/// Ordinary input is encoded working RGB, as returned by develop(output_space=None). HDR input is linear.
/// JPEG XL distance encoding is explicitly unavailable; opt into its pure Rust lossless alternative.
pub fn export_image(
    developed: &Image,
    source: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &ExportOptions,
    cms: &impl ColorManager,
) -> Result<(usize, usize)> {
    let _timing = grainy_core::timing::Span::new("export.encode_write");
    validate(developed)?;
    if developed.shape()[2] != 3 {
        return Err(invalid("export needs RGB pixels"));
    }
    if !options.quality.is_finite() || !(0. ..=100.).contains(&options.quality) {
        return Err(invalid("quality outside 0..100"));
    }
    if options.format == ExportFormat::JpegXl && options.color_space != "sRGB" {
        return Err(invalid("JPEG XL only supports sRGB"));
    }
    if options.format == ExportFormat::JpegXl && !options.jxl_lossless {
        return Err(Error::Unsupported(format!(
            "JPEG XL distance {} / effort 7 needs libjxl; set jxl_lossless explicitly for zune-jpegxl",
            crate::jxl_distance(options.quality)?
        )));
    }
    let timing = grainy_core::timing::Span::new("export.prepare");
    let hdr = options.format == ExportFormat::TiffHdr32;
    if hdr && !options.hdr_enabled {
        return Err(invalid("HDR TIFF requires HDR enabled"));
    }
    let source = source.as_ref();
    let output = output.as_ref();
    if output.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "export destination exists").into());
    }
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(source), std::fs::canonicalize(output)) {
        if a == b {
            return Err(invalid("cannot overwrite source"));
        }
    }
    let mut result = if hdr {
        developed.clone()
    } else if options.working_space == "ProPhoto" || options.color_space != "sRGB" {
        let source = cms.profile_bytes(&Profile::Named(
            if options.working_space == "ProPhoto" { "Luma Wide" } else { "sRGB" }.into(),
        ))?;
        cms.convert(
            developed,
            ColorModel::Rgb,
            &source,
            &cms.profile_bytes(&Profile::Named(options.color_space.clone()))?,
        )?
    } else {
        developed.clone()
    };
    if result.dim() != developed.dim() {
        return Err(invalid("CMS export shape mismatch"));
    }
    validate(&result)?;
    if !hdr {
        grainy_color::clip01_in_place(&mut result);
    }
    drop(timing);
    if let Some(limit) = options.longest {
        let side = crate::export_longest((result.shape()[0], result.shape()[1]), limit)?;
        result = crate::resize_export(&result, side, hdr)?;
    }
    let (h, w, _) = result.dim();
    let width = u32::try_from(w).map_err(codec)?;
    let height = u32::try_from(h).map_err(codec)?;
    let gray = if options.grayscale
        && options.color_space == "sRGB"
        && matches!(options.format, ExportFormat::Jpeg | ExportFormat::Png | ExportFormat::Tiff8 | ExportFormat::Tiff16)
    {
        crate::neutral_gray(&result, 1e-4)
    } else {
        None
    };
    let frame = gray.as_ref().unwrap_or(&result);
    let name = if hdr {
        if options.working_space == "ProPhoto" {
            "Linear ProPhoto"
        } else {
            "Linear sRGB"
        }
    } else if gray.is_some() {
        "Gray sRGB"
    } else {
        &options.color_space
    };
    let icc = cms.profile_bytes(&Profile::Named(name.into()))?;
    let exif = if options.keep_metadata {
        Some(crate::export_exif(
            source,
            &options.user_metadata,
            Some((width, height)),
            if hdr { "HDR linear" } else { &options.color_space },
        )?)
    } else {
        None
    };
    let xmp = if options.keep_metadata {
        Some(crate::xmp_metadata(&options.user_metadata, &options.keywords, exif.as_deref())?)
    } else {
        None
    };
    let iptc =
        if options.keep_metadata { Some(crate::iptc_bytes(&options.user_metadata, &options.keywords)) } else { None };
    let blocks = Blocks { icc: &icc, exif: exif.as_deref(), xmp: xmp.as_deref(), iptc: iptc.as_deref() };
    let eight = |a: &Image| if options.dither { crate::quantize8_dithered(a) } else { crate::quantize8(a) };
    use tiff::encoder::colortype as ct;
    let bytes = match options.format {
        ExportFormat::Png => encode_png(
            &eight(frame)?.into_raw_vec_and_offset().0,
            width,
            height,
            gray.is_some(),
            false,
            &icc,
            exif.as_deref(),
            xmp.as_deref(),
        )?,
        ExportFormat::Png16 => {
            let timing = grainy_core::timing::Span::new("export.quantize16");
            let p: Vec<_> = crate::quantize16(&result)?.iter().flat_map(|v| v.to_be_bytes()).collect();
            drop(timing);
            encode_png(&p, width, height, false, true, &icc, exif.as_deref(), xmp.as_deref())?
        }
        ExportFormat::Jpeg => {
            let mut encoded = encode_jpeg(
                &eight(frame)?.into_raw_vec_and_offset().0,
                width,
                height,
                gray.is_some(),
                options.quality,
                &icc,
                exif.as_deref(),
            )?;
            if options.keep_metadata {
                encoded = crate::metadata::inject_iptc(&encoded, &options.user_metadata, &options.keywords)?;
            }
            encoded
        }
        ExportFormat::Tiff8 => {
            let p = eight(frame)?.into_raw_vec_and_offset().0;
            if gray.is_some() {
                encode_tiff::<ct::Gray8>(&p, width, height, &blocks)?
            } else {
                encode_tiff::<ct::RGB8>(&p, width, height, &blocks)?
            }
        }
        ExportFormat::Tiff16 => {
            let p = crate::quantize16(frame)?.into_raw_vec_and_offset().0;
            if gray.is_some() {
                encode_tiff::<ct::Gray16>(&p, width, height, &blocks)?
            } else {
                encode_tiff::<ct::RGB16>(&p, width, height, &blocks)?
            }
        }
        ExportFormat::TiffHdr32 => {
            encode_tiff::<ct::RGB32Float>(&result.iter().copied().collect::<Vec<_>>(), width, height, &blocks)?
        }
        ExportFormat::Avif => encode_avif(&eight(&result)?, options.quality.max(1.), &blocks)?,
        ExportFormat::JpegXl => {
            let p: Vec<_> = crate::quantize16(&result)?.iter().flat_map(|v| v.to_ne_bytes()).collect();
            let options = zune_core::options::EncoderOptions::new(
                w,
                h,
                zune_core::colorspace::ColorSpace::RGB,
                zune_core::bit_depth::BitDepth::Sixteen,
            );
            let mut data = vec![];
            zune_jpegxl::JxlSimpleEncoder::new(&p, options).encode(&mut data).map_err(codec)?;
            crate::jxl_container(&data, exif.as_deref(), xmp.as_deref())?
        }
    };
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let _timing = grainy_core::timing::Span::new("export.write");
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(output)?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.flush()) {
        drop(file);
        let _ = std::fs::remove_file(output);
        return Err(error.into());
    }
    Ok((w, h))
}
