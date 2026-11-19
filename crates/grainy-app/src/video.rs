//! Video files (APP_PLAN package 32): what the application does with the video rows an older catalogue may hold.
//! Port of `engine.VIDEO_EXTENSIONS` and the places of luma/app.py that skip videos (`prefetch_neighbors`).
//!
//! **Video stays off, deliberately.** Grainy 0.5.74 removed video editing at the owner's request, together with
//! every FFmpeg component (PyAV, imageio-ffmpeg, Qt Multimedia; the build fails if one remains). The oracle 0.6.8
//! has no video feature left to port: it neither imports nor opens videos (`engine.load_image` refuses them with
//! '동영상 파일은 지원하지 않습니다.', which `grainy_engine::files` already ports), and every batch tool skips the
//! rows older catalogues still hold (ported in grainy-service, grainy-mcp, grainy-adobe and the dust commands). The
//! removed implementation (0.5.73: decoding with tagged colour and orientation, playback, poster frame, HDR video
//! to SDR, MP4 export with audio) needed FFmpeg; its dependencies are not in the oracle's environment any more, so
//! no golden can be made for it, and an operating-system decoder alone would give a frame grabber without the
//! encoder, the colour handling and the export the feature consisted of. Bringing video back is a new feature, not a
//! port: it needs its own decision on scope (which formats, export or not, which decoder and encoder), so nothing
//! here pretends to open a video.

use std::path::Path;

/// Port of `engine.VIDEO_EXTENSIONS` (lower case, without the dot).
pub const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "mov", "avi", "mkv", "m4v"];

/// Whether `path` is a video file by its extension (`Path(path).suffix.lower() in VIDEO_EXTENSIONS`).
pub fn is_video(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}
