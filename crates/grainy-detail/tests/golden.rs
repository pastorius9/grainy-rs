//! Golden tests of grainy-detail against the Python oracle (`tools/golden/gen_detail.py`, `golden/detail/`).
//!
//! Tolerances: 0 (bit-exact) everywhere the oracle runs OpenCV's own filters; luminance noise reduction of
//! detail_version 1 and 2 goes through the oracle's Intel IPP bilateral filter (a few float32 ulps of L*, see
//! `IPP`); grain goes through grainy-effects' grain field (`GRAIN`).

use std::path::Path;

use grainy_core::{Image, Plane, Settings};
use grainy_detail as detail;
use grainy_testkit::{assert_close, max_abs_diff, Case};
use ndarray::{s, Array1, Array2, Array3, Axis};
use serde_json::Value;

/// Cases whose L* went through the IPP bilateral filter (sRGB output).
const IPP: f32 = 2e-5;
/// Cases with grain (grainy-effects' grain field; see its own tolerance).
const GRAIN: f32 = 1e-5;
/// Noise reduction converts to CIELAB and back with the C runtime's `cbrtf` and `powf`; where that is not the
/// goldens' C runtime (macOS) they round some values differently in the last place (`cbrtf` of the Microsoft C
/// runtime is up to 2 ulp from the correctly rounded value). L*a*b* itself (a* = 500 (fx - fy)): measured on
/// macOS (CI run 5) 9.16e-5 (`noise`), 8.96e-5 (`colours`), 5.96e-5 (`garden`); tolerance 2e-4, about 2x.
const OTHER_MATHS_LAB: f32 = 2e-4;
/// The RGB results of `develop_detail` with noise reduction on macOS: measured 3.81e-6 (CI run 5); tolerance
/// 8e-6, about 2x.
const OTHER_MATHS_STAGES: f32 = 8e-6;
/// The RGB results of `detail_tools` with noise reduction on macOS: measured 1.155e-5 (CI run 5, over these cases
/// together with the IPP ones); tolerance 2e-5, about 1.7x. The HDR and large-frame stages, whose errors run 5
/// did not print, keep this value too.
const OTHER_MATHS_TOOLS: f32 = 2e-5;

fn inputs() -> Case {
    Case::open("detail", "inputs")
}

fn image(name: &str) -> Image {
    inputs().array(name)
}

fn settings(case: &Case) -> Settings {
    Settings::from_value(case.json()["settings"].clone()).expect("settings object")
}

fn cases(prefix: &str) -> Vec<String> {
    let folder = grainy_testkit::root().join("golden").join("detail");
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .expect("golden/detail")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && Path::new(&folder.join(n).join("case.json")).exists())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no golden cases {prefix}*");
    names
}

/// Checks an output stored as `{name}_bits` (bit sum of the full array) and `{name}_sub` (every third row and
/// column): bit-exact.
fn assert_exact(case: &Case, name: &str, got: &Image, what: &str) {
    let sub: Image = case.array(&format!("{name}_sub"));
    assert_close(&got.slice(ndarray::s![..;3, ..;3, ..]).to_owned(), &sub, 0.0, what);
    let bits = got.iter().fold(0u64, |acc, &v| acc.wrapping_add(f64::from(v).to_bits()));
    let expected: u64 = case.json()[format!("{name}_bits")].as_str().and_then(|v| v.parse().ok()).unwrap_or(0);
    assert_eq!(bits, expected, "{what}: full-array bit sum");
}

/// Wrapping sum of the float64 bit patterns (the generator's `bits_sum`).
fn bits_sum(a: &Image) -> u64 {
    a.iter().fold(0u64, |acc, &v| acc.wrapping_add(f64::from(v).to_bits()))
}

/// The tolerance of a stage or tools case from its settings; `other_maths` applies to noise reduction where the
/// C runtime is not the goldens'.
fn tolerance(s: &Settings, other_maths: f32) -> f32 {
    let mut t = 0f32;
    let version = s.0.get("detail_version").and_then(Value::as_i64).unwrap_or(1);
    if s.f64("noise_luma") != 0.0 && version < 3 {
        t = t.max(IPP);
    }
    if s.f64("grain") != 0.0 {
        t = t.max(GRAIN);
    }
    if noise_reduction(s) {
        t = t.max(grainy_testkit::maths_tolerance(0.0, other_maths));
    }
    t
}

/// Whether a case runs noise reduction (through CIELAB).
fn noise_reduction(s: &Settings) -> bool {
    s.f64("noise_luma") != 0.0 || s.f64("noise_color") != 0.0
}

