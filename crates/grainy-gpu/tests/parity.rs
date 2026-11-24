use grainy_core::{Image, Settings};
use grainy_gpu::{AdapterKind, Gpu, Resident};
use grainy_testkit::{assert_close, input, max_abs_diff, Case};
use ndarray::{s, Array3};
use serde_json::{json, Value};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn index() -> &'static Value {
    static INDEX: OnceLock<Value> = OnceLock::new();
    INDEX.get_or_init(|| Case::open("gpu", "index").json().clone())
}

#[test]
fn fixture_bundle_stays_under_three_megabytes() {
    fn size(path: &std::path::Path) -> u64 {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    size(&entry.path())
                } else {
                    entry.metadata().unwrap().len()
                }
            })
            .sum()
    }
    let bytes = size(&grainy_testkit::root().join("golden/gpu"));
    assert!(bytes <= 3_000_000, "GPU fixtures use {bytes} bytes (limit 3 MB)");
    assert_eq!(index()["defaults"], json!(grainy_core::defaults().0));
}

fn gpu(kind: AdapterKind) -> Option<Gpu> {
    if grainy_gpu::gpu_tests_switched_off(&format!("grainy-gpu parity ({kind:?})")) {
        return None;
    }
    let gpu = Gpu::with_adapter(kind).expect("adapter exists but GPU initialization failed");
    match &gpu {
        Some(g) => println!("ADAPTER {kind:?}: {:?}", g.adapter_info()),
        None => eprintln!("SKIP {kind:?}: no compute adapter available; CPU-only CI"),
    }
    gpu
}
fn settings(v: &Value) -> Settings {
    let mut s = Settings::from_value(index()["defaults"].clone()).unwrap();
    for (key, value) in index()["settings"][v.as_u64().unwrap() as usize].as_object().unwrap() {
        s.set(key, value.clone());
    }
    s
}
fn changes(v: Value) -> Settings {
    let mut s = grainy_core::defaults();
    for (k, v) in v.as_object().unwrap() {
        s.set(k, v.clone());
    }
    s
}
fn cases(kind: &str) -> &[Value] {
    index()["cases"][kind].as_array().unwrap()
}
fn source(case: &Value) -> Image {
    let name = case["source"].as_str().unwrap();
    let mut a = match name {
        "neg" => input("noise") - 0.2f32,
        "tiny" => Array3::from_shape_vec((1, 1, 3), vec![0.3, 0.2, 0.4]).unwrap(),
        "row" => input("colours").slice(s![..1, ..9, ..]).to_owned(),
        "column" => input("colours").slice(s![..9, ..1, ..]).to_owned(),
        "large" => {
            let garden = input("garden");
            Array3::from_shape_fn((683, 1024, 3), |(y, x, c)| garden[[y % 160, x % 240, c]])
        }
        name if name.starts_with("custom:") => Case::open("gpu", "inputs").array(&name[7..]),
        name => input(name),
    };
    if let Some(crop) = case["crop"].as_array() {
        let c: Vec<usize> = crop.iter().map(|v| v.as_u64().unwrap() as usize).collect();
        a = a.slice(s![c[0]..c[1], c[2]..c[3], ..]).to_owned();
    }
    if case["encoded"] == true {
        a = grainy_colorgrade::convert::to_srgb(&a);
    }
    a
}
// The GPU accepts pixels after geometry and before tone. Use the same CPU stages as
// grainy_engine::develop(Output::Working), without normalization or output colour conversion.
fn cpu_chain(a: &Image, s: &Settings, scale: f64) -> Image {
    let a = grainy_tone::develop_tone(a, s, None).unwrap();
    let a = grainy_colorgrade::develop_color(&a, s).unwrap();
    grainy_detail::develop_detail(&a, s, scale).unwrap().mapv(|v| v.clamp(0.0, 1.0))
}
fn cpu_cases(g: &Gpu, label: &str) {
    assert_eq!(index()["defaults"], json!(grainy_core::defaults().0));
    for kind in ["tone", "color", "chain", "grain", "detail_chain"] {
        let mut worst = 0f32;
        for case in cases(kind) {
            let name = case["name"].as_str().unwrap();
            let a = source(case);
            let s = settings(&case["settings"]);
            let scale = case["pixel_scale"].as_f64().unwrap();
            let out = match kind {
                "tone" => g.tone(&a, &s).unwrap(),
                "color" => g.color(&a, &s).unwrap(),
                "chain" | "detail_chain" => g.develop(&a, &s, scale).unwrap(),
                _ => {
                    let grain = grainy_effects::settings_grain((a.dim().0, a.dim().1), &s, scale);
                    Some(g.readback(&g.grain_clip(&g.upload(&a).unwrap(), Some(&grain)).unwrap()).unwrap())
                }
            };
            let promoted = match kind {
                "tone" => grainy_film::active(&s) || grainy_tone::mapping::active(&s),
                "color" => !s.bool("hdr") && s.bool("photo_filter_enabled"),
                "chain" | "detail_chain" => {
                    !s.bool("hdr")
                        && (grainy_film::active(&s)
                            || grainy_tone::mapping::active(&s)
                            || s.bool("photo_filter_enabled"))
                }
                _ => false,
            };
            let expect_supported = case["supported"].as_bool().unwrap_or(true) || promoted;
            assert_eq!(out.is_some(), expect_supported, "{label} {name}: support");
            if let Some(out) = out {
                let film_name = grainy_film::active(&s).then(|| s.str("film_look").to_owned());
                let film_space = grainy_film::WorkingSpace::from_setting(s.str("working_space"));
                let expected = match kind {
                    "tone" => {
                        if let Some(ref stock_name) = film_name {
                            let stock_name = stock_name.clone();
                            let render = move |img: ndarray::ArrayView3<'_, f32>| {
                                grainy_film::render(&img.to_owned(), &stock_name, film_space).unwrap()
                            };
                            grainy_tone::develop_tone(&a, &s, Some(&render)).unwrap()
                        } else {
                            grainy_tone::develop_tone(&a, &s, None).unwrap()
                        }
                    }
                    "color" => grainy_colorgrade::develop_color(&a, &s).unwrap(),
                    "grain" => grainy_effects::add_grain(a, &s, scale).mapv(|v| v.clamp(0.0, 1.0)),
                    _ => {
                        let toned = if let Some(ref stock_name) = film_name {
                            let stock_name = stock_name.clone();
                            let render = move |img: ndarray::ArrayView3<'_, f32>| {
                                grainy_film::render(&img.to_owned(), &stock_name, film_space).unwrap()
                            };
                            grainy_tone::develop_tone(&a, &s, Some(&render)).unwrap()
                        } else {
                            grainy_tone::develop_tone(&a, &s, None).unwrap()
                        };
                        let colored = grainy_colorgrade::develop_color(&toned, &s).unwrap();
                        grainy_detail::develop_detail(&colored, &s, scale).unwrap().mapv(|v| v.clamp(0.0, 1.0))
                    }
                };
                let tol = match kind {
                    "grain" => 0.0,
                    "color" if name.contains("default") => 0.0,
                    _ if film_name.is_some() => 3e-5,
                    "detail_chain" => 2e-5,
                    _ => 1e-5,
                };
                assert_close(&out, &expected, tol, &format!("{label}/{name}"));
                worst = worst.max(max_abs_diff(&out, &expected).0);
            }
        }
        println!("SUMMARY {label} CPU cases {kind}: {worst:.9e}");
    }
    for case in cases("fallback") {
        let name = case["name"].as_str().unwrap();
        let s = settings(&case["settings"]);
        let a = input("colours");
        // Phase 5A promoted film look, DRT tone mapping, and photo filters to full GPU execution.
        let expect_tone =
            case["tone"].as_bool().unwrap() || grainy_film::active(&s) || grainy_tone::mapping::active(&s);
        let expect_color = case["color"].as_bool().unwrap() || (!s.bool("hdr") && s.bool("photo_filter_enabled"));
        assert_eq!(g.tone(&a, &s).unwrap().is_some(), expect_tone, "{name} tone");
        assert_eq!(g.color(&a, &s).unwrap().is_some(), expect_color, "{name} color");
    }
}

