//! Register exposures in scene-linear light and stitch overlapping frames without changing the originals.
//! Coordinates are oriented source pixels. The panorama model is a planar translation (horizontal or vertical).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use grainy_codecs::{ExportFormat, ExportOptions};
use grainy_core::{Image, Plane};
use rayon::prelude::*;

use crate::files::{load_photo, Cms, Photo};

/// Maximum memory retained for already-decoded source images between alignment and blending.
const CACHE_BYTES: usize = 1536 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Hdr,
    Panorama,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Offset {
    x: i64,
    y: i64,
}

#[derive(Clone, Copy)]
struct Frame {
    offset: Offset,
    exposure: f32,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Photo merge cancelled.".into())
    } else {
        Ok(())
    }
}

fn load_linear(path: &Path) -> Result<Image, String> {
    let mut settings = grainy_core::defaults();
    settings.set("raw_mode", "native");
    let loaded = load_photo(path, None, &settings).map_err(|e| format!("{}: {e}", path.display()))?;
    match loaded.photo {
        Photo::Linear(image) => Ok(image),
        Photo::Camera(camera) => grainy_raw::rawcolor::develop_camera(&camera, &settings).map_err(|e| e.to_string()),
    }
}

fn luminance(image: &Image) -> Plane {
    let (h, w, _) = image.dim();
    Plane::from_shape_fn((h, w), |(y, x)| {
        image[[y, x, 0]] * 0.2126 + image[[y, x, 1]] * 0.7152 + image[[y, x, 2]] * 0.0722
    })
}

