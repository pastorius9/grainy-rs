//! The settings model behind the Develop column: the slider table, panels, undo/redo, shortcuts parsing.

use grainy_app::adjustments::{spec, ADJUSTMENTS};
use grainy_app::panels::{self, PANELS};
use grainy_app::shortcuts::parse;
use grainy_app::state::EditState;
use grainy_core::defaults;
use serde_json::json;

#[test]
fn every_adjustment_key_exists_in_the_defaults() {
    let d = defaults();
    assert_eq!(ADJUSTMENTS.len(), 58);
    for a in &ADJUSTMENTS {
        assert!(d.0.contains_key(a.key), "{} is not a settings key", a.key);
        assert!(a.low < a.high && [1.0, 10.0, 100.0].contains(&a.scale), "{}", a.key);
        let reset = a.reset_value();
        assert!(reset >= a.low && reset <= a.high, "{} resets outside its range", a.key);
    }
    let mut keys: Vec<&str> = ADJUSTMENTS.iter().map(|a| a.key).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), 58, "keys are unique");
}

#[test]
fn every_adjustment_belongs_to_a_panel_or_is_always_on() {
    // Owned by a panel's switch, or one of the always-applied keys (negative, straighten).
    for a in &ADJUSTMENTS {
        let owned = panels::owner(a.key).is_some();
        assert!(
            owned || ["negative_density", "negative_range", "straighten", "raw_kelvin", "raw_tint"].contains(&a.key),
            "{}",
            a.key
        );
    }
}

#[test]
fn number_field_format_and_parse() {
    let exposure = spec("exposure").unwrap();
    assert_eq!(exposure.format(0.0), "+0.00");
    assert_eq!(exposure.format(-1.25), "-1.25");
    assert_eq!(exposure.parse("+7"), Some(5.0));
    assert_eq!(exposure.parse("0,333"), Some(0.33));
    assert_eq!(exposure.parse("abc"), None);
    let contrast = spec("contrast").unwrap();
    assert_eq!(contrast.format(12.0), "+12");
    assert_eq!(spec("raw_tint").unwrap().format(-3.5), "-3.5");
    // Oracle quirk kept: add_adjustment's default_value 0 is clamped by the slider.
    assert_eq!(spec("negative_range").unwrap().reset_value(), 3.0);
    assert_eq!(spec("grain_size").unwrap().reset_value(), 25.0);
    assert_eq!(spec("sharpen_radius").unwrap().reset_value(), 0.8);
    assert_eq!(spec("transform_scale").unwrap().reset_value(), 100.0);
}

#[test]
fn one_undo_step_per_commit() {
    let mut edit = EditState::default();
    for v in [0.1, 0.2, 0.3] {
        edit.set("exposure", v); // a drag: live values, no commit
    }
    assert!(edit.commit("보정 변경"));
    assert!(!edit.commit("보정 변경"), "nothing new to commit");
    edit.set("contrast", 10.0);
    edit.commit("보정 변경");
    assert_eq!(edit.depth(), (2, 0));
    assert!(edit.undo());
    assert_eq!(edit.settings().f64("contrast"), 0.0);
    assert_eq!(edit.settings().f64("exposure"), 0.3);
    assert!(edit.undo());
    assert_eq!(edit.settings().f64("exposure"), 0.0);
    assert!(!edit.undo());
    assert!(edit.redo());
    assert_eq!(edit.settings().f64("exposure"), 0.3);
    assert_eq!(edit.depth(), (1, 1));
}

#[test]
fn panel_switch_dot_and_switch_on() {
    let mut edit = EditState::default();
    assert!(!panels::edited(edit.settings(), "effects"));
    edit.set("vignette", 20.0);
    edit.commit("보정 변경");
    assert!(panels::edited(edit.settings(), "effects"));
    edit.switch_panel("effects", false);
    assert!(!panels::is_on(edit.settings(), "effects"));
    assert_eq!(edit.settings().value("panels_off"), &json!(["effects"]));
    assert_eq!(edit.history()[0], "효과 끄기");
    // An edit in a panel that is off switches it on (panel_keys.switch_on).
    edit.set("grain", 10.0);
    assert!(panels::is_on(edit.settings(), "effects"));
    // The colour filter's switch is its own key.
    edit.set("photo_filter_density", 40.0);
    assert!(edit.settings().bool("photo_filter_enabled"));
    // A value back at its default (0 vs 0.0) is not an edit.
    edit.set("vignette", 0);
    edit.set("grain", 0.0);
    assert!(!panels::edited(edit.settings(), "effects"));
}

#[test]
fn panel_table_matches_the_oracle() {
    assert_eq!(PANELS.len(), 15);
    for (key, _) in PANELS {
        if !["crop", "metadata"].contains(&key) {
            assert!(panels::has_switch(key), "{key}");
        }
    }
}

#[test]
fn every_shortcut_of_the_table_parses() {
    for row in &grainy_i18n::shortcuts::TABLE {
        for key in row.keys {
            assert!(parse(key).is_some(), "{key}");
        }
    }
    let p = parse("Ctrl+Shift+Z").unwrap();
    assert!(p.command && p.shift && !p.alt);
}

#[test]
fn drt_look_names_the_values() {
    let mut edit = EditState::default();
    assert_eq!(grainy_app::drt_look(edit.settings()), "표준");
    for (key, v) in ["drt_contrast", "drt_toe", "drt_shoulder", "drt_white"].iter().zip([15.0, 35.0, -20.0, -45.0]) {
        edit.set(key, v);
    }
    assert_eq!(grainy_app::drt_look(edit.settings()), "필름처럼");
    edit.set("drt_toe", 1.0);
    assert_eq!(grainy_app::drt_look(edit.settings()), "");
}

#[test]
fn aspect_crops_are_centred_and_largest() {
    let crop = grainy_app::centred_crop(6000.0, 4000.0, 1.0).unwrap();
    assert!((crop[0] - 1.0 / 6.0).abs() < 1e-12 && crop[1] == 0.0 && crop[3] == 1.0);
    let crop = grainy_app::centred_crop(4000.0, 6000.0, 1.5).unwrap();
    assert!(crop[0] == 0.0 && crop[2] == 1.0);
    let height = (crop[3] - crop[1]) * 6000.0;
    assert!((4000.0 / height - 1.5).abs() < 1e-9);
    assert_eq!(grainy_app::centred_crop(0.0, 10.0, 1.0), None);
}
