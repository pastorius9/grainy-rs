//! Port of luma/scan_border.py: find the unexposed film edge or scanner border around a scanned photo.

use grainy_core::Settings;
use ndarray::{s, Array2, Array3, ArrayView3};

use crate::geometry::{geometry, UseCrop};
use crate::lens::{lens_shading, LensModel};
use crate::numeric::{pairwise_f32, py_round_digits};
use crate::{Error, Result};

/// sRGB luma threshold for border pixels (`DARK`).
pub const DARK: f32 = 0.10;
/// Share of a line that must be border-coloured (`COVERAGE`).
pub const COVERAGE: f64 = 0.9;
/// Film edges are thin; a wider dark run is picture content (`MAX_SIDE`).
pub const MAX_SIDE: f64 = 0.10;
/// The picture inside must differ from the border by this much (`CONTRAST`).
pub const CONTRAST: f64 = 0.08;
/// Soft transition removed after the border (`EDGE_SPAN`).
pub const EDGE_SPAN: f64 = 0.02;
/// Extra inset past the transition (`SAFETY`).
pub const SAFETY: f64 = 0.002;

/// numpy's iterator buffer (elements): reductions over strided views are summed buffer by buffer.
const BUFFER: usize = 8192;

/// One side of the frame: which way its lines run from the edge inward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// `luma.T`: columns from the left.
    Left,
    /// `luma.T[::-1]`: columns from the right.
    Right,
    /// `luma`: rows from the top.
    Top,
    /// `luma[::-1]`: rows from the bottom.
    Bottom,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }
}

/// Port of `_luma`: Rec.709 luma of the image subsampled to at most `longest` pixels on its long side
/// (float32, `small[..., :3] @ weights`). Needs 3 or more channels.
pub fn luma(image: ArrayView3<'_, f32>, longest: usize) -> Result<Array2<f32>> {
    let (h, w, c) = image.dim();
    if c < 3 {
        return Err(Error::Value("scan border detection needs an RGB image".into()));
    }
    let step = ((h.max(w) as f64 / longest as f64).ceil() as usize).max(1);
    let small = image.slice(s![..;step, ..;step, ..]);
    Ok(small.map_axis(ndarray::Axis(2), |p| p[0] * 0.2126f32 + p[1] * 0.7152f32 + p[2] * 0.0722f32))
}

/// The values of lines `start..stop` of a side in the order numpy's mean reduces them.
fn band(luma: &Array2<f32>, side: Side, start: usize, stop: usize) -> Vec<f32> {
    let (h, w) = luma.dim();
    let mut out = Vec::new();
    match side {
        Side::Top => out.extend(luma.slice(s![start..stop, ..]).iter()),
        Side::Bottom => {
            for i in start..stop {
                out.extend(luma.row(h - 1 - i).iter());
            }
        }
        Side::Left => {
            for y in 0..h {
                out.extend(luma.slice(s![y, start..stop]).iter());
            }
        }
        Side::Right => {
            for y in 0..h {
                for i in start..stop {
                    out.push(luma[[y, w - 1 - i]]);
                }
            }
        }
    }
    out
}

/// `float(lines[start:stop].mean())`: float32 pairwise sums (per 8192-value buffer for strided views).
fn band_mean(luma: &Array2<f32>, side: Side, start: usize, stop: usize) -> f64 {
    let values = band(luma, side, start, stop);
    let sum = if side == Side::Top {
        pairwise_f32(&values)
    } else {
        let mut acc = 0.0f32;
        for chunk in values.chunks(BUFFER) {
            acc += pairwise_f32(chunk);
        }
        acc
    };
    f64::from(sum / values.len() as f32)
}

/// Line `i` of a side (the i-th row or column from that edge).
fn line(luma: &Array2<f32>, side: Side, i: usize) -> Vec<f32> {
    let (h, w) = luma.dim();
    match side {
        Side::Top => luma.row(i).to_vec(),
        Side::Bottom => luma.row(h - 1 - i).to_vec(),
        Side::Left => luma.column(i).to_vec(),
        Side::Right => luma.column(w - 1 - i).to_vec(),
    }
}

