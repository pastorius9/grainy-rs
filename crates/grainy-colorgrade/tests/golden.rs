//! Golden tests against the Python oracle (`tools/golden/gen_colorgrade.py`, `golden/colorgrade/`).
//!
//! Each test walks the cases of one kind, prints the largest error of every case and of the kind
//! (`cargo test -p grainy-colorgrade -- --nocapture`), and fails when a case is above its tolerance.

use grainy_colorgrade::{auto_color, convert, photo_filter, point_color, tools, ColorParams};
use grainy_core::{defaults, Image, Plane, Settings};
use grainy_testkit::{max_abs_diff, Case};
use ndarray::{Array2, Array3, Dimension};
use serde_json::{Map, Value};

const MODULE: &str = "colorgrade";
/// Plain float32 arithmetic in the same order as the oracle.
const ARITHMETIC: f32 = 1e-5;

fn names(kind: &str) -> Vec<String> {
    let index = Case::open(MODULE, "index");
    index.json()[kind].as_array().expect("kind in index").iter().map(|v| v.as_str().unwrap().to_owned()).collect()
}

fn image(name: &str) -> Image {
    let (kind, key) = name.split_once(':').unwrap();
    match kind {
        "std" => grainy_testkit::input(key),
        _ => Case::open(MODULE, "inputs").array(key),
    }
}

fn settings(changes: &Value) -> Settings {
    let mut s = defaults();
    for (key, value) in changes.as_object().unwrap() {
        s.set(key, value.clone());
    }
    s
}

/// Cases whose oracle output must be reproduced bit for bit (defaults and switched-off controls).
fn tolerance(name: &str) -> f32 {
    let exact = name.contains("default")
        || name.contains("near_identity")
        || name.contains("nonfinite")
        || name.contains("wrong_shape")
        || name.contains("points_zero")
        || name.ends_with("density_zero")
        || name.ends_with("density_bad")
        || name.ends_with("unknown")
        || name.ends_with("disabled")
        || name == "hook_none";
    if exact {
        0.0
    } else {
        ARITHMETIC
    }
}

/// Compares, prints and returns the error; panics above the tolerance.
fn check<D: Dimension>(
    actual: &ndarray::Array<f32, D>,
    expected: &ndarray::Array<f32, D>,
    tol: f32,
    what: &str,
) -> f32 {
    assert_eq!(actual.shape(), expected.shape(), "{what}: shape");
    let (error, index) = max_abs_diff(actual, expected);
    println!("{what}: max abs error {error:.3e} (tolerance {tol:.0e})");
    if error > tol {
        let a = actual.iter().nth(index).copied().unwrap_or(f32::NAN);
        let e = expected.iter().nth(index).copied().unwrap_or(f32::NAN);
        panic!("{what}: max abs error {error:.6e} > {tol:.0e} at flat index {index} (actual {a}, expected {e})");
    }
    error
}

fn run_kind<F>(kind: &str, f: F)
where
    F: Fn(&str, &Case) -> Image,
{
    let mut worst = 0f32;
    for name in names(kind) {
        let case = Case::open(MODULE, &name);
        let expected: Array3<f32> = case.array("output");
        let actual = f(&name, &case);
        worst = worst.max(check(&actual, &expected, tolerance(&name), &name));
    }
    println!("== {kind}: largest error {worst:.3e}");
}

#[test]
fn stage_develop_color() {
    run_kind("stage", |_, case| {
        let s = settings(&case.json()["changes"]);
        grainy_colorgrade::develop_color(&image(case.json()["image"].as_str().unwrap()), &s).unwrap()
    });
}

#[test]
fn develop_color_mix() {
    run_kind("mix", |_, case| {
        let p = ColorParams::from_settings(&settings(&case.json()["changes"])).unwrap();
        tools::develop_color_mix(&image(case.json()["image"].as_str().unwrap()), &p)
    });
}

#[test]
fn mix_hsl_direct() {
    run_kind("mix_hsl", |_, case| {
        let groups: Vec<Vec<f64>> = serde_json::from_value(case.json()["groups"].clone()).unwrap();
        tools::mix_hsl(&image(case.json()["image"].as_str().unwrap()), &groups)
    });
}

