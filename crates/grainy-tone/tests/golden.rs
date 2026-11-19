//! Golden tests of grainy-tone against the Python oracle (`tools/golden/gen_tone.py`, `golden/tone/`).

use std::sync::Mutex;

use grainy_core::{Image, Plane, Settings};
use grainy_testkit::{assert_close, input, max_abs_diff, root, Case};
use grainy_tone::{cv, hdr, interp, mapping, response, srgb, FilmRender};
use ndarray::{s, Array1, Array2, Array3, ArrayView3, Axis};
use serde_json::Value;

/// Plain float arithmetic.
const ARITH: f32 = 1e-5;
/// Where float32 rounding order differs (powf/log2/exp2 implementations, sums).
const ROUNDING: f32 = 2e-4;
/// Ports of OpenCV filters.
const OPENCV: f32 = 1.0 / 255.0;

fn settings(value: &Value) -> Settings {
    Settings::from_value(value.clone()).expect("settings object")
}

fn neg() -> Image {
    input("noise").mapv(|v| v - 0.2f32)
}

/// A stage input: the standard input (or `neg`) cropped as the generator crops it.
fn stage_input(case: &Case) -> Image {
    let name = case.json()["input"].as_str().unwrap();
    let full = if name == "neg" { neg() } else { input(name) };
    let crop: Vec<usize> =
        case.json()["crop"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
    full.slice(s![crop[0]..crop[1], crop[2]..crop[3], ..]).to_owned()
}

/// The generator's `procedural(height, width)`.
fn procedural(height: usize, width: usize) -> Image {
    Array3::from_shape_fn((height, width, 3), |(y, x, c)| {
        let k = ((7 * x + 13 * y + 5 * c) % 101) as f32;
        k * 0.015f32 + (x as f32) * 0.002f32
    })
}

fn dims(case: &Case) -> (usize, usize) {
    (case.json()["height"].as_u64().unwrap() as usize, case.json()["width"].as_u64().unwrap() as usize)
}

fn max_diff_f64(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| if x.is_nan() && y.is_nan() { 0.0 } else { (x - y).abs() }).fold(0.0, f64::max)
}

fn cases(prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root().join("golden").join("tone"))
        .expect("golden/tone exists; run tools/golden/gen_tone.py")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort();
    names
}

#[test]
fn highlight_rolloff() {
    for name in cases("rolloff_") {
        if name == "rolloff_white" {
            continue;
        }
        let case = Case::open("tone", &name);
        let j = case.json();
        let out = grainy_tone::highlight_rolloff(
            &input(j["input"].as_str().unwrap()),
            j["white"].as_f64().unwrap(),
            j["knee"].as_f64().unwrap(),
        );
        assert_close(&out, &case.array("output"), ARITH, &name);
    }
}

#[test]
fn rolloff_white() {
    let case = Case::open("tone", "rolloff_white");
    let mut worst = 0f64;
    for item in case.json()["cases"].as_array().unwrap() {
        let s = settings(&item["settings"]);
        worst = worst.max((grainy_tone::rolloff_white(&s) - item["rolloff_white"].as_f64().unwrap()).abs());
    }
    println!("rolloff_white: max abs error {worst:.3e}");
    assert!(worst <= 1e-12);
}

#[test]
fn srgb_copies() {
    let case = Case::open("tone", "srgb");
    let x: Image = case.array("input");
    assert_close(&srgb::to_srgb(&x), &case.array("srgb"), ARITH, "to_srgb");
    assert_close(&srgb::to_linear(&x), &case.array("linear"), ARITH, "to_linear");
}

#[test]
fn numpy_interp() {
    let case = Case::open("tone", "interp");
    let x: Array1<f64> = case.array("x");
    let xp: Array1<f64> = case.array("xp");
    let fp: Array1<f64> = case.array("fp");
    let expected: Array1<f64> = case.array("output");
    let out: Vec<f64> = x.iter().map(|&v| interp::interp(v, xp.as_slice().unwrap(), fp.as_slice().unwrap())).collect();
    let worst = max_diff_f64(&out, expected.as_slice().unwrap());
    println!("interp: max abs error {worst:.3e}");
    assert!(worst <= 1e-15);
}

