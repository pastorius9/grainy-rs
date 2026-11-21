use grainy_core::{Image, Settings};
use grainy_raw::{
    dcp::{self, Profile},
    raw_defaults as defaults,
    rawcolor::{self, CameraInfo, CameraSource},
    DecodeMode, RawFile,
};
use grainy_testkit::{assert_close, Case as GoldenCase};
use ndarray::Array3;
use serde_json::Value;
use std::{fs, path::PathBuf};

// RAW-local indirection keeps the shared testkit and the other crates unchanged.
struct Case(GoldenCase);
impl Case {
    fn open(module: &str, name: &str) -> Self {
        Self(GoldenCase::open(module, name))
    }
    fn json(&self) -> &Value {
        self.0.json()
    }
    fn array<T: ndarray_npy::ReadableElement, D: ndarray::Dimension>(&self, name: &str) -> ndarray::Array<T, D> {
        let key = self.json()["arrays"][name].as_str().unwrap();
        self.0.array(&format!("../_arrays/{key}"))
    }
    fn file(&self) -> PathBuf {
        folder(self.json()["file"].as_str().unwrap())
    }
    fn compare_output(&self, actual: &Image, name: &str) {
        if let Some(sample) = self.json().get("sample") {
            assert_eq!(json(actual.shape()), sample["shape"], "{name}: full shape");
            let stride = sample["stride"].as_u64().unwrap() as isize;
            let sampled = actual.slice(ndarray::s![..;stride, ..;stride, ..]).to_owned();
            assert_close(&sampled, &self.array("output"), 1e-5, name);
            let mut sum = [0f64; 3];
            let mut min = [f64::INFINITY; 3];
            let mut max = [f64::NEG_INFINITY; 3];
            for ((_, _, channel), &value) in actual.indexed_iter() {
                assert!(value.is_finite(), "{name}: non-finite full-frame pixel");
                let value = f64::from(value);
                sum[channel] += value;
                min[channel] = min[channel].min(value);
                max[channel] = max[channel].max(value);
            }
            let mean = sum.map(|v| v / (actual.len() / 3) as f64);
            let mut error = 0f64;
            for (key, values) in [("mean", mean), ("min", min), ("max", max)] {
                error = error.max(close_json(&json(values), &sample[key], 1e-5, &format!("{name}/{key}")));
            }
            println!("{name}/statistics: max abs error {error:.3e} (tolerance 1.0e-5)");
        } else {
            assert_close(actual, &self.array("output"), 1e-5, name);
        }
    }
}
fn folder(name: &str) -> PathBuf {
    grainy_testkit::root().join("golden/raw").join(name)
}
fn cases(prefix: &str) -> Vec<String> {
    let mut v: Vec<_> = fs::read_dir(folder(""))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    v.sort();
    v
}
fn from<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}
fn settings(v: &Value) -> Settings {
    if let Some(key) = v["shared_settings"].as_str() {
        static DEFAULTS: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| Case::open("raw", "_settings/defaults").json().clone());
        let mut value = DEFAULTS.clone();
        let delta = Case::open("raw", &format!("_settings/{key}"));
        value.as_object_mut().unwrap().extend(delta.json().as_object().unwrap().clone());
        Settings::from_value(value).unwrap()
    } else {
        Settings::from_value(v.clone()).unwrap()
    }
}
fn close_json(a: &Value, b: &Value, tol: f64, name: &str) -> f64 {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let d = (a.as_f64().unwrap() - b.as_f64().unwrap()).abs();
            assert!(d <= tol, "{name}: {a} != {b}, error {d} > {tol}");
            d
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{name}");
            a.iter().zip(b).map(|(a, b)| close_json(a, b, tol, name)).fold(0., f64::max)
        }
        (Value::Object(a), Value::Object(b)) => b
            .iter()
            .map(|(k, b)| close_json(a.get(k).unwrap_or(&Value::Null), b, tol, &format!("{name}.{k}")))
            .fold(0., f64::max),
        _ => {
            assert_eq!(a, b, "{name}");
            0.
        }
    }
}
fn json<T: serde::Serialize>(a: T) -> Value {
    serde_json::to_value(a).unwrap()
}
fn source(name: &str) -> CameraSource {
    let c = Case::open("raw", &format!("decode_{name}"));
    CameraSource::new(c.array("source_0"), from(&c.json()["info_0"])).unwrap()
}
#[test]
fn decoder_is_bit_exact() {
    assert_eq!(grainy_raw::libraw_version(), "0.22.1-Release");
    for name in cases("decode_") {
        let c = Case::open("raw", &name);
        let path = c.file();
        assert_eq!(defaults::linear_raw(&path), c.json()["linear_raw"].as_bool().unwrap());
        if c.json().get("error").is_some() {
            assert!(matches!(RawFile::open(&path), Err(grainy_raw::Error::UnsupportedRaw)));
            continue;
        }
        let r = RawFile::open(&path).unwrap();
        close_json(&json(r.metadata()), &c.json()["metadata"], 0., &name);
        for half in [false, true] {
            for (label, mode) in
                [("camera", DecodeMode::Camera), ("srgb", DecodeMode::Srgb), ("prophoto", DecodeMode::ProPhoto)]
            {
                let expected: Array3<u16> = c.array(&format!("{label}_{}", u8::from(half)));
                let actual = RawFile::open(&path).unwrap().postprocess(mode, half).unwrap();
                assert_eq!(actual.dim(), expected.dim());
                let error = actual.iter().zip(&expected).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
                println!("{name}/{label}/{half}: max error {error} (tolerance 0)");
                assert_eq!(error, 0, "{name}/{label}/{half}");
            }
        }
        let mut repeat = RawFile::open(&path).unwrap();
        let a = repeat.postprocess(DecodeMode::Camera, false).unwrap();
        let b = repeat.postprocess(DecodeMode::Srgb, true).unwrap();
        assert_eq!(a, c.array::<u16, _>("camera_0"));
        assert_eq!(b, c.array::<u16, _>("srgb_1"));
    }
}
#[test]
fn decode_camera_source() {
    for name in cases("decode_") {
        let c = Case::open("raw", &name);
        if c.json().get("error").is_some() {
            continue;
        }
        let path = c.file();
        for half in [false, true] {
            let info: CameraInfo = from(&c.json()[format!("info_{}", u8::from(half))]);
            let mut metadata = Value::Object(info.metadata.clone());
            let mut raw = RawFile::open(&path).unwrap();
            let actual =
                rawcolor::decode(&mut raw, &mut metadata, if half { Some(16) } else { None }, Some(&path)).unwrap();
            assert_close(
                actual.pixels(),
                &c.array(&format!("source_{}", u8::from(half))),
                1e-5,
                &format!("{name}/source/{half}"),
            );
            close_json(&json(actual.raw_info()), &json(&info), 1e-10, &name);
            close_json(
                &json([
                    metadata["raw_as_shot_kelvin"].as_f64().unwrap(),
                    metadata["raw_as_shot_tint"].as_f64().unwrap(),
                ]),
                &c.json()[format!("as_shot_{}", u8::from(half))],
                1e-10,
                &name,
            );
            close_json(
                &json(rawcolor::default_profile(actual.raw_info()).unwrap()),
                &json(info.base_profile.unwrap()),
                1e-10,
                &name,
            );
        }
    }
}
#[test]
fn profiles_and_camera_matrices() {
    let mut errors = std::collections::BTreeMap::<String, f64>::new();
    let mut measure = |function: &str, a: Value, b: &Value, tolerance: f64| {
        let error = close_json(&a, b, tolerance, function);
        let previous = errors.entry(function.into()).or_default();
        *previous = previous.max(error);
        error
    };
    for name in cases("profile_") {
        let c = Case::open("raw", &name);
        let path = c.file();
        let data = fs::read(&path).unwrap();
        let p = dcp::parse(&data).unwrap();
        let mut maximum = measure("parse", json(&p), &c.json()["profile"], 1e-10);
        assert_eq!(dcp::load_profile(&path).unwrap(), c.json()["record"]);
        assert_eq!(dcp::compiled(&c.json()["record"]).unwrap(), p);
        let tags = dcp::read_tags(&data, None).unwrap();
        let filtered = dcp::read_tags(&data, Some(&[50721])).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[&50721], tags[&50721]);
        for (k, v) in c.json()["tags"].as_object().unwrap() {
            let t = &tags[&k.parse::<u16>().unwrap()];
            let a = match t {
                dcp::Tag::Bytes(b) => json!({"bytes":b}),
                _ => json(t),
            };
            maximum = maximum.max(measure("read_tags", a, v, 1e-10));
        }
        assert_eq!(dcp::parse_tags(&tags, data.starts_with(b"II")).unwrap(), p);
        for sweep in c.json()["sweeps"].as_array().unwrap() {
            let t = sweep["temperature"].as_f64().unwrap();
            let white = from(&sweep["white"]);
            let w = dcp::weight(&p, t, Some(white)).unwrap();
            let info: CameraInfo = from(&sweep["info"]);
            let neutral = from(&sweep["neutral"]);
            maximum = maximum.max(measure("weight", json(w), &sweep["weight"], 1e-10));
            for field in ["cm", "fm"] {
                maximum = maximum.max(measure(
                    "interpolate",
                    json(dcp::interpolate(&p, field, w).unwrap()),
                    &sweep[field],
                    1e-10,
                ));
            }
            maximum = maximum.max(measure(
                "interpolate_table",
                json(dcp::interpolate_table(&p, w).unwrap()),
                &sweep["hs"],
                1e-5,
            ));
            maximum = maximum.max(measure(
                "calibration_matrix",
                json(dcp::calibration_matrix(info.calibration.as_deref().unwrap(), w).unwrap()),
                &sweep["calibration"],
                1e-10,
            ));
            maximum = maximum.max(measure(
                "individual_matrix",
                json(rawcolor::individual_matrix(&p, &info, w).unwrap()),
                &sweep["individual"],
                1e-10,
            ));
            maximum = maximum.max(measure(
                "neutral_white",
                json(rawcolor::neutral_white(&p, neutral, &info).unwrap()),
                &sweep["neutral_white"],
                1e-5,
            ));
            for (field, white) in [("camera", Some(white)), ("camera_inferred", None)] {
                maximum = maximum.max(measure(
                    "camera_to_xyz",
                    json(
                        dcp::camera_to_xyz(
                            &p,
                            neutral,
                            t,
                            info.calibration.as_deref(),
                            info.analog,
                            info.calibration_signature.as_deref().unwrap_or(""),
                            white,
                        )
                        .unwrap(),
                    ),
                    &sweep[field],
                    1e-5,
                ));
            }
        }
        for m in c.json()["matching"].as_array().unwrap() {
            assert_eq!(dcp::camera_matches(&p, &m["metadata"]), m["result"].as_bool().unwrap());
        }
        println!("{name}: parse/matrix maximum {maximum:.3e} (tolerance 1e-5)");
    }
    for (function, error) in errors {
        println!("METRIC {function}: {error:.12e}");
    }
}
#[test]
fn malformed_profiles_are_rejected() {
    for name in cases("invalid_") {
        let c = Case::open("raw", &name);
        assert!(dcp::parse(&fs::read(c.file()).unwrap()).is_err(), "{name}");
    }
    assert!(dcp::compiled(&json!({"data":"!!!!","sha256":""})).is_err());
    let c = Case::open("raw", "profile_test-camera");
    let mut r = c.json()["record"].clone();
    r["sha256"] = "bad".into();
    assert!(dcp::compiled(&r).is_err());
}
#[test]
fn temperature_and_adaptation() {
    let c = Case::open("raw", "temperature");
    let mut maximum = [0f64; 3];
    for v in c.json()["values"].as_array().unwrap() {
        let xyz = rawcolor::temperature_xyz(v["kelvin"].as_f64().unwrap(), v["tint"].as_f64().unwrap()).unwrap();
        maximum[0] = maximum[0].max(close_json(&json(xyz), &v["xyz"], 1e-10, "temperature_xyz"));
        maximum[1] = maximum[1].max(close_json(
            &json(rawcolor::xyz_temperature(xyz).unwrap()),
            &v["reverse"],
            1e-8,
            "xyz_temperature",
        ));
        maximum[2] = maximum[2].max(close_json(
            &json(dcp::adapt_white(xyz, dcp::D50).unwrap()),
            &v["adapt"],
            1e-10,
            "adapt_white",
        ));
    }
    for (function, error) in ["temperature_xyz", "xyz_temperature", "adapt_white"].into_iter().zip(maximum) {
        println!("METRIC {function}: {error:.12e}");
    }
}
#[test]
fn hue_sat_and_look_tables() {
    for name in cases("table_") {
        let c = Case::open("raw", &name);
        let p: Profile =
            from(&Case::open("raw", &format!("profile_{}", c.json()["profile"].as_str().unwrap())).json()["profile"]);
        let table = if c.json()["kind"] == "hs1" { p.hs1.unwrap() } else { p.look.unwrap() };
        let a = dcp::table_map(&c.array("input"), &table, c.json()["encoding"].as_i64().unwrap() as i32).unwrap();
        assert_close(&a, &c.array("output"), 1e-5, &name);
    }
}
#[test]
fn tone_curves() {
    for name in cases("tone_") {
        let c = Case::open("raw", &name);
        let curve: Vec<[f64; 2]> = from(&c.json()["curve"]);
        let a = dcp::tone_map(&c.array("input"), &curve).unwrap();
        assert_close(&a, &c.array("output"), 1e-5, &name);
    }
}
#[test]
fn development() {
    let mut white_error = 0f64;
    for name in cases("develop_") {
        let c = Case::open("raw", &name);
        let source = source(c.json()["source"].as_str().unwrap());
        let s = settings(&c.json()["settings"]);
        let p = if s.bool("dcp_profile") {
            dcp::compiled(s.value("dcp_profile")).unwrap()
        } else {
            rawcolor::default_profile(source.raw_info()).unwrap()
        };
        white_error = white_error.max(close_json(
            &json(rawcolor::resolve_white(&p, source.raw_info(), &s).unwrap()),
            &c.json()["white"],
            1e-5,
            &name,
        ));
        let actual = rawcolor::develop_camera(&source, &s).unwrap();
        c.compare_output(&actual, &name);
    }
    println!("METRIC resolve_white: {white_error:.12e}");
}

