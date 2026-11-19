//! Golden tests of grainy-effects against the Python oracle (`tools/golden/gen_effects.py`).

use grainy_core::{Image, Settings};
use grainy_effects::{
    add_grain, grain_key, grain_pattern, post_crop_vignette, settings_grain, vignette_shape, GrainPattern,
};
use grainy_testkit::{assert_close, Case};
use ndarray::{s, Array2, Array3};
use serde_json::Value;

fn cases(prefix: &str) -> Vec<String> {
    let folder = grainy_testkit::root().join("golden").join("effects");
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .expect("golden/effects: run tools/golden/gen_effects.py")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no golden cases {prefix}*");
    names
}

fn settings(value: &Value) -> Settings {
    Settings::from_value(value.clone()).expect("settings object")
}

fn number(case: &Case, key: &str) -> f64 {
    case.json()[key].as_f64().unwrap_or_else(|| panic!("{key}"))
}

fn shape(value: &Value) -> (usize, usize) {
    let list = value.as_array().expect("shape");
    (list[0].as_u64().expect("h") as usize, list[1].as_u64().expect("w") as usize)
}

fn max_diff_f64(a: &Array2<f64>, e: &Array2<f64>) -> f64 {
    assert_eq!(a.shape(), e.shape());
    a.iter().zip(e.iter()).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max)
}

/// The oracle's pattern touches no blur (it is the float64 field, possibly scaled): the port is then exact.
fn unblurred(size: f64, roughness: f64, pixel_scale: f64) -> bool {
    let sigma = ((size - 25.0) / 25.0).max(0.0) * 1.5 * pixel_scale;
    sigma < 0.3 && roughness == 50.0
}

#[test]
fn grain_pattern_and_settings_grain_match_oracle() {
    let mut worst_pattern = 0f64;
    let mut worst_noise = 0f32;
    for name in cases("pattern_") {
        let case = Case::open("effects", &name);
        let meta = case.json();
        let shape = shape(&meta["shape"]);
        let (size, roughness, pixel_scale) =
            (number(&case, "size"), number(&case, "roughness"), number(&case, "pixel_scale"));
        let large = meta["large"].as_bool().unwrap_or(false);
        let exact = unblurred(size, roughness, pixel_scale);
        let sub = |a: &Array2<f64>| if large { a.slice(s![..;5, ..;7]).to_owned() } else { a.clone() };

        let pattern = grain_pattern(shape, size, roughness, pixel_scale);
        let dtype = meta["pattern_dtype"].as_str().expect("dtype");
        let (ours, expected) = match (&pattern, dtype) {
            (GrainPattern::F64(a), "float64") => (sub(a), case.array::<f64, _>("pattern")),
            (GrainPattern::F32(a), "float32") => {
                (sub(&a.mapv(f64::from)), case.array::<f32, _>("pattern").mapv(f64::from))
            }
            _ => panic!("{name}: dtype differs from the oracle's {dtype}"),
        };
        let error = max_diff_f64(&ours, &expected);
        let tolerance = if exact { 0.0 } else { 2e-4 };
        println!("{name} pattern ({dtype}): max abs error {error:.3e} (tolerance {tolerance:.1e})");
        assert!(error <= tolerance, "{name} pattern: {error}");
        worst_pattern = worst_pattern.max(error);
        if large {
            let sum: f64 = pattern.to_f64().sum();
            let expected_sum = number(&case, "pattern_sum");
            let count = (shape.0 * shape.1) as f64;
            println!(
                "{name} pattern sum {sum} (oracle {expected_sum}), mean error {:.3e}",
                (sum - expected_sum) / count
            );
            assert!((sum - expected_sum).abs() <= 1e-5 * count, "{name} pattern sum");
            if exact {
                // Every value of the full array, bit for bit.
                let bits = pattern.to_f64().iter().fold(0u64, |acc, v| acc.wrapping_add(v.to_bits()));
                assert_eq!(Some(bits.to_string().as_str()), meta["pattern_bits"].as_str(), "{name} pattern bits");
            }
        }

        for grain in [30.0, 100.0] {
            let mut s = grainy_core::defaults();
            s.set("grain", grain);
            s.set("grain_size", size);
            s.set("grain_roughness", roughness);
            let noise = settings_grain(shape, &s, pixel_scale);
            let noise = if large { noise.slice(s![..;5, ..;7]).to_owned() } else { (*noise).clone() };
            let expected: Array2<f32> = case.array(&format!("grain_{grain}"));
            let tolerance = if exact { 0.0 } else { 1e-5 };
            assert_close(&noise, &expected, tolerance, &format!("{name} settings_grain {grain}"));
            worst_noise = worst_noise.max(grainy_testkit::max_abs_diff(&noise, &expected).0);

            let key = grain_key(shape, &s, pixel_scale);
            let oracle = meta[format!("key_{grain}")].as_array().expect("key");
            assert_eq!(key.shape, self::shape(&oracle[0]));
            let fields = [key.grain, key.size, key.roughness, key.pixel_scale];
            for (ours, theirs) in fields.iter().zip(&oracle[1..]) {
                assert_eq!(Some(*ours), theirs.as_f64(), "{name} grain_key");
            }
        }
    }
    println!("grain_pattern: largest error {worst_pattern:.3e}; settings_grain: largest error {worst_noise:.3e}");
}