fn dark_share(values: &[f32]) -> f64 {
    values.iter().filter(|&&v| v < DARK).count() as f64 / values.len() as f64
}

/// Port of `_side`: border depth in lines for one side, and its kind (`"dark"`), or `(0, None)`.
pub fn side(luma: &Array2<f32>, side: Side) -> (usize, Option<&'static str>) {
    let (h, w) = luma.dim();
    let count = match side {
        Side::Top | Side::Bottom => h,
        Side::Left | Side::Right => w,
    };
    let limit = (count as f64 * MAX_SIDE) as usize;
    let mut depth = 0;
    while depth <= limit && depth < count && dark_share(&line(luma, side, depth)) >= COVERAGE {
        depth += 1;
    }
    if depth < 2 || depth > limit {
        return (0, None);
    }
    let stop = (depth + 3usize.max(count / 30)).min(count);
    if stop <= depth || band_mean(luma, side, depth, stop) - band_mean(luma, side, 0, depth) < CONTRAST {
        return (0, None);
    }
    let end = count.min(depth + 1usize.max((count as f64 * EDGE_SPAN) as usize));
    while depth < end && dark_share(&line(luma, side, depth)) > 0.02 {
        depth += 1;
    }
    (depth + (count as f64 * SAFETY).ceil() as usize, Some("dark"))
}

/// The result of [`detect`]: the crop (x0, y0, x1, y1, normalised and rounded to 5 digits) and the kind of
/// each side that has a border, in the oracle's order (left, right, top, bottom).
pub type Border = ([f64; 4], Vec<(&'static str, &'static str)>);

/// Port of `detect` on an sRGB-encoded image (3 channels or more): the border crop, or `None`.
pub fn detect(image: ArrayView3<'_, f32>) -> Result<Option<Border>> {
    let luma = luma(image, 1000)?;
    let (h, w) = luma.dim();
    if h < 20 || w < 20 {
        return Ok(None);
    }
    let sides = [Side::Left, Side::Right, Side::Top, Side::Bottom].map(|s| (s, side(&luma, s)));
    if sides.iter().all(|(_, (depth, _))| *depth == 0) {
        return Ok(None);
    }
    let depth = |i: usize| sides[i].1 .0 as f64;
    let (wf, hf) = (w as f64, h as f64);
    let (x0, x1) = (depth(0) / wf, 1.0 - depth(1) / wf);
    let (y0, y1) = (depth(2) / hf, 1.0 - depth(3) / hf);
    if x1 - x0 < 0.5 || y1 - y0 < 0.5 {
        return Ok(None);
    }
    let crop = [x0, y0, x1, y1].map(|v| py_round_digits(v, 5));
    let kinds =
        sides.iter().filter_map(|(s, (d, kind))| if *d > 0 { kind.map(|k| (s.name(), k)) } else { None }).collect();
    Ok(Some((crop, kinds)))
}

/// Port of `crop_for` from its linear frame on: the caller passes `linear_frame(resize_float(source, 1200),
/// settings)` (RAW development and the negative conversion live elsewhere); this applies lens shading, the
/// uncropped geometry, sRGB encoding and [`detect`]. Returns the crop for `settings['crop']`, or `None`.
pub fn crop_for_frame(
    frame: &Array3<f32>,
    settings: &Settings,
    lens: Option<&dyn LensModel>,
) -> Result<Option<[f64; 4]>> {
    let shaded = lens_shading(frame, settings, lens)?;
    let turned = geometry(&shaded, settings, UseCrop::No, lens)?;
    let encoded: Array3<f32> = grainy_color::to_srgb(&turned);
    Ok(detect(encoded.view())?.map(|(crop, _)| crop))
}
