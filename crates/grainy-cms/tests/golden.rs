//! Golden tests against the Python Grainy (tools/golden/gen_cms.py). Tolerance 0 everywhere: both sides run the
//! same LittleCMS 2.19 code on the same inputs, so every value must be bit-identical (NaN matches NaN).

use std::path::PathBuf;

use grainy_cms::display::{self, SystemProfiles};
use grainy_cms::{
    convert, convert8_planar, convert_samples, output_rgb, profile, CmsError, Destination, Profile, TransformCache,
    BUILTIN,
};
use grainy_core::Image;
use grainy_testkit::Case;
use ndarray::{Array, Array3, Dimension, Ix3};
use serde_json::Value;

const RGB: [&str; 7] =
    ["sRGB", "Adobe RGB", "Display P3", "ProPhoto RGB", "Linear sRGB", "Linear ProPhoto", "Luma Wide"];
const CUSTOM: [&str; 3] = ["matrix_gamma24", "table_curves", "lut_clut17"];

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "_")
}

fn golden_bytes(case: &str) -> Vec<u8> {
    Case::open("cms", case).array::<u8, ndarray::Ix1>("bytes").to_vec()
}

/// Built-in profiles as made by this crate, custom ones from the goldens.
fn named(name: &str) -> Profile {
    if CUSTOM.contains(&name) {
        Profile::from_bytes(golden_bytes(&format!("custom_{name}")))
    } else {
        profile(name).unwrap()
    }
}