#[test]
fn response_curve() {
    let case = Case::open("tone", "response_curve");
    let expected: Array2<f32> = case.array("curves");
    let pairs = case.json()["pairs"].as_array().unwrap();
    let mut out = Array2::<f32>::zeros(expected.raw_dim());
    for (i, pair) in pairs.iter().enumerate() {
        let curve = response::curve(pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap());
        out.row_mut(i).iter_mut().zip(curve).for_each(|(o, v)| *o = v);
    }
    assert_close(&out, &expected, ARITH, "tone_response.curve");
}

#[test]
fn box_filter() {
    for name in cases("box_") {
        let case = Case::open("tone", &name);
        let plane: Plane = case.array("input");
        let r = case.json()["r"].as_u64().unwrap() as usize;
        assert_close(&cv::box_filter(&plane, r), &case.array("output"), OPENCV, &name);
    }
}

#[test]
fn resize_area() {
    for name in cases("area_") {
        let case = Case::open("tone", &name);
        let j = case.json();
        let source = input(j["input"].as_str().unwrap());
        let out =
            cv::resize_area(&source, j["width"].as_u64().unwrap() as usize, j["height"].as_u64().unwrap() as usize)
                .expect("a reduction");
        assert_close(&out, &case.array("output"), OPENCV, &name);
    }
}

#[test]
fn resize_linear_columns() {
    for name in cases("linear_") {
        let case = Case::open("tone", &name);
        let band: Array2<f64> = case.array("input");
        let expected: Array2<f64> = case.array("output");
        let width = case.json()["width"].as_u64().unwrap() as usize;
        let table = cv::LinearColumns::new(band.ncols(), width);
        let mut worst = 0f64;
        for (row, want) in band.outer_iter().zip(expected.outer_iter()) {
            let mut out = vec![0f64; width];
            table.row(&row.to_vec(), &mut out);
            worst = worst.max(max_diff_f64(&out, &want.to_vec()));
        }
        println!("{name}: max abs error {worst:.3e} (tolerance {OPENCV:.1e})");
        assert!(worst <= f64::from(OPENCV));
    }
}

#[test]
fn analyse() {
    for name in cases("analyse_") {
        let case = Case::open("tone", &name);
        let encoded: Image = case.array("input");
        let (h, w, _) = encoded.dim();
        let base = response::analyse(&encoded, (h, w));
        assert_close(&base.a, &case.array("a"), OPENCV, &format!("{name} a"));
        assert_close(&base.b, &case.array("b"), OPENCV, &format!("{name} b"));
    }
}

#[test]
fn apply() {
    let frame: Image = Case::open("tone", "apply_frame").array("frame");
    for name in cases("apply_") {
        if name == "apply_frame" {
            continue;
        }
        let case = Case::open("tone", &name);
        let j = case.json();
        let shape: Vec<usize> = j["shape"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
        let base = response::ToneBase { a: case.array("a"), b: case.array("b"), shape: (shape[0], shape[1]) };
        let local = response::with_curve(base, j["highlights"].as_f64().unwrap(), j["shadows"].as_f64().unwrap());
        let (first, stop) = (j["first"].as_u64().unwrap() as usize, j["stop"].as_u64().unwrap() as usize);
        let out = response::apply(frame.slice(s![first..stop, .., ..]), &local, first).mapv(|v| v as f32);
        assert_close(&out, &case.array("output"), ARITH, &name);
    }
}

#[test]
fn tone_base() {
    for name in cases("tone_base_") {
        let case = Case::open("tone", &name);
        let (h, w) = dims(&case);
        let s = settings(&case.json()["settings"]);
        let base = grainy_tone::tone_base(&procedural(h, w), &s, None).unwrap();
        assert_eq!(base.shape, (h, w));
        assert_close(&base.a, &case.array("a"), OPENCV, &format!("{name} a"));
        assert_close(&base.b, &case.array("b"), OPENCV, &format!("{name} b"));
    }
}

/// A film render that returns the oracle's stored print and checks it is given the oracle's input.
fn stored_film(case: &Case) -> Option<(Image, Image)> {
    case.has("film").then(|| (case.array("film_input"), case.array("film")))
}

#[test]
fn stage() {
    let mut worst = (0f32, String::new());
    for name in cases("stage_") {
        let case = Case::open("tone", &name);
        let j = case.json();
        if j.get("step").is_some() {
            continue; // the large frames have their own test
        }
        let s = settings(&j["settings"]);
        let a = stage_input(&case);
        let film = stored_film(&case);
        let input_error = Mutex::new(0f32);
        let render = |view: ArrayView3<'_, f32>| -> Image {
            let (into, print) = film.as_ref().expect("a film case");
            let (e, _) = max_abs_diff(&view.to_owned(), into);
            let mut worst = input_error.lock().unwrap();
            *worst = worst.max(e);
            print.clone()
        };
        let film_fn: Option<FilmRender<'_>> = film.as_ref().map(|_| &render as FilmRender<'_>);
        let out = grainy_tone::develop_tone(&a, &s, film_fn).unwrap();
        let expected: Image = case.array("output");
        let (e, _) = max_abs_diff(&out, &expected);
        if film.is_some() {
            let e_in = *input_error.lock().unwrap();
            println!("{name}: film input max abs error {e_in:.3e}");
            assert!(e_in <= ARITH, "{name}: film input differs");
        }
        if e > worst.0 {
            worst = (e, name.clone());
        }
        assert_close(&out, &expected, ROUNDING, &format!("{name} ({})", j["dtype"].as_str().unwrap()));
    }
    println!("stage: largest error {:.3e} in {}", worst.0, worst.1);
}

