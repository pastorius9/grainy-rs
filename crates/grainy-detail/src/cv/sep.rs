//! Adapters to the shared OpenCV filters.
use super::Mat;
use grainy_imgproc as cv;
#[cfg(test)]
pub(crate) fn gaussian_kernel(n: usize, s: f64) -> Vec<f32> {
    cv::get_gaussian_kernel(n, s).map(|a| a.to_vec()).unwrap_or_default()
}
pub(crate) fn gaussian_blur(src: &Mat, s: f64) -> Mat {
    let a = src.clone().into_image();
    Mat::from_image(&cv::gaussian_blur(&a, (0, 0), s, 0., cv::Border::Reflect101).unwrap_or(a))
}
pub(crate) fn sep_filter(src: &Mat, k: &[f32]) -> Mat {
    let a = src.clone().into_image();
    Mat::from_image(&cv::sep_filter_2d(&a, k, k, None, 0., cv::Border::Reflect101).unwrap_or(a))
}
