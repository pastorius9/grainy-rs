//! Process Version 4 (`detail_version >= 4`) edge-aware detail processing:
//! - Fast Local Laplacian Pyramid Clarity (Paris, Hasinoff, Kautz 2011 / Aubry et al. 2014):
//!   zero-halo multi-scale midtone micro-contrast in luminance with chromaticity preservation.
//! - Edge-aware luminance Texture: boosts fine detail without color fringing or dark edge halos.
//! - Guided-filter refined Dehaze: eliminates blocky dark-channel halos along skylines and edges.
//!
//! All arithmetic is portable f32 shared with `grainy-gpu/src/laplacian.wgsl`.

use grainy_core::{Image, Plane};
use ndarray::{Array2, Axis, Zip};
use rayon::prelude::*;

use crate::cv::{self, Mat};
use crate::np::{self, clip, dot3, LUMA};
use grainy_tone::cv::box_filter;

/// Number of reference intensity levels for Fast Local Laplacian interpolation.
pub const LAPLACIAN_LEVELS_K: usize = 8;
/// Edge-preservation threshold in luminance for Local Laplacian Clarity.
pub const CLARITY_SIGMA_R: f32 = 0.14;
/// Edge-preservation threshold in luminance for Edge-Aware Texture.
pub const TEXTURE_SIGMA_R: f32 = 0.08;
/// Luminance floor for chromaticity-preserving ratio scaling.
pub const LUMA_FLOOR: f32 = 0.03;

/// Pyramid depth matched across preview and export resolutions via `pixel_scale`.
pub fn pyramid_levels(width: usize, height: usize, pixel_scale: f64) -> usize {
    let target = if pixel_scale > 0.6 {
        5
    } else if pixel_scale > 0.3 {
        4
    } else {
        3
    };
    let mut min_dim = width.min(height);
    let mut levels = 0;
    while min_dim >= 8 && levels < target {
        min_dim = min_dim.div_ceil(2);
        levels += 1;
    }
    levels
}

#[inline]
fn reflect101(mut i: isize, n: usize) -> usize {
    let len = n as isize;
    if len <= 1 {
        return 0;
    }
    while i < 0 || i >= len {
        if i < 0 {
            i = -i;
        }
        if i >= len {
            i = 2 * len - 2 - i;
        }
    }
    i as usize
}

/// 5-tap separable binomial `[1, 4, 6, 4, 1] / 16` 2x downsample.
fn downsample_plane(src: &[f32], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
    let dw = w.div_ceil(2);
    let dh = h.div_ceil(2);
    let mut horiz = vec![0.0_f32; dw * h];
    horiz.par_chunks_mut(dw).enumerate().for_each(|(y, out_row)| {
        let row = &src[y * w..(y + 1) * w];
        for (dx, slot) in out_row.iter_mut().enumerate() {
            let cx = (dx * 2) as isize;
            let s = row[reflect101(cx - 2, w)]
                + 4.0 * row[reflect101(cx - 1, w)]
                + 6.0 * row[reflect101(cx, w)]
                + 4.0 * row[reflect101(cx + 1, w)]
                + row[reflect101(cx + 2, w)];
            *slot = s * (1.0 / 16.0);
        }
    });
    let mut dst = vec![0.0_f32; dw * dh];
    dst.par_chunks_mut(dw).enumerate().for_each(|(dy, out_row)| {
        let cy = (dy * 2) as isize;
        let r0 = &horiz[reflect101(cy - 2, h) * dw..];
        let r1 = &horiz[reflect101(cy - 1, h) * dw..];
        let r2 = &horiz[reflect101(cy, h) * dw..];
        let r3 = &horiz[reflect101(cy + 1, h) * dw..];
        let r4 = &horiz[reflect101(cy + 2, h) * dw..];
        for (x, slot) in out_row.iter_mut().enumerate() {
            *slot = (r0[x] + 4.0 * r1[x] + 6.0 * r2[x] + 4.0 * r3[x] + r4[x]) * (1.0 / 16.0);
        }
    });
    (dst, dw, dh)
}

/// Center-aligned bilinear upsample matching GPU texture/shader coordinates.
fn upsample_plane(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut dst = vec![0.0_f32; dw * dh];
    let sx_scale = sw as f32 / dw.max(1) as f32;
    let sy_scale = sh as f32 / dh.max(1) as f32;
    dst.par_chunks_mut(dw).enumerate().for_each(|(y, out_row)| {
        let fy = ((y as f32 + 0.5) * sy_scale - 0.5).clamp(0.0, (sh.saturating_sub(1)) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        let row0 = &src[y0 * sw..(y0 + 1) * sw];
        let row1 = &src[y1 * sw..(y1 + 1) * sw];
        for (x, slot) in out_row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) * sx_scale - 0.5).clamp(0.0, (sw.saturating_sub(1)) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            let top = row0[x0] + (row0[x1] - row0[x0]) * tx;
            let bot = row1[x0] + (row1[x1] - row1[x0]) * tx;
            *slot = top + (bot - top) * ty;
        }
    });
    dst
}

