//! The render worker: background threads that load photos and develop them with the engine
//! (`grainy_engine::develop_cached`, the oracle's preview path with its `DevelopmentCache`), so the window never
//! waits. One thread develops the open photo's preview (the newest request wins: a slider drag never queues
//! stale renders); another makes thumbnails with each photo's own settings.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use grainy_codecs::ColorManager;
use grainy_core::Settings;
use grainy_engine::files::{load_photo, AnalysisSeed, Photo};
use grainy_engine::{develop, develop_cached, Crop, DevelopmentCache, Options, Output};
use serde_json::{Map, Value};

use crate::commands::auto_source::LoadSettings;
use crate::document::ThumbCache;
use crate::histogram;
use crate::render::{
    Frame, PrefetchJob, Preview, PreviewJob, Renderer, Reply, Resolution, SideFrame, SideJob, Thumbnail, ThumbnailJob,
    Waker,
};

/// Long side of the preview source in pixels (about a large view's long side in device pixels).
pub const PREVIEW_SIZE: usize = 2048;
/// Long side of a thumbnail (the oracle's stored previews are 320 pixels).
pub const THUMB_SIZE: usize = 320;
/// How many thumbnail sources stay loaded.
const THUMB_SOURCES: usize = 48;
/// How many preview sources stay loaded (the fit and full sources of the open photo, and the previous one).
const PREVIEW_SOURCES: usize = 3;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct Queues {
    preview: Option<PreviewJob>,
    /// Neighbours of the open photo to read ahead (the preview thread does it when it has nothing else to do).
    prefetch: Vec<PrefetchJob>,
    thumbnails: Vec<ThumbnailJob>,
    /// The second preview lane's requests, the newest per slot.
    side: Vec<SideJob>,
    /// Jobs taken and not answered yet.
    running: usize,
}

struct Shared {
    library: Option<PathBuf>,
    analysis_seed: Mutex<Option<(PathBuf, LoadSettings, AnalysisSeed)>>,
    queues: Mutex<Queues>,
    preview_ready: Condvar,
    thumbnail_ready: Condvar,
    side_ready: Condvar,
    side_replies: Mutex<Vec<SideFrame>>,
    replies: Mutex<Vec<Reply>>,
    waker: Mutex<Option<Waker>>,
    shutdown: AtomicBool,
    renders: AtomicU64,
    thumbnails: AtomicUsize,
    prefetched: AtomicUsize,
    gpu: Option<Arc<crate::gpu_worker::Control>>,
    shared_wgpu: Option<crate::gpu_worker::SharedWgpu>,
    /// The monitor's profile (`Renderer::set_monitor`), read when a job starts.
    monitor: Mutex<Option<grainy_cms::Profile>>,
    /// The HDR viewing choices (`Renderer::set_hdr_view`), read when a job starts.
    hdr_view: Mutex<crate::hdr::View>,
    /// The fitted photo's long side on screen (`Renderer::set_view_size`), read when a job starts.
    view: Mutex<Option<usize>>,
    /// Normalized `[u0, v0, u1, v1]` viewport region for true 1:1 tile rendering (`Renderer::set_tile_roi`).
    tile_roi: Mutex<Option<[f32; 4]>>,
}

impl Shared {
    fn answer(&self, reply: Reply) {
        lock(&self.replies).push(reply);
        lock(&self.queues).running -= 1;
        if let Some(waker) = lock(&self.waker).clone() {
            waker();
        }
    }
}

/// The render worker. Dropping it stops its threads after their current job.
pub struct Worker {
    shared: Arc<Shared>,
    last_preview: Option<PreviewJob>,
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}

impl Worker {
    /// Starts the preview and thumbnail threads.
    pub fn new() -> Self {
        Self::start(None, None, None)
    }

    /// Port of the oracle's preview worker with GPU warm-up and CPU retry on device failure.
    /// The ordinary constructor remains the CPU reference for tests and deterministic clients.
    pub fn with_gpu(enabled: bool) -> Self {
        Self::start(Some(crate::gpu_worker::Control::new(enabled)), None, None)
    }

    /// Preview worker sharing `eframe`'s `wgpu::Device` for zero-copy GPU texture presentation.
    pub fn with_gpu_shared(enabled: bool, shared_wgpu: Option<crate::gpu_worker::SharedWgpu>) -> Self {
        Self::start(Some(crate::gpu_worker::Control::new(enabled)), shared_wgpu, None)
    }

    fn start(
        gpu: Option<Arc<crate::gpu_worker::Control>>,
        shared_wgpu: Option<crate::gpu_worker::SharedWgpu>,
        library: Option<PathBuf>,
    ) -> Self {
        let shared = Arc::new(Shared {
            library,
            analysis_seed: Mutex::default(),
            queues: Mutex::new(Queues::default()),
            preview_ready: Condvar::new(),
            thumbnail_ready: Condvar::new(),
            side_ready: Condvar::new(),
            side_replies: Mutex::new(Vec::new()),
            replies: Mutex::new(Vec::new()),
            waker: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            renders: AtomicU64::new(0),
            thumbnails: AtomicUsize::new(0),
            prefetched: AtomicUsize::new(0),
            gpu,
            shared_wgpu,
            monitor: Mutex::new(None),
            hdr_view: Mutex::new(crate::hdr::View::default()),
            view: Mutex::new(None),
            tile_roi: Mutex::new(None),
        });
        let preview = shared.clone();
        let spawned = std::thread::Builder::new().name("grainy-preview".into()).spawn(move || preview_loop(&preview));
        if let Err(error) = spawned {
            log::error!("cannot start the preview thread: {error}");
        }
        let thumbs = shared.clone();
        let spawned =
            std::thread::Builder::new().name("grainy-thumbnails".into()).spawn(move || thumbnail_loop(&thumbs));
        if let Err(error) = spawned {
            log::error!("cannot start the thumbnail thread: {error}");
        }
        let side = shared.clone();
        let spawned = std::thread::Builder::new().name("grainy-side".into()).spawn(move || side_loop(&side));
        if let Err(error) = spawned {
            log::error!("cannot start the second preview thread: {error}");
        }
        Self { shared, last_preview: None }
    }