#[test]
fn stage_defaults_exact() {
    // The default settings: the oracle's result (sRGB encoding of the input, clipped).
    for name in [
        "stage_default_ramp",
        "stage_default_wide",
        "stage_default_colours",
        "stage_default_garden",
        "stage_default_noise",
    ] {
        let case = Case::open("tone", name);
        let s = grainy_core::normalized(&serde_json::Map::new());
        assert_eq!(s, settings(&case.json()["settings"]));
        let out = grainy_tone::develop_tone(&stage_input(&case), &s, None).unwrap();
        assert_close(&out, &case.array("output"), ARITH, name);
    }
}

#[test]
fn stage_large_frames() {
    for name in cases("stage_large_") {
        let case = Case::open("tone", &name);
        let j = case.json();
        let (h, w) = dims(&case);
        let s = settings(&j["settings"]);
        let rows = Mutex::new(Vec::new());
        // The generator's stand-in render: clip(a*0.8 + mean of the band's first row over channels, 0, 1).
        let render = |a: ArrayView3<'_, f32>| -> Image {
            rows.lock().unwrap().push(a.dim().0);
            let first = a.index_axis(Axis(0), 0).to_owned();
            let mut out = a.to_owned();
            for mut row in out.outer_iter_mut() {
                for (x, mut pixel) in row.outer_iter_mut().enumerate() {
                    let m = (first[[x, 0]] + first[[x, 1]] + first[[x, 2]]) / 3.0;
                    pixel.iter_mut().for_each(|v| *v = (*v * 0.8f32 + m).clamp(0.0, 1.0));
                }
            }
            out
        };
        let film_calls = j["film_calls"].as_u64().unwrap();
        let film_fn: Option<FilmRender<'_>> = (film_calls > 0).then_some(&render as FilmRender<'_>);
        let out = grainy_tone::develop_tone(&procedural(h, w), &s, film_fn).unwrap();
        let strided = out.slice(s![..;7, ..;5, ..]).to_owned();
        assert_close(&strided, &case.array("output"), ROUNDING, &format!("{name} ({})", j["dtype"].as_str().unwrap()));
        if film_calls > 0 {
            let mut got = rows.lock().unwrap().clone();
            let mut want: Vec<usize> =
                j["film_rows"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
            got.sort_unstable();
            want.sort_unstable();
            assert_eq!(got, want, "{name}: film bands");
        }
    }
}

#[test]
fn pixels_band() {
    let case = Case::open("tone", "pixels_band");
    let j = case.json();
    let s = settings(&j["settings"]);
    let garden = input("garden").slice(s![40..120, 60..180, ..]).to_owned();
    let local = grainy_tone::tone_local(&garden, &s, None, None).unwrap();
    assert!(local.is_some());
    let (first, stop) = (j["first"].as_u64().unwrap() as usize, j["stop"].as_u64().unwrap() as usize);
    let out = grainy_tone::develop_tone_pixels(garden.slice(s![first..stop, .., ..]), &s, local.as_ref(), first, None)
        .unwrap();
    assert_close(&out, &case.array("output"), ROUNDING, "develop_tone_pixels band");
}