#[test]
fn color_tools() {
    run_kind("tools", |_, case| {
        let p = ColorParams::from_settings(&settings(&case.json()["changes"])).unwrap();
        tools::color_tools(&image(case.json()["image"].as_str().unwrap()), &p, None).unwrap()
    });
}

#[test]
fn photo_filters() {
    run_kind("photo_filter", |_, case| {
        photo_filter::apply(&image(case.json()["image"].as_str().unwrap()), &settings(&case.json()["changes"]))
    });
}

fn points_of(value: &Value) -> Vec<Map<String, Value>> {
    value.as_array().unwrap().iter().map(|p| p.as_object().unwrap().clone()).collect()
}

#[test]
fn point_hook() {
    run_kind("hook", |name, case| {
        let points = points_of(&case.json()["points"]);
        let mut calls: Vec<(usize, f64)> = Vec::new();
        let mut hook = |index: usize, a: &Image| calls.push((index, a.iter().map(|&v| f64::from(v)).sum()));
        let out = point_color::apply(image(case.json()["image"].as_str().unwrap()), &points, Some(&mut hook)).unwrap();
        let expected = case.json()["calls"].as_array().unwrap();
        assert_eq!(calls.len(), expected.len(), "{name}: hook calls");
        for ((index, sum), e) in calls.iter().zip(expected) {
            assert_eq!(*index as u64, e[0].as_u64().unwrap(), "{name}: hook index");
            let want = e[1].as_f64().unwrap();
            assert!((sum - want).abs() <= 1e-3 * want.abs().max(1.0), "{name}: hook image sum {sum} != {want}");
        }
        out
    });
}

#[test]
fn point_weights() {
    let mut worst = 0f32;
    for name in names("weights") {
        let case = Case::open(MODULE, &name);
        let expected: Array2<f32> = case.array("output");
        let point = case.json()["point"].as_object().unwrap().clone();
        let img = image(case.json()["image"].as_str().unwrap());
        let actual: Plane = point_color::coverage(&img, &point).unwrap();
        worst = worst.max(check(&actual, &expected, ARITHMETIC, &name));
    }
    println!("== weights: largest error {worst:.3e}");
}

#[test]
fn point_errors() {
    let case = Case::open(MODULE, "point_errors");
    let wheel = image("cg:wheel");
    for item in case.json()["cases"].as_array().unwrap() {
        let point = item[0].as_object().unwrap().clone();
        let result = point_color::apply(wheel.clone(), &[point], None);
        match (&item[1], result) {
            (Value::Null, Ok(_)) => {}
            (Value::String(message), Err(error)) => assert_eq!(&error.to_string(), message),
            (want, got) => panic!("point {}: expected {want}, got {got:?}", item[0]),
        }
    }
}