    /// How many previews were developed (requests replaced while waiting are not).
    pub fn preview_renders(&self) -> u64 {
        self.shared.renders.load(Ordering::SeqCst)
    }

    /// How many thumbnails were made.
    pub fn thumbnails_made(&self) -> usize {
        self.shared.thumbnails.load(Ordering::SeqCst)
    }

    /// How many neighbour sources were read ahead.
    pub fn prefetched(&self) -> usize {
        self.shared.prefetched.load(Ordering::SeqCst)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        let _queues = lock(&self.shared.queues);
        self.shared.preview_ready.notify_all();
        self.shared.thumbnail_ready.notify_all();
        self.shared.side_ready.notify_all();
    }
}

impl Renderer for Worker {
    fn set_library(&mut self, library: Option<PathBuf>) {
        let next = Self::start(self.shared.gpu.clone(), self.shared.shared_wgpu.clone(), library);
        *lock(&next.shared.monitor) = lock(&self.shared.monitor).clone();
        *lock(&next.shared.hdr_view) = *lock(&self.shared.hdr_view);
        *lock(&next.shared.view) = *lock(&self.shared.view);
        *lock(&next.shared.tile_roi) = *lock(&self.shared.tile_roi);
        *lock(&next.shared.waker) = lock(&self.shared.waker).clone();
        *self = next;
    }

    fn prepares_analysis(&self) -> bool {
        true
    }

    fn take_analysis_seed(&mut self, path: &Path, settings: &Settings) -> Option<AnalysisSeed> {
        let mut slot = lock(&self.shared.analysis_seed);
        let key = LoadSettings::new(path, settings);
        if slot.as_ref().is_some_and(|(p, s, _)| p == path && *s == key) {
            slot.take().map(|(_, _, seed)| seed)
        } else {
            None
        }
    }

    fn preview(&mut self, job: PreviewJob) {
        self.last_preview = Some(job.clone());
        let mut queues = lock(&self.shared.queues);
        if let Some(old) = &queues.preview {
            log::debug!("preview request {} replaced by {}", old.id, job.id);
        }
        queues.preview = Some(job);
        self.shared.preview_ready.notify_one();
    }

    fn prefetch(&mut self, jobs: Vec<PrefetchJob>) {
        let mut queues = lock(&self.shared.queues);
        queues.prefetch = jobs;
        self.shared.preview_ready.notify_one();
    }

    fn thumbnail(&mut self, job: ThumbnailJob) {
        let mut queues = lock(&self.shared.queues);
        match queues.thumbnails.iter_mut().find(|j| j.photo == job.photo) {
            Some(waiting) => *waiting = job,
            None => queues.thumbnails.push(job),
        }
        self.shared.thumbnail_ready.notify_one();
    }

    fn poll(&mut self) -> Vec<Reply> {
        std::mem::take(&mut *lock(&self.shared.replies))
    }

    fn idle(&self) -> bool {
        let queues = lock(&self.shared.queues);
        queues.preview.is_none()
            && queues.prefetch.is_empty()
            && queues.thumbnails.is_empty()
            && queues.side.is_empty()
            && queues.running == 0
            && lock(&self.shared.replies).is_empty()
            && lock(&self.shared.side_replies).is_empty()
    }

    fn set_waker(&mut self, waker: Waker) {
        *lock(&self.shared.waker) = Some(waker);
    }

    fn set_monitor(&mut self, profile: Option<grainy_cms::Profile>) {
        *lock(&self.shared.monitor) = profile;
    }

    fn set_hdr_view(&mut self, view: crate::hdr::View) {
        *lock(&self.shared.hdr_view) = view;
    }

    fn set_view_size(&mut self, long: Option<usize>) {
        *lock(&self.shared.view) = long;
    }

    fn set_tile_roi(&mut self, roi: Option<[f32; 4]>) {
        *lock(&self.shared.tile_roi) = roi;
    }

    fn side(&mut self, job: SideJob) {
        let mut queues = lock(&self.shared.queues);
        match queues.side.iter_mut().find(|j| j.slot == job.slot) {
            Some(waiting) => *waiting = job,
            None => queues.side.push(job),
        }
        self.shared.side_ready.notify_one();
    }

    fn poll_side(&mut self) -> Vec<SideFrame> {
        std::mem::take(&mut *lock(&self.shared.side_replies))
    }

    fn gpu_status(&self) -> Option<crate::gpu_worker::Status> {
        self.shared.gpu.as_ref().map(|control| control.status())
    }

    fn set_gpu_enabled(&mut self, enabled: bool) {
        if let Some(control) = &self.shared.gpu {
            control.set_enabled(enabled);
            if let Some(job) = self.last_preview.clone() {
                self.preview(job);
            }
        }
    }
}

#[cfg(test)]
#[test]
fn side_replies_keep_the_renderer_busy_until_consumed() {
    let mut worker = Worker::new();
    lock(&worker.shared.side_replies).push(SideFrame {
        slot: 1,
        id: 1,
        result: Err("generated failure".into()),
        scale: 1.0,
    });
    assert!(!worker.idle());
    assert_eq!(worker.poll_side().len(), 1);
    assert!(worker.idle());
}

/// A loaded source and what a develop needs to know about it.
struct Source {
    path: PathBuf,
    resolution: Resolution,
    load_settings: LoadSettings,
    photo: Arc<Photo>,
    token: u64,
    /// The file's oriented, uncropped (width, height).
    original: (u32, u32),
    /// The source size requested on load. An offline or reduced-resolution file can supply fewer pixels.
    load_limit: usize,
}

impl Source {
    fn long_side(&self) -> usize {
        let (h, w, _) = self.photo.pixels().dim();
        h.max(w)
    }

    /// Original pixels per source pixel.
    fn scale(&self) -> f64 {
        let original = self.original.0.max(self.original.1) as f64;
        (original / self.long_side().max(1) as f64).max(1.0)
    }

