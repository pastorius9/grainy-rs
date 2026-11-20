//! Golden tests for what loading image files needs (tools/golden/gen_cms.py, second run): grey, CMYK and CIELAB
//! transforms (`imagecodecs.cms_transform(colorspace=...)`), the calibrated PNG/TIFF profiles and the Lab
//! profiles (`imagecodecs.cms_profile`), and `cms_info` for them. Tolerance 0 everywhere (same LittleCMS 2.19,
//! same calls): every value must be bit-identical, NaN matching NaN; profile bytes may differ only in the
//! creation time (header bytes 24..36).

use grainy_cms::{
    cms_transform, cms_transform_image, convert, gray_profile, lab_profile, profile, rgb_profile, srgb_transfer_table,
    CmsError, ColorSpace, Profile, Sample, TransferFunction, BUILTIN,
};
use grainy_core::Image;
use grainy_testkit::Case;
use ndarray::{Array1, Array2, Array3, Ix1};
use serde_json::Value;

const WORKING: [&str; 3] = ["Linear sRGB", "Linear ProPhoto", "sRGB"];
const CALIBRATED: [&str; 9] = [
    "png_rgb_gamma",
    "png_rgb_gamma_chrm",
    "png_rgb_chrm",
    "png_gray_gamma",
    "png_gray_chrm",
    "tiff_rgb_chroma",
    "tiff_rgb_curves3",
    "tiff_rgb_curve1",
    "tiff_gray_curve",
];

fn slug(name: &str) -> String {
    name.to_lowercase().replace(' ', "_")
}

fn case(name: &str) -> Case {
    Case::open("cms", name)
}

fn bits(value: f32) -> Option<u32> {
    (!value.is_nan()).then(|| value.to_bits())
}

/// Bit-exact comparison of float samples (NaN = NaN); returns the largest |difference| (0 when exact).
fn exact(actual: &[f32], expected: &[f32], what: &str) -> f64 {
    assert_eq!(actual.len(), expected.len(), "{what}: lengths differ");
    let mut worst = 0f64;
    let mut mismatches = 0;
    for (a, e) in actual.iter().zip(expected) {
        if bits(*a) != bits(*e) {
            mismatches += 1;
            let d = if a.is_nan() || e.is_nan() { f64::INFINITY } else { (*a as f64 - *e as f64).abs() };
            worst = worst.max(d);
        }
    }
    assert_eq!(mismatches, 0, "{what}: {mismatches} of {} values differ, max |error| {worst:e}", actual.len());
    worst
}

/// `golden` is a profile LittleCMS made (on macOS its header names the platform `APPL`, on Windows `MSFT`).
fn same_profile(ours: &Profile, golden: &[u8], what: &str) {
    let golden = grainy_testkit::lcms_created(golden);
    assert_eq!(ours.bytes().len(), golden.len(), "{what}: size");
    let mut masked = ours.bytes().to_vec();
    masked[24..36].copy_from_slice(&golden[24..36]);
    assert!(masked == golden, "{what}: profile bytes differ outside the creation date");
}

fn golden_bytes(name: &str) -> Vec<u8> {
    case(name).array::<u8, Ix1>("bytes").to_vec()
}

/// colorspace, deviceclass and description as `cms_info` reports them.
fn same_info(profile: &Profile, info: &Value, what: &str) {
    let ours = profile.info().unwrap();
    assert_eq!(ours.colorspace, info["colorspace"].as_str(), "{what}: colorspace");
    assert_eq!(ours.deviceclass, info["deviceclass"].as_str(), "{what}: deviceclass");
    assert_eq!(ours.description.as_deref(), info["description"].as_str(), "{what}: description");
}

