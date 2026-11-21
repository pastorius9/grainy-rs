//! Whole exports against the oracle's `engine.export_image` (tools/golden/gen_cli.py, golden/cli/), through
//! `grainy_engine::files::export_photo` and, for some cases, through the built `grainy-cli` binary.
//!
//! Lossless formats (PNG, PNG 16-bit, TIFF 8/16-bit) are compared sample for sample with the decoded pixels of the
//! oracle's file (tolerance 0 where the develop is bit-exact, see `TOLERANCE`). JPEG is compared after decoding
//! with a stated tolerance. AVIF and JPEG XL cannot be decoded here: `gen_cli.py --check` decodes the files this
//! test leaves in `test-output/exports` with Pillow and imagecodecs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use grainy_codecs::{decode_bitmap, read_metadata, ExportFormat, LoadOptions, Metadata, SizeLimit};
use grainy_engine::files::{export_photo, parse_format, Error, ExportRequest};
use grainy_testkit::Case;
use ndarray::ArrayD;
use serde_json::{Map, Value};

fn golden() -> PathBuf {
    grainy_testkit::root().join("golden").join("cli")
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-output").join("exports");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-output").join("scratch").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn source(name: &str) -> PathBuf {
    golden().join("_sources").join(name)
}

fn case_names() -> Vec<String> {
    let text = fs::read_to_string(golden().join("_index.json")).unwrap();
    let index: Value = serde_json::from_str(&text).unwrap();
    index["cases"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_owned()).collect()
}

fn request(options: &Value) -> ExportRequest {
    let format = parse_format(options["format"].as_str().unwrap()).unwrap();
    let longest = match &options["longest"] {
        Value::Null => None,
        Value::Number(n) => Some(SizeLimit::Longest(n.as_u64().unwrap() as usize)),
        Value::Array(a) => Some(SizeLimit::Box {
            width: a[0].as_u64().unwrap() as usize,
            height: a[1].as_u64().unwrap() as usize,
            cover: a[2] == "cover",
        }),
        other => panic!("longest {other}"),
    };
    ExportRequest {
        format,
        quality: options["quality"].as_f64().unwrap(),
        longest,
        color_space: options["color_space"].as_str().unwrap().into(),
        keep_metadata: options["keep_metadata"].as_bool().unwrap(),
        user_metadata: options["user_metadata"].as_object().cloned().unwrap_or_default() as Metadata,
        keywords: options["keywords"].as_str().unwrap().into(),
        grayscale: options["grayscale"].as_bool().unwrap(),
        dither: options["dither"].as_bool().unwrap(),
        jxl_lossless: format == ExportFormat::JpegXl,
    }
}

fn settings(case: &Case) -> Map<String, Value> {
    case.json()["settings"].as_object().unwrap().clone()
}

/// Samples of an image file as `(shape without a trailing 1, integers)`.
fn decode_ints(path: &Path) -> (Vec<usize>, Vec<i64>) {
    let bitmap = decode_bitmap(path, &LoadOptions::default()).unwrap();
    let mut shape = bitmap.samples.shape().to_vec();
    if shape[2] == 1 {
        shape.pop();
    }
    (shape, bitmap.samples.iter().map(|v| *v as i64).collect())
}

fn is_16bit(format: &str) -> bool {
    matches!(format, "PNG 16-bit" | "TIFF 16-bit" | "JPEG XL")
}

fn golden_ints(case: &Case, name: &str, format: &str) -> (Vec<usize>, Vec<i64>) {
    if is_16bit(format) {
        let a: ArrayD<u16> = case.array(name);
        (a.shape().to_vec(), a.iter().map(|v| *v as i64).collect())
    } else {
        let a: ArrayD<u8> = case.array(name);
        (a.shape().to_vec(), a.iter().map(|v| *v as i64).collect())
    }
}

/// (largest absolute difference, mean absolute difference)
fn difference(a: &[i64], b: &[i64]) -> (i64, f64) {
    let diffs = a.iter().zip(b).map(|(x, y)| (x - y).abs());
    (diffs.clone().max().unwrap_or(0), diffs.sum::<i64>() as f64 / a.len().max(1) as f64)
}

// ------------------------------------------------------------------------------------ container blocks