    fn options(&self) -> Options<'static> {
        Options {
            use_crop: Crop::Cropped,
            output: Output::Named("sRGB"),
            original_size: Some((f64::from(self.original.0), f64::from(self.original.1))),
            ..Options::default()
        }
    }

    fn fit_source_long(&self, shown: usize, crop: Crop, settings: &Settings) -> Result<usize, String> {
        let (w, h) = (self.original.0 as usize, self.original.1 as usize);
        let long = w.max(h).max(1);
        let bounds = settings.value("crop");
        if crop != Crop::Cropped || !grainy_core::settings::truthy(bounds) {
            return Ok(shown.clamp(1, long));
        }
        let (w, h) = if settings.i64("rotation").rem_euclid(2) == 0 { (w, h) } else { (h, w) };
        let [l, t, r, b] = grainy_geometry::crop_bounds(bounds, w, h).map_err(|e| e.to_string())?;
        let cropped = (r - l).max(b - t);
        Ok((shown.clamp(1, cropped) as u64 * long as u64).div_ceil(cropped as u64) as usize)
    }
}

fn load(
    path: &Path,
    max_size: Option<usize>,
    settings: &Settings,
    token: u64,
    resolution: Resolution,
    offline: Option<(PathBuf, i64)>,
) -> Result<Source, String> {
    let load_limit = if offline.is_some() { usize::MAX } else { max_size.unwrap_or(usize::MAX) };
    let loaded = match offline {
        Some((directory, id)) => offline_photo(&directory, id, settings)?,
        None if max_size.is_none() => {
            let (photo, metadata) =
                grainy_engine::files::load_full_preview(path, settings).map_err(|e| e.to_string())?;
            return Ok(loaded_source(path, settings, token, resolution, load_limit, photo, metadata));
        }
        None => load_photo(path, max_size, settings).map_err(|e| e.to_string())?,
    };
    Ok(loaded_source(path, settings, token, resolution, load_limit, Arc::new(loaded.photo), loaded.metadata))
}

fn loaded_source(
    path: &Path,
    settings: &Settings,
    token: u64,
    resolution: Resolution,
    load_limit: usize,
    photo: Arc<Photo>,
    metadata: grainy_codecs::Metadata,
) -> Source {
    let (h, w, _) = photo.pixels().dim();
    let size = |key: &str, fallback: usize| {
        metadata.get(key).and_then(serde_json::Value::as_u64).map_or(fallback as u32, |v| v as u32)
    };
    let original = (size("width", w), size("height", h));
    Source {
        path: path.to_path_buf(),
        resolution,
        load_settings: LoadSettings::new(path, settings),
        photo,
        token,
        original,
        load_limit,
    }
}

/// The preview thread's state: the development cache and the loaded sources.
struct PreviewState {
    library: Option<PathBuf>,
    analysis_seed: Option<(PathBuf, LoadSettings, AnalysisSeed)>,
    cache: [DevelopmentCache; 2],
    monitor: crate::monitor_preview::MonitorPreview,
    sources: Vec<Source>,
    /// The fit source reduced to the fitted view's size: (the fit source's token, long side, the reduced source).
    reduced: Option<(u64, usize, Source)>,
    tokens: u64,
    gpu: Option<crate::gpu_worker::PreviewGpu>,
    /// Whole-frame histogram and clipping channels of the open photo, retained across 1:1 viewport tiles.
    last_stats: Option<(u64, crate::histogram::Bins, crate::histogram::Channels)>,
}

/// A fit source is reduced to the view only when that saves at least this share of its long side.
const REDUCE_BELOW: f64 = 0.85;

impl PreviewState {
    fn new(gpu: Option<Arc<crate::gpu_worker::Control>>, shared_wgpu: Option<crate::gpu_worker::SharedWgpu>) -> Self {
        // Grows to a few copies of the source (a full-size frame), at most 1.5 GB.
        Self {
            library: None,
            analysis_seed: None,
            cache: std::array::from_fn(|_| DevelopmentCache::growing(256 << 20, 4.0, Some(1536 << 20))),
            monitor: Default::default(),
            sources: Vec::new(),
            reduced: None,
            tokens: 0,
            gpu: gpu.map(|c| crate::gpu_worker::PreviewGpu::with_shared(c, shared_wgpu)),
            last_stats: None,
        }
    }

    /// The fit source `at` reduced to `long` pixels on its long side with the preview loader's filter, kept
    /// until the source or the size changes. The develop's spatial operations scale with the source
    /// (`original_size`), so the reduced picture is the full one seen at the screen's size.
    fn reduce(&mut self, at: usize, long: usize, settings: &Settings) -> Result<(), String> {
        let parent = &self.sources[at];
        if self.reduced.as_ref().is_some_and(|(token, size, _)| *token == parent.token && *size == long) {
            return Ok(());
        }
        let resize = |image: &grainy_core::Image| grainy_codecs::resize_float(image, long).map_err(|e| e.to_string());
        let photo = match &*parent.photo {
            Photo::Linear(pixels) => Photo::Linear(resize(pixels)?),
            Photo::Camera(camera) => Photo::Camera(Box::new(
                grainy_engine::CameraSource::new(resize(camera.pixels())?, camera.raw_info().clone())
                    .map_err(|e| e.to_string())?,
            )),
        };
        self.tokens += 1;
        let source = Source {
            path: parent.path.clone(),
            resolution: parent.resolution,
            load_settings: LoadSettings::new(&parent.path, settings),
            photo: Arc::new(photo),
            token: self.tokens,
            original: parent.original,
            load_limit: long,
        };
        self.reduced = Some((parent.token, long, source));
        Ok(())
    }

    fn source(&mut self, job: &PreviewJob) -> Result<usize, String> {
        let key = LoadSettings::new(&job.path, &job.settings);
        if let Some(at) = self
            .sources
            .iter()
            .position(|s| s.path == job.path && s.resolution == job.resolution && s.load_settings == key)
        {
            // Most recently used last.
            let source = self.sources.remove(at);
            self.sources.push(source);
            return Ok(self.sources.len() - 1);
        }
        let max = match job.resolution {
            Resolution::Fit => Some(PREVIEW_SIZE),
            Resolution::Full => None,
        };
        self.tokens += 1;
        let offline = offline_for(&job.path, self.library.as_deref(), Some(job.photo as i64));
        let source = if job.resolution == Resolution::Fit && offline.is_none() {
            let (loaded, seed) = grainy_engine::files::load_preview(
                &job.path,
                PREVIEW_SIZE,
                crate::commands::auto_source::SIZE,
                &job.settings,
            )
            .map_err(|e| e.to_string())?;
            self.analysis_seed = seed.map(|seed| (job.path.clone(), key, seed));
            loaded_source(
                &job.path,
                &job.settings,
                self.tokens,
                job.resolution,
                PREVIEW_SIZE,
                Arc::new(loaded.photo),
                loaded.metadata,
            )
        } else {
            load(&job.path, max, &job.settings, self.tokens, job.resolution, offline)?
        };
        if self.sources.len() >= PREVIEW_SOURCES {
            self.sources.remove(0);
        }
        self.sources.push(source);
        Ok(self.sources.len() - 1)
    }

