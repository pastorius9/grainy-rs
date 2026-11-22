//! The interface between the window and whatever draws pictures: the window submits jobs (the open photo's
//! preview, thumbnails) and collects replies on later frames, never waiting. [`crate::worker::Worker`] develops
//! real photos on background threads; [`TestCard`] answers at once with a fixed picture (the snapshot tests of
//! the shell without photos).

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use grainy_core::Settings;

use crate::histogram::{self, Bins, Channels};

/// A GPU-resident RGBA8 preview texture (already monitor-ICC corrected on the GPU) plus its
/// packed 2-bit clipping mask and lazy CPU RGBA8 cache for interactive pickers.
pub struct GpuPreview {
    pub gpu: Arc<grainy_gpu::Gpu>,
    pub presented: grainy_gpu::Presented,
    rgba_cache: OnceLock<Vec<u8>>,
}

impl std::fmt::Debug for GpuPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuPreview")
            .field("width", &self.presented.width)
            .field("height", &self.presented.height)
            .finish()
    }
}

impl GpuPreview {
    pub fn new(gpu: Arc<grainy_gpu::Gpu>, presented: grainy_gpu::Presented) -> Self {
        Self { gpu, presented, rgba_cache: OnceLock::new() }
    }

    /// Lazily reads back the 8-bit RGBA texture to CPU memory (only when an interactive picker
    /// such as the eyedropper or targeted curve tool samples pixels).
    pub fn rgba_bytes(&self) -> &[u8] {
        self.rgba_cache.get_or_init(|| {
            self.gpu
                .readback_texture_rgba(&self.presented.texture, self.presented.width, self.presented.height)
                .unwrap_or_default()
        })
    }
}

/// A rendered picture for the screen: 8-bit sRGB, RGBA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub width: usize,
    pub height: usize,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Which source a preview is developed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// The file reduced to [`crate::worker::PREVIEW_SIZE`] on its long side (fit view, slider drags).
    Fit,
    /// The file at full size (1:1 and closer).
    Full,
}

/// A request for the open photo's picture. A newer request replaces an older one that has not started.
#[derive(Debug, Clone)]
pub struct PreviewJob {
    /// Increasing request number.
    pub id: u64,
    /// The session's photo id.
    pub photo: u64,
    pub path: PathBuf,
    pub settings: Settings,
    pub resolution: Resolution,
    /// `use_crop`: cropped, or uncropped (on the enlarged canvas of a straightened photo) while the crop tool is open.
    pub crop: grainy_engine::Crop,
}

/// A request for a photo's thumbnail with its own settings.
#[derive(Debug, Clone)]
pub struct ThumbnailJob {
    pub photo: u64,
    pub path: PathBuf,
    pub settings: Settings,
    /// The window's key for this request (a newer request for the same photo has another key).
    pub key: u64,
    /// The library's thumbnail cache of the photo (`preview_store`): the stored thumbnail is shown when it is
    /// current, otherwise the photo is rendered with its *stored* settings (`settings` is then not used) and,
    /// in a writable library, the result is published to the cache. `None`: render `settings`.
    pub cache: Option<crate::document::ThumbCache>,
}

