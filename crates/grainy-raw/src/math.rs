use crate::{invalid, Result};
pub(crate) type Mat = [[f64; 3]; 3];
pub(crate) const ID: Mat = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
pub(crate) fn mv(a: Mat, b: [f64; 3]) -> [f64; 3] {
    grainy_core::numpy::numeric::mulv3(&a, b)
}
pub(crate) fn mm(a: Mat, b: Mat) -> Mat {
    grainy_core::numpy::numeric::mul3(&a, &b)
}
pub(crate) fn diag(v: [f64; 3]) -> Mat {
    std::array::from_fn(|i| std::array::from_fn(|j| if i == j { v[i] } else { 0. }))
}
pub(crate) fn inverse(m: Mat) -> Result<Mat> {
    let mut a = m;
    let mut b = ID;
    if a.iter().flatten().any(|v| !v.is_finite()) {
        return Err(invalid("nonfinite matrix"));
    }
    for k in 0..3 {
        let mut p = k;
        for i in k + 1..3 {
            if a[i][k].abs() > a[p][k].abs() {
                p = i;
            }
        }
        if a[p][k].abs() < 1e-300 {
            return Err(invalid("singular matrix"));
        }
        a.swap(k, p);
        b.swap(k, p);
        let s = a[k][k];
        for j in 0..3 {
            a[k][j] /= s;
            b[k][j] /= s;
        }
        for i in 0..3 {
            if i != k {
                let s = a[i][k];
                for j in 0..3 {
                    a[i][j] -= s * a[k][j];
                    b[i][j] -= s * b[k][j];
                }
            }
        }
    }
    Ok(b)
}
pub(crate) fn valid_matrix(a: Mat, bounded: bool) -> Result<Mat> {
    if a.iter().flatten().any(|v| !v.is_finite() || (bounded && v.abs() > 100.)) {
        return Err(invalid("invalid camera matrix"));
    }
    // One-sided Jacobi SVD: the same 2-norm condition limit as numpy.linalg.cond.
    let mut b = a;
    for _ in 0..32 {
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            let x = (0..3).map(|i| b[i][p] * b[i][p]).sum::<f64>();
            let y = (0..3).map(|i| b[i][q] * b[i][q]).sum::<f64>();
            let z = (0..3).map(|i| b[i][p] * b[i][q]).sum::<f64>();
            if z.abs() <= 1e-16 * (x * y).sqrt() {
                continue;
            }
            let tau = (y - x) / (2. * z);
            let t = tau.signum() / (tau.abs() + (1. + tau * tau).sqrt());
            let t = if tau == 0. { 1. } else { t };
            let c = 1. / (1. + t * t).sqrt();
            let s = c * t;
            for row in &mut b {
                let u = row[p];
                let v = row[q];
                row[p] = c * u - s * v;
                row[q] = s * u + c * v;
            }
        }
    }
    let norms: [f64; 3] = std::array::from_fn(|j| (0..3).map(|i| b[i][j] * b[i][j]).sum::<f64>().sqrt());
    let min = norms.iter().copied().fold(f64::INFINITY, f64::min);
    let max = norms.iter().copied().fold(0., f64::max);
    if min == 0. || max / min > 1e6 {
        return Err(invalid("ill-conditioned camera matrix"));
    }
    Ok(a)
}
pub(crate) fn image(a: &grainy_core::Image) -> Result<()> {
    if a.shape()[2] != 3 || a.iter().any(|v| !v.is_finite()) {
        return Err(invalid("expected finite H x W x 3 image"));
    }
    Ok(())
}
pub(crate) fn map(a: &grainy_core::Image, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) -> grainy_core::Image {
    grainy_core::parallel::map_rows(a, |src, mut dst| {
        for (mut o, i) in dst.outer_iter_mut().zip(src.outer_iter()) {
            for (mut o, i) in o.outer_iter_mut().zip(i.outer_iter()) {
                let p = f([i[0], i[1], i[2]]);
                for c in 0..3 {
                    o[c] = p[c];
                }
            }
        }
    })
}
