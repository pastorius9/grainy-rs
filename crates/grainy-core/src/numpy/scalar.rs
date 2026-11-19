//! NumPy scalar operations and the measured matmul accumulation orders.

/// A Python float constant as numpy casts it to float32 (`float64 -> float32`, which can differ from parsing the
/// decimal straight to `f32`).
#[inline]
pub const fn f(x: f64) -> f32 {
    x as f32
}

/// `pixel @ weights` for a float32 pixel, as numpy's matmul of an image with a 3-vector computes it
/// (sequential, unfused: `(r*w0 + g*w1) + b*w2`).
#[inline]
pub fn dot3(p: &[f32], w: &[f32; 3]) -> f32 {
    if p.len() < 3 {
        return f32::NAN;
    }
    p[0] * w[0] + p[1] * w[1] + p[2] * w[2]
}

/// One output channel of `pixel @ matrix.T` as numpy's BLAS (sgemm) computes it: `fma(b, m2, fma(g, m1, r*m0))`.
#[inline]
pub fn gemm3(p: &[f32], row: &[f32; 3]) -> f32 {
    if p.len() < 3 {
        return f32::NAN;
    }
    p[2].mul_add(row[2], p[1].mul_add(row[1], p[0] * row[0]))
}

/// numpy's float32 remainder (`a % b`, the sign of the divisor), `npy_divmodf`.
#[inline]
pub fn py_mod(a: f32, b: f32) -> f32 {
    let m = a % b;
    if b == 0.0 {
        return m;
    }
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m + b
        } else {
            m
        }
    } else {
        0f32.copysign(b)
    }
}

/// numpy's float64 remainder.
#[inline]
pub fn py_mod64(a: f64, b: f64) -> f64 {
    let m = a % b;
    if b == 0.0 {
        return m;
    }
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m + b
        } else {
            m
        }
    } else {
        0f64.copysign(b)
    }
}

/// `np.clip(x, 0, 1)` (NaN stays NaN).
#[inline]
pub fn clip01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// `np.maximum` (NaN wins).
#[inline]
pub fn maximum<T: PartialOrd + Copy>(a: T, b: T) -> T {
    #[allow(clippy::eq_op)]
    let a_nan = a != a;
    if a_nan || a >= b {
        a
    } else {
        b
    }
}

/// `np.minimum` (NaN wins).
#[inline]
pub fn minimum<T: PartialOrd + Copy>(a: T, b: T) -> T {
    #[allow(clippy::eq_op)]
    let a_nan = a != a;
    if a_nan || a <= b {
        a
    } else {
        b
    }
}

/// Port of `np.clip`: NaNs propagate, including NaN bounds.
#[inline]
pub fn clip<T: PartialOrd + Copy>(x: T, low: T, high: T) -> T {
    minimum(maximum(x, low), high)
}
