//! The automatic buttons against the oracle (tools/golden/gen_auto.py): auto tone, the automatic value of one
//! slider, auto colour, automatic white balance, the film negative measurement, automatic alignment, the
//! scan-border crop and the AI batch's auto level / auto crop.
//!
//! Numbers are compared relative to their size with an absolute floor (`|a - b| <= tol * max(1, |b|)`), with
//! `TOLERANCE` = 1e-9; arrays of pixels exactly. Each test prints its largest difference.

use grainy_core::{normalized, Image, Settings};
use grainy_engine::auto::{self, Error};
use grainy_engine::{CameraSource, Externals, Source};
use grainy_testkit::Case;
use ndarray::{Array2, Array3, Ix2};
use serde_json::{Map, Value};

const TOLERANCE: f64 = 1e-9;

enum Owned {
    Linear(Image),
    Camera(Box<CameraSource>),
}

impl Owned {
    fn source(&self) -> Source<'_> {
        match self {
            Owned::Linear(image) => Source::Linear(image),
            Owned::Camera(camera) => Source::Camera(camera),
        }
    }
}

fn scaled(a: &Image, k: f32) -> Image {
    a.mapv(|v| v * k)
}

fn tiled(a: &Image, h: usize, w: usize) -> Image {
    let (ah, aw, _) = a.dim();
    Array3::from_shape_fn((h, w, 3), |(y, x, c)| a[[y % ah, x % aw, c]])
}

fn bordered(a: &Image, top: usize, left: usize, value: f32) -> Image {
    let (h, w, _) = a.dim();
    let mut out = Array3::from_elem((h + 2 * top, w + 2 * left, 3), value);
    out.slice_mut(ndarray::s![top..top + h, left..left + w, ..]).assign(a);
    out
}

/// The sources of gen_auto.py, by the same recipes.
fn source(name: &str) -> Owned {
    let garden = || grainy_testkit::input("garden");
    let negative = || Case::open("engine", "negative").array::<f32, _>("input");
    let image = match name {
        "ramp" | "wide" | "colours" | "patches" | "noise" | "edges" | "garden" => grainy_testkit::input(name),
        "bright" => scaled(&garden(), 4.0),
        "dark" => scaled(&garden(), 0.05),
        "black" => scaled(&garden(), 0.001),
        "flat" => Array3::from_elem((64, 96, 3), 0.18f32),
        "low" => garden().mapv(|v| v * 0.1f32 + 0.3f32),
        "cast" => {
            let mut a = garden();
            for (c, k) in [1.3f32, 1.0, 0.7].into_iter().enumerate() {
                a.slice_mut(ndarray::s![.., .., c]).mapv_inplace(|v| v * k);
            }
            a
        }
        "big" => tiled(&garden(), 1300, 900),
        "border" => bordered(&garden(), 12, 15, 0.002),
        "border_big" => tiled(&bordered(&garden(), 12, 15, 0.002), 1500, 1100),
        "negative" => negative(),
        "negative_bright" => scaled(&negative(), 1.5),
        "camera" => {
            let case = Case::open("engine", "camera_as_shot");
            let info = serde_json::from_value(case.json()["info"].clone()).expect("camera info");
            return Owned::Camera(Box::new(CameraSource::new(case.array("camera"), info).expect("camera source")));
        }
        scene if scene.starts_with("scene_") => {
            let grey: Array2<u8> = Case::open("auto", scene).array::<u8, Ix2>("grey");
            let (h, w) = grey.dim();
            Array3::from_shape_fn((h, w, 3), |(y, x, _)| f32::from(grey[[y, x]]) / 255.0)
        }
        other => panic!("unknown source {other}"),
    };
    Owned::Linear(image)
}

fn names(group: &str) -> Vec<String> {
    let index = Case::open("auto", "_index");
    index.json()[group]
        .as_array()
        .unwrap_or_else(|| panic!("no group {group}"))
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect()
}

/// White balance where the C runtime is not the goldens' (macOS): its temperature and tint are `log2f` of channel
/// gain ratios (`grainy_core::analysis`), which another `log2f` rounds differently in the last place, scaled by
/// 90 and 1/0.013 (run 1: 8.9e-8 in `wb_garden_defaults.temperature`).
const OTHER_MATHS_WHITE_BALANCE: f64 = 1e-5;

/// Compares JSON values; numbers within `TOLERANCE * max(1, |expected|)`. Returns the largest scaled difference.
fn compare(actual: &Value, expected: &Value, path: &str) -> f64 {
    compare_within(actual, expected, path, TOLERANCE)
}

