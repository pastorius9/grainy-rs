//! NumPy reductions, preserving accumulator and division dtypes.

use std::ops::Add;

/// numpy's `pairwise_sum` (blocks of 128 with 8 accumulators, halves above), which `np.add.reduce` applies to a
/// contiguous run starting from zero.
pub fn pairwise<T: Copy + Add<Output = T> + Default>(a: &[T]) -> T {
    let n = a.len();
    if n < 8 {
        let mut res = T::default();
        for &v in a {
            res = res + v;
        }
        res
    } else if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] = r[j] + a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res = res + a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise(&a[..n2]) + pairwise(&a[n2..])
    }
}

const BUFFER: usize = 8192;

/// `a.mean(dtype=np.float64)` of a contiguous float32 array.
pub fn mean_f64(a: &[f32]) -> f64 {
    let mut total = 0f64;
    let mut chunk = Vec::with_capacity(BUFFER);
    for part in a.chunks(BUFFER) {
        chunk.clear();
        chunk.extend(part.iter().map(|&v| f64::from(v)));
        total += pairwise(&chunk);
    }
    total / a.len() as f64
}

/// The mean of float32 values as numpy computes `a.mean(axis=...)` over a non-inner axis: a sequential float32
/// sum, divided by the count in float64 (`true_divide` by an `intp`) and cast back to float32.
#[inline]
pub fn mean_of_sum(sum: f32, count: usize) -> f32 {
    (f64::from(sum) / count as f64) as f32
}

/// `np.mean` of a contiguous float32 array (pairwise sum, divided in float64, cast to float32).
pub fn mean_flat(a: &[f32]) -> f32 {
    mean_of_sum(pairwise(a), a.len())
}

/// `np.mean` of a float32 array: the pairwise sum divided by the count in float32.
pub fn mean_f32(a: &[f32]) -> f32 {
    pairwise(a) / a.len() as f32
}

/// `a.std()` of a contiguous float32 array, as numpy computes it: the mean (float32 sum divided by the count in
/// float64, stored as float32), float32 squared deviations, their float32 sum divided by the count in float64,
/// rounded to float32, float32 square root.
pub fn std(a: &[f32]) -> f32 {
    let count = a.len() as f64;
    let mean = (f64::from(pairwise(a)) / count) as f32;
    let squares: Vec<f32> = a
        .iter()
        .map(|&v| {
            let d = v - mean;
            d * d
        })
        .collect();
    let variance = (f64::from(pairwise(&squares)) / count) as f32;
    variance.sqrt()
}
