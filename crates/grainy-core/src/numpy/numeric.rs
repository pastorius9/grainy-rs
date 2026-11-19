//! numpy equivalents the film model needs: `interp`, `linspace`, small dense linear algebra (`inv`, `lstsq`),
//! a degree-1 `polyfit`, and `percentile`/`median` with numpy's interpolation. Scalar and vector variants preserve their original arithmetic order.

#![allow(clippy::needless_range_loop)]

pub use super::interp_lr;

/// `np.interp(x, xp, fp)` with the end values outside.
pub fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    match (fp.first(), fp.last()) {
        (Some(&left), Some(&right)) => interp_lr(x, xp, fp, left, right),
        _ => f64::NAN,
    }
}

/// `np.linspace(start, stop, n)` in float64: `i*step + start`, the last value exactly `stop`.
pub fn linspace(start: f64, stop: f64, n: usize) -> Vec<f64> {
    let div = n.saturating_sub(1) as f64;
    let delta = stop - start;
    let step = delta / div;
    let mut y: Vec<f64> =
        (0..n).map(|i| if step == 0.0 { i as f64 / div * delta + start } else { i as f64 * step + start }).collect();
    if n > 1 {
        y[n - 1] = stop;
    }
    y
}

/// `np.linspace(start, stop, n)` for float32 scalars (numpy computes it in float32).
pub fn linspace_f32(start: f32, stop: f32, n: usize) -> Vec<f32> {
    let div = n.saturating_sub(1) as f32;
    let delta = stop - start;
    let step = delta / div;
    let mut y: Vec<f32> =
        (0..n).map(|i| if step == 0.0 { i as f32 / div * delta + start } else { i as f32 * step + start }).collect();
    if n > 1 {
        y[n - 1] = stop;
    }
    y
}

#[allow(clippy::needless_range_loop)]
/// A small dense matrix, row-major.
pub type Matrix = Vec<Vec<f64>>;

/// `np.linalg.inv` (LU with partial pivoting, as LAPACK `gesv` on the identity). `None` when singular.
pub fn inv(a: &Matrix) -> Option<Matrix> {
    let n = a.len();
    if a.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut lu = a.clone();
    let mut perm: Vec<usize> = (0..n).collect();
    for k in 0..n {
        let p = (k..n).max_by(|&i, &j| lu[i][k].abs().total_cmp(&lu[j][k].abs()).then(j.cmp(&i)))?;
        if lu[p][k] == 0.0 {
            return None;
        }
        lu.swap(k, p);
        perm.swap(k, p);
        for i in k + 1..n {
            lu[i][k] /= lu[k][k];
            for j in k + 1..n {
                lu[i][j] -= lu[i][k] * lu[k][j];
            }
        }
    }
    let mut out = vec![vec![0.0; n]; n];
    for col in 0..n {
        let mut y: Vec<f64> = (0..n).map(|i| if perm[i] == col { 1.0 } else { 0.0 }).collect();
        for i in 0..n {
            for j in 0..i {
                y[i] -= lu[i][j] * y[j];
            }
        }
        for i in (0..n).rev() {
            for j in i + 1..n {
                y[i] -= lu[i][j] * y[j];
            }
            y[i] /= lu[i][i];
        }
        for i in 0..n {
            out[i][col] = y[i];
        }
    }
    Some(out)
}

