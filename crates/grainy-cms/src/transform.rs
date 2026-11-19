//! Conversions: luma/native_color.py (`Transform`, the bounded transform cache, `convert`, `convert8_planar`)
//! and the conversion part of luma/colorio.py `convert` (native float path or the imagecodecs path).

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use grainy_core::Image;
use ndarray::{Array3, ArrayView3};
use rayon::prelude::*;

use crate::profiles::Profile;
use crate::sys::{self, format, ProfileHandle, Sample, TransformError};
use crate::{CmsError, Result};

/// `native_color.MAX_TRANSFORMS`: transforms kept in the cache.
pub const MAX_TRANSFORMS: usize = 16;
/// `native_color.MAX_PROFILE_BYTES`: profile bytes kept alive by the cache keys.
pub const MAX_PROFILE_BYTES: usize = 32 * 1024 * 1024;
/// `native_color.MAX_PROFILE_SIZE`: the largest profile the native path accepts.
pub const MAX_PROFILE_SIZE: usize = 16 * 1024 * 1024;
/// `native_color.PARALLEL_PIXELS`: smaller buffers are converted on the calling thread.
pub const PARALLEL_PIXELS: usize = 128 * 1024;

/// A reusable LittleCMS transform between two RGB profiles. Port of luma/native_color.py `Transform`: a
/// private context, relative colorimetric intent, `cmsFLAGS_NOCACHE`; float RGB in and out, or (planar8)
/// 8-bit RGB in and float planes out. Immutable and shared between threads.
pub struct Transform {
    inner: sys::Transform,
    planar8: bool,
}

impl Transform {
    /// Port of `native_color.Transform.__init__`.
    pub fn new(source: &Profile, destination: &Profile, planar8: bool) -> Result<Self> {
        let (in_format, out_format) =
            if planar8 { (format::RGB_8, format::RGB_FLT_PLANAR) } else { (format::RGB_FLT, format::RGB_FLT) };
        let inner = sys::Transform::new(source.bytes(), destination.bytes(), in_format, out_format).map_err(
            |error| match error {
                TransformError::Context => CmsError::Context,
                TransformError::Profile => CmsError::InvalidProfile,
                TransformError::NotRgb => CmsError::RgbRequired,
                TransformError::Create => CmsError::TransformFailed,
            },
        )?;
        Ok(Self { inner, planar8 })
    }

    /// Whether this is a `convert8_planar` transform.
    pub fn planar8(&self) -> bool {
        self.planar8
    }
}

/// The counters of the cache (`native_color.cache_info`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheInfo {
    /// Transforms in the cache.
    pub transforms: usize,
    /// Bytes of the profiles of the cached transforms.
    pub profile_bytes: usize,
    /// Lookups that found a transform.
    pub hits: u64,
    /// Lookups that made one.
    pub misses: u64,
}

type Key = (Arc<[u8]>, Arc<[u8]>, bool);

#[derive(Default)]
struct CacheState {
    entries: VecDeque<(Key, Arc<Transform>)>, // least recently used first
    info: CacheInfo,
}

/// A bounded LRU cache of transforms keyed by (source bytes, destination bytes, planar8). Port of the
/// `native_color` module cache: at most [`MAX_TRANSFORMS`] transforms and [`MAX_PROFILE_BYTES`] of key bytes;
/// the transform is made under the lock; a key larger than the byte budget is made but not kept. Evicted
/// transforms stay alive until their last user drops them (`Arc`).
pub struct TransformCache {
    state: Mutex<CacheState>,
}

