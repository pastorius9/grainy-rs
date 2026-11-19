//! numpy semantics the oracle relies on (float32 arithmetic, NaN handling, reductions, `linspace`).

/// Rec. 709 luminance weights (`np.array([.2126, .7152, .0722], np.float32)`).
pub(crate) const LUMA: [f32; 3] = [0.2126_f64 as f32, 0.7152_f64 as f32, 0.0722_f64 as f32];

pub(crate) use grainy_core::numpy::dot3;

/// `np.sum(d * d, axis=-1)` of three values (sequential).
#[inline]
pub(crate) fn sq3(d: [f32; 3]) -> f32 {
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// `np.linalg.norm(d, axis=-1)` of three float32 values.
#[inline]
pub(crate) fn norm3(d: [f32; 3]) -> f32 {
    sq3(d).sqrt()
}

/// `np.clip` (NaN stays NaN).
#[inline]
pub(crate) fn clip<T: PartialOrd + Copy>(x: T, lo: T, hi: T) -> T {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

pub(crate) use grainy_core::numpy::maximum;

pub(crate) use grainy_core::numpy::minimum;

/// Python's `max(a, b)`: `b` only when it is strictly larger (so `max(1, nan)` is 1).
#[inline]
pub(crate) fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// Python's `int(x)` of a float (truncation), or `None` for NaN and infinities (the oracle raises).
#[inline]
pub(crate) fn py_int(x: f64) -> Option<i64> {
    x.is_finite().then(|| x.trunc() as i64)
}

pub(crate) use grainy_core::numpy::mean_of_sum;

#[cfg(test)]
use grainy_core::numpy::pairwise as pairwise_sum;

pub(crate) use grainy_core::numpy::mean_flat;

pub(crate) use grainy_core::numpy::linspace_vector as linspace;

pub(crate) use grainy_core::numpy::linspace_int;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linspace_matches_numpy() {
        assert_eq!(linspace([0.0], [10.0], 5), vec![[0.0], [2.5], [5.0], [7.5], [10.0]]);
        assert_eq!(linspace_int(11, 5), vec![0, 2, 5, 7, 10]);
        assert_eq!(linspace([1.0, 2.0], [3.0, 2.0], 3), vec![[1.0, 2.0], [2.0, 2.0], [3.0, 2.0]]);
        assert_eq!(linspace([0.0], [0.0], 1), vec![[0.0]]);
    }

    #[test]
    fn pairwise_sums_short_runs_sequentially() {
        assert_eq!(pairwise_sum(&[1.0, 2.0, 3.0]), 6.0);
        let v: Vec<f32> = (0..300).map(|i| i as f32 * 0.1).collect();
        assert!((pairwise_sum(&v) - v.iter().sum::<f32>()).abs() < 1e-2);
    }
}
