//! Mask layers: brush, linear and radial components, range components, and their combination
//! (`luma/processing.py` `brush_mask`, `component_mask`, `_layer_component`, `image_dependent`, `layer_mask`,
//! `limit_brush_color`).

use grainy_core::{Image, Plane};
use ndarray::{s, Axis, Zip};
use serde_json::Value;

use crate::np::{self, clip, dot3, maximum, norm3, py_int, py_max, LUMA};
use crate::range::color_coverage;
use crate::value::{self, bad, enabled, flag, num, numbers, Obj};
use crate::{Error, Result, SkyMask, Transform};

/// Bounds `(y0, y1, x0, x1)` of the painted windows of a brush mask (every nonzero pixel lies inside).
pub type Bounds = (usize, usize, usize, usize);

/// Component kinds that read the developed image; every other kind depends only on geometry and the source
/// (`IMAGE_COMPONENTS`).
pub const IMAGE_COMPONENTS: [&str; 2] = ["luma", "color"];

const CENTRE: [[f64; 2]; 1] = [[0.5, 0.5]];

/// Port of luma/processing.py `brush_mask`: a soft round brush along the stroke `points` (normalised
/// coordinates), `radius` relative to the shorter side, `feather` in percent. float32 mask of `shape` (h, w).
pub fn brush_mask(shape: (usize, usize), points: &[[f64; 2]], radius: f64, feather: f64) -> Result<Plane> {
    Ok(brush_mask_bounds(shape, points, radius, feather)?.0)
}

/// Port of luma/processing.py `brush_mask(..., with_bounds=True)`: the mask and the bounds of its painted windows
/// (`None` when nothing was painted).
pub fn brush_mask_bounds(
    shape: (usize, usize),
    points: &[[f64; 2]],
    radius: f64,
    feather: f64,
) -> Result<(Plane, Option<Bounds>)> {
    let (h, w) = shape;
    let mut mask = Plane::zeros((h, w));
    let radius = py_max(1.0, radius * h.min(w) as f64);
    let spacing = py_max(1.0, radius * 0.25);
    let soft = py_max(0.01, feather / 100.0);
    let mut samples: Vec<[f64; 2]> = Vec::new();
    let mut previous = [0f64; 2];
    for (i, p) in points.iter().enumerate() {
        // np.array(p, np.float32) * [w - 1, h - 1]: float32 coordinates times int64, so float64.
        let p = [f64::from(p[0] as f32) * (w as f64 - 1.0), f64::from(p[1] as f32) * (h as f64 - 1.0)];
        if i > 0 {
            let (dx, dy) = (p[0] - previous[0], p[1] - previous[1]);
            let length = (dx * dx + dy * dy).sqrt();
            let count = py_int(length / spacing).ok_or_else(|| bad("points", &Value::from(length)))?.max(1);
            samples.extend(np::linspace(previous, p, count as usize + 1));
        } else {
            samples.push(p);
        }
        previous = p;
    }
    let mut bounds: Option<Bounds> = None;
    let clamp = |v: f64, what: &str| py_int(v).ok_or_else(|| bad(what, &Value::from(v)));
    for [px, py] in samples {
        let x0 = clamp(px - radius - 1.0, "points")?.max(0);
        let x1 = clamp(px + radius + 2.0, "points")?.min(w as i64);
        let y0 = clamp(py - radius - 1.0, "points")?.max(0);
        let y1 = clamp(py + radius + 2.0, "points")?.min(h as i64);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let (x0, x1, y0, y1) = (x0 as usize, x1 as usize, y0 as usize, y1 as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = (x as f64 - px).hypot(y as f64 - py) / radius;
                let weight = clip((1.0 - d) / soft, 0.0, 1.0);
                let m = &mut mask[[y, x]];
                *m = maximum(f64::from(*m), weight) as f32;
            }
        }
        bounds = Some(match bounds {
            None => (y0, y1, x0, x1),
            Some(b) => (b.0.min(y0), b.1.max(y1), b.2.min(x0), b.3.max(x1)),
        });
    }
    Ok((mask, bounds))
}

