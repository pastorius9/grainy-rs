//! AVX2 arithmetic measured by the detail and sky primitive goldens.
pub(crate) mod area;
pub(crate) mod bilateral;
pub(crate) mod deriv;
pub(crate) mod scaled_deriv;
mod sep;
use grainy_core::Image;
use ndarray::Array3;
/// A contiguous float32 image of `cn` interleaved channels (a numpy array's memory layout).
#[derive(Clone, Debug)]
pub(crate) struct Mat {
    pub h: usize,
    pub w: usize,
    pub cn: usize,
    pub data: Vec<f32>,
}
impl Mat {
    pub fn zeros(h: usize, w: usize, cn: usize) -> Self {
        Self { h, w, cn, data: vec![0.0; h * w * cn] }
    }

    pub fn row(&self, y: usize) -> &[f32] {
        let width = self.w * self.cn;
        &self.data[y * width..(y + 1) * width]
    }

    pub fn from_image(a: &Image) -> Self {
        let (h, w, cn) = a.dim();
        Self { h, w, cn, data: a.iter().copied().collect() }
    }

    pub fn into_image(self) -> Image {
        Array3::from_shape_vec((self.h, self.w, self.cn), self.data).unwrap_or_else(|_| Array3::zeros((0, 0, 0)))
    }
}

/// OpenCV's `borderInterpolate` for `BORDER_REFLECT_101` (`gfedcb|abcdefgh|gfedcba`).
pub(crate) fn reflect101(mut p: isize, len: usize) -> usize {
    let len = len as isize;
    if len <= 1 {
        return 0;
    }
    while p < 0 || p >= len {
        p = if p < 0 { -p } else { 2 * len - 2 - p };
    }
    p as usize
}
pub(crate) fn separable(src: &Mat, x: &[f32], y: &[f32]) -> Mat {
    let rows = if x.len() == 1 {
        let mut a = src.clone();
        for v in &mut a.data {
            *v *= x[0];
        }
        a
    } else {
        sep::row_pass(src, x)
    };
    if y.len() == 1 {
        let mut a = rows;
        for v in &mut a.data {
            *v *= y[0];
        }
        a
    } else {
        sep::column_pass(&rows, y)
    }
}
