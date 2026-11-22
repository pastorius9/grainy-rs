//! Port of `native_gpu.set_enabled/status` and `app.MainWindow`'s GPU warm-up and settings action,
//! extended with Phase 2 zero-copy single `wgpu::Device` sharing (`eframe::egui_wgpu::RenderState`),
//! GPU 33³ tetrahedral monitor ICC LUTs, GPU histogram/clipping, and Phase 3 1:1 viewport tiles.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use grainy_core::Image;
use grainy_engine::{
    gpu::{develop_gpu_resident, GpuCache, GpuOutput, TileRequest},
    Options, Source,
};
use grainy_gpu::{AdapterKind, Gpu, MonitorLut};
use serde_json::{Map, Value};

use crate::histogram::{Bins, Channels};
use crate::render::GpuPreview;

/// Handle to `eframe`'s `wgpu` adapter/device/queue so `grainy-gpu` computes directly into
/// textures that `egui_wgpu::Renderer` displays with zero CPU pixel readbacks or uploads.
#[derive(Clone)]
pub struct SharedWgpu {
    pub adapter: grainy_gpu::wgpu::Adapter,
    pub device: grainy_gpu::wgpu::Device,
    pub queue: grainy_gpu::wgpu::Queue,
}

impl SharedWgpu {
    pub fn from_render_state(rs: &eframe::egui_wgpu::RenderState) -> Self {
        Self { adapter: rs.adapter.clone(), device: rs.device.clone(), queue: rs.queue.clone() }
    }
}

/// Output of a GPU preview render attempt.
pub(crate) enum PreviewGpuOutput {
    /// Zero-copy GPU-resident RGBA8 texture (already monitor-ICC corrected on GPU) + 1.5 KB stats.
    ZeroCopy {
        preview: Arc<GpuPreview>,
        bins: Box<Bins>,
        channels: Channels,
        tile_uv: Option<[f32; 4]>,
        full_size: (usize, usize),
    },
    /// CPU-side developed working-space image (CPU fallback, masks tail, or standalone GPU without window).
    Host(Image),
}

/// Port of `native_gpu.status` and `enabled`, observed without waiting for the GPU thread.
#[derive(Clone, Debug)]
pub struct Status {
    /// Whether the user requested acceleration (also respects `GRAINY_GPU=0`).
    pub enabled: bool,
    /// Whether the GPU device has not failed into CPU fallback.
    pub usable: bool,
    /// Adapter name, `off`, or `unavailable (reason)`, as in the oracle.
    pub text: String,
}

pub(crate) struct Control {
    enabled: AtomicBool,
    usable: AtomicBool,
    environment_off: bool,
    status: Mutex<String>,
}

impl Control {
    pub(crate) fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            usable: AtomicBool::new(true),
            environment_off: std::env::var("GRAINY_GPU").as_deref() == Ok("0")
                || crate::context::FROZEN_TIMING.load(Ordering::Relaxed),
            status: Mutex::new("enabled (on demand)".into()),
        })
    }

    pub(crate) fn requested(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed) && !self.environment_off
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn set_usable(&self, usable: bool) {
        self.usable.store(usable, Ordering::Relaxed);
    }

    fn message(&self, text: String) {
        *self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = text;
    }

    pub(crate) fn status(&self) -> Status {
        let enabled = self.enabled();
        let usable = enabled && self.usable.load(Ordering::Relaxed);
        Status {
            enabled,
            usable,
            text: if enabled {
                self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
            } else {
                "off".into()
            },
        }
    }
}

type LutCacheEntry = (String, Option<grainy_cms::Profile>, crate::render::SoftProof, MonitorLut);

pub(crate) struct PreviewGpu {
    control: Arc<Control>,
    shared: Option<SharedWgpu>,
    shared_device: bool,
    gpu: Option<Arc<Gpu>>,
    lut_cache: Option<LutCacheEntry>,
    cache: [GpuCache; 2],
    attempted: bool,
    hits: u64,
}

impl PreviewGpu {
    #[cfg(test)]
    pub(crate) fn new(control: Arc<Control>) -> Self {
        Self::with_shared(control, None)
    }

    pub(crate) fn with_shared(control: Arc<Control>, shared: Option<SharedWgpu>) -> Self {
        Self {
            control,
            shared,
            shared_device: false,
            gpu: None,
            lut_cache: None,
            cache: Default::default(),
            attempted: false,
            hits: 0,
        }
    }