/// Port of luma/processing.py `component_mask`: the mask of a shape component (`brush`, `linear`, `radial`; any
/// other type covers the whole frame) in source pixels of `shape` (h, w).
///
/// The oracle computes linear and radial masks in float64 (numpy scalars promote its float32 grids) and every
/// caller casts them to float32; this returns the float32 values.
pub fn component_mask(shape: (usize, usize), component: &Obj) -> Result<Plane> {
    let (h, w) = shape;
    let kind = match component.get("type") {
        None => "brush",
        Some(Value::String(kind)) => kind.as_str(),
        Some(_) => "",
    };
    if matches!(kind, "subject" | "background" | "object") {
        return crate::selection::coverage(shape, component);
    }
    let p = value::points(component, "points", &CENTRE)?;
    if kind == "brush" {
        return brush_mask(shape, &p, num(component, "radius", 0.04)?, num(component, "feather", 70.0)?);
    }
    if kind != "linear" && kind != "radial" {
        return Ok(Plane::ones((h, w)));
    }
    let (Some(&start), Some(&end)) = (p.first(), p.last()) else {
        return Err(Error::BadValue { key: "points".into(), found: "[]".into() });
    };
    // Row and column coordinates: float32 0..1, promoted to float64 by the float64 numpy scalars below.
    let xs: Vec<f64> = (0..w).map(|x| f64::from(x as f32 / w.saturating_sub(1).max(1) as f32)).collect();
    let ys: Vec<f64> = (0..h).map(|y| f64::from(y as f32 / h.saturating_sub(1).max(1) as f32)).collect();
    let mut out = Plane::zeros((h, w));
    if kind == "linear" {
        let delta = [end[0] - start[0], end[1] - start[1]];
        let denominator = py_max(1e-6, delta[0] * delta[0] + delta[1] * delta[1]);
        for (y, row) in out.axis_iter_mut(Axis(0)).enumerate() {
            let along_y = (ys[y] - start[1]) * delta[1];
            for (x, v) in row.into_iter().enumerate() {
                *v = clip(((xs[x] - start[0]) * delta[0] + along_y) / denominator, 0.0, 1.0) as f32;
            }
        }
        return Ok(out);
    }
    let center = [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0];
    let radius = [maximum((end[0] - start[0]).abs() / 2.0, 0.002), maximum((end[1] - start[1]).abs() / 2.0, 0.002)];
    let turn = num(component, "angle", 0.0)? * (std::f64::consts::PI / 180.0);
    let soft = py_max(0.01, num(component, "feather", 70.0)? / 100.0);
    let (wm, hm) = (w.saturating_sub(1).max(1) as f64, h.saturating_sub(1).max(1) as f64);
    let (w1, h1) = (w as f64 - 1.0, h as f64 - 1.0);
    let (c, sn) = (turn.cos(), turn.sin());
    for (y, row) in out.axis_iter_mut(Axis(0)).enumerate() {
        for (x, v) in row.into_iter().enumerate() {
            let d = if turn != 0.0 || turn.is_nan() {
                // A turned ellipse: its axes are turned in pixels of the photo.
                let dx = (xs[x] - center[0]) * w1;
                let dy = (ys[y] - center[1]) * h1;
                ((dx * c + dy * sn) / (radius[0] * wm)).hypot((dy * c - dx * sn) / (radius[1] * hm))
            } else {
                ((xs[x] - center[0]) / radius[0]).hypot((ys[y] - center[1]) / radius[1])
            };
            *v = clip((1.0 - d) / soft, 0.0, 1.0) as f32;
        }
    }
    Ok(out)
}

/// Port of luma/processing.py `image_dependent`: whether an enabled component of the layer reads the developed
/// image (a luminance or colour range).
pub fn image_dependent(layer: &Obj) -> bool {
    let Some(Value::Array(components)) = layer.get("components") else { return false };
    components
        .iter()
        .filter_map(Value::as_object)
        .any(|c| enabled(c) && value::text(c, "type").is_some_and(|kind| IMAGE_COMPONENTS.contains(&kind)))
}

fn apply_transform(transform: Transform<'_>, mask: Plane) -> Result<Plane> {
    let (h, w) = mask.dim();
    let out = transform(&mask.into_shape_with_order((h, w, 1)).map_err(|e| Error::Shape(e.to_string()))?)?;
    if out.dim().2 != 1 {
        return Err(Error::Shape(format!("the mask transform returned {:?}, not H×W×1", out.shape())));
    }
    Ok(out.index_axis_move(Axis(2), 0))
}

