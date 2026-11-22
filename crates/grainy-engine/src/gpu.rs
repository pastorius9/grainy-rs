//! Port of `luma/engine.py develop(cache=...)` and `native_gpu.develop_resident`.
//! CPU exports are unchanged. One preview worker owns this cache and its GPU. Source tokens have the same
//! contract as `develop_cached`: change the token whenever source pixels or camera calibration change.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use grainy_core::{Image, Plane};
use grainy_gpu::{
    can_gpu_local_layer, CpuStage, Frame as GpuFrame, Gpu, GpuGeometryParams, LocalMask, PresentCrop, Resident,
    StageRuns, TileContext,
};
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
    bound_token: Option<u64>,
    resident: Option<(String, Resident)>,
    raw_gpu: Option<(String, GpuFrame)>,
    whole_base: Option<(String, ToneBase)>,
    mask_planes: HashMap<String, Arc<Plane>>,
    gpu_masks: HashMap<String, LocalMask>,
    mask_uploads: u64,
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
        Self {
            cpu,
            bound_token: None,
            resident: None,
            raw_gpu: None,
            whole_base: None,
            mask_planes: HashMap::new(),
            gpu_masks: HashMap::new(),
            mask_uploads: 0,
            fallback: None,
        }
    }

    /// Number of GPU local mask buffer uploads performed by this cache (for slider-reuse verification).
    pub fn mask_uploads(&self) -> u64 {
        self.mask_uploads
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

fn layer_shape_key(layer: &Map<String, Value>) -> String {
    let mut stripped = Map::new();
    for (k, v) in layer {
        if k != "adjustments" {
            stripped.insert(k.clone(), v.clone());
        }
    }
    serde_json::to_string(&Value::Object(stripped)).unwrap_or_default()
}

type GpuMaskLayer<'a> = (&'a Map<String, Value>, Map<String, Value>);

fn gpu_eligible_masks(s: &grainy_core::Settings, scale: f64) -> Option<Vec<GpuMaskLayer<'_>>> {
    if s.bool("hdr") {
        return None;
    }
    let layers = s.value("masks").as_array()?;
    if layers.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for raw in layers {
        let layer = raw.as_object()?;
        let on = layer.get("enabled").is_none_or(grainy_core::settings::truthy);
        if !on {
            continue;
        }
        if grainy_local::image_dependent(layer) {
            return None;
        }
        let edit = match layer.get("adjustments") {
            None => Map::new(),
            Some(v) => v.as_object()?.clone(),
        };
        if !can_gpu_local_layer(&edit, scale) {
            return None;
        }
        out.push((layer, edit));
    }
    Some(out)
}

