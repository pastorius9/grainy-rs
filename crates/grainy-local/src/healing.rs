//! Local donor selection and boundary-matched healing: `luma/healing.py` (`automatic_offset`, `blend_boundary`,
//! `inpaint`) over the [`Inpaint`] and [`DistanceTransform`] traits.

use std::collections::BTreeSet;

use grainy_core::{Image, Plane};
use grainy_imgproc::Interpolation;
use ndarray::{s, Array2, ArrayView2, Axis, Zip};

use crate::np::{self, mean_flat, py_max};
use crate::{DistanceTransform, Inpaint, Result};

/// The binary mask `np.uint8(weight > 0) * 255` of a weight plane.
fn binary(weight: ArrayView2<'_, f32>) -> Array2<u8> {
    weight.mapv(|w| if w > 0.0 { 255 } else { 0 })
}

/// Port of luma/healing.py `blend_boundary`: the donor's texture with the colour difference to `target` along the
/// unpainted edge interpolated (inpainted) into the painted area. `weight` is the brush weight of the region.
pub fn blend_boundary(
    target: &Image,
    donor: &Image,
    weight: ArrayView2<'_, f32>,
    inpaint: &dyn Inpaint,
) -> Result<Image> {
    let mask = binary(weight);
    if mask.iter().all(|&v| v == 0) || mask.iter().all(|&v| v != 0) {
        return Ok(donor.clone());
    }
    let difference = target - donor;
    let mut result = donor.clone();
    for c in 0..3 {
        let mut known = difference.index_axis(Axis(2), c).to_owned();
        Zip::from(&mut known).and(&mask).for_each(|k, &m| {
            if m > 0 {
                *k = 0.0;
            }
        });
        let filled = inpaint.inpaint_ns(&known, &mask, 3.0)?;
        check_plane(&filled, known.dim())?;
        Zip::from(result.index_axis_mut(Axis(2), c)).and(&filled).for_each(|r, &f| *r += f);
    }
    Ok(result)
}

/// Port of luma/healing.py `inpaint`: each channel of `target` inpainted where `weight` is positive (unchanged
/// when the whole region is painted).
pub fn inpaint(target: &Image, weight: ArrayView2<'_, f32>, inpaint: &dyn Inpaint) -> Result<Image> {
    let mask = binary(weight);
    if mask.iter().all(|&v| v != 0) {
        return Ok(target.clone());
    }
    let mut out = Image::zeros(target.raw_dim());
    for c in 0..3 {
        let plane = target.index_axis(Axis(2), c).to_owned();
        let filled = inpaint.inpaint_ns(&plane, &mask, 3.0)?;
        check_plane(&filled, plane.dim())?;
        out.index_axis_mut(Axis(2), c).assign(&filled);
    }
    Ok(out)
}

pub(crate) fn check_plane(plane: &Plane, dim: (usize, usize)) -> Result<()> {
    if plane.dim() != dim {
        return Err(crate::Error::Shape(format!("an inpainted plane of {:?} for a region of {dim:?}", plane.dim())));
    }
    Ok(())
}

/// The 24 directions `(cos, sin)` of [`automatic_offset`]'s donor search: `np.cos`/`np.sin` of
/// `np.linspace(0, 2π, 24, endpoint=False)`.
pub fn directions() -> [(f64, f64); 24] {
    let step = 2.0 * std::f64::consts::PI / 24.0;
    std::array::from_fn(|i| {
        let angle = i as f64 * step;
        (angle.cos(), angle.sin())
    })
}

