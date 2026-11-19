//! Golden tests against the Python oracle (`tools/golden/gen_color.py`, `golden/color/`).
//! Run with `-- --nocapture` to see the largest error of each case.

use grainy_color as color;
use grainy_core::{Image, Plane};
use grainy_testkit::{assert_close, input, Case};
use ndarray::s;

/// Arithmetic tolerance of the brief; Lab (values up to 100 in `float32`) and HSL get their own, stated below.
const TOL: f32 = 1e-5;
const TOL_LAB: f32 = 2e-4;

const STANDARD: [&str; 7] = ["ramp", "wide", "colours", "patches", "noise", "edges", "garden"];

/// The input of a golden case: the stored hand-made array, or a standard input by name (garden is stored at
/// every other pixel).
fn load(case: &Case, name: &str) -> Image {
    if name == "hand" {
        return case.array("input");
    }
    let a = input(name);
    if name == "garden" {
        a.slice(s![..;2, ..;2, ..]).to_owned()
    } else {
        a
    }
}

fn names() -> Vec<&'static str> {
    let mut v = STANDARD.to_vec();
    v.push("hand");
    v
}

fn clip(a: &Image) -> Image {
    a.mapv(|v| v.clamp(0.0, 1.0))
}

fn per_input(prefix: &str, f: impl Fn(&Case, &str)) {
    for name in names() {
        let case = Case::open("color", &format!("{prefix}_{name}"));
        f(&case, name);
    }
}

#[test]
fn to_linear_matches() {
    per_input("to_linear", |case, name| {
        let a = load(case, name);
        assert_close(&color::to_linear(&a), &case.array("output"), TOL, &format!("to_linear {name}"));
    });
}

#[test]
fn to_srgb_matches() {
    per_input("to_srgb", |case, name| {
        let a = load(case, name);
        assert_close(&color::to_srgb(&a), &case.array("output"), TOL, &format!("to_srgb {name}"));
    });
}

#[test]
fn to_srgb_scalars_match() {
    let case = Case::open("color", "to_srgb_scalars");
    let xs = case.json()["inputs"].as_array().unwrap();
    let ys = case.json()["outputs"].as_array().unwrap();
    for (x, y) in xs.iter().zip(ys) {
        let got = color::srgb_encode(x.as_f64().unwrap() as f32);
        assert!((got - y.as_f64().unwrap() as f32).abs() <= TOL, "srgb_encode({x}) = {got}, oracle {y}");
    }
}

#[test]
fn srgb_round_trip_matches() {
    for name in ["colours", "patches", "garden", "hand"] {
        let case = Case::open("color", &format!("srgb_round_trip_{name}"));
        let a = load(&case, name).mapv(|v| v.max(0.0));
        let back = color::to_linear(&color::to_srgb(&a));
        assert_close(&back, &case.array("output"), TOL, &format!("srgb round trip {name}"));
    }
}

#[test]
fn clip01_matches() {
    for name in ["wide", "hand", "noise"] {
        let case = Case::open("color", &format!("clip01_{name}"));
        let a = load(&case, name);
        let expected: Image = case.array("output");
        assert_close(&color::clip01(&a), &expected, 0.0, &format!("clip01 {name}"));
        let mut b = a.clone();
        color::clip01_in_place(&mut b);
        assert_close(&b, &expected, 0.0, &format!("clip01_in_place {name}"));
    }
}

#[test]
fn rgb_to_hsv_matches() {
    per_input("rgb_to_hsv", |case, name| {
        let a = load(case, name);
        let (h, s, v) = color::rgb_to_hsv(&a);
        assert_close(&h, &case.array::<f32, _>("h"), TOL, &format!("hsv hue {name}"));
        assert_close(&s, &case.array::<f32, _>("s"), TOL, &format!("hsv sat {name}"));
        assert_close(&v, &case.array::<f32, _>("v"), TOL, &format!("hsv value {name}"));
    });
}

#[test]
fn hsv_to_rgb_matches() {
    let case = Case::open("color", "hsv_to_rgb_hand");
    let (h, s, v): (Plane, Plane, Plane) = (case.array("h"), case.array("s"), case.array("v"));
    let out = color::hsv_to_rgb(&h, &s, &v).unwrap();
    assert_close(&out, &case.array("output"), TOL, "hsv_to_rgb hand");
}

#[test]
fn hsv_round_trip_matches() {
    for name in ["colours", "patches", "garden", "hand"] {
        let case = Case::open("color", &format!("hsv_round_trip_{name}"));
        let c = clip(&load(&case, name));
        let (h, s, v) = color::rgb_to_hsv(&c);
        let back = color::hsv_to_rgb(&h, &s, &v).unwrap();
        assert_close(&back, &case.array("output"), TOL, &format!("hsv round trip {name}"));
    }
}

