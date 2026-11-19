//! Retouching: `luma/processing.py apply_retouch` (red eye, clone, heal versions 1 and 2, inpaint, dust).

use grainy_core::Image;
use grainy_imgproc::{Border, Interpolation};
use ndarray::{s, Array2, Axis, Zip};
use serde_json::Value;

use crate::healing;
use crate::mask::brush_mask_bounds;
use crate::np::{maximum, py_int, py_max};
use crate::value::{bad, enabled, num, numbers, object, points};
use crate::{DistanceTransform, DustRepair, Error, Inpaint, Result};

/// The implementations retouching needs for some operations. Operations that need one that is `None` return
/// [`Error::Missing`].
#[derive(Clone, Copy, Default)]
pub struct RetouchTools<'a> {
    /// `cv2.inpaint(..., cv2.INPAINT_NS)`: the kind `inpaint`, and heal version 2 (`blend_boundary`, `inpaint`).
    pub inpaint: Option<&'a dyn Inpaint>,
    /// `cv2.distanceTransform`: heal version 2 without a stored donor (`automatic_offset`).
    pub distance: Option<&'a dyn DistanceTransform>,
    /// `luma/dust.py repair`: operations of type `dust`.
    pub dust: Option<&'a dyn DustRepair>,
}

const CENTRE: [[f64; 2]; 1] = [[0.5, 0.5]];

/// Stands in for a missing [`Inpaint`]: healing calls it only when pixels must really be inpainted.
struct NoInpaint;

impl Inpaint for NoInpaint {
    fn inpaint_ns(&self, _: &grainy_core::Plane, _: &Array2<u8>, _: f64) -> Result<grainy_core::Plane> {
        Err(Error::Missing("inpainting (Inpaint)"))
    }
}

fn need<'a, T: ?Sized>(tool: Option<&'a T>, what: &'static str) -> Result<&'a T> {
    tool.ok_or(Error::Missing(what))
}

/// `cv2.remap(a, xx + dx, yy + dy, INTER_LINEAR, BORDER_REFLECT_101)` over the rows `y0..y1` and columns `x0..x1`.
fn shifted(a: &Image, (y0, y1, x0, x1): (usize, usize, usize, usize), dx: f32, dy: f32) -> Result<Image> {
    let map_x = Array2::from_shape_fn((y1 - y0, x1 - x0), |(_, x)| (x0 + x) as f32 + dx);
    let map_y = Array2::from_shape_fn((y1 - y0, x1 - x0), |(y, _)| (y0 + y) as f32 + dy);
    Ok(grainy_imgproc::remap(a, &map_x, &map_y, Interpolation::Linear, Border::Reflect101)?)
}