#[test]
fn hardware_cpu_cases() {
    let _guard = test_lock();
    if let Some(g) = gpu(AdapterKind::Hardware) {
        cpu_cases(&g, "hardware");
    }
}

#[test]
fn software_cpu_cases() {
    let _guard = test_lock();
    if let Some(g) = gpu(AdapterKind::Software) {
        cpu_cases(&g, "software");
    }
}

fn cpu_matrices(g: &Gpu, label: &str) {
    let garden = input("garden");
    let large = Array3::from_shape_fn((683, 1024, 3), |(y, x, c)| garden[[y % 160, x % 240, c]]);
    let mut inputs: Vec<_> = ["ramp", "wide", "colours", "patches", "noise", "edges", "garden"]
        .iter()
        .map(|n| (n.to_string(), input(n)))
        .collect();
    inputs.push(("garden_1024x683".into(), large));
    for kind in ["tone", "color"] {
        let mut worst = 0f32;
        for (i, v) in index()["matrices"][kind].as_array().unwrap().iter().enumerate() {
            let s = settings(v);
            for (name, a) in &inputs {
                let (actual, expected) = if kind == "tone" {
                    (g.tone(a, &s).unwrap().unwrap(), grainy_tone::develop_tone(a, &s, None).unwrap())
                } else {
                    (g.color(a, &s).unwrap().unwrap(), grainy_colorgrade::develop_color(a, &s).unwrap())
                };
                assert_close(&actual, &expected, 1e-5, &format!("{label}/matrix/{kind}/{i}/{name}"));
                worst = worst.max(max_abs_diff(&actual, &expected).0);
            }
        }
        println!("SUMMARY {label} CPU matrix {kind}: {worst:.9e}");
    }
}