/// A developed preview with what the window shows about it.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The request it answers.
    pub id: u64,
    pub photo: u64,
    pub resolution: Resolution,
    pub preview: Preview,
    pub bins: Bins,
    pub channels: Channels,
    /// One byte per pixel: `histogram::HIGHLIGHT | histogram::SHADOW`.
    pub clipping: Vec<u8>,
    /// Original pixels per frame pixel (1 for a full-size source).
    pub scale: f64,
    /// The file's oriented, uncropped size.
    pub original: (u32, u32),
    /// Milliseconds the develop and the conversion took.
    pub millis: f64,
    /// The development cache's hits and misses after this render.
    pub hits: u64,
    pub misses: u64,
    /// The HDR histogram and indicators of an HDR photo's frame (`None`: an ordinary frame).
    pub hdr: Option<crate::hdr::FrameInfo>,
    /// Extended linear sRGB RGBA for the native HDR output, allocated only while its window is open.
    pub native_hdr: Option<Arc<[f32]>>,
    /// Zero-copy GPU-resident preview texture + packed clip mask when developed on the shared `wgpu::Device`.
    pub gpu: Option<Arc<GpuPreview>>,
    /// Normalized `[u0, v0, u1, v1]` in `[0, 1]` cropped image coordinates when this frame is a 1:1 viewport tile.
    pub tile_uv: Option<[f32; 4]>,
    /// Full cropped frame dimensions `(width, height)` in pixels (matches `preview` except on 1:1 tiles).
    pub full_size: (usize, usize),
}

impl Frame {
    /// The frame's size in original pixels (what 1:1 shows).
    pub fn logical_size(&self) -> (f64, f64) {
        let (w, h) = if self.full_size.0 > 0 && self.full_size.1 > 0 {
            self.full_size
        } else {
            (self.preview.width, self.preview.height)
        };
        (w as f64 * self.scale, h as f64 * self.scale)
    }

    /// CPU RGBA8 pixels of the frame, lazily read back from GPU only if this is a zero-copy GPU frame.
    pub fn rgba(&self) -> Cow<'_, [u8]> {
        if !self.preview.rgba.is_empty() {
            Cow::Borrowed(&self.preview.rgba)
        } else if let Some(gpu) = &self.gpu {
            Cow::Borrowed(gpu.rgba_bytes())
        } else {
            Cow::Borrowed(&[])
        }
    }
}

/// A photo whose preview source is read ahead of time (`prefetch_neighbors`: the photos next to the open one).
#[derive(Debug, Clone)]
pub struct PrefetchJob {
    pub path: PathBuf,
    pub settings: Settings,
}

/// A request of the second preview lane: a picture beside the open photo (the compare view's second picture, the
/// survey's cells, the reference windows; luma/reference_view.py `render_photo`). Each `slot` keeps only its newest
/// request; the open photo's preview lane is not affected.
#[derive(Debug, Clone)]
pub struct SideJob {
    /// Who asks (one picture on screen).
    pub slot: u64,
    /// Increasing request number.
    pub id: u64,
    pub path: PathBuf,
    pub settings: Settings,
    /// Long side of the source (`render_photo`'s `size`); `None` for the full-size file.
    pub max_size: Option<usize>,
}

/// The answer of the second preview lane.
#[derive(Debug, Clone)]
pub struct SideFrame {
    pub slot: u64,
    /// The request it answers.
    pub id: u64,
    pub result: Result<Preview, String>,
    /// Original pixels per picture pixel.
    pub scale: f64,
}

/// A thumbnail.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub photo: u64,
    pub key: u64,
    pub result: Result<Preview, String>,
    pub original: Option<(u32, u32)>,
    /// The cache's status for the badge (`preview_store.inspect`: 오프라인, 원본 없음…), Korean source text.
    pub status: String,
}

/// What comes back from a renderer.
#[derive(Debug, Clone)]
pub enum Reply {
    Frame(Arc<Frame>),
    /// The develop failed: the window keeps the last frame and shows the message.
    Failed {
        id: u64,
        photo: u64,
        message: String,
    },
    Thumbnail(Thumbnail),
}

/// Called from a renderer's threads when a reply is ready (the window repaints).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Turns jobs into pictures without blocking the caller.
/// Soft-proofing view state (simulated output profile + optional out-of-gamut overlay + BPC).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SoftProof {
    pub enabled: bool,
    pub target: String,
    pub gamut_warning: bool,
    pub bpc: bool,
}

impl Default for SoftProof {
    fn default() -> Self {
        Self { enabled: false, target: "sRGB".to_owned(), gamut_warning: false, bpc: true }
    }
}