fn png_chunks(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut pos = 8;
    let mut out = vec![];
    while pos + 12 <= data.len() {
        let n = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        out.push((data[pos + 4..pos + 8].try_into().unwrap(), &data[pos + 8..pos + 8 + n]));
        pos += 12 + n;
    }
    out
}

fn jpeg_segments(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut pos = 2;
    let mut out = vec![];
    while pos + 4 <= data.len() && data[pos] == 0xFF {
        let marker = data[pos + 1];
        let n = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        if marker == 0xDA {
            break;
        }
        out.push((marker, &data[pos + 4..pos + 2 + n]));
        pos += 2 + n;
    }
    out
}

fn tiff_tag(data: &[u8], tag: u16) -> Option<Vec<u8>> {
    let le = data.starts_with(b"II");
    let u16_at = |p: usize| {
        if le {
            u16::from_le_bytes([data[p], data[p + 1]])
        } else {
            u16::from_be_bytes([data[p], data[p + 1]])
        }
    };
    let u32_at = |p: usize| {
        let b: [u8; 4] = data[p..p + 4].try_into().unwrap();
        if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    let root = u32_at(4) as usize;
    (0..u16_at(root) as usize).find_map(|i| {
        let at = root + 2 + i * 12;
        if u16_at(at) != tag {
            return None;
        }
        let count = u32_at(at + 4) as usize;
        Some(if count <= 4 {
            data[at + 8..at + 8 + count].to_vec()
        } else {
            data[u32_at(at + 8) as usize..][..count].to_vec()
        })
    })
}

#[derive(Default, Debug)]
struct Blocks {
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
    iptc: Option<Vec<u8>>,
}

fn found_blocks(path: &Path) -> Blocks {
    use std::io::Read;
    let data = fs::read(path).unwrap();
    let mut blocks = Blocks::default();
    match path.extension().unwrap().to_str().unwrap() {
        "png" => {
            for (kind, payload) in png_chunks(&data) {
                match &kind {
                    b"iCCP" => {
                        let n = payload.iter().position(|b| *b == 0).unwrap();
                        let mut out = vec![];
                        flate2::read::ZlibDecoder::new(&payload[n + 2..]).read_to_end(&mut out).unwrap();
                        blocks.icc = Some(out);
                    }
                    b"eXIf" => blocks.exif = Some(payload.strip_prefix(b"Exif\0\0").unwrap_or(payload).to_vec()),
                    b"iTXt" if payload.starts_with(b"XML:com.adobe.xmp") => {
                        blocks.xmp = Some(payload[b"XML:com.adobe.xmp".len() + 5..].to_vec())
                    }
                    _ => {}
                }
            }
        }
        "jpg" => {
            let mut parts: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
            for (marker, payload) in jpeg_segments(&data) {
                if marker == 0xE2 && payload.starts_with(b"ICC_PROFILE\0") {
                    parts.insert(payload[12], payload[14..].to_vec());
                } else if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
                    blocks.exif = Some(payload[6..].to_vec());
                } else if marker == 0xED {
                    blocks.iptc = Some(payload.to_vec());
                }
            }
            if !parts.is_empty() {
                blocks.icc = Some(parts.into_values().flatten().collect());
            }
        }
        "tif" => {
            blocks.icc = tiff_tag(&data, 34675);
            blocks.xmp = tiff_tag(&data, 700);
            blocks.iptc = tiff_tag(&data, 33723);
        }
        _ => {}
    }
    blocks
}

/// ICC bytes equal except the creation time (bytes 24..36), the one thing that differs between two runs. The
/// output profiles are made by LittleCMS, whose header names the platform (`APPL` on macOS, `MSFT` on Windows).
fn icc_equal(actual: &[u8], expected: &[u8]) -> bool {
    let expected = grainy_testkit::lcms_created(expected);
    actual.len() == expected.len() && actual[..24] == expected[..24] && actual[36..] == expected[36..]
}

// ------------------------------------------------------------------------------------ comparison

