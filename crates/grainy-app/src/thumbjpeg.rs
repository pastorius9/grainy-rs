//! The JPEG packets of the thumbnail cache (`preview_store.publish`: RGB, quality 88, the colour space's ICC
//! profile) and their decoding for the screen. The bytes differ from Pillow's encoder; the cache's validity
//! (`preview_store.inspect`) depends on the stamp (token, path, source, SHA-256 of these bytes), not on the encoder,
//! so both programs read each other's thumbnails. (A public JPEG encoder in grainy-codecs would replace this:
//! see the report's requests.)

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use ndarray::Array3;

/// Serialises initial import publication with the app's thumbnail readers/rebuilders.
/// A newly visible catalogue row must not be rebuilt before its import thumbnail is stored.
pub(crate) fn publication() -> std::sync::MutexGuard<'static, ()> {
    static PUBLICATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    PUBLICATION.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `preview_store.publish`'s quality.
pub const QUALITY: u8 = 88;

/// Encodes H×W×3 bytes (sRGB or the stamp's colour space) as the cache's JPEG.
pub fn encode(pixels: &Array3<u8>, icc: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let (h, w, c) = pixels.dim();
    if c != 3 || h == 0 || w == 0 {
        return Err(format!("thumbnail of shape {h}x{w}x{c}"));
    }
    let bytes: Vec<u8> = pixels.iter().copied().collect();
    encode_rgb(&bytes, w, h, icc)
}

/// Encodes RGBA rows (the worker's thumbnails) as the cache's JPEG; alpha is dropped.
pub fn encode_rgba(rgba: &[u8], width: usize, height: usize, icc: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    if rgba.len() != width * height * 4 {
        return Err("thumbnail size".into());
    }
    let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    encode_rgb(&rgb, width, height, icc)
}

fn encode_rgb(rgb: &[u8], width: usize, height: usize, icc: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, QUALITY);
    if let Some(icc) = icc {
        // A profile the encoder cannot embed only costs the tag.
        let _ = encoder.set_icc_profile(icc);
    }
    encoder.write_image(rgb, width as u32, height as u32, ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Decodes a cached JPEG to RGBA rows: (width, height, bytes).
pub fn decode(jpeg: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let image = image::ImageReader::with_format(Cursor::new(jpeg), image::ImageFormat::Jpeg)
        .decode()
        .map_err(|e| e.to_string())?
        .to_rgba8();
    Ok((image.width() as usize, image.height() as usize, image.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pixels = Array3::from_shape_fn((24, 32, 3), |(y, x, c)| (x * 4 + y * 2 + c * 30) as u8);
        let jpeg = encode(&pixels, None).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        let (w, h, rgba) = decode(&jpeg).unwrap();
        assert_eq!((w, h, rgba.len()), (32, 24, 32 * 24 * 4));
        let largest = pixels
            .iter()
            .zip(rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]))
            .map(|(a, b)| a.abs_diff(b))
            .max();
        assert!(largest.unwrap() < 24, "quality 88 keeps the picture: {largest:?}");
    }
}
