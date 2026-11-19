use crate::database::{Calibration, Lens};
use crate::{invalid, saved_lens, Result};
use grainy_geometry::{LensModel, LensRequest};
use ndarray::{Array3, Array4};

/// Port of `optics._modifier`'s profile-backed `lensfunpy.Modifier`.
/// Construct once with `ProfileModel::from_settings`, then set `Externals::lens = Some(&model)`.
#[derive(Clone, Debug)]
pub struct ProfileModel {
    xml: String,
    lens: Lens,
}

impl ProfileModel {
    /// Port of `optics._saved_lens(settings['lensfun']['xml'])`.
    pub fn from_settings(settings: &grainy_core::Settings) -> Result<Self> {
        Self::from_profile(settings.value("lensfun"))
    }
    /// Port of `optics._saved_lens(data['xml'])` for a stored profile.
    pub fn from_profile(profile: &serde_json::Value) -> Result<Self> {
        let xml = profile
            .get("xml")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("렌즈 보정 데이터가 올바르지 않습니다."))?;
        Ok(Self { xml: xml.into(), lens: saved_lens(xml)? })
    }
    fn prepared(&self, r: &LensRequest, y: usize, rows: usize) -> Result<Prepared> {
        if r.profile.get("xml").and_then(|v| v.as_str()) != Some(&self.xml) {
            return Err(invalid("lens model/profile mismatch"));
        }
        if r.width == 0
            || r.height == 0
            || r.width > i32::MAX as usize
            || r.height > i32::MAX as usize
            || y.checked_add(rows).is_none_or(|end| end > r.height)
            || r.width.checked_mul(rows).and_then(|n| n.checked_mul(6 * 4)).is_none_or(|n| n > isize::MAX as usize)
            || ![r.crop, r.focal, r.aperture, r.distance, r.scale].iter().all(|v| v.is_finite())
            || r.crop <= 0.
            || r.focal <= 0.
            || r.distance <= 0.
            || r.aperture < 0.
            || r.scale < 0.
            || ![r.crop, r.focal, r.aperture, r.distance, r.scale].iter().all(|v| (*v as f32).is_finite())
            || r.crop as f32 <= 0.
            || r.focal as f32 <= 0.
            || r.distance as f32 <= 0.
            || (r.scale > 0. && !(1. / r.scale as f32).is_finite())
        {
            return Err(invalid("invalid lens modifier dimensions or capture conditions"));
        }
        Prepared::new(&self.lens, r)
    }
}

struct Prepared {
    norm: f64,
    cx: f64,
    cy: f64,
    distortion: Option<Calibration>,
    tca: Option<Calibration>,
    vig: Option<Calibration>,
    scale: f32,
    active: bool,
    vignorm: f32,
}

fn factor(c: &Calibration, r2: f32) -> f32 {
    let t = &c.terms;
    match c.model.as_str() {
        "poly3" => 1. - t[0] + t[0] * r2,
        "poly5" => (1f64 + f64::from(t[0] * r2) + f64::from(t[1] * r2 * r2)) as f32,
        "ptlens" => {
            let r = r2.sqrt();
            t[0] * r2 * r + t[1] * r2 + t[2] * r + (1. - (t[0] + t[1] + t[2]))
        }
        _ => 1.,
    }
}

fn estimated_sqrt(x: f32) -> f32 {
    if x <= 0. || !x.is_finite() {
        return x.sqrt();
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 255) as i32 - 127;
    if exponent < -120 {
        return x.sqrt();
    }
    let index = (((bits & 0x7fffff) | ((exponent.rem_euclid(2) as u32) << 23)) >> 12) as usize;
    let root = crate::sqrt_table::SQRT_BITS[index];
    f32::from_bits((i64::from(root) + ((i64::from(exponent.div_euclid(2))) << 23)) as u32)
}

fn simd_factor(c: &Calibration, r2: f32) -> f32 {
    if c.model != "ptlens" {
        return factor(c, r2);
    }
    let t = &c.terms;
    let r = estimated_sqrt(r2);
    t[0] * r2 * r + t[1] * r2 + t[2] * r + (1. - (t[0] + t[1] + t[2]))
}