    /// `_prefetch`: loads the fit source of a neighbour into the sources (as the least recently used one, so the
    /// open photo's source stays). Errors are left for the real load to report.
    fn prefetch(&mut self, job: &PrefetchJob) -> bool {
        let key = LoadSettings::new(&job.path, &job.settings);
        if self.sources.iter().any(|s| s.path == job.path && s.resolution == Resolution::Fit && s.load_settings == key)
            || !grainy_library::file_access::present(&job.path)
        {
            return false;
        }
        self.tokens += 1;
        let Ok(source) = load(&job.path, Some(PREVIEW_SIZE), &job.settings, self.tokens, Resolution::Fit, None) else {
            return false;
        };
        if self.sources.len() >= PREVIEW_SOURCES {
            self.sources.remove(0);
        }
        self.sources.insert(0, source);
        true
    }

    fn render(
        &mut self,
        job: &PreviewJob,
        monitor: Option<&grainy_cms::Profile>,
        hdr_view: crate::hdr::View,
        view: Option<usize>,
        tile_roi: Option<[f32; 4]>,
    ) -> Result<Frame, String> {
        let started = Instant::now();
        let zero_copy_gpu = self.gpu.as_ref().is_some_and(|g| g.has_shared_device());
        // A new monitor's LittleCMS bounds depend only on its profile. Build them alongside
        // the cold RAW decode when CPU monitor conversion will be needed.
        let at = if let Some(profile) = monitor.filter(|p| {
            !zero_copy_gpu
                && !self.monitor.is_cached(p)
                && !job.settings.bool("hdr")
                && job.settings.str("working_space") != "ProPhoto"
        }) {
            let mut cache = std::mem::take(&mut self.monitor);
            let (source, bounds) = rayon::join(|| self.source(job), || cache.table(profile).map(|_| ()));
            self.monitor = cache;
            bounds?;
            source?
        } else {
            self.source(job)?
        };
        let effective = grainy_engine::working_settings(&job.settings.0);
        let fit_long = view
            .filter(|_| job.resolution == Resolution::Fit)
            .map(|shown| self.sources[at].fit_source_long(shown, job.crop, &effective))
            .transpose()?;
        if let Some(needed) = fit_long.filter(|&n| n > self.sources[at].load_limit && n > self.sources[at].long_side())
        {
            // A tight crop or a large monitor can outresolve the default fit decode. Keep the larger source
            // available for later edits. The offline packet, when used, is already the best available source.
            let limit = needed.div_ceil(256) * 256;
            self.tokens += 1;
            self.sources[at] = load(
                &job.path,
                Some(limit),
                &job.settings,
                self.tokens,
                Resolution::Fit,
                offline_for(&job.path, self.library.as_deref(), Some(job.photo as i64)),
            )?;
        }
        let reduce = fit_long.filter(|&long| (long as f64) < self.sources[at].long_side() as f64 * REDUCE_BELOW);
        if let Some(long) = reduce {
            self.reduce(at, long, &job.settings)?;
        }
        let source = match (&self.reduced, reduce) {
            (Some((_, _, reduced)), Some(_)) => reduced,
            _ => &self.sources[at],
        };
        let mut options = source.options();
        options.use_crop = job.crop;
        let cache = &mut self.cache[usize::from(job.resolution == Resolution::Full)];
        if crate::hdr::enabled() && job.settings.bool("hdr") {
            // The HDR branch of the oracle's `render` (CPU only, as the oracle's GPU preview skips HDR photos):
            // the limited working pixels, then the display, histogram and clipping of `crate::hdr::present`.
            let working = Options { output: Output::Working, ..options };
            let image = develop_cached(source.photo.source(), &job.settings.0, &working, cache, source.token)
                .map_err(|e| e.to_string())?;
            let shown = crate::hdr::present(&image, &job.settings, hdr_view, monitor)?;
            self.last_stats = Some((job.photo, shown.bins, shown.channels));
            return Ok(Frame {
                id: job.id,
                photo: job.photo,
                resolution: job.resolution,
                preview: Preview { width: shown.width, height: shown.height, rgba: shown.rgba },
                bins: shown.bins,
                channels: shown.channels,
                clipping: shown.mask,
                scale: source.scale(),
                original: source.original,
                millis: started.elapsed().as_secs_f64() * 1000.0,
                hits: cache.hits(),
                misses: cache.misses(),
                hdr: Some(shown.info),
                native_hdr: shown.native_hdr,
                gpu: None,
                tile_uv: None,
                full_size: (shown.width, shown.height),
            });
        }
        options.output = Output::Working;
        if !zero_copy_gpu {
            if let Photo::Linear(pixels) = &*source.photo {
                if let Some(frame) = crate::basic_preview::render(pixels, &effective, monitor, &mut self.monitor)? {
                    let (height, width, _) = pixels.dim();
                    self.last_stats = Some((job.photo, frame.bins, frame.channels));
                    return Ok(Frame {
                        id: job.id,
                        photo: job.photo,
                        resolution: job.resolution,
                        preview: Preview { width, height, rgba: frame.rgba },
                        bins: frame.bins,
                        channels: frame.channels,
                        clipping: frame.mask,
                        scale: source.scale(),
                        original: source.original,
                        millis: started.elapsed().as_secs_f64() * 1000.,
                        hits: cache.hits(),
                        misses: cache.misses(),
                        hdr: None,
                        native_hdr: None,
                        gpu: None,
                        tile_uv: None,
                        full_size: (width, height),
                    });
                }
            }
        }
        let space = effective.str("working_space");
        let tile = tile_roi
            .filter(|_| job.resolution == Resolution::Full)
            .map(|roi| grainy_engine::gpu::TileRequest { roi, halo: 96 });
        let accelerated = self.gpu.as_mut().and_then(|gpu| {
            gpu.render(
                source.photo.source(),
                &job.settings.0,
                &options,
                source.token,
                job.resolution,
                space,
                monitor,
                tile,
            )
        });
        let (working, hits, misses) = match accelerated {
            Some(Ok(crate::gpu_worker::PreviewGpuOutput::ZeroCopy { preview, bins, channels, tile_uv, full_size })) => {
                let bins = *bins;
                let (hits, misses) = self.gpu.as_ref().map_or((0, 0), |gpu| gpu.counters(job.resolution));
                let (bins, channels) = if tile_uv.is_some() {
                    self.last_stats.filter(|(p, _, _)| *p == job.photo).map_or((bins, channels), |(_, b, c)| (b, c))
                } else {
                    self.last_stats = Some((job.photo, bins, channels));
                    (bins, channels)
                };
                let (width, height) = (preview.presented.width, preview.presented.height);
                return Ok(Frame {
                    id: job.id,
                    photo: job.photo,
                    resolution: job.resolution,
                    preview: Preview { width, height, rgba: Vec::new() },
                    bins,
                    channels,
                    clipping: Vec::new(),
                    scale: source.scale(),
                    original: source.original,
                    millis: started.elapsed().as_secs_f64() * 1000.0,
                    hits,
                    misses,
                    hdr: None,
                    native_hdr: None,
                    gpu: Some(preview),
                    tile_uv,
                    full_size,
                });
            }
            Some(Ok(crate::gpu_worker::PreviewGpuOutput::Host(image))) => {
                let (hits, misses) = self.gpu.as_ref().map_or((0, 0), |gpu| gpu.counters(job.resolution));
                (Arc::new(image), hits, misses)
            }
            Some(Err(err)) => return Err(err),
            None => {
                let image = grainy_engine::preview_working(
                    source.photo.source(),
                    &job.settings.0,
                    &options,
                    cache,
                    source.token,
                )
                .map_err(|e| e.to_string())?;
                (image, cache.hits(), cache.misses())
            }
        };
        let srgb = if space == "ProPhoto" {
            Some(
                grainy_cms::output_rgb(&working, space, grainy_cms::Destination::Named("sRGB"))
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let converted = histogram::convert(srgb.as_ref().unwrap_or(&working));
        let bins = histogram::bins(&converted.rgba, converted.width, converted.height);
        let rgba = match monitor {
            Some(profile) => self.monitor.rgba(&working, space, profile)?,
            None => converted.rgba,
        };
        self.last_stats = Some((job.photo, bins, converted.channels));
        let millis = started.elapsed().as_secs_f64() * 1000.0;
        Ok(Frame {
            id: job.id,
            photo: job.photo,
            resolution: job.resolution,
            preview: Preview { width: converted.width, height: converted.height, rgba },
            bins,
            channels: converted.channels,
            clipping: converted.mask,
            scale: source.scale(),
            original: source.original,
            millis,
            hits,
            misses,
            hdr: None,
            native_hdr: None,
            gpu: None,
            tile_uv: None,
            full_size: (converted.width, converted.height),
        })
    }
}

fn caught<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| Err("internal error".to_owned()))
}