fn pyramid(base: Plane) -> Vec<Plane> {
    let mut levels = vec![base];
    loop {
        let p = levels.last().unwrap();
        let (h, w) = p.dim();
        if h.max(w) <= 80 || h.min(w) < 36 {
            return levels;
        }
        levels.push(Plane::from_shape_fn((h / 2, w / 2), |(y, x)| {
            let (y, x) = (2 * y, 2 * x);
            (p[[y, x]] + p[[y + 1, x]] + p[[y, x + 1]] + p[[y + 1, x + 1]]) * 0.25
        }));
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl Rect {
    fn image(w: usize, h: usize, offset: Offset) -> Self {
        Self { left: offset.x, top: offset.y, right: offset.x + w as i64, bottom: offset.y + h as i64 }
    }

    fn intersect(self, b: Self) -> Self {
        Self {
            left: self.left.max(b.left),
            top: self.top.max(b.top),
            right: self.right.min(b.right),
            bottom: self.bottom.min(b.bottom),
        }
    }

    fn union(self, b: Self) -> Self {
        Self {
            left: self.left.min(b.left),
            top: self.top.min(b.top),
            right: self.right.max(b.right),
            bottom: self.bottom.max(b.bottom),
        }
    }

    fn size(self) -> Result<(usize, usize), String> {
        let (w, h) = (self.right - self.left, self.bottom - self.top);
        if w < 1 || h < 1 {
            Err("There is no shared image area.".into())
        } else {
            Ok((w as usize, h as usize))
        }
    }
}

// Sample the same physical locations in both frames, excluding dark noise and clipped measurements.
fn measurements(a: &Plane, b: &Plane, offset: Offset, n: usize, mut sample: impl FnMut(f32, f32)) {
    let (h, w) = a.dim();
    let overlap = Rect::image(w, h, Offset::default()).intersect(Rect::image(w, h, offset));
    let sx = ((overlap.right - overlap.left).max(1) as usize / n).max(1);
    let sy = ((overlap.bottom - overlap.top).max(1) as usize / n).max(1);
    for y in (overlap.top..overlap.bottom).step_by(sy) {
        for x in (overlap.left..overlap.right).step_by(sx) {
            let v = a[[y as usize, x as usize]];
            let u = b[[(y - offset.y) as usize, (x - offset.x) as usize]];
            if v > 0.002 && u > 0.002 && v < 0.96 && u < 0.96 {
                sample(v, u);
            }
        }
    }
}

fn score(a: &Plane, b: &Plane, d: Offset) -> f64 {
    let (mut n, mut x, mut y, mut xx, mut yy, mut xy) = (0., 0., 0., 0., 0., 0.);
    measurements(a, b, d, 36, |v, u| {
        let (v, u) = (v as f64, u as f64);
        n += 1.;
        x += v;
        y += u;
        xx += v * v;
        yy += u * u;
        xy += v * u;
    });
    let var = ((xx * n - x * x).max(0.) * (yy * n - y * y).max(0.)).sqrt();
    if n < 32. || var < 1e-7 * n * n {
        -1.
    } else {
        (xy * n - x * y) / var
    }
}

fn register(a: &[Plane], b: &[Plane], mode: Mode, cancel: &AtomicBool) -> Result<Offset, String> {
    let mut offset = Offset::default();
    for level in (0..a.len()).rev() {
        let (h, w) = a[level].dim();
        let coarse = level + 1 == a.len();
        let centre = Offset { x: offset.x * 2, y: offset.y * 2 };
        let fraction = if mode == Mode::Hdr { 0.13 } else { 0.78 };
        let rx = if coarse { (w as f64 * fraction) as i64 } else { 3 };
        let ry = if coarse { (h as f64 * fraction) as i64 } else { 3 };
        let mut best = -1.;
        for y in centre.y - ry..=centre.y + ry {
            check_cancel(cancel)?;
            for x in centre.x - rx..=centre.x + rx {
                let d = Offset { x, y };
                let area = (w as i64 - x.abs()).max(0) * (h as i64 - y.abs()).max(0);
                if area < (w * h) as i64 / 5 {
                    continue;
                }
                let corr = score(&a[level], &b[level], d);
                // Prefer minimal motion when several shifts have the same correlation.
                if corr > best + 1e-8
                    || ((corr - best).abs() < 1e-8 && x.abs() + y.abs() < offset.x.abs() + offset.y.abs())
                {
                    offset = d;
                    best = corr;
                }
            }
        }
        // Coarse downsampling shifts the phase of fine detail. Judge confidence against
        // original pixels after refinement, not a blurred thumbnail.
        if best <= -1. || (level == 0 && best < 0.75) {
            return Err("Could not align the photos. Select overlapping, detailed frames of the same scene.".into());
        }
    }
    Ok(offset)
}

fn exposure(a: &Plane, b: &Plane, d: Offset) -> Result<f32, String> {
    let mut ratios = Vec::new();
    measurements(a, b, d, 96, |v, u| ratios.push(u / v));
    if ratios.len() < 32 {
        return Err("Too few unclipped matching pixels to estimate exposure.".into());
    }
    let mid = ratios.len() / 2;
    let ratio = *ratios.select_nth_unstable_by(mid, f32::total_cmp).1;
    if !(1. / 256. ..=256.).contains(&ratio) {
        return Err("The input exposures could not be matched.".into());
    }
    Ok(ratio)
}

struct Placed {
    width: usize,
    height: usize,
    frames: Vec<Frame>,
    cached: Vec<Option<Image>>,
}

fn placements(paths: &[PathBuf], mode: Mode, cancel: &AtomicBool, progress: &dyn Fn(usize)) -> Result<Placed, String> {
    let first_image = load_linear(&paths[0])?;
    let first = pyramid(luminance(&first_image));
    let (h, w) = first[0].dim();
    let frame_bytes = h.saturating_mul(w).saturating_mul(12);
    let mut cached_bytes = 0usize;
    let mut cached = Vec::with_capacity(paths.len());
    if frame_bytes <= CACHE_BYTES {
        cached_bytes += frame_bytes;
        cached.push(Some(first_image));
    } else {
        cached.push(None);
    }
    let mut frames = vec![Frame { offset: Offset::default(), exposure: 1. }];
    let mut previous = first;
    progress(1);
    for (index, path) in paths.iter().enumerate().skip(1) {
        check_cancel(cancel)?;
        let image = load_linear(path)?;
        let current = pyramid(luminance(&image));
        if current[0].dim() != (h, w) {
            return Err("The oriented dimensions of the selected photos must match.".into());
        }
        if cached_bytes.saturating_add(frame_bytes) <= CACHE_BYTES {
            cached_bytes += frame_bytes;
            cached.push(Some(image));
        } else {
            cached.push(None);
        }
        let relative = register(&previous, &current, mode, cancel)?;
        let ratio = exposure(&previous[0], &current[0], relative)?;
        let base = if mode == Mode::Hdr { frames[0] } else { *frames.last().unwrap() };
        frames.push(Frame {
            offset: Offset { x: base.offset.x + relative.x, y: base.offset.y + relative.y },
            exposure: base.exposure * ratio,
        });
        if mode == Mode::Panorama {
            previous = current;
        }
        progress(index + 1);
    }
    if mode == Mode::Hdr {
        let mut scales: Vec<_> = frames.iter().map(|frame| frame.exposure).collect();
        scales.sort_by(f32::total_cmp);
        if scales[scales.len() - 1] / scales[0] < 1.2 {
            return Err("HDR merge needs photographs with different exposures.".into());
        }
        let middle = scales[scales.len() / 2];
        for f in &mut frames {
            f.exposure /= middle;
        }
    }
    Ok(Placed { width: w, height: h, frames, cached })
}

fn canvas(w: usize, h: usize, frames: &[Frame], mode: Mode) -> Result<Rect, String> {
    let first = Rect::image(w, h, Offset::default());
    let mut shared = first;
    let mut full = first;
    for frame in frames {
        let bounds = Rect::image(w, h, frame.offset);
        shared = shared.intersect(bounds);
        full = full.union(bounds);
    }
    if mode == Mode::Hdr {
        shared.size()?;
        return Ok(shared);
    }
    let dx = full.right - full.left - w as i64;
    let dy = full.bottom - full.top - h as i64;
    if dx * 100 / w as i64 >= dy * 100 / h as i64 {
        if dx < (w / 20).max(1) as i64 {
            return Err("Panorama merge needs offset views of the same scene.".into());
        }
        full.top = shared.top;
        full.bottom = shared.bottom;
    } else {
        if dy < (h / 20).max(1) as i64 {
            return Err("Panorama merge needs offset views of the same scene.".into());
        }
        full.left = shared.left;
        full.right = shared.right;
    }
    full.size()?;
    Ok(full)
}

fn zeros(n: usize) -> Result<Vec<f32>, String> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| "Not enough memory for the merged image.".to_string())?;
    v.resize(n, 0.);
    Ok(v)
}