/// A calibrated profile rebuilt from the golden arguments, as `rasterio._png_profile` / `_tiff_profile` call
/// `imagecodecs.cms_profile`.
fn calibrated(name: &str) -> Profile {
    let c = case(&format!("calibrated_{name}"));
    let transfers = case("calibrated_transfers");
    let white = c.array::<f64, Ix1>("white");
    let white = Some([white[0], white[1]]);
    let primaries = c.has("primaries").then(|| {
        let p = c.array::<f64, Ix1>("primaries");
        [p[0], p[1], p[2], p[3], p[4], p[5]]
    });
    let gamma = c.has("gamma").then(|| c.array::<f64, Ix1>("gamma")[0]);
    // `to_linear` is the C runtime's `powf`: equal to the oracle's table with the goldens' C runtime; elsewhere
    // within one unit in the last place (6 of its 65536 entries differ from the correctly rounded values). The
    // profile is built from the oracle's table, so what follows tests LittleCMS alone, exactly, on every system.
    let srgb = transfers.array::<f32, Ix1>("srgb").to_vec();
    let ours = srgb_transfer_table();
    let ulps = ours.iter().zip(&srgb).map(|(a, b)| a.to_bits().abs_diff(b.to_bits())).max().unwrap_or(0);
    let differing = ours.iter().zip(&srgb).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
    println!("to_linear(linspace(0, 1, 65536)): {differing} entries differ, at most {ulps} ulp");
    assert!(ulps <= grainy_testkit::maths_tolerance(0, 1), "to_linear(linspace(0, 1, 65536)): {ulps} ulp");
    let curves3: Array2<f32> = transfers.array("curves3");
    let curve1: Array1<f32> = transfers.array("curve1");
    let rows: Vec<Vec<f32>> = curves3.outer_iter().map(|row| row.to_vec()).collect();
    let curve1 = curve1.to_vec();
    let transfer = match (gamma, c.json()["transfer"].as_str()) {
        (Some(gamma), _) => TransferFunction::Gamma(gamma),
        (None, Some("srgb")) => TransferFunction::Table(&srgb),
        (None, Some("curve1")) => TransferFunction::Table(&curve1),
        (None, Some("curves3")) => TransferFunction::Tables([&rows[0], &rows[1], &rows[2]]),
        other => panic!("{name}: unknown transfer {other:?}"),
    };
    match c.json()["kind"].as_str() {
        Some("gray") => gray_profile(white, transfer).unwrap(),
        _ => rgb_profile(white, primaries, transfer).unwrap(),
    }
}

#[test]
fn calibrated_profile_bytes_and_info() {
    for name in CALIBRATED {
        let ours = calibrated(name);
        let c = case(&format!("calibrated_{name}"));
        same_profile(&ours, &golden_bytes(&format!("calibrated_{name}")), name);
        same_info(&ours, &c.json()["info"], name);
    }
}

#[test]
fn lab_profiles_bytes_and_info() {
    for (name, version4, white) in
        [("lab4", true, None), ("lab2", false, None), ("lab4_d65", true, Some([0.3127, 0.329]))]
    {
        let ours = lab_profile(version4, white).unwrap();
        same_profile(&ours, &golden_bytes(&format!("profile_{name}")), name);
        same_info(&ours, &case(&format!("profile_{name}")).json()["info"], name);
    }
}

#[test]
fn info_of_cmyk_test_profiles_and_builtins() {
    for name in ["cmyk_clut9", "cmyk_clut5_a2b"] {
        let profile = Profile::from_bytes(golden_bytes(&format!("profile_{name}")));
        same_info(&profile, &case(&format!("profile_{name}")).json()["info"], name);
    }
    let info = case("info_builtin");
    for name in BUILTIN {
        same_info(&profile(name).unwrap(), &info.json()["info"][name], name);
    }
}

#[test]
fn gray_profile_rejects_three_curves() {
    let curve = [0.0f32, 1.0];
    let error = gray_profile(None, TransferFunction::Tables([&curve, &curve, &curve])).unwrap_err();
    assert_eq!(error.to_string(), "invalid transferfunction shape or dtype");
}

fn to_rgb<I: Sample>(samples: &[I], source: &Profile, space: ColorSpace, destination: &str) -> Vec<f32> {
    cms_transform::<I, f32>(samples, source, space, &profile(destination).unwrap(), ColorSpace::Rgb).unwrap()
}

#[test]
fn gray_to_working_profiles() {
    let inputs = case("inputs_loading");
    let gray: Array2<f32> = inputs.array("gray");
    let gray_u8: Array2<u8> = inputs.array("gray_u8");
    let gray_u16: Array2<u16> = inputs.array("gray_u16");
    let mut sources = vec![("gray_srgb".to_string(), profile("Gray sRGB").unwrap())];
    for name in CALIBRATED.into_iter().filter(|n| n.contains("gray")) {
        sources.push((name.to_string(), calibrated(name)));
    }
    let levels: Vec<f32> = (0..=255u8).map(|v| v as f32 / 255.0).collect(); // _normalise(np.arange(256))
    let mut worst = 0f64;
    for (name, source) in &sources {
        let golden = case(&format!("transform_gray_{name}"));
        for destination in WORKING {
            let expected: Array3<f32> = golden.array(&format!("to__{}", slug(destination)));
            let ours = to_rgb(gray.as_slice().unwrap(), source, ColorSpace::Gray, destination);
            worst = worst.max(exact(&ours, expected.as_slice().unwrap(), &format!("{name} -> {destination}")));
        }
        let expected: Array3<f32> = golden.array("u8_to__linear_srgb");
        let ours = to_rgb(gray_u8.as_slice().unwrap(), source, ColorSpace::Gray, "Linear sRGB");
        worst = worst.max(exact(&ours, expected.as_slice().unwrap(), &format!("{name} u8")));
        let expected: Array3<f32> = golden.array("u16_to__linear_prophoto");
        let ours = to_rgb(gray_u16.as_slice().unwrap(), source, ColorSpace::Gray, "Linear ProPhoto");
        worst = worst.max(exact(&ours, expected.as_slice().unwrap(), &format!("{name} u16")));
        for (working, destination) in [("srgb", "Linear sRGB"), ("prophoto", "Linear ProPhoto")] {
            let expected: Array2<f32> = golden.array(&format!("gray8_table__{working}"));
            let ours = to_rgb(&levels, source, ColorSpace::Gray, destination);
            worst = worst.max(exact(&ours, expected.as_slice().unwrap(), &format!("{name} gray8 table")));
        }
    }
    println!("grey transforms: {} sources, max |error| {worst:e} (tolerance 0)", sources.len());
}