#[cfg(test)]
#[test]
fn monitor_and_histogram_share_one_develop_and_zoom_keeps_the_fit_stages() {
    let path = PathBuf::from("generated.png");
    let settings = grainy_core::defaults();
    let mut job =
        PreviewJob { id: 1, photo: 1, path: path.clone(), settings, resolution: Resolution::Fit, crop: Crop::Cropped };
    let mut state = PreviewState::new(None, None);
    for (token, resolution) in [(1, Resolution::Fit), (2, Resolution::Full)] {
        state.sources.push(Source {
            path: path.clone(),
            resolution,
            load_settings: LoadSettings::new(&path, &job.settings),
            photo: Arc::new(Photo::Linear(grainy_testkit::input("garden"))),
            token,
            original: (257, 193),
            load_limit: PREVIEW_SIZE,
        });
    }
    let profile = grainy_cms::profile("Display P3").unwrap();
    let first = state.render(&job, Some(&profile), Default::default(), None, None).unwrap();
    assert_eq!((first.hits, first.misses), (0, 1), "one tone operation in the cold neutral frame");
    job.resolution = Resolution::Full;
    state.render(&job, None, Default::default(), None, None).unwrap();
    job.resolution = Resolution::Fit;
    let plain = state.render(&job, None, Default::default(), None, None).unwrap();
    assert_eq!((plain.hits, plain.misses), (1, 1));
    assert_eq!(plain.bins, first.bins);
    assert_eq!(plain.clipping, first.clipping);
}

fn preview_loop(shared: &Shared) {
    let mut state = PreviewState::new(shared.gpu.clone(), shared.shared_wgpu.clone());
    state.library.clone_from(&shared.library);
    if let Some(waker) = lock(&shared.waker).clone() {
        waker();
    }
    loop {
        let job = {
            let mut queues = lock(&shared.queues);
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = queues.preview.take() {
                    queues.running += 1;
                    break job;
                }
                if !queues.prefetch.is_empty() {
                    // Nothing newer is waiting: read one neighbour ahead, then look again.
                    let ahead = queues.prefetch.remove(0);
                    queues.running += 1;
                    drop(queues);
                    if caught(|| Ok(state.prefetch(&ahead))).unwrap_or(false) {
                        shared.prefetched.fetch_add(1, Ordering::SeqCst);
                    }
                    queues = lock(&shared.queues);
                    queues.running -= 1;
                    if let Some(waker) = lock(&shared.waker).clone() {
                        waker();
                    }
                    continue;
                }
                queues = shared.preview_ready.wait(queues).unwrap_or_else(|e| e.into_inner());
            }
        };
        shared.renders.fetch_add(1, Ordering::SeqCst);
        let monitor = lock(&shared.monitor).clone();
        let hdr_view = *lock(&shared.hdr_view);
        let view = *lock(&shared.view);
        let tile_roi = *lock(&shared.tile_roi);
        {
            let mut seed = lock(&shared.analysis_seed);
            if seed
                .as_ref()
                .is_some_and(|(p, s, _)| *p != job.path || *s != LoadSettings::new(&job.path, &job.settings))
            {
                *seed = None;
            }
        }
        let reply = match caught(|| state.render(&job, monitor.as_ref(), hdr_view, view, tile_roi)) {
            Ok(frame) => {
                log::info!(
                    "render {}x{} ({:?}) in {:.0} ms, cache hits {} misses {}",
                    frame.preview.width,
                    frame.preview.height,
                    frame.resolution,
                    frame.millis,
                    frame.hits,
                    frame.misses
                );
                Reply::Frame(Arc::new(frame))
            }
            Err(message) => {
                log::warn!("render of {} failed: {message}", job.path.display());
                Reply::Failed { id: job.id, photo: job.photo, message }
            }
        };
        if let Some(seed) = state.analysis_seed.take() {
            *lock(&shared.analysis_seed) = Some(seed);
        }
        shared.answer(reply);
    }
}

