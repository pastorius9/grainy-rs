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
use grainy_engine::files::{load_photo, Photo};
use grainy_engine::{develop, develop_cached, Crop, DevelopmentCache, Options, Output};
use serde_json::{Map, Value};

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
    /// The monitor's profile (`Renderer::set_monitor`), read when a job starts.
    monitor: Mutex<Option<grainy_cms::Profile>>,
    /// The HDR viewing choices (`Renderer::set_hdr_view`), read when a job starts.
    hdr_view: Mutex<crate::hdr::View>,
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
        Self::start(None)
    }

    /// Port of the oracle's preview worker with GPU warm-up and CPU retry on device failure.
    /// The ordinary constructor remains the CPU reference for tests and deterministic clients.
    pub fn with_gpu(enabled: bool) -> Self {
        Self::start(Some(crate::gpu_worker::Control::new(enabled)))
    }

    fn start(gpu: Option<Arc<crate::gpu_worker::Control>>) -> Self {
        let shared = Arc::new(Shared {
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
            monitor: Mutex::new(None),
            hdr_view: Mutex::new(crate::hdr::View::default()),
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
    working_space: String,
    photo: Arc<Photo>,
    token: u64,
    /// The file's oriented, uncropped (width, height).
    original: (u32, u32),
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
}

fn load(
    path: &Path,
    max_size: Option<usize>,
    settings: &Settings,
    token: u64,
    resolution: Resolution,
) -> Result<Source, String> {
    let loaded = match offline_for(path) {
        Some((directory, id)) => offline_photo(&directory, id, settings)?,
        None => load_photo(path, max_size, settings).map_err(|e| e.to_string())?,
    };
    let (h, w, _) = loaded.photo.pixels().dim();
    let size = |key: &str, fallback: usize| {
        loaded.metadata.get(key).and_then(serde_json::Value::as_u64).map_or(fallback as u32, |v| v as u32)
    };
    let original = (size("width", w), size("height", h));
    Ok(Source {
        path: path.to_path_buf(),
        resolution,
        working_space: settings.str("working_space").to_owned(),
        photo: Arc::new(loaded.photo),
        token,
        original,
    })
}

/// The preview thread's state: the development cache and the loaded sources.
struct PreviewState {
    cache: DevelopmentCache,
    sources: Vec<Source>,
    tokens: u64,
    gpu: Option<crate::gpu_worker::PreviewGpu>,
}

impl PreviewState {
    fn new(gpu: Option<Arc<crate::gpu_worker::Control>>) -> Self {
        // Grows to a few copies of the source (a full-size frame), at most 1.5 GB.
        Self {
            cache: DevelopmentCache::growing(256 << 20, 4.0, Some(1536 << 20)),
            sources: Vec::new(),
            tokens: 0,
            gpu: gpu.map(crate::gpu_worker::PreviewGpu::new),
        }
    }

    fn source(&mut self, job: &PreviewJob) -> Result<usize, String> {
        let space = job.settings.str("working_space");
        if let Some(at) = self
            .sources
            .iter()
            .position(|s| s.path == job.path && s.resolution == job.resolution && s.working_space == space)
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
        let source = load(&job.path, max, &job.settings, self.tokens, job.resolution)?;
        if self.sources.len() >= PREVIEW_SOURCES {
            self.sources.remove(0);
        }
        self.sources.push(source);
        Ok(self.sources.len() - 1)
    }

    /// `_prefetch`: loads the fit source of a neighbour into the sources (as the least recently used one, so the
    /// open photo's source stays). Errors are left for the real load to report.
    fn prefetch(&mut self, job: &PrefetchJob) -> bool {
        let space = job.settings.str("working_space");
        if self
            .sources
            .iter()
            .any(|s| s.path == job.path && s.resolution == Resolution::Fit && s.working_space == space)
            || !grainy_library::file_access::present(&job.path)
        {
            return false;
        }
        self.tokens += 1;
        let Ok(source) = load(&job.path, Some(PREVIEW_SIZE), &job.settings, self.tokens, Resolution::Fit) else {
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
    ) -> Result<Frame, String> {
        let started = Instant::now();
        let at = self.source(job)?;
        let source = &self.sources[at];
        let mut options = source.options();
        options.use_crop = job.crop;
        if crate::hdr::enabled() && job.settings.bool("hdr") {
            // The HDR branch of the oracle's `render` (CPU only, as the oracle's GPU preview skips HDR photos):
            // the limited working pixels, then the display, histogram and clipping of `crate::hdr::present`.
            let working = Options { output: Output::Working, ..options };
            let image = develop_cached(source.photo.source(), &job.settings.0, &working, &mut self.cache, source.token)
                .map_err(|e| e.to_string())?;
            let shown = crate::hdr::present(&image, &job.settings, hdr_view, monitor)?;
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
                hits: self.cache.hits(),
                misses: self.cache.misses(),
                hdr: Some(shown.info),
            });
        }
        let mut develop_with = |options: &Options<'_>| {
            let accelerated = self
                .gpu
                .as_mut()
                .and_then(|gpu| gpu.render(source.photo.source(), &job.settings.0, options, source.token));
            match accelerated {
                Some(result) => {
                    let (hits, misses) = self.gpu.as_ref().map_or((0, 0), crate::gpu_worker::PreviewGpu::counters);
                    result.map(|image| (image, hits, misses))
                }
                None => develop_cached(source.photo.source(), &job.settings.0, options, &mut self.cache, source.token)
                    .map(|image| (image, self.cache.hits(), self.cache.misses()))
                    .map_err(|e| e.to_string()),
            }
        };
        let (image, hits, misses) = develop_with(&options)?;
        let converted = histogram::convert(&image);
        let bins = histogram::bins(&converted.rgba, converted.width, converted.height);
        // With a monitor profile the picture is the working pixels through it (`output_rgb(working, space,
        // monitor_profile)`); the histogram and the clipping are those of the sRGB frame, as in the oracle.
        let rgba = match monitor {
            Some(profile) => {
                let shown = Options { output: Output::Icc(profile), ..options };
                histogram::convert(&develop_with(&shown)?.0).rgba
            }
            None => converted.rgba,
        };
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
        })
    }
}

