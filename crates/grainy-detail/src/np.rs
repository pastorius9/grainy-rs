//! numpy's float semantics where the detail stage depends on them (private helpers).

pub(crate) use grainy_core::numpy::mean_f64;

/// `np.minimum` (NaN propagates).
pub(crate) fn minimum(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a < b {
        a
    } else {
        b
    }
}

/// `np.maximum` (NaN propagates).
pub(crate) fn maximum(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a > b {
        a
    } else {
        b
    }
}

/// `np.clip(v, lo, hi)` for float32 (NaN stays NaN).
pub(crate) fn clip(v: f32, lo: f32, hi: f32) -> f32 {
    minimum(maximum(v, lo), hi)
}

/// `.astype(np.int32)` of a float32 on x86-64: truncation, out-of-range and NaN give `i32::MIN`.
pub(crate) fn f32_to_i32(v: f32) -> i32 {
    if v.is_nan() || v >= 2_147_483_648.0 || v < -2_147_483_648.0 {
        i32::MIN
    } else {
        v as i32
    }
}

/// Rec. 709 luminance weights of the oracle (`np.array([.2126, .7152, .0722], np.float32)`).
pub(crate) const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub(crate) use grainy_core::numpy::dot3;
