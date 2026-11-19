use super::{Decoded, EncodeOptions, Error};
use std::ffi::{c_char, c_int, c_uint};

#[repr(C)]
struct Native {
    data: *mut u8,
    len: usize,
    width: c_uint,
    height: c_uint,
    full_width: c_uint,
    full_height: c_uint,
    channels: c_uint,
    error: [c_char; 256],
}
impl Native {
    fn new() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            width: 0,
            height: 0,
            full_width: 0,
            full_height: 0,
            channels: 0,
            error: [0; 256],
        }
    }
    fn bytes(&self, success: c_int) -> Result<Vec<u8>, Error> {
        if success != 1 {
            let message: Vec<_> = self.error.iter().take_while(|v| **v != 0).map(|v| *v as u8).collect();
            return Err(Error(String::from_utf8_lossy(&message).into_owned()));
        }
        if self.data.is_null() || self.len == 0 || self.len > isize::MAX as usize {
            return Err(Error("invalid native JPEG buffer".into()));
        }
        let mut result = Vec::new();
        result.try_reserve_exact(self.len).map_err(|e| Error(e.to_string()))?;
        // SAFETY: success transfers a live malloc allocation of exactly len bytes to Native.
        result.extend_from_slice(unsafe { std::slice::from_raw_parts(self.data, self.len) });
        Ok(result)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: only the C shim allocates this pointer; free(NULL) is also valid.
        unsafe { grainy_jpeg_free(self.data) }
    }
}
unsafe extern "C" {
    fn grainy_jpeg_decode(input: *const u8, len: usize, target: usize, out: *mut Native) -> c_int;
    fn grainy_jpeg_encode(
        pixels: *const u8,
        width: c_uint,
        height: c_uint,
        channels: c_uint,
        quality: c_int,
        icc: *const u8,
        icc_len: usize,
        exif: *const u8,
        exif_len: usize,
        out: *mut Native,
    ) -> c_int;
    fn grainy_jpeg_free(data: *mut u8);
}

pub(super) fn decode(data: &[u8], target: usize) -> Result<Decoded, Error> {
    let mut native = Native::new();
    // SAFETY: input slice is live throughout the call; out is writable and no callback enters Rust.
    let ok = unsafe { grainy_jpeg_decode(data.as_ptr(), data.len(), target, &mut native) };
    Ok(Decoded {
        pixels: native.bytes(ok)?,
        width: native.width,
        height: native.height,
        full_width: native.full_width,
        full_height: native.full_height,
        channels: native.channels,
    })
}
pub(super) fn encode(pixels: &[u8], options: &EncodeOptions<'_>) -> Result<Vec<u8>, Error> {
    let mut native = Native::new();
    // SAFETY: safe wrapper validated pixel dimensions and marker lengths; all slices stay live.
    let ok = unsafe {
        grainy_jpeg_encode(
            pixels.as_ptr(),
            options.width,
            options.height,
            options.channels,
            options.quality,
            options.icc.as_ptr(),
            options.icc.len(),
            options.exif.as_ptr(),
            options.exif.len(),
            &mut native,
        )
    };
    native.bytes(ok)
}