/// The monitor's ICC profile (`Renderer::set_monitor`), read when a job starts.
pub trait Renderer {
    /// Start a new document with fresh source caches and its own offline packet directory.
    fn set_library(&mut self, _folder: Option<PathBuf>) {}

    /// Whether an accepted frame came from a file renderer and can trigger idle analysis I/O.
    fn prepares_analysis(&self) -> bool {
        false
    }

    /// Take a compatible pre-resize RAW from the visible load for the automatic buttons.
    fn take_analysis_seed(
        &mut self,
        _path: &std::path::Path,
        _settings: &Settings,
    ) -> Option<grainy_engine::files::AnalysisSeed> {
        None
    }
    /// Asks for the open photo's picture; replaces a waiting request.
    fn preview(&mut self, job: PreviewJob);
    /// Asks for a thumbnail; replaces a waiting request for the same photo.
    fn thumbnail(&mut self, job: ThumbnailJob);
    /// Reads the preview sources of these photos while nothing else is asked for; replaces the jobs still waiting
    /// (the person already moved on). A renderer that keeps no sources ignores it.
    fn prefetch(&mut self, _jobs: Vec<PrefetchJob>) {}
    /// The replies that are ready.
    fn poll(&mut self) -> Vec<Reply>;
    /// Whether nothing is waiting or running.
    fn idle(&self) -> bool;
    /// Sets the function called when a reply is ready.
    fn set_waker(&mut self, _waker: Waker) {}
    /// Port of `native_gpu.status`; CPU reference renderers have no acceleration control.
    fn gpu_status(&self) -> Option<crate::gpu_worker::Status> {
        None
    }
    /// Port of `native_gpu.set_enabled`; rerenders the current preview without changing photo settings.
    fn set_gpu_enabled(&mut self, _enabled: bool) {}
    /// Sets the monitor's ICC profile (`display_color.main.profile`, `None` for sRGB): the preview, the thumbnails and
    /// the second lane's pictures are shown through it (`output_rgb(working, space, profile)`; the histogram and the
    /// clipping stay those of the sRGB frame). The window asks for the pictures again; a renderer without colour
    /// management ignores it.
    fn set_monitor(&mut self, _profile: Option<grainy_cms::Profile>) {}
    /// Soft-proofing configuration (simulated output profile + gamut warning); ignored by simple renderers.
    fn set_soft_proof(&mut self, _proof: SoftProof) {}
    /// The viewing choices for HDR photos (`crate::hdr::View`: SDR preview, HDR range view); a renderer without the
    /// HDR preview path ignores it.
    fn set_hdr_view(&mut self, _view: crate::hdr::View) {}
    /// The long side in device pixels the fitted photo occupies on screen (`None`: not fitted). A fit preview is
    /// developed at that size instead of the whole preview source, as Lightroom does; a renderer may ignore it.
    fn set_view_size(&mut self, _long: Option<usize>) {}
    /// Normalized `[u0, v0, u1, v1]` viewport region for true 1:1 tile rendering when zoomed in (`None`: full frame).
    fn set_tile_roi(&mut self, _roi: Option<[f32; 4]>) {}
    /// Asks the second preview lane for a picture; replaces a waiting request of the same slot.
    fn side(&mut self, _job: SideJob) {}
    /// The second lane's answers that are ready.
    fn poll_side(&mut self) -> Vec<SideFrame> {
        Vec::new()
    }
}

/// A fixed test card (colour ramps, a grey ramp, colour patches) shown as photo 0: the shell without photos and
/// its deterministic snapshots. It does not process anything.
#[derive(Debug, Default)]
pub struct TestCard {
    replies: Vec<Reply>,
    shown: bool,
    side: Vec<SideFrame>,
}

impl TestCard {
    /// The card's size.
    pub const SIZE: (usize, usize) = (1500, 1000);

