//! End to end: `load_photo` on generated grey, CMYK, CIELAB and calibrated PNG/TIFF files against the oracle's
//! `engine.load_image` (golden/cms/load_*, tools/golden/gen_cms.py), in both working spaces. PNG and TIFF
//! decoding is exact, so the tolerance is 0 (bit-identical float32, NaN = NaN). Also checks `input_profile` and
//! `decoder` of the metadata.

use std::path::PathBuf;

use grainy_engine::files::{load_photo, Photo};
use grainy_testkit::{root, Case};
use serde_json::{Map, Value};

const CASES: [&str; 30] = [
    "gray8_plain_png",
    "gray8_icc_png",
    "gray16_icc_calibrated_png",
    "gray8_gama_png",
    "gray16_gama_chrm_png",
    "rgb8_gama_png",
    "rgb8_gama_chrm_png",
    "rgb16_gama_png",
    "rgb16_chrm_png",
    "rgb8_srgb_gama_png",
    "rgb8_icc_calibrated_png",
    "gray8_plain_tif",
    "gray16_icc_tif",
    "gray16_white_tif",
    "gray8_white_transfer_tif",
    "gray_f32_tif",
    "rgb8_chroma_tif",
    "rgb16_primaries_tif",
    "rgb8_transfer3_tif",
    "rgb8_transfer1_tif",
    "rgb_f32_chroma_tif",
    "cmyk8_icc_tif",
    "cmyk16_icc_tif",
    "cmyk8_plain_tif",
    "lab8_tif",
    "lab16_tif",
    "icclab8_tif",
    "lab8_icc_lab2_tif",
    // The two files golden/cli could not load before (grey ICC profile, PNG gAMA).
    "cli_gray8_icc_png",
    "cli_png8_gamma_png",
];

fn source(case: &Case, name: &str) -> PathBuf {
    let file = case.json()["file"].as_str().unwrap();
    if file.contains('/') {
        root().join(file)
    } else {
        root().join("golden").join("cms").join(format!("load_{name}")).join(file)
    }
}

fn settings(working: &str) -> grainy_core::Settings {
    let mut stored = Map::new();
    stored.insert("working_space".into(), Value::from(working));
    grainy_core::normalized(&stored)
}

/// Tolerance of the samples: 0 with the goldens' C runtime. Elsewhere (macOS) `to_linear`'s `powf` rounds some
/// samples differently in the last place (run 1: 12 samples of `gray8_plain_png`, 3.0e-8).
fn tolerance() -> f64 {
    grainy_testkit::maths_tolerance(0.0, 1e-6)
}

#[test]
fn load_photo_equals_the_oracle_load_image() {
    let mut report = Vec::new();
    let mut failures = Vec::new();
    let mut largest = 0f64;
    for name in CASES {
        let case = Case::open("cms", &format!("load_{name}"));
        let path = source(&case, name);
        for (working, key) in [("sRGB", "srgb"), ("ProPhoto", "prophoto")] {
            let loaded = load_photo(&path, None, &settings(working))
                .unwrap_or_else(|error| panic!("{name} ({working}): {error}"));
            let Photo::Linear(pixels) = loaded.photo else { panic!("{name}: not a linear photo") };
            let expected: grainy_core::Image = case.array(key);
            assert_eq!(pixels.dim(), expected.dim(), "{name} ({working}): shape");
            let mut worst = 0f64;
            let mut mismatches = 0;
            for (a, e) in pixels.iter().zip(expected.iter()) {
                let same = a.to_bits() == e.to_bits() || (a.is_nan() && e.is_nan());
                if !same {
                    mismatches += 1;
                    let d = if a.is_nan() || e.is_nan() { f64::INFINITY } else { (*a as f64 - *e as f64).abs() };
                    worst = worst.max(d);
                }
            }
            largest = largest.max(worst);
            if mismatches > 0 {
                println!("{name} ({working}): {mismatches} values differ, max |error| {worst:e}");
            }
            if worst > tolerance() || (tolerance() == 0.0 && mismatches > 0) {
                failures.push(format!("{name} ({working}): {mismatches} values differ, max |error| {worst:e}"));
            }
            let profile = &case.json()[format!("input_profile_{key}")];
            assert_eq!(loaded.metadata.get("input_profile").unwrap_or(&Value::Null), profile, "{name}: input_profile");
            assert_eq!(loaded.metadata.get("decoder"), Some(&case.json()["decoder"]), "{name}: decoder");
        }
        report.push(name);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!(
        "{} files x 2 working spaces equal to the oracle (tolerance {:e}, max |error| {largest:e})",
        report.len(),
        tolerance()
    );
}
