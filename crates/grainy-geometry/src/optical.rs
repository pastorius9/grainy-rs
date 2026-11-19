//! Port of luma/processing.py `optical_geometry`: lens profile, guides and the manual transform map.

use std::borrow::Cow;

use grainy_core::Settings;
use grainy_imgproc::{get_perspective_transform, remap, warp_perspective, Border, Interpolation};
use ndarray::{stack, Array2, Array3, Axis};
use serde_json::Value;

use crate::lens::{lens_geometry, LensModel};
use crate::Result;

const KEYS: [&str; 10] = [
    "distortion",
    "distortion_k2",
    "distortion_k3",
    "perspective_h",
    "perspective_v",
    "aspect_scale",
    "shift_x",
    "shift_y",
    "ca_red",
    "ca_blue",
];

/// The x and y source maps of one CA offset.
type Maps = (Array2<f32>, Array2<f32>);

/// `np.maximum(.2, x)` in float32 (NaN propagates).
fn floor_02(x: f32) -> f32 {
    if x.is_nan() || x > 0.2f32 {
        x
    } else {
        0.2f32
    }
}

/// The guide corners `np.asarray(s['guides'], np.float32)`, when they form a 4×2 array of numbers.
fn guide_corners(value: &Value) -> Option<[[f32; 2]; 4]> {
    let rows = value.as_array()?;
    if rows.len() != 4 {
        return None;
    }
    let mut out = [[0.0f32; 2]; 4];
    for (slot, row) in out.iter_mut().zip(rows) {
        let pair = row.as_array()?;
        if pair.len() != 2 {
            return None;
        }
        for (s, v) in slot.iter_mut().zip(pair) {
            *s = match v {
                Value::Number(n) => n.as_f64()? as f32,
                Value::Bool(b) => f32::from(u8::from(*b)),
                Value::Null => f32::NAN,
                _ => return None,
            };
        }
    }
    Some(out)
}