    pub(crate) fn has_shared_device(&self) -> bool {
        self.control.enabled() && self.shared.is_some() && (!self.attempted || self.shared_device)
    }

    fn warm_up(&mut self) {
        if self.attempted || !self.control.enabled() {
            return;
        }
        self.attempted = true;
        let warp = std::env::var("GRAINY_GPU").as_deref() == Ok("warp");
        let kind = if warp { AdapterKind::Software } else { AdapterKind::Hardware };

        // Prefer sharing eframe's wgpu::Device so compute outputs bind directly into egui_wgpu
        // with zero PCIe readback/upload.
        if let Some(shared) = &self.shared {
            let is_cpu = shared.adapter.get_info().device_type == grainy_gpu::wgpu::DeviceType::Cpu;
            if warp || !is_cpu {
                match catch_unwind(AssertUnwindSafe(|| {
                    Gpu::from_shared(&shared.adapter, shared.device.clone(), shared.queue.clone())
                })) {
                    Ok(Ok(gpu)) => {
                        self.control.message(format!("{} (Zero-Copy)", gpu.adapter_info().name));
                        self.gpu = Some(Arc::new(gpu));
                        self.shared_device = true;
                        return;
                    }
                    Ok(Err(error)) => {
                        log::warn!(
                            "Shared wgpu device unavailable for compute ({error}); opening dedicated compute device"
                        );
                    }
                    Err(_) => {
                        log::warn!("Shared wgpu device init panicked; opening dedicated compute device");
                    }
                }
            }
        }

        match catch_unwind(AssertUnwindSafe(|| Gpu::with_adapter(kind))) {
            Ok(Ok(Some(gpu))) => {
                self.control.message(gpu.adapter_info().name.clone());
                self.gpu = Some(Arc::new(gpu));
                self.shared_device = false;
            }
            Ok(Ok(None)) => self.fail("no adapter"),
            Ok(Err(error)) => self.fail(&error.to_string()),
            Err(_) => self.fail("initialization failed"),
        }
    }

    fn fail(&mut self, reason: &str) {
        self.cache = Default::default();
        self.lut_cache = None;
        self.hits = 0;
        self.gpu = None;
        self.shared_device = false;
        self.control.set_usable(false);
        self.control.message(format!("unavailable ({reason})"));
        log::warn!("GPU unavailable ({reason}); previews use CPU");
    }