/// Compares a result with a case's `output` (full array) or `bits_sum` + `output_sub` (large outputs).
fn check(case: &Case, got: &Image, tol: f32, what: &str) -> f32 {
    if case.has("output") {
        let expected: Image = case.array("output");
        assert_close(got, &expected, tol, what);
        return max_abs_diff(got, &expected).0;
    }
    let sub: Image = case.array("output_sub");
    let (step_y, step_x) = (case.json()["step"][0].as_u64().unwrap_or(1), case.json()["step"][1].as_u64().unwrap_or(1));
    let view = got.slice(s![..;step_y as isize, ..;step_x as isize, ..]).to_owned();
    assert_close(&view, &sub, tol, &format!("{what} (subsampled)"));
    if tol == 0.0 {
        let expected: u64 = case.json()["bits_sum"].as_str().and_then(|v| v.parse().ok()).unwrap_or(0);
        assert_eq!(bits_sum(got), expected, "{what}: full-frame bit sum");
    }
    max_abs_diff(&view, &sub).0
}

#[test]
fn noise_parameters() {
    let case = Case::open("detail", "noise_params");
    for row in case.json()["rows"].as_array().cloned().unwrap_or_default() {
        let v: Vec<f64> =
            row.as_array().map(|r| r.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect()).unwrap_or_default();
        let (amount, ps) = (v[0], v[1]);
        let (r, eps, ceps, blend) = detail::chroma_guided_params(amount, ps);
        assert_eq!(r as f64, v[2], "radius {amount} {ps}");
        assert_eq!(f64::from(eps), v[3], "eps {amount} {ps}");
        assert_eq!(f64::from(ceps), v[4], "chroma eps {amount} {ps}");
        assert_eq!(f64::from(blend), v[5], "blend {amount} {ps}");
        assert_eq!(f64::from(detail::wavelet_lambda(amount, ps)), v[6], "lambda {amount} {ps}");
    }
    for (level, k) in case.json()["kernels"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        let expected: Vec<f32> =
            k.as_array().map(|r| r.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default();
        assert_eq!(detail::wavelet_kernel(level as u32), expected, "kernel {level}");
    }
}

#[test]
fn noise_reduction_family() {
    for name in ["garden", "noise", "colours"] {
        let case = Case::open("detail", &format!("noise_{name}"));
        let lab: Image = case.array("lab");
        let tolerance = grainy_testkit::maths_tolerance(0.0, OTHER_MATHS_LAB);
        let ours = grainy_color::rgb_to_lab(&image(name));
        let error = max_abs_diff(&ours, &lab).0;
        grainy_testkit::report_maths(&format!("detail rgb_to_lab {name}"), f64::from(error), f64::from(tolerance));
        assert_close(&ours, &lab, tolerance, &format!("rgb_to_lab {name}"));
        let lightness: Plane = lab.index_axis(Axis(2), 0).to_owned();
        let profiles: Array2<f32> = case.array("profiles");
        let got = detail::wavelet_profiles(&lightness);
        let got = Array2::from_shape_fn((got.len(), 8), |(i, j)| got[i][j]);
        assert_close(&got, &profiles, 0.0, &format!("wavelet_profiles {name}"));
        let smooth = detail::atrous_smooth(&lightness, 0);
        let p0 = Array1::from(detail::noise_profile(&(&lightness - &smooth), &smooth).to_vec());
        assert_close(&p0, &case.array::<f32, _>("profile0"), 0.0, &format!("_noise_profile {name}"));
        for amount in [25.0, 70.0, 100.0] {
            for ps in [1.0, 0.25] {
                let tag = format!("{amount}_{ps:?}");
                let got = detail::luma_wavelet(&lightness, amount, ps, None);
                assert_close(
                    &got,
                    &case.array(&format!("wavelet_{}_{ps:?}", amount as i64)),
                    0.0,
                    &format!("luma_wavelet {name} {tag}"),
                );
                let mut chroma = lab.clone();
                detail::chroma_guided(&mut chroma, amount, ps);
                assert_exact(
                    &case,
                    &format!("chroma_{}_{ps:?}", amount as i64),
                    &chroma,
                    &format!("chroma_guided {name} {tag}"),
                );
            }
            for ps in [1.0, 0.5, 0.25] {
                let got = detail::luma_noise_reduction(&lightness, amount, ps);
                // L* 0..100 through IPP's bilateral filter.
                assert_close(
                    &got,
                    &case.array(&format!("bilateral_{}_{ps:?}", amount as i64)),
                    1e-4,
                    &format!("luma_noise_reduction {name} {amount} {ps}"),
                );
            }
        }
        if case.has("given_profiles") {
            let given: Array2<f32> = case.array("given_profiles");
            let given: Vec<[f32; 8]> = given.rows().into_iter().map(|r| std::array::from_fn(|j| r[j])).collect();
            let got = detail::luma_wavelet(&lightness, 60.0, 1.0, Some(&given));
            assert_close(&got, &case.array("wavelet_given"), 0.0, &format!("luma_wavelet {name} given profiles"));
        }
    }
}

#[test]
fn detail_tools_cases() {
    let mut worst = 0f32;
    let mut noise_worst = 0f32;
    let mut checks = grainy_testkit::Checks::default();
    for name in cases("tools_") {
        let case = Case::open("detail", &name);
        let s = settings(&case);
        let ps = case.json()["pixel_scale"].as_f64().unwrap_or(1.0);
        let input = image(case.json()["image"].as_str().unwrap_or(""));
        let got = detail::detail_tools(&input, &s, ps);
        checks.case(&name, || {
            let error = check(&case, &got, tolerance(&s, OTHER_MATHS_TOOLS), &name);
            worst = worst.max(error);
            if noise_reduction(&s) {
                noise_worst = noise_worst.max(error);
            }
        });
    }
    println!("detail_tools: worst {worst:e}");
    grainy_testkit::report_maths("detail tools with noise reduction", f64::from(noise_worst), OTHER_MATHS_TOOLS.into());
    checks.finish();
}

#[test]
fn sharpen_cases() {
    for name in cases("sharpen_") {
        let case = Case::open("detail", &name);
        let j = case.json();
        let f = |k: &str| j[k].as_f64().unwrap_or(0.0);
        let input = image(j["image"].as_str().unwrap_or(""));
        let got = detail::sharpen(
            &input,
            f("amount"),
            f("radius"),
            f("detail"),
            f("masking"),
            f("pixel_scale"),
            j["luminance"].as_bool().unwrap_or(false),
        );
        check(&case, &got, 0.0, &name);
    }
}

#[test]
fn stage_cases() {
    let mut worst = 0f32;
    let mut noise_worst = 0f32;
    let mut checks = grainy_testkit::Checks::default();
    for name in cases("stage_") {
        if name.starts_with("stage_large") || name.starts_with("stage_hdr") {
            continue;
        }
        let case = Case::open("detail", &name);
        let s = settings(&case);
        let ps = case.json()["pixel_scale"].as_f64().unwrap_or(1.0);
        let input = image(case.json()["image"].as_str().unwrap_or(""));
        let got = detail::develop_detail(&input, &s, ps).expect("develop_detail");
        checks.case(&name, || {
            let error = check(&case, &got, tolerance(&s, OTHER_MATHS_STAGES), &name);
            worst = worst.max(error);
            if noise_reduction(&s) {
                noise_worst = noise_worst.max(error);
            }
        });
    }
    println!("stage: worst {worst:e}");
    grainy_testkit::report_maths(
        "detail stages with noise reduction",
        f64::from(noise_worst),
        OTHER_MATHS_STAGES.into(),
    );
    checks.finish();
}

#[test]
fn default_settings_are_an_exact_no_op_on_display_pixels() {
    let s = Settings::from_value(serde_json::json!({})).expect("settings");
    for name in ["garden", "edges", "noise", "colours", "garden_full"] {
        let input = image(name);
        for ps in [1.0, 0.25] {
            let got = detail::develop_detail(&input, &s, ps).expect("develop_detail");
            assert_close(&got, &input, 0.0, &format!("defaults {name} {ps}"));
        }
    }
}

#[test]
fn hdr_branch() {
    for name in cases("stage_hdr") {
        let case = Case::open("detail", &name);
        let s = settings(&case);
        let input: Image = case.array("input");
        let got = detail::develop_detail(&input, &s, 1.0).expect("develop_detail");
        let error = check(&case, &got, tolerance(&s, OTHER_MATHS_TOOLS), &name);
        if noise_reduction(&s) {
            grainy_testkit::report_maths(&format!("detail {name}"), f64::from(error), OTHER_MATHS_TOOLS.into());
        }
    }
}

/// The large frame tiled from garden, as the generator builds it.
fn large_frame(h: usize, w: usize) -> Image {
    let g = image("garden_full");
    let (gh, gw, _) = g.dim();
    Array3::from_shape_fn((h, w, 3), |(y, x, c)| {
        let (ty, yy) = (y / gh % 2, y % gh);
        let (tx, xx) = (x / gw % 2, x % gw);
        let sy = if ty == 1 { gh - 1 - yy } else { yy };
        let sx = if tx == 1 { gw - 1 - xx } else { xx };
        g[[sy, sx, c]]
    })
}

#[test]
fn large_frame_and_thread_count() {
    for name in cases("stage_large") {
        let case = Case::open("detail", &name);
        let s = settings(&case);
        let shape: Vec<usize> = case.json()["shape"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_u64().unwrap_or(0) as usize).collect())
            .unwrap_or_default();
        let input = large_frame(shape[0], shape[1]);
        let run = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool")
                .install(|| detail::develop_detail(&input, &s, 1.0).expect("develop_detail"))
        };
        let one = run(1);
        let many = run(8);
        assert_eq!(one, many, "{name}: the result depends on the thread count");
        let error = check(&case, &many, tolerance(&s, OTHER_MATHS_TOOLS), &name);
        if noise_reduction(&s) {
            grainy_testkit::report_maths(&format!("detail {name}"), f64::from(error), OTHER_MATHS_TOOLS.into());
        }
    }
}
