//! GPU Local Mask adjustments (Zero-Copy in VRAM, matching grainy-local/src/local.rs).

use grainy_core::Plane;
use serde_json::{Map, Value};

use crate::detail::gaussian;
use crate::device::LOCAL_BASE;
use crate::{Error, Frame, Gpu, Result};

/// A 2D float32 mask plane uploaded to GPU storage (reused across local slider drags).
#[derive(Clone, Debug)]
pub struct LocalMask {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) width: usize,
    pub(crate) height: usize,
    owner: u64,
}

fn truthy(edit: &Map<String, Value>, key: &str) -> bool {
    edit.get(key).is_some_and(grainy_core::settings::truthy)
}

fn num(edit: &Map<String, Value>, key: &str) -> Option<f64> {
    match edit.get(key) {
        None | Some(Value::Null) => Some(0.0),
        Some(Value::Number(n)) => n.as_f64().filter(|v| v.is_finite()),
        Some(Value::Bool(b)) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// Whether a local mask layer's `adjustments` map can execute 100% on the GPU without CPU fallback.
/// Covers exposure, contrast, temperature, tint, saturation, shadows, highlights, whites, blacks,
/// local_curves, hue, HSL mixer, and local clarity.
pub fn can_gpu_local_layer(edit: &Map<String, Value>, pixel_scale: f64) -> bool {
    if truthy(edit, "hdr") {
        return false;
    }
    if ["texture", "dehaze", "noise_luma", "noise_color", "sharpen", "point_colors"].iter().any(|k| truthy(edit, k)) {
        return false;
    }
    if truthy(edit, "clarity") && (2.0 * pixel_scale > 256.0 || !pixel_scale.is_finite() || pixel_scale <= 0.0) {
        return false;
    }
    true
}

type LocalCurveTables = ([u32; 4], [u32; 4], Vec<[f32; 2]>);

fn parse_local_curves(edit: &Map<String, Value>) -> Option<LocalCurveTables> {
    let mut starts = [0u32; 4];
    let mut counts = [0u32; 4];
    let mut table = Vec::new();
    let Some(value) = edit.get("local_curves") else {
        return Some((starts, counts, table));
    };
    let list = value.as_array()?;
    for (ch, points) in list.iter().enumerate() {
        let rows = points.as_array()?;
        if rows.is_empty() || ch >= 4 {
            return None;
        }
        let mut pts = Vec::with_capacity(rows.len());
        let mut identity = true;
        for row in rows {
            let pair = row.as_array()?;
            if pair.len() != 2 {
                return None;
            }
            let x = pair[0].as_f64()? as f32;
            let y = pair[1].as_f64()? as f32;
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            if x != y {
                identity = false;
            }
            pts.push([x, y]);
        }
        if identity {
            continue;
        }
        if !pts.windows(2).all(|w| w[0][0] <= w[1][0]) {
            return None;
        }
        starts[ch] = table.len() as u32;
        counts[ch] = pts.len() as u32;
        table.extend(pts);
    }
    if table.len() > 4096 {
        return None;
    }
    Some((starts, counts, table))
}

fn parse_local_hsl(edit: &Map<String, Value>) -> Option<Option<[f32; 24]>> {
    let Some(value) = edit.get("hsl") else {
        return Some(None);
    };
    let list = value.as_array()?;
    let mut out = [0.0f32; 24];
    let mut any = false;
    for (g, item) in list.iter().take(8).enumerate() {
        let vals = item.as_array()?;
        if vals.len() != 3 {
            return None;
        }
        for c in 0..3 {
            let v = vals[c].as_f64()? as f32;
            if !v.is_finite() {
                return None;
            }
            if v != 0.0 {
                any = true;
            }
            out[g * 3 + c] = v;
        }
    }
    Some(any.then_some(out))
}

impl Gpu {
    /// Upload a float32 mask plane (`H×W`) to GPU storage for zero-copy local adjustment passes.
    pub fn upload_mask(&self, mask: &Plane) -> Result<LocalMask> {
        self.check()?;
        let (height, width) = mask.dim();
        if height == 0 || width == 0 {
            return Err(Error::Input("local mask cannot be empty".into()));
        }
        let std_mask = mask.as_standard_layout();
        let slice = std_mask.as_slice().ok_or_else(|| Error::Input("non-contiguous mask".into()))?;
        let buffer = self.buffer_init("Grainy local mask", bytemuck::cast_slice(slice), wgpu::BufferUsages::STORAGE)?;
        Ok(LocalMask { buffer, width, height, owner: self.owner })
    }

    /// Apply a sequence of GPU-supported local mask layers directly to a GPU-resident frame in one
    /// command batch (zero CPU pixel readback). Returns `Ok(None)` if any layer needs CPU fallback.
    pub fn apply_local_layers(
        &self,
        frame: &Frame,
        layers: &[(&LocalMask, &Map<String, Value>)],
        pixel_scale: f64,
    ) -> Result<Option<Frame>> {
        self.check_frame(frame)?;
        if layers.is_empty() {
            return Ok(Some(frame.clone()));
        }
        for (mask, edit) in layers {
            if mask.owner != self.owner {
                return Err(Error::ForeignFrame);
            }
            if mask.width != frame.width || mask.height != frame.height {
                return Err(Error::Input("local mask size mismatch".into()));
            }
            if !can_gpu_local_layer(edit, pixel_scale) {
                return Ok(None);
            }
        }

        let mut batch = self.batch();
        let mut current = frame.clone();
        let (w, h) = (frame.width, frame.height);
        let last_idx = layers.len() - 1;
        for (layer_idx, (mask, edit)) in layers.iter().enumerate() {
            let clip_final = layer_idx == last_idx;
            let Some((starts, counts, table)) = parse_local_curves(edit) else {
                return Ok(None);
            };
            let Some(hsl_opt) = parse_local_hsl(edit) else {
                return Ok(None);
            };
            let Some(exposure) = num(edit, "exposure") else { return Ok(None) };
            let Some(contrast) = num(edit, "contrast") else { return Ok(None) };
            let Some(temperature) = num(edit, "temperature") else { return Ok(None) };
            let Some(tint) = num(edit, "tint") else { return Ok(None) };
            let Some(saturation) = num(edit, "saturation") else { return Ok(None) };
            let Some(shadows) = num(edit, "shadows") else { return Ok(None) };
            let Some(highlights) = num(edit, "highlights") else { return Ok(None) };
            let Some(blacks) = num(edit, "blacks") else { return Ok(None) };
            let Some(whites) = num(edit, "whites") else { return Ok(None) };
            let Some(hue_deg) = num(edit, "hue") else { return Ok(None) };
            let Some(clarity) = num(edit, "clarity") else { return Ok(None) };

            let has_levels = truthy(edit, "whites") || truthy(edit, "blacks");
            let has_hue = truthy(edit, "hue");
            let has_hsl = hsl_opt.is_some();
            let has_clarity = truthy(edit, "clarity") && clarity != 0.0;
            let flags = u32::from(has_levels)
                | (u32::from(has_hue) << 1)
                | (u32::from(has_hsl) << 2)
                | (u32::from(!has_clarity) << 3)
                | (u32::from(clip_final) << 4);

            let black = (blacks / 200.0) as f32;
            let white = (1.0 + whites / 200.0) as f32;
            let mut p = vec![0.0f32; 46];
            p[0] = w as f32;
            p[1] = h as f32;
            p[2] = flags as f32;
            p[3] = 2f64.powf(exposure) as f32;
            p[4] = (1.0 + contrast / 125.0) as f32;
            p[5] = (temperature / 400.0) as f32;
            p[6] = (-tint * 0.5 / 400.0) as f32;
            p[7] = (-temperature / 400.0) as f32;
            p[8] = (1.0 + saturation / 100.0) as f32;
            p[9] = (shadows / 300.0) as f32;
            p[10] = (highlights / 300.0) as f32;
            p[11] = white - black;
            p[12] = black;
            p[13] = (hue_deg / 360.0) as f32;
            for c in 0..4 {
                p[14 + c] = starts[c] as f32;
                p[18 + c] = counts[c] as f32;
            }
            if let Some(hsl) = hsl_opt {
                p[22..46].copy_from_slice(&hsl);
            }

            let extra0: [Option<&wgpu::Buffer>; 4] = [Some(&mask.buffer), None, None, None];
            let adj = batch.dispatch_buffers(&current, LOCAL_BASE, &p, &table, extra0)?;
            if has_clarity {
                let taps = gaussian(2.0 * pixel_scale);
                if taps.len() > 4096 {
                    return Ok(None);
                }
                let bh_p = [w as f32, h as f32, taps.len() as f32];
                let bh = batch.dispatch(&adj, LOCAL_BASE + 1, &bh_p, &taps, &[])?;
                let bv_p = [w as f32, h as f32, taps.len() as f32, clarity as f32, f32::from(clip_final)];
                let extra2: [Option<&wgpu::Buffer>; 4] =
                    [Some(&mask.buffer), Some(&current.buffer), Some(&adj.buffer), None];
                current = batch.dispatch_buffers(&bh, LOCAL_BASE + 2, &bv_p, &taps, extra2)?;
            } else {
                current = adj;
            }
        }
        batch.submit()?;
        Ok(Some(current))
    }
}