impl Default for TransformCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TransformCache {
    /// An empty cache.
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(CacheState {
                entries: VecDeque::new(),
                info: CacheInfo { transforms: 0, profile_bytes: 0, hits: 0, misses: 0 },
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Port of `native_color._transform`.
    pub fn get(&self, source: &Profile, destination: &Profile, planar8: bool) -> Result<Arc<Transform>> {
        let mut state = self.lock();
        let found = state
            .entries
            .iter()
            .position(|((s, d, p), _)| *p == planar8 && same(s, source.shared()) && same(d, destination.shared()));
        if let Some(index) = found {
            state.info.hits += 1;
            let entry = state.entries.remove(index).expect("index from position");
            let transform = entry.1.clone();
            state.entries.push_back(entry);
            return Ok(transform);
        }
        state.info.misses += 1;
        let transform = Arc::new(Transform::new(source, destination, planar8)?);
        let cost = source.bytes().len() + destination.bytes().len();
        while !state.entries.is_empty()
            && (state.entries.len() >= MAX_TRANSFORMS || state.info.profile_bytes + cost > MAX_PROFILE_BYTES)
        {
            if let Some(((s, d, _), _)) = state.entries.pop_front() {
                state.info.profile_bytes -= s.len() + d.len();
            }
        }
        if cost <= MAX_PROFILE_BYTES {
            state
                .entries
                .push_back(((source.shared().clone(), destination.shared().clone(), planar8), transform.clone()));
            state.info.profile_bytes += cost;
        }
        state.info.transforms = state.entries.len();
        Ok(transform)
    }

    /// Port of `native_color.clear_cache`: empties the cache and resets the counters.
    pub fn clear(&self) {
        *self.lock() = CacheState::default();
    }

    /// Port of `native_color.cache_info`.
    pub fn info(&self) -> CacheInfo {
        let state = self.lock();
        CacheInfo { transforms: state.entries.len(), ..state.info }
    }
}

fn same(a: &Arc<[u8]>, b: &Arc<[u8]>) -> bool {
    Arc::ptr_eq(a, b) || a[..] == b[..]
}

static CACHE: LazyLock<TransformCache> = LazyLock::new(TransformCache::new);

/// The process-wide transform cache used by [`convert`] and [`convert8_planar`].
pub fn cache() -> &'static TransformCache {
    &CACHE
}

/// Port of `native_color.clear_cache`.
pub fn clear_cache() {
    CACHE.clear();
}

/// Port of `native_color.cache_info`.
pub fn cache_info() -> CacheInfo {
    CACHE.info()
}

fn check_native_sizes(source: &Profile, destination: &Profile) -> Result<()> {
    if [source, destination].iter().any(|p| !(128..=MAX_PROFILE_SIZE).contains(&p.bytes().len())) {
        return Err(CmsError::InvalidProfileSize);
    }
    Ok(())
}

/// Port of luma/native_color.py `convert` on interleaved float RGB samples (`len % 3 == 0`): cached NOCACHE
/// transform; buffers of [`PARALLEL_PIXELS`] or more are converted in chunks on the rayon pool (per-pixel
/// transform: the result does not depend on the chunking or the thread count).
pub fn native_convert(rgb: &[f32], source: &Profile, destination: &Profile) -> Result<Vec<f32>> {
    check_native_sizes(source, destination)?;
    if !rgb.len().is_multiple_of(3) {
        return Err(CmsError::Float32RgbRequired);
    }
    let pixels = rgb.len() / 3;
    let mut result = vec![0f32; rgb.len()];
    if pixels == 0 {
        return Ok(result);
    }
    let transform = CACHE.get(source, destination, false)?;
    let workers = rayon::current_num_threads().max(1);
    if pixels < PARALLEL_PIXELS || workers == 1 {
        transform.inner.apply(rgb, &mut result);
    } else {
        let size = (128 * 1024).min(pixels.div_ceil(workers)) * 3;
        result
            .par_chunks_mut(size)
            .zip(rgb.par_chunks(size))
            .for_each(|(out, input)| transform.inner.apply(input, out));
    }
    Ok(result)
}

/// The imagecodecs path of luma/colorio.py `convert` (`imagecodecs.cms_transform(..., colorspace='rgb',
/// outcolorspace='rgb', intent=1)`): profiles opened in the global context, a fresh transform with flags 0,
/// one `cmsDoTransform` over the whole buffer.
fn imagecodecs_convert<I: Sample, O: Sample>(rgb: &[I], source: &Profile, destination: &Profile) -> Result<Vec<O>> {
    let incoming = ProfileHandle::open(source.bytes()).ok_or(CmsError::OpenFailed)?;
    let outgoing = ProfileHandle::open(destination.bytes()).ok_or(CmsError::OpenFailed)?;
    let mut transform =
        sys::LocalTransform::<I, O>::new(&incoming, &outgoing).ok_or(CmsError::CreateTransformFailed)?;
    drop((incoming, outgoing));
    let mut result = vec![O::default(); rgb.len()];
    transform.apply(rgb, &mut result);
    Ok(result)
}

/// Port of luma/colorio.py `convert(rgb, source, destination, outdtype)` on interleaved RGB samples of any of
/// the oracle's dtypes (`u8`, `u16`, `f32`, `f64`; the output type is `outdtype`). Float32 in and out with
/// profiles up to [`MAX_PROFILE_SIZE`] takes the native path ([`native_convert`]); everything else the
/// imagecodecs path (same LittleCMS, global context, flags 0, single-threaded).
pub fn convert_samples<I: Sample, O: Sample>(rgb: &[I], source: &Profile, destination: &Profile) -> Result<Vec<O>> {
    if !rgb.len().is_multiple_of(3) {
        return Err(CmsError::RgbSamplesRequired);
    }
    let native = source.bytes().len().max(destination.bytes().len()) <= MAX_PROFILE_SIZE;
    if let (true, Some(input), true) = (native, I::as_f32(rgb), O::from_f32(Vec::new()).is_some()) {
        let output = native_convert(input, source, destination)?;
        return O::from_f32(output).ok_or(CmsError::Float32RgbRequired);
    }
    imagecodecs_convert(rgb, source, destination)
}

/// The pixel colour spaces of imagecodecs' `cms_transform(colorspace=..., outcolorspace=...)` the oracle uses
/// (packed samples, no extra channels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    /// `'gray'`: one sample per pixel, 0..1 for floats.
    Gray,
    /// `'rgb'`: three samples, 0..1 for floats.
    Rgb,
    /// `'cmyk'`: four samples; LittleCMS float CMYK is in percent (0..100).
    Cmyk,
    /// `'lab'`: L 0..100 and signed a, b for floats.
    Lab,
}

