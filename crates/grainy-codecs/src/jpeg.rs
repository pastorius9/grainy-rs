//! Pillow JPEG save calls from `luma/engine.py export_image`.
use crate::{codec, Result};

pub(crate) fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    gray: bool,
    quality: f64,
    icc: &[u8],
    exif: Option<&[u8]>,
) -> Result<Vec<u8>> {
    grainy_jpeg_sys::encode(
        pixels,
        &grainy_jpeg_sys::EncodeOptions {
            width,
            height,
            channels: if gray { 1 } else { 3 },
            quality: quality as i32,
            icc,
            exif: exif.unwrap_or_default(),
        },
    )
    .map_err(codec)
}
