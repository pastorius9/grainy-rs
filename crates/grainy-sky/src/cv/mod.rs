//! Shape adapters to grainy-imgproc's exact OpenCV ports, checked by the sky cv_* goldens.

mod area;
mod boxf;
mod deriv;
mod hsv;
mod sep;

pub(crate) use area::resize_area;
pub(crate) use boxf::box_filter;
pub(crate) use deriv::{magnitude, sobel};
pub(crate) use hsv::rgb_to_hsv;
pub(crate) use sep::gaussian_blur;

/// A contiguous float32 image of `cn` interleaved channels (a numpy array's memory layout).
#[derive(Clone, Debug, PartialEq)]
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
}