#[test]
fn conversions() {
    let case = Case::open(MODULE, "convert_hsl");
    let rgb = image("cg:rgb");
    let (h, s, l) = convert::rgb_to_hsl(&rgb);
    check(&h, &case.array::<f32, _>("h"), 0.0, "rgb_to_hsl h");
    check(&s, &case.array::<f32, _>("s"), 0.0, "rgb_to_hsl s");
    check(&l, &case.array::<f32, _>("l"), 0.0, "rgb_to_hsl l");
    let out = convert::hsl_to_rgb(&case.array("in_h"), &case.array("in_s"), &case.array("in_l"));
    check(&out, &case.array::<f32, _>("rgb_out"), 0.0, "hsl_to_rgb");
    let inputs = case.json()["single_inputs"].as_array().unwrap();
    for (input, want) in inputs.iter().zip(case.json()["singles"].as_array().unwrap()) {
        let p: Vec<f32> = input.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let (h, s, l) = convert::rgb_to_hsl_pixel(&p, false);
        let want: Vec<f32> = want.as_array().unwrap().iter().map(|v| v[0][0].as_f64().unwrap() as f32).collect();
        assert_eq!([h, s, l].to_vec(), want, "1x1 rgb_to_hsl of {p:?}");
    }

    let case = Case::open(MODULE, "convert_hsv");
    let (h, s, v) = convert::rgb_to_hsv(&rgb);
    check(&h, &case.array::<f32, _>("h"), 0.0, "rgb_to_hsv h");
    check(&s, &case.array::<f32, _>("s"), 0.0, "rgb_to_hsv s");
    check(&v, &case.array::<f32, _>("v"), 0.0, "rgb_to_hsv v");
    let out = convert::hsv_to_rgb(&case.array("in_h"), &case.array("in_s"), &case.array("in_v"));
    check(&out, &case.array::<f32, _>("rgb_out"), 0.0, "hsv_to_rgb");

    let case = Case::open(MODULE, "convert_srgb");
    check(
        &convert::to_linear(&grainy_testkit::input("noise")),
        &case.array::<f32, _>("linear_noise"),
        ARITHMETIC,
        "to_linear noise",
    );
    check(&convert::to_linear(&rgb), &case.array::<f32, _>("linear_rgb"), ARITHMETIC, "to_linear rgb");
    check(
        &convert::to_srgb(&grainy_testkit::input("wide")),
        &case.array::<f32, _>("srgb_wide"),
        ARITHMETIC,
        "to_srgb wide",
    );
    check(&convert::to_srgb(&rgb), &case.array::<f32, _>("srgb_rgb"), ARITHMETIC, "to_srgb rgb");
}

#[test]
fn auto_color_analysis() {
    let mut worst = 0f64;
    for name in names("auto") {
        let case = Case::open(MODULE, &name);
        let meta = case.json();
        let (curves, report) =
            auto_color::channel_points(&image(meta["image"].as_str().unwrap()), meta["amount"].as_f64().unwrap());
        let want = &meta["report"];
        assert_eq!(report.status, want["status"].as_str().unwrap(), "{name}: status");
        assert_eq!(report.amount, want.get("amount").and_then(Value::as_f64), "{name}: amount");
        let mut error = 0f64;
        match (&report.casts, want.get("casts")) {
            (None, None) => {}
            (Some(casts), Some(Value::Array(expected))) => {
                assert_eq!(casts.len(), expected.len(), "{name}: casts");
                for (c, e) in casts.iter().zip(expected) {
                    match (c, e) {
                        (None, Value::Null) => {}
                        (Some(c), Value::Array(e)) => {
                            for (a, b) in c.iter().zip(e) {
                                error = error.max((a - b.as_f64().unwrap()).abs());
                            }
                        }
                        _ => panic!("{name}: cast {c:?} against {e}"),
                    }
                }
            }
            (got, want) => panic!("{name}: casts {got:?} against {want:?}"),
        }
        match (&curves, &meta["curves"]) {
            (None, Value::Null) => {}
            (Some(curves), Value::Array(expected)) => {
                for (curve, e) in curves.iter().zip(expected) {
                    let e = e.as_array().unwrap();
                    assert_eq!(curve.len(), e.len(), "{name}: points in {curve:?} against {e:?}");
                    for (p, q) in curve.iter().zip(e) {
                        error =
                            error.max((p[0] - q[0].as_f64().unwrap()).abs()).max((p[1] - q[1].as_f64().unwrap()).abs());
                    }
                }
            }
            (got, want) => panic!("{name}: curves {got:?} against {want}"),
        }
        // Values are rounded to 4 decimals: a last-bit difference may move one by a rounding step.
        println!("{name}: max abs error {error:.3e} (tolerance 1e-4)");
        assert!(error <= 1.0001e-4, "{name}: curves or casts differ by {error}");
        worst = worst.max(error);
    }
    println!("== auto: largest error {worst:.3e}");
}

#[test]
fn thread_count_does_not_matter() {
    let s = settings(&Case::open(MODULE, "stage_combo_garden").json()["changes"]);
    let img = image("std:garden");
    let big = ndarray::concatenate(ndarray::Axis(0), &[img.view(), img.view(), img.view(), img.view()]).unwrap();
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let a = one.install(|| grainy_colorgrade::develop_color(&big, &s).unwrap());
    let b = grainy_colorgrade::develop_color(&big, &s).unwrap();
    assert_eq!(a, b);
}
