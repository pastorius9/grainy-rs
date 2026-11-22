//! GPU spectral film look (all 82 stocks, color negative/slide + B&W + separable Reflect101 halation).

use std::sync::Arc;

use grainy_core::Settings;
use grainy_film::{matrices, stock, Stock, WorkingSpace, NODES, SAMPLES};

use crate::detail::gaussian;
use crate::device::{Batch, FILM_BASE};
use crate::params::TileContext;
use crate::{Error, Frame, Gpu, Result};

fn invert3(m: [[f64; 3]; 3]) -> [[f32; 3]; 3] {
    let a = m[0][0];
    let b = m[0][1];
    let c = m[0][2];
    let d = m[1][0];
    let e = m[1][1];
    let f = m[1][2];
    let g = m[2][0];
    let h = m[2][1];
    let i = m[2][2];
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let inv = 1.0 / det;
    [
        [((e * i - f * h) * inv) as f32, ((c * h - b * i) * inv) as f32, ((b * f - c * e) * inv) as f32],
        [((f * g - d * i) * inv) as f32, ((a * i - c * g) * inv) as f32, ((c * d - a * f) * inv) as f32],
        [((d * h - e * g) * inv) as f32, ((b * g - a * h) * inv) as f32, ((a * e - b * d) * inv) as f32],
    ]
}

fn pack_stock(s: &Stock) -> Vec<f32> {
    let total = 6 * SAMPLES + NODES * NODES * NODES * 3;
    let mut out = vec![0.0f32; total];
    for c in 0..s.channels.min(3) {
        out[c * SAMPLES..(c + 1) * SAMPLES].copy_from_slice(&s.tables[c][..SAMPLES]);
        let sl = 3 * SAMPLES + c * SAMPLES;
        out[sl..sl + SAMPLES].copy_from_slice(&s.slopes[c][..SAMPLES]);
    }
    if s.channels == 3 && s.cube.len() == NODES * NODES * NODES {
        let base = 6 * SAMPLES;
        for (i, rgb) in s.cube.iter().enumerate() {
            out[base + i * 3] = rgb[0];
            out[base + i * 3 + 1] = rgb[1];
            out[base + i * 3 + 2] = rgb[2];
        }
    }
    out
}

impl Gpu {
    pub(crate) fn film_stock_buffer(&self, name: &str) -> Result<(Arc<Stock>, Arc<wgpu::Buffer>)> {
        let built = stock(name).map_err(|e| Error::Input(e.to_string()))?;
        let mut cache = self.film_stocks.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(buf) = cache.get(name) {
            return Ok((built, buf.clone()));
        }
        let packed = pack_stock(&built);
        let buf = Arc::new(self.buffer_init(
            &format!("Grainy film stock {name}"),
            bytemuck::cast_slice(&packed),
            wgpu::BufferUsages::STORAGE,
        )?);
        if cache.len() >= 12 {
            cache.clear();
        }
        cache.insert(name.to_owned(), buf.clone());
        Ok((built, buf))
    }

