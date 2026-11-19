use crate::{checkpoint, invalid, shapes, Cancel, Result};
use grainy_core::Image;
use grainy_imgproc as cv;
use ndarray::{s, Array2};
use rayon::prelude::*;
use serde_json::Value;

struct Group {
    bounds: [usize; 4],
    parts: Vec<shapes::Ellipse>,
}
fn overlaps(a: [usize; 4], b: [usize; 4], margin: usize) -> bool {
    a[0] <= b[2].saturating_add(margin)
        && b[0] <= a[2].saturating_add(margin)
        && a[1] <= b[3].saturating_add(margin)
        && b[1] <= a[3].saturating_add(margin)
}
pub(crate) fn image(a: &Image) -> Result<()> {
    let (h, w, c) = a.dim();
    if h == 0 || w == 0 || c != 3 || a.iter().any(|v| !v.is_finite()) {
        return Err(invalid("expected finite HxWx3 image"));
    }
    Ok(())
}
/// Port of luma/dust.py repair(copy=True).
pub fn repair(source: &Image, spots: &[Value], cancel: Cancel<'_>, method: &str) -> Result<Image> {
    let mut result = source.clone();
    repair_in_place(&mut result, spots, cancel, method)?;
    Ok(result)
}
/// Port of luma/dust.py repair(copy=False). Parallel waves preserve the sequential read/write dependencies.
pub fn repair_in_place(result: &mut Image, spots: &[Value], cancel: Cancel<'_>, method: &str) -> Result<()> {
    if !["smooth", "texture"].contains(&method) {
        return Err(invalid("repair method"));
    }
    checkpoint(cancel)?;
    image(result)?;
    if spots.is_empty() {
        return Ok(());
    }
    let original = if method == "texture" { Some(result.clone()) } else { None };
    let (h, w, _) = result.dim();
    let mut regions = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for spot in spots {
        checkpoint(cancel)?;
        let parts = shapes::ellipses(spot, (h, w))?;
        regions.push(parts.clone());
        let mut region = Group { bounds: shapes::bounds(&parts, (h, w))?, parts };
        checkpoint(cancel)?;
        let mut i = 0;
        while i < groups.len() {
            if overlaps(region.bounds, groups[i].bounds, 0) {
                let other = groups.remove(i);
                region.bounds = [
                    region.bounds[0].min(other.bounds[0]),
                    region.bounds[1].min(other.bounds[1]),
                    region.bounds[2].max(other.bounds[2]),
                    region.bounds[3].max(other.bounds[3]),
                ];
                region.parts.extend(other.parts);
                i = 0;
            } else {
                i += 1;
            }
        }
        groups.push(region);
    }
    let padded: Vec<_> = groups
        .iter()
        .map(|g| {
            let pad = (g.parts.iter().map(|p| p[2].max(p[3])).fold(0., f64::max) * 1.5).ceil() as usize;
            let pad = pad.max(8);
            [
                g.bounds[0].saturating_sub(pad),
                g.bounds[1].saturating_sub(pad),
                w.min(g.bounds[2].saturating_add(pad)),
                h.min(g.bounds[3].saturating_add(pad)),
            ]
        })
        .collect();
    let mut wave = vec![0; groups.len()];
    for j in 0..groups.len() {
        for i in 0..j {
            if overlaps(padded[j], groups[i].bounds, 1) || overlaps(padded[i], groups[j].bounds, 1) {
                wave[j] = wave[j].max(wave[i] + 1);
            }
        }
    }
    for level in 0..=wave.iter().copied().max().unwrap_or(0) {
        checkpoint(cancel)?;
        let indices: Vec<_> = (0..groups.len()).filter(|&i| wave[i] == level).collect();
        let source: &Image = result;
        let patches: Vec<Result<_>> = indices
            .par_iter()
            .map(|&index| {
                checkpoint(cancel)?;
                let [x0, y0, x1, y1] = padded[index];
                let mut patch = source.slice(s![y0..y1, x0..x1, ..]).to_owned();
                let mut mask = Array2::zeros((y1 - y0, x1 - x0));
                shapes::paint(&mut mask, &groups[index].parts, x0, y0)?;
                if mask.iter().any(|v| *v != 0) && mask.iter().any(|v| *v == 0) {
                    for c in 0..3 {
                        let fixed = cv::inpaint(&patch.slice(s![.., .., c]).to_owned(), &mask, 3.)?;
                        for ((y, x), &v) in fixed.indexed_iter() {
                            if mask[(y, x)] > 0 {
                                patch[(y, x, c)] = v.max(0.);
                            }
                        }
                    }
                }
                Ok((index, patch, mask))
            })
            .collect();
        for item in patches {
            let (index, patch, mask) = item?;
            let [x0, y0, _, _] = padded[index];
            for ((y, x), &v) in mask.indexed_iter() {
                if v > 0 {
                    for c in 0..3 {
                        result[(y0 + y, x0 + x, c)] = patch[(y, x, c)];
                    }
                }
            }
        }
    }
    if let Some(original) = original {
        let mut selected = Array2::zeros((h, w));
        for parts in regions {
            shapes::paint(&mut selected, &parts, 0, 0)?;
        }
        crate::texture::restore(&original, result, &selected, cancel)?;
    }
    checkpoint(cancel)?;
    Ok(())
}