#[test]
fn drt() {
    let case = Case::open("tone", "drt");
    let x: Array1<f32> = case.array("x");
    let curves: Array2<f32> = case.array("curves");
    let renders: ndarray::Array4<f32> = case.array("renders");
    let source = neg().slice(s![..16, .., ..]).mapv(|v| v * 4.0f32);
    let mut worst = 0f64;
    for (i, item) in case.json()["cases"].as_array().unwrap().iter().enumerate() {
        let s = grainy_core::defaults();
        let mut s = s;
        for (k, v) in item["settings"].as_object().unwrap() {
            s.set(k, v.clone());
        }
        let p = mapping::params(&s);
        let want: Vec<f64> = item["params"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let got = [p.p, p.s, p.m, p.toe, p.white, p.shoulder];
        worst = worst.max(max_diff_f64(&got, &want));
        if let Some(look) = item["look"].as_str() {
            assert_eq!(mapping::look_of(&s), look, "look_of case {i}");
        }
        let curve: Array1<f32> = x.mapv(|v| mapping::curve(v, &p));
        assert_close(&curve, &curves.row(i).to_owned(), ROUNDING, &format!("drt curve {i}"));
        let render = mapping::render(&source, &s);
        assert_close(&render, &renders.index_axis(Axis(0), i).to_owned(), ROUNDING, &format!("drt render {i}"));
    }
    println!("drt params: max abs error {worst:.3e}");
    assert!(worst <= 1e-12);
}

#[test]
fn drt_looks() {
    let case = Case::open("tone", "drt_looks");
    let names: Vec<&str> = case.json()["names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(names, mapping::LOOKS.iter().map(|(n, _)| *n).collect::<Vec<_>>());
    for item in case.json()["cases"].as_array().unwrap() {
        let s = settings(&item["settings"]);
        assert_eq!(mapping::look_of(&s), item["look"].as_str().unwrap());
        assert_eq!(mapping::active(&s), item["active"].as_bool().unwrap());
    }
}

#[test]
fn hdr_functions() {
    let case = Case::open("tone", "hdr");
    let j = case.json();
    let a: Image = case.array("input");
    let stack = |name: &str| -> ndarray::Array4<f32> { case.array(name) };
    let (curves, limits, sdr) = (stack("curves"), stack("limits"), stack("sdr"));
    for (i, points) in j["curves"].as_array().unwrap().iter().enumerate() {
        let points: hdr::Points = points
            .as_array()
            .unwrap()
            .iter()
            .map(|p| [p[0].as_f64().unwrap() as f32, p[1].as_f64().unwrap() as f32])
            .collect();
        assert_close(
            &hdr::curve_extended(&a, &points),
            &curves.index_axis(Axis(0), i).to_owned(),
            ARITH,
            &format!("curve_extended {i}"),
        );
    }
    let with = |changes: &Value| {
        let mut s = grainy_core::defaults();
        for (k, v) in changes.as_object().unwrap() {
            s.set(k, v.clone());
        }
        s
    };
    for (i, c) in j["limits"].as_array().unwrap().iter().enumerate() {
        assert_close(
            &hdr::limit(&a, &with(c)),
            &limits.index_axis(Axis(0), i).to_owned(),
            ROUNDING,
            &format!("limit {i}"),
        );
    }
    for (i, c) in j["sdr"].as_array().unwrap().iter().enumerate() {
        assert_close(
            &hdr::sdr_rendition(&a, &with(c)),
            &sdr.index_axis(Axis(0), i).to_owned(),
            ROUNDING,
            &format!("sdr_rendition {i}"),
        );
    }
    let op = hdr::color_operation(&a, |b| b.mapv(|v| (v * 1.2f32 - 0.05f32).clamp(0.0, 1.0)));
    assert_close(&op, &case.array("operation"), ARITH, "color_operation");
}

#[test]
fn thread_count_does_not_matter() {
    let frame = procedural(260, 530);
    let s = grainy_core::normalized(
        &serde_json::json!({"shadows": 60.0, "highlights": -45.0, "exposure": 0.8}).as_object().unwrap().clone(),
    );
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let serial = one.install(|| grainy_tone::develop_tone(&frame, &s, None).unwrap());
    let parallel = grainy_tone::develop_tone(&frame, &s, None).unwrap();
    assert_eq!(serial, parallel);
}
