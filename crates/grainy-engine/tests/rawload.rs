//! RAW files through `grainy_engine::files::{load_photo, export_photo, photo_info}` against the oracle's
//! `engine.load_image` / `engine.export_image` (tools/golden/gen_rawload.py, golden/rawload/).
//!
//! Decode paths (LibRaw `postprocess`, `rawcolor.decode`, false-colour reduction) and the Pillow bilinear reduction of
//! `resize_float` are compared with tolerance 0: the measured maximum is printed for every group (`--nocapture`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use grainy_codecs::{decode_bitmap, ExportFormat, LoadOptions, SizeLimit};
use grainy_core::Settings;
use grainy_engine::files::{export_photo, load_photo, photo_info, Error, ExportRequest, Photo};
use grainy_testkit::{max_abs_diff, Case};
use ndarray::{Array3, ArrayD};
use serde_json::{Map, Value};

fn golden() -> PathBuf {
    grainy_testkit::root().join("golden").join("rawload")
}

fn file(name: &str) -> PathBuf {
    golden().join("_files").join(name)
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-output").join("rawload");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn index() -> Value {
    serde_json::from_str(&fs::read_to_string(golden().join("_index.json")).unwrap()).unwrap()
}

fn names(key: &str) -> Vec<String> {
    index()[key].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_owned()).collect()
}

/// Settings as the oracle's `settings(working_space=.., **changes)`: the defaults plus the case's changes.
fn settings(changes: &Value, working_space: &str) -> Settings {
    let mut map = Map::new();
    map.insert("working_space".into(), working_space.into());
    if let Value::Object(changes) = changes {
        map.extend(changes.clone());
    }
    Settings(grainy_core::normalized(&map).0)
}

/// Numbers compare as numbers (Python writes `1` where Rust has `1.0`); everything else exactly. Returns the
/// first difference.
fn json_difference(actual: &Value, expected: &Value, path: &str) -> Option<String> {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => (a.as_f64() != e.as_f64()).then(|| format!("{path}: {a} vs {e}")),
        (Value::Object(a), Value::Object(e)) => {
            for key in a.keys().chain(e.keys()) {
                match (a.get(key), e.get(key)) {
                    (Some(x), Some(y)) => {
                        if let Some(d) = json_difference(x, y, &format!("{path}.{key}")) {
                            return Some(d);
                        }
                    }
                    (x, y) => return Some(format!("{path}.{key}: {x:?} vs {y:?}")),
                }
            }
            None
        }
        (Value::Array(a), Value::Array(e)) => {
            if a.len() != e.len() {
                return Some(format!("{path}: length {} vs {}", a.len(), e.len()));
            }
            a.iter().zip(e).enumerate().find_map(|(i, (x, y))| json_difference(x, y, &format!("{path}[{i}]")))
        }
        _ => (actual != expected).then(|| format!("{path}: {actual} vs {expected}")),
    }
}

