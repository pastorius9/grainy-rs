//! Exact OpenCV filtering shared by all engine stages.
use grainy_core::Plane;
pub(crate) fn blur(a: &Plane, sigma: f64) -> crate::Result<Plane> {
    Ok(grainy_imgproc::gaussian_blur(a, (0, 0), sigma, 0., grainy_imgproc::Border::Reflect101)?)
}
