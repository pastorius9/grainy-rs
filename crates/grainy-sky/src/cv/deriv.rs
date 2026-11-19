//! Adapters to shared OpenCV derivatives.
use super::Mat;
pub(crate) use grainy_imgproc::magnitude_values as magnitude;
pub(crate) fn sobel(src: &Mat, dx: bool, scale: f64) -> Mat {
    let a = ndarray::Array3::from_shape_vec((src.h, src.w, src.cn), src.data.clone()).unwrap_or_default();
    let a =
        grainy_imgproc::sobel(&a, usize::from(dx), usize::from(!dx), 3, scale, 0., grainy_imgproc::Border::Reflect101)
            .unwrap_or(a);
    let (h, w, cn) = a.dim();
    Mat { h, w, cn, data: a.iter().copied().collect() }
}