fn caught<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| Err("internal error".to_owned()))
}

fn preview_loop(shared: &Shared) {
    let mut state = PreviewState::new(shared.gpu.clone());
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
        let reply = match caught(|| state.render(&job, monitor.as_ref(), hdr_view)) {
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
        shared.answer(reply);
    }
}

/// The thumbnail thread's loaded sources, most recently used last.
struct ThumbnailState {
    sources: Vec<Source>,
}

impl ThumbnailState {
    fn source(&mut self, path: &Path, settings: &Settings) -> Result<usize, String> {
        let space = settings.str("working_space");
        if let Some(at) = self.sources.iter().position(|s| s.path == path && s.working_space == space) {
            let source = self.sources.remove(at);
            self.sources.push(source);
            return Ok(self.sources.len() - 1);
        }
        let source = load(path, Some(THUMB_SIZE), settings, 0, Resolution::Fit)?;
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
        let snapshot = match grainy_catalog::preview::snapshot(&cache.directory, cache.id, true) {
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
    let mut state = ThumbnailState { sources: Vec::new() };
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
            let space = job.settings.str("working_space");
            let at = match sources
                .iter()
                .position(|(size, s)| *size == job.max_size && s.path == job.path && s.working_space == space)
            {
                Some(at) => at,
                None => {
                    let source = load(&job.path, job.max_size, &job.settings, 0, Resolution::Fit)?;
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
    list.retain(|(path, _, _)| path != original);
    list.push((original.to_path_buf(), directory.to_path_buf(), id));
    if list.len() > 256 {
        list.remove(0);
    }
}

/// The packet to load for `path`: when the original is not there and a packet was noted for it.
fn offline_for(path: &Path) -> Option<(PathBuf, i64)> {
    if grainy_library::file_access::present(path) {
        return None;
    }
    let list = lock(&OFFLINE);
    let (_, directory, id) = list.iter().find(|(p, _, _)| p == path)?;
    directory.join("previews").join(format!("{id}.npz")).is_file().then(|| (directory.clone(), *id))
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
    let mut state = ThumbnailState { sources: Vec::new() };
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
        let cache = ThumbCache { directory: catalog.directory().to_path_buf(), id, writable: true };
        let (begun, start) = mpsc::channel();
        let (send, answer) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            begun.send(()).unwrap();
            let mut state = ThumbnailState { sources: Vec::new() };
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