#[test]
fn development_covering_design_is_complete() {
    let mut expected = std::collections::BTreeSet::new();
    for name in cases("decode_") {
        let c = Case::open("raw", &name);
        if c.json().get("error").is_some() {
            continue;
        }
        let design = &c.json()["development_design"];
        let levels: [usize; 3] = from(&design["levels"]);
        let rows: Vec<[usize; 3]> = from(&design["rows"]);
        let unique: std::collections::BTreeSet<_> = rows.iter().copied().collect();
        assert_eq!(unique.len(), rows.len(), "{name}: repeated design row");
        for a in 0..3 {
            for b in a + 1..3 {
                let pairs: std::collections::BTreeSet<_> = rows.iter().map(|row| (row[a], row[b])).collect();
                for av in 0..levels[a] {
                    for bv in 0..levels[b] {
                        assert!(pairs.contains(&(av, bv)), "{name}: missing pair {a}={av}, {b}={bv}");
                    }
                }
            }
        }
        let source = name.strip_prefix("decode_").unwrap();
        for [mode, variant, space] in rows {
            assert!(mode < levels[0] && variant < levels[1] && space < levels[2]);
            let (profile, toggle) = if variant == 0 { (0, 0) } else { (1, variant - 1) };
            let space = ["sRGB", "ProPhoto"][space];
            let name = format!("develop_{source}_{mode}_{profile}_{space}_{toggle}");
            let case = Case::open("raw", &name);
            let s = settings(&case.json()["settings"]);
            assert_eq!(case.json()["source"], source);
            assert_eq!(s.str("working_space"), space);
            assert_eq!(s.str("raw_mode"), ["as_shot", "daylight", "sample", "custom", "custom", "custom"][mode]);
            assert_eq!(s.bool("dcp_profile"), profile == 1);
            for (index, key) in ["dcp_huesat", "dcp_look", "dcp_tone", "dcp_exposure"].into_iter().enumerate() {
                assert_eq!(s.bool(key), profile == 0 || (toggle != index + 1 && toggle != 5), "{name}/{key}");
            }
            if mode >= 3 {
                assert_eq!(s.f32("raw_kelvin"), [2000., 50000., 4300.][mode - 3]);
                assert_eq!(s.f32("raw_tint"), [-150., 150., 17.][mode - 3]);
            }
            if (source == "bayer_0_16" && (mode == 0 || profile == 0))
                || (mode == 0 && profile == 0 && (space == "sRGB" || source == "triple_asset"))
            {
                assert!(case.json().get("sample").is_none(), "{name}: representative must stay full-frame");
            }
            expected.insert(name);
        }
    }
    assert_eq!(expected, cases("develop_").into_iter().collect());
}

