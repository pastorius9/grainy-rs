//! Adapter to shared cv2.COLOR_RGB2HSV.
use super::Mat;
pub(crate) fn rgb_to_hsv(src: &Mat) -> Mat {
    let a = ndarray::Array3::from_shape_vec((src.h, src.w, src.cn), src.data.clone()).unwrap_or_default();
    let a = grainy_imgproc::cvt_color(&a, grainy_imgproc::ColorConversion::Rgb2Hsv).unwrap_or(a);
    let (h, w, cn) = a.dim();
    Mat { h, w, cn, data: a.iter().copied().collect() }
}
