//! The minimal LittleCMS 2.19 FFI and its safe wrappers. The only module of the crate with `unsafe`.
//!
//! LittleCMS is the vendored, unmodified source in `vendor/lcms2-2.19`, compiled by `build.rs`. The wrappers own
//! their handles and free them on drop:
//!
//! * [`ProfileHandle`]: an open profile (`cmsCloseProfile`). Neither `Send` nor `Sync`: it is used and dropped
//!   on the thread that opened it.
//! * [`Transform`]: a transform with its own context (`cmsCreateContext`/`cmsDeleteContext`), as the oracle's
//!   `native_color.Transform`. Created with `cmsFLAGS_NOCACHE`, so `cmsDoTransform` keeps no mutable state and
//!   may run on disjoint buffers from several threads at once (LittleCMS documents transforms without the
//!   one-pixel cache as reentrant): `Send + Sync`.
//! * [`LocalTransform`]: a transform in the global context without `NOCACHE`, as imagecodecs' `cms_transform`
//!   creates it. Its one-pixel cache is mutable, so it is not `Sync` (and stays on its thread).
//!
//! Buffers are passed as slices of the [`Sample`] types the pixel format was created for; lengths are checked
//! before every call, so the safe API cannot make LittleCMS read or write out of bounds.

use std::ffi::{c_char, c_int, c_void};
use std::ptr::NonNull;

type Handle = *mut c_void;

/// `cmsCIExyY`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CIExyY {
    pub x: f64,
    pub y: f64,
    #[allow(non_snake_case)]
    pub Y: f64,
}

/// `cmsCIExyYTRIPLE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CIExyYTriple {
    pub red: CIExyY,
    pub green: CIExyY,
    pub blue: CIExyY,
}

/// `cmsCIEXYZ`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(non_snake_case)]
pub struct CieXyz {
    pub X: f64,
    pub Y: f64,
    pub Z: f64,
}

extern "C" {
    fn cmsGetEncodedCMMversion() -> c_int;
    fn cmsCreateContext(plugin: *mut c_void, user_data: *mut c_void) -> Handle;
    fn cmsDeleteContext(context: Handle);
    fn cmsOpenProfileFromMem(memory: *const c_void, size: u32) -> Handle;
    fn cmsOpenProfileFromMemTHR(context: Handle, memory: *const c_void, size: u32) -> Handle;
    fn cmsCloseProfile(profile: Handle) -> c_int;
    fn cmsGetColorSpace(profile: Handle) -> u32;
    fn cmsGetDeviceClass(profile: Handle) -> u32;
    fn cmsGetProfileInfoASCII(
        profile: Handle,
        info: u32,
        language: *const c_char,
        country: *const c_char,
        buffer: *mut c_char,
        size: u32,
    ) -> u32;
    fn cmsSaveProfileToMem(profile: Handle, memory: *mut c_void, needed: *mut u32) -> c_int;
    fn cmsCreateTransform(
        input: Handle,
        in_format: u32,
        output: Handle,
        out_format: u32,
        intent: u32,
        flags: u32,
    ) -> Handle;
    fn cmsCreateTransformTHR(
        context: Handle,
        input: Handle,
        in_format: u32,
        output: Handle,
        out_format: u32,
        intent: u32,
        flags: u32,
    ) -> Handle;
    fn cmsDeleteTransform(transform: Handle);
    fn cmsDoTransform(transform: Handle, input: *const c_void, output: *mut c_void, pixels: u32);
    fn cmsDoTransformLineStride(
        transform: Handle,
        input: *const c_void,
        output: *mut c_void,
        pixels_per_line: u32,
        lines: u32,
        bytes_per_line_in: u32,
        bytes_per_line_out: u32,
        bytes_per_plane_in: u32,
        bytes_per_plane_out: u32,
    );
    fn cmsCreate_sRGBProfile() -> Handle;
    fn cmsCreateRGBProfile(white: *const CIExyY, primaries: *const CIExyYTriple, curves: *const Handle) -> Handle;
    fn cmsCreateRGBProfileTHR(
        context: Handle,
        white: *const CIExyY,
        primaries: *const CIExyYTriple,
        curves: *const Handle,
    ) -> Handle;
    fn cmsCreateGrayProfile(white: *const CIExyY, curve: Handle) -> Handle;
    fn cmsCreateLab2Profile(white: *const CIExyY) -> Handle;
    fn cmsCreateLab4Profile(white: *const CIExyY) -> Handle;
    fn cmsBuildGamma(context: Handle, gamma: f64) -> Handle;
    fn cmsBuildTabulatedToneCurveFloat(context: Handle, entries: u32, values: *const f32) -> Handle;
    fn cmsFreeToneCurve(curve: Handle);
    fn cmsCreateProfilePlaceholder(context: Handle) -> Handle;
    fn cmsSetProfileVersion(profile: Handle, version: f64);
    fn cmsMLUalloc(context: Handle, items: u32) -> Handle;
    fn cmsMLUsetASCII(mlu: Handle, language: *const c_char, country: *const c_char, text: *const c_char) -> c_int;
    fn cmsMLUfree(mlu: Handle);
    fn cmsWriteTag(profile: Handle, signature: u32, data: *const c_void) -> c_int;
    fn cmsLinkTag(profile: Handle, signature: u32, destination: u32) -> c_int;
    fn cmsSetDeviceClass(profile: Handle, class: u32);
    fn cmsSetColorSpace(profile: Handle, space: u32);
    fn cmsSetPCS(profile: Handle, pcs: u32);
}

