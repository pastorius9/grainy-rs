//! Ports of luma/dust_scratches.py.
use crate::{
    blur, checkpoint, eigen, invalid, median, plane, quantiles, rank, round, shapes, Cancel, Options, Progress, Result,
};
use grainy_core::Plane;
use grainy_imgproc as cv;
use ndarray::{s, Array2};
use serde_json::{json, Value};

/// Port of luma/dust_scratches.py cover; None means the footprint exceeded MAX_PARTS.
pub fn cover(
    component: &Array2<u8>,
    distance: &Plane,
    x0: usize,
    y0: usize,
    cancel: Cancel<'_>,
) -> Result<Option<Vec<shapes::Ellipse>>> {
    plane(distance)?;
    if component.dim() != distance.dim() || distance.iter().any(|v| *v < 0.) {
        return Err(invalid("scratch component/distance"));
    }
    let (height, width) = component.dim();
    let mut active: Vec<_> = component.indexed_iter().filter_map(|(p, &v)| (v != 0).then_some(p)).collect();
    active.sort_by(|a, b| distance[*b].total_cmp(&distance[*a]));
    let mut remaining = component.mapv(|v| v != 0);
    let mut parts = Vec::new();
    for (index, (y, x)) in active.into_iter().enumerate() {
        if index % 256 == 0 {
            checkpoint(cancel)?;
        }
        if !remaining[(y, x)] {
            continue;
        }
        let radius = distance[(y, x)] as f64 + 1.25;
        parts.push([x as f64 + x0 as f64, y as f64 + y0 as f64, radius, radius]);
        if parts.len() > shapes::MAX_PARTS {
            return Ok(None);
        }
        let pad = (radius.ceil() as usize).min(width.max(height));
        let left = x.saturating_sub(pad);
        let top = y.saturating_sub(pad);
        let right = width.min(x.saturating_add(pad).saturating_add(1));
        let bottom = height.min(y.saturating_add(pad).saturating_add(1));
        for yy in top..bottom {
            for xx in left..right {
                if (xx as f64 - x as f64).powi(2) + (yy as f64 - y as f64).powi(2) <= radius * radius {
                    remaining[(yy, xx)] = false;
                }
            }
        }
    }
    Ok(Some(parts))
}
fn distance(mask: &Array2<u8>) -> Result<Plane> {
    let (h, w) = mask.dim();
    let mut padded = Array2::zeros((h + 2, w + 2));
    padded.slice_mut(s![1..h + 1, 1..w + 1]).assign(mask);
    Ok(cv::distance_transform(&padded, 5)?.slice(s![1..h + 1, 1..w + 1]).to_owned())
}
fn ellipse(d: usize) -> Result<Array2<u8>> {
    Ok(cv::get_structuring_element(cv::MorphShape::Ellipse, (d, d), None)?)
}
/// Port of luma/dust_scratches.py candidates.
pub fn candidates(
    gray: &Plane,
    options: &Options,
    cancel: Cancel<'_>,
    mut progress: Progress<'_>,
    tile_size: usize,
) -> Result<Vec<Value>> {
    plane(gray)?;
    options.validate()?;
    if tile_size == 0 {
        return Err(invalid("tile size"));
    }
    let (height, width) = gray.dim();
    let maximum = options.scratch_width as usize;
    let kinds = options.kinds();
    let levels = [1f32, 2.5];
    let mut masks: Vec<_> = kinds
        .iter()
        .flat_map(|kind| levels.into_iter().map(move |level| (*kind, level, Array2::<u8>::zeros((height, width)))))
        .collect();
    let kernel = ellipse(maximum * 2 + 1)?;
    let halo = maximum * 2 + 32;
    let tiles = height.div_ceil(tile_size) * width.div_ceil(tile_size);
    let mut done = 0;
    let floor = 0.004 + 0.08 * (1. - options.sensitivity as f64 / 100.).powi(2);
    let noise_factor = 6. - 3. * options.sensitivity as f64 / 100.;
    for top in (0..height).step_by(tile_size) {
        for left in (0..width).step_by(tile_size) {
            checkpoint(cancel)?;
            let y0 = top.saturating_sub(halo);
            let x0 = left.saturating_sub(halo);
            let bottom = height.min(top.saturating_add(tile_size));
            let right = width.min(left.saturating_add(tile_size));
            let part = gray
                .slice(s![y0..height.min(bottom.saturating_add(halo)), x0..width.min(right.saturating_add(halo))])
                .to_owned();
            let smooth = blur(&part, 0.65)?;
            let residual = (&part - &smooth).mapv(f32::abs);
            let noise = blur(&residual, 3.)?.mapv(|v| v * 1.253_314);
            let threshold = noise.mapv(|v| (floor as f32).max(v * noise_factor as f32));
            for kind in &kinds {
                checkpoint(cancel)?;
                let response = cv::morphology_ex(
                    &smooth,
                    if *kind == "dark" { cv::Morphology::BlackHat } else { cv::Morphology::TopHat },
                    &kernel,
                    None,
                    1,
                    None,
                )?;
                for (k, level, mask) in &mut masks {
                    if k != kind {
                        continue;
                    }
                    for y in top..bottom {
                        for x in left..right {
                            mask[(y, x)] = u8::from(response[(y - y0, x - x0)] > threshold[(y - y0, x - x0)] * *level);
                        }
                    }
                }
            }
            done += 1;
            if let Some(p) = progress.as_deref_mut() {
                p(done.min(tiles.saturating_sub(1)), tiles);
            }
        }
    }
    let mut spots = Vec::new();
    let minimum_length = 24.max(maximum * 3);
    let mut accepted: Vec<_> = kinds.iter().map(|_| Array2::<u8>::zeros((height, width))).collect();
    for (kind, _level, mask) in masks {
        checkpoint(cancel)?;
        let contours = cv::find_contours(&mask, cv::Retrieval::External)?;
        for (ordinal, contour) in contours.contours.iter().enumerate() {
            if ordinal % 64 == 0 {
                checkpoint(cancel)?;
            }
            let rect = cv::bounding_rect(contour)?;
            let (x, y, w, h) = (rect[0] as usize, rect[1] as usize, rect[2] as usize, rect[3] as usize);
            if w.max(h) < minimum_length || w * h > 4_000_000 {
                continue;
            }
            let perimeter = cv::arc_length(contour, true)?;
            if perimeter < (minimum_length * 2) as f64 {
                continue;
            }
            let local_contour = contour.iter().map(|p| [p[0] - x as i32, p[1] - y as i32]).collect();
            let polygon = cv::draw_contours(&Array2::zeros((h, w)), &[local_contour], 1, -1)?;
            let component = Array2::from_shape_fn((h, w), |(yy, xx)| mask[(y + yy, x + xx)] & polygon[(yy, xx)]);
            let area = component.iter().map(|v| *v as usize).sum::<usize>();
            if area < minimum_length || perimeter * perimeter < 4. * std::f64::consts::PI * area as f64 * 3. {
                continue;
            }
            let moments = cv::moments(&component, true)?;
            let eig = eigen(moments.mu20 / area as f64, moments.mu11 / area as f64, moments.mu02 / area as f64);
            if eig[1] < eig[0] * 4. {
                let outline_area = cv::contour_area(contour, false)?;
                let hull_area = cv::contour_area(&cv::convex_hull(contour, false)?, false)?;
                if outline_area > 1f64.max(hull_area) * 0.25 {
                    let holes = cv::find_contours(&component, cv::Retrieval::CComp)?;
                    let mut largest_hole = 0f64;
                    for (c, entry) in holes.contours.iter().zip(&holes.hierarchy) {
                        if entry[3] >= 0 {
                            largest_hole = largest_hole.max(cv::contour_area(c, false)?);
                        }
                    }
                    if largest_hole < outline_area * 0.45 {
                        continue;
                    }
                }
            }
            let distances = distance(&component)?;
            let values: Vec<_> =
                component.indexed_iter().filter_map(|(p, &v)| (v > 0).then_some(distances[p])).collect();
            let thickness = 2. * quantiles(&values, &[0.85])?[0] as f64;
            if thickness > maximum as f64 + 1.
                || distances.iter().copied().fold(0., f32::max) as f64 > maximum as f64 * 0.65 + 1.
                || area as f64 / thickness.max(1.) < minimum_length as f64
            {
                continue;
            }
            let pad = maximum * 2 + 4;
            let left = x.saturating_sub(pad);
            let top = y.saturating_sub(pad);
            let right = width.min(x + w + pad);
            let bottom = height.min(y + h + pad);
            let mut local = Array2::zeros((bottom - top, right - left));
            local.slice_mut(s![y - top..y - top + h, x - left..x - left + w]).assign(&component);
            let outer = cv::dilate(&local, &ellipse(maximum * 3 + 1)?, None, 1, None)?;
            let inner = cv::dilate(&local, &ellipse(maximum + 1)?, None, 1, None)?;
            let surrounding: Vec<_> = outer
                .indexed_iter()
                .filter_map(|((yy, xx), &v)| (v > 0 && inner[(yy, xx)] == 0).then_some(gray[(top + yy, left + xx)]))
                .collect();
            if surrounding.len() < 8 {
                continue;
            }
            let center = median(
                &component
                    .indexed_iter()
                    .filter_map(|((yy, xx), &v)| (v > 0).then_some(gray[(y + yy, x + xx)]))
                    .collect::<Vec<_>>(),
            )?;
            let background = median(&surrounding)?;
            let contrast = if kind == "dark" { background - center } else { center - background };
            let q = quantiles(&surrounding, &[0.1, 0.9])?;
            let spread = (q[1] - q[0]) as f64;
            if contrast < floor || spread > contrast * 2. {
                continue;
            }
            if options.reduce_patterns {
                let boundary =
                    if kind == "dark" { background - contrast * 0.5 } else { background + contrast * 0.5 } as f32;
                let support = gray
                    .slice(s![top..bottom, left..right])
                    .mapv(|v| u8::from(if kind == "dark" { v < boundary } else { v > boundary }));
                let (_, labels) = cv::connected_components(&support, 8)?;
                let touching: std::collections::HashSet<_> =
                    local.indexed_iter().filter_map(|(p, &v)| (v > 0 && labels[p] > 0).then_some(labels[p])).collect();
                if !touching.is_empty() {
                    let sd = distance(&support)?;
                    let max = labels
                        .indexed_iter()
                        .filter_map(|(p, v)| touching.contains(v).then_some(sd[p]))
                        .fold(0., f32::max);
                    if max as f64 > maximum as f64 * 0.65 + 1. {
                        continue;
                    }
                }
            }
            let Some(mut parts) = cover(&component, &distances, x, y, cancel)? else {
                continue;
            };
            if parts.is_empty() {
                continue;
            }
            let ki = kinds.iter().position(|k| *k == kind).ok_or_else(|| invalid("polarity"))?;
            let covered = parts
                .iter()
                .filter(|p| accepted[ki][(p[1].round_ties_even() as usize, p[0].round_ties_even() as usize)] > 0)
                .count();
            if covered as f64 / parts.len() as f64 >= 0.95 {
                continue;
            }
            for p in &mut parts {
                p[2] = p[2].min(width as f64 / 2.);
                p[3] = p[3].min(height as f64 / 2.);
            }
            let metadata = json!({"polarity":kind,"review":true,"detail_kind":"scratch","diameter":round(thickness,1),"length":round(area as f64/thickness.max(1.),1),"contrast":round(contrast,5),"score":round(contrast/0.003f64.max(spread*0.5),4)});
            spots.push(shapes::make_spot(
                &parts,
                gray.dim(),
                metadata.as_object().cloned().ok_or_else(|| invalid("metadata"))?,
            )?);
            shapes::paint(&mut accepted[ki], &parts, 0, 0)?;
        }
    }
    checkpoint(cancel)?;
    if let Some(p) = progress {
        p(tiles, tiles);
    }
    spots.sort_by(|a, b| rank(a, b).then_with(|| a["polarity"].as_str().cmp(&b["polarity"].as_str())));
    Ok(spots)
}
