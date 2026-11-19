//! Golden tests against `tools/golden/gen_sky.py` (luma/sky_mask.py run by the oracle).
//!
//! Each case is compared at every intermediate the oracle exposes; a mismatch is reported with the largest
//! absolute difference on the stored subsample. The tolerances are stated per item below.

use std::collections::BTreeMap;

use grainy_core::{Image, Plane};
use grainy_testkit::Case;
use ndarray::{s, Array2, Array3, ArrayD, Axis};
use serde_json::Value;

use super::*;

const MODULE: &str = "sky";

fn bits(values: impl Iterator<Item = f32>) -> String {
    values.fold(0u64, |acc, v| acc.wrapping_add(f64::from(v).to_bits())).to_string()
}

fn bool_hash<'a>(values: impl Iterator<Item = &'a bool>) -> Value {
    let (mut count, mut sum) = (0u64, 0u64);
    for (i, &b) in values.enumerate() {
        if b {
            count += 1;
            sum += i as u64;
        }
    }
    serde_json::json!([count, sum])
}

/// The largest difference between `values` (H×W×C flattened in a shape) subsampled by the case's stride and the
/// stored subsample.
fn sub_diff(case: &Case, name: &str, a: &ArrayD<f32>) -> f32 {
    let k = case.json()[format!("{name}_stride")].as_u64().unwrap_or(1) as usize;
    let expected: ArrayD<f32> = case.array(&format!("{name}_sub"));
    let sub = a.slice_each_axis(|ax| {
        if ax.axis.index() < 2 {
            ndarray::Slice::new(0, None, k as isize)
        } else {
            ndarray::Slice::new(0, None, 1)
        }
    });
    if sub.shape() != expected.shape() {
        return f32::INFINITY;
    }
    sub.iter().zip(expected.iter()).map(|(a, b)| if a == b { 0.0 } else { (a - b).abs() }).fold(0.0, f32::max)
}

/// Comparison log: item -> (cases, exact cases, largest subsample difference).
#[derive(Default)]
struct Log {
    items: BTreeMap<String, (usize, usize, f32)>,
    failures: Vec<String>,
    current: String,
}

impl Log {
    fn floats(&mut self, case: &Case, item: &str, name: &str, a: ArrayD<f32>) {
        let meta = case.json();
        let shape: Vec<usize> = meta[format!("{name}_shape")]
            .as_array()
            .map(|v| v.iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect())
            .unwrap_or_default();
        let exact =
            shape == a.shape() && meta[format!("{name}_bits")].as_str() == Some(bits(a.iter().copied()).as_str());
        let diff = if exact { 0.0 } else { sub_diff(case, name, &a) };
        let case_name = self.current.clone();
        self.record(item, exact, diff, || {
            format!("{case_name} {name}: shape {:?} vs {:?}, sub diff {diff:e}", a.shape(), shape)
        });
    }

    fn equal(&mut self, item: &str, ok: bool, what: impl FnOnce() -> String) {
        self.record(item, ok, if ok { 0.0 } else { f32::INFINITY }, what);
    }

    fn record(&mut self, item: &str, exact: bool, diff: f32, what: impl FnOnce() -> String) {
        let e = self.items.entry(item.to_string()).or_default();
        e.0 += 1;
        if exact {
            e.1 += 1;
        } else {
            e.2 = e.2.max(diff);
            if diff > tolerance(item) {
                self.failures.push(what());
            }
        }
    }
}

/// Everything must be bit-exact except what derives from `cv2.magnitude`, which the oracle's OpenCV hands to
/// Intel IPP (1–2 ulp from the correctly computed value, see `cv::deriv`): the magnitude itself, the texture (its
/// box mean) and the scores that take the texture through the `calm` ramp. Their largest differences are
/// measured on the stored subsamples. The edges, candidates, detections and masks that follow are exact.
fn tolerance(item: &str) -> f32 {
    match item {
        "features.score" | "features.blue" => 1e-6,
        "features.texture" | "texture (box of IPP magnitude)" | "magnitude (IPP in the oracle)" => 1e-7,
        _ => 0.0,
    }
}

fn input_raw(scene: &str) -> (Array3<u8>, f32) {
    let case = Case::open(MODULE, &format!("input_{scene}"));
    (case.array("raw"), case.json()["gain"].as_f64().unwrap_or(1.0) as f32)
}