trait Exact: Copy + PartialEq + std::fmt::Debug {
    fn same(self, other: Self) -> bool;
    fn diff(self, other: Self) -> f64;
}
impl Exact for f32 {
    fn same(self, other: Self) -> bool {
        self.to_bits() == other.to_bits() || (self.is_nan() && other.is_nan())
    }
    fn diff(self, other: Self) -> f64 {
        if self.is_nan() || other.is_nan() {
            if self.is_nan() && other.is_nan() {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            (self as f64 - other as f64).abs()
        }
    }
}
impl Exact for u8 {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn diff(self, other: Self) -> f64 {
        (self as f64 - other as f64).abs()
    }
}
impl Exact for u16 {
    fn same(self, other: Self) -> bool {
        self == other
    }
    fn diff(self, other: Self) -> f64 {
        (self as f64 - other as f64).abs()
    }
}

/// Bit-exact comparison; reports the largest difference when it fails.
fn assert_exact<T: Exact>(actual: &[T], expected: &[T], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: lengths differ");
    let mismatches = actual.iter().zip(expected).filter(|(a, e)| !a.same(**e)).count();
    if mismatches > 0 {
        let worst = actual.iter().zip(expected).map(|(a, e)| a.diff(*e)).fold(0.0, f64::max);
        let first = actual.iter().zip(expected).position(|(a, e)| !a.same(*e)).unwrap();
        panic!(
            "{what}: {mismatches} of {} values differ, max |error| {worst:e}; first at {first}: {:?} vs {:?}",
            actual.len(),
            actual[first],
            expected[first]
        );
    }
}

fn flat<T: Clone, D: Dimension>(a: &Array<T, D>) -> Vec<T> {
    a.iter().cloned().collect()
}

fn inputs() -> Case {
    Case::open("cms", "inputs")
}

#[test]
fn links_littlecms_2_19() {
    assert_eq!(grainy_cms::lcms_version(), 2190);
}

#[test]
fn builtin_profile_bytes() {
    for name in BUILTIN {
        let ours = profile(name).unwrap();
        // LittleCMS made these profiles: on macOS its header names the platform `APPL`, on Windows `MSFT`.
        let golden = grainy_testkit::lcms_created(&golden_bytes(&format!("profile_{}", slug(name))));
        assert_eq!(ours.bytes().len(), golden.len(), "{name}: size");
        // Header bytes 24..36 are the creation time (UTC, when the profile was made); all else is identical.
        let mut masked = ours.bytes().to_vec();
        masked[24..36].copy_from_slice(&golden[24..36]);
        assert!(masked == golden, "{name}: profile bytes differ outside the creation date");
        assert_eq!(&ours.bytes()[..4], &golden[..4]);
        // The same bytes every time (made once per process).
        assert_eq!(profile(name).unwrap().bytes(), ours.bytes());
    }
    // Pillow's sRGB profile (engine.srgb_profile, its own LittleCMS 2.19) is the same profile.
    let pillow = grainy_testkit::lcms_created(&golden_bytes("profile_pillow_srgb"));
    let mut ours = profile("sRGB").unwrap().bytes().to_vec();
    ours[24..36].copy_from_slice(&pillow[24..36]);
    assert_eq!(ours, pillow);
}

#[test]
fn convert_every_builtin_pair() {
    let ins = inputs();
    let mix: Array3<f32> = ins.array("mix");
    let u8s: Array3<u8> = ins.array("u8");
    let u16s: Array3<u16> = ins.array("u16");
    for source in RGB {
        for destination in RGB {
            let case = Case::open("cms", &format!("convert_{}__{}", slug(source), slug(destination)));
            let (src, dst) = (named(source), named(destination));
            let what = format!("{source} -> {destination}");
            let out = convert(&mix, &src, &dst).unwrap();
            assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>("f32")), &format!("{what} f32"));
            let check = |name: &str, out: Vec<f32>| {
                assert_exact(&out, &flat(&case.array::<f32, Ix3>(name)), &format!("{what} {name}"));
            };
            let out8: Vec<u8> = convert_samples(&flat(&u8s), &src, &dst).unwrap();
            assert_exact(&out8, &flat(&case.array::<u8, Ix3>("u8_u8")), &format!("{what} u8_u8"));
            let out16: Vec<u16> = convert_samples(&flat(&u16s), &src, &dst).unwrap();
            assert_exact(&out16, &flat(&case.array::<u16, Ix3>("u16_u16")), &format!("{what} u16_u16"));
            if case.has("u8_f32") {
                check("u8_f32", convert_samples(&flat(&u8s), &src, &dst).unwrap());
                check("u16_f32", convert_samples(&flat(&u16s), &src, &dst).unwrap());
                let f16: Vec<u16> = convert_samples(&flat(&mix), &src, &dst).unwrap();
                assert_exact(&f16, &flat(&case.array::<u16, Ix3>("f32_u16")), &format!("{what} f32_u16"));
                let f8: Vec<u8> = convert_samples(&flat(&mix), &src, &dst).unwrap();
                assert_exact(&f8, &flat(&case.array::<u8, Ix3>("f32_u8")), &format!("{what} f32_u8"));
            }
        }
    }
}

#[test]
fn convert_nan_infinity_huge() {
    let special: Array3<f32> = inputs().array("special");
    let case = Case::open("cms", "convert_special");
    for (i, pair) in case.json()["pairs"].as_array().unwrap().iter().enumerate() {
        let (source, destination) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        let out = convert(&special, &named(source), &named(destination)).unwrap();
        assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>(&format!("out{i}"))), &format!("special {i}"));
    }
}

#[test]
fn convert_full_size_inputs() {
    for name in ["colours", "wide"] {
        let input = grainy_testkit::input(name);
        let case = Case::open("cms", &format!("convert_full_{name}"));
        let out = convert(&input, &named("Luma Wide"), &named("sRGB")).unwrap();
        assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>("luma_wide__srgb")), name);
        let out = convert(&input, &named("Linear ProPhoto"), &named("Linear sRGB")).unwrap();
        assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>("linear_prophoto__linear_srgb")), name);
    }
}