#[test]
fn rgb_to_hsl_matches() {
    per_input("rgb_to_hsl", |case, name| {
        let a = load(case, name);
        let (h, s, l) = color::rgb_to_hsl(&a);
        assert_close(&h, &case.array::<f32, _>("h"), TOL, &format!("hsl hue {name}"));
        assert_close(&s, &case.array::<f32, _>("s"), TOL, &format!("hsl sat {name}"));
        assert_close(&l, &case.array::<f32, _>("l"), TOL, &format!("hsl light {name}"));
    });
}

#[test]
fn hsl_to_rgb_matches() {
    let case = Case::open("color", "hsl_to_rgb_hand");
    let (h, s, l): (Plane, Plane, Plane) = (case.array("h"), case.array("s"), case.array("l"));
    let out = color::hsl_to_rgb(&h, &s, &l).unwrap();
    assert_close(&out, &case.array("output"), TOL, "hsl_to_rgb hand");
}

#[test]
fn hsl_round_trip_matches() {
    for name in ["colours", "patches", "garden", "hand"] {
        let case = Case::open("color", &format!("hsl_round_trip_{name}"));
        let a = load(&case, name);
        let (h, s, l) = color::rgb_to_hsl(&a);
        let back = color::hsl_to_rgb(&h, &s, &l).unwrap();
        assert_close(&back, &case.array("output"), TOL, &format!("hsl round trip {name}"));
    }
}

#[test]
fn rgb_to_lab_matches() {
    per_input("rgb_to_lab", |case, name| {
        let a = load(case, name);
        assert_close(&color::rgb_to_lab(&a), &case.array("output"), TOL_LAB, &format!("rgb_to_lab {name}"));
    });
}

#[test]
fn lab_to_rgb_matches() {
    let case = Case::open("color", "lab_to_rgb_grid");
    let lab: Image = case.array("input");
    assert_close(&color::lab_to_rgb(&lab), &case.array("output"), TOL, "lab_to_rgb grid");
    let case = Case::open("color", "lab_to_rgb_neutral");
    let lab: Image = case.array("input");
    assert_close(&color::lab_to_rgb(&lab), &case.array("output"), TOL, "lab_to_rgb neutral");
}

#[test]
fn lab_round_trip_matches() {
    for name in ["colours", "patches", "garden", "hand"] {
        let case = Case::open("color", &format!("lab_round_trip_{name}"));
        let c = clip(&load(&case, name));
        let back = color::lab_to_rgb(&color::rgb_to_lab(&c));
        assert_close(&back, &case.array("output"), TOL, &format!("lab round trip {name}"));
    }
}

// ---------------------------------------------------------------- illuminants

fn f(v: &serde_json::Value) -> f64 {
    v.as_f64().unwrap()
}

fn pair(v: &serde_json::Value) -> [f64; 2] {
    [f(&v[0]), f(&v[1])]
}

fn triple(v: &serde_json::Value) -> [f64; 3] {
    [f(&v[0]), f(&v[1]), f(&v[2])]
}

fn bytes(v: &serde_json::Value) -> Option<Vec<u8>> {
    v.as_array().map(|a| a.iter().map(|b| b.as_u64().unwrap() as u8).collect())
}

/// Checks `got` against an oracle record (`error`, `none` or `value`), returns the largest error.
fn check(case: &serde_json::Value, got: Result<Option<Vec<f64>>, color::ColorError>, what: &str) -> f64 {
    if case.get("error").is_some() {
        assert!(got.is_err(), "{what}: the oracle raises, got {got:?}");
        return 0.0;
    }
    let got = got.unwrap_or_else(|e| panic!("{what}: unexpected error {e}"));
    if case.get("none").is_some() {
        assert!(got.is_none(), "{what}: the oracle gives None");
        return 0.0;
    }
    let got = got.unwrap_or_else(|| panic!("{what}: got None"));
    let expected: Vec<f64> = case["value"].as_array().unwrap().iter().map(f).collect();
    assert_eq!(got.len(), expected.len(), "{what}");
    got.iter().zip(&expected).map(|(g, e)| (g - e).abs() / e.abs().max(1.0)).fold(0.0, f64::max)
}

const TOL_F64: f64 = 1e-9;

#[test]
fn illuminant_tables_match() {
    let case = Case::open("color", "illuminants_tables");
    let j = case.json();
    for (code, xy) in color::illuminants::STANDARD_XY {
        assert_eq!(pair(&j["standard_xy"][code.to_string()]), xy, "standard xy {code}");
    }
    assert_eq!(j["standard_xy"].as_object().unwrap().len(), color::illuminants::STANDARD_XY.len());
    let table = color::illuminants::observer().unwrap();
    assert_eq!(table.len() as u64, j["observer_rows"].as_u64().unwrap());
    let sum: Vec<f64> = (0..4).map(|c| table.iter().map(|r| r[c]).sum()).collect();
    for c in 0..4 {
        assert!((sum[c] - f(&j["observer_sum"][c])).abs() < 1e-9, "observer column {c}");
    }
    for (key, row) in [("observer_first", 0), ("observer_mid", 200), ("observer_last", 470)] {
        for c in 0..4 {
            assert_eq!(table[row][c], f(&j[key][c]), "{key} column {c}");
        }
    }
    // the isotemperature lines are checked through temperature_tint/temperature_xy below; the 325 mired entry too
    assert_eq!(j["lines"].as_array().unwrap().len(), 31);
    assert_eq!(f(&j["lines"][19][1]), 0.24702);
}