fn blend(
    paths: &[PathBuf],
    mut placed: Placed,
    bounds: Rect,
    mode: Mode,
    cancel: &AtomicBool,
    progress: &dyn Fn(usize),
) -> Result<Image, String> {
    let (w, h) = bounds.size()?;
    let count = w.checked_mul(h).ok_or("Merged image dimensions are too large.")?;
    let mut sums = zeros(count.checked_mul(3).ok_or("Merged image dimensions are too large.")?)?;
    let mut weights = zeros(count)?;
    let input = (placed.width, placed.height);
    for (i, (path, frame)) in paths.iter().zip(&placed.frames).enumerate() {
        check_cancel(cancel)?;
        let image = match placed.cached.get_mut(i).and_then(Option::take) {
            Some(image) => image,
            None => load_linear(path)?,
        };
        let (ih, iw, _) = image.dim();
        if (iw, ih) != input {
            return Err(format!("Input changed while merging: {}", path.display()));
        }
        let r = bounds.intersect(Rect::image(iw, ih, frame.offset));
        if r.top < r.bottom && r.left < r.right {
            let y_start = (r.top - bounds.top) as usize;
            let y_end = (r.bottom - bounds.top) as usize;
            let x_start = (r.left - bounds.left) as usize;
            let x_end = (r.right - bounds.left) as usize;
            let edge_scale = (iw.min(ih) as f32 * 0.15).max(1.);
            let inv_exposure = 1. / frame.exposure;
            let sum_rows = &mut sums[y_start * w * 3..y_end * w * 3];
            let weight_rows = &mut weights[y_start * w..y_end * w];
            sum_rows.par_chunks_mut(w * 3).zip(weight_rows.par_chunks_mut(w)).enumerate().for_each(
                |(row_off, (sum_row, weight_row))| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let y = r.top + row_off as i64;
                    let sy = (y - frame.offset.y) as usize;
                    let w_span = &mut weight_row[x_start..x_end];
                    let s_span = &mut sum_row[x_start * 3..x_end * 3];
                    for (dx, (weight_slot, sum_px)) in
                        w_span.iter_mut().zip(s_span.as_chunks_mut::<3>().0.iter_mut()).enumerate()
                    {
                        let x = bounds.left + (x_start + dx) as i64;
                        let sx = (x - frame.offset.x) as usize;
                        let px = [image[[sy, sx, 0]], image[[sy, sx, 1]], image[[sy, sx, 2]]];
                        let weight = match mode {
                            Mode::Hdr => {
                                // Brighter unsaturated observations have lower relative shot noise.
                                // Exclude all three channels together to prevent colour casts near clipping.
                                let peak = px[0].max(px[1]).max(px[2]).max(0.);
                                frame.exposure * (peak / 0.025).clamp(0.001, 1.) * ((0.985 - peak) / 0.12).clamp(0., 1.)
                                    + 1e-10 * inv_exposure
                            }
                            Mode::Panorama => {
                                let edge = (sx + 1).min(iw - sx).min(sy + 1).min(ih - sy) as f32;
                                (edge / edge_scale).min(1.)
                            }
                        };
                        *weight_slot += weight;
                        let scale = weight * inv_exposure;
                        sum_px[0] += px[0] * scale;
                        sum_px[1] += px[1] * scale;
                        sum_px[2] += px[2] * scale;
                    }
                },
            );
            check_cancel(cancel)?;
        }
        progress(paths.len() + i + 1);
    }
    let (pixels, _) = sums.as_chunks_mut::<3>();
    pixels.par_iter_mut().zip(weights.par_iter()).try_for_each(|(px, &w)| {
        if w <= 0. {
            return Err("The selected photos leave a gap in the panorama.".to_string());
        }
        for c in px {
            *c /= w;
        }
        Ok(())
    })?;
    Image::from_shape_vec((h, w, 3), sums).map_err(|e| e.to_string())
}

