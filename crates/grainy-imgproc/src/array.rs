use ndarray::{Array, Array3, ArrayView3, Axis, Dimension, Ix2, Ix3};

use crate::{Error, Result};

mod sealed {
    pub trait Sample: Copy + Default + Send + Sync + PartialOrd + 'static {
        const IS_U8: bool;
        fn float(self) -> f32;
        fn from_float(value: f32) -> Self;
    }
    impl Sample for f32 {
        const IS_U8: bool = false;
        fn float(self) -> f32 {
            self
        }
        fn from_float(value: f32) -> Self {
            value
        }
    }
    impl Sample for u8 {
        const IS_U8: bool = true;
        fn float(self) -> f32 {
            self as f32
        }
        fn from_float(value: f32) -> Self {
            value.round_ties_even().clamp(0.0, 255.0) as u8
        }
    }
}

/// The CV_32F and CV_8U sample types used by the ports of cv2 image operations.
/// Sealed to keep conversion and saturation semantics consistent with OpenCV.
pub trait Pixel: sealed::Sample {}
impl Pixel for f32 {}
impl Pixel for u8 {}

pub(crate) fn view<T, D: Dimension>(a: &Array<T, D>) -> Result<ArrayView3<'_, T>> {
    if a.is_empty() || a.shape().iter().any(|&n| n > i32::MAX as usize) {
        return Err(Error::Shape("nonempty dimensions no larger than i32::MAX required"));
    }
    match a.ndim() {
        2 => a.view().into_dimensionality::<Ix2>().map(|v| v.insert_axis(Axis(2))),
        3 => a.view().into_dimensionality::<Ix3>(),
        _ => return Err(Error::Shape("expected HxW or HxWxC")),
    }
    .map_err(|_| Error::Shape("expected HxW or HxWxC"))
}

pub(crate) fn finish<T, D: Dimension>(a: Array3<T>, rank: usize) -> Result<Array<T, D>> {
    if rank == 2 { a.index_axis_move(Axis(2), 0).into_dimensionality::<D>() } else { a.into_dimensionality::<D>() }
        .map_err(|_| Error::Shape("output rank differs from input"))
}

pub(crate) fn shape(h: usize, w: usize, c: usize) -> Result<()> {
    if h == 0
        || w == 0
        || c == 0
        || h > i32::MAX as usize
        || w > i32::MAX as usize
        || h.checked_mul(w).and_then(|n| n.checked_mul(c)).filter(|&n| n <= isize::MAX as usize / 8).is_none()
    {
        return Err(Error::Shape("invalid or overflowing output dimensions"));
    }
    Ok(())
}

pub(crate) fn anchor(size: (usize, usize), at: Option<(usize, usize)>) -> Result<(usize, usize)> {
    shape(size.1, size.0, 1)?;
    let a = at.unwrap_or((size.0 / 2, size.1 / 2));
    if a.0 >= size.0 || a.1 >= size.1 {
        return Err(Error::Argument("anchor outside kernel"));
    }
    Ok(a)
}

pub(crate) fn generate<T: Default + Clone + Send, F>(h: usize, w: usize, c: usize, f: F) -> Array3<T>
where
    F: Fn(usize, usize, usize) -> T + Sync,
{
    let bands = grainy_core::parallel::bands(h, w, |first, last| {
        let mut out = Array3::default((last - first, w, c));
        for ((y, x, ch), value) in out.indexed_iter_mut() {
            *value = f(y + first, x, ch);
        }
        out
    });
    let mut out = Array3::default((h, w, c));
    let mut first = 0;
    for band in bands {
        let last = first + band.shape()[0];
        out.slice_mut(ndarray::s![first..last, .., ..]).assign(&band);
        first = last;
    }
    out
}
