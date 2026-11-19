//! Minimal, owned C ABI for the oracle's rawpy and LittleCMS calls.
use crate::{invalid, Error, Result};
use ndarray::Array3;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{c_char, c_void, CStr},
    path::Path,
    ptr::NonNull,
};

#[repr(C)]
struct Meta {
    sizes: [u32; 8],
    pixel_aspect: f64,
    flip: i32,
    colors: i32,
    filters: u32,
    black: [u32; 4],
    white: u32,
    linear_max: [u32; 4],
    camera_wb: [f32; 4],
    daylight_wb: [f32; 4],
    rgb_xyz: [f32; 12],
    color_matrix: [f32; 12],
    iso: f32,
    shutter: f32,
    aperture: f32,
    focal: f32,
    timestamp: i64,
    color_desc: [c_char; 5],
    make: [c_char; 64],
    model: [c_char; 64],
    serial: [c_char; 64],
    lens: [c_char; 128],
}
extern "C" {
    fn gr_version() -> *const c_char;
    fn gr_error(code: i32) -> *const c_char;
    fn gr_open(path: *const c_void, meta: *mut Meta, error: *mut i32) -> *mut c_void;
    fn gr_close(handle: *mut c_void);
    fn gr_free_image(image: *mut c_void);
    fn gr_process(
        handle: *mut c_void,
        mode: i32,
        half: i32,
        pixels: *mut *const u16,
        h: *mut u32,
        w: *mut u32,
        channels: *mut u32,
        error: *mut i32,
    ) -> *mut c_void;
    fn gr_prophoto_srgb(input: *const f32, output: *mut f32, count: u32) -> i32;
}
fn decoder_error(code: i32) -> Error {
    if code == -2 {
        Error::UnsupportedRaw
    } else {
        // SAFETY: LibRaw returns a static NUL-terminated error description.
        let message = unsafe { CStr::from_ptr(gr_error(code)) }.to_string_lossy().into_owned();
        Error::LibRaw { code, message }
    }
}
fn string<const N: usize>(a: &[c_char; N]) -> String {
    let bytes: Vec<u8> = a.iter().take_while(|x| **x != 0).map(|x| *x as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
/// Port of rawpy.libraw_version: the linked decoder version.
pub fn libraw_version() -> String {
    // SAFETY: LibRaw::version returns static NUL-terminated storage.
    unsafe { CStr::from_ptr(gr_version()) }.to_string_lossy().into_owned()
}
/// Port of the rawpy metadata read by engine.load_image and rawcolor.decode.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RawMetadata {
    /// rawpy.sizes: raw_height, raw_width, height, width, top_margin, left_margin, iheight, iwidth.
    pub sizes: [u32; 8],
    /// rawpy.sizes.pixel_aspect.
    pub pixel_aspect: f64,
    /// rawpy.sizes.flip (LibRaw orientation).
    pub flip: i32,
    /// rawpy.num_colors.
    pub num_colors: i32,
    /// LibRaw's CFA filter descriptor.
    pub filters: u32,
    /// rawpy.color_desc, with the trailing NUL removed.
    pub color_desc: String,
    /// rawpy.black_level_per_channel.
    pub black_level_per_channel: [u32; 4],
    /// rawpy.white_level.
    pub white_level: u32,
    /// rawpy.camera_white_level_per_channel.
    pub camera_white_level_per_channel: Option<[u32; 4]>,
    /// rawpy.camera_whitebalance.
    pub camera_whitebalance: [f32; 4],
    /// rawpy.daylight_whitebalance.
    pub daylight_whitebalance: [f32; 4],
    /// rawpy.rgb_xyz_matrix (4 by 3).
    pub rgb_xyz_matrix: [[f32; 3]; 4],
    /// rawpy.color_matrix (3 by 4).
    pub color_matrix: [[f32; 4]; 3],
    /// engine.read_metadata make.
    pub make: String,
    /// engine.read_metadata camera.
    pub camera: String,
    /// engine.read_metadata serial.
    pub serial: String,
    /// engine.read_metadata lens.
    pub lens: String,
    /// engine.read_metadata iso.
    pub iso: f32,
    /// engine.read_metadata shutter, in seconds.
    pub shutter: f32,
    /// engine.read_metadata aperture.
    pub aperture: f32,
    /// engine.read_metadata focal_length.
    pub focal_length: f32,
    /// rawpy.other.timestamp, seconds since the Unix epoch.
    pub timestamp: i64,
}
/// The two postprocess parameter sets in engine.load_image and rawcolor.decode.
#[derive(Clone, Copy, Debug)]
pub enum DecodeMode {
    /// rawcolor.decode: unity white balance, camera RGB, adjust_maximum_thr=0.
    Camera,
    /// engine.load_image: camera WB, linear sRGB, adjust_maximum_thr=.75.
    Srgb,
    /// engine.load_image: camera WB, linear ProPhoto, adjust_maximum_thr=.75.
    ProPhoto,
}
/// Port of rawpy.imread. Owns a decoded file; each handle stays on its creating thread.
pub struct RawFile {
    handle: NonNull<c_void>,
    metadata: RawMetadata,
}
impl RawFile {
    /// Port of rawpy.imread(path), including native Windows Unicode paths.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let _timing = grainy_core::timing::Span::new("load.raw_open_unpack");
        #[cfg(windows)]
        let encoded: Vec<u16> = {
            use std::os::windows::ffi::OsStrExt;
            let mut v: Vec<u16> = path.as_ref().as_os_str().encode_wide().collect();
            if v.contains(&0) {
                return Err(invalid("NUL in RAW path"));
            }
            v.push(0);
            v
        };
        #[cfg(not(windows))]
        let encoded = {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::CString::new(path.as_ref().as_os_str().as_bytes()).map_err(|_| invalid("NUL in RAW path"))?
        };
        let mut error = 0;
        // SAFETY: Meta is a C POD. gr_open initializes it before returning a
        // successful handle. The path remains alive throughout the synchronous call.
        let mut meta = std::mem::MaybeUninit::<Meta>::uninit();
        let handle = unsafe { gr_open(encoded.as_ptr().cast(), meta.as_mut_ptr(), &mut error) };
        let handle = NonNull::new(handle).ok_or_else(|| decoder_error(error))?;
        // SAFETY: successful gr_open has fully initialized the C POD.
        let m = unsafe { meta.assume_init() };
        let metadata = RawMetadata {
            sizes: m.sizes,
            pixel_aspect: m.pixel_aspect,
            flip: m.flip,
            num_colors: m.colors,
            filters: m.filters,
            color_desc: string(&m.color_desc),
            black_level_per_channel: m.black,
            white_level: m.white,
            camera_white_level_per_channel: m.linear_max.iter().all(|x| *x > 0).then_some(m.linear_max),
            camera_whitebalance: m.camera_wb,
            daylight_whitebalance: m.daylight_wb,
            rgb_xyz_matrix: std::array::from_fn(|i| std::array::from_fn(|j| m.rgb_xyz[i * 3 + j])),
            color_matrix: std::array::from_fn(|i| std::array::from_fn(|j| m.color_matrix[i * 4 + j])),
            make: string(&m.make),
            camera: string(&m.model),
            serial: string(&m.serial),
            lens: string(&m.lens),
            iso: m.iso,
            shutter: m.shutter,
            aperture: m.aperture,
            focal_length: m.focal,
            timestamp: m.timestamp,
        };
        Ok(Self { handle, metadata })
    }
    /// Port of the rawpy metadata properties, captured before postprocessing.
    pub fn metadata(&self) -> &RawMetadata {
        &self.metadata
    }
    /// Port of raw.postprocess for the exact parameter sets used by Grainy.
    pub fn postprocess(&mut self, mode: DecodeMode, half_size: bool) -> Result<Array3<u16>> {
        let _timing = grainy_core::timing::Span::new("load.raw_postprocess");
        let mut pixels = std::ptr::null();
        let (mut h, mut w, mut c, mut error) = (0, 0, 0, 0);
        let mode = match mode {
            DecodeMode::Camera => 0,
            DecodeMode::Srgb => 1,
            DecodeMode::ProPhoto => 4,
        };
        // SAFETY: the handle is uniquely borrowed. Returned storage belongs to
        // LibRaw until gr_free_image; it is copied into an owned ndarray below.
        let image = unsafe {
            gr_process(
                self.handle.as_ptr(),
                mode,
                i32::from(half_size),
                &mut pixels,
                &mut h,
                &mut w,
                &mut c,
                &mut error,
            )
        };
        if image.is_null() {
            return Err(decoder_error(error));
        }
        struct ImageGuard(*mut c_void);
        impl Drop for ImageGuard {
            fn drop(&mut self) {
                /* SAFETY: unique LibRaw image allocation. */
                unsafe { gr_free_image(self.0) }
            }
        }
        let _guard = ImageGuard(image);
        let len = (h as usize)
            .checked_mul(w as usize)
            .and_then(|n| n.checked_mul(c as usize))
            .filter(|n| *n <= isize::MAX as usize / 2)
            .ok_or_else(|| invalid("RAW dimensions overflow"))?;
        if pixels.is_null() {
            return Err(invalid("LibRaw returned a null pixel buffer"));
        }
        // SAFETY: validated bitmap dimensions describe image->data (16-bit).
        let data = unsafe { std::slice::from_raw_parts(pixels, len) }.to_vec();
        Array3::from_shape_vec((h as usize, w as usize, c as usize), data).map_err(|e| invalid(e.to_string()))
    }
}
impl Drop for RawFile {
    fn drop(&mut self) {
        /* SAFETY: handle is owned and destroyed exactly once. */
        unsafe { gr_close(self.handle.as_ptr()) }
    }
}

