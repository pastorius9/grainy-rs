//! Port of `native_gpu.set_enabled/status` and `app.MainWindow`'s GPU warm-up and settings action.
//! Compute and shader compilation live on the preview thread; thumbnails and exports retain their CPU lane.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use grainy_core::Image;
use grainy_engine::{
    gpu::{develop_gpu_cached, GpuCache},
    Options, Source,
};
use grainy_gpu::{AdapterKind, Gpu};
use serde_json::{Map, Value};

/// Port of `native_gpu.status` and `enabled`, observed without waiting for the GPU thread.
#[derive(Clone, Debug)]
pub struct Status {
    /// Whether the user requested acceleration (also respects `GRAINY_GPU=0`).
    pub enabled: bool,
    /// Adapter name, `off`, or `unavailable (reason)`, as in the oracle.
    pub text: String,
}

pub(crate) struct Control {
    enabled: AtomicBool,
    environment_off: bool,
    status: Mutex<String>,
}

impl Control {
    pub(crate) fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            environment_off: std::env::var("GRAINY_GPU").as_deref() == Ok("0")
                || crate::context::FROZEN_TIMING.load(Ordering::Relaxed),
            status: Mutex::new("enabled (on demand)".into()),
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed) && !self.environment_off
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn message(&self, text: String) {
        *self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = text;
    }

    pub(crate) fn status(&self) -> Status {
        let enabled = self.enabled();
        Status {
            enabled,
            text: if enabled {
                self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
            } else {
                "off".into()
            },
        }
    }
}

pub(crate) struct PreviewGpu {
    control: Arc<Control>,
    gpu: Option<Gpu>,
    cache: [GpuCache; 2],
    attempted: bool,
    hits: u64,
}

impl PreviewGpu {
    pub(crate) fn new(control: Arc<Control>) -> Self {
        Self { control, gpu: None, cache: Default::default(), attempted: false, hits: 0 }
    }

    fn warm_up(&mut self) {
        if self.attempted || !self.control.enabled() {
            return;
        }
        self.attempted = true;
        let kind = if std::env::var("GRAINY_GPU").as_deref() == Ok("warp") {
            AdapterKind::Software
        } else {
            AdapterKind::Hardware
        };
        match catch_unwind(AssertUnwindSafe(|| Gpu::with_adapter(kind))) {
            Ok(Ok(Some(gpu))) => {
                self.control.message(gpu.adapter_info().name.clone());
                self.gpu = Some(gpu);
            }
            Ok(Ok(None)) => self.fail("no adapter"),
            Ok(Err(error)) => self.fail(&error.to_string()),
            Err(_) => self.fail("initialization failed"),
        }
    }

    fn fail(&mut self, reason: &str) {
        self.cache = Default::default();
        self.hits = 0;
        self.gpu = None;
        self.control.message(format!("unavailable ({reason})"));
        log::warn!("GPU unavailable ({reason}); previews use CPU");
    }

    pub(crate) fn render(
        &mut self,
        source: Source<'_>,
        settings: &Map<String, Value>,
        options: &Options<'_>,
        token: u64,
        resolution: crate::render::Resolution,
    ) -> Option<Result<Image, String>> {
        if !self.control.enabled() {
            return None;
        }
        self.warm_up();
        let gpu = self.gpu.as_ref()?;
        let cache = &mut self.cache[usize::from(resolution == crate::render::Resolution::Full)];
        let before = cache.runs();
        match catch_unwind(AssertUnwindSafe(|| develop_gpu_cached(source, settings, options, gpu, cache, token))) {
            Ok(Ok(image)) => {
                let after = cache.runs();
                if before.uploads == after.uploads {
                    let slots = match cache.fallback() {
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
                Some(Ok(image))
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

/// Port of `app.MainWindow` GPU startup. Explicit in-memory constructors remain deterministic CPU test hosts.
pub(crate) fn install(cx: &mut crate::context::AppContext) {
    let enabled = cx
        .data_folder
        .as_ref()
        .map(|folder| grainy_platform::preferences::Preferences::new(folder.join("settings.json")).use_gpu())
        .unwrap_or(true);
    let mut worker = crate::worker::Worker::with_gpu(enabled);
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
            .render(Source::Linear(&image), &Map::new(), &Options::default(), 1, crate::render::Resolution::Fit)
            .is_none());
        assert!(state.gpu.is_none());
        assert_eq!(state.cache[0].runs(), grainy_gpu::StageRuns::default());
    }
}