/// [`compare`] with another relative tolerance.
fn compare_within(actual: &Value, expected: &Value, path: &str, tolerance: f64) -> f64 {
    let compare = |a: &Value, e: &Value, p: &str| compare_within(a, e, p, tolerance);
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            let d = (a - e).abs() / e.abs().max(1.0);
            assert!(d <= tolerance, "{path}: {a} against the oracle's {e} ({d:e})");
            d
        }
        (Value::Bool(a), Value::Number(e)) | (Value::Number(e), Value::Bool(a)) => {
            assert_eq!(f64::from(u8::from(*a)), e.as_f64().unwrap(), "{path}");
            0.0
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: length of {actual} against {expected}");
            a.iter().zip(e).enumerate().map(|(i, (a, e))| compare(a, e, &format!("{path}[{i}]"))).fold(0.0, f64::max)
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut ak: Vec<&String> = a.keys().collect();
            let mut ek: Vec<&String> = e.keys().collect();
            ak.sort();
            ek.sort();
            assert_eq!(ak, ek, "{path}: keys");
            e.iter().map(|(k, v)| compare(&a[k], v, &format!("{path}.{k}"))).fold(0.0, f64::max)
        }
        _ => {
            assert_eq!(actual, expected, "{path}");
            0.0
        }
    }
}

/// The oracle's result: `normalized(settings)` with the changed keys of the case.
fn expected_settings(case: &Case, base: &Map<String, Value>) -> Value {
    let mut map = base.clone();
    for (k, v) in case.json()["changed"].as_object().expect("changed") {
        map.insert(k.clone(), v.clone());
    }
    for k in case.json()["removed"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        map.remove(k.as_str().unwrap());
    }
    Value::Object(map)
}

fn stored_settings(case: &Case) -> Settings {
    Settings(case.json()["settings"].as_object().expect("settings").clone())
}

fn check_error(name: &str, error: &Error, expected: &Value) {
    assert_eq!(error.to_string(), expected["message"].as_str().unwrap(), "{name}: message");
    assert_eq!(error.is_alignment(), expected["kind"] == "AlignmentError", "{name}: kind");
}

fn report(group: &str, worst: (f64, String), count: usize) {
    println!("{group}: {count} cases, largest scaled difference {:.3e} ({})", worst.0, worst.1);
}

fn track(worst: &mut (f64, String), d: f64, name: &str) {
    if d > worst.0 || worst.1.is_empty() {
        *worst = (d, name.to_owned());
    }
}

#[test]
fn auto_tone_matches_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("tone");
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let s = stored_settings(&case);
        let (adjusted, report) = auto::auto_tone_settings(owned.source(), &s, &Externals::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let mut d = compare(&Value::Object(adjusted.0), &expected_settings(&case, &normalized(&s.0).0), name);
        d = d.max(compare(&Value::Object(report), &case.json()["report"], &format!("{name} report")));
        track(&mut worst, d, name);
    }
    report("auto tone", worst, cases.len());
}

#[test]
fn auto_slider_values_match_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("slider");
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let key = case.json()["key"].as_str().unwrap();
        let value = auto::auto_slider_value(owned.source(), &stored_settings(&case), key, &Externals::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let d = compare(&serde_json::json!(value), &case.json()["value"], name);
        track(&mut worst, d, name);
    }
    report("auto slider", worst, cases.len());
}

#[test]
fn auto_colour_matches_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("colour");
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let s = stored_settings(&case);
        let amount = case.json()["amount"].as_f64().unwrap();
        let (adjusted, report) = auto::auto_color_settings(owned.source(), &s, amount, &Externals::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let mut d = compare(&Value::Object(adjusted.0), &expected_settings(&case, &normalized(&s.0).0), name);
        d = d.max(compare(&Value::Object(report), &case.json()["report"], &format!("{name} report")));
        track(&mut worst, d, name);
    }
    report("auto colour", worst, cases.len());
}

#[test]
fn automatic_white_balance_matches_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("wb");
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let s = stored_settings(&case);
        match auto::automatic_white_balance(owned.source(), &s, &Externals::default()) {
            Ok(values) => {
                let expected = case.json()["values"].as_object().unwrap_or_else(|| panic!("{name}: expected an error"));
                let mut whole = s.0.clone();
                whole.extend(expected.clone());
                let tolerance = grainy_testkit::maths_tolerance(TOLERANCE, OTHER_MATHS_WHITE_BALANCE);
                let d = compare_within(&Value::Object(values.0), &Value::Object(whole), name, tolerance);
                track(&mut worst, d, name);
            }
            Err(error) => check_error(name, &error, &case.json()["error"]),
        }
    }
    report("automatic white balance", worst, cases.len());
}