/// The thumbnail thread's loaded sources, most recently used last.
struct ThumbnailState {
    library: Option<PathBuf>,
    sources: Vec<Source>,
}

impl ThumbnailState {
    fn source(&mut self, path: &Path, settings: &Settings) -> Result<usize, String> {
        let key = LoadSettings::new(path, settings);
        if let Some(at) = self.sources.iter().position(|s| s.path == path && s.load_settings == key) {
            let source = self.sources.remove(at);
            self.sources.push(source);
            return Ok(self.sources.len() - 1);
        }
        let source = load(
            path,
            Some(THUMB_SIZE),
            settings,
            0,
            Resolution::Fit,
            offline_for(path, self.library.as_deref(), None),
        )?;
        if self.sources.len() >= THUMB_SOURCES {
            self.sources.remove(0);
        }
        self.sources.push(source);
        Ok(self.sources.len() - 1)
    }

    /// Develops `path` with `settings` at the thumbnail size.
    fn render(&mut self, path: &Path, settings: &Settings) -> (Result<Preview, String>, Option<(u32, u32)>) {
        let at = match self.source(path, settings) {
            Ok(at) => at,
            Err(message) => return (Err(message), None),
        };
        let source = &self.sources[at];
        let result = develop(source.photo.source(), &settings.0, &source.options()).map_err(|e| e.to_string());
        let preview = result.map(|image| {
            let converted = histogram::convert(&image);
            Preview { width: converted.width, height: converted.height, rgba: converted.rgba }
        });
        (preview, Some(source.original))
    }

    fn make(&mut self, job: &ThumbnailJob) -> (Result<Preview, String>, Option<(u32, u32)>, String) {
        match &job.cache {
            None => {
                let (preview, original) = self.render(&job.path, &job.settings);
                (preview, original, String::new())
            }
            Some(cache) => self.cached(cache),
        }
    }

    /// The library's thumbnail (`PhotoModel.thumbnail` and `preview_store.rebuild`): the stored one when it is
    /// current; otherwise a render with the stored settings, published when the library is writable. The
    /// snapshot is taken before rendering, so a render made stale by a newer edit is not stored.
    fn cached(&mut self, cache: &ThumbCache) -> (Result<Preview, String>, Option<(u32, u32)>, String) {
        let _publication = crate::thumbjpeg::publication();
        let snapshot = match grainy_catalog::preview::snapshot_from(
            cache.snapshot.as_deref().unwrap_or(&cache.directory),
            &cache.directory,
            cache.id,
            true,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => return (Err(error.to_string()), None, "읽기 오류".into()),
        };
        let inspection = match grainy_catalog::preview::inspect_snapshot(&cache.directory, cache.id, snapshot.clone()) {
            Ok(inspection) => inspection,
            Err(error) => return (Err(error.to_string()), None, "읽기 오류".into()),
        };
        let status = inspection.state["status"].as_str().unwrap_or("").to_owned();
        let rebuild = inspection.state["rebuild"].as_bool().unwrap_or(false);
        let stored = || -> Option<Preview> {
            let (width, height, rgba) = crate::thumbjpeg::decode(inspection.image.as_deref()?).ok()?;
            Some(Preview { width, height, rgba })
        };
        if !rebuild {
            return match stored() {
                Some(preview) => (Ok(preview), None, status),
                None => (Err(status.clone()), None, status),
            };
        }
        let Some(snapshot) = snapshot else { return (Err(status.clone()), None, status) };
        let Some(path) = snapshot["path"].as_str().map(PathBuf::from) else {
            return (Err(status.clone()), None, status);
        };
        let settings = match &snapshot["settings"] {
            Value::Object(map) => grainy_core::normalized(map),
            _ => grainy_core::normalized(&Map::new()),
        };
        if snapshot["source"].is_null() && !snapshot["offline"].is_null() {
            register_offline(&path, &cache.directory, cache.id);
        }
        let (mut preview, original) = self.render(&path, &settings);
        let mut stored_copy = None;
        if let (Ok(rendered), true) = (&preview, cache.writable) {
            let preview = rendered;
            let icc = grainy_engine::files::Cms::for_loading()
                .profile_bytes(&grainy_codecs::Profile::Named("sRGB".into()))
                .ok();
            match crate::thumbjpeg::encode_rgba(&preview.rgba, preview.width, preview.height, icc) {
                Ok(jpeg) => {
                    match grainy_catalog::preview::publish(&cache.directory, &snapshot, &jpeg, None, "sRGB") {
                        // What is shown is what is stored, as on the next start: whether a thumbnail was just
                        // rendered or read back must not change its pixels (the JPEG rounds them).
                        Ok(_) => {
                            if let Ok((width, height, rgba)) = crate::thumbjpeg::decode(&jpeg) {
                                stored_copy = Some(Preview { width, height, rgba });
                            }
                        }
                        Err(error) => log::warn!("thumbnail of photo {} not stored: {error}", cache.id),
                    }
                }
                Err(error) => log::warn!("thumbnail of photo {} not encoded: {error}", cache.id),
            }
        }
        if let Some(copy) = stored_copy {
            preview = Ok(copy);
        }
        match preview {
            Ok(preview) => (Ok(preview), original, String::new()),
            // A failed render keeps the stored picture with its status (`원본 없음 · 이전 미리보기`).
            Err(message) => match stored() {
                Some(old) => (Ok(old), None, status),
                None => (Err(message), None, status),
            },
        }
    }
}

