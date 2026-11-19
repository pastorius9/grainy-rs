//! `np.interp` (float64), as the point curve and the process 3 response use it.

/// Port of np.interp with default endpoint values and its one-point NaN behavior.
pub fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len().min(fp.len());
    if n == 0 {
        return f64::NAN;
    }
    if n == 1 {
        return fp[0];
    }
    interp_lr(x, &xp[..n], &fp[..n], fp[0], fp[n - 1])
}

/// Port of `np.interp(x, xp, fp, left, right)` for one value (numpy's `arr_interp`): the largest `j` with
/// `xp[j] <= x`, exact `fp[j]` on a sample, and the same NaN fallbacks.
pub fn interp_lr(x: f64, xp: &[f64], fp: &[f64], left: f64, right: f64) -> f64 {
    let n = xp.len().min(fp.len());
    if n == 0 {
        return f64::NAN;
    }
    if x.is_nan() {
        return x;
    }
    if x < xp[0] {
        return left;
    }
    if x > xp[n - 1] {
        return right;
    }
    let j = xp[..n].partition_point(|&v| v <= x).saturating_sub(1);
    if j == n - 1 || xp[j] == x {
        return fp[j];
    }
    let slope = (fp[j + 1] - fp[j]) / (xp[j + 1] - xp[j]);
    let mut result = slope * (x - xp[j]) + fp[j];
    if result.is_nan() {
        result = slope * (x - xp[j + 1]) + fp[j + 1];
        if result.is_nan() && fp[j] == fp[j + 1] {
            result = fp[j];
        }
    }
    result
}