impl ColorSpace {
    /// Samples per pixel.
    pub fn channels(self) -> usize {
        match self {
            ColorSpace::Gray => 1,
            ColorSpace::Rgb | ColorSpace::Lab => 3,
            ColorSpace::Cmyk => 4,
        }
    }

    fn layout(self) -> (u32, u32) {
        let pixel_type = match self {
            ColorSpace::Gray => format::PT_GRAY,
            ColorSpace::Rgb => format::PT_RGB,
            ColorSpace::Cmyk => format::PT_CMYK,
            ColorSpace::Lab => format::PT_LAB,
        };
        (pixel_type, self.channels() as u32)
    }
}

/// Port of `imagecodecs.cms_transform(data, profile, outprofile, colorspace=colorspace,
/// outcolorspace=outcolorspace, outdtype=O, intent=1)` on packed samples (`samples.len()` a multiple of the
/// input channel count): both profiles opened in the global context, `cmsCreateTransform` with the formats
/// imagecodecs derives (`COLORSPACE_SH | CHANNELS_SH | BYTES_SH`, `FLOAT_SH` for floats), relative
/// colorimetric intent, flags 0, then one `cmsDoTransform` over all pixels. This is how `rasterio._linear`
/// converts grey (`data[..., 0]`, a 2-D array), CMYK (percent) and CIELAB samples to the working profile, and
/// how `_gray8_table` converts the 256 grey levels.
pub fn cms_transform<I: Sample, O: Sample>(
    samples: &[I],
    source: &Profile,
    colorspace: ColorSpace,
    destination: &Profile,
    outcolorspace: ColorSpace,
) -> Result<Vec<O>> {
    if !samples.len().is_multiple_of(colorspace.channels()) {
        return Err(CmsError::SamplesShape);
    }
    let incoming = ProfileHandle::open(source.bytes()).ok_or(CmsError::OpenFailed)?;
    let outgoing = ProfileHandle::open(destination.bytes()).ok_or(CmsError::OpenFailed)?;
    let mut transform =
        sys::LocalTransform::<I, O>::packed(&incoming, colorspace.layout(), &outgoing, outcolorspace.layout())
            .ok_or(CmsError::CreateTransformFailed)?;
    drop((incoming, outgoing));
    let pixels = samples.len() / colorspace.channels();
    let mut result = vec![O::default(); pixels * outcolorspace.channels()];
    if pixels > 0 {
        transform.apply(samples, &mut result);
    }
    Ok(result)
}

