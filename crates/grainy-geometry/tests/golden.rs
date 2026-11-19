//! Golden tests of grainy-geometry against the Python oracle (`tools/golden/gen_geometry.py`, `golden/geometry/`).

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;

use grainy_core::Settings;
use grainy_geometry::lens::{lens_geometry_with_budget, lens_shading};
use grainy_geometry::scan_border::{crop_for_frame, detect};
use grainy_geometry::upright::{self, Line};
use grainy_geometry::{
    geometry, optical_geometry, orient_pixels, rng, straighten, Error, LensModel, LensRequest, UseCrop,
};
use grainy_testkit::{assert_close, input, max_abs_diff, root, Case};
use ndarray::{Array2, Array3, Array4};
use serde_json::Value;

const MODULE: &str = "geometry";

fn synth(h: usize, w: usize, c: usize, seed: usize) -> Array3<f32> {
    Array3::from_shape_fn((h, w, c), |(y, x, ch)| {
        let k = ((7 * x + 13 * y + 5 * ch + 3 * seed) % 101) as f32;
        k * 0.015f32 + (x as f32) * 0.002f32
    })
}

fn source(spec: &Value) -> Array3<f32> {
    let n = |k: &str| spec[k].as_u64().unwrap() as usize;
    if spec["kind"] == "synth" {
        return synth(n("h"), n("w"), n("c"), n("seed"));
    }
    let a = input(spec["name"].as_str().unwrap());
    if spec["channels"].as_u64() == Some(1) {
        a.slice(ndarray::s![.., .., 1..2]).to_owned()
    } else {
        a
    }
}

fn settings(changes: &Value) -> Settings {
    let mut s = grainy_core::defaults();
    if let Some(map) = changes.as_object() {
        for (k, v) in map {
            s.set(k, v.clone());
        }
    }
    s
}

fn names(prefix: &str) -> Vec<String> {
    let text = std::fs::read_to_string(root().join("golden").join(MODULE).join("index.json"))
        .expect("golden/geometry/index.json; run tools/golden/gen_geometry.py");
    let all: Vec<String> = serde_json::from_str(&text).unwrap();
    let selected: Vec<String> = all.into_iter().filter(|n| n.starts_with(prefix)).collect();
    assert!(!selected.is_empty(), "no golden cases start with {prefix}");
    selected
}

fn error_matches(error: &Error, expected: &Value, case: &str) {
    let message = expected["message"].as_str().unwrap();
    match (error, expected["type"].as_str().unwrap()) {
        (Error::Value(m), "ValueError") | (Error::Alignment(m), "AlignmentError") => {
            assert_eq!(m, message, "{case}: message")
        }
        (e, t) => panic!("{case}: expected {t}({message}), got {e:?}"),
    }
}

/// Runs a geometry/optical case; returns the largest error (or 0 for an expected error).
fn run_case(name: &str, lens: Option<&dyn LensModel>) -> f32 {
    let case = Case::open(MODULE, name);
    let meta = case.json();
    let a = source(&meta["input"]);
    let s = settings(&meta["changes"]);
    let result = if meta["function"] == "optical" {
        optical_geometry(&a, &s, lens)
    } else {
        let use_crop = match &meta["use_crop"] {
            Value::String(t) if t == "all" => UseCrop::All,
            Value::Bool(false) => UseCrop::No,
            _ => UseCrop::Yes,
        };
        geometry(&a, &s, use_crop, lens)
    };
    if !meta["error"].is_null() {
        error_matches(&result.expect_err(name), &meta["error"], name);
        return 0.0;
    }
    let result = result.unwrap_or_else(|e| panic!("{name}: {e}"));
    let expected: Array3<f32> = case.array("output");
    assert_eq!(result.dim(), expected.dim(), "{name}: shape");
    max_abs_diff(&result, &expected).0
}

fn run_group(prefix: &str, tolerance: f32) -> f32 {
    let mut worst = (0.0f32, String::new());
    for name in names(prefix) {
        if name == "straighten_functions" {
            continue;
        }
        let e = run_case(&name, None);
        assert!(e <= tolerance, "{name}: max abs error {e:e} > {tolerance:e}");
        if e > worst.0 || worst.1.is_empty() {
            worst = (e.max(worst.0), name);
        }
    }
    println!("{prefix}*: max abs error {:.3e} (tolerance {tolerance:.1e}; worst {})", worst.0, worst.1);
    worst.0
}