    fn monitor_lut(
        &mut self,
        space: &str,
        monitor: Option<&grainy_cms::Profile>,
        soft_proof: &crate::render::SoftProof,
    ) -> Result<Option<MonitorLut>, (bool, String)> {
        let needs_srgb = space == "ProPhoto";
        if !needs_srgb && monitor.is_none() && !soft_proof.enabled {
            return Ok(None);
        }
        if let Some((cached_space, cached_mon, cached_proof, lut)) = &self.lut_cache {
            if cached_space == space && cached_mon.as_ref() == monitor && cached_proof == soft_proof {
                return Ok(Some(lut.clone()));
            }
        }
        let gpu = self.gpu.as_ref().ok_or_else(|| (true, "missing GPU".to_owned()))?;
        let grid = grainy_gpu::lut33_input_grid();
        let src_name = if space == "ProPhoto" { "Luma Wide" } else { space };
        let src = grainy_cms::profile(src_name).map_err(|e| (false, e.to_string()))?;
        let srgb = grainy_cms::profile("sRGB").map_err(|e| (false, e.to_string()))?;
        let srgb_lut = if needs_srgb {
            Some(grainy_cms::native_convert(&grid, &src, &srgb).map_err(|e| (false, e.to_string()))?)
        } else {
            None
        };
        let display_lut = if soft_proof.enabled {
            let proof_profile = grainy_cms::profile(&soft_proof.target).map_err(|e| (false, e.to_string()))?;
            let display = monitor.unwrap_or(&srgb);
            Some(
                grainy_cms::soft_proof(
                    &grid,
                    &src,
                    &proof_profile,
                    display,
                    grainy_cms::ProofOptions {
                        intent: grainy_cms::RenderingIntent::RelativeColorimetric,
                        black_point_compensation: soft_proof.bpc,
                        gamut_warning: soft_proof.gamut_warning,
                    },
                )
                .map_err(|e| (false, e.to_string()))?,
            )
        } else {
            match monitor {
                Some(profile) => {
                    Some(grainy_cms::native_convert(&grid, &src, profile).map_err(|e| (false, e.to_string()))?)
                }
                None => srgb_lut.clone(),
            }
        };
        let lut = gpu.upload_monitor_lut(display_lut.as_deref(), srgb_lut.as_deref()).map_err(|e| {
            let device_err = !matches!(e, grainy_gpu::Error::Limit(_) | grainy_gpu::Error::Input(_));
            (device_err, e.to_string())
        })?;
        self.lut_cache = Some((space.to_owned(), monitor.cloned(), soft_proof.clone(), lut.clone()));
        Ok(Some(lut))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        source: Source<'_>,
        settings: &Map<String, Value>,
        options: &Options<'_>,
        token: u64,
        resolution: crate::render::Resolution,
        space: &str,
        monitor: Option<&grainy_cms::Profile>,
        soft_proof: &crate::render::SoftProof,
        tile: Option<TileRequest>,
    ) -> Option<Result<PreviewGpuOutput, String>> {
        if !self.control.enabled() {
            return None;
        }
        self.warm_up();
        let gpu = self.gpu.clone()?;
        // Without a shared zero-copy device, keep ProPhoto on the canonical CPU CMS path.
        if space == "ProPhoto" && !self.shared_device {
            return None;
        }
        let cache_idx = usize::from(resolution == crate::render::Resolution::Full);
        let before = self.cache[cache_idx].runs();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            develop_gpu_resident(source, settings, options, &gpu, &mut self.cache[cache_idx], token, tile)
        }));
        match outcome {
            Ok(Ok(output)) => {
                let after = self.cache[cache_idx].runs();
                if before.uploads == after.uploads {
                    let slots = match self.cache[cache_idx].fallback() {
                        Some(grainy_gpu::CpuStage::Tone) => 0,
                        Some(grainy_gpu::CpuStage::Color) => 1,
                        Some(grainy_gpu::CpuStage::Detail) => 2,
                        None => 4,
                    };
                    self.hits += [
                        before.tone == after.tone,
                        before.color == after.color,
                        before.detail == after.detail,
                        before.finish == after.finish,
                    ]
                    .into_iter()
                    .take(slots)
                    .filter(|hit| *hit)
                    .count() as u64;
                }
                match output {
                    GpuOutput::Resident { frame, present_crop, tile_uv, full_size } => {
                        if self.shared_device {
                            let lut = match self.monitor_lut(space, monitor, soft_proof) {
                                Ok(lut) => lut,
                                Err((device_err, err)) => {
                                    if device_err {
                                        self.fail(&err);
                                    } else {
                                        log::warn!("GPU monitor LUT skipped ({err}); using CPU");
                                    }
                                    return None;
                                }
                            };
                            let compute_hist = tile_uv.is_none();
                            match catch_unwind(AssertUnwindSafe(|| {
                                gpu.present_frame(&frame, lut.as_ref(), present_crop, compute_hist)
                            })) {
                                Ok(Ok(presented)) => {
                                    let bins = presented.bins;
                                    let channels = presented.channels;
                                    let preview = Arc::new(GpuPreview::new(gpu, presented));
                                    Some(Ok(PreviewGpuOutput::ZeroCopy {
                                        preview,
                                        bins: Box::new(bins),
                                        channels,
                                        tile_uv,
                                        full_size,
                                    }))
                                }
                                Ok(Err(grainy_gpu::Error::Limit(reason) | grainy_gpu::Error::Input(reason))) => {
                                    log::warn!("GPU zero-copy present skipped ({reason}); using CPU");
                                    None
                                }
                                Ok(Err(error)) => {
                                    self.fail(&error.to_string());
                                    None
                                }
                                Err(_) => {
                                    self.fail("present failed");
                                    None
                                }
                            }
                        } else {
                            // Standalone compute device (no shared egui_wgpu renderer): read back float frame.
                            match catch_unwind(AssertUnwindSafe(|| gpu.readback(&frame))) {
                                Ok(Ok(image)) => Some(Ok(PreviewGpuOutput::Host(image))),
                                Ok(Err(error)) => {
                                    self.fail(&error.to_string());
                                    None
                                }
                                Err(_) => {
                                    self.fail("readback failed");
                                    None
                                }
                            }
                        }
                    }
                    GpuOutput::Host(image) => Some(Ok(PreviewGpuOutput::Host(image))),
                }
            }
            Ok(Err(grainy_engine::gpu::Error::Engine(error))) => Some(Err(error.to_string())),
            Ok(Err(grainy_engine::gpu::Error::Gpu(
                grainy_gpu::Error::Limit(reason) | grainy_gpu::Error::Input(reason),
            ))) => {
                log::warn!("GPU cannot render this preview ({reason}); use CPU");
                None
            }
            Ok(Err(error)) => {
                self.fail(&error.to_string());
                None
            }
            Err(_) => {
                self.fail("render failed");
                None
            }
        }
    }

    pub(crate) fn counters(&self, resolution: crate::render::Resolution) -> (u64, u64) {
        let cache = &self.cache[usize::from(resolution == crate::render::Resolution::Full)];
        let runs = cache.runs();
        (
            cache.host_cache().hits() + self.hits,
            cache.host_cache().misses() + runs.tone + runs.color + runs.detail + runs.finish,
        )
    }
}

