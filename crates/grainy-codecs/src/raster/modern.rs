//! AVIF's native decoder and JPEG XL's Rust decoder feed the same color-managed bitmap pipeline.
use std::ffi::{c_char, CStr};
use std::ptr::NonNull;

use jxl_oxide::{AllocTracker, JxlImage, PixelFormat};

use super::{base, shape, ColorModel, DecodedBitmap, Profile};
use crate::{codec, invalid, Result};

const MAX_PIXELS: usize = 128 * 1024 * 1024;

#[repr(C)]
struct Avif {
    width: u32,
    height: u32,
    channels: u32,
    depth: u32,
    orientation: u32,
    primaries: u32,
    transfer: u32,
    alpha_premultiplied: u32,
    pixels: *const u16,
    icc: *const u8,
    icc_size: usize,
}

unsafe extern "C" {
    fn grainy_avif_read(data: *const u8, len: usize, out: *mut *mut Avif) -> *const c_char;
    fn grainy_avif_copy(image: *const Avif, out: *mut f32, count: usize) -> i32;
    fn grainy_avif_close(image: *mut Avif);
}

struct OwnedAvif(NonNull<Avif>);
impl Drop for OwnedAvif {
    fn drop(&mut self) {
        // The native allocation is exclusively owned by this handle.
        unsafe { grainy_avif_close(self.0.as_ptr()) }
    }
}

fn samples(width: usize, height: usize, channels: usize) -> Result<Vec<f32>> {
    let count = width
        .checked_mul(height)
        .filter(|n| (1..=MAX_PIXELS).contains(n))
        .and_then(|n| n.checked_mul(channels))
        .ok_or_else(|| invalid("image dimensions exceed the decoder limit"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(codec)?;
    out.resize(count, 0.0);
    Ok(out)
}

pub(crate) fn avif_exif(data: &[u8]) -> Option<Vec<u8>> {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn grainy_avif_exif(
            data: *const u8,
            len: usize,
            receive: unsafe extern "C" fn(*const u8, usize, *mut c_void),
            context: *mut c_void,
        );
    }
    unsafe extern "C" fn receive(ptr: *const u8, len: usize, context: *mut c_void) {
        // The synchronous callback borrows the live decoder's checked TIFF payload.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        unsafe { *context.cast::<Option<Vec<u8>>>() = Some(bytes.to_vec()) };
    }
    let mut result: Option<Vec<u8>> = None;
    unsafe {
        grainy_avif_exif(data.as_ptr(), data.len(), receive, (&raw mut result).cast());
    }
    result
}

pub(super) fn avif(data: &[u8]) -> Result<DecodedBitmap> {
    let mut image = std::ptr::null_mut();
    // The decoder borrows the bytes only while this function owns the native result.
    let message = unsafe { grainy_avif_read(data.as_ptr(), data.len(), &mut image) };
    if !message.is_null() {
        // libavif's error strings are zero-terminated and static.
        return Err(codec(unsafe { CStr::from_ptr(message) }.to_string_lossy()));
    }
    let owned = OwnedAvif(NonNull::new(image).ok_or_else(|| invalid("AVIF decoder returned no image"))?);
    // This C struct prefix is valid until OwnedAvif drops. No fields are written from Rust.
    let im = unsafe { owned.0.as_ref() };
    let (w, h, c) = (im.width as usize, im.height as usize, im.channels as usize);
    let mut pixels = samples(w, h, c)?;
    // Native code checks the destination's element count before writing the color samples.
    if unsafe { grainy_avif_copy(image, pixels.as_mut_ptr(), pixels.len()) } == 0 {
        return Err(invalid("unsupported AVIF color primaries"));
    }
    let icc = if im.icc_size == 0 {
        None
    } else {
        // The decoder owns exactly icc_size bytes for this profile until the handle drops.
        Some(unsafe { std::slice::from_raw_parts(im.icc, im.icc_size) }.to_owned())
    };
    let gray = icc.as_ref().is_some_and(|p| p.get(16..20) == Some(b"GRAY"));
    let pixels = shape(h, w, c, pixels)?;
    let (values, model) = if gray {
        let indices: &[usize] = if c == 4 { &[0, 3] } else { &[0] };
        (pixels.select(ndarray::Axis(2), indices), ColorModel::Gray)
    } else {
        (pixels, ColorModel::Rgb)
    };
    let mut out = base(values, model, "libavif/dav1d", im.depth as u8);
    out.divisor = 1.;
    out.orientation = im.orientation as u16;
    out.associated = im.alpha_premultiplied != 0;
    out.linear = icc.is_none();
    out.profile = icc.map(Profile::Embedded);
    Ok(out)
}

pub(super) fn jxl(data: &[u8]) -> Result<DecodedBitmap> {
    let image =
        JxlImage::builder().alloc_tracker(AllocTracker::with_limit(1024 * 1024 * 1024)).read(data).map_err(codec)?;
    if image.num_loaded_keyframes() == 0 {
        return Err(invalid("JPEG XL contains no complete frame"));
    }
    let format = image.pixel_format();
    let model = match format {
        PixelFormat::Rgb | PixelFormat::Rgba => ColorModel::Rgb,
        PixelFormat::Gray | PixelFormat::Graya => ColorModel::Gray,
        PixelFormat::Cmyk | PixelFormat::Cmyka => ColorModel::Cmyk,
    };
    let (w, h, c) = (image.width() as usize, image.height() as usize, format.channels());
    let mut values = samples(w, h, c)?;
    // The stream applies codestream orientation and outputs normalized floating-point samples.
    let render = image.render_frame(0).map_err(codec)?;
    if render.stream().write_to_buffer(&mut values) != values.len() {
        return Err(invalid("JPEG XL frame is incomplete"));
    }
    let depth = image.image_header().metadata.bit_depth.bits_per_sample() as u8;
    let mut out = base(shape(h, w, c, values)?, model, "jxl-oxide", depth);
    out.divisor = 1.;
    out.associated =
        image.image_header().metadata.ec_info.iter().find_map(|channel| channel.alpha_associated()).unwrap_or(false);
    out.profile = Some(Profile::Embedded(image.rendered_icc()));
    Ok(out)
}
