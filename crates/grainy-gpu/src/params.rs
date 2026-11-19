//! Host constants of native_gpu._tone_params, _color_params and _point_params.
use crate::{Error, Result};
use grainy_colorgrade::{convert, ColorParams};
use grainy_core::{Image, Settings};
use grainy_tone::ToneBase;
use serde_json::Value;

pub(crate) type Built = (Vec<f32>, Vec<[f32; 2]>);

pub(crate) const TONE_KEYS: &[&str] = &[
    "exposure",
    "temperature",
    "tint",
    "kelvin",
    "wb_reference",
    "kelvin_enabled",
    "lens_vignette",
    "shadows",
    "highlights",
    "blacks",
    "whites",
    "contrast",
    "curve",
    "hdr",
    "tone_version",
    "film_look",
    "film_strength",
    "working_space",
    "tone_mapping",
    "drt_contrast",
    "drt_toe",
    "drt_shoulder",
    "drt_white",
];
pub(crate) const COLOR_KEYS: &[&str] = &[
    "saturation",
    "vibrance",
    "mixer_mode",
    "hsl",
    "rgb_curves",
    "parametric",
    "calibration",
    "point_colors",
    "grading",
    "grading_balance",
    "camera_matrix",
    "photo_filter",
    "photo_filter_enabled",
    "photo_filter_density",
    "photo_filter_luminosity",
    "hdr",
];

fn curve(value: &Value) -> Option<Vec<[f32; 2]>> {
    let mut out = Vec::new();
    for pair in value.as_array()? {
        let pair = pair.as_array()?;
        if pair.len() != 2 {
            return None;
        }
        let x = pair[0].as_f64()? as f32;
        let y = pair[1].as_f64()? as f32;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        out.push([x, y]);
    }
    (out.len() >= 2 && out.len() <= 4096).then_some(out)
}
fn identity(curve: &[[f32; 2]]) -> bool {
    curve.iter().all(|[x, y]| (x - y).abs() <= 1e-8f32 + 1e-5f32 * y.abs())
}
fn sorted(curve: &[[f32; 2]]) -> bool {
    curve.windows(2).all(|p| p[0][0] <= p[1][0])
}

pub(crate) fn needs_local(s: &Settings) -> bool {
    s.f64("tone_version") >= 3.0 && !s.bool("hdr") && (s.bool("highlights") || s.bool("shadows"))
}

pub(crate) fn tone(s: &Settings, image: &Image, base: Option<&ToneBase>) -> Result<Option<Built>> {
    if grainy_film::active(s) || grainy_tone::mapping::active(s) {
        return Ok(None);
    }
    let Some(mut table) = curve(s.value("curve")) else {
        return Ok(None);
    };
    let unchanged = identity(&table);
    if !unchanged && !sorted(&table) {
        return Ok(None);
    }
    let n = table.len();
    let (h, w, c) = image.dim();
    if c != 3 || h == 0 || w == 0 {
        return Err(Error::Input("tone needs finite nonempty RGB".into()));
    }
    let wb = grainy_core::white_balance(s);
    let mut flags = u32::from(s.bool("shadows") || s.bool("highlights"))
        | (u32::from(s.bool("contrast")) << 1)
        | (u32::from(!unchanged) << 2)
        | (u32::from(s.bool("lens_vignette")) << 3)
        | (u32::from(s.bool("hdr")) << 4);
    let white = grainy_tone::rolloff_white(s);
    if !s.bool("hdr") && s.f64("tone_version") >= 2.0 && white > 1.0 {
        flags |= 32;
    }
    let slope = ((table[n - 1][1] - table[n - 2][1]) / (table[n - 1][0] - table[n - 2][0]).max(1e-6)).max(0.0);
    let mut p = vec![
        w as f32,
        h as f32,
        0.0,
        n as f32,
        wb[0],
        wb[1],
        wb[2],
        2f64.powf(s.f64("exposure")) as f32,
        (s.f64("shadows") / 180.0) as f32,
        (s.f64("highlights") / 180.0) as f32,
        (s.f64("blacks").clamp(-200.0, 200.0) / 250.0) as f32,
        2f64.powf(s.f64("whites").clamp(-200.0, 200.0) / 100.0) as f32,
        (1.0 + s.f64("contrast") / 125.0) as f32,
        (s.f64("lens_vignette") / 100.0) as f32,
        slope,
        white as f32,
        ((white - 0.5) / 0.5) as f32,
        0.0,
        0.0,
        0.05,
    ];
    if needs_local(s) {
        let local = grainy_tone::tone_local(image, s, base, None)
            .map_err(|e| Error::Input(e.to_string()))?
            .ok_or_else(|| Error::Input("missing local analysis".into()))?;
        let (mh, mw) = local.base.a.dim();
        if mh > 1024 || mw > 1024 {
            return Ok(None);
        }
        p[17] = mw as f32;
        p[18] = mh as f32;
        table.extend(local.curve.iter().map(|&v| [v, 0.0]));
        table.extend(local.base.a.iter().zip(local.base.b.iter()).map(|(&a, &b)| [a, b]));
        flags = (flags & !1) | 64;
    }
    p[2] = flags as f32;
    if !p.iter().all(|v| v.is_finite()) {
        return Err(Error::Input("non-finite tone constants".into()));
    }
    Ok(Some((p, table)))
}

