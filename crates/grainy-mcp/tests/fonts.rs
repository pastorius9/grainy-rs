//! The contact sheet's caption font against Pillow's layout (golden/mcp/fonts.json): the font file, the size metrics
//! and the advance widths that decide where captions are clipped, plus `photo_images.mark`.

use grainy_mcp::images::mark;
use grainy_mcp::text::{font_path, Font};
use serde_json::Value;

fn golden() -> Value {
    let path = grainy_testkit::root().join("golden").join("mcp").join("fonts.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn marks_match_the_oracle() {
    for case in golden()["marks"].as_array().unwrap() {
        let text = mark(case[0].as_i64().unwrap(), case[1].as_f64().unwrap(), case[2].as_i64().unwrap());
        assert_eq!(text, case[3].as_str().unwrap());
    }
}

#[test]
fn caption_layout_matches_pillow() {
    let golden = golden();
    let samples: Vec<&str> = golden["samples"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    let Some(path) = font_path() else {
        eprintln!("no caption font on this machine: captions are not drawn");
        return;
    };
    let mut worst = 0.0f64;
    for case in golden["cases"].as_array().unwrap() {
        let expected_path = case["path"].as_str().unwrap_or("");
        let same_file = path.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(expected_path);
        if !same_file {
            eprintln!("the oracle used {expected_path}, this machine has {}: layout not compared", path.display());
            return;
        }
        let size = case["size"].as_u64().unwrap() as u32;
        let font = Font::new(size).unwrap();
        let metrics = case["metrics"].as_array().unwrap();
        assert_eq!(
            font.metrics(),
            (metrics[0].as_i64().unwrap() as i32, metrics[1].as_i64().unwrap() as i32),
            "size {size}: ascent and descent"
        );
        for (text, length) in samples.iter().zip(case["lengths"].as_array().unwrap()) {
            let d = (font.length(text) - length.as_f64().unwrap()).abs();
            worst = worst.max(d);
        }
    }
    eprintln!("caption advance widths: largest difference from Pillow {worst} px");
    assert!(worst <= 2.0, "advance widths differ by {worst} px");
}

/// Pillow's own font (`ImageFont.load_default(size)`), the oracle's caption font where neither Malgun Gothic nor
/// DejaVu Sans is found (macOS): the same bytes, laid out as Pillow lays them out (golden/mcp/builtin_font.json).
#[test]
fn the_builtin_font_is_pillows_and_lays_out_as_pillow() {
    let path = grainy_testkit::root().join("golden").join("mcp").join("builtin_font.json");
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(grainy_mcp::text::BUILTIN).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(digest, golden["sha256"].as_str().unwrap(), "the bytes of Pillow's load_default");
    assert_eq!(grainy_mcp::text::BUILTIN.len() as u64, golden["bytes"].as_u64().unwrap());
    let samples: Vec<&str> = golden["samples"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    let mut worst = 0.0f64;
    for case in golden["cases"].as_array().unwrap() {
        let size = case["size"].as_u64().unwrap() as u32;
        let font = Font::builtin(size).expect("the built-in font parses");
        let metrics = case["metrics"].as_array().unwrap();
        assert_eq!(
            font.metrics(),
            (metrics[0].as_i64().unwrap() as i32, metrics[1].as_i64().unwrap() as i32),
            "size {size}: ascent and descent"
        );
        for (text, length) in samples.iter().zip(case["lengths"].as_array().unwrap()) {
            worst = worst.max((font.length(text) - length.as_f64().unwrap()).abs());
        }
    }
    // Pillow's own advances (assets/pillow-default-font-advances.json): every width equals Pillow's.
    eprintln!("built-in font advance widths: largest difference from Pillow {worst} px");
    assert_eq!(worst, 0.0, "advance widths differ by {worst} px");
    // A system without the candidate files draws with it (`font_path` then names no file).
    assert_eq!(grainy_mcp::text::uses_builtin(), grainy_mcp::text::font_path().is_none());
    assert!(Font::new(13).is_some(), "captions are always drawn");
}

/// Where Pillow's `truetype` looks for a bare font name on each system (the oracle's `photo_images._font` asks for
/// `DejaVuSans.ttf`).
#[test]
fn the_font_folders_are_pillows() {
    use grainy_mcp::text::{font_folders, FontSystem};
    use std::path::PathBuf;
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_owned())
    };
    let paths = |list: &[&str]| list.iter().map(PathBuf::from).collect::<Vec<_>>();
    assert_eq!(
        font_folders(FontSystem::Windows, &env(&[("WINDIR", r"C:\Windows")])),
        vec![PathBuf::from(r"C:\Windows").join("fonts")]
    );
    assert_eq!(font_folders(FontSystem::Windows, &env(&[])), Vec::<PathBuf>::new());
    assert_eq!(
        font_folders(FontSystem::MacOs, &env(&[("HOME", "/Users/kim")])),
        vec![
            PathBuf::from("/Library/Fonts"),
            PathBuf::from("/System/Library/Fonts"),
            PathBuf::from("/Users/kim").join("Library").join("Fonts")
        ]
    );
    assert_eq!(
        font_folders(FontSystem::Linux, &env(&[("HOME", "/home/kim")])),
        vec![
            PathBuf::from("/home/kim").join(".local").join("share").join("fonts"),
            PathBuf::from("/usr/local/share").join("fonts"),
            PathBuf::from("/usr/share").join("fonts"),
        ]
    );
    assert_eq!(
        font_folders(FontSystem::Linux, &env(&[("XDG_DATA_HOME", "/x"), ("XDG_DATA_DIRS", "/a:/b")])),
        paths(&["/x", "/a", "/b"]).into_iter().map(|p| p.join("fonts")).collect::<Vec<_>>()
    );
    assert_eq!(font_folders(FontSystem::Other, &env(&[("HOME", "/h")])), Vec::<PathBuf>::new());
}