    /// Run linear preparation + film look (with optional GPU halation) + partial-strength DRT/PV4/rolloff blend
    /// inside an existing GPU batch, returning the sRGB-encoded `tone_encoded` frame.
    /// Returns `Ok(None)` only if the stock's halation kernel exceeds shader bounds on a 1:1 tile.
    pub(crate) fn film_encode_in_batch(
        &self,
        batch: &mut Batch<'_>,
        source: &Frame,
        s: &Settings,
        tile: Option<TileContext>,
    ) -> Result<Option<Frame>> {
        let name = s.str("film_look");
        let (built, stock_buf) = self.film_stock_buffer(name)?;
        let (w, h) = (source.width, source.height);
        let (full_w, full_h) = tile.map_or((w, h), |t| t.full_size);
        let wb = grainy_core::white_balance(s);
        let uv = tile.map_or([0.0, 0.0, 1.0, 1.0], |t| t.uv);
        let prep_flags = (u32::from(s.bool("lens_vignette")) << 3) | (u32::from(tile.is_some()) << 8);
        let prep_p = [
            w as f32,
            h as f32,
            prep_flags as f32,
            0.0,
            wb[0],
            wb[1],
            wb[2],
            2f64.powf(s.f64("exposure")) as f32,
            (s.f64("lens_vignette") / 100.0) as f32,
            uv[0],
            uv[1],
            uv[2],
            uv[3],
        ];
        let prepared = batch.dispatch(source, FILM_BASE, &prep_p, &[], &[])?;

        let space = WorkingSpace::from_setting(s.str("working_space"));
        let (into64, out64) = matrices(space);
        let into: [[f32; 3]; 3] = into64.map(|r| r.map(|v| v as f32));
        let out: [[f32; 3]; 3] = out64.map(|r| r.map(|v| v as f32));
        let back: [[f32; 3]; 3] = invert3(into.map(|r| r.map(f64::from)));

        // Optional separable Reflect101 halation glow.
        let mut halation_frame: Option<Frame> = None;
        let mut halation_strength = [0.0f32; 3];
        if let Some((radius, threshold, strength)) = built.halation_spec() {
            let sigma = radius * (full_h as f64).hypot(full_w as f64);
            if sigma >= 0.5 {
                let taps = gaussian(sigma);
                if taps.len() > 4096 || (tile.is_some() && taps.len() / 2 > 96) {
                    return Ok(None);
                }
                let bright_p = [
                    w as f32, h as f32, 0.0, threshold, into[0][0], into[0][1], into[0][2], into[1][0], into[1][1],
                    into[1][2], into[2][0], into[2][1], into[2][2],
                ];
                let bright = batch.dispatch(&prepared, FILM_BASE + 1, &bright_p, &[], &[])?;
                let blur_p = [w as f32, h as f32, taps.len() as f32];
                let blur_h = batch.dispatch(&bright, FILM_BASE + 2, &blur_p, &taps, &[])?;
                let blur_v = batch.dispatch(&blur_h, FILM_BASE + 3, &blur_p, &taps, &[])?;
                halation_frame = Some(blur_v);
                halation_strength = strength;
            }
        }

        let strength = (s.f64("film_strength").clamp(0.0, 100.0) / 100.0) as f32;
        let is_bw = built.channels == 1;
        let drt_active = grainy_tone::mapping::active(s);
        let tone_v = s.f64("tone_version");
        let plain_v4 = !drt_active && tone_v >= 4.0 && !s.bool("hdr");
        let white = grainy_tone::rolloff_white(s);
        let plain_rolloff = !drt_active && !plain_v4 && !s.bool("hdr") && tone_v >= 2.0 && white > 1.0;
        let flags = u32::from(is_bw)
            | (u32::from(halation_frame.is_some()) << 1)
            | (u32::from(drt_active) << 2)
            | (u32::from(plain_v4) << 3)
            | (u32::from(plain_rolloff) << 4);
        let drt = grainy_tone::mapping::params(s);
        let d65_w = Stock::d65_white();

        let mut ap = vec![0.0f32; 64];
        ap[0] = w as f32;
        ap[1] = h as f32;
        ap[2] = flags as f32;
        ap[3] = strength;
        for r in 0..3 {
            for c in 0..3 {
                ap[4 + r * 3 + c] = into[r][c];
                ap[13 + r * 3 + c] = out[r][c];
                ap[22 + r * 3 + c] = back[r][c];
            }
        }
        for r in 0..built.channels.min(3) {
            for c in 0..3 {
                ap[31 + r * 3 + c] = built.matrix[r][c] as f32;
            }
        }
        ap[40] = built.density_offset();
        ap[41] = (1.0 / built.dx) as f32;
        ap[42] = (SAMPLES as f64 - 1.001) as f32;
        ap[43..46].copy_from_slice(&built.lo);
        ap[46..49].copy_from_slice(&built.inverse_step);
        ap[49] = built.paper_white.unwrap_or(0.0) as f32;
        ap[50..53].copy_from_slice(&halation_strength);
        ap[53] = white as f32;
        ap[54] = ((white - 0.5) / 0.5) as f32;
        ap[55] = drt.p as f32;
        ap[56] = drt.s as f32;
        ap[57] = drt.m as f32;
        ap[58] = drt.toe as f32;
        ap[59] = drt.white as f32;
        ap[60] = drt.shoulder as f32;
        ap[61..64].copy_from_slice(&d65_w);

        let extra: [Option<&wgpu::Buffer>; 4] =
            [halation_frame.as_ref().map(|f| &f.buffer), Some(stock_buf.as_ref()), None, None];
        let encoded = batch.dispatch_buffers(&prepared, FILM_BASE + 4, &ap, &[], extra)?;
        Ok(Some(encoded))
    }
}