/// [`cms_transform`] on an H×W×C float32 image (C = the input channel count) to an H×W×C' float32 image: the
/// float32 calls of `rasterio._linear` (grey, CMYK in percent, CIELAB to the RGB working profile).
pub fn cms_transform_image(
    pixels: &Image,
    source: &Profile,
    colorspace: ColorSpace,
    destination: &Profile,
    outcolorspace: ColorSpace,
) -> Result<Image> {
    let (h, w, c) = pixels.dim();
    if c != colorspace.channels() {
        return Err(CmsError::SamplesShape);
    }
    let input = pixels.as_standard_layout();
    let samples = input.as_slice().expect("standard layout");
    let output = cms_transform::<f32, f32>(samples, source, colorspace, destination, outcolorspace)?;
    Ok(Image::from_shape_vec((h, w, outcolorspace.channels()), output).expect("pixel count kept"))
}

/// Port of luma/colorio.py `convert` for an H×W×3 float32 image (float32 out).
pub fn convert(rgb: &Image, source: &Profile, destination: &Profile) -> Result<Image> {
    if rgb.dim().2 != 3 {
        return Err(CmsError::Float32RgbRequired);
    }
    let input = rgb.as_standard_layout();
    let samples = input.as_slice().expect("standard layout");
    let output = convert_samples::<f32, f32>(samples, source, destination)?;
    Ok(Image::from_shape_vec(rgb.raw_dim(), output).expect("same shape"))
}

/// Port of luma/native_color.py `convert8_planar`: 8-bit RGB (H×W×3) to float planes (3×H×W), the same values
/// as `convert(rgb8 / 255)` without the float input frame. Rows are split over the rayon pool for large frames.
pub fn convert8_planar(rgb8: ArrayView3<'_, u8>, source: &Profile, destination: &Profile) -> Result<Array3<f32>> {
    check_native_sizes(source, destination)?;
    let (h, w, c) = rgb8.dim();
    if c != 3 {
        return Err(CmsError::Rgb8Required);
    }
    let mut result = Array3::<f32>::zeros((3, h, w));
    if h * w == 0 {
        return Ok(result);
    }
    if h * w * 4 > u32::MAX as usize {
        return Err(CmsError::PlaneStrideTooLarge);
    }
    let transform = CACHE.get(source, destination, true)?;
    let input = rgb8.as_standard_layout();
    let samples = input.as_slice().expect("standard layout");
    let workers = rayon::current_num_threads().max(1);
    let planes = result.as_slice_mut().expect("standard layout");
    if h * w < PARALLEL_PIXELS || workers == 1 {
        transform.inner.rows_to_planes(samples, planes, w, h);
        return Ok(result);
    }
    let size = ((128 * 1024) / w.max(1)).min(h.div_ceil(workers)).max(1);
    let blocks: Vec<(usize, Vec<f32>)> = (0..h)
        .step_by(size)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|first| {
            let count = size.min(h - first);
            let mut block = vec![0f32; 3 * count * w];
            transform.inner.rows_to_planes(&samples[first * w * 3..(first + count) * w * 3], &mut block, w, count);
            (first, block)
        })
        .collect();
    for (first, block) in blocks {
        let count = block.len() / (3 * w);
        for channel in 0..3 {
            let to = channel * h * w + first * w;
            planes[to..to + count * w].copy_from_slice(&block[channel * count * w..(channel + 1) * count * w]);
        }
    }
    Ok(result)
}
