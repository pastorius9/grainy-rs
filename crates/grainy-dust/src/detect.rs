use crate::{
    blur, checkpoint, component_indices, eigen, invalid, median, num, plane, quantiles, rank, repeated_detail, round,
    Cancel, Progress, Result,
};
use grainy_core::Plane;
use grainy_imgproc as cv;
use ndarray::{s, Array2};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Instant};

/// Port of luma/dust.py Options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    /// Port of Options.sensitivity.
    pub sensitivity: i32,
    /// Port of Options.minimum, in source pixels.
    pub minimum: i32,
    /// Port of Options.maximum, in source pixels.
    pub maximum: i32,
    /// Port of Options.polarity: both, dark or bright.
    pub polarity: String,
    /// Port of Options.detailed.
    pub detailed: bool,
    /// Port of Options.reduce_patterns.
    pub reduce_patterns: bool,
    /// Port of Options.soft.
    pub soft: bool,
    /// Port of Options.suppress_grain.
    pub suppress_grain: bool,
    /// Port of Options.scratches.
    pub scratches: bool,
    /// Port of Options.scratch_width.
    pub scratch_width: i32,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            sensitivity: 50,
            minimum: 3,
            maximum: 40,
            polarity: "both".into(),
            detailed: false,
            reduce_patterns: false,
            soft: false,
            suppress_grain: false,
            scratches: false,
            scratch_width: 12,
        }
    }
}
impl Options {
    /// Port of luma/dust.py Options.validate.
    pub fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.sensitivity)
            || !(2..=160).contains(&self.minimum)
            || self.maximum < self.minimum
            || self.maximum > 160
            || !["both", "dark", "bright"].contains(&self.polarity.as_str())
            || !(2..=40).contains(&self.scratch_width)
        {
            return Err(invalid("detector options"));
        }
        Ok(())
    }
    pub(crate) fn kinds(&self) -> Vec<&str> {
        if self.polarity == "both" {
            vec!["dark", "bright"]
        } else {
            vec![&self.polarity]
        }
    }
}
/// Port of luma/dust.py detect return dictionary. `seconds` is elapsed runtime, not golden data.
#[derive(Debug, Serialize, Deserialize)]
pub struct Detection {
    /// Port of detect.spots, in ranked order.
    pub spots: Vec<Value>,
    /// Port of detect.total (before limiting).
    pub total: usize,
    /// Port of detect.truncated.
    pub truncated: bool,
    /// Port of detect.width.
    pub width: usize,
    /// Port of detect.height.
    pub height: usize,
    /// Port of detect.seconds.
    pub seconds: f64,
}
fn ordinary_rank(a: &Value, b: &Value) -> std::cmp::Ordering {
    a["review"].as_bool().unwrap_or(false).cmp(&b["review"].as_bool().unwrap_or(false)).then_with(|| rank(a, b))
}