#[test]
fn convert_through_test_profiles_both_ways() {
    let ins = inputs();
    let mix: Array3<f32> = ins.array("mix");
    for name in CUSTOM {
        let custom = named(name);
        let case = Case::open("cms", &format!("convert_custom_{name}"));
        for other in ["sRGB", "Luma Wide", "Linear sRGB"] {
            let to = convert(&mix, &custom, &named(other)).unwrap();
            assert_exact(&flat(&to), &flat(&case.array::<f32, Ix3>(&format!("to__{}", slug(other)))), name);
            let from = convert(&mix, &named(other), &custom).unwrap();
            assert_exact(&flat(&from), &flat(&case.array::<f32, Ix3>(&format!("from__{}", slug(other)))), name);
        }
        let out: Vec<u8> = convert_samples(&flat(&ins.array::<u8, Ix3>("u8")), &custom, &named("sRGB")).unwrap();
        assert_exact(&out, &flat(&case.array::<u8, Ix3>("u8_to__srgb")), name);
        let out: Vec<u16> = convert_samples(&flat(&ins.array::<u16, Ix3>("u16")), &named("sRGB"), &custom).unwrap();
        assert_exact(&out, &flat(&case.array::<u16, Ix3>("u16_from__srgb")), name);
    }
}

#[test]
fn convert8_planar_every_source() {
    let u8s: Array3<u8> = inputs().array("u8");
    let as_float = u8s.mapv(|v| v as f32 / 255.0);
    for source in RGB.iter().chain(CUSTOM.iter()) {
        for destination in ["Linear sRGB", "Linear ProPhoto"] {
            let case = Case::open("cms", &format!("planar8_{}__{}", slug(source), slug(destination)));
            let planes = convert8_planar(u8s.view(), &named(source), &named(destination)).unwrap();
            assert_exact(&flat(&planes), &flat(&case.array::<f32, Ix3>("planes")), source);
            // The oracle's claim: the same values as the float path on v / 255.
            let float = convert(&as_float, &named(source), &named(destination)).unwrap();
            let moved = float.permuted_axes([2, 0, 1]);
            assert_exact(&flat(&planes), &flat(&moved.to_owned()), &format!("{source} planar8 == float"));
        }
    }
}

#[test]
fn output_rgb_every_destination() {
    let mix: Array3<f32> = inputs().array("mix");
    for working in ["sRGB", "ProPhoto"] {
        for destination in RGB.iter().chain(CUSTOM.iter()) {
            let case = Case::open("cms", &format!("output_{}__{}", slug(working), slug(destination)));
            let icc;
            let target = if CUSTOM.contains(destination) {
                icc = named(destination);
                Destination::Icc(&icc)
            } else {
                Destination::Named(destination)
            };
            let out = output_rgb(&mix, working, target).unwrap();
            assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>("output")), &format!("{working} -> {destination}"));
        }
        let case = Case::open("cms", &format!("output_garden_{}", slug(working)));
        let garden: Array3<f32> = case.array("input");
        for (key, destination) in [("display_p3", "Display P3"), ("adobe_rgb", "Adobe RGB")] {
            let out = output_rgb(&garden, working, Destination::Named(destination)).unwrap();
            assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>(key)), &format!("garden {working} {key}"));
        }
    }
}

#[test]
fn display_rgb_cases() {
    let mix: Array3<f32> = inputs().array("mix");
    let case = Case::open("cms", "display_rgb");
    assert_exact(&flat(&grainy_cms::display_rgb(&mix, None).unwrap()), &flat(&case.array::<f32, Ix3>("none")), "none");
    for (key, name) in [("display_p3", "Display P3"), ("matrix_gamma24", "matrix_gamma24")] {
        let out = grainy_cms::display_rgb(&mix, Some(&named(name))).unwrap();
        assert_exact(&flat(&out), &flat(&case.array::<f32, Ix3>(key)), key);
    }
}