#[test]
fn quarter_turns_flip_crop_are_exact() {
    run_group("orient_c", 0.0);
    run_group("orient_garden", 0.0);
    run_group("orient_odd", 0.0);
    run_group("orient_nocrop", 0.0);
    run_group("orient_all", 0.0);
    run_group("noop", 0.0);
    let a = input("edges");
    assert!(matches!(geometry(&a, &grainy_core::defaults(), UseCrop::Yes, None).unwrap(), Cow::Borrowed(_)));
}

#[test]
fn straighten_matches_pillow() {
    run_group("straighten_", 0.0);
}

#[test]
fn manual_transform_and_guides() {
    run_group("optical_", 0.0);
    run_group("geometry_", 0.0);
    let a = synth(37, 53, 3, 8);
    let s = settings(&serde_json::json!({"transform_scale": 100.0, "distortion": 0}));
    assert!(matches!(optical_geometry(&a, &s, None).unwrap(), Cow::Borrowed(_)));
}

#[test]
fn upright_apply() {
    run_group("upright_", 0.0);
}

#[test]
fn orient_pixels_matches() {
    for name in names("orient_pixels") {
        let case = Case::open(MODULE, &name);
        let a = source(&case.json()["input"]);
        let expected: Array3<f32> = case.array("output");
        let got = orient_pixels(a.view(), case.json()["orientation"].as_i64().unwrap());
        assert_eq!(got, expected, "{name}");
    }
}

fn lines_of(case: &Case) -> Option<Array2<f32>> {
    case.has("lines").then(|| case.array("lines"))
}