fn auto_scale(d: &Calibration, cx: f64, cy: f64) -> f32 {
    // Black-box fit: boundary rays, float32 radius/angle and finite-difference
    // sample, double trig and Newton arithmetic. The independent stress audit
    // still finds differences; this is not claimed as an exact native solver.
    let mut scale = 0f32;
    for (x, y) in [(-cx, -cy), (0., -cy), (cx, -cy), (-cx, 0.), (cx, 0.), (-cx, cy), (0., cy), (cx, cy)] {
        let radius = (x * x + y * y).sqrt() as f32;
        let base = f64::from(y.abs().atan2(x.abs()) as f32);
        let angle = f64::from(
            (if x >= 0. && y >= 0. {
                base
            } else if x < 0. && y >= 0. {
                std::f64::consts::PI - base
            } else if x < 0. {
                std::f64::consts::PI + base
            } else {
                2. * std::f64::consts::PI - base
            }) as f32,
        );
        let ux = angle.cos();
        let uy = angle.sin();
        let mapped = |r: f64| {
            let x = (ux * r) as f32;
            let y = (uy * r) as f32;
            let k = factor(d, x * x + y * y);
            if ux.abs() > uy.abs() {
                f64::from(x * k) / ux
            } else {
                f64::from(y * k) / uy
            }
        };
        let mut r = radius;
        for _ in 0..50 {
            let value = mapped(f64::from(r));
            let residual = value - f64::from(radius);
            if residual.abs() * ux.abs().max(uy.abs()) < 0.001 {
                break;
            }
            let derivative = (mapped(f64::from((f64::from(r) + 0.0001) as f32)) - value) / 0.0001;
            if derivative.abs() < 1e-8 {
                break;
            }
            let next = f64::from(r) - residual / derivative;
            if next <= 0. || !next.is_finite() {
                break;
            }
            r = next as f32;
        }
        scale = scale.max(radius / r);
    }
    scale
}

impl Prepared {
    fn new(lens: &Lens, r: &LensRequest) -> Result<Self> {
        let crop = f64::from(lens.crop_factor) / f64::from(r.crop as f32);
        let diag2 =
            (r.width.saturating_sub(1).max(1) as f64).powi(2) + (r.height.saturating_sub(1).max(1) as f64).powi(2);
        let aspect = f64::from(lens.aspect_ratio);
        // Rounding the squared normalization ratio before the double square
        // root fits the measured maps, including the radius-bin boundaries.
        // Keep the strict synthetic audit separate: exact native normalization
        // and solver parity across arbitrary profiles are not yet established.
        let norm = 2. * f64::from(((aspect * aspect + 1.) / diag2) as f32).sqrt() * crop;
        let cx = r.width.saturating_sub(1).max(1) as f64 * 0.5 * norm;
        let cy = r.height.saturating_sub(1).max(1) as f64 * 0.5 * norm;
        let distortion = if r.flags.distortion { lens.interpolate_distortion(r.focal as f32) } else { None };
        let tca = if r.flags.tca { lens.interpolate_tca(r.focal as f32) } else { None };
        let vig = if r.flags.vignetting {
            lens.interpolate_vignetting(r.focal as f32, r.aperture as f32, r.distance as f32)
        } else {
            None
        };
        let active = distortion.is_some() || tca.is_some();
        let scale = if r.flags.scale {
            if r.scale == 0. {
                let mut scale = 1f32;
                if let Some(d) = &distortion {
                    scale = auto_scale(d, cx, cy);
                }
                if active {
                    // The second (TCA) margin is a float32 multiplication. Doing
                    // both margins in double changes the engine's scale by a ULP.
                    let scale = (f64::from(scale) * 1.001) as f32;
                    if distortion.is_some() && tca.is_some() {
                        scale * 1.001
                    } else {
                        scale
                    }
                } else {
                    1.
                }
            } else {
                r.scale as f32
            }
        } else {
            1.
        };
        Ok(Self {
            norm,
            cx,
            cy,
            distortion,
            tca,
            vig,
            scale,
            active: active || r.flags.scale,
            vignorm: (1. / (aspect * aspect + 1.).sqrt()) as f32,
        })
    }