#[test]
fn camera_colour_branches() {
    for name in cases("camera_math_") {
        let c = Case::open("raw", &name);
        let source = CameraSource::new(c.array("input"), from(&c.json()["info"])).unwrap();
        c.compare_output(&rawcolor::develop_camera(&source, &settings(&c.json()["settings"])).unwrap(), &name);
    }
    let c = Case::open("raw", "default_profile");
    let info: CameraInfo = from(&c.json()["info"]);
    let profile = rawcolor::default_profile(&info).unwrap();
    close_json(&json(&profile), &c.json()["profile"], 1e-10, "default_profile");
    close_json(
        &json(rawcolor::neutral_white(&profile, info.neutral, &info).unwrap()),
        &c.json()["white"],
        1e-8,
        "neutral_white/default",
    );
    for v in c.json()["calibration"].as_array().unwrap() {
        close_json(
            &json(dcp::calibration_matrix(&from::<Vec<[[f64; 3]; 3]>>(&v["input"]), from(&v["weight"])).unwrap()),
            &v["output"],
            1e-10,
            "calibration_matrix",
        );
    }
    let c = Case::open("raw", "noops");
    let a: Image = c.array("input");
    for (i, v) in c.json()["values"].as_array().unwrap().iter().enumerate() {
        let s = settings(&v["settings"]);
        assert_eq!(rawcolor::enabled(&s), v["enabled"]);
        assert_eq!(rawcolor::linear_frame_plain(&a, &s).unwrap(), c.array::<f32, _>(&i.to_string()));
    }
}
#[test]
fn linear_frames() {
    for name in cases("linear_frame_") {
        let c = Case::open("raw", &name);
        let source = source(c.json()["source"].as_str().unwrap());
        let s = settings(&c.json()["settings"]);
        assert_close(&rawcolor::linear_frame(&source, &s).unwrap(), &c.array("output"), 1e-5, &name);
        if s.bool("negative") {
            assert_eq!(rawcolor::develop_camera(&source, &s).unwrap(), *source.pixels());
        }
    }
    for name in cases("plain_") {
        let c = Case::open("raw", &name);
        let a: Image = c.array("input");
        assert_eq!(rawcolor::develop_plain(&a).unwrap(), c.array::<f32, _>("develop"));
        assert_close(
            &rawcolor::linear_frame_plain(&a, &settings(&c.json()["settings"])).unwrap(),
            &c.array("linear"),
            1e-5,
            &name,
        );
    }
}
#[test]
fn false_colour_median() {
    for name in cases("false_color_") {
        let c = Case::open("raw", &name);
        assert_close(&rawcolor::reduce_false_color(&c.array("input")).unwrap(), &c.array("output"), 0., &name);
    }
}
#[test]
fn import_rules() {
    let c = Case::open("raw", "defaults");
    let v = c.json();
    let anchors = defaults::iso_anchors(&from::<Vec<Value>>(&v["photos"])).unwrap();
    close_json(&json(&anchors), &v["anchors"], 0., "iso_anchors");
    for r in v["rule_inputs"].as_array().unwrap() {
        let rule = defaults::make_rule(
            &r["info"],
            &settings(&r["settings"]),
            r["scope"].as_str().unwrap(),
            Some(&from::<Vec<String>>(&r["groups"])),
            &anchors,
            r["name"].as_str().unwrap(),
        )
        .unwrap();
        close_json(&rule, &r["rule"], 0., "make_rule");
    }
    let rules: Vec<Value> = from(&v["rules"]);
    for r in v["values"].as_array().unwrap() {
        let info = &r["info"];
        assert_eq!(defaults::camera_key(info), r["key"]);
        assert_eq!(defaults::serial(info), r["serial"]);
        assert_eq!(defaults::is_raw(info, None), r["is_raw"]);
        assert_eq!(json(defaults::iso_value(info)), r["iso"]);
        assert_eq!(json(defaults::builtin_luma_nr(defaults::iso_value(info))), r["luma"]);
        assert_eq!(defaults::builtin_iso_detail(defaults::iso_value(info)), r["detail"]);
        for (field, rules, base) in [
            ("empty", &[][..], None),
            ("resolved", rules.as_slice(), None),
            (
                "base",
                rules.as_slice(),
                Some({
                    let mut s = grainy_core::settings::defaults();
                    s.set("exposure", -1.);
                    s
                }),
            ),
        ] {
            let (s, n) = defaults::resolve(info, rules, None, base.as_ref()).unwrap();
            close_json(&json!([s.0, n]), &r[field], 0., field);
        }
    }
    for r in v["retargets"].as_array().unwrap() {
        let s = settings(&r["settings"]);
        assert_eq!(rawcolor::enabled(&s), r["enabled"]);
        close_json(&json(rawcolor::retarget(&s, &r["metadata"]).unwrap().0), &r["output"], 0., "retarget");
    }
    for r in v["keys"].as_array().unwrap() {
        assert_eq!(defaults::key(r["input"].as_str().unwrap()), r["output"]);
    }
}

