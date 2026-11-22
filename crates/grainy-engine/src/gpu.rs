//! Port of `luma/engine.py develop(cache=...)` and `native_gpu.develop_resident`.
//! CPU exports are unchanged. One preview worker owns this cache and its GPU. Source tokens have the same
//! contract as `develop_cached`: change the token whenever source pixels or camera calibration change.

use std::sync::Arc;

use grainy_core::Image;
use grainy_gpu::{CpuStage, Frame as GpuFrame, Gpu, PresentCrop, Resident, StageRuns, TileContext};
use grainy_tone::ToneBase;
use serde_json::{Map, Value};

use crate::{
    pixel_scale, settings_key, stages, working_settings, DevelopmentCache, Externals, Frame, Options, Output, Source,
    COLOR_KEYS, DETAIL_KEYS, GEOMETRY_KEYS, RAW_KEYS, TONE_KEYS,
};

#[cfg(test)]
mod tests;

/// Requested normalized `[u0, v0, u1, v1]` viewport region for true 1:1 GPU tile rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRequest {
    /// Normalized `[u0, v0, u1, v1]` in `[0, 1]` cropped image space.
    pub roi: [f32; 4],
    /// Overlap halo in full-resolution pixels so spatial filters remain seamless at tile edges.
    pub halo: usize,
}

/// Output of [`develop_gpu_resident`]: either a GPU-resident frame (for zero-copy presentation
/// directly into `egui_wgpu`) or a CPU host image when a CPU fallback or mask tail ran.
pub enum GpuOutput {
    Resident {
        frame: GpuFrame,
        present_crop: Option<PresentCrop>,
        tile_uv: Option<[f32; 4]>,
        full_size: (usize, usize),
    },
    Host(Image),
}

/// Errors of the GPU branch of `engine.develop`; device errors let the worker retry this request on the CPU.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A CPU prefix, fallback or output operation failed.
    #[error(transparent)]
    Engine(#[from] crate::Error),
    /// GPU initialization, input, resource or device failure.
    #[error(transparent)]
    Gpu(#[from] grainy_gpu::Error),
}

/// Port of `engine.develop`'s worker-owned host cache and `native_gpu` resident slots.
pub struct GpuCache {
    cpu: DevelopmentCache,
    resident: Option<(String, Resident)>,
    whole_base: Option<(String, ToneBase)>,
    fallback: Option<CpuStage>,
}

impl Default for GpuCache {
    fn default() -> Self {
        Self::new(DevelopmentCache::growing(256 << 20, 4.0, Some(1536 << 20)))
    }
}

impl GpuCache {
    /// Port of `DevelopmentCache` construction with an explicit host-memory budget.
    pub fn new(cpu: DevelopmentCache) -> Self {
        Self { cpu, resident: None, whole_base: None, fallback: None }
    }

    /// Port of resident dispatch diagnostics (counts for the currently uploaded geometry).
    pub fn runs(&self) -> StageRuns {
        self.resident.as_ref().map_or(StageRuns::default(), |(_, r)| r.runs())
    }

    /// First CPU stage of the last successful `develop`, if a setting needs a fallback.
    pub fn fallback(&self) -> Option<CpuStage> {
        self.fallback
    }

    /// Host cache counters for the preview diagnostics of `engine.develop`.
    pub fn host_cache(&self) -> &DevelopmentCache {
        &self.cpu
    }
}

/// Port of `engine.develop` with cached CPU geometry and resident GPU tone/colour/detail/grain.
/// Unsupported stages and their successors use the CPU. Masks always follow detail/grain and precede clip.
/// Callers retry with `develop_cached` on a GPU error; neither export nor the CPU reference is changed.
pub fn develop_gpu_cached<'a>(
    source: impl Into<Source<'a>>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
    gpu: &Gpu,
    cache: &mut GpuCache,
    token: u64,
) -> Result<Image, Error> {
    match develop_gpu_resident(source, settings, options, gpu, cache, token, None)? {
        GpuOutput::Resident { frame, .. } => Ok(gpu.readback(&frame)?),
        GpuOutput::Host(image) => Ok(image),
    }
}