fn build_gaussian_pyramid(base: Vec<f32>, w: usize, h: usize, levels: usize) -> Vec<(Vec<f32>, usize, usize)> {
    let mut pyr = Vec::with_capacity(levels + 1);
    pyr.push((base, w, h));
    for _ in 0..levels {
        let (prev, pw, ph) = pyr.last().unwrap();
        let next = downsample_plane(prev, *pw, *ph);
        pyr.push(next);
    }
    pyr
}

/// Fast Local Laplacian Clarity (`detail_version >= 4`):
/// enhances or softens mid-scale luminance contrast without haloing strong object boundaries.
pub fn local_laplacian_clarity(a: &Image, clarity: f64, pixel_scale: f64) -> Image {
    let (h, w, _) = a.dim();
    let levels = pyramid_levels(w, h, pixel_scale);
    if levels == 0 || clarity == 0.0 {
        return a.as_standard_layout().into_owned();
    }
    let std = a.as_standard_layout();
    let src = std.as_slice().unwrap_or(&[]);
    let n = w * h;
    let mut luma = vec![0.0_f32; n];
    luma.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let base = y * w * 3;
        for (x, slot) in row.iter_mut().enumerate() {
            let p = &src[base + x * 3..base + x * 3 + 3];
            *slot = dot3(p, &LUMA).clamp(0.0, 1.0);
        }
    });

    let g_pyr = build_gaussian_pyramid(luma.clone(), w, h, levels);
    let mut l_out: Vec<Vec<f32>> = g_pyr[..levels].iter().map(|(_, lw, lh)| vec![0.0_f32; lw * lh]).collect();

    let boost = (clarity.clamp(-100.0, 100.0) / 100.0 * 0.85) as f32;
    let inv_2s2 = 1.0 / (2.0 * CLARITY_SIGMA_R * CLARITY_SIGMA_R);
    let k_max = (LAPLACIAN_LEVELS_K - 1) as f32;

    for k in 0..LAPLACIAN_LEVELS_K {
        let gk = k as f32 / k_max;
        let remapped: Vec<f32> = luma
            .par_iter()
            .map(|&y| {
                let d = y - gk;
                y + boost * d * (-d * d * inv_2s2).exp()
            })
            .collect();
        let r_pyr = build_gaussian_pyramid(remapped, w, h, levels);
        for l in 0..levels {
            let (ref r_cur, lw, lh) = r_pyr[l];
            let (ref r_next, nw, nh) = r_pyr[l + 1];
            let up = upsample_plane(r_next, nw, nh, lw, lh);
            let g_cur = &g_pyr[l].0;
            l_out[l].par_iter_mut().enumerate().for_each(|(i, acc)| {
                let t = g_cur[i].clamp(0.0, 1.0) * k_max;
                let k0 = (t.floor() as usize).min(LAPLACIAN_LEVELS_K - 2);
                let f = t - k0 as f32;
                let band = r_cur[i] - up[i];
                if k == k0 {
                    *acc += band * (1.0 - f);
                } else if k == k0 + 1 {
                    *acc += band * f;
                }
            });
        }
    }

    let (mut cur, mut cw, mut ch) = g_pyr[levels].clone();
    for l in (0..levels).rev() {
        let (_, lw, lh) = g_pyr[l];
        let mut up = upsample_plane(&cur, cw, ch, lw, lh);
        up.par_iter_mut().zip(&l_out[l]).for_each(|(u, &b)| *u += b);
        cur = up;
        cw = lw;
        ch = lh;
    }

    let mut out = vec![0.0_f32; n * 3];
    out.par_chunks_mut(w * 3).enumerate().for_each(|(y, out_row)| {
        let row_src = &src[y * w * 3..(y + 1) * w * 3];
        let row_y = &luma[y * w..(y + 1) * w];
        let row_new = &cur[y * w..(y + 1) * w];
        for x in 0..w {
            let scale = (row_new[x].clamp(0.0, 1.0) + LUMA_FLOOR) / (row_y[x] + LUMA_FLOOR);
            for c in 0..3 {
                out_row[x * 3 + c] = (row_src[x * 3 + c] * scale).max(0.0);
            }
        }
    });
    Image::from_shape_vec((h, w, 3), out).expect("shape matches input")
}

/// Edge-aware luminance Texture (`detail_version >= 4`): boosts fine detail gated by `TEXTURE_SIGMA_R`
/// so crisp textures pop without color fringing or dark halos around high-contrast boundaries.
pub fn edge_aware_texture(a: &mut Image, texture: f64, pixel_scale: f64) {
    let (h, w, _) = a.dim();
    let sigma = (1.3 * pixel_scale).max(0.05);
    let luma: Plane = a.map_axis(Axis(2), |p| dot3(&[p[0], p[1], p[2]], &LUMA).clamp(0.0, 1.0));
    let luma_img = Image::from_shape_fn((h, w, 1), |(y, x, _)| luma[[y, x]]);
    let blurred_mat = cv::gaussian_blur(&Mat::from_image(&luma_img), sigma);
    let blurred = blurred_mat.into_image();
    let t = (texture.clamp(-100.0, 100.0) / 100.0) as f32;
    let inv_2s2 = 1.0 / (2.0 * TEXTURE_SIGMA_R * TEXTURE_SIGMA_R);
    Zip::from(a.rows_mut()).and(&luma).and(blurred.index_axis(Axis(2), 0)).par_for_each(|mut p, &y, &b| {
        let d = y - b;
        let gate = (-d * d * inv_2s2).exp();
        let y_new = clip(y + d * gate * t, 0.0, 1.0);
        let scale = (y_new + LUMA_FLOOR) / (y + LUMA_FLOOR);
        p.mapv_inplace(|v| v * scale);
    });
}