#[allow(clippy::too_many_arguments)]
fn prepare_mask_planes(
    mask_planes: &mut HashMap<String, Arc<Plane>>,
    frame: &Image,
    source_shape: (usize, usize),
    colour_source: Option<&Image>,
    s: &grainy_core::Settings,
    crop: crate::Crop,
    externals: &Externals<'_>,
    base_key: &str,
    layers: &[GpuMaskLayer<'_>],
) -> Result<Vec<(String, Arc<Plane>)>, Error> {
    let use_crop = match crop {
        crate::Crop::Cropped => grainy_geometry::UseCrop::Yes,
        crate::Crop::Uncropped => grainy_geometry::UseCrop::No,
        crate::Crop::All => grainy_geometry::UseCrop::All,
    };
    let transform = |mask: &Image| -> grainy_local::Result<Image> {
        grainy_geometry::geometry(mask, s, use_crop, externals.lens)
            .map(Cow::into_owned)
            .map_err(|e| grainy_local::Error::Invalid(e.to_string()))
    };
    let mut out = Vec::with_capacity(layers.len());
    for (idx, (layer, _)) in layers.iter().enumerate() {
        let key = format!("{base_key}|{idx}|{}", layer_shape_key(layer));
        let plane = if let Some(cached) = mask_planes.get(&key) {
            cached.clone()
        } else {
            let computed = Arc::new(
                grainy_local::layer_mask(frame, source_shape, layer, &transform, colour_source, externals.sky)
                    .map_err(crate::Error::from)?,
            );
            if mask_planes.len() >= 16 {
                mask_planes.clear();
            }
            mask_planes.insert(key.clone(), computed.clone());
            computed
        };
        out.push((key, plane));
    }
    Ok(out)
}

fn upload_cached_masks(
    gpu: &Gpu,
    gpu_masks: &mut HashMap<String, LocalMask>,
    mask_uploads: &mut u64,
    planes: &[(String, Arc<Plane>)],
    tile_slice: Option<(usize, usize, usize, usize)>,
) -> Result<Vec<LocalMask>, Error> {
    let mut out = Vec::with_capacity(planes.len());
    for (base_key, plane) in planes {
        let key = match tile_slice {
            Some((px0, py0, pw, ph)) => format!("tile|{base_key}|{px0},{py0},{pw},{ph}"),
            None => base_key.clone(),
        };
        if let Some(cached) = gpu_masks.get(&key) {
            out.push(cached.clone());
            continue;
        }
        let uploaded = match tile_slice {
            Some((px0, py0, pw, ph)) => {
                let sub = plane.slice(ndarray::s![py0..py0 + ph, px0..px0 + pw]).to_owned();
                gpu.upload_mask(&sub)?
            }
            None => gpu.upload_mask(plane)?,
        };
        if gpu_masks.len() >= 24 {
            gpu_masks.clear();
        }
        gpu_masks.insert(key, uploaded.clone());
        *mask_uploads += 1;
        out.push(uploaded);
    }
    Ok(out)
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
    if cache.bound_token != Some(token) {
        cache.bound_token = Some(token);
        cache.raw_gpu = None;
        cache.whole_base = None;
        cache.mask_planes.clear();
        cache.gpu_masks.clear();
    }
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
    let masks = s.value("masks").as_array().is_some_and(|layers| !layers.is_empty());
    let retouch_empty = s.value("retouch").as_array().is_none_or(Vec::is_empty);

    // PV4 Single-Pass GPU Catmull-Rom Geometry Warp fast path:
    // keeps the pre-geometry linear frame on GPU so Straighten/Perspective/Distortion/Crop drags
    // execute in one Bicubic GPU pass with zero CPU re-warping and zero PCIe re-upload.
    let (lin_h, lin_w, _) = linear.dim();
    let pv4_gpu_geom = if s.f64("tone_version") >= 4.0 && !s.bool("hdr") && !masks && tile.is_none() && retouch_empty {
        GpuGeometryParams::from_settings(
            lin_w,
            lin_h,
            &s,
            options.use_crop == crate::Crop::Cropped,
            options.use_crop == crate::Crop::All,
        )
    } else {
        None
    };
    if let Some(geom) = pv4_gpu_geom {
        let raw_gpu_key = format!("{token}|{version}|{raw_key}|{}", negative_key.as_deref().unwrap_or("-"));
        if cache.raw_gpu.as_ref().is_none_or(|(k, _)| *k != raw_gpu_key) {
            cache.raw_gpu = Some((raw_gpu_key, gpu.upload(&linear)?));
        }
        let resident_key = format!("pv4geo|{token}|{version}|{geometry_key}");
        if cache.resident.as_ref().is_none_or(|(k, _)| *k != resident_key) {
            let raw_frame = &cache.raw_gpu.as_ref().unwrap().1;
            let warped = gpu.warp_geometry(raw_frame, &geom)?;
            // Downscale source before small analysis warp, scaled by inverse crop fraction so tight
            // crops still yield ~512px of cropped content for ToneBase.
            let analysis_src = {
                let crop_frac = (geom.out_width as f64 / geom.rot_width.max(1) as f64)
                    .min(geom.out_height as f64 / geom.rot_height.max(1) as f64)
                    .clamp(0.125, 1.0);
                let small_limit = (512.0 / crop_frac) / lin_w.max(lin_h) as f64;
                let small = if small_limit < 1.0 {
                    let sw = ((lin_w as f64 * small_limit).round() as usize).max(1);
                    let sh = ((lin_h as f64 * small_limit).round() as usize).max(1);
                    grainy_imgproc::resize(&linear, (sw, sh), grainy_imgproc::Interpolation::Area)
                        .unwrap_or_else(|_| linear.to_owned())
                } else {
                    linear.to_owned()
                };
                stages::geometry(&small, &s, options.use_crop, &externals)?.unwrap_or(small)
            };
            match &mut cache.resident {
                Some((k, resident)) => {
                    resident.replace_frame(warped, analysis_src);
                    *k = resident_key;
                }
                slot @ None => *slot = Some((resident_key, Resident::from_frame(warped, analysis_src))),
            }
        }
        let resident =
            &mut cache.resident.as_mut().ok_or_else(|| grainy_gpu::Error::Device("missing resident".into()))?.1;
        let result = gpu.develop_resident_prefix(resident, &s, scale, true)?;
        if result.fallback.is_none() {
            cache.fallback = None;
            let gpu_frame = result.frame.ok_or_else(|| grainy_gpu::Error::Device("missing finish".into()))?;
            return Ok(GpuOutput::Resident {
                frame: gpu_frame,
                present_crop: None,
                tile_uv: None,
                full_size: (geom.out_width, geom.out_height),
            });
        }
    } else {
        cache.raw_gpu = None;
    }

    let moved = match host.as_deref_mut().and_then(|host| host.lookup("geometry", &geometry_key)) {
        Some(image) => Some(image),
        None => stages::geometry(&linear, &s, options.use_crop, &externals)?.map(|image| match host.as_deref_mut() {
            Some(host) => host.store("geometry", &geometry_key, image),
            None => Arc::new(image),
        }),
    };
    let frame = moved.as_deref().unwrap_or(&linear);
    let (full_h, full_w, _) = frame.dim();
    let gpu_masks_plan = gpu_eligible_masks(&s, scale);
    let masks_on_gpu = masks && gpu_masks_plan.is_some();
    let active_gpu_masks = gpu_masks_plan.as_ref().is_some_and(|l| !l.is_empty());
    let (src_h, src_w, _) = source.pixels().dim();
    let mask_base_key = format!("{token}|{version}|{geometry_key}");

    // True 1:1 viewport tile fast path: slice only the visible tile + spatial halo from the
    // cached post-geometry frame and supply a shared whole-frame ToneBase so zoomed slider drags
    // develop ~2 MP instead of 45 MP with zero seams (now also supporting Film, DRT, and GPU local masks!).
    // Skip tiling when globally position- or frame-statistics-dependent stages are active so panning
    // never shifts grain patterns, wavelet thresholds, or multi-level Laplacian pyramids.
    let tile_unsafe =
        s.bool("grain") || s.bool("noise_luma") || (s.bool("clarity") && s.f64("detail_version") >= 4.0);
    if let Some(req) = tile.filter(|_| !tile_unsafe && (!masks || masks_on_gpu) && full_w.max(full_h) > 1536) {
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
            let base_ref = if needs_base {
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
                    let base = if grainy_film::active(&s) {
                        let name = s.str("film_look").to_owned();
                        let space = grainy_film::WorkingSpace::from_setting(s.str("working_space"));
                        let render = move |a: ndarray::ArrayView3<'_, f32>| {
                            grainy_film::render(&a.to_owned(), &name, space).unwrap_or_else(|_| a.to_owned())
                        };
                        grainy_tone::tone_base(frame, &s, Some(&render)).map_err(crate::Error::from)?
                    } else {
                        grainy_tone::tone_base(frame, &s, None).map_err(crate::Error::from)?
                    };
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
            let result = gpu.develop_resident_tile(resident, &s, scale, !active_gpu_masks, Some(tile_ctx), base_ref)?;
            if result.fallback.is_none() {
                cache.fallback = None;
                let mut gpu_frame =
                    result.frame.ok_or_else(|| grainy_gpu::Error::Device("missing tile finish".into()))?;
                let mut ok_masked = true;
                if let Some(layers) = gpu_masks_plan.as_deref() {
                    if !layers.is_empty() {
                        let colour_source = if stages::needs_colour_source(&s) {
                            let key = format!("{raw_key}|{}", negative_key.as_deref().unwrap_or("-"));
                            Some(crate::stage(&mut host, "mask-source", &key, || Ok(grainy_color::to_srgb(&linear)))?)
                        } else {
                            None
                        };
                        let planes = prepare_mask_planes(
                            &mut cache.mask_planes,
                            frame,
                            (src_h, src_w),
                            colour_source.as_deref(),
                            &s,
                            options.use_crop,
                            &externals,
                            &mask_base_key,
                            layers,
                        )?;
                        let uploaded = upload_cached_masks(
                            gpu,
                            &mut cache.gpu_masks,
                            &mut cache.mask_uploads,
                            &planes,
                            Some((px0, py0, pw, ph)),
                        )?;
                        let pair_refs: Vec<(&LocalMask, &Map<String, Value>)> =
                            uploaded.iter().zip(layers.iter()).map(|(m, (_, e))| (m, e)).collect();
                        if let Some(masked) = gpu.apply_local_layers(&gpu_frame, &pair_refs, scale)? {
                            gpu_frame = masked;
                        } else {
                            ok_masked = false;
                        }
                    }
                }
                if ok_masked {
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
            }
            // Tile needed a fallback: fall through to full-frame path.
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
            // Zero-copy GPU local mask fast path: compute/cache mask planes & GPU buffers once,
            // then run all local mask adjustments directly in VRAM!
            if let Some(layers) = gpu_masks_plan.as_deref() {
                if layers.is_empty() {
                    // All mask layers are disabled: still clip to [0, 1] and stay zero-copy!
                    let clipped = gpu.grain_clip(&gpu_frame, None)?;
                    return Ok(GpuOutput::Resident {
                        frame: clipped,
                        present_crop: None,
                        tile_uv: None,
                        full_size: (full_w, full_h),
                    });
                }
                let colour_source = if stages::needs_colour_source(&s) {
                    let key = format!("{raw_key}|{}", negative_key.as_deref().unwrap_or("-"));
                    Some(crate::stage(&mut host, "mask-source", &key, || Ok(grainy_color::to_srgb(&linear)))?)
                } else {
                    None
                };
                let planes = prepare_mask_planes(
                    &mut cache.mask_planes,
                    frame,
                    (src_h, src_w),
                    colour_source.as_deref(),
                    &s,
                    options.use_crop,
                    &externals,
                    &mask_base_key,
                    layers,
                )?;
                let uploaded = upload_cached_masks(gpu, &mut cache.gpu_masks, &mut cache.mask_uploads, &planes, None)?;
                let pair_refs: Vec<(&LocalMask, &Map<String, Value>)> =
                    uploaded.iter().zip(layers.iter()).map(|(m, (_, e))| (m, e)).collect();
                if let Some(masked) = gpu.apply_local_layers(&gpu_frame, &pair_refs, scale)? {
                    return Ok(GpuOutput::Resident {
                        frame: masked,
                        present_crop: None,
                        tile_uv: None,
                        full_size: (full_w, full_h),
                    });
                }
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