#[test]
fn add_grain_matches_oracle() {
    for name in cases("add_grain_") {
        let case = Case::open("effects", &name);
        let meta = case.json();
        let input: Image = grainy_testkit::input(meta["image"].as_str().expect("image"));
        let s = settings(&meta["settings"]);
        let expected: Array3<f32> = case.array("output");
        let output = add_grain(input.clone(), &s, number(&case, "pixel_scale"));
        let grain = s.f64("grain");
        if grain == 0.0 {
            assert_eq!(output, input, "{name}: grain 0 must be an exact no-op");
        }
        let exact = unblurred(s.f64("grain_size"), s.f64("grain_roughness"), number(&case, "pixel_scale"));
        assert_close(&output, &expected, if exact { 0.0 } else { 1e-5 }, &name);
    }
}

#[test]
fn vignette_matches_oracle() {
    for name in cases("vignette_") {
        let case = Case::open("effects", &name);
        let meta = case.json();
        let input: Image = if case.has("input") {
            case.array("input")
        } else {
            grainy_testkit::input(meta["image"].as_str().expect("image"))
        };
        let s = settings(&meta["settings"]);
        let (height, width, _) = input.dim();

        let shape = vignette_shape(&s, width, height);
        let ours = [shape.round, shape.p, shape.scale, shape.exponent, shape.protect, shape.aspect_x, shape.aspect_y];
        let oracle: Vec<f64> = meta["shape"].as_array().expect("shape").iter().filter_map(Value::as_f64).collect();
        for (a, e) in ours.iter().zip(&oracle) {
            assert!((a - e).abs() <= 1e-12 * e.abs().max(1.0), "{name} vignette_shape {ours:?} vs {oracle:?}");
        }

        let expected: Array3<f32> = case.array("output");
        let output = post_crop_vignette(input.clone(), &s);
        if s.f64("vignette") == 0.0 {
            assert_eq!(output, input, "{name}: amount 0 must be an exact no-op");
        }
        assert_close(&output, &expected, 1e-5, &name);
    }
}

#[test]
fn defaults_are_exact_no_ops() {
    let s = grainy_core::defaults();
    let image = grainy_testkit::input("garden");
    assert_eq!(add_grain(image.clone(), &s, 1.0), image);
    assert_eq!(post_crop_vignette(image.clone(), &s), image);
}

#[test]
fn results_do_not_depend_on_thread_count() {
    let image = Image::from_shape_fn((523, 611, 3), |(y, x, c)| ((y * 7 + x * 3 + c) % 97) as f32 / 96.0);
    let mut s = grainy_core::defaults();
    s.set("grain", 60.0);
    s.set("grain_size", 70.0);
    s.set("grain_roughness", 75.0);
    s.set("vignette", -40.0);
    s.set("vignette_highlights", 50.0);
    s.set("vignette_roundness", -20.0);
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        // A different pixel scale per run keeps the grain caches from serving the other run's result.
        pool.install(|| {
            (add_grain(image.clone(), &s, 0.9 + threads as f64 * 1e-9), post_crop_vignette(image.clone(), &s))
        })
    };
    let (grain_1, vignette_1) = run(1);
    let (grain_n, vignette_n) = run(4);
    assert_eq!(vignette_1, vignette_n);
    assert_close(&grain_1, &grain_n, 1e-6, "grain at 1 and 4 threads (pixel scales differ by 3e-9)");
}