/// `cmsGetEncodedCMMversion()`: 2190 for the vendored LittleCMS 2.19.
pub fn version() -> i32 {
    // SAFETY: no arguments, returns a constant.
    unsafe { cmsGetEncodedCMMversion() }
}

/// ICC signatures (four ASCII characters, big-endian) used here.
pub mod sig {
    pub const RGB_DATA: u32 = 0x5247_4220; // 'RGB '
    pub const GRAY_DATA: u32 = 0x4752_4159; // 'GRAY'
    pub const XYZ_DATA: u32 = 0x5859_5A20; // 'XYZ '
    pub const LAB_DATA: u32 = 0x4C61_6220; // 'Lab '
    pub const CMYK_DATA: u32 = 0x434D_594B; // 'CMYK'
    pub const DISPLAY_CLASS: u32 = 0x6D6E_7472; // 'mntr'
    pub const COLORSPACE_CLASS: u32 = 0x7370_6163; // 'spac'
    pub const INPUT_CLASS: u32 = 0x7363_6E72; // 'scnr'
    pub const OUTPUT_CLASS: u32 = 0x7072_7472; // 'prtr'
    pub const LINK_CLASS: u32 = 0x6C69_6E6B; // 'link'
    pub const ABSTRACT_CLASS: u32 = 0x6162_7374; // 'abst'
    pub const NAMED_COLOR_CLASS: u32 = 0x6E6D_636C; // 'nmcl'
    pub const COPYRIGHT_TAG: u32 = 0x6370_7274; // 'cprt'
    pub const DESCRIPTION_TAG: u32 = 0x6465_7363; // 'desc'
    pub const DEVICE_MFG_DESC_TAG: u32 = 0x646D_6E64; // 'dmnd'
    pub const DEVICE_MODEL_DESC_TAG: u32 = 0x646D_6464; // 'dmdd'
    pub const MEDIA_WHITE_POINT_TAG: u32 = 0x7774_7074; // 'wtpt'
    pub const MEDIA_BLACK_POINT_TAG: u32 = 0x626B_7074; // 'bkpt'
    pub const RED_COLORANT_TAG: u32 = 0x7258_595A; // 'rXYZ'
    pub const GREEN_COLORANT_TAG: u32 = 0x6758_595A; // 'gXYZ'
    pub const BLUE_COLORANT_TAG: u32 = 0x6258_595A; // 'bXYZ'
    pub const RED_TRC_TAG: u32 = 0x7254_5243; // 'rTRC'
    pub const GREEN_TRC_TAG: u32 = 0x6754_5243; // 'gTRC'
    pub const BLUE_TRC_TAG: u32 = 0x6254_5243; // 'bTRC'
}