/// How far (in integer samples) a case may be from the oracle, and why. Zero wherever the develop is bit-exact.
/// Elsewhere than with the goldens' C runtime (macOS), `powf`, `expf`, `cbrtf`, ... round some intermediate values
/// differently in the last place: 16-bit samples then differ by a few levels (measured on macOS, CI runs 1 and 5:
/// at most 3 of 65535, in `cs_sRGB_wide_tiff16` and `meta_rich_p3_tiff16`; tolerance 4, the next level up);
/// 8-bit samples did not differ (tolerance 0).
fn tolerance(name: &str, case: &Case, format: &str) -> i64 {
    let _ = name;
    let base = if case.json()["exact"] == false {
        1 // luminance noise reduction and sized grain are not bit-exact in the engine (1e-6 level)
    } else {
        0
    };
    grainy_testkit::maths_tolerance(base, if is_16bit(format) { base.max(4) } else { base })
}

struct Row {
    name: String,
    max: i64,
    mean: f64,
    note: String,
    sixteen: bool,
}

fn check_blocks(name: &str, case: &Case, path: &Path, failures: &mut Vec<String>) {
    if matches!(path.extension().and_then(|e| e.to_str()), Some("avif" | "jxl")) {
        return; // containers read by gen_cli.py --check
    }
    let found = &case.json()["found"];
    let actual = found_blocks(path);
    if let Some(digest) = found["icc"].as_str() {
        let expected = fs::read(golden().join("_icc").join(format!("{digest}.icc"))).unwrap();
        match &actual.icc {
            Some(icc) if icc_equal(icc, &expected) => {}
            Some(icc) => failures.push(format!("{name}: ICC differs ({} vs {} bytes)", icc.len(), expected.len())),
            None => failures.push(format!("{name}: no ICC profile in the file")),
        }
    }
    let folder = golden().join(name);
    for (key, actual) in [("exif", &actual.exif), ("xmp", &actual.xmp), ("iptc", &actual.iptc)] {
        let tiff = path.extension().unwrap() == "tif";
        if key == "exif" && tiff {
            continue; // compared through read_metadata below
        }
        if found[key].as_bool() == Some(true) {
            let expected = fs::read(folder.join(format!("{key}.bin"))).unwrap();
            if actual.as_deref() != Some(expected.as_slice()) {
                failures.push(format!("{name}: {key} block differs"));
            }
        } else if actual.is_some() && key != "iptc" {
            failures.push(format!("{name}: unexpected {key} block"));
        }
    }
    if let Some(expected) = case.json().get("read_metadata") {
        let got = read_metadata(path).unwrap();
        if &Value::Object(got.clone()) != expected {
            failures.push(format!("{name}: read_metadata differs: {got:?} vs {expected}"));
        }
    }
}

