//! NumPy's scalar semantics (NaN-propagating maximum/minimum/clip) and the two float types the oracle mixes.

use std::ops::{Add, Div, Mul, Sub};

use grainy_core::Image;
use ndarray::{ArrayView3, Axis};
use rayon::prelude::*;

pub(crate) use grainy_core::numpy::maximum;

pub(crate) use grainy_core::numpy::minimum;

pub(crate) use grainy_core::numpy::clip;

/// Python's `round(x)` for floats: halves go to the even neighbour.
pub(crate) fn round_half_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// float32 or float64 pixels: after process 3 Highlights/Shadows the oracle's tone stage computes in float64.
pub(crate) trait Float:
    Copy + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + Send + Sync
{
    const ZERO: Self;
    const ONE: Self;
    /// A Python float (or a float32 scalar) in this type, as NumPy converts a weak scalar.
    fn of(value: f64) -> Self;
    fn to_f64(self) -> f64;
    fn to_f32(self) -> f32;
}

impl Float for f32 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    #[inline]
    fn of(value: f64) -> Self {
        value as f32
    }
    #[inline]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
}

impl Float for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    #[inline]
    fn of(value: f64) -> Self {
        value
    }
    #[inline]
    fn to_f64(self) -> f64 {
        self
    }
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
}

/// A standard-layout copy of a view (bands of a standard array are already contiguous).
pub(crate) fn owned(a: ArrayView3<'_, f32>) -> Image {
    a.as_standard_layout().into_owned()
}

/// Calls `f(row_index, row)` for every row (a slice of width × channels values) in parallel.
pub(crate) fn rows_mut<T, F>(data: &mut [T], row_len: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    if row_len == 0 {
        return;
    }
    data.par_chunks_mut(row_len).enumerate().for_each(|(y, row)| f(y, row));
}

/// Applies `f` to every pixel (3 channels) of a standard-layout image in place, in parallel rows.
pub(crate) fn pixels_mut<F>(image: &mut Image, f: F)
where
    F: Fn(&mut [f32]) + Sync,
{
    let (_, width, channels) = image.dim();
    let row = width * channels;
    if let Some(data) = image.as_slice_mut() {
        rows_mut(data, row, |_, r| r.chunks_exact_mut(channels).for_each(&f));
    } else {
        image.axis_iter_mut(Axis(0)).into_par_iter().for_each(|mut r| {
            for mut pixel in r.outer_iter_mut() {
                let mut values: Vec<f32> = pixel.iter().copied().collect();
                f(&mut values);
                pixel.iter_mut().zip(values).for_each(|(slot, value)| *slot = value);
            }
        });
    }
}