/// Guided-filter refined Dehaze (`detail_version >= 4`): refines dark-channel transmission with
/// luminance guidance so skylines and tree branches stay completely free of blocky erosion halos.
pub fn guided_dehaze(a: &mut Image, dehaze: f64, diameter: usize) {
    let strength = (dehaze / 100.0 * 0.8) as f32;
    let darkest: Plane = a.map_axis(Axis(2), |p| np::minimum(np::minimum(p[0], p[1]), p[2]));
    let dark = cv::erode(&darkest, diameter);
    let t_raw = dark.mapv(|dk| clip(1.0 - strength * dk, 0.2, 1.8));
    let guide: Plane = a.map_axis(Axis(2), |p| dot3(&[p[0], p[1], p[2]], &LUMA).clamp(0.0, 1.0));

    let radius = (diameter * 2).max(5);
    let mean_i = box_filter(&guide, radius);
    let mean_p = box_filter(&t_raw, radius);
    let corr_ii = box_filter(&(&guide * &guide), radius);
    let corr_ip = box_filter(&(&guide * &t_raw), radius);

    let (h, w) = guide.dim();
    let mut coeff_a = Array2::<f32>::zeros((h, w));
    let mut coeff_b = Array2::<f32>::zeros((h, w));
    Zip::from(&mut coeff_a).and(&mut coeff_b).and(&mean_i).and(&mean_p).and(&corr_ii).and(&corr_ip).par_for_each(
        |ca, cb, &mi, &mp, &cii, &cip| {
            let var_i = (cii - mi * mi).max(0.0);
            let cov_ip = cip - mi * mp;
            let av = cov_ip / (var_i + 0.005);
            *ca = av;
            *cb = mp - av * mi;
        },
    );
    let mean_a = box_filter(&coeff_a, radius);
    let mean_b = box_filter(&coeff_b, radius);

    Zip::from(a.rows_mut()).and(&guide).and(&mean_a).and(&mean_b).par_for_each(|mut p, &g, &ma, &mb| {
        let transmission = clip(ma * g + mb, 0.2, 1.8);
        p.mapv_inplace(|v| (v - 1.0) / transmission + 1.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop_detail;
    use grainy_core::defaults;

    #[test]
    fn pv4_local_laplacian_clarity_is_halo_free_on_sharp_step_edge() {
        // Sharp high-contrast vertical boundary (dark building 0.15 vs bright sky 0.85) with
        // subtle fine texture inside the building so Clarity should boost texture WITHOUT haloing the edge.
        let image = Image::from_shape_fn((64, 96, 3), |(y, x, _)| {
            if x < 48 {
                0.15 + if (x + y) % 4 < 2 { 0.02 } else { -0.02 }
            } else {
                0.85
            }
        });
        let mut s_v3 = defaults();
        s_v3.set("detail_version", 3);
        s_v3.set("clarity", 100.0);
        let out_v3 = develop_detail(&image, &s_v3, 1.0).unwrap();

        let mut s_v4 = defaults();
        s_v4.set("detail_version", 4);
        s_v4.set("clarity", 100.0);
        let out_v4 = develop_detail(&image, &s_v4, 1.0).unwrap();

        // Measure bright halo overshoot on the sky side right next to the building edge (x = 48..54).
        let v3_overshoot = (48..54).map(|x| out_v3[[32, x, 0]] - 0.85).fold(0.0_f32, f32::max);
        let v4_overshoot = (48..54).map(|x| out_v4[[32, x, 0]] - 0.85).fold(0.0_f32, f32::max);
        assert!(
            v4_overshoot < v3_overshoot * 0.25 && v4_overshoot < 0.03,
            "PV4 Local Laplacian must eliminate edge halo (v4 overshoot {v4_overshoot}, v3 overshoot {v3_overshoot})"
        );
        // Meanwhile, verify fine midtone texture inside the building WAS boosted!
        let tex_in = (image[[32, 20, 0]] - image[[32, 22, 0]]).abs();
        let tex_v4 = (out_v4[[32, 20, 0]] - out_v4[[32, 22, 0]]).abs();
        assert!(tex_v4 > tex_in * 1.25, "PV4 Local Laplacian must still boost fine texture ({tex_v4} vs {tex_in})");
    }
}

#[test]
fn clarity_preserves_extended_highlights() {
    let image = Image::from_elem((16, 16, 3), 2.0);
    for amount in [-75., 75.] {
        let out = local_laplacian_clarity(&image, amount, 1.);
        assert!(out.iter().all(|&v| (v - 2.).abs() < 1e-5));
    }
}