/// Port of luma/processing.py `optical_geometry` on an H×W×C frame (1 or 3 channels; masks are H×W×1): the
/// lens profile (when `lensfun_enabled`), the four-corner guides (`cv2.getPerspectiveTransform` and
/// `warpPerspective`), then the manual map (perspective, shift, scale, aspect, radial distortion k1..k3,
/// lateral CA per channel) through `cv2.remap` (bilinear, zero border). The map is built in float32 with the
/// oracle's operation order. Returns the input borrowed when nothing applies.
pub fn optical_geometry<'a>(
    a: &'a Array3<f32>,
    s: &Settings,
    lens: Option<&dyn LensModel>,
) -> Result<Cow<'a, Array3<f32>>> {
    let mut a: Cow<'a, Array3<f32>> = Cow::Borrowed(a);
    if s.bool("lensfun_enabled") {
        a = match a {
            Cow::Borrowed(b) => lens_geometry(b, s, lens)?,
            Cow::Owned(o) => Cow::Owned(lens_geometry(&o, s, lens)?.into_owned()),
        };
    }
    let guides = s.value("guides");
    if !KEYS.iter().any(|k| s.bool(k)) && s.f64("transform_scale") == 100.0 && !grainy_core::settings::truthy(guides) {
        return Ok(a);
    }
    let (h, w, channels) = a.dim();
    if grainy_core::settings::truthy(guides) {
        if let Some(g) = guide_corners(guides) {
            // float32 guides times the int64 list [w-1, h-1]: an exact float64 product, rounded to float32.
            let corners: [[f32; 2]; 4] = std::array::from_fn(|i| {
                [(f64::from(g[i][0]) * (w as f64 - 1.0)) as f32, (f64::from(g[i][1]) * (h as f64 - 1.0)) as f32]
            });
            let (wm, hm) = ((w as f64 - 1.0) as f32, (h as f64 - 1.0) as f32);
            let target = [[0.0, 0.0], [wm, 0.0], [wm, hm], [0.0, hm]];
            let matrix = get_perspective_transform(corners, target)?;
            let warped =
                warp_perspective(a.as_ref(), matrix, (w, h), Interpolation::Linear, Border::Constant(0.0), false)?;
            a = Cow::Owned(warped);
        }
    }
    // Row/column vectors broadcast to the same per-pixel float32 operations as the oracle's grid.
    let scale = (h.max(w) as f64 / 2.0) as f32;
    let half_w = ((w as f64 - 1.0) / 2.0) as f32;
    let half_h = ((h as f64 - 1.0) / 2.0) as f32;
    let ph = (s.f64("perspective_h") / 160.0) as f32;
    let pv = (s.f64("perspective_v") / 160.0) as f32;
    let zoom = {
        let z = s.f64("transform_scale") / 100.0;
        (if z > 0.2 { z } else { 0.2 }) as f32
    };
    let aspect = 2f64.powf(s.f64("aspect_scale") / 100.0) as f32;
    let sx = (s.f64("shift_x") / 100.0) as f32;
    let sy = (s.f64("shift_y") / 100.0) as f32;
    let d1 = (s.f64("distortion") / 100.0) as f32;
    let d2 = (s.f64("distortion_k2") / 100.0) as f32;
    let d3 = (s.f64("distortion_k3") / 100.0) as f32;
    let offsets: Vec<f64> = (0..channels)
        .map(|c| {
            if channels != 3 {
                0.0
            } else if c == 0 {
                s.f64("ca_red") / 10000.0
            } else if c == 2 {
                s.f64("ca_blue") / 10000.0
            } else {
                0.0
            }
        })
        .collect();
    let us: Vec<f32> = (0..w).map(|x| (x as f32 - half_w) / scale).collect();
    let vs: Vec<f32> = (0..h).map(|y| (y as f32 - half_h) / scale).collect();
    // u, v and factor per pixel; the maps for each CA offset follow.
    let mut uu = Array2::<f32>::zeros((h, w));
    let mut vv = Array2::<f32>::zeros((h, w));
    let mut ff = Array2::<f32>::zeros((h, w));
    for y in 0..h {
        let v0 = vs[y];
        for x in 0..w {
            let u0 = us[x];
            let den = floor_02(1.0f32 + ph * u0 + pv * v0);
            let u = (u0 / den + sx) / zoom / aspect;
            let v = (v0 / den + sy) / zoom;
            let r2 = u * u + v * v;
            let factor = 1.0f32 + d1 * r2 + d2 * r2 * r2 + d3 * r2 * r2 * r2;
            uu[[y, x]] = u;
            vv[[y, x]] = v;
            ff[[y, x]] = factor;
        }
    }
    let maps = |ca: f64| -> (Array2<f32>, Array2<f32>) {
        let k = (1.0 + ca) as f32;
        let mut mx = Array2::<f32>::zeros((h, w));
        let mut my = Array2::<f32>::zeros((h, w));
        ndarray::Zip::from(&mut mx).and(&mut my).and(&uu).and(&vv).and(&ff).for_each(|mx, my, &u, &v, &f| {
            *mx = u * f * k * scale + half_w;
            *my = v * f * k * scale + half_h;
        });
        (mx, my)
    };
    let first = offsets.first().copied().unwrap_or(0.0);
    if offsets.iter().all(|&o| o == first) && matches!(channels, 1 | 3 | 4) {
        let (mx, my) = maps(first);
        let result = remap(a.as_ref(), &mx, &my, Interpolation::Linear, Border::Constant(0.0))?;
        return Ok(Cow::Owned(result));
    }
    let mut cache: Vec<(f64, Maps)> = Vec::new();
    let mut planes = Vec::with_capacity(channels);
    for (c, &ca) in offsets.iter().enumerate() {
        if !cache.iter().any(|(k, _)| *k == ca) {
            cache.push((ca, maps(ca)));
        }
        let (mx, my) = &cache.iter().find(|(k, _)| *k == ca).map(|(_, m)| m).unwrap_or(&cache[0].1);
        let plane = a.index_axis(Axis(2), c).to_owned();
        planes.push(remap(&plane, mx, my, Interpolation::Linear, Border::Constant(0.0))?);
    }
    let views: Vec<_> = planes.iter().map(|p| p.view()).collect();
    Ok(Cow::Owned(stack(Axis(2), &views).map_err(|_| grainy_imgproc::Error::Shape("channel stack"))?))
}