fn thumbnail_loop(shared: &Shared) {
    let mut state = ThumbnailState { sources: Vec::new(), library: shared.library.clone() };
    loop {
        let job = {
            let mut queues = lock(&shared.queues);
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if !queues.thumbnails.is_empty() {
                    queues.running += 1;
                    break queues.thumbnails.remove(0);
                }
                queues = shared.thumbnail_ready.wait(queues).unwrap_or_else(|e| e.into_inner());
            }
        };
        let monitor = lock(&shared.monitor).clone();
        let (result, original, status) = catch_unwind(AssertUnwindSafe(|| state.make(&job)))
            .unwrap_or_else(|_| (Err("internal error".into()), None, String::new()));
        // Stored thumbnails are sRGB; the screen shows them through the monitor profile (`PhotoModel.thumbnail`).
        let result = match (result, &monitor) {
            (Ok(preview), Some(profile)) => crate::display::monitor_preview(preview, profile),
            (result, _) => result,
        };
        shared.thumbnails.fetch_add(1, Ordering::SeqCst);
        shared.answer(Reply::Thumbnail(Thumbnail { photo: job.photo, key: job.key, result, original, status }));
    }
}

/// How many sources the second preview lane keeps (the compare view's, a reference window's, a few survey cells).
const SIDE_SOURCES: usize = 6;

/// The second preview lane (`render_photo` in the oracle's worker threads): loads at the asked size and develops
/// on the CPU path, so it never waits for or disturbs the open photo's preview lane.
fn side_loop(shared: &Shared) {
    let mut sources: Vec<(Option<usize>, Source)> = Vec::new();
    loop {
        let job = {
            let mut queues = lock(&shared.queues);
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if !queues.side.is_empty() {
                    queues.running += 1;
                    break queues.side.remove(0);
                }
                queues = shared.side_ready.wait(queues).unwrap_or_else(|e| e.into_inner());
            }
        };
        let monitor = lock(&shared.monitor).clone();
        let result = caught(|| {
            let key = LoadSettings::new(&job.path, &job.settings);
            let at = match sources
                .iter()
                .position(|(size, s)| *size == job.max_size && s.path == job.path && s.load_settings == key)
            {
                Some(at) => at,
                None => {
                    let source = load(
                        &job.path,
                        job.max_size,
                        &job.settings,
                        0,
                        Resolution::Fit,
                        offline_for(&job.path, shared.library.as_deref(), None),
                    )?;
                    if sources.len() >= SIDE_SOURCES {
                        sources.remove(0);
                    }
                    sources.push((job.max_size, source));
                    sources.len() - 1
                }
            };
            let entry = sources.remove(at);
            sources.push(entry);
            let source = &sources[sources.len() - 1].1;
            let options = match &monitor {
                Some(profile) => Options { output: Output::Icc(profile), ..source.options() },
                None => source.options(),
            };
            let image = develop(source.photo.source(), &job.settings.0, &options).map_err(|e| e.to_string())?;
            let converted = histogram::convert(&image);
            Ok((Preview { width: converted.width, height: converted.height, rgba: converted.rgba }, source.scale()))
        });
        let frame = match result {
            Ok((preview, scale)) => SideFrame { slot: job.slot, id: job.id, result: Ok(preview), scale },
            Err(message) => {
                log::warn!("second picture of {} failed: {message}", job.path.display());
                SideFrame { slot: job.slot, id: job.id, result: Err(message), scale: 1.0 }
            }
        };
        lock(&shared.side_replies).push(frame);
        lock(&shared.queues).running -= 1;
        if let Some(waker) = lock(&shared.waker).clone() {
            waker();
        }
    }
}

// ---- offline editing (work package 22) -------------------------------------------------------------------------

/// Originals that are missing but have an offline packet (`previews/<id>.npz`): (original, library, id).
static OFFLINE: Mutex<Vec<(PathBuf, PathBuf, i64)>> = Mutex::new(Vec::new());

/// Notes that photo `id` of the library in `directory` has an offline packet for `original`
/// (`activate`'s `offline=` and `preview_store.rebuild`'s `state['offline']`): while `original` is missing, the
/// workers develop the packet instead (`preview_store.load_offline`).
pub fn register_offline(original: &Path, directory: &Path, id: i64) {
    let mut list = lock(&OFFLINE);
    list.retain(|(path, lib, _)| path != original || lib != directory);
    list.push((original.to_path_buf(), directory.to_path_buf(), id));
    if list.len() > 256 {
        list.remove(0);
    }
}

/// The packet to load for `path`: when the original is not there and a packet was noted for it.
fn offline_for(path: &Path, library: Option<&Path>, id: Option<i64>) -> Option<(PathBuf, i64)> {
    let directory = library?;
    let id =
        id.or_else(|| lock(&OFFLINE).iter().rev().find(|(p, lib, _)| p == path && lib == directory).map(|v| v.2))?;
    if grainy_library::file_access::present(path) {
        return None;
    }
    directory.join("previews").join(format!("{id}.npz")).is_file().then(|| (directory.to_owned(), id))
}