#[test]
fn unicode_camera_keys_match_python() {
    let c = Case::open("raw", "unicode");
    assert_eq!(c.json()["version"], "15.1.0");
    assert_eq!(unicode_normalization::UNICODE_VERSION, (15, 1, 0));
    for v in c.json()["keys"].as_array().unwrap() {
        let input = v["input"].as_str().unwrap();
        assert_eq!(defaults::key(input), v["output"], "U+{:X}", input.chars().next().unwrap() as u32);
        let p = Profile { camera: input.into(), ..Profile::default() };
        assert!(dcp::camera_matches(&p, &json!({"camera":v["folded"]})));
    }
}

#[test]
fn calibration_tags_and_validation() {
    for name in cases("calibration_") {
        let c = Case::open("raw", &name);
        let path = c.file();
        let mut info: CameraInfo = from(&c.json()["before"]);
        assert_eq!(defaults::linear_raw(&path), c.json()["linear_raw"].as_bool().unwrap());
        let result = rawcolor::read_dng_calibration(&path, &mut info);
        if c.json().get("error").is_some() {
            assert!(result.is_err(), "{name}");
        } else {
            result.unwrap();
            close_json(&json(info), &c.json()["after"], 1e-10, &name);
        }
    }
    let mut info = CameraInfo::default();
    rawcolor::read_dng_calibration("nonexistent.nef", &mut info).unwrap();
}