/// Pixel formats from the pinned `lcms2.h`: `COLORSPACE_SH(PT_RGB) | CHANNELS_SH(3) | BYTES_SH(n)`, plus
/// `FLOAT_SH(1)` for floats and `PLANAR_SH(1)` for planes.
pub mod format {
    const RGB: u32 = (4 << 16) | (3 << 3);
    /// `FLOAT_SH(1)`.
    pub const FLOAT: u32 = 1 << 22;
    pub const RGB_8: u32 = RGB | 1;
    pub const RGB_FLT: u32 = FLOAT | RGB | 4;
    pub const PLANAR: u32 = 1 << 12;
    pub const RGB_FLT_PLANAR: u32 = RGB_FLT | PLANAR;

    /// `PT_GRAY`, `PT_RGB`, `PT_CMYK`, `PT_Lab` (the `COLORSPACE_SH` values of `lcms2.h`).
    pub const PT_GRAY: u32 = 3;
    pub const PT_RGB: u32 = 4;
    pub const PT_CMYK: u32 = 6;
    pub const PT_LAB: u32 = 10;

    /// A packed (interleaved) format as imagecodecs' `_cms_format` builds it for an explicit colour space
    /// without extra channels: `COLORSPACE_SH(pixel_type) | CHANNELS_SH(channels) | BYTES_SH(bytes)`, plus
    /// `FLOAT_SH(1)` for floats; `type_bits` is a [`super::Sample::TYPE_BITS`].
    pub const fn packed(pixel_type: u32, channels: u32, type_bits: u32) -> u32 {
        (pixel_type << 16) | (channels << 3) | type_bits
    }
}

/// `INTENT_RELATIVE_COLORIMETRIC`, the only intent the oracle uses.
pub const INTENT_RELATIVE_COLORIMETRIC: u32 = 1;
/// `cmsFLAGS_NOCACHE`.
pub const FLAGS_NOCACHE: u32 = 0x0040;

mod sealed {
    pub trait Sealed {}
}

/// A sample type LittleCMS reads and writes: `u8`, `u16`, `f32`, `f64` (numpy `B`, `H`, `f`, `d`).
pub trait Sample: sealed::Sealed + Copy + Default + Send + Sync + 'static {
    /// `BYTES_SH(n)`, plus `FLOAT_SH(1)` for floats: the sample-type bits of a pixel format.
    const TYPE_BITS: u32;
    /// The interleaved RGB pixel format of this sample type.
    const FORMAT: u32 = format::packed(format::PT_RGB, 3, Self::TYPE_BITS);

    /// The samples as `f32` when this is `f32` (the oracle's native float path), else `None`.
    #[doc(hidden)]
    fn as_f32(samples: &[Self]) -> Option<&[f32]> {
        let _ = samples;
        None
    }

    /// `f32` values as this type when it is `f32`, else `None`.
    #[doc(hidden)]
    fn from_f32(values: Vec<f32>) -> Option<Vec<Self>> {
        let _ = values;
        None
    }
}

impl sealed::Sealed for u8 {}
impl sealed::Sealed for u16 {}
impl sealed::Sealed for f32 {}
impl sealed::Sealed for f64 {}
impl Sample for u8 {
    const TYPE_BITS: u32 = 1;
}
impl Sample for u16 {
    const TYPE_BITS: u32 = 2;
}
impl Sample for f32 {
    const TYPE_BITS: u32 = format::FLOAT | 4;

    fn as_f32(samples: &[Self]) -> Option<&[f32]> {
        Some(samples)
    }

    fn from_f32(values: Vec<f32>) -> Option<Vec<Self>> {
        Some(values)
    }
}
impl Sample for f64 {
    const TYPE_BITS: u32 = format::FLOAT; // BYTES_SH(0) means 8 bytes
}

/// An open profile; closed on drop.
pub struct ProfileHandle(NonNull<c_void>);