#[test]
fn hardware_cpu_matrices() {
    let _guard = test_lock();
    if let Some(g) = gpu(AdapterKind::Hardware) {
        cpu_matrices(&g, "hardware");
    }
}

#[test]
fn software_cpu_matrices() {
    let _guard = test_lock();
    if let Some(g) = gpu(AdapterKind::Software) {
        cpu_matrices(&g, "software");
    }
}

#[test]
fn resident_retry_after_color_encoding_failure_recomputes_tone() {
    let _guard = test_lock();
    let Some(g) = gpu(AdapterKind::Hardware) else {
        return;
    };
    let source = input("garden");
    let mut cache = Resident::new(&g, &source).unwrap();
    let mut settings = changes(json!({"exposure": 1.0}));
    settings.set("saturation", 1e200);
    let before = cache.runs();
    assert!(g.develop_resident(&mut cache, &settings, 1.0).is_err());
    assert_eq!(cache.runs(), before);
    settings.set("saturation", 0);
    let frame = g.develop_resident(&mut cache, &settings, 1.0).unwrap().unwrap();
    assert_close(&g.readback(&frame).unwrap(), &cpu_chain(&source, &settings, 1.0), 2e-5, "retried batch");
}

#[test]
fn resident_invalidation_and_chain() {
    let _guard = test_lock();
    let Some(g) = gpu(AdapterKind::Hardware) else {
        return;
    };
    let a = input("garden");
    let mut r = Resident::new(&g, &a).unwrap();
    let mut s = changes(json!({"shadows":40.0,"highlights":-60.0}));
    for step in 0..11 {
        let before = r.runs();
        match step {
            1 => s.set("saturation", 35.0),
            2 => s.set("grain", 75.0),
            3 => s.set("shadows", 65.0),
            4 => s.set("exposure", 0.7),
            5 => s.set("unrelated_ui_key", 1),
            6 => r.replace(&g, &a.slice(s![..;-1,..;-1,..]).to_owned()).unwrap(),
            8 => s.set("vignette", 60.0),
            9 => s.set("grain_size", 75.0),
            10 => s.set("grain", 0.0),
            _ => {}
        }
        let frame = g.develop_resident(&mut r, &s, 0.7).unwrap().unwrap();
        let got = g.readback(&frame).unwrap();
        let src = if step >= 6 { a.slice(s![..;-1,..;-1,..]).to_owned() } else { a.clone() };
        let expected = cpu_chain(&src, &s, 0.7);
        assert_close(&got, &expected, 2e-5, &format!("resident chain {step}"));
        let after = r.runs();
        if step != 6 {
            assert_eq!(before.uploads, after.uploads);
        }
        match step {
            1 => {
                assert_eq!(before.tone, after.tone);
                assert_eq!(before.color + 1, after.color);
            }
            2 | 9 | 10 => {
                assert_eq!(before.detail, after.detail);
                assert_eq!(before.finish + 1, after.finish);
            }
            3 => {
                assert_eq!(before.analyses, after.analyses);
                assert_eq!(before.tone + 1, after.tone);
            }
            4 => assert_eq!(before.analyses + 1, after.analyses),
            5 | 7 => assert_eq!(before, after),
            8 => {
                assert_eq!(before.color, after.color);
                assert_eq!(before.detail + 1, after.detail);
            }
            _ => {}
        }
        if [3, 4, 5, 7, 8, 10].contains(&step) {
            assert_eq!(before.grain_uploads, after.grain_uploads);
        }
    }
}