fn segments(a: &Array2<f32>, width: usize, height: usize) -> (Vec<Line>, Vec<f64>) {
    // estimate's selection of segments, as the generator's `filtered`.
    let mut lines: Vec<Line> = a
        .rows()
        .into_iter()
        .map(|r| [[f64::from(r[0]), f64::from(r[1])], [f64::from(r[2]), f64::from(r[3])]])
        .collect();
    let mut lengths: Vec<f64> = lines.iter().map(|l| (l[1][0] - l[0][0]).hypot(l[1][1] - l[0][1])).collect();
    let shortest = (height.min(width) as f64 * 0.06).max(20.0);
    let keep: Vec<bool> = lengths.iter().map(|&l| l >= shortest).collect();
    lines = lines.into_iter().zip(&keep).filter(|(_, &k)| k).map(|(l, _)| l).collect();
    lengths = lengths.into_iter().zip(&keep).filter(|(_, &k)| k).map(|(l, _)| l).collect();
    if lines.len() > 160 {
        let mut order: Vec<usize> = (0..lengths.len()).collect();
        order.sort_by(|&a, &b| lengths[a].total_cmp(&lengths[b]));
        let top = &order[order.len() - 160..];
        lines = top.iter().map(|&i| lines[i]).collect();
        lengths = top.iter().map(|&i| lengths[i]).collect();
    }
    (lines, lengths)
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

struct Worst(&'static str, f64);

impl Worst {
    fn check(&mut self, got: f64, expected: f64, tolerance: f64, what: &str) {
        let e = (got - expected).abs();
        assert!(e <= tolerance, "{what}: got {got}, expected {expected} (error {e:e} > {tolerance:e})");
        self.1 = self.1.max(e);
    }
}

#[test]
fn upright_estimate() {
    let mut worst_matrix = Worst("matrix", 0.0);
    let mut worst_scale = Worst("crop_scale", 0.0);
    let mut worst_angle = Worst("rotation/confidence", 0.0);
    for name in names("estimate_") {
        let case = Case::open(MODULE, &name);
        let meta = case.json();
        let (w, h) = (meta["width"].as_u64().unwrap() as usize, meta["height"].as_u64().unwrap() as usize);
        let lines = lines_of(&case);
        let result =
            upright::estimate_from_lines(w, h, lines.as_ref().map(|l| l.view()), meta["mode"].as_str().unwrap());
        if !meta["error"].is_null() {
            error_matches(&result.expect_err(&name), &meta["error"], &name);
            continue;
        }
        let got = result.unwrap_or_else(|e| panic!("{name}: {e}"));
        let r = &meta["result"];
        assert_eq!(got.mode, r["mode"].as_str().unwrap(), "{name}: mode");
        assert_eq!(got.requested_mode, r["requested_mode"].as_str().unwrap(), "{name}");
        assert_eq!(got.algorithm, r["algorithm"].as_str().unwrap(), "{name}: algorithm");
        assert_eq!(got.lines as u64, r["lines"].as_u64().unwrap(), "{name}: lines");
        assert_eq!(got.inliers as u64, r["inliers"].as_u64().unwrap(), "{name}: inliers");
        worst_angle.check(got.rotation, f(&r["rotation"]), 1e-9, &format!("{name}: rotation"));
        worst_angle.check(got.confidence, f(&r["confidence"]), 1e-9, &format!("{name}: confidence"));
        worst_scale.check(got.crop_scale, f(&r["crop_scale"]), 1e-6, &format!("{name}: crop_scale"));
        for i in 0..3 {
            for j in 0..3 {
                worst_matrix.check(got.matrix[i][j], f(&r["matrix"][i][j]), 1e-9, &format!("{name}: matrix[{i}][{j}]"));
            }
        }
    }
    for w in [worst_matrix, worst_scale, worst_angle] {
        println!("estimate {}: max abs error {:.3e}", w.0, w.1);
    }
}

fn expect_err<T>(result: Result<T, Error>, expected: &Value, what: &str) -> Option<T> {
    if !expected["error"].is_null() {
        match result {
            Err(e) => error_matches(&e, &expected["error"], what),
            Ok(_) => panic!("{what}: expected an error"),
        }
        return None;
    }
    Some(result.unwrap_or_else(|e| panic!("{what}: {e}")))
}

#[test]
fn upright_internals() {
    let mut worst = Worst("internals", 0.0);
    for name in names("internals_") {
        let case = Case::open(MODULE, &name);
        let meta = case.json();
        let (w, h) = (meta["width"].as_u64().unwrap() as usize, meta["height"].as_u64().unwrap() as usize);
        let (lines, lengths) = segments(&case.array("lines"), w, h);
        let r = &meta["results"];
        if let Some((a, c)) = expect_err(upright::roll(&lines, &lengths), &r["roll"], &format!("{name} roll")) {
            worst.check(a, f(&r["roll"]["value"][0]), 1e-9, &format!("{name} roll"));
            worst.check(c, f(&r["roll"]["value"][1]), 1e-12, &format!("{name} roll share"));
        }
        let what = format!("{name} horizon_roll");
        if let Some((a, c)) = expect_err(upright::horizon_roll(&lines, &lengths, w, h), &r["horizon_roll"], &what) {
            worst.check(a, f(&r["horizon_roll"]["value"][0]), 1e-9, &what);
            worst.check(c, f(&r["horizon_roll"]["value"][1]), 1e-12, &what);
        }
        let plumbs = upright::plumb_rolls(&lines, &lengths, w, h);
        let expected = r["plumb_rolls"]["value"].as_array().unwrap();
        assert_eq!(plumbs.len(), expected.len(), "{name}: plumb count");
        for (p, e) in plumbs.iter().zip(expected) {
            worst.check(p.angle, f(&e["angle"]), 1e-9, &format!("{name} plumb angle"));
            worst.check(p.score, f(&e["score"]), 1e-9, &format!("{name} plumb score"));
            worst.check(p.share, f(&e["share"]), 1e-12, &format!("{name} plumb share"));
            worst.check(p.heights, f(&e["heights"]), 1e-12, &format!("{name} plumb heights"));
            assert_eq!(p.count as u64, e["count"].as_u64().unwrap(), "{name}");
            assert_eq!(p.centred, e["centred"].as_bool().unwrap(), "{name}");
            assert_eq!(p.both, e["both"].as_bool().unwrap(), "{name}");
        }
        let wide = upright::wide_roll(&lines, &lengths, w, h);
        match (&wide, &r["wide_roll"]["value"]) {
            (None, Value::Null) => {}
            (Some((a, c)), Value::Array(e)) => {
                worst.check(*a, f(&e[0]), 1e-9, &format!("{name} wide"));
                worst.check(*c, f(&e[1]), 1e-12, &format!("{name} wide share"));
            }
            (got, e) => panic!("{name} wide_roll: got {got:?}, expected {e}"),
        }
        let what = format!("{name} level");
        if let Some((a, c, algorithm)) = expect_err(upright::level(&lines, &lengths, w, h), &r["level"], &what) {
            worst.check(a, f(&r["level"]["value"][0]), 1e-9, &what);
            worst.check(c, f(&r["level"]["value"][1]), 1e-12, &what);
            assert_eq!(algorithm, r["level"]["value"][2].as_str().unwrap(), "{what}");
        }
        for (key, vertical) in [("vanishing_v", true), ("vanishing_h", false)] {
            let scale = w.max(h) as f64 / 2.0;
            let (wf, hf) = (w as f64, h as f64);
            let normal = [
                [1.0 / scale, 0.0, -(wf - 1.0) / 2.0 / scale],
                [0.0, 1.0 / scale, -(hf - 1.0) / 2.0 / scale],
                [0.0, 0.0, 1.0],
            ];
            let leveled = upright::points(&normal, &lines).unwrap();
            let pick = |l: &Line| {
                let (dx, dy) = ((l[1][0] - l[0][0]).abs(), (l[1][1] - l[0][1]).abs());
                if vertical {
                    dy > dx * 1.5
                } else {
                    dx > dy * 1.5
                }
            };
            let chosen: Vec<usize> = (0..lines.len()).filter(|&i| pick(&lines[i])).collect();
            let sel_lines: Vec<Line> = chosen.iter().map(|&i| leveled[i]).collect();
            let sel_lengths: Vec<f64> = chosen.iter().map(|&i| lengths[i]).collect();
            let what = format!("{name} {key}");
            if let Some((point, count, conf)) =
                expect_err(upright::vanishing(&sel_lines, &sel_lengths, vertical), &r[key], &what)
            {
                let e = &r[key]["value"];
                let ep: Vec<f64> = e[0].as_array().unwrap().iter().map(f).collect();
                let sign = if point.iter().zip(&ep).map(|(a, b)| a * b).sum::<f64>() < 0.0 { -1.0 } else { 1.0 };
                for k in 0..3 {
                    worst.check(point[k] * sign, ep[k], 1e-9, &format!("{what} point"));
                }
                assert_eq!(count as u64, e[1].as_u64().unwrap(), "{what} count");
                worst.check(conf, f(&e[2]), 1e-12, &what);
            }
        }
    }
    println!("upright internals: max abs error {:.3e}", worst.1);
}

#[test]
fn upright_crop_scale_and_choice() {
    let case = Case::open(MODULE, "crop_scale");
    let mut worst = Worst("crop_scale", 0.0);
    for c in case.json()["cases"].as_array().unwrap() {
        let m: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| f(&c["matrix"][i][j])));
        let (w, h) = (c["width"].as_u64().unwrap() as usize, c["height"].as_u64().unwrap() as usize);
        let got = upright::crop_scale(&m, w, h);
        match (&c["value"][0], &c["value"][1]) {
            (Value::Null, e) => error_matches(&got.expect_err("crop_scale"), e, "crop_scale"),
            (v, _) => worst.check(got.unwrap(), f(v), 1e-6, "crop_scale"),
        }
    }
    println!("crop_scale: max abs error {:.3e}", worst.1);
    let case = Case::open(MODULE, "choice42");
    for c in case.json()["cases"].as_array().unwrap() {
        let n = c["n"].as_u64().unwrap() as usize;
        let expected: Vec<i64> = c["indices"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        assert_eq!(rng::choice(42, n, 500), expected, "choice({n}, 500)");
    }
}

fn crop4(v: &Value) -> [f64; 4] {
    std::array::from_fn(|i| f(&v[i]))
}

fn assert_crop(got: [f64; 4], expected: &Value, what: &str, worst: &mut f64) {
    for i in 0..4 {
        let e = (got[i] - f(&expected[i])).abs();
        assert!(e <= 1e-12, "{what}: {got:?} vs {expected}");
        *worst = worst.max(e);
    }
}

#[test]
fn straighten_functions() {
    let case = Case::open(MODULE, "straighten_functions");
    let meta = case.json();
    let aspects: Vec<Option<f64>> = meta["aspects"].as_array().unwrap().iter().map(|v| v.as_f64()).collect();
    let crops: Vec<[f64; 4]> = meta["crops"].as_array().unwrap().iter().map(crop4).collect();
    let mut worst = 0.0f64;
    for r in meta["records"].as_array().unwrap() {
        let (w, h, angle) = (f(&r["width"]), f(&r["height"]), f(&r["angle"]));
        let what = format!("{w}x{h} at {angle}");
        let pad = straighten::padding(angle, w as usize, h as usize);
        assert_eq!(
            [pad.0 as u64, pad.1 as u64],
            [r["padding"][0].as_u64().unwrap(), r["padding"][1].as_u64().unwrap()],
            "{what}"
        );
        for (i, aspect) in aspects.iter().enumerate() {
            assert_crop(
                straighten::largest(angle, w, h, *aspect),
                &r["largest"][i],
                &format!("largest {what}"),
                &mut worst,
            );
        }
        for (i, c) in crops.iter().enumerate() {
            assert_eq!(straighten::valid(*c, angle, w, h), r["valid"][i].as_bool().unwrap(), "valid {what} {c:?}");
            assert_crop(straighten::shrink(*c, angle, w, h), &r["shrink"][i], &format!("shrink {what}"), &mut worst);
        }
        for (i, c) in crops.iter().take(4).enumerate() {
            let e = (straighten::ratio(*c, w, h) - f(&r["ratio"][i])).abs();
            assert!(e <= 1e-12, "ratio {what}");
        }
        let previous = [[0.1, 0.1, 0.9, 0.9], [0.4, 0.4, 0.6, 0.6]];
        let mut k = 0;
        for c in &crops {
            for p in previous {
                assert_crop(
                    straighten::limit(*c, p, angle, w, h),
                    &r["limit"][k],
                    &format!("limit {what}"),
                    &mut worst,
                );
                k += 1;
            }
        }
        let refit_crops = [
            None,
            Some([0.0, 0.0, 1.0, 1.0]),
            Some([0.1, 0.1, 0.9, 0.9]),
            Some([0.2, 0.3, 0.6, 0.7]),
            Some(straighten::largest(3.0, w, h, None)),
        ];
        let mut k = 0;
        for c in refit_crops {
            for previous_angle in [0.0, 3.0] {
                for aspect in [None, Some(1.5)] {
                    let got = straighten::refit(c, previous_angle, angle, w, h, aspect);
                    match (&got, &r["refit"][k]) {
                        (None, Value::Null) => {}
                        (Some(g), e @ Value::Array(_)) => assert_crop(*g, e, &format!("refit {what}"), &mut worst),
                        (g, e) => panic!("refit {what}: got {g:?}, expected {e}"),
                    }
                    k += 1;
                }
            }
        }
    }
    for item in meta["close"].as_array().unwrap() {
        let a: Vec<f64> = item[0].as_array().unwrap().iter().map(f).collect();
        let b: Vec<f64> = item[1].as_array().unwrap().iter().map(f).collect();
        assert_eq!(straighten::close(&a, &b), item[2].as_bool().unwrap());
    }
    println!("straighten functions: max abs error {worst:.3e}");
}

#[test]
fn scan_borders() {
    for name in names("scan_") {
        if name.starts_with("scan_crop_for") {
            continue;
        }
        let case = Case::open(MODULE, &name);
        let image: Array3<f32> = case.array("input");
        let got = detect(image.view()).unwrap();
        let expected = &case.json()["crop"];
        match (&got, expected) {
            (None, Value::Null) => {}
            (Some((crop, kinds)), Value::Array(_)) => {
                assert_eq!(crop.to_vec(), crop4(expected).to_vec(), "{name}");
                let ek = case.json()["kinds"].as_object().unwrap();
                assert_eq!(kinds.len(), ek.len(), "{name}");
                for (side, kind) in kinds {
                    assert_eq!(ek[*side].as_str().unwrap(), *kind, "{name}");
                }
            }
            (g, e) => panic!("{name}: got {g:?}, expected {e}"),
        }
    }
    for name in names("scan_crop_for") {
        let case = Case::open(MODULE, &name);
        let frame: Array3<f32> = case.array("frame");
        let got = crop_for_frame(&frame, &settings(&case.json()["changes"]), None).unwrap();
        match (got, &case.json()["crop"]) {
            (None, Value::Null) => {}
            (Some(c), e @ Value::Array(_)) => assert_eq!(c.to_vec(), crop4(e).to_vec(), "{name}"),
            (g, e) => panic!("{name}: got {g:?}, expected {e}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Lens: lensfun replayed from the generator's recording of every Modifier call.

enum Call {
    Geometry { y: usize, rows: usize, coords: Option<Array3<f32>> },
    Subpixel { y: usize, rows: usize, coords: Option<Array4<f32>> },
    Color { before: Array3<f32>, after: Array3<f32>, done: bool },
}

struct Recorded {
    requests: Vec<Value>,
    calls: RefCell<VecDeque<Call>>,
    seen: RefCell<usize>,
}

impl Recorded {
    fn open(case: &Case) -> Self {
        let mut requests = Vec::new();
        let mut calls = VecDeque::new();
        for (i, call) in case.json()["calls"].as_array().unwrap().iter().enumerate() {
            let key = format!("call{i}");
            let y = call["y"].as_u64().unwrap_or(0) as usize;
            let rows = call["rows"].as_u64().unwrap_or(0) as usize;
            match call["kind"].as_str().unwrap() {
                "create" => requests.push(call["request"].clone()),
                "geometry" => {
                    calls.push_back(Call::Geometry { y, rows, coords: case.has(&key).then(|| case.array(&key)) })
                }
                "subpixel" => {
                    calls.push_back(Call::Subpixel { y, rows, coords: case.has(&key).then(|| case.array(&key)) })
                }
                "color" => calls.push_back(Call::Color {
                    before: case.array(&format!("{key}_before")),
                    after: case.array(&key),
                    done: call["done"].as_bool().unwrap(),
                }),
                _ => {}
            }
        }
        Self { requests, calls: RefCell::new(calls), seen: RefCell::new(0) }
    }

    fn check_request(&self, request: &LensRequest) {
        let mut seen = self.seen.borrow_mut();
        let e = &self.requests[(*seen).min(self.requests.len() - 1)];
        *seen += 1;
        assert_eq!(request.width as u64, e["width"].as_u64().unwrap());
        assert_eq!(request.height as u64, e["height"].as_u64().unwrap());
        for (got, key) in [
            (request.crop, "crop"),
            (request.focal, "focal"),
            (request.aperture, "aperture"),
            (request.distance, "distance"),
            (request.scale, "scale"),
        ] {
            assert_eq!(got, f(&e[key]), "request {key}");
        }
        assert_eq!(request.flags.vignetting, e["vignetting"].as_bool().unwrap());
        assert_eq!(request.flags.distortion, e["distortion"].as_bool().unwrap());
        assert_eq!(request.flags.tca, e["tca"].as_bool().unwrap());
        assert_eq!(request.flags.scale, e["flag_scale"].as_bool().unwrap());
        assert!(request.profile["xml"].as_str().unwrap().contains("<lensdatabase"));
    }
}

impl LensModel for Recorded {
    fn geometry_coordinates(
        &self,
        request: &LensRequest,
        y: usize,
        rows: usize,
    ) -> Result<Option<Array3<f32>>, String> {
        if y == 0 {
            self.check_request(request);
        }
        match self.calls.borrow_mut().pop_front() {
            Some(Call::Geometry { y: ey, rows: er, coords }) => {
                assert_eq!((y, rows), (ey, er), "geometry strip");
                Ok(coords)
            }
            _ => panic!("unexpected geometry call"),
        }
    }

    fn subpixel_coordinates(
        &self,
        request: &LensRequest,
        y: usize,
        rows: usize,
    ) -> Result<Option<Array4<f32>>, String> {
        if y == 0 {
            self.check_request(request);
        }
        match self.calls.borrow_mut().pop_front() {
            Some(Call::Subpixel { y: ey, rows: er, coords }) => {
                assert_eq!((y, rows), (ey, er), "subpixel strip");
                Ok(coords)
            }
            _ => panic!("unexpected subpixel call"),
        }
    }

    fn apply_color(&self, request: &LensRequest, pixels: &mut Array3<f32>) -> Result<bool, String> {
        self.check_request(request);
        match self.calls.borrow_mut().pop_front() {
            Some(Call::Color { before, after, done }) => {
                assert_eq!(*pixels, before, "colour input");
                pixels.assign(&after);
                Ok(done)
            }
            _ => panic!("unexpected colour call"),
        }
    }
}

#[test]
fn lens_paths() {
    let mut worst = (0.0f32, String::new());
    for name in names("lens_") {
        let case = Case::open(MODULE, &name);
        let meta = case.json();
        let a = source(&meta["input"]);
        let mut changes = meta["changes"].clone();
        changes["lensfun"] = meta["lensfun"].clone();
        let s = settings(&changes);
        let model = Recorded::open(&case);
        let budget = meta["budget"].as_u64().map_or(grainy_geometry::MAP_BUDGET, |b| b as usize);
        let result = match meta["function"].as_str().unwrap() {
            "geometry" => geometry(&a, &s, UseCrop::Yes, Some(&model)),
            "lens_geometry" => lens_geometry_with_budget(&a, &s, Some(&model), budget),
            _ => lens_shading(&a, &s, Some(&model)),
        };
        if !meta["error"].is_null() {
            error_matches(&result.expect_err(&name), &meta["error"], &name);
            continue;
        }
        let result = result.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(model.calls.borrow().is_empty(), "{name}: lens calls left over");
        if meta["unchanged"].as_bool() == Some(true) {
            assert!(matches!(result, Cow::Borrowed(_)), "{name}: expected the input back");
        }
        let expected: Array3<f32> = case.array("output");
        let e = max_abs_diff(&result, &expected).0;
        assert!(e == 0.0, "{name}: max abs error {e:e}");
        if e >= worst.0 {
            worst = (e, name);
        }
    }
    println!("lens_*: max abs error {:.3e} (tolerance 0)", worst.0);
}

#[test]
fn lens_model_is_required() {
    let case = Case::open(MODULE, "lens_geometry_chromatic");
    let mut changes = case.json()["changes"].clone();
    changes["lensfun"] = case.json()["lensfun"].clone();
    let s = settings(&changes);
    let a = synth(10, 12, 3, 0);
    assert_eq!(geometry(&a, &s, UseCrop::Yes, None).unwrap_err(), Error::LensModelMissing);
    assert_eq!(lens_shading(&a, &s, None).unwrap_err(), Error::LensModelMissing);
    // Profile stored but correction switched off: no model needed, nothing changes.
    let mut off = s.clone();
    off.set("lensfun_enabled", false);
    assert!(matches!(geometry(&a, &off, UseCrop::Yes, None).unwrap(), Cow::Borrowed(_)));
}

#[test]
fn masks_and_frames_turn_alike() {
    // The mask path (one channel) gives the same pixels as the middle channel of a frame (no lateral CA).
    let a = synth(37, 53, 3, 21);
    let mask = a.slice(ndarray::s![.., .., 1..2]).to_owned();
    let s = settings(
        &serde_json::json!({"straighten": 3.5, "distortion": 7.0, "rotation": 1, "crop": [0.1, 0.1, 0.9, 0.9]}),
    );
    let frame = geometry(&a, &s, UseCrop::Yes, None).unwrap();
    let single = geometry(&mask, &s, UseCrop::Yes, None).unwrap();
    assert_close(&single, &frame.slice(ndarray::s![.., .., 1..2]).to_owned(), 0.0, "mask vs frame");
}