pub(crate) fn color(s: &Settings, w: usize, h: usize) -> Result<Option<Built>> {
    if s.bool("hdr") || s.bool("photo_filter_enabled") {
        return Ok(None);
    }
    let c = match ColorParams::from_settings(s) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    if c.hsl.len() != 8 || c.calibration.len() != 3 || c.grading.len() != 3 || c.parametric.len() != 4 {
        return Ok(None);
    }
    let mut p = vec![0f32; 66];
    p[0] = w as f32;
    p[1] = h as f32;
    p[4] = (1.0 + c.saturation / 100.0) as f32;
    p[5] = (c.vibrance / 100.0) as f32;
    p[6] = (c.grading_balance / 300.0) as f32;
    let any = |g: &[Vec<f64>]| g.iter().flatten().any(|&v| v != 0.0);
    let mut flags = 0u32;
    if c.saturation != 0.0 || c.vibrance != 0.0 {
        flags |= 1;
    }
    if c.vibrance != 0.0 {
        flags |= 2;
    }
    if any(&c.hsl) {
        flags |= if c.mixer_mode == "hsl" { 4 } else { 8 };
    }
    if c.parametric.iter().any(|&v| v != 0.0) {
        flags |= 16;
    }
    if any(&c.calibration) {
        flags |= 32;
    }
    if any(&c.grading) {
        flags |= 64;
    }
    let mut table = Vec::new();
    for (i, raw) in s.value("rgb_curves").as_array().into_iter().flatten().take(3).enumerate() {
        let Some(points) = curve(raw) else {
            return Ok(None);
        };
        if identity(&points) {
            continue;
        }
        if !sorted(&points) {
            return Ok(None);
        }
        p[8 + i] = table.len() as f32;
        p[11 + i] = points.len() as f32;
        table.extend(points);
    }
    if table.len() > 4096 {
        return Ok(None);
    }
    for (slot, v) in p[14..38].iter_mut().zip(c.hsl.iter().flatten()) {
        *slot = *v as f32;
    }
    for (slot, v) in p[38..42].iter_mut().zip(&c.parametric) {
        *slot = *v as f32;
    }
    for (slot, v) in p[42..48].iter_mut().zip(c.calibration.iter().flatten()) {
        *slot = *v as f32;
    }
    for (i, g) in c.grading.iter().enumerate() {
        let hue = ((g[0] / 360.0).rem_euclid(1.0) * 360.0) as f32;
        let tint = convert::hls_to_rgb_pixel(hue, 0.5, 1.0, false);
        for j in 0..3 {
            p[48 + i * 3 + j] = (tint[j] - 0.5) * g[1] as f32 / 200.0 + (g[2] / 400.0) as f32;
        }
    }
    if let Some(matrix) = c.camera_matrix {
        flags |= 128;
        for (slot, v) in p[57..66].iter_mut().zip(matrix.iter().flatten()) {
            *slot = *v;
        }
    }
    let mut count = 0;
    for point in &c.point_colors {
        let get = |k: &str, d: f64| point.get(k).map_or(Some(d), Value::as_f64);
        let Some(version) = get("version", 1.0) else {
            return Ok(None);
        };
        let amounts = [get("hue", 0.0), get("saturation", 0.0), get("lightness", 0.0)];
        let [Some(hue), Some(sat), Some(light)] = amounts else {
            return Ok(None);
        };
        if version >= 2.0 && hue == 0.0 && sat == 0.0 && light == 0.0 {
            continue;
        }
        let mut rgb = [0.5f32; 3];
        if let Some(raw) = point.get("rgb") {
            let Some(raw) = raw.as_array().filter(|a| a.len() == 3) else {
                return Ok(None);
            };
            for (slot, v) in rgb.iter_mut().zip(raw) {
                let Some(v) = v.as_f64() else {
                    return Ok(None);
                };
                *slot = v as f32;
            }
        }
        if !rgb.iter().all(|v| v.is_finite()) {
            return Ok(None);
        }
        let (rh, rs, rl) = convert::rgb_to_hsl_pixel(&rgb, false);
        let [Some(mut hw), Some(sw), Some(lw)] =
            [get("range", 0.12), get("saturation_range", 0.6), get("lightness_range", 0.5)]
        else {
            return Ok(None);
        };
        if version >= 2.0
            && [hw, sw, lw]
                .iter()
                .zip([0.5, 1.0, 1.0])
                .any(|(&v, limit)| !v.is_finite() || v <= 0.0 || v as f32 > limit)
        {
            return Ok(None);
        }
        if version < 2.0 {
            hw = hw.max(0.005);
        }
        p.extend([
            rh,
            rs,
            rl,
            0.0,
            hw as f32,
            sw as f32,
            lw as f32,
            0.0,
            hue as f32,
            sat as f32,
            light as f32,
            0.0,
            f32::from(version >= 2.0),
            f32::from(rs < 0.02 || hw as f32 >= 0.5),
            f32::from(sw as f32 >= 1.0),
            f32::from(lw as f32 >= 1.0),
        ]);
        count += 1;
        if count > 256 {
            return Ok(None);
        }
    }
    if count > 0 {
        flags |= 256;
    }
    p[2] = flags as f32;
    p[3] = count as f32;
    if !p.iter().all(|v| v.is_finite()) {
        return Err(Error::Input("non-finite color constants".into()));
    }
    Ok(Some((p, table)))
}

pub(crate) fn key(s: &Settings, keys: &[&str]) -> Vec<Value> {
    keys.iter().map(|k| s.value(k).clone()).collect()
}
