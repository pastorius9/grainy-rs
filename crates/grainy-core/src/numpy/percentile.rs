//! NumPy percentiles: scalar q keeps float32, vector q produces float64.

/// The two sorted neighbours of the 'linear' virtual index `(n - 1) * q / 100` and its fraction; `None` for an
/// empty array or one with a NaN. Sorts `values`.
fn neighbours(values: &mut [f32], q: f64) -> Option<(f32, f32, f64)> {
    let n = values.len();
    if n == 0 || values.iter().any(|v| v.is_nan()) {
        return None;
    }
    if !values.is_sorted_by(|a, b| a <= b) {
        values.sort_unstable_by(f32::total_cmp);
    }
    let index = (n - 1) as f64 * (q.clamp(0.0, 100.0) / 100.0);
    let previous = index.floor();
    let t = index - previous;
    if !q.is_finite() {
        return None;
    }
    if index >= (n - 1) as f64 {
        return Some((values[n - 1], values[n - 1], t));
    }
    let lo = previous.max(0.0) as usize;
    Some((values[lo], values[lo + 1], t))
}

/// `np.percentile(values, [q, …])` (method 'linear', a list of `q`) of float32 values: numpy's `_lerp` with a
/// float64 fraction, float64 results. `values` is reordered. A NaN makes every result NaN, as numpy's.
pub fn percentiles(values: &mut [f32], qs: &[f64]) -> Vec<f64> {
    qs.iter()
        .map(|&q| match neighbours(values, q) {
            None => f64::NAN,
            Some((a, b, t)) => {
                let diff = f64::from(b - a);
                if t >= 0.5 {
                    f64::from(b) - diff * (1.0 - t)
                } else {
                    f64::from(a) + diff * t
                }
            }
        })
        .collect()
}

/// `np.percentile(values, q)` with a Python number `q`: numpy's weak promotion keeps the result float32 and
/// interpolates in float32.
pub fn percentile(values: &mut [f32], q: f64) -> f32 {
    match neighbours(values, q) {
        None => f32::NAN,
        Some((a, b, t)) => {
            let diff = b - a;
            if t >= 0.5 {
                b - diff * ((1.0 - t) as f32)
            } else {
                a + diff * (t as f32)
            }
        }
    }
}

/// `np.percentile(values, [q, …])` of float64 values.
pub fn percentiles_f64(values: &[f64], qs: &[f64]) -> Vec<f64> {
    let mut sorted = values.to_vec();
    if sorted.is_empty() || sorted.iter().any(|v| v.is_nan()) {
        return vec![f64::NAN; qs.len()];
    }
    sorted.sort_unstable_by(f64::total_cmp);
    let n = sorted.len();
    qs.iter()
        .map(|&q| {
            if !q.is_finite() {
                return f64::NAN;
            }
            let index = (n - 1) as f64 * (q.clamp(0.0, 100.0) / 100.0);
            let previous = index.floor();
            let t = index - previous;
            let (a, b) = if index >= (n - 1) as f64 {
                (sorted[n - 1], sorted[n - 1])
            } else {
                (sorted[previous as usize], sorted[previous as usize + 1])
            };
            let diff = b - a;
            if t >= 0.5 {
                b - diff * (1.0 - t)
            } else {
                a + diff * t
            }
        })
        .collect()
}

/// `np.median` of float32 values (the mean of the two middle values in float32 for an even count). Reorders.
pub fn median_f32(values: &mut [f32]) -> f32 {
    let n = values.len();
    if n == 0 {
        return f32::NAN;
    }
    values.sort_unstable_by(f32::total_cmp);
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// Index and weight of numpy's linear quantile: `(previous, next, gamma)` for `q` in 0..1 of `n` sorted values.
fn quantile_index(n: usize, q: f64) -> (usize, usize, f64) {
    let virtual_index = (n - 1) as f64 * q;
    let previous = virtual_index.floor();
    let gamma = virtual_index - previous;
    if virtual_index >= (n - 1) as f64 {
        (n - 1, n - 1, gamma)
    } else if virtual_index < 0.0 {
        (0, 0, gamma)
    } else {
        (previous as usize, previous as usize + 1, gamma)
    }
}
/// `np.percentile(a, p)` of an integer array (as float64).
pub fn percentile_i64(a: &mut [i64], p: f64) -> f64 {
    if a.is_empty() || !p.is_finite() || !(0.0..=100.0).contains(&p) {
        return f64::NAN;
    }
    a.sort_unstable();
    let (i, j, t) = quantile_index(a.len(), p / 100.0);
    let (lo, hi) = (a[i], a[j]);
    let diff = hi.wrapping_sub(lo) as f64;
    if t >= 0.5 {
        hi as f64 - diff * (1.0 - t)
    } else {
        lo as f64 + diff * t
    }
}