impl ProfileHandle {
    /// `cmsOpenProfileFromMem` (global context), as imagecodecs opens profiles. `None` when LittleCMS rejects
    /// the data (or it is longer than `u32::MAX`).
    pub fn open(data: &[u8]) -> Option<Self> {
        let size = u32::try_from(data.len()).ok()?;
        // SAFETY: LittleCMS copies `size` bytes from a valid slice (an empty slice gives a dangling but
        // never-read pointer with size 0, which it rejects).
        NonNull::new(unsafe { cmsOpenProfileFromMem(data.as_ptr().cast(), size) }).map(Self)
    }

    fn open_in(context: &Context, data: &[u8]) -> Option<Self> {
        let size = u32::try_from(data.len()).ok()?;
        // SAFETY: as `open`; the context outlives the profile (profiles are closed before their context).
        NonNull::new(unsafe { cmsOpenProfileFromMemTHR(context.0.as_ptr(), data.as_ptr().cast(), size) }).map(Self)
    }

    /// `cmsGetColorSpace`.
    pub fn color_space(&self) -> u32 {
        // SAFETY: valid open profile.
        unsafe { cmsGetColorSpace(self.0.as_ptr()) }
    }

    /// `cmsGetDeviceClass`.
    pub fn device_class(&self) -> u32 {
        // SAFETY: valid open profile.
        unsafe { cmsGetDeviceClass(self.0.as_ptr()) }
    }

    /// The description as imagecodecs' `cms_info` reads it: `cmsGetProfileInfoASCII(cmsInfoDescription,
    /// cmsNoLanguage, cmsNoCountry)` when the text needs 1..=512 bytes, else `None`. Raw bytes, NUL included.
    pub fn description_bytes(&self) -> Option<Vec<u8>> {
        let none = [0 as c_char; 3];
        // SAFETY: a NULL buffer with size 0 asks for the needed size.
        let size = unsafe {
            cmsGetProfileInfoASCII(self.0.as_ptr(), 0, none.as_ptr(), none.as_ptr(), std::ptr::null_mut(), 0)
        };
        if size == 0 || size > 512 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        // SAFETY: the buffer holds `size` bytes.
        unsafe {
            cmsGetProfileInfoASCII(self.0.as_ptr(), 0, none.as_ptr(), none.as_ptr(), buffer.as_mut_ptr().cast(), size)
        };
        Some(buffer)
    }

    /// `cmsSaveProfileToMem` twice (size, then data), as `imagecodecs.cms_profile` does.
    pub fn save(&self) -> Option<Vec<u8>> {
        let mut needed = 0u32;
        // SAFETY: a NULL buffer asks for the size.
        if unsafe { cmsSaveProfileToMem(self.0.as_ptr(), std::ptr::null_mut(), &mut needed) } == 0 {
            return None;
        }
        let mut data = vec![0u8; needed as usize];
        // SAFETY: the buffer holds `needed` bytes.
        if unsafe { cmsSaveProfileToMem(self.0.as_ptr(), data.as_mut_ptr().cast(), &mut needed) } == 0 {
            return None;
        }
        data.truncate(needed as usize);
        Some(data)
    }

    fn raw(&self) -> Handle {
        self.0.as_ptr()
    }
}

impl Drop for ProfileHandle {
    fn drop(&mut self) {
        // SAFETY: owned handle, closed once.
        unsafe { cmsCloseProfile(self.0.as_ptr()) };
    }
}

/// A private LittleCMS context; deleted on drop.
struct Context(NonNull<c_void>);

impl Context {
    fn new() -> Option<Self> {
        // SAFETY: no plugins, no user data.
        NonNull::new(unsafe { cmsCreateContext(std::ptr::null_mut(), std::ptr::null_mut()) }).map(Self)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: owned context; everything created in it was released before (field order in Transform).
        unsafe { cmsDeleteContext(self.0.as_ptr()) };
    }
}

/// Why a transform could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// `cmsCreateContext` failed.
    Context,
    /// One of the profiles did not open.
    Profile,
    /// A profile is not RGB.
    NotRgb,
    /// `cmsCreateTransform` returned NULL.
    Create,
}