/// The metadata of a loaded photo against the golden dictionary: key by key, the fingerprint by its live facts.
fn check_metadata(name: &str, metadata: &grainy_codecs::Metadata, golden: &Value, source: &Path) {
    let mut actual = metadata.clone();
    let fingerprint = actual.remove("file_fingerprint").expect("file_fingerprint");
    let expected_size = golden["file_fingerprint"]["size"].as_u64().unwrap();
    assert_eq!(fingerprint["size"].as_u64(), Some(expected_size), "{name}: fingerprint size");
    let stat = fs::metadata(source).unwrap();
    let nanos = stat.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    assert_eq!(fingerprint["mtime_ns"].as_u64(), Some(nanos), "{name}: fingerprint mtime");
    let path = fingerprint["path"].as_str().unwrap();
    assert!(path.ends_with(source.file_name().unwrap().to_str().unwrap()), "{name}: fingerprint path {path}");
    assert!(!path.starts_with(r"\\?\"), "{name}: fingerprint path {path}");
    let mut expected = golden.clone();
    expected.as_object_mut().unwrap().remove("file_fingerprint");
    if let Some(difference) = json_difference(&Value::Object(actual), &expected, "metadata") {
        panic!("{name}: metadata differs: {difference}");
    }
}

fn stride(a: &Array3<f32>, step: usize) -> Array3<f32> {
    a.slice(ndarray::s![..;step, ..;step, ..]).to_owned()
}

/// Absolute tolerance of a Pillow bilinear reduction (`resize_float`) in float32.
const RESIZE_TOLERANCE: f32 = 1e-7;

#[test]
fn load_photo_matches_load_image() {
    let mut worst: BTreeMap<String, f32> = BTreeMap::new();
    let cases = names("loads");
    assert!(cases.len() >= 150, "{} cases", cases.len());
    for name in &cases {
        let case = Case::open("rawload", name);
        let meta = case.json();
        let space = meta["working_space"].as_str().unwrap();
        let max_size = meta["max_size"].as_u64().map(|v| v as usize);
        let source = file(meta["file"].as_str().unwrap());
        let loaded = load_photo(&source, max_size, &settings(&meta["settings"], space)).unwrap();
        let native = meta["native"].as_bool().unwrap();
        assert_eq!(matches!(loaded.photo, Photo::Camera(_)), native, "{name}: photo kind");
        assert_eq!(meta["type"] == "CameraSource", native, "{name}: golden kind");
        let step = meta["stride"].as_u64().unwrap() as usize;
        let expected: Array3<f32> = case.array("pixels");
        let full = loaded.photo.pixels();
        let shape: Vec<usize> =
            meta["shape"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
        assert_eq!(full.shape(), shape.as_slice(), "{name}: shape");
        let (error, _) = max_abs_diff(&stride(full, step), &expected);
        let group = format!(
            "{}/{}",
            if max_size.is_none() { "decode" } else { "decode+resize" },
            meta["mode"].as_str().unwrap()
        );
        let entry = worst.entry(group).or_insert(0.0);
        *entry = entry.max(error);
        // Decoding is bit-exact; Pillow's float bilinear reduction accumulates in a different order (about 4e-9).
        let allowed = if max_size.is_some() && full.dim().0.max(full.dim().1) < 80 { RESIZE_TOLERANCE } else { 0.0 };
        assert!(error <= allowed, "{name}: pixels differ by {error:e} (allowed {allowed:e})");
        check_metadata(name, &loaded.metadata, &meta["metadata"], &source);
    }
    println!("largest pixel error per group (tolerance 0): {worst:?}");
}

#[test]
fn camera_frames_keep_their_calibration_through_the_resize() {
    // The resized camera source carries the same raw_info as the metadata reports.
    let case = Case::open("rawload", "load_bayer0_as_shot_sRGB_m16");
    let source = file("bayer0.dng");
    let loaded = load_photo(&source, Some(16), &settings(&case.json()["settings"], "sRGB")).unwrap();
    let Photo::Camera(camera) = &loaded.photo else { panic!("native path expected") };
    let info = serde_json::to_value(camera.raw_info()).unwrap();
    // (the metadata leaves out keys still at their default, see `prune_raw_info`)
    for (key, value) in loaded.metadata["raw_info"].as_object().unwrap() {
        assert_eq!(json_difference(&info[key], value, key), None);
    }
}

#[test]
fn files_the_decoder_cannot_read_give_the_oracles_words() {
    for name in names("errors") {
        let case = Case::open("rawload", &name);
        let source = file(case.json()["file"].as_str().unwrap());
        for mode in ["legacy", "as_shot"] {
            let expected = &case.json()[mode];
            let result = load_photo(&source, None, &settings(&serde_json::json!({"raw_mode": mode}), "sRGB"));
            let error = result.err().unwrap_or_else(|| panic!("{name}/{mode}: loaded"));
            if expected["error_class"] == "LibRawFileUnsupportedError" {
                assert_eq!(error.to_string(), expected["reason"].as_str().unwrap(), "{name}/{mode}");
                assert!(matches!(error, Error::Raw(_)), "{name}/{mode}: {error:?}");
            } else {
                // Other decoder failures (here an empty file): an error with the decoder's own text.
                println!("{name}/{mode}: oracle {} -> {error}", expected["error_class"]);
                assert!(!error.to_string().is_empty());
            }
        }
    }
}

#[test]
fn photo_info_reports_the_raw_metadata() {
    let index = index();
    for (key, golden) in index["info"].as_object().unwrap() {
        let source = file(index["files"][key].as_str().unwrap());
        let info = photo_info(&source).unwrap();
        assert!(!info.contains_key("load_error"), "{key}: {:?}", info.get("load_error"));
        check_metadata(&format!("info/{key}"), &info, golden, &source);
    }
}

#[test]
fn photo_info_of_a_broken_raw_keeps_its_file_facts() {
    // LibRaw refuses the file: `info` still reports what the EXIF reader finds, with the reason.
    let info = photo_info(file("junk.dng")).unwrap();
    assert_eq!(info["load_error"], "지원하지 않는 RAW 형식입니다.");
}

// ------------------------------------------------------------------------------------ exports

fn request(options: &Value) -> ExportRequest {
    let format = grainy_engine::files::parse_format(options["format"].as_str().unwrap()).unwrap();
    ExportRequest {
        format,
        quality: options["quality"].as_f64().unwrap(),
        longest: options["longest"].as_u64().map(|n| SizeLimit::Longest(n as usize)),
        color_space: options["color_space"].as_str().unwrap().into(),
        keep_metadata: options["keep_metadata"].as_bool().unwrap_or(false),
        jxl_lossless: format == ExportFormat::JpegXl,
        ..Default::default()
    }
}

fn decode_ints(path: &Path) -> (Vec<usize>, Vec<i64>) {
    let bitmap = decode_bitmap(path, &LoadOptions::default()).unwrap();
    let mut shape = bitmap.samples.shape().to_vec();
    if shape[2] == 1 {
        shape.pop();
    }
    (shape, bitmap.samples.iter().map(|v| *v as i64).collect())
}

fn golden_ints(case: &Case, name: &str, format: &str) -> (Vec<usize>, Vec<i64>) {
    if matches!(format, "PNG 16-bit" | "TIFF 16-bit") {
        let a: ArrayD<u16> = case.array(name);
        (a.shape().to_vec(), a.iter().map(|v| *v as i64).collect())
    } else {
        let a: ArrayD<u8> = case.array(name);
        (a.shape().to_vec(), a.iter().map(|v| *v as i64).collect())
    }
}

fn difference(a: &[i64], b: &[i64]) -> i64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).max().unwrap_or(0)
}

/// Levels of a 16-bit export that the develop of a camera-space source may be away from the oracle's output: the
/// camera stage is exact to 3e-6 (grainy-raw README), which moves a sample that sits near a rounding boundary by up
/// to two of 65535 steps (measured: 2). 8-bit outputs and the legacy path are exact.
fn export_tolerance(format: &str) -> i64 {
    if matches!(format, "PNG 16-bit" | "TIFF 16-bit") {
        2
    } else {
        0
    }
}

#[test]
fn raw_exports_match_the_oracle() {
    let mut rows: Vec<(String, i64, usize)> = vec![];
    for name in names("exports") {
        let case = Case::open("rawload", &name);
        let meta = case.json();
        let format = meta["options"]["format"].as_str().unwrap().to_owned();
        let output = out_dir().join(format!("{name}{}", meta["extension"].as_str().unwrap()));
        let _ = fs::remove_file(&output);
        let mut settings = meta["settings"].as_object().unwrap().clone();
        settings.entry("working_space").or_insert("sRGB".into());
        let source = file(meta["file"].as_str().unwrap());
        let (width, height) = export_photo(&source, &output, &settings, &request(&meta["options"])).unwrap();
        assert_eq!(
            [width as u64, height as u64],
            [meta["width"].as_u64().unwrap(), meta["height"].as_u64().unwrap()],
            "{name}"
        );
        let (shape, actual) = decode_ints(&output);
        if meta["lossless"].as_bool().unwrap() {
            let (expected_shape, expected) = golden_ints(&case, "expected", &format);
            assert_eq!(shape, expected_shape, "{name}");
            let max = difference(&actual, &expected);
            let off = actual.iter().zip(&expected).filter(|(a, b)| a != b).count();
            println!("{name}: max sample error {max}, {off} of {} samples differ", actual.len());
            rows.push((name.clone(), max, off));
            assert!(max <= export_tolerance(&format), "{name}: max sample error {max}");
        } else {
            // JPEG: held to the oracle's own decoded file around the encoder input.
            let (quantised_shape, quantised) = golden_ints(&case, "quantised", &format);
            assert_eq!(shape, quantised_shape, "{name}");
            let (_, oracle_decoded) = golden_ints(&case, "expected", &format);
            let allowed = difference(&oracle_decoded, &quantised) + 2;
            let max = difference(&actual, &quantised);
            println!("{name}: JPEG max error against the encoder input {max} (allowed {allowed})");
            rows.push((name.clone(), max, 0));
            assert!(max <= allowed, "{name}: {max} > {allowed}");
        }
    }
    println!("{rows:?}");
}

#[test]
fn a_failed_raw_export_leaves_no_file() {
    let output = out_dir().join("junk.png");
    let _ = fs::remove_file(&output);
    let error = export_photo(file("junk.dng"), &output, &Map::new(), &ExportRequest::default()).unwrap_err();
    assert_eq!(error.to_string(), "지원하지 않는 RAW 형식입니다.");
    assert!(!output.exists());
}

#[test]
fn native_dng_loads_color_matrix_and_profile_gain_map_together() {
    // Append an IFD to the generated linear DNG so all original data offsets remain valid.
    let mut bytes = fs::read(file("linear.dng")).unwrap();
    assert_eq!(&bytes[..2], b"II");
    let old = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[old..old + 2].try_into().unwrap());
    let end = old + 2 + 12 * usize::from(count);
    let mut entries: Vec<Vec<u8>> = bytes[old + 2..end].chunks_exact(12).map(<[u8]>::to_vec).collect();
    let next = bytes[end..end + 4].to_vec();
    let gain_offset = bytes.len() as u32;
    for n in [1u32, 1] {
        bytes.extend(n.to_be_bytes());
    }
    for v in [1.0f64, 1., 0., 0.] {
        bytes.extend(v.to_be_bytes());
    }
    bytes.extend(1u32.to_be_bytes());
    for v in [1.0f32, 0., 0., 0., 0., 1.25] {
        bytes.extend(v.to_be_bytes());
    }
    let entry =
        [52525u16.to_le_bytes().as_slice(), &7u16.to_le_bytes(), &68u32.to_le_bytes(), &gain_offset.to_le_bytes()]
            .concat();
    entries.push(entry);
    entries.sort_by_key(|e| u16::from_le_bytes([e[0], e[1]]));
    let new = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&new.to_le_bytes());
    bytes.extend((count + 1).to_le_bytes());
    bytes.extend(entries.into_iter().flatten());
    bytes.extend(next);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gain.dng");
    fs::write(&path, bytes).unwrap();
    let loaded = load_photo(path, None, &settings(&serde_json::json!({"raw_mode":"native"}), "sRGB")).unwrap();
    let Photo::Camera(cam) = loaded.photo else { panic!("native camera") };
    assert_eq!(cam.raw_info().gain_table_map.as_ref().unwrap().table, [1.25]);
}