fn down2(raw: &Array3<u8>) -> Array3<u8> {
    let (h, w, c) = raw.dim();
    Array3::from_shape_fn((h / 2, w / 2, c), |(y, x, ch)| {
        let v = |dy: usize, dx: usize| u32::from(raw[[2 * y + dy, 2 * x + dx, ch]]);
        ((v(0, 0) + v(0, 1) + v(1, 0) + v(1, 1) + 2) / 4) as u8
    })
}

fn up2(raw: &Array3<u8>) -> Array3<u8> {
    let (h, w, c) = raw.dim();
    Array3::from_shape_fn((2 * h, 2 * w, c), |(y, x, ch)| {
        let (i, j) = (y / 2, x / 2);
        let v = |a: usize, b: usize| u32::from(raw[[a.min(h - 1), b.min(w - 1), ch]]);
        (match (y % 2, x % 2) {
            (0, 0) => v(i, j),
            (0, _) => (v(i, j) + v(i, j + 1)).div_ceil(2),
            (_, 0) => (v(i, j) + v(i + 1, j)).div_ceil(2),
            _ => (v(i, j) + v(i, j + 1) + v(i + 1, j) + v(i + 1, j + 1) + 2) / 4,
        }) as u8
    })
}

fn to_float(raw: &Array3<u8>, gain: f32) -> Image {
    raw.mapv(|v| {
        let a = f32::from(v) / 255.0;
        if gain != 1.0 {
            a * gain
        } else {
            a
        }
    })
}

fn source_of(meta: &Value) -> Image {
    let scene = meta["scene"].as_str().unwrap_or_default();
    match scene {
        "garden" => grainy_testkit::input("garden"),
        "black" => Image::zeros((64, 96, 3)),
        _ => {
            let (raw, gain) = input_raw(scene);
            let raw = match meta["size"].as_str() {
                Some("half") => down2(&raw),
                Some("double") => up2(&raw),
                _ => raw,
            };
            to_float(&raw, gain)
        }
    }
}

/// The generator's numpy closures, by name.
type Closure = Box<dyn Fn(&Image) -> Result<Image>>;

fn transform_of(name: &str) -> Option<Closure> {
    let rot = |k: i64| -> Closure { Box::new(move |m: &Image| Ok(rot90(m, k))) };
    match name {
        "none" => None,
        "identity" => Some(Box::new(|m: &Image| Ok(m.clone()))),
        "crop" => Some(Box::new(|m: &Image| {
            let (h, w, _) = m.dim();
            Ok(m.slice(s![h / 10..h * 9 / 10, w * 2 / 10.., ..]).to_owned())
        })),
        "flip" => Some(Box::new(|m: &Image| Ok(m.slice(s![.., ..;-1, ..]).to_owned()))),
        "flipv" => Some(Box::new(|m: &Image| Ok(m.slice(s![..;-1, .., ..]).to_owned()))),
        "rot1" => Some(rot(1)),
        "rot2" => Some(rot(2)),
        "rot3" => Some(rot(3)),
        "croprot1" => Some(Box::new(|m: &Image| {
            let (h, w, _) = m.dim();
            Ok(rot90(&m.slice(s![..h * 3 / 4, w / 8.., ..]).to_owned(), 1))
        })),
        "pad" => Some(Box::new(|m: &Image| {
            let (h, w, c) = m.dim();
            let mut out = Image::zeros((h + h / 6, w, c));
            out.slice_mut(s![h / 6.., .., ..]).assign(m);
            Ok(out)
        })),
        "tiny" => Some(Box::new(|m: &Image| Ok(Image::zeros(m.dim())))),
        _ => panic!("unknown transform {name}"),
    }
}

fn dyn2(a: &Plane) -> ArrayD<f32> {
    a.clone().into_dyn()
}

fn sens_tag(s: f64) -> String {
    if s.fract() == 0.0 {
        format!("{}", s as i64)
    } else {
        format!("{s}")
    }
}

