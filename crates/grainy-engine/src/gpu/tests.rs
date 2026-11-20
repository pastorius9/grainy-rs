use super::*;
use crate::{develop_cached, CameraSource, Crop};
use grainy_gpu::AdapterKind;
use grainy_testkit::{input, max_abs_diff, Case};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

fn device() -> (MutexGuard<'static, ()>, Option<&'static Gpu>) {
    static LOCK: Mutex<()> = Mutex::new(());
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if grainy_gpu::gpu_tests_switched_off("grainy-engine gpu::tests") {
        return (guard, None);
    }
    let gpu = GPU.get_or_init(|| Gpu::with_adapter(AdapterKind::Hardware).expect("GPU initialization"));
    if gpu.is_none() {
        eprintln!("SKIP GPU preview: no adapter (CPU-only CI)");
    }
    (guard, gpu.as_ref())
}

#[test]
fn engine_matrix_matches_cpu_worker() {
    let (_guard, Some(gpu)) = device() else { return };
    println!("ADAPTER {}", gpu.adapter_info().name);
    let index = Case::open("engine", "_index");
    let mut groups: BTreeMap<String, (usize, f32, String)> = BTreeMap::new();
    let mut failures = Vec::new();
    for (token, name) in index.json()["cases"].as_array().unwrap().iter().enumerate() {
        let name = name.as_str().unwrap();
        let case = Case::open("engine", name);
        let meta = case.json();
        let settings = meta["settings"].as_object().unwrap();
        let crop = match &meta["use_crop"] {
            Value::Bool(true) => Crop::Cropped,
            Value::Bool(false) => Crop::Uncropped,
            Value::String(s) if s == "all" => Crop::All,
            _ => panic!("crop"),
        };
        let options = Options {
            use_crop: crop,
            output: meta["output"].as_str().map_or(Output::Working, Output::Named),
            original_size: meta["original_size"].as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap())),
            ..Options::default()
        };
        let camera = case.has("camera").then(|| {
            CameraSource::new(case.array("camera"), serde_json::from_value(meta["info"].clone()).unwrap()).unwrap()
        });
        let image = if camera.is_some() {
            Image::zeros((1, 1, 3))
        } else if case.has("input") {
            case.array("input")
        } else {
            input(meta["source"].as_str().unwrap())
        };
        let source = camera.as_ref().map_or(Source::Linear(&image), Source::Camera);
        let mut cpu = DevelopmentCache::default();
        let mut cache = GpuCache::default();
        let expected = develop_cached(source, settings, &options, &mut cpu, token as u64).unwrap();
        let actual = develop_gpu_cached(source, settings, &options, gpu, &mut cache, token as u64)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(actual.dim(), expected.dim(), "{name}");
        assert!(actual.iter().all(|v| v.is_finite()), "{name}");
        let error = max_abs_diff(&actual, &expected).0;
        let group = if let Some(stage) = cache.fallback() {
            format!("fallback-{stage:?}")
        } else if name.starts_with("mask") || name.starts_with("local") {
            "masks".into()
        } else if name.starts_with("camera") {
            "RAW".into()
        } else if name.starts_with("negative") {
            "negative".into()
        } else if name.starts_with("lens")
            || name.starts_with("geometry")
            || name.starts_with("crop")
            || name.starts_with("retouch")
        {
            "geometry".into()
        } else if name.starts_with("noise")
            || name.starts_with("grain")
            || name.starts_with("vignette")
            || name.starts_with("sharpen")
            || name.starts_with("clarity")
            || name.starts_with("detail")
            || name.starts_with("dehaze")
        {
            "detail".into()
        } else {
            "tone-colour-other".into()
        };
        let g = groups.entry(group).or_default();
        g.0 += 1;
        if error > g.1 {
            g.1 = error;
            g.2 = name.into();
        }
        let effective = working_settings(settings);
        let defaults = grainy_core::defaults();
        let has_detail = DETAIL_KEYS.iter().any(|key| effective.value(key) != defaults.value(key))
            || effective.value("masks").as_array().is_some_and(|layers| !layers.is_empty());
        let tolerance = if cache.fallback().is_some() {
            0.0
        } else if has_detail {
            2e-5
        } else {
            1e-5
        };
        println!("GPU_CASE {name} error={error:.8e} tolerance={tolerance:.1e} fallback={:?}", cache.fallback());
        if error > tolerance {
            failures.push(format!("{name} {error:e} > {tolerance:e}"));
        }
    }
    for (group, (count, error, name)) in groups {
        println!("GPU_GROUP {group}: n={count} max={error:.8e} worst={name}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_monitor_conversion_preserves_the_uploaded_geometry() {
    let (_guard, Some(gpu)) = device() else { return };
    let source = input("garden");
    let mut cache = GpuCache::default();
    let opts = Options::default();
    let s = Map::new();
    develop_gpu_cached(&source, &s, &opts, gpu, &mut cache, 9).unwrap();
    let before = cache.runs();
    let profile = grainy_cms::profile("Display P3").unwrap();
    let monitor = Options { output: Output::Icc(&profile), ..opts };
    develop_gpu_cached(&source, &s, &monitor, gpu, &mut cache, 9).unwrap();
    assert_eq!(cache.runs(), before);
    develop_gpu_cached(&source, &s, &opts, gpu, &mut cache, 9).unwrap();
    assert_eq!(cache.runs(), before);
}

#[test]
fn slider_cache_reuses_the_resident_prefix() {
    let (_guard, Some(gpu)) = device() else { return };
    let image = input("garden");
    let mut cache = GpuCache::default();
    let mut settings = Map::new();
    let opts = Options::default();
    let mut render = |s: &Map<String, Value>, token| {
        let image = develop_gpu_cached(&image, s, &opts, gpu, &mut cache, token).unwrap();
        (cache.runs(), image)
    };
    let (a, first) = render(&settings, 1);
    let (b, same) = render(&settings, 1);
    assert_eq!(a, b);
    assert_eq!(first, same);
    settings.insert("exposure".into(), json!(0.3));
    let (c, _) = render(&settings, 1);
    assert_eq!((c.uploads, c.tone - a.tone, c.color - a.color, c.detail - a.detail), (1, 1, 1, 1));
    settings.insert("saturation".into(), json!(20));
    let (d, _) = render(&settings, 1);
    assert_eq!((d.tone, d.color - c.color, d.detail - c.detail), (c.tone, 1, 1));
    settings.insert("vignette".into(), json!(-30));
    let (e, _) = render(&settings, 1);
    assert_eq!((e.tone, e.color, e.detail - d.detail), (d.tone, d.color, 1));
    settings.insert("grain".into(), json!(20));
    let (f, _) = render(&settings, 1);
    assert_eq!((f.tone, f.color, f.detail, f.finish - e.finish, f.grain_uploads), (e.tone, e.color, e.detail, 1, 1));
    settings.insert("masks".into(), mask());
    let (g, _) = render(&settings, 1);
    assert_eq!((g.tone, g.color, g.detail, g.grain_uploads), (f.tone, f.color, f.detail, f.grain_uploads));
    settings.insert("masks".into(), json!([]));
    let (h, _) = render(&settings, 1);
    assert_eq!((h.tone, h.color, h.detail), (g.tone, g.color, g.detail));
    settings.insert("crop".into(), json!([0.1, 0.1, 0.9, 0.9]));
    let (i, _) = render(&settings, 1);
    assert_eq!((i.uploads, i.tone, i.color, i.detail), (2, 1, 1, 1));
    let (j, _) = render(&settings, 2);
    assert_eq!(j.uploads, 3);
}

fn mask() -> Value {
    json!([{"components":[{"type":"linear","points":[[0.2,0.2],[0.8,0.8]]}],"adjustments":{"exposure":-0.5}}])
}

#[test]
fn masks_see_unclipped_grain_and_detail() {
    let (_guard, Some(gpu)) = device() else { return };
    let image = input("wide");
    let settings = json!({"exposure":2.0,"grain":80,"grain_size":50,"masks":mask()});
    let mut cache = GpuCache::default();
    let opts = Options::default();
    let actual = develop_gpu_cached(&image, settings.as_object().unwrap(), &opts, gpu, &mut cache, 1).unwrap();
    let expected = crate::develop(&image, settings.as_object().unwrap(), &opts).unwrap();
    let error = max_abs_diff(&actual, &expected).0;
    println!("unclipped mask/grain max={error:e}, tolerance=2e-5");
    assert!(error <= 2e-5);
    let s = working_settings(settings.as_object().unwrap());
    let toned = crate::tone(&image, &s).unwrap();
    let colored = grainy_colorgrade::develop_color(&toned, &s).unwrap();
    let mut detailed = grainy_detail::develop_detail(&colored, &s, 1.0).unwrap();
    grainy_color::clip01_in_place(&mut detailed);
    let mut wrong =
        stages::local(detailed, (image.dim().0, image.dim().1), None, &s, Crop::Cropped, 1.0, &opts.externals).unwrap();
    grainy_color::clip01_in_place(&mut wrong);
    assert!(max_abs_diff(&expected, &wrong).0 > 1e-3, "fixture must detect clipping before masks");
    let hits = cache.host_cache().hits();
    let runs = cache.runs();
    let mut changed = settings.as_object().unwrap().clone();
    changed.get_mut("masks").unwrap()[0]["adjustments"]["exposure"] = json!(-0.25);
    let next = develop_gpu_cached(&image, &changed, &opts, gpu, &mut cache, 1).unwrap();
    let expected = crate::develop(&image, &changed, &opts).unwrap();
    assert!(max_abs_diff(&next, &expected).0 <= 2e-5);
    assert_eq!(cache.runs(), runs, "mask-only edit dispatches no GPU stages");
    assert_eq!(cache.host_cache().hits(), hits + 1, "mask-only edit reuses the host detail readback");
}

#[test]
fn cached_fallbacks_are_exact_and_do_not_contaminate_later_gpu_frames() {
    let (_guard, Some(gpu)) = device() else { return };
    let image = input("garden");
    let mut cache = GpuCache::default();
    for edit in [
        json!({"exposure":0.3}),
        json!({"exposure":0.3,"photo_filter_enabled":true,"photo_filter":"cooling80"}),
        json!({"exposure":0.3,"photo_filter_enabled":true,"photo_filter":"cooling80","vignette":-30}),
        json!({"exposure":0.3,"film_look":"kodak-portra-400","film_strength":100}),
        json!({"exposure":0.3,"hdr":true}),
        json!({"exposure":0.3}),
        json!({"negative":true}),
        json!({"negative":false,"masks":mask()}),
    ] {
        let options = Options::default();
        let settings = edit.as_object().unwrap();
        let actual = develop_gpu_cached(&image, settings, &options, gpu, &mut cache, 1).unwrap();
        let expected = crate::develop(&image, settings, &options).unwrap();
        let error = max_abs_diff(&actual, &expected).0;
        let tolerance = if cache.fallback().is_some() { 0.0 } else { 2e-5 };
        println!("fallback transition {edit}: max={error:e} tolerance={tolerance:e}");
        assert!(error <= tolerance);
    }
    for output in [Output::Named("Display P3"), Output::Working, Output::Named("sRGB")] {
        let options = Options { output, ..Options::default() };
        let actual = develop_gpu_cached(&image, &Map::new(), &options, gpu, &mut cache, 1).unwrap();
        let expected = crate::develop(&image, &Map::new(), &options).unwrap();
        let error = max_abs_diff(&actual, &expected).0;
        assert!(error <= if cache.fallback().is_some() { 0.0 } else { 2e-5 });
    }
    let mut tiny = GpuCache::new(DevelopmentCache::new(0));
    let a = develop_gpu_cached(&image, &Map::new(), &Options::default(), gpu, &mut tiny, 1).unwrap();
    assert_eq!(tiny.host_cache().bytes(), 0);
    assert!(max_abs_diff(&a, &crate::develop(&image, &Map::new(), &Options::default()).unwrap()).0 <= 2e-5);
}

#[test]
#[ignore = "performance measurement on generated 2048-pixel and 24 MP frames"]
fn timings() {
    let (_guard, Some(gpu)) = device() else { return };
    println!("ADAPTER {} (initialization excluded)", gpu.adapter_info().name);
    for (w, h) in [(2048, 1365), (6000, 4000)] {
        let image =
            Image::from_shape_fn((h, w, 3), |(y, x, c)| 0.01 + ((x * 13 + y * 7 + c * 43) % 1024) as f32 / 1100.0);
        let mut cpu = DevelopmentCache::growing(256 << 20, 4.0, Some(1536 << 20));
        let mut cache = GpuCache::default();
        let opts = Options { original_size: Some((6000., 4000.)), ..Options::default() };
        let mut settings = Map::new();
        let mut changed_mask = mask();
        changed_mask[0]["adjustments"]["exposure"] = json!(-0.25);
        for (label, key, value) in [
            ("first", "exposure", json!(0.0)),
            ("exposure", "exposure", json!(0.4)),
            ("late", "vignette", json!(-25)),
            ("mask", "masks", mask()),
            ("mask-edit", "masks", changed_mask),
        ] {
            settings.insert(key.into(), value);
            let start = Instant::now();
            let a = develop_cached(&image, &settings, &opts, &mut cpu, 1).unwrap();
            let cpu_ms = start.elapsed().as_secs_f64() * 1000.;
            let start = Instant::now();
            let b = develop_gpu_cached(&image, &settings, &opts, gpu, &mut cache, 1).unwrap();
            let gpu_ms = start.elapsed().as_secs_f64() * 1000.;
            let error = max_abs_diff(&a, &b).0;
            assert!(error <= 2e-5, "{label}: {error:e}");
            println!("GPU_TIMING {w}x{h} {label}: CPU={cpu_ms:.2}ms GPU={gpu_ms:.2}ms max={error:e}");
        }
    }
}
