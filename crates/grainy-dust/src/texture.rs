//! Ports of luma/dust_texture.py. Prepared writes are applied only after all donor searches finish.
use crate::{checkpoint, invalid, Cancel, Result};
use grainy_core::Image;
use grainy_imgproc as cv;
use ndarray::{s, Array2};
use rayon::prelude::*;
/// Port of luma/dust_texture.py BLOCK.
pub const BLOCK: usize = 8;
/// Port of luma/dust_texture.py CONTEXT.
pub const CONTEXT: usize = 4;
/// Port of luma/dust_texture.py HALO.
pub const HALO: usize = 6;
/// Port of luma/dust_texture.py SEARCH.
pub const SEARCH: usize = 48;
/// Port of luma/dust_texture.py restore stats.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    /// Port of restore stats.tiles.
    pub tiles: usize,
    /// Port of restore stats.transferred_pixels.
    pub transferred_pixels: usize,
    /// Port of restore stats.fallback_pixels.
    pub fallback_pixels: usize,
}
#[derive(Debug)]
struct Pending {
    y: usize,
    x: usize,
    selected: Array2<bool>,
    values: Vec<[f32; 3]>,
}
/// Port of luma/dust_texture.py _prepare return value, with immutable pending writes.
#[derive(Debug)]
pub struct Prepared {
    pending: Vec<Pending>,
    stats: Stats,
}
impl Prepared {
    /// Port of _prepare transferred/fallback counts, plus the number of requested blocks.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }
    /// Port of luma/dust_texture.py restore's ordered pending-write application.
    pub fn apply(self, smooth: &mut Image) -> Result<Stats> {
        crate::repair::image(smooth)?;
        let (h, w, _) = smooth.dim();
        if self.pending.iter().any(|p| p.y + p.selected.nrows() > h || p.x + p.selected.ncols() > w) {
            return Err(invalid("pending texture shape"));
        }
        for p in self.pending {
            let mut n = 0;
            for ((y, x), &v) in p.selected.indexed_iter() {
                if v {
                    for c in 0..3 {
                        smooth[(p.y + y, p.x + x, c)] = p.values[n][c];
                    }
                    n += 1;
                }
            }
        }
        Ok(self.stats)
    }
}
fn validate(original: &Image, smooth: &Image, mask: &Array2<u8>) -> Result<()> {
    crate::repair::image(original)?;
    crate::repair::image(smooth)?;
    if original.dim() != smooth.dim() || mask.dim() != (original.dim().0, original.dim().1) {
        return Err(invalid("texture shape mismatch"));
    }
    Ok(())
}
/// Port of luma/dust_texture.py _prepare. Blocks use row,column block indices.
pub fn prepare(
    original: &Image,
    smooth: &Image,
    mask: &Array2<u8>,
    blocks: &[(usize, usize)],
    cancel: Cancel<'_>,
) -> Result<Prepared> {
    validate(original, smooth, mask)?;
    let (h, w) = mask.dim();
    let mut pending = Vec::new();
    let mut stats = Stats { tiles: blocks.len(), ..Stats::default() };
    for &(row, column) in blocks {
        checkpoint(cancel)?;
        if row >= h.div_ceil(BLOCK) || column >= w.div_ceil(BLOCK) {
            return Err(invalid("texture block index"));
        }
        let y = row * BLOCK;
        let x = column * BLOCK;
        let bottom = h.min(y + BLOCK);
        let right = w.min(x + BLOCK);
        let top = y.saturating_sub(CONTEXT);
        let left = x.saturating_sub(CONTEXT);
        let end_y = h.min(bottom + CONTEXT);
        let end_x = w.min(right + CONTEXT);
        let th = end_y - top;
        let tw = end_x - left;
        let sy = HALO.max(top.saturating_sub(SEARCH));
        let sx = HALO.max(left.saturating_sub(SEARCH));
        let ey = h.saturating_sub(HALO).min(end_y + SEARCH);
        let ex = w.saturating_sub(HALO).min(end_x + SEARCH);
        let selected = mask.slice(s![y..bottom, x..right]).mapv(|v| v > 0);
        let count_selected = selected.iter().filter(|&&v| v).count();
        if ey < sy + th || ex < sx + tw {
            stats.fallback_pixels += count_selected;
            continue;
        }
        let source = original.slice(s![sy - HALO..ey + HALO, sx - HALO..ex + HALO, ..]).to_owned();
        let source_blur = cv::gaussian_blur_unfused(&source, (13, 13), 1.5, 0., cv::Border::Reflect101)?;
        let source_low = source_blur.slice(s![HALO..source.dim().0 - HALO, HALO..source.dim().1 - HALO, ..]).to_owned();
        let local = smooth
            .slice(s![
                top.saturating_sub(HALO)..h.min(end_y + HALO),
                left.saturating_sub(HALO)..w.min(end_x + HALO),
                ..
            ])
            .to_owned();
        let low = cv::gaussian_blur_unfused(&local, (13, 13), 1.5, 0., cv::Border::Reflect101)?;
        let ty = top - top.saturating_sub(HALO);
        let tx = left - left.saturating_sub(HALO);
        let target_low = low.slice(s![ty..ty + th, tx..tx + tw, ..]).to_owned();
        let count = th * tw;
        let mut score =
            cv::match_template(&source_low, &target_low, cv::TemplateMatch::SqDiff)?.mapv(|v| v / (count * 3) as f32);
        let means = cv::box_filter(&source_low, (tw, th), Some((0, 0)), true, cv::Border::Reflect101)?;
        // NumPy's multi-axis reduction over interleaved RGB runs three float32 accumulators.
        let mut mean = [0f32; 3];
        for yy in 0..th {
            for xx in 0..tw {
                for c in 0..3 {
                    mean[c] += target_low[(yy, xx, c)];
                }
            }
        }
        for v in &mut mean {
            *v /= count as f32;
        }
        let occupied = mask.slice(s![sy - HALO..ey + HALO, sx - HALO..ex + HALO]).mapv(|v| u8::from(v > 0));
        let integral = cv::integral_u8(&occupied)?;
        let kh = th + HALO * 2;
        let kw = tw + HALO * 2;
        for ((yy, xx), value) in score.indexed_iter_mut() {
            let mut offset = [0f32; 3];
            for c in 0..3 {
                let d = means[(yy, xx, c)] - mean[c];
                offset[c] = d * d;
            }
            *value -= ((offset[0] + offset[1]) + offset[2]) / 3. * 0.9;
            let dx = xx as i64 + sx as i64 - left as i64;
            let dy = yy as i64 + sy as i64 - top as i64;
            *value = (*value as f64 + 1e-6f32 as f64 * (dx * dx + dy * dy) as f64 / (SEARCH * SEARCH) as f64) as f32;
            let used =
                integral[(yy + kh, xx + kw)] - integral[(yy, xx + kw)] - integral[(yy + kh, xx)] + integral[(yy, xx)];
            if used > 0 {
                *value = f32::INFINITY;
            }
        }
        let mut best = (f32::INFINITY, 0, 0);
        for ((yy, xx), &v) in score.indexed_iter() {
            if v < best.0 {
                best = (v, yy, xx);
            }
        }
        if !best.0.is_finite() {
            stats.fallback_pixels += count_selected;
            continue;
        }
        let (_, dy, dx) = best;
        let by = y - top;
        let bx = x - left;
        let mut values = Vec::new();
        for ((yy, xx), &v) in selected.indexed_iter() {
            if v {
                let mut pixel = [0.; 3];
                for c in 0..3 {
                    pixel[c] = ((target_low[(by + yy, bx + xx, c)]
                        + original[(sy + dy + by + yy, sx + dx + bx + xx, c)])
                        - source_low[(dy + by + yy, dx + bx + xx, c)])
                        .max(0.);
                }
                values.push(pixel);
            }
        }
        pending.push(Pending { y, x, selected, values });
        stats.transferred_pixels += count_selected;
    }
    Ok(Prepared { pending, stats })
}
/// Port of luma/dust_texture.py restore. Returns the oracle's stats while updating smooth in place.
pub fn restore(original: &Image, smooth: &mut Image, mask: &Array2<u8>, cancel: Cancel<'_>) -> Result<Stats> {
    checkpoint(cancel)?;
    validate(original, smooth, mask)?;
    let (h, w) = mask.dim();
    let mut blocks = Vec::new();
    for row in 0..h.div_ceil(BLOCK) {
        for column in 0..w.div_ceil(BLOCK) {
            if mask
                .slice(s![row * BLOCK..h.min((row + 1) * BLOCK), column * BLOCK..w.min((column + 1) * BLOCK)])
                .iter()
                .any(|v| *v > 0)
            {
                blocks.push((row, column));
            }
        }
    }
    let results: Vec<_> = blocks.par_iter().map(|&b| prepare(original, smooth, mask, &[b], cancel)).collect();
    let mut all = Prepared { pending: Vec::new(), stats: Stats::default() };
    for result in results {
        let p = result?;
        all.pending.extend(p.pending);
        all.stats.tiles += p.stats.tiles;
        all.stats.transferred_pixels += p.stats.transferred_pixels;
        all.stats.fallback_pixels += p.stats.fallback_pixels;
    }
    checkpoint(cancel)?;
    all.apply(smooth)
}
