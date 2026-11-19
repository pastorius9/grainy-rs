//! `cv2.Laplacian(plane, cv2.CV_32F)` (ksize 1: the 3×3 kernel 0 1 0 / 1 -4 1 / 0 1 0) and `cv2.Sobel(plane,
//! cv2.CV_32F, 1, 0)` / `(0, 1)` (ksize 3) on float32 planes, reflect-101 borders. The sums keep OpenCV's order:
//! the Laplacian adds its taps row-major (up, left, -4·centre, right, down); Sobel is a separable filter whose
//! smoothing half is `(a + b) + (c + c)` (in the row filter's scalar tail, by pairs, `(a + 2c) + b`) and derivative half
//! `b - a`.

use grainy_core::Plane;
use rayon::prelude::*;

use super::reflect101;

/// Lanes of the row filter's vector body; the scalar tail sums pairs of pixels as `a + 2c + b` left to right and a
/// last odd pixel as the body does.
const LANES: usize = 8;

fn map(src: &Plane, f: impl Fn(&dyn Fn(isize, isize) -> f32, usize) -> f32 + Sync) -> Plane {
    let (h, w) = src.dim();
    let mut out = Plane::zeros((h, w));
    if let Some(data) = out.as_slice_mut() {
        data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, dst)| {
            for (x, slot) in dst.iter_mut().enumerate() {
                let at = |dy: isize, dx: isize| src[[reflect101(y as isize + dy, h), reflect101(x as isize + dx, w)]];
                *slot = f(&at, x);
            }
        });
    }
    out
}

/// `cv2.Sobel(plane, cv2.CV_32F, 1, 0)`: horizontal difference, vertical smoothing.
pub(crate) fn sobel_x(src: &Plane) -> Plane {
    map(src, |at, _| {
        let d = |dy: isize| at(dy, 1) - at(dy, -1);
        let c = d(0);
        (d(-1) + d(1)) + (c + c)
    })
}

/// `cv2.Sobel(plane, cv2.CV_32F, 0, 1)`: horizontal smoothing, vertical difference.
pub(crate) fn sobel_y(src: &Plane) -> Plane {
    let width = src.ncols();
    let body = width / LANES * LANES;
    let pairs = body + (width - body) / 2 * 2;
    map(src, |at, x| {
        let tail = x >= body && x < pairs;
        let s = |dy: isize| {
            let c = at(dy, 0);
            if tail {
                at(dy, -1) + c * 2.0 + at(dy, 1)
            } else {
                (at(dy, -1) + at(dy, 1)) + (c + c)
            }
        };
        s(1) - s(-1)
    })
}
