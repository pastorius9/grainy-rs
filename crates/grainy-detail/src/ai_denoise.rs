//! Local AI RAW Denoise Engine (Phase 6):
//! Built-in zero-download **Opponent-Space Non-Local Means (3×3 Patch Collaborative Denoiser)**
//! with exact GPU compute parity (`grainy-gpu/src/ai_denoise.wgsl`), preserving fine high-contrast
//! edges (hair, foliage, eyelashes) while suppressing high-ISO luminance and chroma noise.

use grainy_core::Image;
use rayon::prelude::*;

/// Computes the collaborative NLM parameters `(h_luma_sq, h_chroma_sq, blend)` from `amount` in `0..=100`.
pub fn params(amount: f64, pixel_scale: f64) -> (f32, f32, f32) {
    let t = (amount / 100.0).clamp(0.0, 1.0);
    let scale = pixel_scale.clamp(0.25, 2.0);
    // Non-linear response curve so 20–50 gives clean, natural photographic noise removal
    // and 70–100 handles ISO 12800–25600 high-noise RAWs cleanly.
    let h_luma = ((0.010 + 0.068 * t.powf(1.35)) * (0.65 + 0.35 * scale)) as f32;
    let h_chroma = h_luma * 2.4;
    let blend = (t * 1.15).clamp(0.0, 1.0) as f32;
    ((h_luma * h_luma).max(1e-8), (h_chroma * h_chroma).max(1e-8), blend)
}

#[inline]
fn to_ycc(rgb: [f32; 3]) -> [f32; 3] {
    let y = (rgb[0] * 0.2126 + rgb[1] * 0.7152) + rgb[2] * 0.0722;
    [y, rgb[2] - y, rgb[0] - y]
}

#[inline]
fn from_ycc(ycc: [f32; 3]) -> [f32; 3] {
    let y = ycc[0];
    let cb = ycc[1];
    let cr = ycc[2];
    let r = y + cr;
    let b = y + cb;
    let g = (y - 0.2126 * r - 0.0722 * b) * (1.0 / 0.7152);
    [r, g, b]
}

#[inline]
fn clamp_i32(v: i32, hi: i32) -> usize {
    v.clamp(0, hi) as usize
}

/// Applies the built-in Non-Local Means opponent-space collaborative denoiser (`amount` in `0..=100`).
pub fn apply(image: &Image, amount: f64, pixel_scale: f64) -> Image {
    if !amount.is_finite() || amount <= 0.0 {
        return image.clone();
    }
    let (h, w, c) = image.dim();
    if h == 0 || w == 0 || c != 3 {
        return image.clone();
    }
    let (h_luma_sq, h_chroma_sq, blend) = params(amount, pixel_scale);
    let inv_h_luma = -1.0 / h_luma_sq;
    let inv_h_chroma = -1.0 / h_chroma_sq;

    let src = image.as_standard_layout();
    let src_slice = src.as_slice().expect("standard layout");
    let mut ycc = vec![[0.0f32; 3]; h * w];
    ycc.par_chunks_exact_mut(w).enumerate().for_each(|(y, row)| {
        let base = y * w * 3;
        for (x, slot) in row.iter_mut().enumerate() {
            let i = base + x * 3;
            *slot = to_ycc([src_slice[i], src_slice[i + 1], src_slice[i + 2]]);
        }
    });

    let mut out = vec![0.0f32; h * w * 3];
    let wi = w as i32 - 1;
    let hi = h as i32 - 1;
    out.par_chunks_exact_mut(w * 3).enumerate().for_each(|(y, row)| {
        let yi = y as i32;
        for (x, dst) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let xi = x as i32;
            let center = ycc[y * w + x];
            let mut sum_y = center[0];
            let mut sum_cb = center[1];
            let mut sum_cr = center[2];
            let mut w_y = 1.0f32;
            let mut w_c = 1.0f32;

            // 7x7 search window with 3x3 patch distance (skipping center offset (0,0) which has weight 1.0).
            for dy in -3..=3 {
                for dx in -3..=3 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let mut dist_y = 0.0f32;
                    let mut dist_c = 0.0f32;
                    for py in -1..=1 {
                        let ay = clamp_i32(yi + py, hi) * w;
                        let by = clamp_i32(yi + dy + py, hi) * w;
                        for px in -1..=1 {
                            let a = ycc[ay + clamp_i32(xi + px, wi)];
                            let b = ycc[by + clamp_i32(xi + dx + px, wi)];
                            let dy0 = a[0] - b[0];
                            let dcb = a[1] - b[1];
                            let dcr = a[2] - b[2];
                            dist_y += dy0 * dy0;
                            dist_c += dcb * dcb + dcr * dcr;
                        }
                    }
                    let d_luma = (dist_y + 0.25 * dist_c) * (1.0 / 9.0);
                    let d_chroma = (0.35 * dist_y + dist_c) * (1.0 / 9.0);
                    let wy = (d_luma * inv_h_luma).exp();
                    let wc = (d_chroma * inv_h_chroma).exp();
                    let nb = ycc[clamp_i32(yi + dy, hi) * w + clamp_i32(xi + dx, wi)];
                    sum_y += wy * nb[0];
                    w_y += wy;
                    sum_cb += wc * nb[1];
                    sum_cr += wc * nb[2];
                    w_c += wc;
                }
            }
            let den_ycc = [sum_y / w_y, sum_cb / w_c, sum_cr / w_c];
            let rgb = from_ycc(den_ycc);
            let si = (y * w + x) * 3;
            for ch in 0..3 {
                dst[ch] = src_slice[si + ch] + (rgb[ch] - src_slice[si + ch]) * blend;
            }
        }
    });
    Image::from_shape_vec((h, w, 3), out).expect("valid shape")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_denoise_suppresses_high_iso_noise_while_preserving_step_edge() {
        // Synthetic high-ISO image: smooth dark half (0.2) + sharp vertical edge + bright half (0.8) with noise.
        let (h, w) = (48, 64);
        let mut seed = 0x9e37_79b9u32;
        let noisy = Image::from_shape_fn((h, w, 3), |(_, x, _)| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((seed >> 16) as f32 / 65535.0 - 0.5) * 0.14;
            let base = if x < w / 2 { 0.2 } else { 0.8 };
            (base + n).clamp(0.0, 1.0)
        });
        let clean = apply(&noisy, 65.0, 1.0);
        let noise_var = |img: &Image| -> f32 {
            let mut sum = 0.0f32;
            let mut count = 0.0f32;
            for y in 4..h - 4 {
                for x in 4..(w / 2 - 4) {
                    let d = img[[y, x, 0]] - 0.2;
                    sum += d * d;
                    count += 1.0;
                }
            }
            sum / count
        };
        assert!(noise_var(&clean) < noise_var(&noisy) * 0.20, "smooth region noise reduced >80%");
        // Sharp step edge remains crisp right across the boundary.
        let edge_jump = clean[[h / 2, w / 2 + 1, 0]] - clean[[h / 2, w / 2 - 2, 0]];
        assert!(edge_jump > 0.52, "edge preserved sharply, jump={edge_jump}");
    }
}
