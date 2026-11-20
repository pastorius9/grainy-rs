//! `grainy-cli render` and `info` on RAW files against the oracle (golden/rawload, tools/golden/gen_rawload.py).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use grainy_codecs::{decode_bitmap, LoadOptions};
use grainy_testkit::Case;
use serde_json::Value;

fn golden() -> PathBuf {
    grainy_testkit::root().join("golden").join("rawload")
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-output").join("rawload").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_grainy-cli"))
}

#[test]
fn the_binary_renders_a_dng_with_the_oracles_pixels() {
    let dir = scratch("render");
    // (case, extra flags): a native-path 8-bit export with a mix of settings, a legacy-path 16-bit export of a rotated
    // file shrunk to 48 pixels, and a camera-space 16-bit export.
    for name in ["export_bayer2_as_shot_mix_tiff8", "export_bayer3_legacy_flip_longest", "export_bayer0_legacy_png16"] {
        let case = Case::open("rawload", name);
        let meta = case.json();
        let settings_file = dir.join(format!("{name}.json"));
        fs::write(&settings_file, meta["settings"].to_string()).unwrap();
        let output = dir.join(format!("{name}{}", meta["extension"].as_str().unwrap()));
        let mut command = cli();
        command
            .arg("render")
            .arg(golden().join("_files").join(meta["file"].as_str().unwrap()))
            .arg("-o")
            .arg(&output)
            .arg("--settings")
            .arg(&settings_file)
            .args(["--format", meta["options"]["format"].as_str().unwrap()]);
        if let Some(n) = meta["options"]["longest"].as_u64() {
            command.args(["--longest", &n.to_string()]);
        }
        let result = command.output().unwrap();
        assert!(result.status.success(), "{name}: {}", String::from_utf8_lossy(&result.stderr));
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains(&format!("({}x{})", meta["width"], meta["height"])), "{name}: {stdout}");
        let bitmap = decode_bitmap(&output, &LoadOptions::default()).unwrap();
        let actual: Vec<i64> = bitmap.samples.iter().map(|v| *v as i64).collect();
        let sixteen = meta["options"]["format"].as_str().unwrap().contains("16");
        let expected: Vec<i64> = if sixteen {
            case.array::<u16, ndarray::IxDyn>("expected").iter().map(|v| *v as i64).collect()
        } else {
            case.array::<u8, ndarray::IxDyn>("expected").iter().map(|v| *v as i64).collect()
        };
        assert_eq!(actual.len(), expected.len(), "{name}");
        let max = actual.iter().zip(&expected).map(|(a, b)| (a - b).abs()).max().unwrap();
        // Exact with the goldens' C runtime; elsewhere (macOS) `powf`, `expf`, ... round some intermediate values
        // differently in the last place, which moves a 16-bit sample by a level or so (run 1: 1 of 65535).
        let allowed = grainy_testkit::maths_tolerance(0, if sixteen { 4 } else { 0 });
        println!("{name}: binary output max sample error {max} (tolerance {allowed})");
        grainy_testkit::report_maths(&format!("cli raw render {name} (levels)"), max as f64, allowed as f64);
        assert!(max <= allowed, "{name}: max sample error {max}");
    }
}

#[test]
fn the_binary_prints_raw_info_as_json() {
    let source = golden().join("_files").join("bayer1.dng");
    let result = cli().arg("info").arg(&source).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let info: Value = serde_json::from_slice(&result.stdout).unwrap();
    let index: Value = serde_json::from_str(&fs::read_to_string(golden().join("_index.json")).unwrap()).unwrap();
    let expected = &index["info"]["bayer1"];
    for key in [
        "make",
        "camera",
        "format",
        "width",
        "height",
        "source_orientation",
        "decoder",
        "raw_native",
        "raw_as_shot_kelvin",
    ] {
        assert_eq!(info[key], expected[key], "{key}");
    }
    // The file is rotated by its orientation tag: LibRaw flip 3 is kept, flips 5 and 6 swap the shown size.
    assert_eq!(info["source_orientation"], 3);
    // `info` loads with the default settings, which develop a RAW file the legacy way.
    assert_eq!(info["raw_native"], false);
}

#[test]
fn the_binary_says_why_a_raw_file_is_refused() {
    let dir = scratch("refused");
    let output = dir.join("out.png");
    for (file, text) in [("junk.dng", "지원하지 않는 RAW 형식입니다."), ("junk.nef", "니콘의 고효율(HE) RAW")]
    {
        let result =
            cli().arg("render").arg(golden().join("_files").join(file)).arg("-o").arg(&output).output().unwrap();
        assert_eq!(result.status.code(), Some(1), "{file}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.starts_with("grainy-cli: ") && stderr.contains(text), "{file}: {stderr}");
        assert!(!output.exists());
    }
    // A video is still refused.
    let video = dir.join("clip.mp4");
    fs::write(&video, b"x").unwrap();
    let result = cli().arg("render").arg(&video).arg("-o").arg(&output).output().unwrap();
    assert!(String::from_utf8_lossy(&result.stderr).contains("동영상 파일은 지원하지 않습니다."));
}