pub(crate) fn prophoto_srgb(a: &grainy_core::Image) -> Result<grainy_core::Image> {
    use rayon::prelude::*;
    let _timing = grainy_core::timing::Span::new("raw.cms");
    let input = a.as_standard_layout();
    let slice = input.as_slice().ok_or_else(|| invalid("noncontiguous RGB"))?;
    let mut data = vec![0f32; slice.len()];
    // Every job owns its transform (LittleCMS transforms contain mutable state).
    // Independent pixels keep the same ICC round-trip and native arithmetic.
    data.par_chunks_mut(3 * 128 * 1024).zip(slice.par_chunks(3 * 128 * 1024)).try_for_each(|(dst, src)| {
        // SAFETY: nonoverlapping slices contain count*3 f32 samples each.
        let code = unsafe { gr_prophoto_srgb(src.as_ptr(), dst.as_mut_ptr(), (src.len() / 3) as u32) };
        if code != 0 {
            return Err(invalid("LittleCMS transform failed"));
        }
        Ok(())
    })?;
    Array3::from_shape_vec(a.raw_dim(), data).map_err(|e| invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_camera_cms_matches_serial_native_pixels() {
        let image = Array3::from_shape_fn((531, 523, 3), |(y, x, c)| {
            ((y * 701 + x * 131 + c * 17) % 65536) as f32 / 16384.0 - 0.5
        });
        let strided = image.permuted_axes([1, 0, 2]);
        let input = strided.as_standard_layout().to_owned();
        let slice = input.as_slice().unwrap();
        let mut serial = vec![0f32; slice.len()];
        // SAFETY: disjoint owned buffers each contain count*3 f32 samples.
        assert_eq!(unsafe { gr_prophoto_srgb(slice.as_ptr(), serial.as_mut_ptr(), (slice.len() / 3) as u32) }, 0);
        let expected: Vec<_> = serial.iter().map(|x| x.to_bits()).collect();
        for threads in [1, 2, 4] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let actual = pool.install(|| prophoto_srgb(&strided)).unwrap();
            assert_eq!(actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), expected);
        }
    }
}
