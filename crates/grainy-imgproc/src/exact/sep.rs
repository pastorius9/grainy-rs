//! OpenCV's separable linear filter on float32 images (`cv2.GaussianBlur` with `ksize=(0, 0)` and `cv2.sepFilter2D`
//! with symmetric kernels), reproducing the arithmetic of the AVX2 build the oracle runs (opencv-python 5.0 on an
//! FMA-capable x86-64): rows first, then columns, reflect-101 borders, float32 accumulation.
//!
//! Measured against cv2 (see the `cv_*` goldens), not read from OpenCV's source:
//! - rows, kernels of 3 or 5 taps: `s = (x[-1]+x[1])*k1; s = fma(x0, k0, s); s = fma(x[-2]+x[2], k2, s)` in the
//!   8-lane body, the same terms unfused in the scalar tail;
//! - rows, longer kernels: `s = fma(x[t], k[t], s)` over the taps in order from `s = 0` in the 8-lane body, unfused
//!   (`s = k0*x0; s += k[t]*x[t]`) in the tail;
//! - columns: `s = x0*k0; s = fma(x[-t]+x[t], k[t], s)` in the 8-lane body, unfused in the scalar tail.

use rayon::prelude::*;

use super::{reflect101, Mat};

/// Lanes of the fused row body (AVX2, float32).
const ROW_LANES: usize = 8;

pub(super) fn row_pass(src: &Mat, k: &[f32]) -> Mat {
    let n = k.len();
    let r = n / 2;
    let (h, w, cn) = (src.h, src.w, src.cn);
    let width = w * cn;
    let body = if width >= ROW_LANES { width / ROW_LANES * ROW_LANES } else { 0 };
    let mut out = Mat::zeros(h, w, cn);
    let index: Vec<usize> = (0..w + 2 * r).map(|i| reflect101(i as isize - r as isize, w)).collect();
    out.data.par_chunks_mut(width.max(1)).enumerate().for_each(|(y, dst)| {
        let row = src.row(y);
        let mut ext = Vec::with_capacity((w + 2 * r) * cn);
        for &x in &index {
            ext.extend_from_slice(&row[x * cn..(x + 1) * cn]);
        }
        // ext[i + t*cn] is tap t of output element i.
        let tap = |i: usize, t: isize| ext[(i as isize + (r as isize + t) * cn as isize) as usize];
        for (i, slot) in dst.iter_mut().enumerate() {
            let fused = i < body;
            *slot = if n <= 5 {
                let k0 = k[r];
                let k1 = k[r + 1];
                let a1 = tap(i, -1) + tap(i, 1);
                if fused {
                    let mut s = tap(i, 0).mul_add(k0, a1 * k1);
                    if n == 5 {
                        s = (tap(i, -2) + tap(i, 2)).mul_add(k[r + 2], s);
                    }
                    s
                } else {
                    let mut s = k0 * tap(i, 0) + k1 * a1;
                    if n == 5 {
                        s += k[r + 2] * (tap(i, -2) + tap(i, 2));
                    }
                    s
                }
            } else if fused {
                let mut s = 0f32;
                for (t, &kt) in k.iter().enumerate() {
                    s = ext[i + t * cn].mul_add(kt, s);
                }
                s
            } else {
                let mut s = k[0] * ext[i];
                for (t, &kt) in k.iter().enumerate().skip(1) {
                    s += kt * ext[i + t * cn];
                }
                s
            };
        }
    });
    out
}

pub(super) fn column_pass(src: &Mat, k: &[f32]) -> Mat {
    let n = k.len();
    let r = n / 2;
    let (h, w, cn) = (src.h, src.w, src.cn);
    let width = w * cn;
    let lanes = 8;
    let body = width / lanes * lanes;
    let mut out = Mat::zeros(h, w, cn);
    out.data.par_chunks_mut(width.max(1)).enumerate().for_each(|(y, dst)| {
        let rows: Vec<&[f32]> = (0..n).map(|t| src.row(reflect101(y as isize + t as isize - r as isize, h))).collect();
        let centre = rows[r];
        for (i, slot) in dst.iter_mut().enumerate() {
            let mut s = centre[i] * k[r];
            if i < body {
                for t in 1..=r {
                    s = (rows[r - t][i] + rows[r + t][i]).mul_add(k[r + t], s);
                }
            } else {
                for t in 1..=r {
                    s += (rows[r + t][i] + rows[r - t][i]) * k[r + t];
                }
            }
            *slot = s;
        }
    });
    out
}