/// Zero-copy GPU develop entry point: keeps the final float32 frame resident in VRAM when no CPU
/// fallback or CPU local mask tail ran, and supports padded 1:1 viewport tiles with shared whole-frame
/// `ToneBase` analysis so zoomed edits stay seamless at 60 fps on high-MP RAWs.
pub fn develop_gpu_resident<'a>(
    source: impl Into<Source<'a>>,
    settings: &Map<String, Value>,
    options: &Options<'_>,
    gpu: &Gpu,
    cache: &mut GpuCache,
    token: u64,
    tile: Option<TileRequest>,
) -> Result<GpuOutput, Error> {
    let source = source.into();
    if source.pixels().dim().2 != 3 {
        return Err(crate::Error::Shape(source.pixels().shape().to_vec()).into());
    }
    let s = working_settings(settings);
    let scale = pixel_scale(source.pixels(), options.original_size)?;
    cache.cpu.bind(token, source.pixels().len() * std::mem::size_of::<f32>());
    // LittleCMS uses quantized tables: a GPU ULP can cross a table boundary and exceed the preview's
    // 2e-5 tolerance after conversion. Keep a canonical CPU input for nonidentity output transforms.
    let cms_output = !matches!(options.output, Output::Working)
        && (s.str("working_space") == "ProPhoto" || !matches!(options.output, Output::Named("sRGB")));
    // HDR needs the CPU colour/detail tail. Keep its extended-range result bit-exact throughout too.
    if s.bool("hdr") || cms_output {
        cache.fallback = Some(CpuStage::Tone);
        return Ok(GpuOutput::Host(crate::develop_cached(source, settings, options, &mut cache.cpu, token)?));
    }
    let profile_lens = if options.externals.lens.is_none() && s.bool("lensfun_enabled") && s.bool("lensfun") {
        Some(grainy_lens::ProfileModel::from_settings(&s).map_err(crate::Error::from)?)
    } else {
        None
    };
    let externals = Externals {
        lens: options.externals.lens.or(profile_lens.as_ref().map(|model| model as &dyn grainy_geometry::LensModel)),
        ..options.externals
    };
    if let Some(missing) = stages::pending(&s, &externals) {
        return Err(crate::Error::Pending(missing).into());
    }
    let usable = source.pixels().len() * 8 <= cache.cpu.max_bytes();
    let version = cache.cpu.version();
    let mut host = usable.then_some(&mut cache.cpu);
    let (raw_key, camera_linear) = match source {
        Source::Camera(camera) => {
            let mut key = settings_key(&s, &RAW_KEYS);
            if s.i64("tone_version") >= 4 {
                key.push_str("|pv4");
            }
            let image = crate::stage(&mut host, "raw", &key, || Ok(grainy_raw::rawcolor::develop_camera(camera, &s)?))?;
            (key, Frame::Stage(image))
        }
        Source::Linear(image) => ("-".to_owned(), Frame::Source(image)),
    };
    let negative_key =
        s.bool("negative").then(|| format!("{raw_key}|{}", settings_key(&s, &grainy_film::negative::KEYS)));
    let linear = match &negative_key {
        Some(key) => Frame::Stage(crate::stage(&mut host, "negative", key, || {
            Ok(grainy_film::negative::apply(&camera_linear, &s))
        })?),
        None => camera_linear,
    };
    let geometry_key = format!(
        "{raw_key}|{}|{:?}|{}",
        negative_key.as_deref().unwrap_or("-"),
        options.use_crop,
        settings_key(&s, &GEOMETRY_KEYS)
    );
    let moved = match host.as_deref_mut().and_then(|host| host.lookup("geometry", &geometry_key)) {
        Some(image) => Some(image),
        None => stages::geometry(&linear, &s, options.use_crop, &externals)?.map(|image| match host.as_deref_mut() {
            Some(host) => host.store("geometry", &geometry_key, image),
            None => Arc::new(image),
        }),
    };
    let frame = moved.as_deref().unwrap_or(&linear);
    let (full_h, full_w, _) = frame.dim();
    let masks = s.value("masks").as_array().is_some_and(|layers| !layers.is_empty());

    // True 1:1 viewport tile fast path: slice only the visible tile + spatial halo from the
    // cached post-geometry frame and supply a shared whole-frame ToneBase so zoomed slider drags
    // develop ~2 MP instead of 45 MP with zero seams.
    if let Some(req) = tile.filter(|_| !masks && full_w.max(full_h) > 1536) {
        let u0 = req.roi[0].clamp(0.0, 1.0);
        let v0 = req.roi[1].clamp(0.0, 1.0);
        let u1 = req.roi[2].clamp(u0, 1.0);
        let v1 = req.roi[3].clamp(v0, 1.0);
        let x0 = ((u0 * full_w as f32).floor() as usize).min(full_w.saturating_sub(1));
        let y0 = ((v0 * full_h as f32).floor() as usize).min(full_h.saturating_sub(1));
        let x1 = ((u1 * full_w as f32).ceil() as usize).clamp(x0 + 1, full_w);
        let y1 = ((v1 * full_h as f32).ceil() as usize).clamp(y0 + 1, full_h);
        let tile_w = x1 - x0;
        let tile_h = y1 - y0;
        // Only tile when it meaningfully reduces pixel work (< 75% of full frame).
        if (tile_w as u64) * (tile_h as u64) * 4 < (full_w as u64) * (full_h as u64) * 3 {
            let px0 = x0.saturating_sub(req.halo);
            let py0 = y0.saturating_sub(req.halo);
            let px1 = (x1 + req.halo).min(full_w);
            let py1 = (y1 + req.halo).min(full_h);
            let pw = px1 - px0;
            let ph = py1 - py0;
            let tile_key = format!("tile|{token}|{version}|{geometry_key}|{px0},{py0},{pw},{ph}");
            if cache.resident.as_ref().is_none_or(|(key, _)| *key != tile_key) {
                let sub = frame.slice(ndarray::s![py0..py1, px0..px1, ..]).to_owned();
                match &mut cache.resident {
                    Some((key, resident)) => {
                        resident.replace(gpu, &sub)?;
                        *key = tile_key;
                    }
                    slot @ None => *slot = Some((tile_key, Resident::new(gpu, &sub)?)),
                }
            }
            let needs_base =
                s.f64("tone_version") >= 3.0 && !s.bool("hdr") && (s.bool("shadows") || s.bool("highlights"));
            let base_ref = if needs_base && !grainy_film::active(&s) && !grainy_tone::mapping::active(&s) {
                let base_keys = [
                    "exposure",
                    "temperature",
                    "tint",
                    "kelvin",
                    "wb_reference",
                    "kelvin_enabled",
                    "lens_vignette",
                    "hdr",
                    "tone_version",
                    "film_look",
                    "film_strength",
                    "working_space",
                    "tone_mapping",
                    "drt_contrast",
                    "drt_toe",
                    "drt_shoulder",
                    "drt_white",
                ];
                let bk = format!("{token}|{version}|{geometry_key}|{}", settings_key(&s, &base_keys));
                if cache.whole_base.as_ref().is_none_or(|(k, _)| *k != bk) {
                    let base = grainy_tone::tone_base(frame, &s, None).map_err(crate::Error::from)?;
                    cache.whole_base = Some((bk, base));
                }
                cache.whole_base.as_ref().map(|(_, b)| b)
            } else {
                None
            };
            let tile_ctx = TileContext {
                uv: [
                    px0 as f32 / full_w as f32,
                    py0 as f32 / full_h as f32,
                    pw as f32 / full_w as f32,
                    ph as f32 / full_h as f32,
                ],
                full_size: (full_w, full_h),
            };
            let resident =
                &mut cache.resident.as_mut().ok_or_else(|| grainy_gpu::Error::Device("missing resident".into()))?.1;
            let result = gpu.develop_resident_tile(resident, &s, scale, true, Some(tile_ctx), base_ref)?;
            if result.fallback.is_none() {
                cache.fallback = None;
                let gpu_frame = result.frame.ok_or_else(|| grainy_gpu::Error::Device("missing tile finish".into()))?;
                return Ok(GpuOutput::Resident {
                    frame: gpu_frame,
                    present_crop: Some(PresentCrop {
                        x: (x0 - px0) as u32,
                        y: (y0 - py0) as u32,
                        width: tile_w as u32,
                        height: tile_h as u32,
                    }),
                    tile_uv: Some([
                        x0 as f32 / full_w as f32,
                        y0 as f32 / full_h as f32,
                        x1 as f32 / full_w as f32,
                        y1 as f32 / full_h as f32,
                    ]),
                    full_size: (full_w, full_h),
                });
            }
            // Tile needed a CPU fallback (e.g. film look or DRT): fall through to full-frame path.
        }
    }

    let resident_key = format!("{token}|{version}|{:?}|{geometry_key}", frame.dim());
    if cache.resident.as_ref().is_none_or(|(key, _)| *key != resident_key) {
        match &mut cache.resident {
            Some((key, resident)) => {
                resident.replace(gpu, frame)?;
                *key = resident_key;
            }
            slot @ None => *slot = Some((resident_key, Resident::new(gpu, frame)?)),
        }
    }
    let resident = &mut cache.resident.as_mut().ok_or_else(|| grainy_gpu::Error::Device("missing resident".into()))?.1;
    let dispatch = grainy_core::timing::Span::new("gpu.resident_prefix");
    let result = gpu.develop_resident_prefix(resident, &s, scale, !masks)?;
    drop(dispatch);
    cache.fallback = result.fallback;
    let tone_key = format!("gpu|{geometry_key}|{}", settings_key(&s, &TONE_KEYS));
    let color_key = format!("{tone_key}|{}", settings_key(&s, &COLOR_KEYS));
    let detail_key = format!("{color_key}|{scale}|{}", settings_key(&s, &DETAIL_KEYS));
    // A CPU suffix gets the canonical CPU prefix too: nonlinear colour/detail operations can amplify an
    // upstream GPU ULP (all_garden measured 1.12e-4). Cache that replay independently, retaining GPU slots
    // for a later supported edit. Thus every unsupported path is bit-exact, not merely a CPU operation on
    // slightly different input pixels. No GPU readback is needed at a fallback boundary.
    let a = match result.fallback {
        Some(_) => {
            let toned = crate::stage(&mut host, "tone", &tone_key, || crate::tone(frame, &s))?;
            let colored =
                crate::stage(&mut host, "color", &color_key, || Ok(grainy_colorgrade::develop_color(&toned, &s)?))?;
            crate::stage(&mut host, "detail", &detail_key, || Ok(grainy_detail::develop_detail(&colored, &s, scale)?))?
        }
        None => {
            if let Some(host) = host.as_deref_mut() {
                host.discard(&["tone", "color", "detail"]);
            }
            let gpu_frame = result.frame.ok_or_else(|| grainy_gpu::Error::Device("missing finish".into()))?;
            // The finish shader already ran the final clip. Stay zero-copy in VRAM!
            if !masks {
                return Ok(GpuOutput::Resident {
                    frame: gpu_frame,
                    present_crop: None,
                    tile_uv: None,
                    full_size: (full_w, full_h),
                });
            }
            let readback = || -> Result<Image, Error> { Ok(gpu.readback(&gpu_frame)?) };
            match host.as_deref_mut() {
                Some(host) => host.evaluate("gpu-mask-detail", &detail_key, readback)?,
                None => Arc::new(readback()?),
            }
        }
    };
    let tail = grainy_core::timing::Span::new("gpu.host_tail");
    let a = Arc::try_unwrap(a).unwrap_or_else(|image| (*image).clone());
    let colour_source = if stages::needs_colour_source(&s) {
        let key = format!("{raw_key}|{}", negative_key.as_deref().unwrap_or("-"));
        Some(crate::stage(&mut host, "mask-source", &key, || Ok(grainy_color::to_srgb(&linear)))?)
    } else {
        None
    };
    let (height, width, _) = source.pixels().dim();
    let mut a = stages::local(a, (height, width), colour_source.as_deref(), &s, options.use_crop, scale, &externals)?;
    grainy_color::clip01_in_place(&mut a);
    drop(tail);
    let destination = match options.output {
        Output::Working => return Ok(GpuOutput::Host(a)),
        Output::Named("sRGB") if s.str("working_space") != "ProPhoto" => return Ok(GpuOutput::Host(a)),
        Output::Named(name) => grainy_cms::Destination::Named(name),
        Output::Icc(profile) => grainy_cms::Destination::Icc(profile),
    };
    Ok(GpuOutput::Host(grainy_cms::output_rgb(&a, s.str("working_space"), destination).map_err(crate::Error::from)?))
}
