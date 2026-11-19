//! Adapter to shared cv2.INTER_AREA.
use super::Mat;
pub(crate) fn resize_area(src: &Mat, w: usize, h: usize) -> Mat {
    let a = ndarray::Array3::from_shape_vec((src.h, src.w, src.cn), src.data.clone()).unwrap_or_default();
    let a = grainy_imgproc::resize(&a, (w, h), grainy_imgproc::Interpolation::Area).unwrap_or(a);
    let (h, w, cn) = a.dim();
    Mat { h, w, cn, data: a.iter().copied().collect() }
}
