//! Whole develops against the oracle's `engine.develop` (tools/golden/gen_engine.py).

use grainy_core::Image;
use grainy_engine::{develop, CameraSource, Crop, Error, Options, Output};
use grainy_testkit::{max_abs_diff, Case};
use ndarray::s;
use serde_json::{Map, Value};

/// The stages are bit-exact except luminance noise reduction (up to 4.6e-6: the oracle's bilateral filter runs in
/// Intel IPP) and grain with a size or roughness (1.2e-7).
const TOLERANCE: f32 = 2e-5;
/// A LittleCMS conversion after such a stage works in 16-bit steps internally: a last-bit difference before it
/// can come out one to five steps (1.5e-5 each) apart. Measured: 7.6e-5 at most (`all_garden_film_wide`).
const CONVERTED_TOLERANCE: f32 = 2e-4;
/// Lens profiles: grainy-lens reimplements the lens models from their published description (no lensfun code), so
/// coordinates agree to about a thousandth of a pixel on real profiles, not exactly. Measured 2026-11-14 after the
/// automatic-scale fix: 1e-5 at most over the `lens_*` cases (it was 1.6e-3 before).
const LENS_TOLERANCE: f32 = 2e-5;

fn truthy(value: &Value) -> bool {
    value.as_bool().unwrap_or(false)
}

fn options(meta: &Value) -> (Crop, Option<String>, Option<(f64, f64)>) {
    let use_crop = match &meta["use_crop"] {
        Value::Bool(true) => Crop::Cropped,
        Value::Bool(false) => Crop::Uncropped,
        Value::String(text) if text == "all" => Crop::All,
        other => panic!("use_crop {other}"),
    };
    let output = meta["output"].as_str().map(str::to_owned);
    let original = meta["original_size"].as_array().map(|size| (size[0].as_f64().unwrap(), size[1].as_f64().unwrap()));
    (use_crop, output, original)
}

fn settings(meta: &Value) -> Map<String, Value> {
    meta["settings"].as_object().expect("settings").clone()
}

fn source(case: &Case) -> Image {
    if case.has("input") {
        case.array("input")
    } else {
        grainy_testkit::input(case.json()["source"].as_str().expect("source"))
    }
}

#[test]
fn develops_match_the_oracle() {
    let index = Case::open("engine", "_index");
    let names: Vec<&str> =
        index.json()["cases"].as_array().unwrap().iter().map(|name| name.as_str().unwrap()).collect();
    assert!(names.len() >= 120, "only {} cases", names.len());
    let mut worst = (0f32, "");
    for name in names {
        let case = Case::open("engine", name);
        let (use_crop, output, original_size) = options(case.json());
        let output = match output.as_deref() {
            Some(space) => Output::Named(space),
            None => Output::Working,
        };
        let options = Options { use_crop, output, original_size, ..Options::default() };
        let result = if case.has("camera") {
            let info = serde_json::from_value(case.json()["info"].clone()).expect("camera info");
            let camera = CameraSource::new(case.array("camera"), info).expect("camera source");
            develop(&camera, &settings(case.json()), &options)
        } else {
            develop(&source(&case), &settings(case.json()), &options)
        }
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        let error = if case.has("output") {
            let expected: Image = case.array("output");
            max_abs_diff(&result, &expected).0
        } else {
            let expected: Image = case.array("output_sub");
            let shape: Vec<usize> =
                case.json()["shape"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as usize).collect();
            assert_eq!(result.shape(), shape.as_slice(), "{name}: shape");
            let step = case.json()["step"].as_u64().unwrap() as isize;
            let sub = result.slice(s![..;step, ..;step, ..]).to_owned();
            let mut error = max_abs_diff(&sub, &expected).0;
            for (channel, mean) in case.json()["mean"].as_array().unwrap().iter().enumerate() {
                let plane = result.slice(s![.., .., channel]);
                let actual = plane.iter().map(|&v| f64::from(v)).sum::<f64>() / plane.len() as f64;
                error = error.max((actual - mean.as_f64().unwrap()).abs() as f32);
            }
            error
        };
        let converted = case.json()["settings"]["working_space"] == "ProPhoto"
            || !matches!(case.json()["output"].as_str(), None | Some("sRGB"));
        let lens = crate::truthy(&case.json()["settings"]["lensfun_enabled"])
            && !case.json()["settings"]["panels_off"].as_array().is_some_and(|off| off.iter().any(|p| p == "lens"));
        let tolerance = if lens {
            LENS_TOLERANCE
        } else if converted {
            CONVERTED_TOLERANCE
        } else {
            TOLERANCE
        };
        println!("{name}: max abs error {error:.3e}");
        assert!(error <= tolerance, "{name}: max abs error {error:e} above {tolerance:e}");
        if error > worst.0 {
            worst = (error, name);
        }
    }
    println!("largest error over all develops: {:.3e} ({})", worst.0, worst.1);
}