/// Installs the preview worker sharing `eframe`'s `wgpu_render_state` for zero-copy GPU presentation.
pub(crate) fn install_with_wgpu(
    cx: &mut crate::context::AppContext,
    render_state: Option<eframe::egui_wgpu::RenderState>,
) {
    let enabled = cx
        .data_folder
        .as_ref()
        .map(|folder| grainy_platform::preferences::Preferences::new(folder.join("settings.json")).use_gpu())
        .unwrap_or(true);
    let shared = render_state.as_ref().map(SharedWgpu::from_render_state);
    cx.wgpu_render_state = render_state;
    let mut worker = crate::worker::Worker::with_gpu_shared(enabled, shared);
    let ctx = cx.egui.clone();
    crate::render::Renderer::set_waker(&mut worker, Arc::new(move || ctx.request_repaint()));
    cx.renderer = Box::new(worker);
}

/// Port of `app.update_gpu_action` and `set_gpu`: checkable action, adapter tooltip and status-bar message.
pub(crate) fn menu(ui: &mut eframe::egui::Ui, cx: &mut crate::context::AppContext) {
    use crate::text::{tr, tr_with};
    let status = cx.renderer.gpu_status();
    let enabled = status.as_ref().is_some_and(|status| status.enabled);
    let state = status.as_ref().map_or_else(|| tr("꺼짐"), |s| if s.enabled { s.text.clone() } else { tr("꺼짐") });
    let response = ui
        .add_enabled(status.is_some(), eframe::egui::Button::new(tr("GPU 가속 사용")).selected(enabled))
        .on_hover_text(tr_with("GPU: {0}", &[&state]));
    if response.clicked() {
        cx.renderer.set_gpu_enabled(!enabled);
        if let Some(folder) = &cx.data_folder {
            if let Err(error) =
                grainy_platform::preferences::Preferences::new(folder.join("settings.json")).save_use_gpu(!enabled)
            {
                log::warn!("GPU preference not saved: {error}");
            }
        }
        let status = cx.renderer.gpu_status();
        let state = status.map_or_else(|| tr("꺼짐"), |s| if s.enabled { s.text } else { tr("꺼짐") });
        cx.set_status(tr_with("GPU 가속: {0}", &[&state]));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_is_lazy_when_disabled_and_failure_releases_the_context() {
        let control = Control::new(false);
        let mut state = PreviewGpu::new(control.clone());
        assert!(!state.attempted);
        assert!(state.gpu.is_none());
        assert_eq!(control.status().text, "off");
        // Inject the same state transition used by device loss and initialization errors.
        state.attempted = true;
        state.fail("device lost (test)");
        control.set_enabled(true);
        if !control.environment_off {
            assert_eq!(control.status().text, "unavailable (device lost (test))");
        }
        let image = Image::zeros((2, 3, 3));
        assert!(state
            .render(
                Source::Linear(&image),
                &Map::new(),
                &Options::default(),
                1,
                crate::render::Resolution::Fit,
                "sRGB",
                None,
                &crate::render::SoftProof::default(),
                None,
            )
            .is_none());
        assert!(state.gpu.is_none());
        assert_eq!(state.cache[0].runs(), grainy_gpu::StageRuns::default());
    }
}