/// Port of luma/processing.py `_layer_component`: one component's coverage in the frame `image` (the developed
/// image the layer works on). Shape components are drawn in `source_shape` and sent through `transform`; a sky
/// component is detected by `sky` in `source_image` (the developed colour source) when given, else in `image`.
pub fn layer_component(
    image: &Image,
    source_shape: (usize, usize),
    component: &Obj,
    transform: Transform<'_>,
    source_image: Option<&Image>,
    sky: Option<&dyn SkyMask>,
) -> Result<Plane> {
    let kind = component.get("type").and_then(Value::as_str);
    match kind {
        Some("luma") => {
            let range = match component.get("range") {
                None => vec![0.25, 0.75],
                Some(v) => numbers(v, "range", 2)?,
            };
            let soft = py_max(0.001, num(component, "softness", 0.1)?);
            let (low, soft32, top) = (range[0] as f32, soft as f32, (range[1] + soft) as f32);
            Ok(image.map_axis(Axis(2), |p| {
                let l = dot3(&[p[0], p[1], p[2]], &LUMA);
                clip((l - low + soft32) / soft32, 0.0, 1.0) * clip((top - l) / soft32, 0.0, 1.0)
            }))
        }
        Some("color") => {
            let tolerance = num(component, "tolerance", 0.3)?;
            if let Some(samples) = component.get("samples") {
                return color_coverage(image, samples, tolerance);
            }
            let reference = match component.get("rgb") {
                None => [0.5f32; 3],
                Some(v) => {
                    let r = numbers(v, "rgb", 3)?;
                    [r[0] as f32, r[1] as f32, r[2] as f32]
                }
            };
            let tol = py_max(0.01, tolerance) as f32;
            Ok(image.map_axis(Axis(2), |p| {
                let d = norm3([p[0] - reference[0], p[1] - reference[1], p[2] - reference[2]]);
                clip(1.0 - d / tol, 0.0, 1.0)
            }))
        }
        Some("sky") => {
            // Found in the developed colour source like an auto-masked brush, so global edits do not move it.
            let sky = sky.ok_or(Error::Missing("sky detection (SkyMask)"))?;
            match source_image {
                None => sky.sky_mask(image, component, None),
                Some(source) => apply_transform(transform, sky.sky_mask(source, component, Some(transform))?),
            }
        }
        _ => {
            let mut source_mask = component_mask(source_shape, component)?;
            if kind == Some("brush") && flag(component, "auto_mask") {
                source_mask = limit_brush_color(source_image.unwrap_or(image), component, &source_mask)?;
            }
            apply_transform(transform, source_mask)
        }
    }
}

/// Port of luma/processing.py `limit_brush_color`: keeps the brush only where the source colour is close to the
/// colour under the stroke's seed (`auto_mask`), in source coordinates.
pub fn limit_brush_color(image: &Image, component: &Obj, mask: &Plane) -> Result<Plane> {
    let (h, w, _) = image.dim();
    if (h, w) != mask.dim() {
        return Err(Error::Invalid("Brush colour source dimensions do not match".into()));
    }
    // The oracle evaluates the default (the stroke's first point) even when a seed is stored.
    let first = value::points(component, "points", &CENTRE)?
        .first()
        .copied()
        .ok_or_else(|| Error::BadValue { key: "points".into(), found: "[]".into() })?;
    let seed = match component.get("seed") {
        Some(v) => {
            let s = numbers(v, "seed", 2)?;
            [s[0], s[1]]
        }
        None => first,
    };
    let x = clip((seed[0] * (w as f64 - 1.0)).round_ties_even(), 0.0, w as f64 - 1.0);
    let y = clip((seed[1] * (h as f64 - 1.0)).round_ties_even(), 0.0, h as f64 - 1.0);
    let (x, y) = (
        py_int(x).ok_or_else(|| bad("seed", &Value::from(x)))?,
        py_int(y).ok_or_else(|| bad("seed", &Value::from(y)))?,
    );
    let radius = ((h.min(w) as f64 * 0.0025).round_ties_even() as i64).max(1);
    let (ry0, ry1) = ((y - radius).max(0) as usize, (y + radius + 1).min(h as i64) as usize);
    let (rx0, rx1) = ((x - radius).max(0) as usize, (x + radius + 1).min(w as i64) as usize);
    let mut sum = [0f32; 3];
    for yy in ry0..ry1 {
        for xx in rx0..rx1 {
            for c in 0..3 {
                sum[c] += image[[yy, xx, c]];
            }
        }
    }
    let count = (ry1 - ry0) * (rx1 - rx0);
    let reference = sum.map(|v| np::mean_of_sum(v, count));
    let Some((y0, y1, x0, x1)) = occupied_bounds(mask) else { return Ok(mask.clone()) };
    let tolerance = py_max(0.001, num(component, "auto_tolerance", 0.18)?) as f32;
    let mut result = mask.clone();
    Zip::from(result.slice_mut(s![y0..y1, x0..x1])).and(image.slice(s![y0..y1, x0..x1, ..]).lanes(Axis(2))).for_each(
        |m, p| {
            let d = norm3([p[0] - reference[0], p[1] - reference[1], p[2] - reference[2]]);
            *m *= clip(1.0 - d / tolerance, 0.0, 1.0);
        },
    );
    Ok(result)
}