    fn row(&self, width: usize, y: f32, subpixel: bool, aligned: bool) -> Vec<[f32; 2]> {
        let channels = if subpixel { 3 } else { 1 };
        let mut points = Vec::with_capacity(width * channels);
        let mut x = -self.cx as f32;
        for _ in 0..width {
            for _ in 0..channels {
                points.push([x * (1. / self.scale), y * (1. / self.scale)]);
            }
            // The measured grid fits addition of a double normalization step,
            // followed by float32 rounding. Rounding the step first can move a
            // ptlens radius across a discontinuous estimator-table boundary.
            x = (f64::from(x) + self.norm) as f32;
        }
        if let Some(d) = &self.distortion {
            let count = points.len();
            let grouped = if aligned { count / 4 * 4 } else { 0 };
            for start in (0..grouped).step_by(4) {
                let group = [points[start], points[start + 1], points[start + 2], points[start + 3]];
                for i in 0..4 {
                    if d.model == "poly3" || d.model == "ptlens" {
                        // Measured oracle lane pairing: x uses pair 0/2; y uses pair 1/3.
                        let p = group[(i % 2) * 2];
                        let q = group[(i % 2) * 2 + 1];
                        points[start + i][0] *= simd_factor(d, p[0] * p[0] + p[1] * p[1]);
                        points[start + i][1] *= simd_factor(d, q[0] * q[0] + q[1] * q[1]);
                    } else {
                        let p = group[i];
                        let f = factor(d, p[0] * p[0] + p[1] * p[1]);
                        points[start + i] = [p[0] * f, p[1] * f];
                    }
                }
            }
            for p in &mut points[grouped..] {
                let f = factor(d, p[0] * p[0] + p[1] * p[1]);
                p[0] *= f;
                p[1] *= f;
            }
        }
        for (i, p) in points.iter_mut().enumerate() {
            if subpixel && i % 3 != 1 {
                if let Some(t) = &self.tca {
                    let k = usize::from(i % 3 == 2);
                    let terms = &t.terms;
                    let f = if t.model == "linear" {
                        terms[k]
                    } else {
                        let r2 = p[0] * p[0] + p[1] * p[1];
                        terms[k] + terms[k + 2] * r2.sqrt() + terms[k + 4] * r2
                    };
                    p[0] *= f;
                    p[1] *= f;
                }
            }
            p[0] = ((f64::from(p[0]) + self.cx) / self.norm) as f32;
            p[1] = ((f64::from(p[1]) + self.cy) / self.norm) as f32;
        }
        points
    }
}

impl LensModel for ProfileModel {
    /// Port of `optics._modifier(...).apply_geometry_distortion`.
    fn geometry_coordinates(
        &self,
        r: &LensRequest,
        y: usize,
        rows: usize,
    ) -> std::result::Result<Option<Array3<f32>>, String> {
        let p = self.prepared(r, y, rows).map_err(|e| e.to_string())?;
        if p.distortion.is_none() && !r.flags.scale {
            return Ok(None);
        }
        let bands = grainy_core::parallel::bands(rows, r.width, |start, end| {
            let mut data = Vec::with_capacity((end - start) * r.width * 2);
            // Advance from the strip's first row to preserve the oracle's float32 accumulation.
            let mut py = (y as f64 * p.norm - p.cy) as f32;
            for _ in 0..start {
                py = (f64::from(py) + p.norm) as f32;
            }
            for row in start..end {
                for xy in p.row(r.width, py, false, (row * r.width).is_multiple_of(2)) {
                    data.extend(xy);
                }
                py = (f64::from(py) + p.norm) as f32;
            }
            data
        });
        let data = bands.into_iter().flatten().collect();
        Array3::from_shape_vec((rows, r.width, 2), data).map(Some).map_err(|e| e.to_string())
    }
    /// Port of `optics._modifier(...).apply_subpixel_geometry_distortion`.
    fn subpixel_coordinates(
        &self,
        r: &LensRequest,
        y: usize,
        rows: usize,
    ) -> std::result::Result<Option<Array4<f32>>, String> {
        let p = self.prepared(r, y, rows).map_err(|e| e.to_string())?;
        if !p.active {
            return Ok(None);
        }
        let bands = grainy_core::parallel::bands(rows, r.width, |start, end| {
            let mut data = Vec::with_capacity((end - start) * r.width * 6);
            let mut py = (y as f64 * p.norm - p.cy) as f32;
            for _ in 0..start {
                py = (f64::from(py) + p.norm) as f32;
            }
            for row in start..end {
                for xy in p.row(r.width, py, true, (row * r.width).is_multiple_of(2)) {
                    data.extend(xy);
                }
                py = (f64::from(py) + p.norm) as f32;
            }
            data
        });
        Array4::from_shape_vec((rows, r.width, 3, 2), bands.into_iter().flatten().collect())
            .map(Some)
            .map_err(|e| e.to_string())
    }
    /// Port of `optics._modifier(..., color=True).apply_color_modification`.
    fn apply_color(&self, r: &LensRequest, pixels: &mut Array3<f32>) -> std::result::Result<bool, String> {
        let p = self.prepared(r, 0, r.height).map_err(|e| e.to_string())?;
        if pixels.dim() != (r.height, r.width, 3) {
            return Err("vignetting requires an HxWx3 frame matching the request".into());
        }
        let Some(v) = p.vig.as_ref() else { return Ok(false) };
        let gains = grainy_core::parallel::bands(r.height, r.width, |start, end| {
            let mut gains = Vec::with_capacity((end - start) * r.width);
            let mut y = -p.cy as f32;
            for _ in 0..start {
                y += p.norm as f32;
            }
            for _ in start..end {
                let mut x = -p.cx as f32;
                for _ in 0..r.width {
                    let xx = x * p.vignorm;
                    let yy = y * p.vignorm;
                    let r2 = xx * xx + yy * yy;
                    let f = 1. + r2 * (v.terms[0] + r2 * (v.terms[1] + r2 * v.terms[2]));
                    gains.push(1. / f);
                    x += p.norm as f32;
                }
                y += p.norm as f32;
            }
            gains
        });
        for (mut pixel, gain) in pixels.lanes_mut(ndarray::Axis(2)).into_iter().zip(gains.into_iter().flatten()) {
            pixel.mapv_inplace(|v| v * gain);
        }
        Ok(true)
    }
}