#[test]
fn defaults_only_clip() {
    let input = grainy_testkit::input("noise");
    let result = develop(&input, &Map::new(), &Options::default()).unwrap();
    // Process 3 defaults: the highlight roll-off, the sRGB encoding and the clip; nothing else changes pixels.
    let case = Case::open("engine", "defaults_noise");
    let expected: Image = case.array("output");
    // Exact with the goldens' C runtime; elsewhere (macOS) the sRGB encoding's `powf` rounds some samples
    // differently in the last place (measured on macOS, CI runs 1 and 5: 1.19e-7, one ulp below 1; tolerance
    // 2.5e-7, about 2x).
    let error = max_abs_diff(&result, &expected).0;
    println!("defaults_only_clip: max abs error {error:e}");
    let tolerance = grainy_testkit::maths_tolerance(0.0, 2.5e-7);
    grainy_testkit::report_maths("engine defaults_only_clip", f64::from(error), f64::from(tolerance));
    assert!(error <= tolerance, "defaults_only_clip: max abs error {error:e}");
}

#[test]
fn an_unusable_lens_profile_is_an_error() {
    let index = Case::open("engine", "_index");
    let input = grainy_testkit::input("colours");
    for broken in index.json()["pending"].as_array().unwrap() {
        let name = broken["name"].as_str().unwrap();
        let settings = broken["settings"].as_object().unwrap().clone();
        match develop(&input, &settings, &Options::default()) {
            Err(Error::Lens(error)) => println!("{name}: refused ({error})"),
            other => panic!("{name}: expected a lens error, got {:?}", other.map(|image| image.dim())),
        }
    }
    // A caller's Externals without a piece the settings need: refused, not skipped.
    let bare = grainy_engine::Externals { lens: None, dust: None, sky: None };
    let sky = serde_json::json!({"masks": [{"components": [{"type": "sky"}], "adjustments": {"exposure": 1.0}}]});
    let options = Options { externals: bare, ..Options::default() };
    assert!(matches!(develop(&input, sky.as_object().unwrap(), &options), Err(Error::Pending("sky masks"))));
}

#[test]
fn bad_original_size_is_an_error() {
    let input = grainy_testkit::input("ramp");
    for size in [(0.0, 10.0), (f64::NAN, 10.0), (10.0, -1.0), (f64::INFINITY, 5.0)] {
        let options = Options { original_size: Some(size), ..Options::default() };
        assert!(matches!(develop(&input, &Map::new(), &options), Err(Error::OriginalSize)));
    }
}

fn changed(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(key, value)| ((*key).to_owned(), value.clone())).collect()
}

#[test]
fn cached_develops_reuse_stages_with_the_same_pixels() {
    use grainy_engine::{develop_cached, DevelopmentCache};
    let input = grainy_testkit::input("garden");
    let options = Options::default();
    let mut cache = DevelopmentCache::default();
    let base = changed(&[("exposure", 0.4.into()), ("saturation", 20.0.into()), ("sharpen", 40.0.into())]);
    let sharper = changed(&[("exposure", 0.4.into()), ("saturation", 20.0.into()), ("sharpen", 70.0.into())]);
    let brighter = changed(&[("exposure", 0.9.into()), ("saturation", 20.0.into()), ("sharpen", 70.0.into())]);

    let first = develop_cached(&input, &base, &options, &mut cache, 1).unwrap();
    assert_eq!(first, develop(&input, &base, &options).unwrap());
    assert_eq!((cache.hits(), cache.misses()), (0, 3));

    // A detail slider: tone and colour come from the cache.
    let second = develop_cached(&input, &sharper, &options, &mut cache, 1).unwrap();
    assert_eq!(second, develop(&input, &sharper, &options).unwrap());
    assert_eq!((cache.hits(), cache.misses()), (2, 4));

    // The same settings again: nothing is computed.
    assert_eq!(develop_cached(&input, &sharper, &options, &mut cache, 1).unwrap(), second);
    assert_eq!((cache.hits(), cache.misses()), (5, 4));

    // A tone slider: every stage after it runs again.
    let third = develop_cached(&input, &brighter, &options, &mut cache, 1).unwrap();
    assert_eq!(third, develop(&input, &brighter, &options).unwrap());
    assert_eq!((cache.hits(), cache.misses()), (5, 7));

    // Another source drops everything.
    let other = grainy_testkit::input("colours");
    let version = cache.version();
    let fourth = develop_cached(&other, &brighter, &options, &mut cache, 2).unwrap();
    assert_eq!(fourth, develop(&other, &brighter, &options).unwrap());
    assert_eq!(cache.version(), version + 1);
    assert_eq!((cache.hits(), cache.misses()), (5, 10));

    // A negative is a cached stage of its own, and every stage after it runs again.
    let negative = changed(&[("negative", true.into()), ("exposure", 0.9.into())]);
    develop_cached(&other, &negative, &options, &mut cache, 2).unwrap();
    assert_eq!(cache.misses(), 14);

    // A budget too small for two copies of the source switches the cache off; pixels are the same.
    let mut small = DevelopmentCache::new(input.len() * 4);
    assert_eq!(develop_cached(&input, &base, &options, &mut small, 1).unwrap(), first);
    assert_eq!((small.hits(), small.misses(), small.bytes()), (0, 0, 0));
}
