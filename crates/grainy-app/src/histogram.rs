//! The displayed frame, its histogram and its clipping: port of luma/widgets.py `histogram_bins`,
//! `clipping_channels` and the clipping overlay of `PhotoView.drawForeground`.

use grainy_core::Image;
use rayon::prelude::*;

/// 128 bins per channel.
pub type Bins = [[u32; 128]; 3];

/// Shadow and highlight clipping per channel (`clipping_channels`): `(shadows, highlights)`, each R, G, B.
pub type Channels = ([bool; 3], [bool; 3]);

/// Overlay mask bit: a channel at or above white (drawn red).
pub const HIGHLIGHT: u8 = 1;
/// Overlay mask bit: a channel below black (drawn blue).
pub const SHADOW: u8 = 2;

// Python floats compared with a float32 array become float32 (numpy 2's weak scalars).
const LOW: f32 = (1.0f64 / 510.0) as f32;
const HIGH: f32 = (1.0f64 - 1.0f64 / 510.0) as f32;

/// The 8-bit value the screen shows for a developed value: `np.uint8(np.clip(v,0,1)*255+.5)` in float32
/// (NaN becomes 0, as numpy's cast of NaN does on this platform).
pub fn to_byte(v: f32) -> u8 {
    let x = v.clamp(0.0, 1.0) * 255.0 + 0.5;
    if x.is_nan() {
        0
    } else {
        x as u8
    }
}

/// An 8-bit RGBA frame of a developed sRGB image, with its clipping mask and channels.
pub struct Converted {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// One byte per pixel: [`HIGHLIGHT`] and [`SHADOW`] bits.
    pub mask: Vec<u8>,
    pub channels: Channels,
}

/// Converts a developed H×W×3 image (display-referred, 0–1) to RGBA bytes and the clipping mask, in one pass
/// over rows on the rayon pool.
pub fn convert(image: &Image) -> Converted {
    let (height, width, _) = image.dim();
    let standard = image.as_standard_layout();
    let data = standard.as_slice().unwrap_or(&[]);
    let mut rgba = vec![0u8; width * height * 4];
    let mut mask = vec![0u8; width * height];
    // Per row: low/high flags per channel.
    let flags: Vec<[bool; 6]> = rgba
        .par_chunks_mut((width * 4).max(1))
        .zip(mask.par_chunks_mut(width.max(1)))
        .enumerate()
        .map(|(y, (row, mask_row))| {
            let mut f = [false; 6];
            let src = &data[y * width * 3..(y + 1) * width * 3];
            for x in 0..width {
                let p = &src[x * 3..x * 3 + 3];
                let mut bits = 0u8;
                for c in 0..3 {
                    let v = p[c];
                    row[x * 4 + c] = to_byte(v);
                    // fmin/fmax skip NaN; a NaN compares false, as `any(a < ...)` does.
                    if v < LOW {
                        f[c] = true;
                        bits |= SHADOW;
                    }
                    if v >= HIGH {
                        f[3 + c] = true;
                        bits |= HIGHLIGHT;
                    }
                }
                row[x * 4 + 3] = 255;
                mask_row[x] = bits;
            }
            f
        })
        .collect();
    let mut channels = ([false; 3], [false; 3]);
    for f in flags {
        for c in 0..3 {
            channels.0[c] |= f[c];
            channels.1[c] |= f[3 + c];
        }
    }
    Converted { width, height, rgba, mask, channels }
}

/// Port of `histogram_bins` on the bytes the screen shows (the oracle bins `uint8(clip(v)*255+.5) >> 1` of the
/// float frame; the bytes are those values): every `step`-th row and column, `step = max(1, height // 300)`.
pub fn bins(rgba: &[u8], width: usize, height: usize) -> Bins {
    let mut bins = [[0u32; 128]; 3];
    if rgba.len() < width * height * 4 {
        return bins;
    }
    let step = (height / 300).max(1);
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            let at = (y * width + x) * 4;
            for c in 0..3 {
                bins[c][usize::from(rgba[at + c] >> 1)] += 1;
            }
        }
    }
    bins
}

/// The overlay picture of the clipping mask (`clipped` of `drawForeground`): highlights red, shadows blue (shadows
/// drawn last, as the oracle does), the rest transparent.
pub fn overlay(mask: &[u8], shadows: bool, highlights: bool) -> Vec<u8> {
    let mut out = vec![0u8; mask.len() * 4];
    for (pixel, bits) in out.as_chunks_mut::<4>().0.iter_mut().zip(mask) {
        if highlights && bits & HIGHLIGHT != 0 {
            pixel.copy_from_slice(&[255, 40, 30, 150]);
        }
        if shadows && bits & SHADOW != 0 {
            pixel.copy_from_slice(&[40, 90, 255, 160]);
        }
    }
    out
}

/// numpy's `percentile(bins, 98)` (linear interpolation) for the histogram's scale.
pub fn percentile98(bins: &[u32; 128]) -> f32 {
    let mut sorted: Vec<u32> = bins.to_vec();
    sorted.sort_unstable();
    let at = 0.98 * 127.0;
    let low = at as usize;
    let frac = at - low as f32;
    sorted[low] as f32 * (1.0 - frac) + sorted[(low + 1).min(127)] as f32 * frac
}