fn check_case(name: &str, log: &mut Log) {
    let case = Case::open(MODULE, name);
    log.current = name.to_string();
    let meta = case.json().clone();
    let source = source_of(&meta);
    let shape: Vec<usize> = meta["shape"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
    assert_eq!(&source.shape()[..2], &shape[..], "{name}: input shape");
    let transforms: Vec<String> =
        meta["transforms"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let components: Vec<Value> = meta["components"].as_array().unwrap().clone();
    let sensitivities: Vec<f64> =
        meta["sensitivities"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    for (ti, tname) in transforms.iter().enumerate() {
        let boxed = transform_of(tname);
        let transform = boxed.as_deref();
        let p = format!("t_{tname}");
        let at = |k: &str| format!("{p}_{k}");
        let v = view((shape[0], shape[1]), transform).unwrap();
        let expected: Vec<usize> =
            meta[at("view")].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as usize).collect();
        let got = vec![v.rect[0], v.rect[1], v.rect[2], v.rect[3], v.turns];
        log.equal("view", got == expected, || format!("{name} {p} view {got:?} vs {expected:?}"));
        let copy = detection_copy(&source, transform).unwrap();
        log.floats(&case, "detection_copy (small)", &at("small"), copy.small.clone().into_dyn());
        let small = &copy.small;
        let too_small = meta.get(at("tooSmall")).is_some();
        if !too_small && ti == 0 {
            for &sens in &sensitivities {
                let tag = format!("f{}", sens_tag(sens));
                let f = features(small, sens).unwrap();
                log.floats(&case, "features.score", &at(&format!("{tag}_score")), dyn2(&f.score));
                log.floats(&case, "features.blue", &at(&format!("{tag}_blue")), dyn2(&f.blue));
                log.floats(&case, "features.L", &at(&format!("{tag}_L")), dyn2(&f.l));
                log.floats(&case, "features.texture", &at(&format!("{tag}_texture")), dyn2(&f.texture));
                let e = bool_hash(f.edges.iter());
                log.equal("features.edges", e == meta[at(&format!("{tag}_edges"))], || {
                    format!("{name} {p} {tag} edges {e} vs {}", meta[at(&format!("{tag}_edges"))])
                });
                if sens == 50.0 {
                    let cand = candidates_of(&f);
                    let ch = bool_hash(cand.mapv(|c| c > 0).iter());
                    log.equal("candidates", ch == meta[at("candidates")], || {
                        format!("{name} {p} candidates {ch} vs {}", meta[at("candidates")])
                    });
                    let line = horizon(&f.l, &cand).unwrap();
                    let got = line.map_or(Value::Null, |l| serde_json::json!(l));
                    log.equal("horizon", got == meta[at("horizon")], || {
                        format!("{name} {p} horizon {got} vs {}", meta[at("horizon")])
                    });
                }
                let sky = detect(small, sens).unwrap();
                let dh = bool_hash(sky.iter());
                let key = at(&format!("detect{}", sens_tag(sens)));
                log.equal("detect", dh == meta[&key], || format!("{name} {key} {dh} vs {}", meta[&key]));
            }
        } else if !too_small {
            let sky = detect(small, 50.0).unwrap();
            let dh = bool_hash(sky.iter());
            log.equal("detect", dh == meta[at("detect50")], || {
                format!("{name} {p} detect50 {dh} vs {}", meta[at("detect50")])
            });
        }
        for (i, component) in components.iter().enumerate() {
            if i > 0 && ti > 0 && i != 2 && i != 6 {
                continue;
            }
            let q = format!("{p}_c{i}");
            let atq = |k: &str| format!("{q}_{k}");
            if !too_small {
                let sens = number(component, "sensitivity", DEFAULT_SENSITIVITY).unwrap();
                let sky = detect(small, sens).unwrap();
                let any = sky.iter().any(|&b| b);
                log.equal("detect.any", Value::Bool(any) == meta[atq("any")], || format!("{name} {q} any {any}"));
                if any && meta[atq("any")] == Value::Bool(true) {
                    let g = guided(small, &sky, number(component, "feather", DEFAULT_FEATHER).unwrap()).unwrap();
                    log.equal(
                        "radius/eps",
                        meta[atq("r")].as_u64() == Some(g.r as u64)
                            && meta[atq("eps")].as_f64() == Some(f64::from(g.eps)),
                        || format!("{name} {q} r/eps"),
                    );
                    log.floats(&case, "resize INTER_AREA (guide)", &atq("guide"), g.guide.clone().into_dyn());
                    log.floats(&case, "resize INTER_AREA (p)", &atq("p"), dyn2(&g.p));
                    log.floats(&case, "reach (dilate + box)", &atq("reach"), dyn2(&g.reach));
                    log.floats(&case, "_guided_coefficients", &atq("coef"), g.coefficients.clone().into_dyn());
                }
            }
            let mask = sky_mask(&source, component, transform).unwrap();
            log.floats(&case, "sky_mask", &atq("mask"), dyn2(&mask));
        }
    }
}

fn case_names() -> Vec<String> {
    let dir = grainy_testkit::root().join("golden").join(MODULE);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}; run tools/golden/gen_sky.py", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with("input_") && !n.starts_with("cv_") && !n.starts_with("_"))
        .collect();
    names.sort();
    names
}

fn report(log: &Log) {
    println!("{:<34} {:>6} {:>6} {:>12} {:>10}", "item", "cases", "exact", "max |diff|", "tolerance");
    for (item, (n, exact, diff)) in &log.items {
        println!("{item:<34} {n:>6} {exact:>6} {diff:>12.3e} {:>10.0e}", tolerance(item));
    }
    for f in log.failures.iter().take(60) {
        println!("  mismatch: {f}");
    }
}

#[test]
fn golden_cases() {
    use rayon::prelude::*;
    let logs: Vec<Log> = case_names()
        .par_iter()
        .map(|name| {
            let mut log = Log::default();
            check_case(name, &mut log);
            log
        })
        .collect();
    let mut log = Log::default();
    for part in logs {
        for (item, (n, exact, diff)) in part.items {
            let e = log.items.entry(item).or_default();
            e.0 += n;
            e.1 += exact;
            e.2 = e.2.max(diff);
        }
        log.failures.extend(part.failures);
    }
    report(&log);
    assert!(log.failures.is_empty(), "{} mismatches", log.failures.len());
}

// ------------------------------------------------------------------------------------------------ primitives

#[test]
fn golden_primitives() {
    let dir = grainy_testkit::root().join("golden").join(MODULE);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("cv_"))
        .collect();
    names.sort();
    let mut log = Log::default();
    for name in names {
        let case = Case::open(MODULE, &name);
        log.current = name.clone();
        let meta = case.json().clone();
        let small: Image = match meta["scene"].as_str() {
            Some("odd") => to_float(&case.array::<u8, _>("raw"), 1.0),
            _ => detection_copy(&source_of(&meta), None).unwrap().small,
        };
        let (h, w, _) = small.dim();
        let white = meta["white"].as_f64().unwrap();
        let gain = (1.0 / white) as f32;
        let n = small.mapv(|v| np_max(np_min(v * gain, 1.0), 0.0));
        log.floats(&case, "n", "n", n.clone().into_dyn());
        let nm = mat(&n);
        log.floats(&case, "cvtColor RGB2HSV", "hsv", image(cv::rgb_to_hsv(&nm)).unwrap().into_dyn());
        let l: Plane = n.map_axis(Axis(2), |p| (p[0] * LUMA[0] + p[1] * LUMA[1]) + p[2] * LUMA[2]);
        log.floats(&case, "L = n @ LUMA", "L", dyn2(&l));
        let sigma = meta["sigma"].as_f64().unwrap();
        let scale = meta["scale"].as_f64().unwrap();
        let bl = cv::gaussian_blur(&mat1(&l), sigma);
        log.floats(&case, "GaussianBlur 1ch", "blurL", plane(bl.clone()).unwrap().into_dyn());
        let bn = cv::gaussian_blur(&nm, sigma);
        log.floats(&case, "GaussianBlur 3ch", "blurN", image(bn.clone()).unwrap().into_dyn());
        for (tag, a) in [("L", &bl), ("N", &bn)] {
            let gx = cv::sobel(a, true, scale);
            let gy = cv::sobel(a, false, scale);
            let shape = |m: &Mat| -> ArrayD<f32> {
                if m.cn == 1 {
                    plane(m.clone()).unwrap().into_dyn()
                } else {
                    image(m.clone()).unwrap().into_dyn()
                }
            };
            log.floats(&case, &format!("Sobel dx {}ch", a.cn), &format!("gx{tag}"), shape(&gx));
            log.floats(&case, &format!("Sobel dy {}ch", a.cn), &format!("gy{tag}"), shape(&gy));
            let m = Mat { data: cv::magnitude(&gx.data, &gy.data), ..gx.clone() };
            log.floats(&case, "magnitude (IPP in the oracle)", &format!("mag{tag}"), shape(&m));
        }
        let unit = h.max(w) as f64 / DETECT_SIZE as f64;
        let gradient = {
            let gx = cv::sobel(&bl, true, scale);
            let gy = cv::sobel(&bl, false, scale);
            cv::magnitude(&gx.data, &gy.data)
        };
        let texture = box_plane(&gradient, h, w, round(2.0 * unit).max(1) as usize);
        log.floats(
            &case,
            "texture (box of IPP magnitude)",
            "texture",
            Plane::from_shape_vec((h, w), texture).unwrap().into_dyn(),
        );
        let smooth = cv::gaussian_blur(&mat1(&l), unit.max(0.5));
        log.floats(&case, "GaussianBlur 1ch", "smooth", plane(smooth).unwrap().into_dyn());
        let (dw, dh) = ((w / 2).max(1), (h / 2).max(1));
        log.floats(
            &case,
            "resize INTER_AREA 3ch",
            "area_half",
            image(cv::resize_area(&mat(&small), dw, dh)).unwrap().into_dyn(),
        );
        let g1 = small.index_axis(Axis(2), 1).to_owned();
        log.floats(
            &case,
            "resize INTER_AREA 1ch",
            "area_half1",
            plane(cv::resize_area(&mat1(&g1), dw, dh)).unwrap().into_dyn(),
        );
        for k in [1usize, 2, 4, 9] {
            log.floats(&case, "boxFilter 3ch", &format!("box3_{k}"), box_filter(&small, k).unwrap().into_dyn());
            let r0 = small.index_axis(Axis(2), 0).to_owned();
            log.floats(
                &case,
                "boxFilter 1ch",
                &format!("box1_{k}"),
                plane(cv::box_filter(&mat1(&r0), 2 * k + 1)).unwrap().into_dyn(),
            );
        }
        let _ = Array2::<f32>::zeros((1, 1));
    }
    report(&log);
    assert!(log.failures.is_empty(), "{} mismatches", log.failures.len());
}