fn message<T>(result: Result<T, CmsError>) -> Value {
    match result {
        Ok(_) => Value::Null,
        Err(error) => Value::String(error.to_string()),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grainy_cms_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn invalid_profiles_give_the_oracle_errors() {
    let ins = inputs();
    let hand: Array3<f32> = ins.array("hand");
    let u8s: Array3<u8> = ins.array("u8");
    let srgb = named("sRGB");
    let linear = named("Linear sRGB");
    let dir = temp_dir("bad");
    for name in ["empty", "short", "truncated", "garbage", "bad_magic", "gray", "lab", "input_class"] {
        let case = Case::open("cms", &format!("bad_{name}"));
        let data = case.array::<u8, ndarray::Ix1>("bytes").to_vec();
        let bad = Profile::from_bytes(data.clone());
        let path = dir.join(format!("{name}.icc"));
        std::fs::write(&path, &data).unwrap();
        let errors = &case.json()["errors"];
        let expect = |key: &str, got: Value| {
            let want = &errors[key];
            let want = if want.is_null() { Value::Null } else { want[1].clone() };
            assert_eq!(got, want, "bad_{name} {key}");
        };
        expect("convert_to", message(convert(&hand, &srgb, &bad)));
        expect("convert_from", message(convert(&hand, &bad, &srgb)));
        expect("convert_u8", message(convert_samples::<u8, u8>(&flat(&u8s), &bad, &srgb)));
        expect("planar8", message(convert8_planar(u8s.view(), &bad, &linear)));
        expect("profile_path", message(profile(path.to_str().unwrap())));
        expect("checked", message(display::checked(&data, "name.icc")));
    }
    let _ = std::fs::remove_dir_all(&dir);

    let misc = Case::open("cms", "errors_misc");
    let two = Image::zeros((2, 2, 2));
    assert_eq!(message(convert(&two, &srgb, &srgb)), misc.json()["native_shape"][1]);
    let two8 = Array3::<u8>::zeros((2, 2, 2));
    assert_eq!(message(convert8_planar(two8.view(), &srgb, &srgb)), misc.json()["planar8_dtype"][1]);
    assert!(matches!(profile("Z:/no/such/profile.icc"), Err(CmsError::Io(_))));
    assert_eq!(misc.json()["profile_missing"][0], "FileNotFoundError");
    // The oracle's temporary folder is stored as `<tmp>`; Rust's message names its own folder, masked alike.
    let oracle = misc.json()["profile_missing"][1].as_str().unwrap();
    assert_eq!(oracle, "[Errno 2] No such file or directory: '<tmp>\\\\missing.icc'");
    let folder = std::env::temp_dir().join("grainy-cms-no-such-folder");
    let own = message(profile(folder.join("missing.icc").to_str().unwrap()))
        .as_str()
        .unwrap()
        .replace(&folder.display().to_string(), "<tmp>");
    assert!(!own.is_empty() && !own.contains(&folder.display().to_string()), "{own}");
}

#[test]
fn display_checked_profiles() {
    let case = Case::open("cms", "display_checked");
    for (name, expected) in case.json()["checked"].as_object().unwrap() {
        let data = if CUSTOM.contains(&name.as_str()) {
            golden_bytes(&format!("custom_{name}"))
        } else {
            golden_bytes(&format!("profile_{}", slug(name)))
        };
        let result = display::checked(&data, &format!("{name}.icc"));
        if let Some(error) = expected.get("error") {
            assert_eq!(message(result), error[1], "{name}");
        } else {
            let found = result.unwrap();
            assert_eq!(found.description, expected["description"].as_str().unwrap(), "{name}");
            assert_eq!(found.digest, expected["digest"].as_str().unwrap(), "{name}");
            assert_eq!(found.data.bytes(), &data[..]);
        }
    }
}

struct FakeSystem {
    mac: bool,
    windows: Result<Option<String>, String>,
    mac_data: Option<Vec<u8>>,
}

impl SystemProfiles for FakeSystem {
    fn is_mac(&self) -> bool {
        self.mac
    }
    fn windows_profile_path(&self, _screen: &str) -> Result<Option<String>, String> {
        self.windows.clone()
    }
    fn mac_profile_data(&self, _screen: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.mac_data.clone())
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn display_resolve_cases() {
    let dir = temp_dir("resolve");
    let p3 = golden_bytes("profile_display_p3");
    let file = |name: &str, data: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        path.to_str().unwrap().to_string()
    };
    let p3_file = file("p3.icc", &p3);
    let lut_file = file("lut.icc", &golden_bytes("custom_lut_clut17"));
    let garbage = golden_bytes("bad_garbage");
    let garbage_file = file("garbage.icc", &garbage);
    let gray_file = file("gray.icc", &golden_bytes("bad_gray"));
    let windows = |answer: Result<Option<String>, String>| FakeSystem { mac: false, windows: answer, mac_data: None };
    let mac = |data: Option<Vec<u8>>| FakeSystem { mac: true, windows: Ok(None), mac_data: data };
    let none = windows(Ok(None));
    let cases: Vec<(&str, display::DisplayProfile)> = vec![
        ("off", display::resolve("off", "S1", "", &none)),
        ("manual_none", display::resolve("manual", "S1", "", &none)),
        ("manual_p3", display::resolve("manual", "S1", &p3_file, &none)),
        ("manual_lut", display::resolve("manual", "S1", &lut_file, &none)),
        ("manual_garbage", display::resolve("manual", "S1", &garbage_file, &none)),
        ("manual_gray", display::resolve("manual", "S1", &gray_file, &none)),
        ("bogus_mode", display::resolve("bogus", "S1", "", &none)),
        ("auto_windows_none", display::resolve("auto", "", "", &none)),
        ("auto_windows_p3", display::resolve("auto", r"\\.\DISPLAY1", "", &windows(Ok(Some(p3_file.clone()))))),
        (
            "auto_windows_error",
            display::resolve("auto", "bad", "", &windows(Err("Windows 모니터 이름을 확인할 수 없습니다.".into()))),
        ),
        ("auto_mac_p3", display::resolve("auto", "Studio Display (2)", "", &mac(Some(p3.clone())))),
        ("auto_mac_noname", display::resolve("auto", " (2)", "", &mac(Some(p3.clone())))),
        ("auto_mac_none", display::resolve("auto", "Studio Display (2)", "", &mac(None))),
        ("auto_mac_garbage", display::resolve("auto", "Studio Display (2)", "", &mac(Some(garbage.clone())))),
        ("manual_mac_p3", display::resolve("manual", "Studio Display (2)", &p3_file, &mac(None))),
        ("off_mac", display::resolve("off", "Studio Display (2)", "", &mac(None))),
    ];
    let golden = Case::open("cms", "display_resolve");
    let all = golden.json()["resolved"].as_object().unwrap();
    assert_eq!(all.len(), cases.len());
    for (key, got) in cases {
        let want = &all[key];
        let file_name = std::path::Path::new(&got.path).file_name().map(|n| n.to_string_lossy().into_owned());
        assert_eq!(file_name.unwrap_or_default(), want["path"].as_str().unwrap(), "{key} path");
        assert_eq!(got.mode, want["mode"].as_str().unwrap(), "{key} mode");
        assert_eq!(got.screen, want["screen"].as_str().unwrap(), "{key} screen");
        assert_eq!(got.description, want["description"].as_str().unwrap(), "{key} description");
        assert_eq!(got.message, want["message"].as_str().unwrap(), "{key} message");
        assert_eq!(got.error, want["error"].as_bool().unwrap(), "{key} error");
        assert_eq!(got.digest, want["digest"].as_str().unwrap(), "{key} digest");
        let data = got.data.as_ref().map(|p| sha256_hex(p.bytes()));
        assert_eq!(data.as_deref(), want["data_sha256"].as_str(), "{key} data");
    }
    // Empty data from the system is no profile (the oracle's `if system:` on `b''`): as without data.
    let empty = display::resolve("auto", "Studio Display (2)", "", &mac(Some(Vec::new())));
    let none = display::resolve("auto", "Studio Display (2)", "", &mac(None));
    assert_eq!((empty.message, empty.error, empty.digest), (none.message, none.error, none.digest));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transform_cache_matches_the_oracle() {
    let case = Case::open("cms", "cache");
    let meta = case.json();
    assert_eq!(meta["max_transforms"], grainy_cms::MAX_TRANSFORMS);
    assert_eq!(meta["max_profile_bytes"], grainy_cms::MAX_PROFILE_BYTES);
    let pad = meta["pad"].as_u64().unwrap() as usize;
    let padded = |name: &str| {
        let mut data = golden_bytes(&format!("profile_{}", slug(name)));
        data.resize(data.len() + pad, 0);
        Profile::from_bytes(data)
    };
    let pads = [padded("sRGB"), padded("Display P3"), padded("Adobe RGB")];
    let lookup = |label: &str| match label {
        "pad0" => pads[0].clone(),
        "pad1" => pads[1].clone(),
        "pad2" => pads[2].clone(),
        name if CUSTOM.contains(&name) => named(name),
        name => Profile::from_bytes(golden_bytes(&format!("profile_{}", slug(name)))),
    };
    let cache = TransformCache::new();
    for (i, step) in meta["steps"].as_array().unwrap().iter().enumerate() {
        let source = lookup(step["source"].as_str().unwrap());
        let destination = lookup(step["destination"].as_str().unwrap());
        cache.get(&source, &destination, step["planar8"].as_bool().unwrap()).unwrap();
        let info = cache.info();
        assert_eq!(info.transforms as u64, step["transforms"].as_u64().unwrap(), "step {i} transforms");
        assert_eq!(info.profile_bytes as u64, step["profile_bytes"].as_u64().unwrap(), "step {i} bytes");
        assert_eq!(info.hits, step["hits"].as_u64().unwrap(), "step {i} hits");
        assert_eq!(info.misses, step["misses"].as_u64().unwrap(), "step {i} misses");
    }
    cache.clear();
    assert_eq!(cache.info(), grainy_cms::CacheInfo::default());
}

fn big_image() -> Image {
    // 400 x 400 = 160k pixels: above PARALLEL_PIXELS, so the parallel paths run.
    let colours = grainy_testkit::input("noise");
    Image::from_shape_fn((400, 400, 3), |(y, x, c)| colours[[y % 64, x % 96, c]] * (1.0 + (x as f32) / 400.0))
}

#[test]
fn results_do_not_depend_on_thread_count() {
    let image = big_image();
    let (src, dst) = (named("Luma Wide"), named("Display P3"));
    let single = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let many = rayon::ThreadPoolBuilder::new().num_threads(7).build().unwrap();
    let one = single.install(|| convert(&image, &src, &dst).unwrap());
    let seven = many.install(|| convert(&image, &src, &dst).unwrap());
    assert_exact(&flat(&one), &flat(&seven), "convert threads");
    let rgb8 = image.mapv(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let one = single.install(|| convert8_planar(rgb8.view(), &src, &dst).unwrap());
    let seven = many.install(|| convert8_planar(rgb8.view(), &src, &dst).unwrap());
    assert_exact(&flat(&one), &flat(&seven), "planar8 threads");
    let float = convert(&rgb8.mapv(|v| v as f32 / 255.0), &src, &dst).unwrap();
    assert_exact(&flat(&one), &flat(&float.permuted_axes([2, 0, 1]).to_owned()), "planar8 == float, parallel");
}

#[test]
fn empty_and_shape_edge_cases() {
    let srgb = named("sRGB");
    let empty = Image::zeros((0, 5, 3));
    assert_eq!(convert(&empty, &srgb, &named("Display P3")).unwrap().dim(), (0, 5, 3));
    // The native path checks sizes before anything else, even for empty images.
    let tiny = Profile::from_bytes(vec![0u8; 10]);
    assert_eq!(convert(&empty, &srgb, &tiny), Err(CmsError::InvalidProfileSize));
    assert_eq!(convert_samples::<f32, f32>(&[0.0, 1.0], &srgb, &srgb), Err(CmsError::RgbSamplesRequired));
}

#[test]
fn vendored_littlecms_is_the_pinned_source() {
    // vendor/lcms2-2.19/SOURCE.json lists the SHA-256 of every retained file (the oracle's copy).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("lcms2-2.19");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(root.join("SOURCE.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], "2.19.0");
    let files = manifest["files"].as_object().unwrap();
    assert!(files.len() > 30);
    for (name, digest) in files {
        let data = std::fs::read(root.join(name)).unwrap();
        assert_eq!(sha256_hex(&data), digest.as_str().unwrap(), "{name}");
    }
}
