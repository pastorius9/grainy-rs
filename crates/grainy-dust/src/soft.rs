//! Ports of luma/dust_soft.py.
use crate::{
    blur, checkpoint, eigen, invalid, num, plane, quantiles, rank, repeated_detail, round, Cancel, Options, Progress,
    Result,
};
use grainy_core::Plane;
use grainy_imgproc as cv;
use ndarray::s;
use serde_json::{json, Value};
use std::collections::HashMap;
/// Port of luma/dust_soft.py candidates.
pub fn candidates(
    gray: &Plane,
    options: &Options,
    cancel: Cancel<'_>,
    mut progress: Progress<'_>,
    tile_size: usize,
) -> Result<Vec<Value>> {
    plane(gray)?;
    options.validate()?;
    if tile_size < 1 {
        return Err(invalid("tile size"));
    }
    let (height, width) = gray.dim();
    let mut spots = Vec::new();
    let floor = 0.002 + 0.03 * (1. - options.sensitivity as f64 / 100.).powi(2);
    let factor = if options.suppress_grain {
        8. - 4. * options.sensitivity as f64 / 100.
    } else {
        5.5 - 3. * options.sensitivity as f64 / 100.
    };
    let scales: Vec<f64> = [1.4, 2.2, 3.5, 5.5, 8.5, 13.5, 21., 34.]
        .into_iter()
        .filter(|s| options.minimum as f64 <= 4. * s && 4. * s <= options.maximum as f64)
        .collect();
    let mut completed = 0;
    let total = height.div_ceil(tile_size) * width.div_ceil(tile_size);
    for top in (0..height).step_by(tile_size) {
        for left in (0..width).step_by(tile_size) {
            checkpoint(cancel)?;
            for &sigma in &scales {
                checkpoint(cancel)?;
                let diameter = 2 * (sigma.round_ties_even() as usize).max(1) + 1;
                let kernel = cv::get_structuring_element(cv::MorphShape::Ellipse, (diameter, diameter), None)?;
                let r = 2.2 * sigma + 1.;
                let pad = (r * 2.3).ceil() as usize;
                let halo = ((if options.reduce_patterns { 28. } else { 24. }) * sigma).ceil() as usize + 24;
                let y0 = top.saturating_sub(halo);
                let x0 = left.saturating_sub(halo);
                let part = gray
                    .slice(s![
                        y0..height.min(top.saturating_add(tile_size).saturating_add(halo)),
                        x0..width.min(left.saturating_add(tile_size).saturating_add(halo))
                    ])
                    .to_owned();
                let smooth = blur(&part, sigma)?;
                let difference = &smooth - &blur(&part, sigma * 1.6)?;
                let local_noise = blur(&difference.mapv(f32::abs), sigma * 4.)?.mapv(|v| v * 1.253_314);
                for kind in options.kinds() {
                    let response = if kind == "bright" { difference.clone() } else { difference.mapv(|v| -v) };
                    let dilated = cv::dilate(&response, &kernel, None, 1, None)?;
                    let peaks: Vec<_> = response
                        .indexed_iter()
                        .filter_map(|(p, &v)| {
                            (v > (floor as f32).max(local_noise[p] * factor as f32) && v >= dilated[p]).then_some(p)
                        })
                        .collect();
                    for (index, (y, x)) in peaks.into_iter().enumerate() {
                        if index % 128 == 0 {
                            checkpoint(cancel)?;
                        }
                        let gx = x + x0;
                        let gy = y + y0;
                        if gx < left
                            || gx >= width.min(left.saturating_add(tile_size))
                            || gy < top
                            || gy >= height.min(top.saturating_add(tile_size))
                        {
                            continue;
                        }
                        if gx.min(gy).min(width - 1 - gx).min(height - 1 - gy) < pad
                            || x.min(y).min(part.ncols() - 1 - x).min(part.nrows() - 1 - y) < pad
                        {
                            continue;
                        }
                        let step = ((sigma * 0.5).round_ties_even() as usize).max(1);
                        let peak = response[(y, x)] as f64;
                        // NumPy float32 scalars weakly promote Python floats, including 2*peak here.
                        let xx = (response[(y, x + step)] + response[(y, x - step)] - (2. * peak) as f32) as f64;
                        let yy = (response[(y + step, x)] + response[(y - step, x)] - (2. * peak) as f32) as f64;
                        let xy = ((response[(y + step, x + step)] - response[(y - step, x + step)])
                            - response[(y + step, x - step)]
                            + response[(y - step, x - step)]) as f64
                            / 4.;
                        let eig = eigen(xx, xy, yy);
                        if eig[1] >= 0. || eig[1] / eig[0] < 0.35 {
                            continue;
                        }
                        let mut ring = Vec::new();
                        for dy in -(pad as i64)..=pad as i64 {
                            for dx in -(pad as i64)..=pad as i64 {
                                let d = (dx * dx + dy * dy) as f64;
                                if d > (r * 1.3).powi(2) && d < (r * 2.2).powi(2) {
                                    ring.push(difference[((y as i64 + dy) as usize, (x as i64 + dx) as usize)].abs());
                                }
                            }
                        }
                        if peak < floor.max(quantiles(&ring, &[0.8])?[0] as f64 * 2.5) {
                            continue;
                        }
                        if options.reduce_patterns {
                            let size = r.ceil() as usize;
                            if repeated_detail(&smooth, [x - size, y - size, size * 2 + 1, size * 2 + 1], true)? {
                                continue;
                            }
                        }
                        spots.push(json!({"x":gx as f64/(width-1).max(1) as f64,"y":gy as f64/(height-1).max(1) as f64,"rx":r/width as f64,"ry":r/height as f64,"polarity":kind,"review":true,"detail_kind":"soft","diameter":round(4.*sigma,1),"contrast":round(peak,5),"score":round(peak/floor.max(local_noise[(y,x)] as f64),4)}));
                    }
                }
            }
            completed += 1;
            if let Some(p) = progress.as_deref_mut() {
                p(completed, total);
            }
        }
    }
    checkpoint(cancel)?;
    Ok(spots)
}
/// Port of luma/dust_soft.py merge. Prior candidates keep their original order and priority.
pub fn merge(
    existing: &[Value],
    additional: &[Value],
    width: usize,
    height: usize,
    maximum: usize,
    cancel: Cancel<'_>,
) -> Result<Vec<Value>> {
    crate::shape((height, width))?;
    if maximum == 0 {
        return Err(invalid("merge geometry"));
    }
    for s in existing.iter().chain(additional) {
        crate::spot_bounds(s, (height, width))?;
        crate::number(s, "score")?;
    }
    let cell = |s: &Value| {
        (
            (num(s, "x") * (width - 1) as f64 / maximum as f64).floor() as i64,
            (num(s, "y") * (height - 1) as f64 / maximum as f64).floor() as i64,
        )
    };
    let mut kept = existing.to_vec();
    let mut bins: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in existing.iter().enumerate() {
        bins.entry(cell(s)).or_default().push(i);
    }
    let mut ordered = additional.to_vec();
    ordered.sort_by(|a, b| rank(a, b).then_with(|| a["polarity"].as_str().cmp(&b["polarity"].as_str())));
    for spot in ordered {
        checkpoint(cancel)?;
        let x = num(&spot, "x") * (width - 1) as f64;
        let y = num(&spot, "y") * (height - 1) as f64;
        let (cx, cy) = cell(&spot);
        let mut overlap = false;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(ids) = bins.get(&(cx + dx, cy + dy)) {
                    for &i in ids {
                        let s = &kept[i];
                        if s["polarity"] == spot["polarity"]
                            && (x - num(s, "x") * (width - 1) as f64).powi(2)
                                + (y - num(s, "y") * (height - 1) as f64).powi(2)
                                <= 3f64.max(num(s, "rx").min(num(&spot, "rx")) * width as f64 * 0.5).powi(2)
                        {
                            overlap = true;
                        }
                    }
                }
            }
        }
        if !overlap {
            bins.entry((cx, cy)).or_default().push(kept.len());
            kept.push(spot);
        }
    }
    Ok(kept)
}