#[test]
fn frame_roundtrip_validation_and_tiny_images() {
    let _guard = test_lock();
    let Some(g) = gpu(AdapterKind::Hardware) else {
        return;
    };
    for name in ["ramp", "wide", "colours", "patches", "noise", "edges", "garden"] {
        let a = input(name).slice(s![..;-1,..;-1,..]).to_owned();
        let f = g.upload(&a).unwrap();
        assert_eq!(f.shape(), a.dim());
        assert_eq!(g.readback(&f).unwrap(), a);
        let s = grainy_core::defaults();
        let out = g.tone_frame(&f, &s, &a).unwrap().unwrap();
        assert_close(&g.readback(&out).unwrap(), &grainy_tone::develop_tone(&a, &s, None).unwrap(), 2e-6, "tone frame");
        let out = g.color_frame(&f, &s).unwrap().unwrap();
        assert_eq!(g.readback(&out).unwrap(), a.mapv(|v| v.clamp(0.0, 1.0)));
    }
    for (h, w) in [(1, 1), (1, 9), (9, 1), (17, 19)] {
        let a = Array3::from_shape_fn((h, w, 3), |(y, x, c)| ((x + 2 * y + c) % 17) as f32 / 16.0);
        let s = changes(json!({"exposure":0.5,"lens_vignette":-60.0,"shadows":40.0,"highlights":-30.0}));
        assert_close(
            &g.tone(&a, &s).unwrap().unwrap(),
            &grainy_tone::develop_tone(&a, &s, None).unwrap(),
            1e-5,
            "tiny tone",
        );
    }
    assert!(g.upload(&Image::zeros((0, 2, 3))).is_err());
    assert!(g.upload(&Image::zeros((2, 2, 4))).is_err());
    assert!(g.upload(&Image::from_elem((2, 2, 3), f32::NAN)).is_err());
    assert!(g.develop(&input("ramp"), &grainy_core::defaults(), f64::NAN).is_err());
    let frame = g.upload(&input("ramp")).unwrap();
    assert!(g.grain_clip(&frame, Some(&ndarray::Array2::zeros((2, 2)))).is_err());
    if let Some(other) = gpu(AdapterKind::Hardware) {
        assert!(other.readback(&frame).is_err());
    }
}