/// Port of luma/healing.py `automatic_offset`: a nearby clean translation `(dx, dy)` in pixels of `image` for the
/// painted `mask`, ranked by how well the donor matches the ring around the painted shape, or `None`.
///
/// The search runs on a copy of at most 512 pixels (area resize). `distance` is OpenCV's L2 distance transform.
pub fn automatic_offset(image: &Image, mask: &Plane, distance: &dyn DistanceTransform) -> Result<Option<(f64, f64)>> {
    let (h, w) = mask.dim();
    let scale = {
        let ratio = 512.0 / h.max(w) as f64;
        if ratio < 1.0 {
            ratio
        } else {
            1.0
        }
    };
    let size = (
        ((w as f64 * scale).round_ties_even() as usize).max(1),
        ((h as f64 * scale).round_ties_even() as usize).max(1),
    );
    let small;
    let a: &Image = if scale < 1.0 {
        small = grainy_imgproc::resize(image, size, Interpolation::Area)?;
        &small
    } else {
        image
    };
    // Area resize keeps very small marks in the exclusion map.
    let painted = mask.mapv(|v| if v > 0.0 { 1f32 } else { 0.0 });
    let marked = grainy_imgproc::resize(&painted, size, Interpolation::Area)?.mapv(|v| v > 0.0);
    let (hh, ww) = marked.dim();
    let mut xs_min = usize::MAX;
    let mut xs_max = 0;
    let mut ys_min = usize::MAX;
    let mut ys_max = 0;
    for ((y, x), &m) in marked.indexed_iter() {
        if m {
            xs_min = xs_min.min(x);
            xs_max = xs_max.max(x);
            ys_min = ys_min.min(y);
            ys_max = ys_max.max(y);
        }
    }
    if xs_min == usize::MAX {
        return Ok(None);
    }
    let marked_u8 = marked.mapv(u8::from);
    let field = distance.distance_l2(&marked_u8)?;
    let largest = field.iter().copied().fold(f32::NEG_INFINITY, np::maximum);
    let radius = py_max(2.0, f64::from(largest));
    // With every pixel painted the distance map holds float32's largest value and numpy cannot allocate the
    // dilation kernel; the oracle raises this error.
    if radius * 0.5 >= 4.0e18 || radius.is_nan() {
        return Err(crate::Error::Invalid("Maximum allowed dimension exceeded".into()));
    }
    let margin = ((radius * 0.5).round_ties_even() as i64).max(2);
    let side = (2 * margin + 1) as usize;
    let grown = grainy_imgproc::dilate(&marked_u8, &Array2::ones((side, side)), None, 1, None)?;
    let mut rx = Vec::new();
    let mut ry = Vec::new();
    for ((y, x), &g) in grown.indexed_iter() {
        if g > 0 && !marked[[y, x]] {
            ry.push(y as i64);
            rx.push(x as i64);
        }
    }
    if rx.is_empty() {
        return Ok(None);
    }
    let step = (rx.len() / 512).max(1);
    let rx: Vec<i64> = rx.into_iter().step_by(step).collect();
    let ry: Vec<i64> = ry.into_iter().step_by(step).collect();
    let baseline: Vec<[f32; 3]> =
        rx.iter().zip(&ry).map(|(&x, &y)| std::array::from_fn(|c| a[[y as usize, x as usize, c]])).collect();
    let (x0, x1, y0, y1) = (xs_min as i64, xs_max as i64 + 1, ys_min as i64, ys_max as i64 + 1);
    let footprint = marked.slice(s![y0 as usize..y1 as usize, x0 as usize..x1 as usize]);
    let mut shifts = BTreeSet::new();
    let longest = hh.max(ww) as f64;
    for distance in [radius * 3.0, radius * 4.5, radius * 7.0, radius * 11.0, longest * 0.25, longest * 0.5] {
        for (c, sn) in directions() {
            shifts.insert(((distance * c).round_ties_even() as i64, (distance * sn).round_ties_even() as i64));
        }
    }
    // Also allow near-border strokes to find a source beyond their full width.
    for (dx, dy) in [(x1 - x0 + margin, 0), (0, y1 - y0 + margin)] {
        shifts.insert((dx, dy));
        shifts.insert((-dx, -dy));
    }
    let (hh, ww) = (hh as i64, ww as i64);
    let mut best = None;
    let mut best_score = f64::INFINITY;
    for (dx, dy) in shifts {
        if x0 + dx < 0 || x1 + dx > ww || y0 + dy < 0 || y1 + dy > hh {
            continue;
        }
        let shifted = marked.slice(s![(y0 + dy) as usize..(y1 + dy) as usize, (x0 + dx) as usize..(x1 + dx) as usize]);
        if Zip::from(&shifted).and(&footprint).fold(false, |any, &p, &q| any || (p && q)) {
            continue;
        }
        let mut delta: Vec<f32> = Vec::new();
        for (i, (&x, &y)) in rx.iter().zip(&ry).enumerate() {
            let (sx, sy) = (x + dx, y + dy);
            if sx >= 0 && sx < ww && sy >= 0 && sy < hh {
                for c in 0..3 {
                    delta.push(a[[sy as usize, sx as usize, c]] - baseline[i][c]);
                }
            }
        }
        let valid = delta.len() / 3;
        if (valid as f64) < py_max(4.0, rx.len() as f64 * 0.5) {
            continue;
        }
        // Match structure as well as average colour. A modest distance penalty breaks ties on flat fields.
        let squares: Vec<f32> = delta.iter().map(|d| d * d).collect();
        let mut sums = [0f32; 3];
        for row in delta.as_chunks::<3>().0 {
            for c in 0..3 {
                sums[c] += row[c];
            }
        }
        let means = sums.map(|v| np::mean_of_sum(v, valid));
        let deviations: Vec<f32> =
            delta.as_chunks::<3>().0.iter().flat_map(|row| [0, 1, 2].map(|c| (row[c] - means[c]).abs())).collect();
        let structure = 0.15_f64 as f32 * mean_flat(&deviations);
        let mut score = f64::from(mean_flat(&squares) + structure);
        score += 1e-5 * (dx * dx + dy * dy) as f64 / (ww * ww + hh * hh).max(1) as f64;
        if score < best_score {
            best_score = score;
            best = Some(((dx * w as i64) as f64 / ww as f64, (dy * h as i64) as f64 / hh as f64));
        }
    }
    Ok(best)
}
