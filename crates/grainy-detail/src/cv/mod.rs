//! Shape adapters to grainy-imgproc's exact OpenCV ports, checked by the detail cv_* goldens.

mod sep;

use cv::Border;
use grainy_core::{Image, Plane};
use grainy_imgproc as cv;
use ndarray::{Array2, Array3};

pub(crate) use sep::{gaussian_blur, sep_filter};

/// A contiguous float32 image of `cn` interleaved channels (a numpy array's memory layout).
#[derive(Clone, Debug)]
pub(crate) struct Mat {
    pub h: usize,
    pub w: usize,
    pub cn: usize,
    pub data: Vec<f32>,
}

impl Mat {
    pub fn from_image(a: &Image) -> Self {
        let (h, w, cn) = a.dim();
        Self { h, w, cn, data: a.iter().copied().collect() }
    }

    pub fn from_plane(a: &Plane) -> Self {
        let (h, w) = a.dim();
        Self { h, w, cn: 1, data: a.iter().copied().collect() }
    }

    pub fn into_image(self) -> Image {
        Array3::from_shape_vec((self.h, self.w, self.cn), self.data).unwrap_or_else(|_| Array3::zeros((0, 0, 0)))
    }

    pub fn into_plane(self) -> Plane {
        Array2::from_shape_vec((self.h, self.w * self.cn), self.data).unwrap_or_else(|_| Array2::zeros((0, 0)))
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn box_filter(src: &Mat, k: usize) -> Mat {
    let a = src.clone().into_image();
    Mat::from_image(&cv::box_filter(&a, (k, k), None, true, Border::Reflect101).unwrap_or(a))
}
pub(crate) fn bilateral_filter(src: &Plane, d: i32, color: f64, space: f64) -> Plane {
    cv::bilateral_filter(src, d, color, space, Border::Reflect101).unwrap_or_else(|_| src.clone())
}
pub(crate) fn median_blur5(src: &Plane) -> Plane {
    cv::median_blur(src, 5).unwrap_or_else(|_| src.clone())
}
pub(crate) fn laplacian(src: &Plane) -> Plane {
    cv::laplacian(src, Border::Reflect101).unwrap_or_else(|_| src.clone())
}
pub(crate) fn sobel_x(src: &Plane) -> Plane {
    cv::sobel(src, 1, 0, 3, 1., 0., Border::Reflect101).unwrap_or_else(|_| src.clone())
}
pub(crate) fn sobel_y(src: &Plane) -> Plane {
    cv::sobel(src, 0, 1, 3, 1., 0., Border::Reflect101).unwrap_or_else(|_| src.clone())
}
pub(crate) fn erode(src: &Plane, d: usize) -> Plane {
    cv::erode(src, &ndarray::Array2::ones((d, d)), None, 1, None).unwrap_or_else(|_| src.clone())
}