#[test]
fn measured_radius_estimator() {
    let case = grainy_testkit::Case::open("lens", "radius");
    let expected: Array3<f32> = case.array("coordinates");
    let scalar: ndarray::Array2<f32> = case.array("scalar");
    let j = case.json();
    let lens = crate::saved_lens(j["xml"].as_str().unwrap()).unwrap();
    let calibration = &lens.distortion[0];
    let mut maximum = 0f32;
    for (i, start) in j["starts"].as_array().unwrap().iter().enumerate() {
        let x = start[0].as_f64().unwrap() as f32 - 1.;
        let y = start[1].as_f64().unwrap() as f32 - 1.;
        let actual = x * simd_factor(calibration, x * x + y * y) + 1.;
        maximum = maximum.max((actual - expected[[i, 0, 0]]).abs());
        let f = factor(calibration, x * x + y * y);
        maximum = maximum.max((x * f + 1. - scalar[[i, 0]]).abs());
        maximum = maximum.max((y * f + 1. - scalar[[i, 1]]).abs());
    }
    println!("radius estimator coordinates: maximum {maximum:.7e}, tolerance 2e-4");
    assert!(maximum <= 2e-4);
}

#[test]
fn measured_profile_scales() {
    use grainy_geometry::lens::LensFlags;
    let case = grainy_testkit::Case::open("lens", "scales");
    let j = case.json();
    let mut maximum = 0f32;
    let mut max_ulps = 0;
    let mut count = 0;
    for row in j["cases"].as_array().unwrap().iter().filter(|row| row["suite"] == "profiles") {
        let xml = j["xmls"][row["xml_index"].as_u64().unwrap() as usize].as_str().unwrap();
        let lens = crate::saved_lens(xml).unwrap();
        let bits = row["flags"].as_u64().unwrap();
        let r = LensRequest {
            profile: serde_json::json!({"xml": xml}),
            width: row["width"].as_u64().unwrap() as usize,
            height: row["height"].as_u64().unwrap() as usize,
            crop: row["crop"].as_f64().unwrap(),
            focal: row["focal"].as_f64().unwrap(),
            aperture: row["aperture"].as_f64().unwrap(),
            distance: row["distance"].as_f64().unwrap(),
            scale: 0.,
            flags: LensFlags { distortion: bits & 8 != 0, tca: bits & 1 != 0, vignetting: bits & 2 != 0, scale: true },
        };
        let actual = Prepared::new(&lens, &r).unwrap().scale;
        let expected: Vec<f32> = row["scales"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let error = expected.iter().map(|v| (actual - v).abs()).fold(f32::INFINITY, f32::min);
        let ulps = expected.iter().map(|v| actual.to_bits().abs_diff(v.to_bits())).min().unwrap();
        maximum = maximum.max(error);
        max_ulps = max_ulps.max(ulps);
        count += 1;
        println!(
            "profile scale {}: error={error:.7e} ulps={ulps} actual={actual:?} expected={expected:?}",
            row["name"]
        );
        assert!(ulps <= 1, "{}: scale exceeds one float32 ULP", row["name"]);
    }
    println!("profile scales: {count} cases, maximum {maximum:.7e}, {max_ulps} ULPs; tolerance 1 ULP");
}
