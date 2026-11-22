//! Subject selection data lives in the same source coordinates as brushes. Preview and full
//! export sample the saved soft matte before applying lens, rotation and crop geometry.
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{Read, Write};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use grainy_core::Plane;
use serde_json::{json, Map, Value};

use crate::{Error, Result};

/// The native selection grid and U²-Net input/output use the same pixel centres.
pub const SIDE: usize = 320;
pub type Box2d = [f64; 4];
pub const FULL: Box2d = [0., 0., 1., 1.];

fn bounds_valid(b: Box2d) -> bool {
    b.iter().all(|v| (0.0..=1.0).contains(v)) && b[2] > b[0] && b[3] > b[1]
}

/// An edge-aware foreground estimate for installations without learned weights. Flood from all
/// four edges, tracking the smallest colour barrier that can connect each pixel to the border.
/// A gently varying background stays connected; an enclosed, contrasting object resists flooding.
pub fn foreground(rgb: &[[u8; 3]]) -> Result<Vec<u8>> {
    if rgb.len() != SIDE * SIDE {
        return Err(Error::Invalid("invalid selection image dimensions".into()));
    }
    let mut barrier = vec![u16::MAX; rgb.len()];
    let mut queue = BinaryHeap::new();
    for y in 0..SIDE {
        for x in 0..SIDE {
            if y == 0 || x == 0 || y + 1 == SIDE || x + 1 == SIDE {
                let i = y * SIDE + x;
                barrier[i] = 0;
                queue.push(Reverse((0u16, i)));
            }
        }
    }
    while let Some(Reverse((cost, i))) = queue.pop() {
        if cost != barrier[i] {
            continue;
        }
        let (y, x) = (i / SIDE, i % SIDE);
        let mut visit = |j: usize| {
            let a = rgb[i];
            let b = rgb[j];
            let delta = (0..3).map(|c| u16::from(a[c].abs_diff(b[c]))).sum::<u16>();
            let next = cost.max(delta);
            if next < barrier[j] {
                barrier[j] = next;
                queue.push(Reverse((next, j)));
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < SIDE {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - SIDE);
        }
        if y + 1 < SIDE {
            visit(i + SIDE);
        }
    }
    let mut counts = [0usize; 766];
    for &v in &barrier {
        counts[v as usize] += 1;
    }
    let quantile = |pct: usize| {
        let mut n = 0;
        counts
            .iter()
            .position(|v| {
                n += *v;
                n >= rgb.len() * pct / 100
            })
            .unwrap_or(765) as f32
    };
    let low = quantile(20);
    let high = quantile(98);
    if high <= low + 4. {
        return Err(Error::Invalid("No distinct foreground was found.".into()));
    }
    Ok(barrier
        .into_iter()
        .map(|v| {
            // Discard the weakest part of the barrier range (background texture).
            let t = ((f32::from(v) - low) / (high - low) * 1.8 - 0.55).clamp(0., 1.);
            (255.0 * t * t * (3. - 2. * t)).round() as u8
        })
        .collect())
}

/// Separable O(N) box mean over a `SIDE × SIDE` float plane with clamped borders.
fn box_mean(src: &[f32], radius: usize) -> Vec<f32> {
    let n = SIDE;
    let mut tmp = vec![0.0f32; n * n];
    let mut out = vec![0.0f32; n * n];
    let r = radius as isize;
    let win = (2 * radius + 1) as f32;
    for y in 0..n {
        let row = y * n;
        let mut acc = 0.0f32;
        for dx in -r..=r {
            let x = dx.clamp(0, (n - 1) as isize) as usize;
            acc += src[row + x];
        }
        tmp[row] = acc / win;
        for x in 1..n {
            let add_x = (x + radius).min(n - 1);
            let sub_x = x.saturating_sub(radius + 1);
            acc += src[row + add_x] - src[row + sub_x];
            tmp[row + x] = acc / win;
        }
    }
    for x in 0..n {
        let mut acc = 0.0f32;
        for dy in -r..=r {
            let y = dy.clamp(0, (n - 1) as isize) as usize;
            acc += tmp[y * n + x];
        }
        out[x] = acc / win;
        for y in 1..n {
            let add_y = (y + radius).min(n - 1);
            let sub_y = y.saturating_sub(radius + 1);
            acc += tmp[add_y * n + x] - tmp[sub_y * n + x];
            out[y * n + x] = acc / win;
        }
    }
    out
}

/// Edge-preserving 3-channel RGB Guided Filter (He et al.) matte refinement at `SIDE × SIDE`.
/// Aligns soft foreground/subject/skin matte transitions with fine luminance and colour edges
/// (hair, fur, foliage, jawlines) while keeping solid interior/exterior regions untouched.
pub fn refine_matte(rgb: &[[u8; 3]], matte: &[u8], radius: usize, eps: f32) -> Result<Vec<u8>> {
    let n = SIDE * SIDE;
    if rgb.len() != n || matte.len() != n {
        return Err(Error::Invalid("invalid selection matte dimensions".into()));
    }
    if radius == 0 {
        return Ok(matte.to_vec());
    }
    let mut r = vec![0.0f32; n];
    let mut g = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    let mut p = vec![0.0f32; n];
    for i in 0..n {
        r[i] = f32::from(rgb[i][0]) / 255.0;
        g[i] = f32::from(rgb[i][1]) / 255.0;
        b[i] = f32::from(rgb[i][2]) / 255.0;
        p[i] = f32::from(matte[i]) / 255.0;
    }
    let mr = box_mean(&r, radius);
    let mg = box_mean(&g, radius);
    let mb = box_mean(&b, radius);
    let mp = box_mean(&p, radius);

    let prod = |a: &[f32], c: &[f32]| -> Vec<f32> { a.iter().zip(c).map(|(x, y)| x * y).collect() };
    let rr = box_mean(&prod(&r, &r), radius);
    let rg = box_mean(&prod(&r, &g), radius);
    let rb = box_mean(&prod(&r, &b), radius);
    let gg = box_mean(&prod(&g, &g), radius);
    let gb = box_mean(&prod(&g, &b), radius);
    let bb = box_mean(&prod(&b, &b), radius);
    let rp = box_mean(&prod(&r, &p), radius);
    let gp = box_mean(&prod(&g, &p), radius);
    let bp = box_mean(&prod(&b, &p), radius);

    let eps = eps.max(1e-6);
    let mut ar = vec![0.0f32; n];
    let mut ag = vec![0.0f32; n];
    let mut ab = vec![0.0f32; n];
    let mut bv = vec![0.0f32; n];
    for i in 0..n {
        let a11 = rr[i] - mr[i] * mr[i] + eps;
        let a12 = rg[i] - mr[i] * mg[i];
        let a13 = rb[i] - mr[i] * mb[i];
        let a22 = gg[i] - mg[i] * mg[i] + eps;
        let a23 = gb[i] - mg[i] * mb[i];
        let a33 = bb[i] - mb[i] * mb[i] + eps;

        let i11 = a22 * a33 - a23 * a23;
        let i12 = a13 * a23 - a12 * a33;
        let i13 = a12 * a23 - a13 * a22;
        let i22 = a11 * a33 - a13 * a13;
        let i23 = a13 * a12 - a11 * a23;
        let i33 = a11 * a22 - a12 * a12;
        let det = (a11 * i11 + a12 * i12 + a13 * i13).max(1e-12);

        let cr = rp[i] - mr[i] * mp[i];
        let cg = gp[i] - mg[i] * mp[i];
        let cb = bp[i] - mb[i] * mp[i];

        ar[i] = (i11 * cr + i12 * cg + i13 * cb) / det;
        ag[i] = (i12 * cr + i22 * cg + i23 * cb) / det;
        ab[i] = (i13 * cr + i23 * cg + i33 * cb) / det;
        bv[i] = mp[i] - (ar[i] * mr[i] + ag[i] * mg[i] + ab[i] * mb[i]);
    }
    let mar = box_mean(&ar, radius);
    let mag = box_mean(&ag, radius);
    let mab = box_mean(&ab, radius);
    let mbv = box_mean(&bv, radius);
    Ok((0..n)
        .map(|i| {
            let q = (mar[i] * r[i] + mag[i] * g[i] + mab[i] * b[i] + mbv[i]).clamp(0.0, 1.0);
            (q * 255.0).round() as u8
        })
        .collect())
}

/// Adaptive portrait skin matte in YCbCr chromaticity space (covering light through deep skin tones),
/// optionally weighted by a foreground subject matte and refined along fine facial/hair boundaries.
pub fn portrait_skin(rgb: &[[u8; 3]], fg: Option<&[u8]>) -> Result<Vec<u8>> {
    let n = SIDE * SIDE;
    if rgb.len() != n || fg.is_some_and(|m| m.len() != n) {
        return Err(Error::Invalid("invalid selection image dimensions".into()));
    }
    let mut prob = vec![0.0f32; n];
    for (i, &[ru, gu, bu]) in rgb.iter().enumerate() {
        let r = f32::from(ru);
        let g = f32::from(gu);
        let b = f32::from(bu);
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let cb = 128.0 - 0.168736 * r - 0.331264 * g + 0.5 * b;
        let cr = 128.0 + 0.5 * r - 0.418688 * g - 0.081312 * b;

        // Soft distance outside the human skin Cb/Cr locus [77..127] x [133..174].
        let dcb = if cb < 77.0 {
            77.0 - cb
        } else if cb > 127.0 {
            cb - 127.0
        } else {
            0.0
        };
        let dcr = if cr < 133.0 {
            133.0 - cr
        } else if cr > 174.0 {
            cr - 174.0
        } else {
            0.0
        };
        let chroma = (-((dcb * dcb + dcr * dcr) / (2.0 * 7.5 * 7.5))).exp();
        // Luminance gate (excludes black shadows/hair while preserving deep brown skin tones).
        let luma = ((y - 22.0) / 24.0).clamp(0.0, 1.0) * ((252.0 - y) / 16.0).clamp(0.0, 1.0);
        // Warmth and saturation plausibility (excludes blue/green casts and neon red clothing).
        let warm = ((r - b - 4.0) / 18.0).clamp(0.0, 1.0) * ((r - g + 10.0) / 22.0).clamp(0.0, 1.0);
        let spread = r.max(g).max(b) - r.min(g).min(b);
        let sat = ((165.0 - spread) / 35.0).clamp(0.0, 1.0);
        prob[i] = (chroma * luma * warm * sat).clamp(0.0, 1.0);
    }
    if let Some(fg) = fg {
        let fg_skin: f32 = prob.iter().zip(fg).map(|(&s, &f)| s * f32::from(f) / 255.0).sum();
        let all_skin: f32 = prob.iter().sum();
        if all_skin > 1.0 && fg_skin / all_skin > 0.25 {
            for (s, &f) in prob.iter_mut().zip(fg) {
                let w = 0.15 + 0.85 * (f32::from(f) / 255.0);
                *s *= w;
            }
        }
    }
    let raw: Vec<u8> = prob.into_iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let refined = refine_matte(rgb, &raw, 4, 2e-3)?;
    let covered = refined.iter().filter(|&&v| v >= 64).count();
    if covered < n / 400 {
        return Err(Error::Invalid("No distinct portrait skin region was found.".into()));
    }
    Ok(refined)
}

/// Encode a soft matte only once. The model and source file are then unnecessary for playback.
pub fn component(kind: &str, bounds: Box2d, matte: &[u8]) -> Result<Map<String, Value>> {
    if !matches!(kind, "subject" | "background" | "object" | "portrait")
        || !bounds_valid(bounds)
        || matte.len() != SIDE * SIDE
    {
        return Err(Error::Invalid("invalid selection matte".into()));
    }
    let mut out = ZlibEncoder::new(Vec::new(), Compression::fast());
    out.write_all(matte).map_err(|e| Error::Invalid(e.to_string()))?;
    let bytes = out.finish().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(json!({"type":kind,"version":1,"bounds":bounds,"coverage":STANDARD.encode(bytes)}).as_object().unwrap().clone())
}

pub(crate) fn coverage(shape: (usize, usize), c: &Map<String, Value>) -> Result<Plane> {
    let bad = || Error::Invalid("invalid stored selection matte".into());
    if c.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(bad());
    }
    let b: Box2d = serde_json::from_value(c.get("bounds").cloned().ok_or_else(bad)?).map_err(|_| bad())?;
    if !bounds_valid(b) {
        return Err(bad());
    }
    let text = c.get("coverage").and_then(Value::as_str).ok_or_else(bad)?;
    if text.len() > (SIDE * SIDE + 1024) * 2 {
        return Err(bad());
    }
    let compressed = STANDARD.decode(text).map_err(|_| bad())?;
    let mut matte = Vec::with_capacity(SIDE * SIDE);
    ZlibDecoder::new(compressed.as_slice())
        .take((SIDE * SIDE + 1) as u64)
        .read_to_end(&mut matte)
        .map_err(|_| bad())?;
    if matte.len() != SIDE * SIDE {
        return Err(bad());
    }
    let invert = c.get("type").and_then(Value::as_str) == Some("background");
    let (h, w) = shape;
    let axis = |i: usize, n: usize, lo: f64, hi: f64| {
        let f = ((i as f64 + 0.5) / n as f64 - lo) / (hi - lo);
        if !(0.0..=1.0).contains(&f) {
            return None;
        }
        let p = (f * SIDE as f64 - 0.5).clamp(0., (SIDE - 1) as f64);
        Some((p as usize, (p as usize + 1).min(SIDE - 1), (p.fract()) as f32))
    };
    let xs: Vec<_> = (0..w).map(|x| axis(x, w, b[0], b[2])).collect();
    let ys: Vec<_> = (0..h).map(|y| axis(y, h, b[1], b[3])).collect();
    Ok(Plane::from_shape_fn(shape, |(y, x)| {
        let v = if let (Some((x0, x1, dx)), Some((y0, y1, dy))) = (xs[x], ys[y]) {
            let lerp = |r: usize| f32::from(matte[r * SIDE + x0]) * (1. - dx) + f32::from(matte[r * SIDE + x1]) * dx;
            (lerp(y0) * (1. - dy) + lerp(y1) * dy) / 255.
        } else {
            0.
        };
        if invert {
            1. - v
        } else {
            v
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_foreground_survives_resize_and_background_is_its_complement() {
        let img: Vec<_> = (0..SIDE * SIDE)
            .map(|i| {
                let x = i % SIDE;
                let y = i / SIDE;
                if x.abs_diff(SIDE / 2) + y.abs_diff(SIDE / 2) < 70 {
                    [200, 35, 70]
                } else {
                    [20, 100, 190]
                }
            })
            .collect();
        let mask = foreground(&img).unwrap();
        let subject = component("subject", FULL, &mask).unwrap();
        let encoded = serde_json::to_string(&subject).unwrap();
        let a = coverage((80, 100), &serde_json::from_str(&encoded).unwrap()).unwrap();
        let b = coverage((80, 100), &component("background", FULL, &mask).unwrap()).unwrap();
        assert!(a[[40, 50]] > 0.99 && a[[24, 29]] < 0.01);
        assert!(a.iter().zip(b.iter()).all(|(x, y)| (x + y - 1.).abs() < 1e-6));
        let obj = coverage((80, 100), &component("object", [0., 0., 0.5, 0.5], &mask).unwrap()).unwrap();
        assert!(obj[[20, 25]] > 0.99 && obj[[40, 50]] == 0.);
        assert!(foreground(&vec![[0; 3]; SIDE * SIDE]).is_err());

        // Guided matte refinement keeps crisp interior/exterior boundaries intact.
        let refined = refine_matte(&img, &mask, 4, 1e-3).unwrap();
        let ref_cov = coverage((80, 100), &component("subject", FULL, &refined).unwrap()).unwrap();
        assert!(ref_cov[[40, 50]] > 0.98 && ref_cov[[24, 29]] < 0.02);
    }

    #[test]
    fn portrait_skin_detects_skin_tones_and_rejects_non_skin() {
        let img: Vec<_> = (0..SIDE * SIDE)
            .map(|i| {
                let x = i % SIDE;
                let y = i / SIDE;
                let dx = x as isize - (SIDE / 2) as isize;
                let dy = y as isize - (SIDE / 2) as isize;
                if dx * dx + dy * dy < 60 * 60 {
                    [215, 165, 138] // natural portrait skin tone
                } else {
                    [40, 110, 175] // cool blue background
                }
            })
            .collect();
        let skin = portrait_skin(&img, None).unwrap();
        let cov = coverage((80, 80), &component("portrait", FULL, &skin).unwrap()).unwrap();
        assert!(cov[[40, 40]] > 0.9, "center skin coverage {}", cov[[40, 40]]);
        assert!(cov[[5, 5]] < 0.05, "background leakage {}", cov[[5, 5]]);
        // Pure blue frame has no portrait skin.
        assert!(portrait_skin(&vec![[30, 90, 180]; SIDE * SIDE], None).is_err());
    }
}