#[test]
fn exports_match_the_oracle() {
    let mut rows: Vec<Row> = vec![];
    let mut failures: Vec<String> = vec![];
    let mut manifest: Vec<Value> = vec![];
    let exports = out_dir();
    let names = case_names();
    assert!(names.len() >= 100, "only {} cases", names.len());
    for name in &names {
        if name.starts_with("src_jpeg") {
            continue; // exact JPEG-source comparisons have their own test
        }
        let case = Case::open("cli", name);
        let options = &case.json()["options"];
        let format = options["format"].as_str().unwrap().to_owned();
        let extension = case.json()["extension"].as_str().unwrap();
        let output = exports.join(format!("{name}{extension}"));
        let _ = fs::remove_file(&output);
        let result =
            export_photo(source(case.json()["source"].as_str().unwrap()), &output, &settings(&case), &request(options));
        if let Some(expected) = case.json()["error"].as_str() {
            match result {
                Err(error) if error.to_string() == expected && !output.exists() => rows.push(Row {
                    name: name.clone(),
                    max: 0,
                    mean: 0.0,
                    note: "refused with the oracle's text".into(),
                    sixteen: false,
                }),
                other => failures.push(format!("{name}: expected the oracle's refusal {expected:?}, got {other:?}")),
            }
            continue;
        }
        let (width, height) = match result {
            Ok(size) => size,
            Err(error) => {
                failures.push(format!("{name}: {error}"));
                continue;
            }
        };
        if [width, height]
            != [case.json()["width"].as_u64().unwrap() as usize, case.json()["height"].as_u64().unwrap() as usize]
        {
            failures.push(format!("{name}: size {width}x{height}"));
        }
        check_blocks(name, &case, &output, &mut failures);
        match case.json()["lossless"].as_bool().unwrap() {
            true => {
                let (shape, actual) = decode_ints(&output);
                let (expected_shape, expected) = golden_ints(&case, "expected", &format);
                if shape != expected_shape {
                    failures.push(format!("{name}: shape {shape:?} vs {expected_shape:?}"));
                    continue;
                }
                let (max, mean) = difference(&actual, &expected);
                let allowed = tolerance(name, &case, &format);
                if max > allowed {
                    failures.push(format!("{name}: max sample error {max} above {allowed}"));
                }
                let note = format!("tolerance {allowed}");
                rows.push(Row { name: name.clone(), max, mean, note, sixteen: is_16bit(&format) });
            }
            false => {
                manifest.push(serde_json::json!({"case": name, "file": output.file_name().unwrap().to_str().unwrap()}));
                if format == "JPEG" {
                    let (shape, actual) = decode_ints(&output);
                    let (expected_shape, expected) = golden_ints(&case, "expected", &format);
                    if shape != expected_shape {
                        failures.push(format!("{name}: shape {shape:?} vs {expected_shape:?}"));
                        continue;
                    }
                    let (max, mean) = difference(&actual, &expected);
                    if max != 0 {
                        failures.push(format!("{name}: JPEG sample error {max}; expected exact Pillow pixels"));
                    }
                    rows.push(Row {
                        name: name.clone(),
                        max,
                        mean,
                        note: "JPEG vs Pillow decoded output; tolerance 0".into(),
                        sixteen: false,
                    });
                } else {
                    rows.push(Row {
                        name: name.clone(),
                        max: -1,
                        mean: 0.0,
                        note: format!("{format}: blocks here, pixels by gen_cli.py --check"),
                        sixteen: false,
                    });
                }
            }
        }
    }
    fs::write(exports.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    let mut worst: BTreeMap<String, i64> = BTreeMap::new();
    for row in &rows {
        println!("{:<34} max {:>3} mean {:.4}  {}", row.name, row.max, row.mean, row.note);
        let group = row.name.split('_').next().unwrap().to_owned();
        let entry = worst.entry(group).or_insert(0);
        *entry = (*entry).max(row.max);
    }
    println!("largest sample error per group: {worst:?}");
    let sixteen = rows.iter().filter(|r| r.note.starts_with("tolerance") && r.sixteen).map(|r| r.max).max();
    let eight = rows.iter().filter(|r| r.note.starts_with("tolerance") && !r.sixteen).map(|r| r.max).max();
    grainy_testkit::report_maths("cli 16-bit export samples (levels)", sixteen.unwrap_or(0) as f64, 4.0);
    grainy_testkit::report_maths("cli 8-bit export samples (levels)", eight.unwrap_or(0) as f64, 0.0);
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

// ------------------------------------------------------------------------------------ the JPEG source

#[test]
fn jpeg_source_matches_pillow_exactly() {
    for name in ["src_jpeg_png16", "src_jpeg_tiff8_mix"] {
        let case = Case::open("cli", name);
        let output = out_dir().join(format!("{name}{}", case.json()["extension"].as_str().unwrap()));
        let _ = fs::remove_file(&output);
        export_photo(source("jpeg.jpg"), &output, &settings(&case), &request(&case.json()["options"])).unwrap();
        let format = case.json()["options"]["format"].as_str().unwrap();
        let (shape, actual) = decode_ints(&output);
        let (expected_shape, expected) = golden_ints(&case, "expected", format);
        assert_eq!(shape, expected_shape);
        let (max, mean) = difference(&actual, &expected);
        println!("{name}: max {max} mean {mean:.3} (tolerance 0)");
        assert_eq!(max, 0, "{name}: JPEG source pixels must equal Pillow");
    }
}

// ------------------------------------------------------------------------------------ refusals

fn asserts_no_file(path: &Path) {
    assert!(!path.exists(), "{} was created", path.display());
}

#[test]
fn the_source_is_never_overwritten() {
    let dir = scratch("same_file");
    let copy = dir.join("photo.png");
    fs::copy(source("png8.png"), &copy).unwrap();
    let before = fs::read(&copy).unwrap();
    let spelled = dir.join(".").join("photo.png");
    for output in [&copy, &spelled] {
        let error = export_photo(
            &copy,
            output,
            &Map::new(),
            &ExportRequest { format: ExportFormat::Png, ..Default::default() },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "원본 파일에는 덮어쓸 수 없습니다.");
    }
    assert_eq!(fs::read(&copy).unwrap(), before);
}

#[test]
fn an_existing_output_is_not_overwritten() {
    let dir = scratch("existing");
    let output = dir.join("out.png");
    fs::write(&output, b"keep me").unwrap();
    let error = export_photo(
        source("png8.png"),
        &output,
        &Map::new(),
        &ExportRequest { format: ExportFormat::Png, ..Default::default() },
    )
    .unwrap_err();
    assert!(matches!(&error, Error::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists), "{error:?}");
    assert_eq!(fs::read(&output).unwrap(), b"keep me");
}

#[test]
fn a_develop_that_fails_leaves_no_file() {
    let dir = scratch("pending");
    let cases = [(
        "lens",
        serde_json::json!({"lensfun_enabled": true, "lensfun": {"crop": 1.0, "focal": 50.0, "xml": "<lens/>"}}),
    )];
    for (name, settings) in cases {
        let output = dir.join(format!("{name}.png"));
        let error = export_photo(
            source("png8.png"),
            &output,
            settings.as_object().unwrap(),
            &ExportRequest { format: ExportFormat::Png, ..Default::default() },
        )
        .unwrap_err();
        assert!(matches!(error, Error::Develop(grainy_engine::Error::Lens(_))), "{name}: {error:?}");
        assert!(error.to_string().contains("렌즈"), "{error}");
        println!("{name}: {error}");
        asserts_no_file(&output);
    }
}

#[test]
fn sources_that_cannot_be_read_yet_fail_with_a_message_and_no_file() {
    let dir = scratch("unsupported");
    let request = ExportRequest { format: ExportFormat::Png, ..Default::default() };
    // (Grey ICC profiles and PNG gAMA load since grainy-cms converts them: crates/grainy-engine/tests/cms_loading.rs.)
    // Video files are refused before they are read (RAW files are read: see tests/rawload.rs).
    {
        let (name, text) = ("a.mp4", "동영상 파일은 지원하지 않습니다.");
        let path = dir.join(name);
        fs::write(&path, b"not really").unwrap();
        let output = dir.join(format!("{name}.png"));
        let error = export_photo(&path, &output, &Map::new(), &request).unwrap_err();
        assert!(error.to_string().starts_with(text), "{error}");
        asserts_no_file(&output);
    }
    // Not an image at all.
    let path = dir.join("broken.png");
    fs::write(&path, b"nothing").unwrap();
    let output = dir.join("broken.out.png");
    assert!(export_photo(&path, &output, &Map::new(), &request).is_err());
    asserts_no_file(&output);
}

#[test]
fn a_failed_export_leaves_nothing_behind() {
    // JPEG XL needs the lossless opt-in: without it the message says why and no file appears.
    let dir = scratch("jxl_opt_in");
    let output = dir.join("out.jxl");
    let error = export_photo(
        source("png8.png"),
        &output,
        &Map::new(),
        &ExportRequest { format: ExportFormat::JpegXl, ..Default::default() },
    )
    .unwrap_err();
    assert!(error.to_string().contains("libjxl"), "{error}");
    asserts_no_file(&output);
}

// ------------------------------------------------------------------------------------ the binary

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_grainy-cli"))
}

#[test]
fn cli_hdr_round_trip_preserves_highlights_with_explicit_settings() {
    use grainy_codecs::{export_image, ExportOptions};
    use grainy_engine::files::Cms;

    let dir = scratch("hdr-cli");
    let input = dir.join("linear.tif");
    let settings = dir.join("hdr.json");
    fs::write(&settings, r#"{"hdr":true}"#).unwrap();
    let pixels = grainy_core::Image::from_shape_fn((32, 64, 3), |(_, x, _)| if x < 32 { 0.18 } else { 3.0 });
    let options = ExportOptions {
        format: ExportFormat::TiffHdr32,
        hdr_enabled: true,
        keep_metadata: false,
        ..Default::default()
    };
    export_image(&pixels, &input, &input, &options, &Cms::for_export_of_developed_pixels()).unwrap();
    let render = |from: &Path, to: &Path, format: &str, hdr: bool| {
        let mut command = cli();
        command.arg("render").arg(from).arg("-o").arg(to).args(["--format", format, "--quality", "100"]);
        if hdr {
            command.arg("--settings").arg(&settings);
        }
        command.output().unwrap()
    };
    let baseline = dir.join("baseline.tif");
    let avif = dir.join("hdr.avif");
    let decoded = dir.join("decoded.tif");
    assert!(!render(&input, &baseline, "TIFF HDR 32-bit", false).status.success());
    assert!(!baseline.exists());
    for (from, to, format) in [
        (&input, &baseline, "TIFF HDR 32-bit"),
        (&input, &avif, "AVIF HDR 10-bit"),
        (&avif, &decoded, "TIFF HDR 32-bit"),
    ] {
        let result = render(from, to, format, true);
        assert!(result.status.success(), "{format}: {}", String::from_utf8_lossy(&result.stderr));
    }
    let reference = decode_bitmap(&baseline, &LoadOptions::default()).unwrap();
    let roundtrip = decode_bitmap(&decoded, &LoadOptions::default()).unwrap();
    assert_eq!(roundtrip.bit_depth, 32);
    assert_eq!(roundtrip.samples.dim(), pixels.dim());
    assert!(roundtrip.samples[[16, 48, 0]] > 1.0);
    for x in [16, 48] {
        for c in 0..3 {
            assert!((reference.samples[[16, x, c]] - roundtrip.samples[[16, x, c]]).abs() < 0.10);
        }
    }
}

fn flags(case: &Case) -> Vec<String> {
    let o = &case.json()["options"];
    let mut args: Vec<String> =
        vec!["--format".into(), o["format"].as_str().unwrap().into(), "--quality".into(), o["quality"].to_string()];
    match &o["longest"] {
        Value::Null => {}
        Value::Number(n) => args.extend(["--longest".into(), n.to_string()]),
        Value::Array(a) => {
            args.extend(["--box".into(), format!("{}x{}", a[0], a[1]), "--fit".into(), a[2].as_str().unwrap().into()])
        }
        _ => unreachable!(),
    }
    args.extend(["--color-space".into(), o["color_space"].as_str().unwrap().into()]);
    for (key, flag) in [("keep_metadata", "--keep-metadata"), ("grayscale", "--grayscale"), ("dither", "--dither")] {
        if o[key] == true {
            args.push(flag.into());
        }
    }
    if !o["keywords"].as_str().unwrap().is_empty() {
        args.extend(["--keywords".into(), o["keywords"].as_str().unwrap().into()]);
    }
    args
}

#[test]
fn the_binary_renders_the_same_pixels() {
    let names = [
        "fmt_png16",
        "set_mix_png16_png16",
        "set_film_wide_tiff8",
        "size_box_cover",
        "gray_png",
        "cs_Display_P3_wide_tiff16",
        "dither_png",
        "meta_rich_tiff16",
        "meta_rich_png",
        "src_tiff16_p3_png16",
    ];
    let dir = scratch("binary");
    for name in names {
        let case = Case::open("cli", name);
        let o = &case.json()["options"];
        let format = o["format"].as_str().unwrap();
        let settings_file = dir.join(format!("{name}.json"));
        fs::write(&settings_file, Value::Object(settings(&case)).to_string()).unwrap();
        let mut command = cli();
        command.args(["render", source(case.json()["source"].as_str().unwrap()).to_str().unwrap(), "-o"]);
        let output = dir.join(format!("{name}{}", case.json()["extension"].as_str().unwrap()));
        command.arg(&output).arg("--settings").arg(&settings_file).args(flags(&case));
        if let Some(meta) = o["user_metadata"].as_object() {
            let file = dir.join(format!("{name}.metadata.json"));
            fs::write(&file, Value::Object(meta.clone()).to_string()).unwrap();
            command.arg("--metadata").arg(file);
        }
        let result = command.output().unwrap();
        assert!(result.status.success(), "{name}: {}", String::from_utf8_lossy(&result.stderr));
        let stdout = String::from_utf8_lossy(&result.stdout);
        let expected_line = format!("({}x{})", case.json()["width"], case.json()["height"]);
        assert!(stdout.contains(&expected_line), "{name}: {stdout}");
        let (shape, actual) = decode_ints(&output);
        let (expected_shape, expected) = golden_ints(&case, "expected", format);
        assert_eq!(shape, expected_shape, "{name}");
        let (max, _) = difference(&actual, &expected);
        assert!(max <= tolerance(name, &case, format), "{name}: max sample error {max}");
        let mut failures = vec![];
        check_blocks(name, &case, &output, &mut failures);
        assert!(failures.is_empty(), "{failures:?}");
        println!("{name}: binary output equals the oracle's pixels (max error 0)");
    }
}

#[test]
fn the_binary_reports_failures_on_one_line() {
    let dir = scratch("binary_errors");
    let png = source("png8.png");
    let existing = dir.join("exists.png");
    fs::write(&existing, b"x").unwrap();
    let crop = dir.join("lens.json");
    fs::write(&crop, r#"{"lensfun_enabled": true, "lensfun": {"crop": 1.0, "focal": 50.0, "xml": "<lens/>"}}"#)
        .unwrap();
    let cases: Vec<(Vec<String>, &str)> = vec![
        (vec!["render".into(), png.to_str().unwrap().into(), "-o".into(), existing.to_str().unwrap().into()], "exists"),
        (
            vec!["render".into(), png.to_str().unwrap().into(), "-o".into(), png.to_str().unwrap().into()],
            "원본 파일에는 덮어쓸 수 없습니다.",
        ),
        (
            vec![
                "render".into(),
                png.to_str().unwrap().into(),
                "-o".into(),
                dir.join("crop.png").to_str().unwrap().into(),
                "--settings".into(),
                crop.to_str().unwrap().into(),
            ],
            "렌즈 프로파일 하나가 필요합니다.",
        ),
        (vec!["render".into(), png.to_str().unwrap().into()], "needs -o"),
        (
            vec![
                "render".into(),
                png.to_str().unwrap().into(),
                "-o".into(),
                dir.join("x.bin").to_str().unwrap().into(),
            ],
            "cannot tell the format",
        ),
        (
            vec![
                "render".into(),
                png.to_str().unwrap().into(),
                "-o".into(),
                dir.join("x.png").to_str().unwrap().into(),
                "--format".into(),
                "BMP".into(),
            ],
            "unknown format",
        ),
        (
            vec![
                "render".into(),
                "-o".into(),
                dir.join("x.png").to_str().unwrap().into(),
                dir.join("missing.png").to_str().unwrap().into(),
            ],
            "missing.png",
        ),
        (vec!["frobnicate".into()], "unknown command"),
    ];
    for (args, expected) in cases {
        let result = cli().args(&args).output().unwrap();
        assert_eq!(result.status.code(), Some(1), "{args:?}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(stderr.trim_end().lines().count(), 1, "{args:?}: {stderr}");
        assert!(stderr.starts_with("grainy-cli: ") && stderr.contains(expected), "{args:?}: {stderr}");
        println!("{expected}: {}", stderr.trim_end());
    }
    assert!(!dir.join("crop.png").exists() && !dir.join("x.png").exists());
    assert_eq!(fs::read(&existing).unwrap(), b"x");
}

#[test]
fn info_prints_the_metadata_as_json() {
    let result = cli().args(["info", source("png8_exif.png").to_str().unwrap()]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let info: Value = serde_json::from_slice(&result.stdout).unwrap();
    // 40x60 source stored with orientation 6: shown as 40 wide and 60 high.
    assert_eq!((info["width"].as_u64(), info["height"].as_u64()), (Some(40), Some(60)));
    assert_eq!(info["format"], "PNG");
    assert_eq!(info["make"], "Camera Co");
    assert_eq!(info["source_orientation"], 6);
    let missing = cli().args(["info", "nowhere.png"]).output().unwrap();
    assert_eq!(missing.status.code(), Some(1));
}