/// A NOCACHE transform with its own context (the oracle's `native_color.Transform`). Reentrant.
pub struct Transform {
    handle: NonNull<c_void>,
    in_bytes: usize,
    out_bytes: usize,
    planar_out: bool,
    // Dropped after `handle` (see Drop): the context must outlive the transform.
    _context: Context,
}

// SAFETY: the transform is immutable after creation (cmsFLAGS_NOCACHE disables its only mutable state, the
// one-pixel cache) and owns a private context; LittleCMS allows concurrent cmsDoTransform calls on such a
// transform with disjoint buffers, and freeing it from another thread.
unsafe impl Send for Transform {}
// SAFETY: see Send.
unsafe impl Sync for Transform {}

fn sample_bytes(format: u32) -> usize {
    match format & 7 {
        0 => 8,
        n => n as usize,
    }
}

impl Transform {
    /// `cmsCreateContext`, `cmsOpenProfileFromMemTHR` for both profiles, both RGB, then
    /// `cmsCreateTransformTHR(context, in, in_format, out, out_format, INTENT_RELATIVE_COLORIMETRIC,
    /// cmsFLAGS_NOCACHE)`; the profiles are closed again.
    pub fn new(source: &[u8], destination: &[u8], in_format: u32, out_format: u32) -> Result<Self, TransformError> {
        let context = Context::new().ok_or(TransformError::Context)?;
        let handle = {
            let incoming = ProfileHandle::open_in(&context, source);
            let outgoing = ProfileHandle::open_in(&context, destination);
            let (Some(incoming), Some(outgoing)) = (incoming, outgoing) else {
                return Err(TransformError::Profile);
            };
            if incoming.color_space() != sig::RGB_DATA || outgoing.color_space() != sig::RGB_DATA {
                return Err(TransformError::NotRgb);
            }
            // SAFETY: valid context and profiles; the transform keeps no reference to the profiles.
            let raw = unsafe {
                cmsCreateTransformTHR(
                    context.0.as_ptr(),
                    incoming.raw(),
                    in_format,
                    outgoing.raw(),
                    out_format,
                    INTENT_RELATIVE_COLORIMETRIC,
                    FLAGS_NOCACHE,
                )
            };
            NonNull::new(raw).ok_or(TransformError::Create)?
        };
        Ok(Self {
            handle,
            in_bytes: 3 * sample_bytes(in_format),
            out_bytes: 3 * sample_bytes(out_format),
            planar_out: out_format & format::PLANAR != 0,
            _context: context,
        })
    }

    /// `cmsDoTransform` over `input.len() / 3` interleaved pixels. The sample types must be the ones the
    /// transform was created for (checked through their sizes); counts above `u32::MAX` are split.
    pub fn apply<I: Sample, O: Sample>(&self, input: &[I], output: &mut [O]) {
        assert!(!self.planar_out, "planar transforms take rows");
        assert!(std::mem::size_of::<I>() * 3 == self.in_bytes && std::mem::size_of::<O>() * 3 == self.out_bytes);
        do_transform(self.handle.as_ptr(), input, output);
    }

    /// `cmsDoTransformLineStride` for 8-bit interleaved rows in, float planes out (`convert8_planar`):
    /// `lines` rows of `width` pixels; the output holds three planes of `lines * width` floats.
    pub fn rows_to_planes(&self, input: &[u8], output: &mut [f32], width: usize, lines: usize) {
        assert!(self.planar_out && self.in_bytes == 3 && self.out_bytes == 12);
        let pixels = width * lines;
        assert!(input.len() >= pixels * 3 && output.len() >= pixels * 3, "buffer too small");
        if pixels == 0 {
            return;
        }
        let as_u32 = |value: usize| u32::try_from(value).expect("plane stride above u32::MAX");
        // SAFETY: lengths checked above; the input is read as `lines` rows of `width * 3` bytes, the output written
        // as three planes of `pixels * 4` bytes each.
        unsafe {
            cmsDoTransformLineStride(
                self.handle.as_ptr(),
                input.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                as_u32(width),
                as_u32(lines),
                as_u32(width * 3),
                as_u32(width * 4),
                as_u32(pixels * 3),
                as_u32(pixels * 4),
            )
        };
    }
}

