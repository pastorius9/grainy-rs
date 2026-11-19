//! numpy's float32 `exp` and `log` as it computes them on x86-64 with AVX2 and FMA3 (numpy's
//! `loops_exponent_log.dispatch.c.src`, `simd_exp_f32` / `simd_log_f32`): rational approximations with fused
//! multiply-adds. They are not correctly rounded (up to ~3 ulp), so `f32::exp`/`f32::ln` differ from numpy in a
//! third of all values; these give numpy's bits.

// The coefficients are numpy's C literals (npy_simd_data.h), rounded to float32 as C rounds them.
#![allow(clippy::excessive_precision)]

const LOG2E: f32 = std::f32::consts::LOG2_E;
const LOGE2: f32 = std::f32::consts::LN_2;
const SQRT1_2: f32 = std::f32::consts::FRAC_1_SQRT_2;
const CVT_MAGIC: f32 = 12582912.0; // 0x1.8p+23
const CODY_WAITE_HIGH: f32 = -6.931_457_5e-1;
const CODY_WAITE_LOW: f32 = -1.428_606_8e-6;
const EXP_P: [f32; 6] = [
    9.999_999_999_980_87e-1,
    7.257_664_613_233_124e-1,
    2.473_615_434_895_521e-1,
    5.114_512_081_637_298e-2,
    6.757_896_990_527_505e-3,
    5.082_762_527_590_694e-4,
];
const EXP_Q: [f32; 3] = [1.0, -2.742_335_390_411_667_5e-1, 2.159_509_375_685_83e-2];
const LOG_P: [f32; 6] = [
    0.0,
    9.999_999_999_999_999e-1,
    2.112_677_543_073_053,
    1.480_000_633_576_506_6,
    3.808_837_741_388_408e-1,
    2.589_979_117_907_922_7e-2,
];
const LOG_Q: [f32; 6] = [
    1.0,
    2.612_677_543_073_109_3,
    2.453_006_071_784_736_4,
    9.864_942_958_519_419e-1,
    1.546_476_374_983_906_7e-1,
    5.875_095_403_124_574e-3,
];

/// poly · 2^quadrant as numpy's `fma_scalef_ps`: the quadrant added to the exponent bits, results below
/// 2^-125 through a division.
fn scale(poly: f32, quadrant: f32) -> f32 {
    let add = |p: f32, q: f32| f32::from_bits((p.to_bits() as i32).wrapping_add((q as i32) << 23) as u32);
    if quadrant <= -125.0 {
        let diff = -(quadrant + 125.0);
        let power = 1i32.checked_shl(diff as u32).unwrap_or(0);
        add(poly, quadrant.max(-125.0)) / power as f32
    } else {
        add(poly, quadrant)
    }
}

/// numpy's float32 `np.exp`.
pub fn exp(x: f32) -> f32 {
    const XMAX: f32 = 88.722_84;
    const XMIN: f32 = -103.972_08;
    if x.is_nan() {
        return f32::NAN;
    }
    if x >= XMAX {
        return f32::INFINITY;
    }
    if x <= XMIN {
        return 0.0;
    }
    let mut quadrant = x * LOG2E;
    quadrant = (quadrant + CVT_MAGIC) - CVT_MAGIC;
    let mut r = quadrant.mul_add(CODY_WAITE_HIGH, x);
    r = quadrant.mul_add(CODY_WAITE_LOW, r);
    r = quadrant.mul_add(0.0, r);
    let mut num = EXP_P[5].mul_add(r, EXP_P[4]);
    num = num.mul_add(r, EXP_P[3]);
    num = num.mul_add(r, EXP_P[2]);
    num = num.mul_add(r, EXP_P[1]);
    num = num.mul_add(r, EXP_P[0]);
    let mut den = EXP_Q[2].mul_add(r, EXP_Q[1]);
    den = den.mul_add(r, EXP_Q[0]);
    scale(num / den, quadrant)
}

/// numpy's float32 `np.log`.
pub fn ln(x: f32) -> f32 {
    if x.is_nan() {
        return f32::NAN;
    }
    if x < 0.0 {
        return -f32::NAN;
    }
    if x == 0.0 {
        return f32::NEG_INFINITY;
    }
    if x == f32::INFINITY {
        return f32::INFINITY;
    }
    // exponent and mantissa in [0.5, 1), denormals scaled up by 2^100 first
    let (bits, offset) =
        if x < f32::MIN_POSITIVE { ((x * f32::from_bits(0x7180_0000)).to_bits(), 100.0) } else { (x.to_bits(), 0.0) };
    let mut exponent = ((bits >> 23) as i32 - 0x7e) as f32 - offset;
    let mut m = f32::from_bits((bits & 0x007f_ffff) | (126 << 23));
    if m <= SQRT1_2 {
        m += m;
        exponent -= 1.0;
    }
    let r = m - 1.0;
    let mut num = LOG_P[5].mul_add(r, LOG_P[4]);
    num = num.mul_add(r, LOG_P[3]);
    num = num.mul_add(r, LOG_P[2]);
    num = num.mul_add(r, LOG_P[1]);
    num = num.mul_add(r, LOG_P[0]);
    let mut den = LOG_Q[5].mul_add(r, LOG_Q[4]);
    den = den.mul_add(r, LOG_Q[3]);
    den = den.mul_add(r, LOG_Q[2]);
    den = den.mul_add(r, LOG_Q[1]);
    den = den.mul_add(r, LOG_Q[0]);
    exponent.mul_add(LOGE2, num / den)
}
