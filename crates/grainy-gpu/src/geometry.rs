//! Single-pass GPU Catmull-Rom Bicubic geometry warp (rotation, flip, straighten, perspective,
//! scale/aspect/shift, radial distortion k1..k3, lateral CA, and crop in one GPU pass).

use grainy_core::Settings;
use serde_json::Value;

use crate::device::GEOMETRY_STAGE;
use crate::{Error, Frame, Gpu, Result};

/// Parameters describing a single-pass GPU geometry warp from a source frame to an output frame.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuGeometryParams {
    pub out_width: usize,
    pub out_height: usize,
    pub rot_width: usize,
    pub rot_height: usize,
    pub pad_x: usize,
    pub pad_y: usize,
    pub crop_l: usize,
    pub crop_t: usize,
    pub turns: u32,
    pub flip: bool,
    pub straighten_deg: f64,
    pub has_optical: bool,
    pub ph: f32,
    pub pv: f32,
    pub zoom: f32,
    pub aspect: f32,
    pub sx: f32,
    pub sy: f32,
    pub d1: f32,
    pub d2: f32,
    pub d3: f32,
    pub ca_r: f32,
    pub ca_b: f32,
}

const OPTICAL_KEYS: [&str; 10] = [
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

fn py_round(x: f64) -> f64 {
    x.round_ties_even()
}

fn straighten_padding(angle: f64, w: usize, h: usize) -> (usize, usize) {
    let rad = angle.abs().to_radians();
    let (s, c) = rad.sin_cos();
    let nw = (w as f64 * c + h as f64 * s).ceil() as usize;
    let nh = (w as f64 * s + h as f64 * c).ceil() as usize;
    (nw.saturating_sub(w).div_ceil(2), nh.saturating_sub(h).div_ceil(2))
}

impl GpuGeometryParams {
    /// Build GPU geometry parameters from `settings` if all active geometry operations are supported
    /// by the single-pass GPU Catmull-Rom warp (no lensfun profile, no 4-corner guides, no upright).
    /// Returns `Ok(None)` when a CPU-only geometry stage is active or when geometry is a no-op.
    pub fn from_settings(
        src_w: usize,
        src_h: usize,
        s: &Settings,
        use_crop_yes: bool,
        use_crop_all: bool,
    ) -> Option<Self> {
        if src_w == 0 || src_h == 0 {
            return None;
        }
        if s.bool("lensfun_enabled") || s.bool("upright") || grainy_core::settings::truthy(s.value("guides")) {
            return None;
        }
        let has_optical = OPTICAL_KEYS.iter().any(|k| s.bool(k)) || s.f64("transform_scale") != 100.0;
        let turns = (-s.i64("rotation")).rem_euclid(4) as u32;
        let flip = s.bool("flip");
        let straighten_deg = s.f64("straighten");
        let crop_val = if use_crop_yes { s.value("crop") } else { &Value::Null };
        let has_crop = grainy_core::settings::truthy(crop_val);
        if !has_optical && turns == 0 && !flip && straighten_deg == 0.0 && !has_crop {
            return None;
        }
        let (mut rot_w, mut rot_h) = if turns % 2 == 1 { (src_h, src_w) } else { (src_w, src_h) };
        let (pad_x, pad_y) = if straighten_deg != 0.0 && use_crop_all {
            let (px, py) = straighten_padding(straighten_deg, rot_w, rot_h);
            rot_w += 2 * px;
            rot_h += 2 * py;
            (px, py)
        } else {
            (0, 0)
        };
        let (crop_l, crop_t, out_width, out_height) = if has_crop {
            let arr = crop_val.as_array()?;
            if arr.len() != 4 {
                return None;
            }
            let vals: Option<Vec<f64>> = arr.iter().map(Value::as_f64).collect();
            let vals = vals?;
            let (wi, hi) = (rot_w as i64, rot_h as i64);
            let l = (py_round(vals[0] * rot_w as f64) as i64).clamp(0, wi - 1);
            let t = (py_round(vals[1] * rot_h as f64) as i64).clamp(0, hi - 1);
            let r = (py_round(vals[2] * rot_w as f64) as i64).min(wi).max(l + 1);
            let b = (py_round(vals[3] * rot_h as f64) as i64).min(hi).max(t + 1);
            (l as usize, t as usize, (r - l) as usize, (b - t) as usize)
        } else {
            (0, 0, rot_w, rot_h)
        };
        let zoom = (s.f64("transform_scale") / 100.0).max(0.2) as f32;
        Some(Self {
            out_width,
            out_height,
            rot_width: rot_w,
            rot_height: rot_h,
            pad_x,
            pad_y,
            crop_l,
            crop_t,
            turns,
            flip,
            straighten_deg,
            has_optical,
            ph: (s.f64("perspective_h") / 160.0) as f32,
            pv: (s.f64("perspective_v") / 160.0) as f32,
            zoom,
            aspect: 2f64.powf(s.f64("aspect_scale") / 100.0) as f32,
            sx: (s.f64("shift_x") / 100.0) as f32,
            sy: (s.f64("shift_y") / 100.0) as f32,
            d1: (s.f64("distortion") / 100.0) as f32,
            d2: (s.f64("distortion_k2") / 100.0) as f32,
            d3: (s.f64("distortion_k3") / 100.0) as f32,
            ca_r: (s.f64("ca_red") / 10000.0) as f32,
            ca_b: (s.f64("ca_blue") / 10000.0) as f32,
        })
    }
}

impl Gpu {
    /// Execute a single-pass GPU Catmull-Rom Bicubic geometry warp from a GPU-resident linear source frame.
    pub fn warp_geometry(&self, source: &Frame, geom: &GpuGeometryParams) -> Result<Frame> {
        self.check_frame(source)?;
        if geom.out_width == 0 || geom.out_height == 0 {
            return Err(Error::Input("geometry output cannot be empty".into()));
        }
        let has_straighten = geom.straighten_deg != 0.0;
        let has_ca = geom.ca_r != 0.0 || geom.ca_b != 0.0;
        let flags = u32::from(geom.has_optical)
            | (u32::from(has_straighten) << 1)
            | (u32::from(geom.flip) << 2)
            | (u32::from(has_ca) << 3);
        let rad = geom.straighten_deg.to_radians();
        let (sa, ca) = rad.sin_cos();
        let p = [
            geom.out_width as f32,
            geom.out_height as f32,
            source.width as f32,
            source.height as f32,
            flags as f32,
            geom.turns as f32,
            geom.crop_l as f32,
            geom.crop_t as f32,
            geom.rot_width as f32,
            geom.rot_height as f32,
            ca as f32,
            sa as f32,
            geom.ph,
            geom.pv,
            geom.zoom,
            geom.aspect,
            geom.sx,
            geom.sy,
            geom.d1,
            geom.d2,
            geom.d3,
            geom.ca_r,
            geom.ca_b,
            geom.pad_x as f32,
            geom.pad_y as f32,
        ];
        let mut batch = self.batch();
        let out = batch.dispatch_sized(source, GEOMETRY_STAGE, geom.out_width, geom.out_height, &p)?;
        batch.submit()?;
        Ok(out)
    }
}