/// Port of `preview_store.load_offline`: the packet's pixels as a develop source (`rawcolor.CameraSource` for a
/// native RAW packet), converted to the settings' working space, with its info (`offline_preview`,
/// `offline_fingerprint`).
pub fn offline_photo(directory: &Path, id: i64, settings: &Settings) -> Result<grainy_engine::files::Loaded, String> {
    use grainy_catalog::preview::{load_offline, Pixels};
    fn image(p: Pixels) -> Result<grainy_core::Image, String> {
        ndarray::Array3::from_shape_vec((p.shape[0], p.shape[1], p.shape[2]), p.values).map_err(|e| e.to_string())
    }
    let mut failure = None;
    let convert = |p: Pixels, from: &str, to: &str| -> grainy_catalog::Result<Pixels> {
        let converted = image(p).and_then(|source| {
            let profile = |name: &str| grainy_cms::profile(name).map_err(|e| e.to_string());
            grainy_cms::convert(&source, &profile(from)?, &profile(to)?).map_err(|e| e.to_string())
        });
        match converted {
            Ok(out) => {
                let (h, w, c) = out.dim();
                Ok(Pixels { shape: [h, w, c], values: out.into_raw_vec_and_offset().0, half: false })
            }
            Err(message) => {
                failure = Some(message);
                Ok(Pixels { shape: [0, 0, 3], values: Vec::new(), half: false })
            }
        }
    };
    let (pixels, info) = load_offline(directory, id, settings, convert).map_err(|e| e.to_string())?;
    if let Some(message) = failure {
        return Err(message);
    }
    let pixels = image(pixels)?;
    let photo = if grainy_core::settings::truthy(&info["raw_native"]) {
        let raw: grainy_raw::rawcolor::CameraInfo =
            serde_json::from_value(info["raw_info"].clone()).map_err(|e| e.to_string())?;
        Photo::Camera(Box::new(grainy_engine::CameraSource::new(pixels, raw).map_err(|e| e.to_string())?))
    } else {
        Photo::Linear(pixels)
    };
    let metadata = match info {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    Ok(grainy_engine::files::Loaded { photo, metadata })
}

/// Port of `preview_store.rebuild(directory, ident, cancel, force)` (선택 사진 미리보기 다시 만들기): the stored
/// thumbnail of one photo made again from its original (or its offline packet), and published. Returns the oracle's
/// status (`갱신 완료`, `최신`, `원본 없음`, `중단`…).
pub fn rebuild_preview(cache: &ThumbCache, force: bool, cancel: &AtomicBool) -> String {
    let _publication = crate::thumbjpeg::publication();
    if cancel.load(Ordering::SeqCst) {
        return "중단".into();
    }
    let inspection = match grainy_catalog::preview::inspect(&cache.directory, cache.id) {
        Ok(inspection) => inspection,
        Err(error) => return format!("오류: {error}"),
    };
    let status = inspection.state["status"].as_str().unwrap_or("").to_owned();
    if !force && inspection.state["rebuild"] != true {
        return if status.is_empty() { "최신".into() } else { status };
    }
    let snapshot = match grainy_catalog::preview::snapshot(&cache.directory, cache.id, true) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return "제거됨".into(),
        Err(error) => return format!("오류: {error}"),
    };
    let path = PathBuf::from(snapshot["path"].as_str().unwrap_or(""));
    if snapshot["source"].is_null() {
        if snapshot["offline"].is_null() {
            return if snapshot["denied"] == true { "접근 권한 필요" } else { "원본 없음" }.into();
        }
        register_offline(&path, &cache.directory, cache.id);
    }
    let settings = match &snapshot["settings"] {
        Value::Object(map) => grainy_core::normalized(map),
        _ => grainy_core::normalized(&Map::new()),
    };
    let mut state = ThumbnailState { sources: Vec::new(), library: Some(cache.directory.clone()) };
    let (preview, _) = state.render(&path, &settings);
    let preview = match preview {
        Ok(preview) => preview,
        Err(message) => return format!("오류: {message}"),
    };
    if cancel.load(Ordering::SeqCst) {
        return "중단".into();
    }
    let icc =
        grainy_engine::files::Cms::for_loading().profile_bytes(&grainy_codecs::Profile::Named("sRGB".into())).ok();
    let jpeg = match crate::thumbjpeg::encode_rgba(&preview.rgba, preview.width, preview.height, icc) {
        Ok(jpeg) => jpeg,
        Err(error) => return format!("오류: {error}"),
    };
    match grainy_catalog::preview::publish(&cache.directory, &snapshot, &jpeg, None, "sRGB") {
        Ok(true) => "갱신 완료".into(),
        Ok(false) => "변경됨 · 다시 확인".into(),
        Err(error) => format!("오류: {error}"),
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_worker_waits_for_the_import_thumbnail_before_reading_the_new_row() {
        let root = std::env::temp_dir().join(format!("grainy-review-thumbnail-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("generated.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([50, 100, 150])).save(&source).unwrap();
        let catalog = grainy_catalog::Catalog::new(root.join("library")).unwrap();
        let publication = crate::thumbjpeg::publication();
        let id = catalog.add(source.to_str().unwrap(), None, None, true).unwrap();
        let cache = ThumbCache { snapshot: None, directory: catalog.directory().to_path_buf(), id, writable: true };
        let (begun, start) = mpsc::channel();
        let (send, answer) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            begun.send(()).unwrap();
            let mut state = ThumbnailState { sources: Vec::new(), library: None };
            send.send(state.cached(&cache)).unwrap();
        });
        start.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(answer.recv_timeout(Duration::from_millis(250)), Err(mpsc::RecvTimeoutError::Timeout)));
        // Distinct dimensions make an early worker rebuild distinguishable from the import's packet.
        let jpeg = crate::thumbjpeg::encode(&ndarray::Array3::from_elem((5, 7, 3), 80), None).unwrap();
        let snapshot = grainy_catalog::preview::snapshot(catalog.directory(), id, true).unwrap().unwrap();
        assert!(grainy_catalog::preview::publish(catalog.directory(), &snapshot, &jpeg, None, "sRGB").unwrap());
        drop(publication);
        let (preview, _, _) = answer.recv_timeout(Duration::from_secs(10)).unwrap();
        let preview = preview.unwrap();
        assert_eq!((preview.width, preview.height, preview.rgba), crate::thumbjpeg::decode(&jpeg).unwrap());
        worker.join().unwrap();
        drop(catalog);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[test]
fn offline_sources_are_scoped_to_the_requesting_library() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("missing.png");
    let a = root.path().join("a");
    let b = root.path().join("b");
    for (dir, id) in [(&a, 7), (&b, 9)] {
        std::fs::create_dir_all(dir.join("previews")).unwrap();
        std::fs::write(dir.join("previews").join(format!("{id}.npz")), b"marker").unwrap();
        register_offline(&source, dir, id);
    }
    assert_eq!(offline_for(&source, Some(&a), None), Some((a.clone(), 7)));
    assert_eq!(offline_for(&source, Some(&b), Some(9)), Some((b, 9)));
    assert_eq!(offline_for(&source, None, None), None);
}
