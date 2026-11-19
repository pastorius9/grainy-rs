//! `cv2.Sobel(a, cv2.CV_32F, dx, 1 - dx, ksize=3, scale=scale)` and `cv2.magnitude` on float32 arrays.
//!
//! Sobel: OpenCV scales the smoothing kernel (`[1, 2, 1]·scale`; the derivative `[-1, 0, 1]` stays) and runs its
//! small symmetric/antisymmetric separable filter, rows then columns, reflect-101 borders. Arithmetic of the AVX2
//! build: rows with the scaled kernel compute `fma(c, k0, (a + b)·k1)` in the 8-lane body and `c·k0 + (a + b)·k1`
//! in the scalar tail; columns `fma(a + b, k1, c·k0)` in the body and `(a + b)·k1 + c·k0` in the tail; the
//! derivative half is `b - a` everywhere.
//!
//! Magnitude: `sqrt(fma(x, x, y·y))`, OpenCV's own vector loop. The oracle's build routes `cv2.magnitude` to
//! Intel IPP (`ippsMagnitude_32f`, AVX2), whose results differ from that by up to 2 ulp in about a third of the
//! pixels, in a CPU-dependent way (an approximate reciprocal square root); that is not reproduced.

use rayon::prelude::*;

use super::{reflect101, Mat};

const LANES: usize = 8;

/// Row pass over the interleaved `w·cn` values of each row: `None` is the derivative, `Some([k0, k1])` the
/// smoothing kernel `[k1, k0, k1]`.
fn rows(src: &Mat, smooth: Option<[f32; 2]>) -> Mat {
    let (h, w, cn) = (src.h, src.w, src.cn);
    let width = w * cn;
    let body = width / LANES * LANES;
    let mut out = Mat::zeros(h, w, cn);
    out.data.par_chunks_mut(width.max(1)).enumerate().for_each(|(y, dst)| {
        let row = src.row(y);
        let at = |i: usize, t: isize| row[reflect101((i / cn) as isize + t, w) * cn + i % cn];
        for (i, slot) in dst.iter_mut().enumerate() {
            let (a, c, b) = (at(i, -1), at(i, 0), at(i, 1));
            *slot = match smooth {
                None => b - a,
                Some([k0, k1]) => {
                    if i < body {
                        c.mul_add(k0, (a + b) * k1)
                    } else {
                        c * k0 + (a + b) * k1
                    }
                }
            };
        }
    });
    out
}

fn columns(src: &Mat, smooth: Option<[f32; 2]>) -> Mat {
    let (h, w, cn) = (src.h, src.w, src.cn);
    let width = w * cn;
    let body = width / LANES * LANES;
    let mut out = Mat::zeros(h, w, cn);
    out.data.par_chunks_mut(width.max(1)).enumerate().for_each(|(y, dst)| {
        let up = src.row(reflect101(y as isize - 1, h));
        let centre = src.row(y);
        let down = src.row(reflect101(y as isize + 1, h));
        for (i, slot) in dst.iter_mut().enumerate() {
            let (a, c, b) = (up[i], centre[i], down[i]);
            *slot = match smooth {
                None => b - a,
                Some([k0, k1]) => {
                    if i < body {
                        (a + b).mul_add(k1, c * k0)
                    } else {
                        (a + b) * k1 + c * k0
                    }
                }
            };
        }
    });
    out
}

/// `cv2.Sobel(src, cv2.CV_32F, dx, 1 - dx, ksize=3, scale=scale)` with `dx` 1 (`true`) or 0, `scale != 1`.
pub(crate) fn sobel(src: &Mat, dx: bool, scale: f64) -> Mat {
    // getDerivKernels gives float [1, 2, 1]; `kernel *= scale` multiplies in double and rounds to float.
    let smooth = [(2.0 * scale) as f32, scale as f32];
    if dx {
        columns(&rows(src, None), Some(smooth))
    } else {
        columns(&rows(src, Some(smooth)), None)
    }
}