#[test]
fn raw_exif_metadata() {
    for name in cases("metadata_") {
        let c = Case::open("raw", &name);
        let actual = grainy_raw::metadata::read_metadata(c.file()).unwrap();
        assert_eq!(actual, c.json()["output"], "{name}");
    }
}
#[test]
fn invalid_inputs_and_parallel_determinism() {
    assert!(rawcolor::temperature_xyz(1999., 0.).is_err());
    assert!(rawcolor::xyz_temperature([0.; 3]).is_err());
    assert!(CameraSource::new(Image::zeros((2, 2, 4)), CameraInfo::default()).is_err());
    assert!(dcp::tone_map(&Image::zeros((1, 1, 3)), &[]).is_err());
    assert!(defaults::iso_anchors(&[]).is_err());
    assert!(defaults::make_rule(&json!({}), &grainy_core::settings::defaults(), "serial", None, &[], "").is_err());
    let input = Image::from_shape_fn((420, 380, 3), |(y, x, c)| ((y * 61 + x * 17 + c * 5) % 997) as f32 / 996.);
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| rawcolor::reduce_false_color(&input).unwrap());
    let many = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap()
        .install(|| rawcolor::reduce_false_color(&input).unwrap());
    assert_eq!(one, many);
}
use serde_json::json;
#[cfg(all(windows, target_env = "msvc"))]
#[test]
fn xtrans_partial_tiles_equal_the_serial_rgb16_decode() {
    use sha2::{Digest, Sha256};
    use std::{fs, io::Write};

    // Three columns of tiles and partial bottom/right edges exercise above-right
    // dependencies. The first 528 bytes from tifffile 2026.2.20 describe our own
    // uncompressed little-endian 16-bit DNG, black 64, white 16383, D65 identity
    // matrix, unity as-shot neutral and the 6x6 CFA below. No camera data is embedded.
    const CFA: [[u8; 6]; 6] = [
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 2, 1, 1, 0, 1],
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 0, 1, 1, 2, 1],
    ];
    let mut bytes = Vec::with_capacity(528 + 2 * 1031 * 1043);
    bytes.extend_from_slice(include_bytes!("fixtures/xtrans-1031x1043-header.bin"));
    for y in 0..1043 {
        for x in 0..1031 {
            let sample = match CFA[y % 6][x % 6] {
                0 => (x * 13 + y * 29 + x * y / 9) % 16320,
                1 => ((x / 8 + y / 13) % 2) * 14000 + 130,
                _ => ((x ^ y) * 37 + x) % 16220,
            };
            bytes.extend_from_slice(&(64 + sample as u16).to_le_bytes());
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("grainy-xtrans-{}-{now}.dng", std::process::id()));
    let mut file = fs::File::create_new(&path).unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);
    let result =
        grainy_raw::RawFile::open(&path).and_then(|mut raw| raw.postprocess(grainy_raw::DecodeMode::Srgb, false));
    fs::remove_file(&path).unwrap();
    let pixels = result.unwrap();
    let mut digest = Sha256::new();
    for &sample in &pixels {
        digest.update(sample.to_le_bytes());
    }
    // Captured with the unmodified 0.3.0 MSVC LibRaw raster-order decoder.
    assert_eq!(format!("{:x}", digest.finalize()), "881954068bab37ff1737f7afb5ac2dec373efad416ad25e0906c1ebb17ebda92");
}
