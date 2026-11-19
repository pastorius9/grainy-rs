//! Array goldens of `tools/golden/gen_library_arrays.py`: the recovery descriptor and `compare`, the 5x5 Gaussian
//! blur it relies on (against OpenCV) and `automatic_white_balance` (numpy float32 maths).

use grainy_core::Settings;
use grainy_library::folder_recovery::{compare, compare_stats, descriptor, Descriptor, SmallRgb};
use grainy_library::npx::{gaussian_blur5, gaussian_kernel5};
use grainy_library::workflow_data::automatic_white_balance;
use grainy_testkit::{max_abs_diff, Case};
use ndarray::{Array3, Array4, Array5};

#[test]
fn descriptor_scales_the_small_picture() {
    let case = Case::open("library", "descriptor");
    let pixels: Array3<u8> = case.array("pixels");
    let expected: Array3<f32> = case.array("value");
    let d = descriptor(&SmallRgb { pixels, width: 64, height: 64 });
    assert_eq!(d.value, expected, "value must be exactly pixels / 255 in float32");
    assert_eq!(d.ratio, case.json()["ratio"].as_f64().unwrap());
}

#[test]
fn compare_decides_like_the_oracle() {
    let case = Case::open("library", "compare");
    let pairs: Array5<u8> = case.array("pairs");
    let ratios = case.json()["ratios"].as_array().unwrap();
    let expected = case.json()["expected"].as_array().unwrap();
    let kinds = case.json()["kinds"].as_array().unwrap();
    let stats = case.json()["stats"].as_array().unwrap();
    let mut worst = 0.0_f64;
    for i in 0..pairs.shape()[0] {
        let make = |k: usize, ratio: f64| Descriptor {
            value: pairs.index_axis(ndarray::Axis(0), i).index_axis(ndarray::Axis(0), k).mapv(|v| f32::from(v) / 255.0),
            ratio,
        };
        let (a, b) = (make(0, ratios[i][0].as_f64().unwrap()), make(1, ratios[i][1].as_f64().unwrap()));
        assert_eq!(compare(&a, &b), expected[i].as_bool().unwrap(), "pair {i} ({})", kinds[i]);
        let got = compare_stats(&a, &b);
        for (k, v) in got.iter().enumerate() {
            let want = stats[i][k].as_f64().unwrap();
            worst = worst.max((f64::from(*v) - want).abs());
            assert!((f64::from(*v) - want).abs() < 1e-6, "pair {i} ({}) statistic {k}: {v} vs {want}", kinds[i]);
        }
    }
    eprintln!("compare statistics: largest difference {worst:e}");
}

#[test]
fn gaussian_blur_matches_opencv() {
    let case = Case::open("library", "blur");
    let input: Array3<f32> = case.array("input");
    let expected: Array3<f32> = case.array("output");
    let kernel = gaussian_kernel5(0.8);
    for (k, v) in case.json()["kernel"].as_array().unwrap().iter().enumerate() {
        assert_eq!(kernel[k], v.as_f64().unwrap() as f32, "kernel tap {k}");
    }
    let (worst, _) = max_abs_diff(&gaussian_blur5(&input, 0.8), &expected);
    eprintln!("gaussian blur: largest difference {worst:e}");
    assert!(worst < 1e-6, "blur differs from cv2 by {worst}");
}

#[test]
fn automatic_white_balance_matches_the_oracle() {
    let mut worst = 0.0_f64;
    for n in 0..10 {
        let case = Case::open("library", &format!("awb_{n}"));
        let meta = case.json();
        let gains: Vec<f32> = meta["gains"].as_array().unwrap().iter().map(|g| g.as_f64().unwrap() as f32).collect();
        let mut source = grainy_testkit::input(meta["input"].as_str().unwrap());
        for mut row in source.rows_mut() {
            for (c, v) in row.iter_mut().enumerate() {
                *v *= gains[c % 3];
            }
        }
        let settings = Settings(meta["settings"].as_object().unwrap().clone());
        let prepare = |a: &Array3<f32>, _: &Settings| Ok(a.clone());
        let got = automatic_white_balance(&source, &settings, &prepare);
        if let Some(error) = meta.get("error") {
            let e = got.expect_err("the oracle raised");
            assert_eq!(e.python_type(), error["type"].as_str().unwrap());
            assert_eq!(e.to_string(), error["message"].as_str().unwrap());
            continue;
        }
        let r = got.unwrap();
        let want = &meta["result"];
        for key in ["temperature", "tint"] {
            let (g, w) = (r.f64(key), want[key].as_f64().unwrap());
            worst = worst.max((g - w).abs());
            assert!((g - w).abs() < 2e-4, "case {n} {key}: {g} vs {w}");
        }
        assert_eq!(r.bool("kelvin_enabled"), want["kelvin_enabled"].as_bool().unwrap());
        // (the golden stores the settings dict with sorted keys, so only the key set is comparable)
        let mut keys: Vec<&str> = r.0.keys().map(String::as_str).collect();
        let mut want_keys: Vec<&str> = want["keys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
        keys.sort_unstable();
        want_keys.sort_unstable();
        assert_eq!(keys, want_keys, "keys of the settings");
    }
    eprintln!("automatic white balance: largest difference {worst:e}");
}

#[test]
fn seeds_are_available() {
    let seeds: Array4<u8> = Case::open("library", "_seeds").array("seeds");
    assert_eq!(seeds.shape(), &[8, 64, 64, 3]);
}