/// Port of luma/processing.py `apply_retouch`: the retouch operations in order on a copy of `source` (the
/// scene-linear frame before geometry), negatives floored at 0. With no operations the source is returned as it is.
pub fn apply_retouch(source: &Image, operations: &[Value], tools: &RetouchTools<'_>) -> Result<Image> {
    if operations.is_empty() {
        return Ok(source.clone());
    }
    let mut a = source.clone();
    let (h, w, _) = a.dim();
    for op in operations {
        let op = object(op, "retouch")?;
        if !enabled(op) {
            continue;
        }
        let kind = match op.get("type") {
            None => Some("heal"),
            Some(Value::String(kind)) => Some(kind.as_str()),
            Some(_) => None,
        };
        if kind == Some("dust") {
            let spots: &[Value] = match op.get("spots") {
                None => &[],
                Some(Value::Array(list)) => list,
                Some(v) => return Err(bad("spots", v)),
            };
            let method = match op.get("repair_method") {
                None => "smooth",
                Some(Value::String(m)) => m.as_str(),
                Some(v) => return Err(bad("repair_method", v)),
            };
            need(tools.dust, "dust repair (DustRepair)")?.repair(&mut a, spots, method)?;
            continue;
        }
        let stroke = points(op, "points", &CENTRE)?;
        let radius = num(op, "radius", 0.015)?;
        let (mask, bounds) = brush_mask_bounds((h, w), &stroke, radius, num(op, "feather", 60.0)?)?;
        let Some((by0, by1, bx0, bx1)) = bounds else { continue };
        // Nonzero pixels lie inside the painted windows; searching those avoids a full-frame scan.
        let mut extent: Option<(usize, usize, usize, usize)> = None;
        for ((y, x), &m) in mask.slice(s![by0..by1, bx0..bx1]).indexed_iter() {
            if m > 0.0 {
                let (y, x) = (y + by0, x + bx0);
                extent = Some(match extent {
                    None => (y, y, x, x),
                    Some(e) => (e.0.min(y), e.1.max(y), e.2.min(x), e.3.max(x)),
                });
            }
        }
        let Some((ys_min, ys_max, xs_min, xs_max)) = extent else { continue };
        let reach = radius * h.min(w) as f64 * 2.0;
        let pad = py_int(reach).ok_or_else(|| bad("radius", &Value::from(radius)))?.max(5);
        let x0 = (xs_min as i64 - pad).max(0) as usize;
        let x1 = (xs_max as i64 + pad + 1).min(w as i64) as usize;
        let y0 = (ys_min as i64 - pad).max(0) as usize;
        let y1 = (ys_max as i64 + pad + 1).min(h as i64) as usize;
        let window = (y0, y1, x0, x1);
        let part = a.slice(s![y0..y1, x0..x1, ..]).to_owned();
        let weight = mask.slice(s![y0..y1, x0..x1]);
        let fixed = match kind {
            Some("red_eye") => {
                let mut fixed = part.clone();
                Zip::from(fixed.lanes_mut(Axis(2))).for_each(|mut p| {
                    if p[0] > maximum(p[1], p[2]) * 1.4_f64 as f32 {
                        p[0] = (p[1] + p[2]) / 2.0;
                    }
                });
                fixed
            }
            Some("inpaint") => {
                let tool = need(tools.inpaint, "inpainting (Inpaint)")?;
                let binary = weight.mapv(|v| if v > 0.05_f64 as f32 { 255u8 } else { 0 });
                let mut fixed = Image::zeros(part.raw_dim());
                for c in 0..3 {
                    let plane = part.index_axis(Axis(2), c).to_owned();
                    let filled = tool.inpaint_ns(&plane, &binary, 3.0)?;
                    healing::check_plane(&filled, plane.dim())?;
                    fixed.index_axis_mut(Axis(2), c).assign(&filled);
                }
                fixed
            }
            Some("heal") if num(op, "version", 1.0)? >= 2.0 => {
                let offset = match op.get("source") {
                    None | Some(Value::Null) => {
                        let distance = need(tools.distance, "a distance transform (DistanceTransform)")?;
                        healing::automatic_offset(&a, &mask, distance)?
                    }
                    Some(donor) => {
                        let d = numbers(donor, "source", 2)?;
                        let p = stroke.first().ok_or_else(|| bad("points", &Value::Array(Vec::new())))?;
                        Some(((d[0] - p[0]) * w as f64, (d[1] - p[1]) * h as f64))
                    }
                };
                let tool = tools.inpaint.unwrap_or(&NoInpaint);
                match offset {
                    None => healing::inpaint(&part, weight, tool)?,
                    Some((dx, dy)) => {
                        let sampled = shifted(&a, window, dx as f32, dy as f32)?;
                        healing::blend_boundary(&part, &sampled, weight, tool)?
                    }
                }
            }
            _ => {
                let first = stroke.first().ok_or_else(|| bad("points", &Value::Array(Vec::new())))?;
                let donor = match op.get("source") {
                    None => *first,
                    Some(v) => {
                        let d = numbers(v, "source", 2)?;
                        [d[0], d[1]]
                    }
                };
                let (dx, dy) = (donor[0] - first[0], donor[1] - first[1]);
                let mut fixed = shifted(&a, window, (dx * w as f64) as f32, (dy * h as f64) as f32)?;
                if kind == Some("heal") {
                    let sigma = py_max(1.0, pad as f64 / 3.0);
                    let from_part = grainy_detail::blur(&part, sigma);
                    let from_fixed = grainy_detail::blur(&fixed, sigma);
                    Zip::from(&mut fixed).and(&from_part).and(&from_fixed).for_each(|f, &p, &q| *f += p - q);
                }
                fixed
            }
        };
        if fixed.dim() != part.dim() {
            return Err(Error::Shape(format!(
                "a retouch result of {:?} for a region of {:?}",
                fixed.dim(),
                part.dim()
            )));
        }
        Zip::from(a.slice_mut(s![y0..y1, x0..x1, ..]).lanes_mut(Axis(2)))
            .and(part.lanes(Axis(2)))
            .and(fixed.lanes(Axis(2)))
            .and(weight)
            .for_each(|mut out, p, f, &wt| {
                for c in 0..3 {
                    out[c] = p[c] * (1.0 - wt) + f[c] * wt;
                }
            });
    }
    a.mapv_inplace(|v| maximum(v, 0.0));
    Ok(a)
}