/// The bounding rows and columns `(y0, y1, x0, x1)` of the positive pixels of a plane.
pub(crate) fn occupied_bounds(mask: &Plane) -> Option<Bounds> {
    let rows: Vec<usize> =
        mask.axis_iter(Axis(0)).enumerate().filter(|(_, r)| r.iter().any(|&v| v > 0.0)).map(|(i, _)| i).collect();
    let cols: Vec<usize> =
        mask.axis_iter(Axis(1)).enumerate().filter(|(_, c)| c.iter().any(|&v| v > 0.0)).map(|(i, _)| i).collect();
    Some((*rows.first()?, rows.last()? + 1, *cols.first()?, cols.last()? + 1))
}

/// Port of luma/processing.py `layer_mask` (without its cache): the layer's components combined in order (add,
/// subtract, intersect; brush strokes of `paint_version` 2 deposit flow up to a density), inverted and scaled by
/// the opacity. `shape` is the frame (h, w) the layer works on (`image.shape[:2]`).
///
/// Cache: the oracle memoises each component (`component:{layer}:{index}`) keyed by the geometry for shape
/// components and by the image state for range components; a cache would wrap the [`layer_component`] call.
pub fn layer_mask(
    image: &Image,
    source_shape: (usize, usize),
    layer: &Obj,
    transform: Transform<'_>,
    source_image: Option<&Image>,
    sky: Option<&dyn SkyMask>,
) -> Result<Plane> {
    let (h, w, _) = image.dim();
    let mut result = Plane::zeros((h, w));
    for (index, component) in value::objects(layer, "components")?.into_iter().enumerate() {
        if !enabled(component) {
            continue;
        }
        let kind = component.get("type").and_then(Value::as_str);
        let mut mask = layer_component(image, source_shape, component, transform, source_image, sky)?;
        if mask.dim() != (h, w) {
            return Err(Error::Shape(format!(
                "a component mask of {:?} does not fit the frame {:?}",
                mask.dim(),
                (h, w)
            )));
        }
        if flag(component, "invert") {
            mask.mapv_inplace(|v| 1.0 - v);
        }
        let operation = match component.get("operation") {
            None => "add",
            Some(Value::String(op)) => op.as_str(),
            Some(_) => "",
        };
        if kind == Some("brush") && component.get("paint_version").and_then(value::number) == Some(2.0) {
            // Each completed stroke deposits flow once; repeated strokes build toward the density ceiling.
            let flow = clip(num(component, "flow", 100.0)? / 100.0, 0.0, 1.0) as f32;
            let density = clip(num(component, "density", 100.0)? / 100.0, 0.0, 1.0) as f32;
            Zip::from(&mut result).and(&mask).for_each(|r, &m| {
                let coverage = m * flow;
                *r = match operation {
                    "subtract" => *r * (1.0 - coverage * density),
                    "intersect" => *r * (coverage * density),
                    _ => *r + maximum(density - *r, 0.0) * coverage,
                };
            });
        } else if index == 0 {
            result = mask;
        } else {
            Zip::from(&mut result).and(&mask).for_each(|r, &m| {
                *r = match operation {
                    "subtract" => *r * (1.0 - m),
                    "intersect" => *r * m,
                    _ => maximum(*r, m),
                };
            });
        }
    }
    if flag(layer, "invert") {
        result.mapv_inplace(|v| 1.0 - v);
    }
    let opacity = num(layer, "opacity", 1.0)? as f32;
    result.mapv_inplace(|v| clip(v * opacity, 0.0, 1.0));
    Ok(result)
}
