//! Ports of luma/dust_shapes.py.
use crate::{invalid, spot_bounds, Result};
use ndarray::Array2;
use serde_json::{json, Map, Value};
/// Port of luma/dust_shapes.py MAX_PARTS.
pub const MAX_PARTS: usize = 2048;
/// Pixel-space ellipse used by luma/dust_shapes.py ellipses.
pub type Ellipse = [f64; 4];
/// Port of luma/dust_shapes.py ellipses.
pub fn ellipses(spot: &Value, shape: (usize, usize)) -> Result<Vec<Ellipse>> {
    let parent = spot_bounds(spot, shape)?;
    let Some(parts) = spot.get("parts") else {
        return Ok(vec![parent]);
    };
    let parts =
        parts.as_array().filter(|a| !a.is_empty() && a.len() <= MAX_PARTS).ok_or_else(|| invalid("parts count"))?;
    parts
        .iter()
        .map(|part| {
            let mut p = part
                .as_object()
                .filter(|p| p.len() == 4 && ["x", "y", "rx", "ry"].iter().all(|k| p.contains_key(*k)))
                .ok_or_else(|| invalid("part keys"))?
                .clone();
            p.insert("scale".into(), spot.get("scale").cloned().unwrap_or(json!(100)));
            spot_bounds(&Value::Object(p), shape)
        })
        .collect()
}
fn check(parts: &[Ellipse], shape: (usize, usize)) -> Result<()> {
    crate::shape(shape)?;
    if parts.is_empty()
        || shape.0 == 0
        || shape.1 == 0
        || parts.iter().any(|p| {
            p.iter().any(|v| !v.is_finite()) || p[2] <= 0. || p[3] <= 0. || p.iter().any(|v| v.abs() > i32::MAX as f64)
        })
    {
        return Err(invalid("ellipse geometry"));
    }
    Ok(())
}
/// Port of luma/dust_shapes.py bounds; exclusive right/bottom pixel coordinates.
pub fn bounds(parts: &[Ellipse], shape: (usize, usize)) -> Result<[usize; 4]> {
    check(parts, shape)?;
    let (h, w) = shape;
    let left = parts.iter().map(|p| p[0] - p[2]).fold(f64::INFINITY, f64::min).floor() - 1.;
    let top = parts.iter().map(|p| p[1] - p[3]).fold(f64::INFINITY, f64::min).floor() - 1.;
    let right = parts.iter().map(|p| p[0] + p[2]).fold(f64::NEG_INFINITY, f64::max).ceil() + 2.;
    let bottom = parts.iter().map(|p| p[1] + p[3]).fold(f64::NEG_INFINITY, f64::max).ceil() + 2.;
    Ok([
        left.max(0.) as usize,
        top.max(0.) as usize,
        right.min(w as f64).max(0.) as usize,
        bottom.min(h as f64).max(0.) as usize,
    ])
}
/// Port of luma/dust_shapes.py paint. Existing mask values outside the ellipses are retained.
pub fn paint(mask: &mut Array2<u8>, parts: &[Ellipse], x0: usize, y0: usize) -> Result<()> {
    if parts.is_empty() {
        return Ok(());
    }
    check(parts, mask.dim())?;
    let (h, w) = mask.dim();
    if x0.checked_add(w).is_none() || y0.checked_add(h).is_none() {
        return Err(invalid("mask origin"));
    }
    for &[x, y, rx, ry] in parts {
        let left = (x - rx).floor().max(x0 as f64) as usize;
        let top = (y - ry).floor().max(y0 as f64) as usize;
        let right = ((x + rx).ceil() + 1.).min((x0 + w) as f64).max(0.) as usize;
        let bottom = ((y + ry).ceil() + 1.).min((y0 + h) as f64).max(0.) as usize;
        for yy in top..bottom {
            for xx in left..right {
                if ((xx as f64 - x) / rx).powi(2) + ((yy as f64 - y) / ry).powi(2) <= 1. {
                    mask[(yy - y0, xx - x0)] = 255;
                }
            }
        }
    }
    Ok(())
}
/// Port of luma/dust_shapes.py make_spot.
pub fn make_spot(parts: &[Ellipse], shape: (usize, usize), metadata: Map<String, Value>) -> Result<Value> {
    if parts.len() > MAX_PARTS {
        return Err(invalid("too many parts"));
    }
    let (h, w) = shape;
    let [left, top, right, bottom] = bounds(parts, shape)?;
    let cx = (left as f64 + right as f64 - 1.) / 2.;
    let cy = (top as f64 + bottom as f64 - 1.) / 2.;
    let mut result = json!({"x":cx/w.saturating_sub(1).max(1) as f64,"y":cy/h.saturating_sub(1).max(1) as f64,
        "rx":(0.75f64.max((right as f64-left as f64)/2.)/w as f64).min(0.5),"ry":(0.75f64.max((bottom as f64-top as f64)/2.)/h as f64).min(0.5),
        "parts":parts.iter().map(|p|json!({"x":p[0]/w.saturating_sub(1).max(1) as f64,"y":p[1]/h.saturating_sub(1).max(1) as f64,"rx":p[2]/w as f64,"ry":p[3]/h as f64})).collect::<Vec<_>>()});
    for (k, v) in metadata {
        if result.get(&k).is_some() {
            return Err(invalid("duplicate geometry metadata"));
        }
        result[&k] = v;
    }
    ellipses(&result, shape)?;
    Ok(result)
}
/// Port of luma/dust_shapes.py manual; points are normalized x,y pairs.
pub fn manual(points: &[[f64; 2]], shape: (usize, usize), diameter: f64) -> Result<Value> {
    crate::shape(shape)?;
    let (h, w) = shape;
    let radius = (diameter / 2.).min(h.min(w) as f64 / 2.);
    if !diameter.is_finite()
        || !(0.5..=80.).contains(&radius)
        || points.is_empty()
        || points.len() > 8192
        || points.iter().flatten().any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
    {
        return Err(invalid("manual path or diameter"));
    }
    let pixels: Vec<_> = points
        .iter()
        .map(|p| [p[0] * w.saturating_sub(1).max(1) as f64, p[1] * h.saturating_sub(1).max(1) as f64])
        .collect();
    let mut parts = vec![[pixels[0][0], pixels[0][1], radius, radius]];
    for pair in pixels.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let count = (((dx * dx + dy * dy).sqrt() / 0.5f64.max(radius * 0.75)).ceil() as usize).max(1);
        if parts.len() + count > MAX_PARTS {
            return Err(invalid("manual path too long"));
        }
        for i in 1..=count {
            parts.push([a[0] + dx * (i as f64 / count as f64), a[1] + dy * (i as f64 / count as f64), radius, radius]);
        }
    }
    let metadata = json!({"manual":true,"polarity":"manual","diameter":radius*2.,"score":0,"detail_kind":"scratch"});
    make_spot(&parts, shape, metadata.as_object().cloned().ok_or_else(|| invalid("metadata"))?)
}