/// `np.linalg.lstsq(a, b, rcond=None)[0]`: the minimum-norm least-squares solution through a singular value
/// decomposition (one-sided Jacobi), singular values below `eps*max(m, n)*largest` treated as zero, as LAPACK
/// `gelsd` does.
pub fn lstsq(a: &Matrix, b: &[f64]) -> Result<Vec<f64>, &'static str> {
    let m = a.len();
    let n = a.first().map_or(0, Vec::len);
    if b.len() != m || a.iter().any(|r| r.len() != n) {
        return Err("incompatible least-squares dimensions");
    }
    let mut u = a.clone();
    let mut v: Matrix = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    for _ in 0..60 {
        let mut rotated = false;
        for p in 0..n {
            for q in p + 1..n {
                let (mut alpha, mut beta, mut gamma) = (0.0, 0.0, 0.0);
                for row in &u {
                    alpha += row[p] * row[p];
                    beta += row[q] * row[q];
                    gamma += row[p] * row[q];
                }
                if gamma == 0.0 || gamma.abs() <= f64::EPSILON * (alpha * beta).sqrt() {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                for row in u.iter_mut() {
                    let (x, y) = (row[p], row[q]);
                    row[p] = c * x - s * y;
                    row[q] = s * x + c * y;
                }
                for row in v.iter_mut() {
                    let (x, y) = (row[p], row[q]);
                    row[p] = c * x - s * y;
                    row[q] = s * x + c * y;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let sigma: Vec<f64> = (0..n).map(|j| u.iter().map(|row| row[j] * row[j]).sum::<f64>().sqrt()).collect();
    let largest = sigma.iter().copied().fold(0.0, f64::max);
    let cutoff = f64::EPSILON * m.max(n) as f64 * largest;
    let mut x = vec![0.0; n];
    for j in 0..n {
        if sigma[j] > cutoff {
            // coefficient of the j-th right singular vector: (u_j . b) / sigma_j, with u_j = column / sigma_j
            let dot: f64 = (0..m).map(|i| u[i][j] * b[i]).sum();
            let k = dot / (sigma[j] * sigma[j]);
            for (xi, row) in x.iter_mut().zip(&v) {
                *xi += row[j] * k;
            }
        }
    }
    Ok(x)
}

/// The slope of `np.polyfit(x, y, 1)`.
pub fn polyfit_slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let mx = x.iter().sum::<f64>() / n;
    let my = y.iter().sum::<f64>() / n;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    for (a, b) in x.iter().zip(y) {
        sxy += (a - mx) * (b - my);
        sxx += (a - mx) * (a - mx);
    }
    sxy / sxx
}

pub use super::percentiles;

pub use super::median_f32;

/// numpy's float32 `(..., 3) @ m.T` for one pixel: a fused multiply-add chain, as its BLAS computes it.
#[inline]
pub fn matmul3(p: [f32; 3], m: &[[f32; 3]; 3]) -> [f32; 3] {
    let row = |r: &[f32; 3]| super::gemm3(&p, r);
    [row(&m[0]), row(&m[1]), row(&m[2])]
}

/// A 3×3 float64 matrix product.
pub fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

/// A 3×3 float64 matrix times a vector.
pub fn mulv3(a: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

/// `np.linalg.inv` of a 3×3 matrix (zeros when singular; the matrices inverted here never are).
pub fn inv3(a: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let m: Matrix = a.iter().map(|r| r.to_vec()).collect();
    let r = inv(&m).unwrap_or_else(|| vec![vec![0.0; 3]; 3]);
    [0, 1, 2].map(|i| [r[i][0], r[i][1], r[i][2]])
}

/// Matrix to float32, as `.astype(np.float32)`.
pub fn to_f32(a: &[[f64; 3]; 3]) -> [[f32; 3]; 3] {
    a.map(|r| r.map(|v| v as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_matrix_shapes_are_rejected() {
        let ragged = vec![vec![1.0, 2.0], vec![3.0]];
        assert!(inv(&ragged).is_none());
        assert!(lstsq(&ragged, &[1.0, 2.0]).is_err());
        assert!(lstsq(&vec![vec![1.0, 2.0]], &[]).is_err());
    }

    #[test]
    fn interp_matches_numpy_rules() {
        let xp = [0.0, 1.0, 2.0];
        let fp = [0.0, 10.0, 30.0];
        assert_eq!(interp(1.5, &xp, &fp), 20.0);
        assert_eq!(interp(-1.0, &xp, &fp), 0.0);
        assert_eq!(interp(3.0, &xp, &fp), 30.0);
        assert_eq!(interp(2.0, &xp, &fp), 30.0);
        assert!(interp_lr(f64::NAN, &xp, &fp, 0.0, 0.0).is_nan());
    }

    #[test]
    fn lstsq_solves_square_and_rank_deficient() {
        let a = vec![vec![2.0, 1.0, 0.0], vec![1.0, 3.0, 1.0], vec![0.0, 1.0, 4.0]];
        let x = lstsq(&a, &[1.0, 2.0, 3.0]).unwrap();
        for (row, b) in a.iter().zip([1.0, 2.0, 3.0]) {
            let y: f64 = row.iter().zip(&x).map(|(p, q)| p * q).sum();
            assert!((y - b).abs() < 1e-12);
        }
        let x = lstsq(&vec![vec![1.0, 0.0], vec![0.0, 0.0]], &[2.0, 5.0]).unwrap();
        assert!((x[0] - 2.0).abs() < 1e-15 && x[1] == 0.0);
    }

    #[test]
    fn percentile_and_median() {
        let mut v = [4.0f32, 1.0, 3.0, 2.0];
        assert_eq!(percentiles(&mut v, &[50.0, 0.0, 100.0]), vec![2.5, 1.0, 4.0]);
        assert_eq!(median_f32(&mut v), 2.5);
    }
}
