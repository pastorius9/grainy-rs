//! `normalized()`, `effective()`, `white_balance()` and `tone_white()` against the oracle (tools/golden/gen_core.py).

use grainy_core::{defaults, effective, normalized, tone_white, white_balance, Settings};
use grainy_testkit::Case;
use serde_json::Value;

#[test]
fn defaults_are_the_oracles() {
    let d = defaults();
    assert_eq!(d.0.len(), 106);
    assert_eq!(d.f32("grain_size"), 25.0);
    assert_eq!(d.i64("tone_version"), 3);
    assert_eq!(d.str("tone_mapping"), "classic");
    assert!(d.is_none("crop") && !d.bool("negative") && !d.bool("masks"));
    assert_eq!(d.f32("not a key"), 0.0); // unknown keys read as nothing
}

#[test]
fn normalized_and_effective_match_the_oracle() {
    let case = Case::open("core", "normalized");
    let cases = case.json()["cases"].as_object().expect("cases");
    assert!(cases.len() >= 12);
    for (name, entry) in cases {
        let stored = entry["stored"].as_object().expect("stored");
        let normal = normalized(stored);
        assert_eq!(Value::Object(normal.0.clone()), entry["normalized"], "normalized: {name}");
        assert_eq!(Value::Object(effective(&normal).0), entry["effective"], "effective: {name}");
    }
}

#[test]
fn white_balance_and_tone_white_match_the_oracle() {
    let case = Case::open("core", "white_balance");
    for entry in case.json()["cases"].as_array().expect("cases") {
        let settings = Settings::from_value(entry["settings"].clone()).expect("settings");
        let gain = white_balance(&settings);
        let expected: Vec<f32> =
            entry["gain"].as_array().expect("gain").iter().map(|x| x.as_f64().unwrap() as f32).collect();
        for c in 0..3 {
            assert!(
                (gain[c] - expected[c]).abs() <= 2e-7 * expected[c].abs().max(1.0),
                "gain {c} of {}: {} vs {}",
                entry["settings"],
                gain[c],
                expected[c]
            );
        }
        let white = entry["tone_white"].as_f64().expect("tone_white");
        assert!(
            (tone_white(&settings) - white).abs() <= 1e-6 * white.max(1.0),
            "tone_white of {}: {} vs {white}",
            entry["settings"],
            tone_white(&settings)
        );
    }
}
