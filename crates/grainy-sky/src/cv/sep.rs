//! Adapter to shared OpenCV filter.
use super::Mat;
use grainy_imgproc as cv;
pub(crate) fn gaussian_blur(src: &Mat, sigma: f64) -> Mat {
    let a = ndarray::Array3::from_shape_vec((src.h, src.w, src.cn), src.data.clone()).unwrap_or_default();
    let a = cv::gaussian_blur(&a, (0, 0), sigma, 0., cv::Border::Reflect101).unwrap_or(a);
    Mat { h: src.h, w: src.w, cn: src.cn, data: a.iter().copied().collect() }
}