#[test]
fn negative_measurement_matches_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("negative");
    let none = Externals::default();
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let s = stored_settings(&case);
        let part = auto::negative_measured(owned.source(), &s, &none).unwrap_or_else(|error| panic!("{name}: {error}"));
        let shape: Vec<usize> = part.shape().to_vec();
        assert_eq!(serde_json::json!(shape), case.json()["measured_shape"], "{name}: shape");
        if case.has("measured") {
            let expected: Image = case.array("measured");
            assert_eq!(grainy_testkit::max_abs_diff(&part, &expected).0, 0.0, "{name}: measured pixels");
        }
        let sums: Vec<f64> =
            (0..3).map(|c| part.slice(ndarray::s![.., .., c]).iter().map(|&v| f64::from(v)).sum()).collect();
        let mut d = compare(&serde_json::json!(sums), &case.json()["measured_sum"], &format!("{name} sum"));
        let sample = auto::negative_sample(owned.source(), &s, &none).unwrap();
        assert_eq!(serde_json::json!(sample.shape()), case.json()["sample_shape"], "{name}: sample shape");
        let sums: Vec<f64> = (0..3).map(|c| sample.column(c).iter().map(|&v| f64::from(v)).sum()).collect();
        d = d.max(compare(&serde_json::json!(sums), &case.json()["sample_sum"], &format!("{name} sample")));
        let converted = auto::negative_converted(owned.source(), &s, &none).unwrap();
        let mut whole = s.0.clone();
        whole.extend(case.json()["converted"].as_object().unwrap().clone());
        d = d.max(compare(&Value::Object(converted.0), &Value::Object(whole), &format!("{name} converted")));
        track(&mut worst, d, name);
    }
    report("negative measured/sample/converted", worst, cases.len());

    let mut worst = (0.0, String::new());
    let rolls = names("together");
    for name in &rolls {
        let case = Case::open("auto", name);
        let members = case.json()["members"].as_array().unwrap();
        let mut samples = Vec::new();
        let mut settings = Vec::new();
        for member in members {
            let owned = source(member["source"].as_str().unwrap());
            let s = Settings(member["settings"].as_object().unwrap().clone());
            samples.push(auto::negative_sample(owned.source(), &s, &none).unwrap());
            settings.push(s);
        }
        let results = auto::negative_converted_together(&samples, &settings);
        let mut d: f64 = 0.0;
        for (i, (result, s)) in results.into_iter().zip(&settings).enumerate() {
            let mut whole = s.0.clone();
            whole.extend(case.json()["converted"][i].as_object().unwrap().clone());
            d = d.max(compare(&Value::Object(result.0), &Value::Object(whole), &format!("{name}[{i}]")));
        }
        track(&mut worst, d, name);
    }
    report("negative converted_together", worst, rolls.len());
}

#[test]
fn upright_estimates_match_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("upright");
    let mut estimates = 0;
    for name in &cases {
        let case = Case::open("auto", name);
        let Owned::Linear(image) = source(case.json()["source"].as_str().unwrap()) else { unreachable!() };
        let mode = case.json()["mode"].as_str().unwrap();
        match auto::upright_estimate(&image, mode) {
            Ok(estimate) => {
                let expected = &case.json()["estimate"];
                assert!(!expected.is_null(), "{name}: the oracle raised {}", case.json()["error"]);
                let d = compare(&estimate.to_json(), expected, name);
                track(&mut worst, d, name);
                estimates += 1;
            }
            Err(error) => {
                assert!(
                    case.json().get("error").is_some(),
                    "{name}: {error}, the oracle found {}",
                    case.json()["estimate"]
                );
                check_error(name, &error, &case.json()["error"]);
            }
        }
    }
    report(&format!("upright estimate ({estimates} estimates)"), worst, cases.len());
}

#[test]
fn scan_border_crops_match_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("border");
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let longest = case.json()["longest"].as_u64().unwrap() as usize;
        let crop = auto::scan_border_crop(owned.source(), &stored_settings(&case), longest, &Externals::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let d = compare(&serde_json::json!(crop), &case.json()["crop"], name);
        track(&mut worst, d, name);
    }
    report("scan border crop", worst, cases.len());
}

#[test]
fn automatic_edits_match_the_oracle() {
    let mut worst = (0.0, String::new());
    let cases = names("automatic");
    for name in &cases {
        let case = Case::open("auto", name);
        let owned = source(case.json()["source"].as_str().unwrap());
        let s = stored_settings(&case);
        let flags = (case.json()["auto_straighten"].as_bool().unwrap(), case.json()["auto_crop"].as_bool().unwrap());
        match auto::automatic(owned.source(), &s, flags.0, flags.1, &Externals::default()) {
            Ok(result) => {
                assert!(case.json().get("error").is_none(), "{name}: the oracle raised {}", case.json()["error"]);
                let d = compare(&Value::Object(result.0), &expected_settings(&case, &s.0), name);
                track(&mut worst, d, name);
            }
            Err(error) => check_error(name, &error, &case.json()["error"]),
        }
    }
    report("photo_edits.automatic", worst, cases.len());
}

#[test]
fn ui_wrappers_reduce_as_the_oracle() {
    // measure_negative = converted(resize_float(source, 1200), …): a frame within 1200 pixels is measured as it is.
    let owned = source("negative");
    let s = Settings(Map::new());
    let none = Externals::default();
    let direct = auto::negative_converted(owned.source(), &s, &none).unwrap();
    assert_eq!(auto::measure_negative(owned.source(), &s, &none).unwrap().0, direct.0);
    // upright_for leaves the settings' own alignment out: the same estimate with or without one.
    let scene = source("scene_building");
    let mut tilted = Settings(Map::new());
    tilted.set("straighten", 3.0);
    tilted.set("crop", serde_json::json!([0.1, 0.1, 0.9, 0.9]));
    let a = auto::upright_for(scene.source(), &tilted, "level", &none).unwrap();
    let b = auto::upright_for(scene.source(), &Settings(Map::new()), "level", &none).unwrap();
    assert_eq!(a, b);
}
