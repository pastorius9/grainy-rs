//! Golden tests of the private OpenCV ports against cv2 (`golden/detail/cv_*`).

use grainy_core::{Image, Plane};
use grainy_testkit::{assert_close, Case};
use ndarray::Array1;

use super::*;

const IMAGES: [&str; 3] = ["garden", "odd", "noise"];
const PLANES: [&str; 4] = ["odd", "L", "Lnoise", "Lodd"];

/// Checks an output stored as `{name}_bits` (bit sum of the full array) and `{name}_sub` (every third row and
/// column): bit-exact.
fn assert_exact(case: &Case, name: &str, got: &Image, what: &str) {
    let sub: Image = case.array(&format!("{name}_sub"));
    assert_close(&got.slice(ndarray::s![..;3, ..;3, ..]).to_owned(), &sub, 0.0, what);
    let bits = got.iter().fold(0u64, |acc, &v| acc.wrapping_add(f64::from(v).to_bits()));
    let expected: u64 = case.json()[format!("{name}_bits")].as_str().and_then(|v| v.parse().ok()).unwrap_or(0);
    assert_eq!(bits, expected, "{what}: full-array bit sum");
}

#[test]
fn gaussian_blur_is_exact() {
    for name in IMAGES {
        let case = Case::open("detail", &format!("cv_image_{name}"));
        let input: Image = case.array("input");
        let m = Mat::from_image(&input);
        for sigma in case.json()["sigmas"].as_array().cloned().unwrap_or_default() {
            let s = sigma.as_f64().unwrap_or(0.0);
            let kernel: Array1<f32> = case.array(&format!("kernel_{sigma}"));
            let n = kernel.len();
            if n > 1 {
                assert_eq!(sep::gaussian_kernel(n, s).as_slice(), kernel.as_slice().unwrap_or(&[]), "kernel {s}");
            }
            let got = gaussian_blur(&m, s.max(0.05)).into_image();
            assert_exact(&case, &format!("gauss_{sigma}"), &got, &format!("GaussianBlur {name} σ {s}"));
        }
    }
}

#[test]
fn box_filter_and_erode_are_exact() {
    for name in IMAGES {
        let case = Case::open("detail", &format!("cv_image_{name}"));
        let input: Image = case.array("input");
        let m = Mat::from_image(&input);
        for r in case.json()["radii"].as_array().cloned().unwrap_or_default() {
            let r = r.as_u64().unwrap_or(1) as usize;
            let got = box_filter(&m, 2 * r + 1).into_image();
            assert_exact(&case, &format!("box_{r}"), &got, &format!("boxFilter {name} r {r}"));
        }
        let minimum: Plane = case.array("min");
        for d in [3usize, 7] {
            assert_close(&erode(&minimum, d), &case.array(&format!("erode_{d}")), 0.0, &format!("erode {name} {d}"));
        }
    }
}

#[test]
fn plane_filters_are_exact() {
    for name in PLANES {
        let case = Case::open("detail", &format!("cv_plane_{name}"));
        let input: Plane = case.array("input");
        let m = Mat::from_plane(&input);
        for r in [1usize, 2, 7, 30] {
            let got = box_filter(&m, 2 * r + 1).into_plane();
            assert_close(&got, &case.array(&format!("box_{r}")), 0.0, &format!("{name} boxFilter r {r}"));
        }
        assert_close(&box_filter(&m, 5).into_plane(), &case.array("box55"), 0.0, &format!("{name} boxFilter 5x5"));
        assert_close(&median_blur5(&input), &case.array("median5"), 0.0, &format!("{name} medianBlur 5"));
        for level in 0..5 {
            let got = sep_filter(&m, &crate::noise::wavelet_kernel(level)).into_plane();
            assert_close(
                &got,
                &case.array(&format!("atrous_{level}")),
                0.0,
                &format!("{name} sepFilter2D level {level}"),
            );
        }
        assert_close(&laplacian(&input), &case.array("laplacian"), 0.0, &format!("{name} Laplacian"));
        assert_close(&sobel_x(&input), &case.array("sobel_x"), 0.0, &format!("{name} Sobel x"));
        assert_close(&sobel_y(&input), &case.array("sobel_y"), 0.0, &format!("{name} Sobel y"));
    }
}

/// OpenCV's own bilateral filter is reproduced exactly; the oracle's build replaces it with Intel IPP's, which
/// differs by a few float32 ulps of L* (0..100).
#[test]
fn bilateral_filter_matches() {
    for name in PLANES {
        let case = Case::open("detail", &format!("cv_plane_{name}"));
        let input: Plane = case.array("input");
        let params = case.json()["bilateral"].as_array().cloned().unwrap_or_default();
        for (i, p) in params.iter().enumerate() {
            let d = p[0].as_i64().unwrap_or(0) as i32;
            let (sc, ss) = (p[1].as_f64().unwrap_or(0.0), p[2].as_f64().unwrap_or(0.0));
            let got = bilateral_filter(&input, d, sc, ss);
            assert_close(
                &got,
                &case.array(&format!("bilateral_noipp_{i}")),
                0.0,
                &format!("{name} bilateral {i} (OpenCV)"),
            );
            assert_close(&got, &case.array(&format!("bilateral_{i}")), 1e-4, &format!("{name} bilateral {i} (IPP)"));
        }
    }
}
