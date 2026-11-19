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