impl Drop for Transform {
    fn drop(&mut self) {
        // SAFETY: owned transform, deleted once, before its context (dropped afterwards as a field).
        unsafe { cmsDeleteTransform(self.handle.as_ptr()) };
    }
}

fn do_transform<I: Sample, O: Sample>(handle: Handle, input: &[I], output: &mut [O]) {
    do_transform_channels(handle, input, 3, output, 3);
}

/// `cmsDoTransform` over `input.len() / in_channels` packed pixels, in calls of at most `u32::MAX` pixels.
/// The output must hold `out_channels` samples per pixel.
fn do_transform_channels<I: Sample, O: Sample>(
    handle: Handle,
    input: &[I],
    in_channels: usize,
    output: &mut [O],
    out_channels: usize,
) {
    let pixels = input.len() / in_channels;
    assert!(output.len() >= pixels * out_channels, "output buffer too small");
    // cmsDoTransform takes a uint32 pixel count.
    let step = u32::MAX as usize;
    let mut start = 0;
    while start < pixels {
        let count = step.min(pixels - start);
        // SAFETY: `count` pixels of `in_channels` / `out_channels` samples are inside both slices; the formats
        // match the sample types and the channel counts.
        unsafe {
            cmsDoTransform(
                handle,
                input[start * in_channels..].as_ptr().cast(),
                output[start * out_channels..].as_mut_ptr().cast(),
                count as u32,
            )
        };
        start += count;
    }
}

/// A transform as imagecodecs' `cms_transform` makes it: global context, `cmsCreateTransform(in, I::FORMAT,
/// out, O::FORMAT, INTENT_RELATIVE_COLORIMETRIC, 0)`. Not `Sync` (its one-pixel cache is mutable).
pub struct LocalTransform<I: Sample, O: Sample> {
    handle: NonNull<c_void>,
    in_channels: usize,
    out_channels: usize,
    _types: std::marker::PhantomData<(I, O)>,
}

impl<I: Sample, O: Sample> LocalTransform<I, O> {
    /// The RGB to RGB transform between two open profiles, or `None` when `cmsCreateTransform` returns NULL.
    pub fn new(source: &ProfileHandle, destination: &ProfileHandle) -> Option<Self> {
        Self::packed(source, (format::PT_RGB, 3), destination, (format::PT_RGB, 3))
    }

    /// A transform between packed pixels of the given (`PT_*` pixel type, channel count) layouts:
    /// `cmsCreateTransform(in, packed(input), out, packed(output), INTENT_RELATIVE_COLORIMETRIC, 0)`, the call
    /// imagecodecs' `cms_transform(colorspace=..., outcolorspace=..., intent=1)` makes. `None` when LittleCMS
    /// refuses (for example a layout that does not match the profile's colour space) or a channel count is
    /// outside 1..=4.
    pub fn packed(
        source: &ProfileHandle,
        input: (u32, u32),
        destination: &ProfileHandle,
        output: (u32, u32),
    ) -> Option<Self> {
        if !(1..=4).contains(&input.1) || !(1..=4).contains(&output.1) {
            return None;
        }
        let in_format = format::packed(input.0, input.1, I::TYPE_BITS);
        let out_format = format::packed(output.0, output.1, O::TYPE_BITS);
        // SAFETY: valid profiles; the transform keeps no reference to them.
        let raw = unsafe {
            cmsCreateTransform(source.raw(), in_format, destination.raw(), out_format, INTENT_RELATIVE_COLORIMETRIC, 0)
        };
        NonNull::new(raw).map(|handle| Self {
            handle,
            in_channels: input.1 as usize,
            out_channels: output.1 as usize,
            _types: std::marker::PhantomData,
        })
    }

    /// `cmsDoTransform` over `input.len() / in_channels` pixels (a trailing partial pixel is ignored); the
    /// output must hold `out_channels` samples for each of them.
    pub fn apply(&mut self, input: &[I], output: &mut [O]) {
        do_transform_channels(self.handle.as_ptr(), input, self.in_channels, output, self.out_channels);
    }
}