fn detail_cases(g: &Gpu, label: &str) {
    let mut worst = 0f32;
    let mut failures = Vec::new();
    for case in cases("detail") {
        let name = case["name"].as_str().unwrap();
        let s = settings(&case["settings"]);
        assert!(!s.bool("grain")); // GPU detail excludes grain; CPU develop_detail includes it.
        let scale = case["pixel_scale"].as_f64().unwrap();
        let a = source(case);
        let actual = g.detail(&a, &s, scale).unwrap().unwrap();
        let expected = grainy_detail::develop_detail(&a, &s, scale).unwrap();
        let error = max_abs_diff(&actual, &expected).0;
        println!("{label}/{name}: max abs error {error:.9e} (tolerance 2e-5)");
        if error > 2e-5 {
            failures.push((name, error));
        }
        worst = worst.max(error);
        if name.ends_with("default") {
            assert_eq!(actual, expected);
        }
    }
    println!("SUMMARY {label} CPU detail: {worst:.9e}; failures {failures:?}");
    assert!(failures.is_empty());
}

#[test]
fn hardware_cpu_detail() {
    let _guard = test_lock();
    if let Some(g) = gpu(AdapterKind::Hardware) {
        detail_cases(&g, "hardware");
    }
}

#[test]
fn software_cpu_detail() {
    let _guard = test_lock();
    if let Some(g) = gpu(AdapterKind::Software) {
        detail_cases(&g, "software");
    }
}

#[test]
fn hardware_and_software_agree_on_complete_chain() {
    let _guard = test_lock();
    let Some(hardware) = gpu(AdapterKind::Hardware) else {
        return;
    };
    let Some(software) = gpu(AdapterKind::Software) else {
        return;
    };
    let s = settings(&cases("detail_chain")[0]["settings"]);
    let mut worst = 0f32;
    for name in ["ramp", "wide", "colours", "patches", "noise", "edges", "garden"] {
        let a = input(name);
        let h = hardware.develop(&a, &s, 0.7).unwrap().unwrap();
        let w = software.develop(&a, &s, 0.7).unwrap().unwrap();
        assert_close(&h, &w, 2e-5, &format!("hardware/software complete {name}"));
        worst = worst.max(max_abs_diff(&h, &w).0);
    }
    println!("SUMMARY direct hardware/software chain: {worst:.9e}");
}

#[test]
fn detail_and_parameter_fallbacks_are_explicit() {
    let _guard = test_lock();
    let Some(g) = gpu(AdapterKind::Hardware) else {
        return;
    };
    let a = input("colours");
    let s = changes(json!({"hdr":true}));
    assert!(g.detail(&a, &s, 1.0).unwrap().is_none());
    assert!(!g.detail_tools_on_gpu(&s, 1.0));
    let s = changes(json!({"noise_luma":50.0}));
    assert!(!g.detail_tools_on_gpu(&s, 1.0));
    assert!(g.detail_tools_on_gpu(&s, 0.1));
    assert!(g.detail_tools_on_gpu(&grainy_core::defaults(), 1.0));
    assert!(g.detail(&a, &s, 0.0).is_err());
    assert!(g.detail(&a, &s, 33.0).unwrap().is_none());
    let s = changes(json!({"noise_luma":50.0,"wavelet_profiles":[[1.0]]}));
    assert!(g.detail(&a, &s, 1.0).is_err());
    let s = changes(json!({"point_colors":vec![json!({"hue":1.0});257]}));
    assert!(g.color(&a, &s).unwrap().is_none());
    let curve: Vec<_> = (0..2049).map(|i| json!([i as f64 / 2048.0, 0.5])).collect();
    let s = changes(json!({"rgb_curves":[curve.clone(),curve.clone(),curve]}));
    assert!(g.color(&a, &s).unwrap().is_none());
}
