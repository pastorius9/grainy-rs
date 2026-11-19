//! Shared exact cv2.GaussianBlur implementation.
use ndarray::{Array2, ArrayView2};
pub(crate) fn gaussian_blur(src: ArrayView2<'_, f32>, sigma: f64) -> Array2<f32> {
    let a = src.to_owned();
    grainy_imgproc::gaussian_blur(&a, (0, 0), sigma, 0., grainy_imgproc::Border::Reflect101).unwrap_or(a)
}
#[cfg(test)]
mod tests {
    use super::gaussian_blur;
    use grainy_testkit::{assert_close, Case};
    use ndarray::Array2;

    #[test]
    fn matches_cv2_gaussian_blur() {
        for shape in ["37x53", "5x7", "64x96", "1x9"] {
            for sigma in ["0.3", "0.7", "1.125", "1.5", "4.5", "15.5"] {
                let case = Case::open("effects", &format!("blur_{shape}_{sigma}"));
                let input: Array2<f32> = case.array("input");
                let expected: Array2<f32> = case.array("output");
                let sigma = case.json()["sigma"].as_f64().expect("sigma");
                // Tolerance for OpenCV filter ports is 1/255; the port is far closer.
                assert_close(&gaussian_blur(input.view(), sigma), &expected, 1e-5, &format!("blur {shape} σ {sigma}"));
            }
        }
    }
}