// ------------------------------------------------------------------------------------------------ behaviour

fn clear_scene() -> Image {
    let (raw, gain) = input_raw("clear");
    to_float(&raw, gain)
}

#[test]
fn bad_inputs_are_errors() {
    let image = clear_scene();
    assert!(sky_mask(&image, &serde_json::json!({"sensitivity": "high"}), None).is_err());
    assert!(sky_mask(&Image::zeros((0, 0, 3)), &serde_json::json!({}), None).is_err());
    assert!(features(&Image::zeros((20, 20, 4)), 50.0).is_err());
    assert!(horizon(&Plane::zeros((20, 20)), &Array2::zeros((10, 20))).is_err());
    let failing = |_: &Image| -> Result<Image> { Err(Error::Transform("no geometry".into())) };
    assert!(matches!(sky_mask(&image, &serde_json::json!({}), Some(&failing)), Err(Error::Transform(_))));
}

#[test]
fn small_frames_and_extra_channels() {
    // Below 16 detection pixels the mask is empty; a fourth channel is ignored as the oracle's [..., :3].
    assert!(sky_mask(&Image::ones((12, 40, 3)), &serde_json::json!({}), None).unwrap().iter().all(|&v| v == 0.0));
    let image = clear_scene();
    let mut four = Image::zeros((image.dim().0, image.dim().1, 4));
    four.slice_mut(s![.., .., ..3]).assign(&image);
    let a = sky_mask(&image, &serde_json::json!({}), None).unwrap();
    let b = sky_mask(&four, &serde_json::json!({}), None).unwrap();
    assert_eq!(a, b);
    assert!(a.iter().any(|&v| v > 0.9));
}

#[test]
fn independent_of_thread_count() {
    let image = clear_scene();
    let component = serde_json::json!({"sensitivity": 80, "feather": 60});
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| sky_mask(&image, &component, None).unwrap())
    };
    assert_eq!(run(1), run(4));
}
