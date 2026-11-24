//! Adobe DNG 1.6 `ProfileGainTableMap` (TIFF tag `52525` / `0xCD2D`) parser and evaluator
//! for Apple ProRAW and computational DNG files.
//!
//! Per the Adobe DNG 1.6 specification, all multi-byte header fields and IEEE-754 floats inside
//! `ProfileGainTableMap` are stored in **big-endian** byte order regardless of the enclosing
//! TIFF container's byte order.

use crate::{invalid, Result};
use grainy_core::Image;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// TIFF tag number of `ProfileGainTableMap` (`0xCD2D`).
pub const TAG_PROFILE_GAIN_TABLE_MAP: u16 = 52525;

/// Parsed Adobe DNG 1.6 `ProfileGainTableMap` (spatially-varying, tone-dependent linear gain map).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProfileGainTableMap {
    pub map_points_v: u32,
    pub map_points_h: u32,
    pub map_spacing_v: f64,
    pub map_spacing_h: f64,
    pub map_origin_v: f64,
    pub map_origin_h: f64,
    pub map_points_n: u32,
    pub map_input_weights: [f32; 5],
    pub table: Vec<f32>,
}

impl ProfileGainTableMap {
    /// Parses a raw DNG 1.6 `ProfileGainTableMap` byte buffer (big-endian layout).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        // Header: 2*u32 (8) + 4*f64 (32) + 1*u32 (4) + 5*f32 (20) = 64 bytes.
        if bytes.len() < 64 {
            return Err(invalid("ProfileGainTableMap payload too short"));
        }
        let u32_be = |off: usize| u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        let f32_be = |off: usize| f32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        let f64_be = |off: usize| {
            f64::from_be_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
                bytes[off + 4],
                bytes[off + 5],
                bytes[off + 6],
                bytes[off + 7],
            ])
        };

        let map_points_v = u32_be(0);
        let map_points_h = u32_be(4);
        let map_spacing_v = f64_be(8);
        let map_spacing_h = f64_be(16);
        let map_origin_v = f64_be(24);
        let map_origin_h = f64_be(32);
        let map_points_n = u32_be(40);
        let map_input_weights = [f32_be(44), f32_be(48), f32_be(52), f32_be(56), f32_be(60)];

        if map_points_v == 0
            || map_points_h == 0
            || map_points_n == 0
            || map_points_v > 512
            || map_points_h > 512
            || map_points_n > 256
        {
            return Err(invalid("ProfileGainTableMap dimensions out of bounds"));
        }
        if ![map_spacing_v, map_spacing_h, map_origin_v, map_origin_h].iter().all(|v| v.is_finite())
            || map_spacing_v <= 0.0
            || map_spacing_h <= 0.0
            || !map_input_weights.iter().all(|v| v.is_finite())
        {
            return Err(invalid("ProfileGainTableMap header contains invalid floats"));
        }
        let entries = (map_points_v as usize)
            .checked_mul(map_points_h as usize)
            .and_then(|n| n.checked_mul(map_points_n as usize))
            .ok_or_else(|| invalid("ProfileGainTableMap entry count overflow"))?;
        let needed = 64usize
            .checked_add(entries.checked_mul(4).ok_or_else(|| invalid("ProfileGainTableMap byte size overflow"))?)
            .ok_or_else(|| invalid("ProfileGainTableMap byte size overflow"))?;
        if bytes.len() < needed {
            return Err(invalid("ProfileGainTableMap table truncated"));
        }
        let mut table = Vec::with_capacity(entries);
        for i in 0..entries {
            let g = f32_be(64 + i * 4);
            table.push(if g.is_finite() { g.clamp(0.125, 16.0) } else { 1.0 });
        }
        Ok(Self {
            map_points_v,
            map_points_h,
            map_spacing_v,
            map_spacing_h,
            map_origin_v,
            map_origin_h,
            map_points_n,
            map_input_weights,
            table,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !(1..=512).contains(&self.map_points_v)
            || !(1..=512).contains(&self.map_points_h)
            || !(1..=256).contains(&self.map_points_n)
            || self.table.len() != (self.map_points_v * self.map_points_h * self.map_points_n) as usize
            || ![self.map_spacing_v, self.map_spacing_h].iter().all(|v| v.is_finite() && *v > 0.)
            || ![self.map_origin_v, self.map_origin_h].iter().all(|v| v.is_finite())
            || !self.map_input_weights.iter().chain(&self.table).all(|v| v.is_finite())
        {
            return Err(invalid("invalid stored ProfileGainTableMap"));
        }
        Ok(())
    }

    #[inline]
    fn sample_n(&self, r: usize, c: usize, n_pos: f32) -> f32 {
        let nv = self.map_points_n as usize;
        let base = (r * self.map_points_h as usize + c) * nv;
        if nv == 1 {
            return self.table[base];
        }
        let clamped = n_pos.clamp(0.0, (nv - 1) as f32);
        let i0 = (clamped.floor() as usize).min(nv - 1);
        let i1 = (i0 + 1).min(nv - 1);
        let frac = clamped - i0 as f32;
        self.table[base + i0] * (1.0 - frac) + self.table[base + i1] * frac
    }

    /// Evaluates the gain at normalized image coordinates `(v_norm, h_norm)` in `[0, 1]` and linear RGB.
    #[inline]
    pub fn gain_at(&self, v_norm: f64, h_norm: f64, rgb: [f32; 3]) -> f32 {
        let max_v = (self.map_points_v - 1) as f64;
        let max_h = (self.map_points_h - 1) as f64;
        let vf = ((v_norm - self.map_origin_v) / self.map_spacing_v).clamp(0.0, max_v);
        let hf = ((h_norm - self.map_origin_h) / self.map_spacing_h).clamp(0.0, max_h);
        let r0 = (vf.floor() as usize).min(self.map_points_v as usize - 1);
        let r1 = (r0 + 1).min(self.map_points_v as usize - 1);
        let c0 = (hf.floor() as usize).min(self.map_points_h as usize - 1);
        let c1 = (c0 + 1).min(self.map_points_h as usize - 1);
        let fr = (vf - r0 as f64) as f32;
        let fc = (hf - c0 as f64) as f32;

        let r = rgb[0].max(0.0);
        let g = rgb[1].max(0.0);
        let b = rgb[2].max(0.0);
        let lo = r.min(g).min(b);
        let hi = r.max(g).max(b);
        let w = self.map_input_weights;
        let weight = (w[0] * r + w[1] * g + w[2] * b + w[3] * lo + w[4] * hi).clamp(0.0, 1.0);
        let n_pos = weight * (self.map_points_n.saturating_sub(1)) as f32;

        let g00 = self.sample_n(r0, c0, n_pos);
        let g01 = self.sample_n(r0, c1, n_pos);
        let g10 = self.sample_n(r1, c0, n_pos);
        let g11 = self.sample_n(r1, c1, n_pos);
        let top = g00 * (1.0 - fc) + g01 * fc;
        let bot = g10 * (1.0 - fc) + g11 * fc;
        top * (1.0 - fr) + bot * fr
    }

    /// Applies the spatially-varying linear gain table map in-place across an H×W×3 linear image.
    pub fn apply(&self, image: &mut Image) {
        self.apply_oriented(image, 0);
    }

    pub(crate) fn apply_oriented(&self, image: &mut Image, flip: i32) {
        // Invert LibRaw's output orientation to sample the sensor-coordinate table.
        let gain_at = |mut v, mut h, rgb| {
            if flip & 4 != 0 {
                std::mem::swap(&mut v, &mut h);
            }
            if flip & 2 != 0 {
                v = 1. - v;
            }
            if flip & 1 != 0 {
                h = 1. - h;
            }
            self.gain_at(v, h, rgb)
        };
        let (h, w, c) = image.dim();
        if h == 0 || w == 0 || c != 3 {
            return;
        }
        let inv_h = 1.0 / h as f64;
        let inv_w = 1.0 / w as f64;
        if let Some(slice) = image.as_slice_mut() {
            slice.par_chunks_exact_mut(w * 3).enumerate().for_each(|(y, row)| {
                let v_norm = (y as f64 + 0.5) * inv_h;
                for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let h_norm = (x as f64 + 0.5) * inv_w;
                    let gain = gain_at(v_norm, h_norm, *px);
                    px[0] *= gain;
                    px[1] *= gain;
                    px[2] *= gain;
                }
            });
        } else {
            for y in 0..h {
                let v_norm = (y as f64 + 0.5) * inv_h;
                for x in 0..w {
                    let h_norm = (x as f64 + 0.5) * inv_w;
                    let px = [image[[y, x, 0]], image[[y, x, 1]], image[[y, x, 2]]];
                    let gain = gain_at(v_norm, h_norm, px);
                    for ch in 0..3 {
                        image[[y, x, ch]] *= gain;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_be(map: &ProfileGainTableMap) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&map.map_points_v.to_be_bytes());
        out.extend_from_slice(&map.map_points_h.to_be_bytes());
        out.extend_from_slice(&map.map_spacing_v.to_be_bytes());
        out.extend_from_slice(&map.map_spacing_h.to_be_bytes());
        out.extend_from_slice(&map.map_origin_v.to_be_bytes());
        out.extend_from_slice(&map.map_origin_h.to_be_bytes());
        out.extend_from_slice(&map.map_points_n.to_be_bytes());
        for w in map.map_input_weights {
            out.extend_from_slice(&w.to_be_bytes());
        }
        for g in &map.table {
            out.extend_from_slice(&g.to_be_bytes());
        }
        out
    }

    #[test]
    fn proraw_gain_table_map_parses_and_applies_spatial_tone_gain() {
        // 2x2 spatial grid, 2 tone stops: shadow gain lifts left side (2.0), highlights keep 1.0.
        let src = ProfileGainTableMap {
            map_points_v: 2,
            map_points_h: 2,
            map_spacing_v: 1.0,
            map_spacing_h: 1.0,
            map_origin_v: 0.0,
            map_origin_h: 0.0,
            map_points_n: 2,
            map_input_weights: [0.0, 1.0, 0.0, 0.0, 0.0],
            table: vec![
                2.0, 1.0, // (0, 0): shadow 2x, highlight 1x
                1.0, 1.0, // (0, 1): unity
                2.0, 1.0, // (1, 0)
                1.0, 1.0, // (1, 1)
            ],
        };
        let bytes = encode_be(&src);
        let parsed = ProfileGainTableMap::parse(&bytes).unwrap();
        assert_eq!(parsed, src);

        // Dark pixel near left edge gets ~2x gain; bright pixel near left edge gets ~1x; right edge stays ~1x.
        assert!((parsed.gain_at(0.5, 0.0, [0.0, 0.0, 0.0]) - 2.0).abs() < 1e-6);
        assert!((parsed.gain_at(0.5, 0.0, [1.0, 1.0, 1.0]) - 1.0).abs() < 1e-6);
        assert!((parsed.gain_at(0.5, 1.0, [0.0, 0.0, 0.0]) - 1.0).abs() < 1e-6);

        let mut img = Image::from_elem((4, 4, 3), 0.25f32);
        parsed.apply(&mut img);
        assert!(img[[0, 0, 0]] > img[[0, 3, 0]], "left shadow lifted more than right");
        assert!(ProfileGainTableMap::parse(&bytes[..32]).is_err());
    }
}

#[test]
fn gain_map_validates_cached_shape_and_uses_sensor_axes() {
    let mut map = ProfileGainTableMap {
        map_points_v: 1,
        map_points_h: 2,
        map_points_n: 1,
        map_spacing_v: 1.,
        map_spacing_h: 0.5,
        map_origin_v: 0.,
        map_origin_h: 0.25,
        map_input_weights: [0.; 5],
        table: vec![1., 3.],
    };
    let mut image = Image::ones((2, 1, 3));
    map.apply_oriented(&mut image, 4);
    assert_eq!((image[[0, 0, 0]], image[[1, 0, 0]]), (1., 3.));
    map.table.pop();
    let info = crate::rawcolor::CameraInfo { gain_table_map: Some(map), ..Default::default() };
    assert!(crate::rawcolor::CameraSource::new(image, info).is_err());
}