    /// The test card's pixels.
    pub fn picture() -> Preview {
        let (width, height) = Self::SIZE;
        let mut rgba = vec![0u8; width * height * 4];
        let patches: [[u8; 3]; 6] =
            [[176, 92, 72], [208, 168, 96], [96, 140, 104], [80, 120, 168], [136, 104, 160], [200, 200, 196]];
        for y in 0..height {
            let v = y as f32 / (height - 1) as f32;
            for x in 0..width {
                let u = x as f32 / (width - 1) as f32;
                let rgb: [f32; 3] = if v < 0.62 {
                    // A soft "sky to ground" field with a hue sweep.
                    let sky = [0.55 + 0.25 * u, 0.62 + 0.1 * (1.0 - u), 0.78 - 0.2 * u];
                    let ground = [0.32 + 0.1 * u, 0.36, 0.26 + 0.08 * (1.0 - u)];
                    let t = (v / 0.62).powf(1.6);
                    [0, 1, 2].map(|c| sky[c] * (1.0 - t) + ground[c] * t)
                } else if v < 0.72 {
                    // Grey ramp, 0 to 1.
                    [u, u, u]
                } else {
                    // Six colour patches.
                    let index = ((u * 6.0) as usize).min(5);
                    let inside = (u * 6.0).fract() > 0.06 && (u * 6.0).fract() < 0.94 && v > 0.75 && v < 0.97;
                    if inside {
                        patches[index].map(|c| f32::from(c) / 255.0)
                    } else {
                        [0.12, 0.12, 0.13]
                    }
                };
                let at = (y * width + x) * 4;
                for c in 0..3 {
                    rgba[at + c] = (rgb[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
                rgba[at + 3] = 255;
            }
        }
        Preview { width, height, rgba }
    }

    fn frame(id: u64, photo: u64) -> Frame {
        let preview = Self::picture();
        let bins = histogram::bins(&preview.rgba, preview.width, preview.height);
        let (w, h) = Self::SIZE;
        Frame {
            id,
            photo,
            resolution: Resolution::Fit,
            clipping: vec![0; w * h],
            preview,
            bins,
            channels: ([false; 3], [false; 3]),
            scale: 1.0,
            original: (w as u32, h as u32),
            millis: 0.0,
            hits: 0,
            misses: 0,
            hdr: None,
            native_hdr: None,
            gpu: None,
            tile_uv: None,
            full_size: (w, h),
        }
    }
}

impl Renderer for TestCard {
    fn preview(&mut self, job: PreviewJob) {
        self.replies.push(Reply::Frame(Arc::new(Self::frame(job.id, job.photo))));
    }

    fn thumbnail(&mut self, job: ThumbnailJob) {
        let card = Self::picture();
        // Every fifth pixel: a 300×200 thumbnail.
        let (w, h) = (card.width / 5, card.height / 5);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let at = (y * 5 * card.width + x * 5) * 4;
                rgba.extend_from_slice(&card.rgba[at..at + 4]);
            }
        }
        let preview = Preview { width: w, height: h, rgba };
        self.replies.push(Reply::Thumbnail(Thumbnail {
            photo: job.photo,
            key: job.key,
            result: Ok(preview),
            original: Some((card.width as u32, card.height as u32)),
            status: String::new(),
        }));
    }

    fn poll(&mut self) -> Vec<Reply> {
        if !self.shown {
            // Shown once at start as "photo 0" (no photo open), as the first run's shell did.
            self.shown = true;
            self.replies.insert(0, Reply::Frame(Arc::new(Self::frame(0, 0))));
        }
        std::mem::take(&mut self.replies)
    }

    fn idle(&self) -> bool {
        self.replies.is_empty()
    }

    fn side(&mut self, job: SideJob) {
        self.side.push(SideFrame { slot: job.slot, id: job.id, result: Ok(Self::picture()), scale: 1.0 });
    }

    fn poll_side(&mut self) -> Vec<SideFrame> {
        std::mem::take(&mut self.side)
    }
}
