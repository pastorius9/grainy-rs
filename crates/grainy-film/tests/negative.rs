//! Golden tests of film negatives against luma/negative.py (tools/golden/gen_film.py).
mod common;

use common::{a32, close32, every};
use grainy_core::{Image, Settings};
use grainy_film::negative;
use grainy_testkit::Case;
use ndarray::{Array2, Array3};
use serde_json::Value;

fn inputs() -> Case {
    Case::open("film", "negative_inputs")
}

fn image(case: &Case, name: &str) -> Image {
    case.array::<f32, ndarray::Ix3>(name)
}

fn settings(value: &Value) -> Settings {
    Settings::from_value(value.clone()).unwrap()
}

fn ends_json(ends: ([f64; 3], [f64; 3])) -> Value {
    serde_json::json!([ends.0.to_vec(), ends.1.to_vec()])
}

/// JSON equality with numbers compared within `tolerance`; prints the largest numeric difference.
fn same(actual: &Value, expected: &Value, tolerance: f64, path: &str, worst: &mut f64) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let d = (a.as_f64().unwrap() - e.as_f64().unwrap()).abs();
            *worst = worst.max(d);
            assert!(d <= tolerance, "{path}: {a} vs {e}");
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: lengths");
            for (i, (x, y)) in a.iter().zip(e).enumerate() {
                same(x, y, tolerance, &format!("{path}[{i}]"), worst);
            }
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut ka: Vec<&String> = a.keys().collect();
            let mut ke: Vec<&String> = e.keys().collect();
            ka.sort();
            ke.sort();
            assert_eq!(ka, ke, "{path}: keys");
            for (k, v) in a {
                same(v, &e[k], tolerance, &format!("{path}.{k}"), worst);
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn check(actual: &Value, expected: &Value, what: &str) {
    let mut worst = 0.0;
    same(actual, expected, 0.0, what, &mut worst);
    println!("{what}: max abs error {worst:.3e} (tolerance 0)");
}

#[test]
fn analyse_and_ends() {
    let ins = inputs();
    let case = Case::open("film", "negative_analyse");
    let r = &case.json()["results"];
    for name in ["colour", "mono", "dusty"] {
        let frame = image(&ins, name);
        let inner: Vec<f32> = negative::inside(&frame).iter().copied().collect();
        close32(&inner, &a32(&case, &format!("inside_{name}")), 0.0, &format!("inside {name}"));
        check(&ends_json(negative::analyse(&frame)), &r[name], &format!("analyse {name}"));
    }
    let few = Array2::from_elem((50, 3), 1e-6f32);
    check(&ends_json(negative::ends(few.view())), &r["few"], "ends, too few usable pixels");
    let tiny = Array2::from_shape_fn((120, 3), |(_, c)| [0.5f32, 0.4, 0.3][c]);
    check(&ends_json(negative::ends(tiny.view())), &r["tiny"], "ends, one colour");
    // np.linspace(.001, .9, 3003, dtype=float32): computed in float64, then cast
    let n = 3 * 1001;
    let step = (0.9 - 0.001) / (n - 1) as f64;
    let ramp: Vec<f32> = (0..n).map(|i| if i == n - 1 { 0.9f32 } else { (i as f64 * step + 0.001) as f32 }).collect();
    let ramp = Array2::from_shape_vec((1001, 3), ramp).unwrap();
    check(&ends_json(negative::ends(ramp.view())), &r["ramp"], "ends, ramp");
    let colour = image(&ins, "colour");
    let corner = colour.slice(ndarray::s![..5, ..7, ..]).to_owned();
    let list = |a: [f64; 3]| serde_json::json!(a.to_vec());
    check(&list(negative::base_from(&corner)), &r["base_from"], "base_from");
    check(&list(negative::base_from(&Array3::zeros((2, 2, 3)))), &r["base_from_dark"], "base_from dark");
}

#[test]
fn measured_and_sample() {
    let ins = inputs();
    let colour = image(&ins, "colour");
    let case = Case::open("film", "negative_measured");
    let cases = case.json()["cases"].as_array().unwrap();
    let defaults = grainy_core::defaults();
    for (i, c) in cases.iter().enumerate() {
        if c.get("big_rows").is_some() {
            continue;
        }
        let mut s = defaults.clone();
        for (k, v) in c["settings"].as_object().unwrap() {
            s.set(k, v.clone());
        }
        let part = negative::measured(&colour, &s).unwrap();
        let shape: Vec<u64> = c["shape"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        assert_eq!(
            part.shape().iter().map(|&v| v as u64).collect::<Vec<_>>(),
            shape,
            "measured shape {}",
            c["settings"]
        );
        close32(
            &every(&part, 0, 3),
            &a32(&case, &format!("measured_{i}")),
            0.0,
            &format!("measured {}", c["settings"]),
        );
        let sample = negative::sample(&colour, &s).unwrap();
        assert_eq!(sample.nrows() as u64, c["sample_rows"].as_u64().unwrap());
        let sub: Vec<f32> = sample.outer_iter().step_by(7).flat_map(|r| r.to_vec()).collect();
        close32(&sub, &a32(&case, &format!("sample_{i}")), 0.0, "sample");
        let part_rows = Array2::from_shape_vec((part.len() / 3, 3), part.iter().copied().collect()).unwrap();
        check(&ends_json(negative::ends(part_rows.view())), &c["ends"], &format!("ends of measured {}", c["settings"]));
    }
    let big_rows = cases.iter().find_map(|c| c.get("big_rows")).unwrap().as_u64().unwrap();
    let big = Array3::from_shape_fn((240, 360, 3), |(y, x, c)| colour[[y / 3, x / 3, c]]);
    let sample = negative::sample(&big, &defaults).unwrap();
    assert_eq!(sample.nrows() as u64, big_rows);
    let sub: Vec<f32> = sample.outer_iter().step_by(7).flat_map(|r| r.to_vec()).collect();
    close32(&sub, &a32(&case, "sample_big"), 0.0, "sample of a large frame");
    let mut bent = defaults.clone();
    bent.set("straighten", 2.0);
    assert!(matches!(negative::measured(&colour, &bent), Err(grainy_film::Error::Unsupported(_))));
}

#[test]
fn apply() {
    let ins = inputs();
    let case = Case::open("film", "negative_apply");
    let colour = image(&ins, "colour");
    let mono = image(&ins, "mono");
    let dusty = image(&ins, "dusty");
    for c in case.json()["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let s = settings(&c["settings"]);
        let (source, stored, step) = match name {
            "mono" | "mono_dense" => (mono.clone(), name.to_owned(), 1),
            "dusty" => (dusty.clone(), name.to_owned(), 1),
            "four_channels" => {
                let four =
                    Array3::from_shape_fn((64, 96, 4), |(y, x, ch)| if ch < 3 { mono[[y, x, ch]] } else { 0.25 });
                (four, name.to_owned(), 1)
            }
            _ => (colour.clone(), format!("colour_{name}"), 2),
        };
        let out = negative::apply(&source, &s);
        assert_eq!(!negative::enabled(&s), c["same"].as_bool().unwrap(), "{name}: off");
        if !negative::enabled(&s) {
            assert_eq!(out, source);
        }
        close32(&every(&out, 0, step), &a32(&case, &stored), 1e-5, &format!("apply {name}"));
    }
}

#[test]
fn converted_and_roll() {
    let ins = inputs();
    let case = Case::open("film", "negative_convert");
    let meta = case.json();
    let colour = image(&ins, "colour");
    let mono = image(&ins, "mono");
    for c in meta["single"].as_array().unwrap() {
        let s = settings(&c["settings"]);
        check(&Value::Object(negative::converted(&colour, &s).unwrap().0), &c["colour"], "converted colour");
        check(&Value::Object(negative::converted(&mono, &s).unwrap().0), &c["mono"], "converted mono");
    }
    let roll_settings: Vec<Settings> = meta["roll_settings"].as_array().unwrap().iter().map(settings).collect();
    let samples: Vec<Array2<f32>> =
        (0..3).map(|i| negative::sample(&image(&ins, &format!("frame_{i}")), &roll_settings[i]).unwrap()).collect();
    let roll = negative::converted_together(&samples, &roll_settings);
    for (i, (ours, theirs)) in roll.iter().zip(meta["roll"].as_array().unwrap()).enumerate() {
        check(&Value::Object(ours.0.clone()), theirs, &format!("converted_together frame {i}"));
    }
    let placed = negative::placed(&grainy_core::defaults(), samples[0].view(), [0.8, 0.5, 0.27], [0.01, 0.005, 0.002]);
    check(&Value::Object(placed.0), &meta["placed"], "placed");
    assert_eq!(Value::Object(negative::defaults()), meta["defaults"]);
    assert_eq!(serde_json::to_value(negative::KEYS).unwrap(), meta["keys"]);
}
