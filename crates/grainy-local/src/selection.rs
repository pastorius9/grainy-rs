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

/// Encode a soft matte only once. The model and source file are then unnecessary for playback.
pub fn component(kind: &str, bounds: Box2d, matte: &[u8]) -> Result<Map<String, Value>> {
    if !matches!(kind, "subject" | "background" | "object") || !bounds_valid(bounds) || matte.len() != SIDE * SIDE {
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
    }
}