/// Port of luma/dust.py detect, including tiling, limits, cancellation and progress.
pub fn detect(
    gray: &Plane,
    options: &Options,
    cancel: Cancel<'_>,
    mut progress: Progress<'_>,
    tile_size: usize,
    limit: usize,
) -> Result<Detection> {
    options.validate()?;
    checkpoint(cancel)?;
    plane(gray)?;
    if tile_size < 32 || limit < 1 {
        return Err(invalid("detector bounds"));
    }
    let (height, width) = gray.dim();
    let started = Instant::now();
    let maximum = options.maximum as usize;
    let halo = maximum * 2 + 12;
    let mut sizes = vec![maximum + 3 + maximum % 2];
    for size in [9, 17] {
        if options.detailed && options.minimum as usize + 3 <= size && size < maximum + 3 {
            sizes.push(size);
        }
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    let threshold = 0.003 + 0.095 * (1. - options.sensitivity as f64 / 100.).powi(2);
    let noise_factor = 7. - 4. * options.sensitivity as f64 / 100.;
    let mut spots = Vec::new();
    let mut total = 0;
    let mut completed = 0;
    let mut accepted: HashMap<(i64, i64, String), Vec<[f64; 4]>> = HashMap::new();
    let tiles = height.div_ceil(tile_size) * width.div_ceil(tile_size);
    let soft = options.detailed && options.soft;
    let stages = 1 + usize::from(soft) + usize::from(options.scratches);
    for top in (0..height).step_by(tile_size) {
        for left in (0..width).step_by(tile_size) {
            checkpoint(cancel)?;
            let y0 = top.saturating_sub(halo);
            let x0 = left.saturating_sub(halo);
            let part = gray
                .slice(s![
                    y0..height.min(top.saturating_add(tile_size).saturating_add(halo)),
                    x0..width.min(left.saturating_add(tile_size).saturating_add(halo))
                ])
                .to_owned();
            let smooth = blur(&part, 0.65)?;
            let noise = median(&part.iter().zip(&smooth).map(|(a, b)| (a - b).abs()).collect::<Vec<_>>())? / 0.67449;
            let cutoff = threshold.max(noise * noise_factor);
            for kind in options.kinds() {
                checkpoint(cancel)?;
                let operation = if kind == "dark" { cv::Morphology::BlackHat } else { cv::Morphology::TopHat };
                for &size in &sizes {
                    let response = cv::morphology_ex(&smooth, operation, &Array2::ones((size, size)), None, 1, None)?;
                    for level in [cutoff, cutoff * 2.5, cutoff * 6.] {
                        let comp =
                            cv::connected_components_with_stats(&response.mapv(|v| u8::from(v > level as f32)), 8)?;
                        let eligible = component_indices(
                            &comp.stats,
                            options.minimum as f64,
                            maximum as f64,
                            (x0, y0),
                            [
                                left,
                                top,
                                width.min(left.saturating_add(tile_size)),
                                height.min(top.saturating_add(tile_size)),
                            ],
                        )?;
                        checkpoint(cancel)?;
                        for (ordinal, index) in eligible.into_iter().enumerate() {
                            if ordinal % 128 == 0 {
                                checkpoint(cancel)?;
                            }
                            let stat = comp.stats.row(index);
                            let (x, y, w, h, area) = (
                                stat[0] as usize,
                                stat[1] as usize,
                                stat[2] as usize,
                                stat[3] as usize,
                                stat[4] as usize,
                            );
                            let diameter = (4. * area as f64 / std::f64::consts::PI).sqrt();
                            let cx = x as f64 + (w - 1) as f64 / 2.;
                            let cy = y as f64 + (h - 1) as f64 / 2.;
                            let gx = cx + x0 as f64;
                            let gy = cy + y0 as f64;
                            let cell = (
                                (gx / maximum as f64).floor() as i64,
                                (gy / maximum as f64).floor() as i64,
                                kind.to_owned(),
                            );
                            let mut covered = false;
                            for dy in -1..=1 {
                                for dx in -1..=1 {
                                    if let Some(v) = accepted.get(&(cell.0 + dx, cell.1 + dy, kind.to_owned())) {
                                        if v.iter()
                                            .any(|p| ((gx - p[0]) / p[2]).powi(2) + ((gy - p[1]) / p[3]).powi(2) <= 1.)
                                        {
                                            covered = true;
                                        }
                                    }
                                }
                            }
                            if covered {
                                continue;
                            }
                            let points: Vec<_> = (0..h)
                                .flat_map(|yy| (0..w).map(move |xx| (xx, yy)))
                                .filter(|&(xx, yy)| comp.labels[(y + yy, x + xx)] == index as i32)
                                .collect();
                            let mut rx = 1f64.max((w - 1) as f64 / 2.);
                            let mut ry = 1f64.max((h - 1) as f64 / 2.);
                            let fit = points
                                .iter()
                                .map(|&(xx, yy)| {
                                    (((xx + x) as f64 - cx) / rx).powi(2) + (((yy + y) as f64 - cy) / ry).powi(2)
                                })
                                .map(f64::sqrt)
                                .fold(1., f64::max);
                            rx = rx * fit + 1.5;
                            ry = ry * fit + 1.5;
                            let pad = (rx.max(ry) * 2.4).ceil() as usize + 2;
                            if gx.min(gy).min((width - 1) as f64 - gx).min((height - 1) as f64 - gy) < pad as f64 {
                                continue;
                            }
                            let iy = cy.round_ties_even() as usize;
                            let ix = cx.round_ties_even() as usize;
                            if iy < pad || ix < pad || iy + pad >= part.nrows() || ix + pad >= part.ncols() {
                                continue;
                            }
                            let mut surrounding = Vec::new();
                            let mut response_ring = Vec::new();
                            let mut samples: Vec<Vec<f32>> = vec![Vec::new(); 8];
                            for yy in iy - pad..=iy + pad {
                                for xx in ix - pad..=ix + pad {
                                    let dx = xx as f64 - cx;
                                    let dy = yy as f64 - cy;
                                    let distance = (dx / rx).powi(2) + (dy / ry).powi(2);
                                    if (1.5f64.powi(2)..=2.3f64.powi(2)).contains(&distance) {
                                        surrounding.push(part[(yy, xx)]);
                                    }
                                    if (1.3f64.powi(2)..=2.3f64.powi(2)).contains(&distance) {
                                        response_ring.push(response[(yy, xx)]);
                                    }
                                    if (1.1f64.powi(2)..=1.7f64.powi(2)).contains(&distance) {
                                        let sector = (((dy.atan2(dx) + std::f64::consts::PI) * 4.
                                            / std::f64::consts::PI)
                                            as usize)
                                            % 8;
                                        samples[sector].push(part[(yy, xx)]);
                                    }
                                }
                            }
                            if surrounding.len() < 8 {
                                continue;
                            }
                            let center =
                                median(&points.iter().map(|&(xx, yy)| smooth[(y + yy, x + xx)]).collect::<Vec<_>>())?;
                            let background = median(&surrounding)?;
                            let mut contrast = if kind == "dark" { background - center } else { center - background };
                            let qs = quantiles(&surrounding, &[0.1, 0.9])?;
                            let mut spread = (qs[1] - qs[0]) as f64;
                            let quiet =
                                contrast >= cutoff && spread <= contrast * (0.25 + options.sensitivity as f64 * 0.006);
                            let review = !quiet || size != sizes[0];
                            if review && !options.detailed {
                                continue;
                            }
                            let mut core_response = 0.;
                            let mut background_response = 0.;
                            if review {
                                background_response = quantiles(&response_ring, &[0.6])?[0] as f64;
                                core_response = median(
                                    &points.iter().map(|&(xx, yy)| response[(y + yy, x + xx)]).collect::<Vec<_>>(),
                                )?;
                                if core_response < cutoff.max(background_response * 2.5) {
                                    continue;
                                }
                            }
                            if !quiet {
                                if !options.detailed
                                    || size > 17
                                    || area as f64 / ((w * h) as f64) < 0.55
                                    || w.max(h) as f64 / w.min(h) as f64 > 1.8
                                {
                                    continue;
                                }
                                let n = points.len() as f64;
                                let mx = points.iter().map(|p| p.0 as f64).sum::<f64>() / n;
                                let my = points.iter().map(|p| p.1 as f64).sum::<f64>() / n;
                                let a = points.iter().map(|p| (p.0 as f64 - mx).powi(2)).sum::<f64>() / (n - 1.);
                                let d = points.iter().map(|p| (p.1 as f64 - my).powi(2)).sum::<f64>() / (n - 1.);
                                let b = points.iter().map(|p| (p.0 as f64 - mx) * (p.1 as f64 - my)).sum::<f64>()
                                    / (n - 1.);
                                let eig = eigen(a, b, d);
                                if eig[0] < eig[1] * 0.45 {
                                    continue;
                                }
                                let mut arc = [false; 8];
                                for sector in 0..8 {
                                    let values = &samples[sector];
                                    if values.len() < 3 {
                                        continue;
                                    }
                                    let mut difference = median(values)? - center;
                                    if kind == "bright" {
                                        difference = -difference;
                                    }
                                    let q = quantiles(values, &[0.15, 0.85])?;
                                    let deviation = (q[1] - q[0]) as f64;
                                    arc[sector] =
                                        difference > (cutoff * 1.6).max(0.045) && deviation < difference * 0.65;
                                }
                                let starts: Vec<_> = (0..8).filter(|i| (0..3).all(|j| arc[(i + j) % 8])).collect();
                                if starts.is_empty() {
                                    continue;
                                }
                                contrast = f64::NEG_INFINITY;
                                for start in starts {
                                    let values: Vec<_> =
                                        (0..3).flat_map(|j| samples[(start + j) % 8].iter().copied()).collect();
                                    let bg = median(&values)?;
                                    contrast = contrast.max(if kind == "dark" { bg - center } else { center - bg });
                                }
                                spread = spread.max(contrast);
                            }
                            if review && repeated_detail(&smooth, [x, y, w, h], options.reduce_patterns)? {
                                continue;
                            }
                            let score = if review {
                                core_response / 0.003f64.max(noise).max(background_response)
                            } else {
                                contrast / 0.003f64.max(noise + spread * 0.5)
                            };
                            if !review {
                                accepted.entry(cell).or_default().push([gx, gy, rx, ry]);
                            }
                            spots.push(json!({"x":gx/(width-1).max(1) as f64,"y":gy/(height-1).max(1) as f64,"rx":rx/width as f64,"ry":ry/height as f64,"polarity":kind,"review":review,"score":round(score,4),"diameter":round(diameter,1),"contrast":round(contrast,5)}));
                            total += 1;
                        }
                    }
                    if !options.detailed && spots.len() > limit.saturating_mul(2) {
                        spots.sort_by(ordinary_rank);
                        spots.truncate(limit);
                    }
                }
            }
            completed += 1;
            if let Some(p) = progress.as_deref_mut() {
                p(completed, tiles * stages);
            }
        }
    }
    checkpoint(cancel)?;
    spots.sort_by(ordinary_rank);
    if options.detailed {
        let mut kept: Vec<Value> = Vec::new();
        let mut bins: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for spot in spots {
            checkpoint(cancel)?;
            let cell = (
                (num(&spot, "x") * width as f64 / maximum as f64).floor() as i64,
                (num(&spot, "y") * height as f64 / maximum as f64).floor() as i64,
            );
            let mut overlap = false;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(ids) = bins.get(&(cell.0 + dx, cell.1 + dy)) {
                        for &id in ids {
                            let s = &kept[id];
                            if s["polarity"] == spot["polarity"]
                                && ((num(&spot, "x") - num(s, "x")) / num(s, "rx")).powi(2)
                                    + ((num(&spot, "y") - num(s, "y")) / num(s, "ry")).powi(2)
                                    <= 1.
                            {
                                overlap = true;
                            }
                        }
                    }
                }
            }
            if !overlap {
                bins.entry(cell).or_default().push(kept.len());
                kept.push(spot);
            }
        }
        if soft {
            let mut p = |done: usize, _: usize| {
                if let Some(f) = progress.as_deref_mut() {
                    f(tiles + done, tiles * stages);
                }
            };
            let extra = crate::soft::candidates(gray, options, cancel, Some(&mut p), tile_size)?;
            kept = crate::soft::merge(&kept, &extra, width, height, maximum, cancel)?;
        }
        total = kept.len();
        spots = kept;
        spots.truncate(limit);
    } else {
        spots.truncate(limit);
    }
    if options.scratches {
        let mut p = |done: usize, _: usize| {
            if let Some(f) = progress.as_deref_mut() {
                f(tiles * (1 + usize::from(soft)) + done, tiles * stages);
            }
        };
        let extra = crate::scratches::candidates(gray, options, cancel, Some(&mut p), tile_size)?;
        total += extra.len();
        spots.truncate(limit.saturating_sub(extra.len()));
        spots.extend(extra.into_iter().take(limit));
    }
    Ok(Detection { spots, total, truncated: total > limit, width, height, seconds: started.elapsed().as_secs_f64() })
}