impl<I: Sample, O: Sample> Drop for LocalTransform<I, O> {
    fn drop(&mut self) {
        // SAFETY: owned transform, deleted once.
        unsafe { cmsDeleteTransform(self.handle.as_ptr()) };
    }
}

/// A tone curve for profile building; freed on drop.
pub struct ToneCurve(NonNull<c_void>);

impl ToneCurve {
    /// `cmsBuildGamma(NULL, gamma)`.
    pub fn gamma(gamma: f64) -> Option<Self> {
        // SAFETY: plain value argument.
        NonNull::new(unsafe { cmsBuildGamma(std::ptr::null_mut(), gamma) }).map(Self)
    }

    /// `cmsBuildTabulatedToneCurveFloat(NULL, n, values)`.
    pub fn table_f32(values: &[f32]) -> Option<Self> {
        let n = u32::try_from(values.len()).ok()?;
        // SAFETY: LittleCMS copies `n` floats from the slice.
        NonNull::new(unsafe { cmsBuildTabulatedToneCurveFloat(std::ptr::null_mut(), n, values.as_ptr()) }).map(Self)
    }
}

impl Drop for ToneCurve {
    fn drop(&mut self) {
        // SAFETY: owned curve, freed once (profiles copy the curves they store).
        unsafe { cmsFreeToneCurve(self.0.as_ptr()) };
    }
}

fn wrap(raw: Handle) -> Option<ProfileHandle> {
    NonNull::new(raw).map(ProfileHandle)
}

/// `cmsCreate_sRGBProfile()`.
pub fn create_srgb() -> Option<ProfileHandle> {
    // SAFETY: no arguments.
    wrap(unsafe { cmsCreate_sRGBProfile() })
}

/// `cmsCreateRGBProfile(white, primaries, curves)`; `curves = None` passes NULL. Identical curve objects
/// (the same `&ToneCurve` twice) make LittleCMS link the TRC tags instead of writing copies.
pub fn create_rgb(white: &CIExyY, primaries: &CIExyYTriple, curves: Option<[&ToneCurve; 3]>) -> Option<ProfileHandle> {
    let table = curves.map(|c| [c[0].0.as_ptr(), c[1].0.as_ptr(), c[2].0.as_ptr()]);
    let pointer = table.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
    // SAFETY: valid structs and curves (or NULL); LittleCMS copies the curves.
    wrap(unsafe { cmsCreateRGBProfile(white, primaries, pointer) })
}

/// `cmsCreateRGBProfileTHR(NULL, white, primaries, curves)`.
pub fn create_rgb_thr(white: &CIExyY, primaries: &CIExyYTriple, curves: [&ToneCurve; 3]) -> Option<ProfileHandle> {
    let table = [curves[0].0.as_ptr(), curves[1].0.as_ptr(), curves[2].0.as_ptr()];
    // SAFETY: valid structs and curves; LittleCMS copies the curves.
    wrap(unsafe { cmsCreateRGBProfileTHR(std::ptr::null_mut(), white, primaries, table.as_ptr()) })
}

/// `cmsCreateGrayProfile(white, curve)`.
pub fn create_gray(white: &CIExyY, curve: &ToneCurve) -> Option<ProfileHandle> {
    create_gray_opt(Some(white), Some(curve))
}

/// `cmsCreateGrayProfile(white | NULL, curve | NULL)`, as imagecodecs' `cms_profile('gray', ...)` calls it.
pub fn create_gray_opt(white: Option<&CIExyY>, curve: Option<&ToneCurve>) -> Option<ProfileHandle> {
    let white = white.map_or(std::ptr::null(), |w| w as *const CIExyY);
    let curve = curve.map_or(std::ptr::null_mut(), |c| c.0.as_ptr());
    // SAFETY: a valid struct or NULL, a valid curve or NULL (LittleCMS checks both); it copies the curve.
    wrap(unsafe { cmsCreateGrayProfile(white, curve) })
}