/// Merge the selected originals in supplied order. HDR uses a static scene with exposure brackets;
/// planar panorama uses adjacent views with common content. The result is a new linear float TIFF.
pub fn merge_to_tiff(
    paths: &[PathBuf],
    output: &Path,
    mode: Mode,
    cancel: &AtomicBool,
    progress: &dyn Fn(usize),
) -> Result<(usize, usize), String> {
    if paths.len() < 2 {
        return Err("Select at least two photos to merge.".into());
    }
    if !output
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("tif") || s.eq_ignore_ascii_case("tiff"))
    {
        return Err("Choose a TIFF filename (.tif or .tiff).".into());
    }
    if output.exists() {
        return Err(format!("The file already exists: {}", output.display()));
    }
    check_cancel(cancel)?;
    let mut identities = std::collections::HashSet::new();
    let fingerprint = |path: &Path| -> Result<_, String> {
        let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((metadata.len(), metadata.modified().map_err(|e| e.to_string())?))
    };
    let mut initial = Vec::with_capacity(paths.len());
    for path in paths {
        let canonical = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !identities.insert(canonical) {
            return Err("Select different original files to merge.".into());
        }
        initial.push(fingerprint(path)?);
    }
    let placed = placements(paths, mode, cancel, progress)?;
    let bounds = canvas(placed.width, placed.height, &placed.frames, mode)?;
    let image = blend(paths, placed, bounds, mode, cancel, progress)?;
    check_cancel(cancel)?;
    for (path, before) in paths.iter().zip(initial) {
        if fingerprint(path)? != before {
            return Err(format!("Input changed while merging: {}", path.display()));
        }
    }
    let options = ExportOptions { format: ExportFormat::TiffHdr32, hdr_enabled: true, ..ExportOptions::default() };
    grainy_codecs::export_image(&image, &paths[0], output, &options, &Cms::for_export_of_developed_pixels())
        .map_err(|e| e.to_string())
}