#[test]
fn cmyk_and_lab_to_working_profiles() {
    let inputs = case("inputs_loading");
    let cmyk: Image = inputs.array::<f32, ndarray::Ix3>("cmyk");
    let lab: Image = inputs.array::<f32, ndarray::Ix3>("lab");
    let mut worst = 0f64;
    let cases = [
        ("cmyk_clut9", Profile::from_bytes(golden_bytes("profile_cmyk_clut9")), ColorSpace::Cmyk, &cmyk),
        ("cmyk_clut5_a2b", Profile::from_bytes(golden_bytes("profile_cmyk_clut5_a2b")), ColorSpace::Cmyk, &cmyk),
        ("lab4", lab_profile(true, None).unwrap(), ColorSpace::Lab, &lab),
        ("lab2", lab_profile(false, None).unwrap(), ColorSpace::Lab, &lab),
        ("lab4_d65", lab_profile(true, Some([0.3127, 0.329])).unwrap(), ColorSpace::Lab, &lab),
    ];
    for (name, source, space, input) in cases {
        let golden = case(&format!("transform_{name}"));
        for destination in WORKING {
            let expected: Image = golden.array(&format!("to__{}", slug(destination)));
            let ours =
                cms_transform_image(input, &source, space, &profile(destination).unwrap(), ColorSpace::Rgb).unwrap();
            assert_eq!(ours.dim(), expected.dim());
            worst = worst.max(exact(
                ours.as_slice().unwrap(),
                expected.as_slice().unwrap(),
                &format!("{name} -> {destination}"),
            ));
        }
    }
    println!("CMYK and CIELAB transforms: max |error| {worst:e} (tolerance 0)");
}

#[test]
fn calibrated_rgb_to_working_profiles() {
    let mix: Image = case("inputs").array("mix");
    let mut worst = 0f64;
    for name in CALIBRATED.into_iter().filter(|n| n.contains("rgb")) {
        let source = calibrated(name);
        let golden = case(&format!("transform_rgb_{name}"));
        for destination in WORKING {
            let expected: Image = golden.array(&format!("to__{}", slug(destination)));
            let ours = convert(&mix, &source, &profile(destination).unwrap()).unwrap();
            worst = worst.max(exact(
                ours.as_slice().unwrap(),
                expected.as_slice().unwrap(),
                &format!("{name} -> {destination}"),
            ));
        }
    }
    println!("calibrated RGB conversions: max |error| {worst:e} (tolerance 0)");
}

#[test]
fn wrong_colour_space_fails_as_imagecodecs() {
    let golden = case("transform_errors");
    let message = |key: &str| golden.json()[key][1].as_str().unwrap().to_string();
    let srgb = profile("sRGB").unwrap();
    let gray = profile("Gray sRGB").unwrap();
    let cmyk = Profile::from_bytes(golden_bytes("profile_cmyk_clut9"));
    let rgb = [0.5f32; 6];
    let cases = [
        ("gray_as_rgb", cms_transform::<f32, f32>(&rgb, &gray, ColorSpace::Rgb, &srgb, ColorSpace::Rgb)),
        ("cmyk_as_lab", cms_transform::<f32, f32>(&rgb, &cmyk, ColorSpace::Lab, &srgb, ColorSpace::Rgb)),
        ("rgb_as_gray", cms_transform::<f32, f32>(&rgb, &srgb, ColorSpace::Gray, &srgb, ColorSpace::Rgb)),
    ];
    for (key, result) in cases {
        let error = result.unwrap_err();
        assert_eq!(error, CmsError::CreateTransformFailed, "{key}");
        assert_eq!(error.to_string(), message(key), "{key}");
    }
    // Samples that are not whole pixels of the colour space.
    let error = cms_transform::<f32, f32>(&[0.0; 5], &cmyk, ColorSpace::Cmyk, &srgb, ColorSpace::Rgb).unwrap_err();
    assert_eq!(error, CmsError::SamplesShape);
    let wrong = Image::zeros((2, 2, 3));
    assert_eq!(
        cms_transform_image(&wrong, &cmyk, ColorSpace::Cmyk, &srgb, ColorSpace::Rgb).unwrap_err(),
        CmsError::SamplesShape
    );
    // Empty input converts to nothing.
    assert!(to_rgb::<f32>(&[], &gray, ColorSpace::Gray, "sRGB").is_empty());
}
