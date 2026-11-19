use crate::Pixel;
use ndarray::ArrayView3;

/// Port of cv2 BORDER_* extrapolation modes used by filters, remap, and morphology.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Border {
    /// BORDER_CONSTANT with the same value in every channel.
    Constant(f32),
    /// BORDER_CONSTANT with an OpenCV Scalar (up to four separate channel values).
    ConstantChannels([f32; 4]),
    /// BORDER_REPLICATE: repeat the edge pixel.
    Replicate,
    /// BORDER_REFLECT: reflect including the edge pixel.
    Reflect,
    /// BORDER_REFLECT_101 / BORDER_DEFAULT: reflect excluding the edge pixel.
    #[default]
    Reflect101,
    /// BORDER_WRAP: periodic coordinates (resampling and morphology only).
    Wrap,
}

pub(crate) fn index(p: i64, n: usize, border: Border) -> Option<usize> {
    let n = n as i64;
    if p >= 0 && p < n {
        return Some(p as usize);
    }
    match border {
        Border::Constant(_) | Border::ConstantChannels(_) => None,
        Border::Replicate => Some(p.clamp(0, n - 1) as usize),
        Border::Wrap => Some(p.rem_euclid(n) as usize),
        Border::Reflect => {
            let t = p.rem_euclid(n * 2);
            Some(if t < n { t } else { 2 * n - 1 - t } as usize)
        }
        Border::Reflect101 => {
            if n == 1 {
                return Some(0);
            }
            let t = p.rem_euclid(2 * n - 2);
            Some(if t < n { t } else { 2 * n - 2 - t } as usize)
        }
    }
}

pub(crate) fn constant(border: Border, c: usize) -> f32 {
    match border {
        Border::Constant(v) => v,
        Border::ConstantChannels(v) => v[c % 4],
        _ => 0.0,
    }
}

pub(crate) fn sample<T: Pixel>(a: ArrayView3<'_, T>, y: i64, x: i64, c: usize, b: Border) -> f32 {
    match (index(y, a.shape()[0], b), index(x, a.shape()[1], b)) {
        (Some(y), Some(x)) => a[[y, x, c]].float(),
        _ => T::from_float(constant(b, c)).float(),
    }
}