#[test]
fn temperature_tint_matches() {
    let case = Case::open("color", "illuminants_temperature_tint");
    let mut worst = 0.0f64;
    for c in case.json()["cases"].as_array().unwrap() {
        let xy = pair(&c["xy"]);
        let got = color::illuminants::temperature_tint(xy).map(|(k, t)| Some(vec![k, t]));
        worst = worst.max(check(c, got, &format!("temperature_tint {xy:?}")));
    }
    println!("temperature_tint: max relative error {worst:.3e}");
    assert!(worst < TOL_F64);
}

#[test]
fn temperature_xy_matches() {
    let case = Case::open("color", "illuminants_temperature_xy");
    let mut worst = 0.0f64;
    for c in case.json()["cases"].as_array().unwrap() {
        let (k, t) = (f(&c["kelvin"]), f(&c["tint"]));
        let got = color::illuminants::temperature_xy(k, t).map(|xy| Some(xy.to_vec()));
        worst = worst.max(check(c, got, &format!("temperature_xy {k} {t}")));
    }
    println!("temperature_xy: max relative error {worst:.3e}");
    assert!(worst < TOL_F64);
}

#[test]
fn xy_xyz_conversions_match() {
    let case = Case::open("color", "illuminants_xy_xyz");
    let mut worst = 0.0f64;
    for c in case.json()["xyz_to_xy"].as_array().unwrap() {
        let got = color::illuminants::xyz_to_xy(triple(&c["xyz"])).map(|v| Some(v.to_vec()));
        worst = worst.max(check(c, got, "xyz_to_xy"));
    }
    for c in case.json()["xy_to_xyz"].as_array().unwrap() {
        let got = color::illuminants::xy_to_xyz(pair(&c["xy"])).map(|v| Some(v.to_vec()));
        worst = worst.max(check(c, got, "xy_to_xyz"));
    }
    println!("xy/xyz: max relative error {worst:.3e}");
    assert!(worst < TOL_F64);
    assert!(color::illuminants::xyz_to_xy([f64::NAN, 1.0, 1.0]).is_err());
    assert!(color::illuminants::xy_to_xyz([f64::INFINITY, 0.3]).is_err());
    assert!(color::illuminants::temperature_xy(f64::INFINITY, 0.0).is_err());
    assert!(color::illuminants::temperature_tint([f64::NAN, 0.3]).is_err());
}

#[test]
fn custom_xy_matches() {
    let case = Case::open("color", "illuminants_custom_xy");
    let mut worst = 0.0f64;
    let mut count = 0;
    for c in case.json()["cases"].as_array().unwrap() {
        let little = c["endian"].as_str().unwrap() == "<";
        let got = match bytes(&c["payload"]) {
            Some(p) => color::illuminants::custom_xy(&p, little).map(|v| Some(v.to_vec())),
            None => color::illuminants::light_xy(255, None, little).map(|v| v.map(|x| x.to_vec())),
        };
        worst = worst.max(check(c, got, &format!("custom_xy case {count}")));
        count += 1;
    }
    println!("custom_xy: {count} cases, max relative error {worst:.3e}");
    assert!(worst < TOL_F64);
}

#[test]
fn light_xy_matches() {
    let case = Case::open("color", "illuminants_light_xy");
    let mut worst = 0.0f64;
    for c in case.json()["cases"].as_array().unwrap() {
        let code = c["code"].as_i64().unwrap();
        let little = c.get("endian").is_none_or(|e| e == "<");
        let payload = c.get("payload").and_then(bytes);
        let got = color::illuminants::light_xy(code, payload.as_deref(), little).map(|v| v.map(|x| x.to_vec()));
        worst = worst.max(check(c, got, &format!("light_xy {code}")));
    }
    println!("light_xy: max relative error {worst:.3e}");
    assert!(worst < TOL_F64);
}

#[test]
fn triple_weights_matches() {
    let case = Case::open("color", "illuminants_triple_weights");
    let mut worst = 0.0f64;
    for c in case.json()["cases"].as_array().unwrap() {
        let w = &c["whites"];
        let whites = [pair(&w[0]), pair(&w[1]), pair(&w[2])];
        let got = color::illuminants::triple_weights(&whites, triple(&c["white"])).map(|v| Some(v.to_vec()));
        worst = worst.max(check(c, got, "triple_weights"));
    }
    println!("triple_weights: max relative error {worst:.3e}");
    assert!(worst < TOL_F64);
}

#[test]
fn reexports_exist() {
    let rgb = color::kelvin_rgb(6500.0);
    assert!(rgb.iter().all(|v| *v > 0.0));
    let _ = color::white_balance;
}
