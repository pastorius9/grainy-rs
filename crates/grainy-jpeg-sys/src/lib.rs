//! Shared libjpeg-turbo 2.1.5.1. Ports the Pillow JPEG calls made by
//! `luma/rasterio.py read_bitmap` and `luma/engine.py export_image`.
//! Native headers and namespacing are exported as `DEP_GRAINY_JPEG_INCLUDE` for LibRaw.

#[allow(unsafe_code)]
mod ffi;

/// Failure from the Pillow operations underlying `rasterio.read_bitmap` / `engine.export_image`.
#[derive(Debug)]
pub struct Error(String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

/// Samples returned by the Pillow JPEG decoder in `luma/rasterio.py read_bitmap`.
pub struct Decoded {
    /// Packed row-major L, RGB or non-inverted CMYK bytes, as `np.asarray(image)`.
    pub pixels: Vec<u8>,
    /// Width after `Image.draft`.
    pub width: u32,
    /// Height after `Image.draft`.
    pub height: u32,
    /// Native width, before `Image.draft` and EXIF orientation.
    pub full_width: u32,
    /// Native height, before `Image.draft` and EXIF orientation.
    pub full_height: u32,
    /// 1 = L, 3 = RGB, 4 = CMYK.
    pub channels: u32,
}

/// Port of `luma/rasterio.py read_bitmap`'s Pillow JPEG load/draft operation.
/// `max_size` requests a preview; draft retains at least twice that size in both axes.
/// Returns un-oriented samples; the caller handles EXIF and ICC. Rejects 12-bit JPEG.
pub fn decode(data: &[u8], max_size: Option<usize>) -> Result<Decoded, Error> {
    if max_size == Some(0) {
        return Err(Error("zero JPEG preview size".into()));
    }
    ffi::decode(data, max_size.map(|v| v.saturating_mul(2)).unwrap_or(0))
}

/// Pillow `Image.save` arguments used by `luma/engine.py export_image`.
pub struct EncodeOptions<'a> {
    /// Packed image width.
    pub width: u32,
    /// Packed image height.
    pub height: u32,
    /// One gray channel or three RGB channels.
    pub channels: u32,
    /// `int(quality)`, clamped by libjpeg to 1..100; -1 selects Pillow's default 75.
    pub quality: i32,
    /// ICC bytes, split into numbered APP2 markers (65,519 profile bytes each).
    pub icc: &'a [u8],
    /// Raw EXIF APP1 payload, including the `Exif\0\0` prefix if present.
    pub exif: &'a [u8],
}

/// Port of `luma/engine.py export_image`'s JPEG save: baseline, 4:4:4, default Huffman tables.
/// Matches Pillow's marker order and ICC chunking for already quantized gray/RGB samples.
pub fn encode(pixels: &[u8], options: &EncodeOptions<'_>) -> Result<Vec<u8>, Error> {
    let expected = (options.width as usize)
        .checked_mul(options.height as usize)
        .and_then(|v| v.checked_mul(options.channels as usize));
    if !matches!(options.channels, 1 | 3)
        || options.width == 0
        || options.height == 0
        || options.width > 65500
        || options.height > 65500
        || expected != Some(pixels.len())
        || pixels.len() > 512 * 1024 * 1024
    {
        return Err(Error("invalid JPEG pixel dimensions or allocation limit".into()));
    }
    if options.exif.len() > 65533 || options.icc.len() > 65519 * 255 {
        return Err(Error("JPEG metadata exceeds APP marker limits".into()));
    }
    ffi::encode(pixels, options)
}