/// `cmsCreateRGBProfile(white | NULL, primaries | NULL, curves | NULL)`, as imagecodecs'
/// `cms_profile('rgb', ...)` calls it.
pub fn create_rgb_opt(
    white: Option<&CIExyY>,
    primaries: Option<&CIExyYTriple>,
    curves: Option<[&ToneCurve; 3]>,
) -> Option<ProfileHandle> {
    let white = white.map_or(std::ptr::null(), |w| w as *const CIExyY);
    let primaries = primaries.map_or(std::ptr::null(), |p| p as *const CIExyYTriple);
    let table = curves.map(|c| [c[0].0.as_ptr(), c[1].0.as_ptr(), c[2].0.as_ptr()]);
    let pointer = table.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
    // SAFETY: valid structs or NULL, valid curves or NULL; LittleCMS copies what it keeps.
    wrap(unsafe { cmsCreateRGBProfile(white, primaries, pointer) })
}

/// `cmsCreateLab4Profile(white | NULL)` (`lab4`) or `cmsCreateLab2Profile(white | NULL)` (`lab2`); NULL is D50.
pub fn create_lab(version4: bool, white: Option<&CIExyY>) -> Option<ProfileHandle> {
    let white = white.map_or(std::ptr::null(), |w| w as *const CIExyY);
    // SAFETY: a valid struct or NULL.
    wrap(unsafe {
        if version4 {
            cmsCreateLab4Profile(white)
        } else {
            cmsCreateLab2Profile(white)
        }
    })
}

/// Builder calls on a profile (placeholder profiles, extra tags).
impl ProfileHandle {
    /// `cmsCreateProfilePlaceholder(NULL)`.
    pub fn placeholder() -> Option<Self> {
        // SAFETY: global context.
        wrap(unsafe { cmsCreateProfilePlaceholder(std::ptr::null_mut()) })
    }

    /// `cmsSetProfileVersion`.
    pub fn set_version(&self, version: f64) {
        // SAFETY: valid profile.
        unsafe { cmsSetProfileVersion(self.raw(), version) }
    }

    /// `cmsSetDeviceClass`, `cmsSetColorSpace`, `cmsSetPCS`.
    pub fn set_header(&self, class: u32, space: u32, pcs: u32) {
        // SAFETY: valid profile.
        unsafe {
            cmsSetDeviceClass(self.raw(), class);
            cmsSetColorSpace(self.raw(), space);
            cmsSetPCS(self.raw(), pcs);
        }
    }

    /// `cmsMLUalloc(NULL, 1)`, `cmsMLUsetASCII(mlu, "en", "US", text)`, `cmsWriteTag(tag, mlu)`, `cmsMLUfree`.
    /// Returns false when the MLU cannot be allocated (the write's own result is ignored, as imagecodecs does).
    pub fn write_text(&self, tag: u32, text: &std::ffi::CStr) -> bool {
        // SAFETY: global context.
        let mlu = unsafe { cmsMLUalloc(std::ptr::null_mut(), 1) };
        if mlu.is_null() {
            return false;
        }
        // SAFETY: valid MLU, NUL-terminated language/country/text; the tag write copies the MLU, freed after.
        unsafe {
            cmsMLUsetASCII(mlu, c"en".as_ptr(), c"US".as_ptr(), text.as_ptr());
            cmsWriteTag(self.raw(), tag, mlu);
            cmsMLUfree(mlu);
        }
        true
    }

    /// `cmsWriteTag(tag, &xyz)` (result ignored, as imagecodecs does).
    pub fn write_xyz(&self, tag: u32, xyz: &CieXyz) {
        // SAFETY: valid profile; LittleCMS copies the value.
        unsafe { cmsWriteTag(self.raw(), tag, (xyz as *const CieXyz).cast()) };
    }

    /// `cmsWriteTag(tag, curve)` (result ignored).
    pub fn write_curve(&self, tag: u32, curve: &ToneCurve) {
        // SAFETY: valid profile and curve; LittleCMS copies the curve.
        unsafe { cmsWriteTag(self.raw(), tag, curve.0.as_ptr()) };
    }

    /// `cmsLinkTag(tag, destination)` (result ignored).
    pub fn link_tag(&self, tag: u32, destination: u32) {
        // SAFETY: valid profile.
        unsafe { cmsLinkTag(self.raw(), tag, destination) };
    }
}
