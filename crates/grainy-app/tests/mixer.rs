//! Work package 7: the colour mixer, its targeted adjustment and the point colours.
//!
//! * Model tests against goldens made with the oracle's own code (`tools/golden/gen_app_mixer.py`): luma/targeted.py
//!   `hsl_weights`, `drag_hsl` and a `TargetedAdjust('hsl')` session; luma/app.py `hsl_changed` / `update_hsl`;
//!   luma/studio.py `pick_point_color`, `load_points`, `load_point`, `point_changed`, `remove_point` with
//!   luma/point_color.py `sample`; `sample` and `preview` (the coverage overlay) of versions 1 and 2.
//! * The window headless (egui_kittest on wgpu): a pick by a click on the photo, the overlay, a targeted mixer drag,
//!   and snapshots in Korean and English (`tests/snapshots/mixer-*.png`).

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::panels::mixer::{hsl_changed, shown_hsl};
use grainy_app::state::EditState;
use grainy_app::tools::point_color::{self, PointColors, CONTROLS};
use grainy_app::tools::targeted::{self, Kind, TargetedAdjust};
use grainy_app::GrainyApp;
use grainy_core::{normalized, Settings};
use grainy_engine::Source;
use grainy_i18n::Language;
use grainy_testkit::Case;
use ndarray::{Array2, Array3};
use serde_json::{json, Map, Value};

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("a number: {v}"))
}

fn settings_with(changes: &Value) -> Settings {
    let mut map = grainy_core::defaults().0.clone();
    for (k, v) in changes.as_object().expect("changes") {
        map.insert(k.clone(), v.clone());
    }
    normalized(&map)
}

/// The largest difference between two JSON values of the same shape (numbers by value, the rest equal).
fn json_difference(actual: &Value, expected: &Value, what: &str) -> f64 {
    match (actual, expected) {
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{what}: {actual} vs {expected}");
            a.iter().zip(e).map(|(x, y)| json_difference(x, y, what)).fold(0.0, f64::max)
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut keys: Vec<&String> = a.keys().collect();
            keys.sort();
            let mut expected_keys: Vec<&String> = e.keys().collect();
            expected_keys.sort();
            assert_eq!(keys, expected_keys, "{what}: {actual} vs {expected}");
            a.iter().map(|(k, x)| json_difference(x, &e[k], what)).fold(0.0, f64::max)
        }
        (Value::Number(_), Value::Number(_)) => (f(actual) - f(expected)).abs(),
        _ => {
            assert_eq!(actual, expected, "{what}");
            0.0
        }
    }
}

fn panels_off(settings: &Settings) -> Value {
    settings.value("panels_off").clone()
}

// ---- targeted.py: hsl_weights, drag_hsl, the session ---------------------------------------------------------

#[test]
fn hsl_weights_and_drags_match_the_oracle() {
    let case = Case::open("app", "mixer_targeted");
    let meta = case.json();
    assert_eq!(f(&meta["per_pixel"]), targeted::HSL_PER_PIXEL);
    for (c, e) in targeted::HSL_CENTERS.iter().zip(meta["centers"].as_array().unwrap()) {
        assert_eq!(f64::from(*c), f(e));
    }
    let mut largest = 0f64;
    let mut weights = Vec::new();
    for (rgb, expected) in meta["colours"].as_array().unwrap().iter().zip(meta["weights"].as_array().unwrap()) {
        let rgb = [0, 1, 2].map(|i| f(&rgb[i]) as f32);
        let w = targeted::hsl_weights(rgb);
        for (a, e) in w.iter().zip(expected.as_array().unwrap()) {
            largest = largest.max((f64::from(*a) - f(e)).abs());
        }
        weights.push(w);
    }
    println!("hsl_weights: largest difference {largest:e}");
    assert!(largest <= 1e-6, "hsl_weights {largest:e}");
    let start = &meta["start"];
    let mut largest_drag = 0f64;
    for (c, expected) in meta["drag_cases"].as_array().unwrap().iter().zip(meta["drags"].as_array().unwrap()) {
        let index = c[0].as_u64().unwrap() as usize;
        let value = targeted::drag_hsl(start, &weights[index], c[1].as_u64().unwrap() as usize, f(&c[2]));
        largest_drag = largest_drag.max(json_difference(&value, expected, &format!("drag_hsl {c}")));
    }
    println!("drag_hsl: largest difference {largest_drag:e}");
    assert_eq!(largest_drag, 0.0, "drag_hsl");
}

#[test]
fn targeted_hsl_session_matches_the_oracle() {
    let case = Case::open("app", "mixer_targeted");
    let meta = case.json();
    let picture: Array3<f32> = case.array("picture");
    let view = picture.view();
    let mut edit = EditState::new(settings_with(&json!({"hsl": meta["start"], "panels_off": ["mixer"]})));
    let mut adjust = TargetedAdjust::default();
    let mut largest = 0f64;
    for (step, expected) in meta["session"].as_array().unwrap().iter().zip(meta["trace"].as_array().unwrap()) {
        match step[0].as_str().unwrap() {
            "toggle" if step[1].as_bool().unwrap() => adjust.kind = Some(Kind::Hsl),
            "toggle" => adjust.stop(),
            "started" => {
                let rgb = targeted::sample(&view, f(&step[1]), f(&step[2]));
                adjust.started_with(&mut edit, rgb);
            }
            "dragged" => adjust.dragged(&mut edit, f(&step[1])),
            "target" => adjust.hsl_target = step[1].as_u64().unwrap() as usize,
            _ => adjust.finished(&mut edit),
        }
        let what = format!("{step}");
        largest = largest.max(json_difference(edit.settings().value("hsl"), &expected["hsl"], &what));
        assert_eq!(adjust.hsl_color as u64, expected["color"].as_u64().unwrap(), "{what}: colour");
        assert_eq!(panels_off(edit.settings()), expected["panels_off"], "{what}: panels_off");
        assert_eq!(edit.history().len() as u64, expected["commits"].as_u64().unwrap(), "{what}: commits");
    }
    let labels: Vec<&str> = edit.history().iter().rev().map(String::as_str).collect();
    let expected: Vec<&str> =
        meta["commits"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(labels, expected);
    println!("TargetedAdjust('hsl') session: largest hsl difference {largest:e}");
    assert_eq!(largest, 0.0);
}

// ---- app.py: hsl_changed / update_hsl ----------------------------------------------------------------------------

#[test]
fn mixer_panel_steps_match_the_oracle() {
    let case = Case::open("app", "mixer_panel");
    let meta = case.json();
    let mut edit = EditState::new(settings_with(&json!({"panels_off": ["mixer"]})));
    let mut color = 0usize;
    for (step, expected) in meta["steps"].as_array().unwrap().iter().zip(meta["trace"].as_array().unwrap()) {
        if step[0] == "color" {
            color = step[1].as_u64().unwrap() as usize;
        } else {
            let column: usize = step[1].as_str().unwrap().parse().unwrap();
            let value = hsl_changed(edit.settings(), color, column, f(&step[2]));
            edit.set("hsl", value);
        }
        let what = format!("{step}");
        assert_eq!(json_difference(edit.settings().value("hsl"), &expected["hsl"], &what), 0.0);
        // update_hsl runs when the colour changes (a moved slider already shows its own value).
        if step[0] == "color" {
            let shown = shown_hsl(edit.settings(), color);
            for (a, e) in shown.iter().zip(expected["shown"].as_array().unwrap()) {
                assert_eq!(*a, f(e), "{what}: shown");
            }
        }
        assert_eq!(color as u64, expected["color"].as_u64().unwrap());
        assert_eq!(panels_off(edit.settings()), expected["panels_off"], "{what}: panels_off");
    }
}

// ---- studio.py point colours ---------------------------------------------------------------------------------

#[test]
fn point_session_matches_the_oracle() {
    let case = Case::open("app", "mixer_points");
    let meta = case.json();
    let source: Array3<f32> = case.array("source");
    let mut start = meta["settings"].clone();
    start["panels_off"] = json!(["mixer"]);
    let mut edit = EditState::new(settings_with(&start));
    let mut state = PointColors::default();
    state.load_points(edit.settings());
    let mut largest_rgb = 0f64;
    for (step, expected) in meta["session"].as_array().unwrap().iter().zip(meta["trace"].as_array().unwrap()) {
        let what = format!("{step}");
        let slide = |state: &PointColors, edit: &mut EditState, key: &str, value: f64| {
            let i = CONTROLS.iter().position(|c| c.0 == key).unwrap();
            let scale = CONTROLS[i].4;
            let (shown, enabled) = state.shown(edit.settings());
            // QSlider.setValue emits only when the position moves.
            if enabled && (value * scale).round() != (shown[i] * scale).round() {
                state.changed(edit, key, ((value * scale).round()) / scale);
            }
        };
        match step[0].as_str().unwrap() {
            "pick" => {
                edit.commit("보정 변경");
                let rgb =
                    point_color::sample(Source::Linear(&source), &edit.settings().0, [f(&step[1]), f(&step[2])], None)
                        .expect("sample");
                state.added(&mut edit, rgb);
            }
            "slide" => slide(&state, &mut edit, step[1].as_str().unwrap(), f(&step[2])),
            "release" => {
                edit.commit("보정 변경");
            }
            "reset" => {
                let key = step[1].as_str().unwrap();
                let reset = CONTROLS.iter().find(|c| c.0 == key).unwrap().5;
                slide(&state, &mut edit, key, reset);
                edit.commit("보정 변경");
            }
            "select" => state.select(step[1].as_i64().unwrap() as isize),
            "remove" => state.remove(&mut edit),
            "legacy" => {
                let mut next = edit.settings().clone();
                let mut list = point_color::points(&next);
                list.push(step[1].clone());
                next.set("point_colors", Value::Array(list));
                edit.set_all(next);
                edit.commit("보정 변경");
                state.load_points(edit.settings());
            }
            other => panic!("unknown step {other}"),
        }
        let points = edit.settings().value("point_colors");
        largest_rgb = largest_rgb.max(json_difference(points, &expected["point_colors"], &what));
        assert_eq!(state.index as i64, expected["index"].as_i64().unwrap(), "{what}: index");
        assert_eq!(point_color::points(edit.settings()).len() as u64, expected["items"].as_u64().unwrap());
        let (values, enabled) = state.shown(edit.settings());
        for (i, (key, ..)) in CONTROLS.iter().enumerate() {
            let e = &expected["controls"][key];
            let scale = CONTROLS[i].4;
            assert_eq!((values[i] * scale).round() / scale, f(&e["value"]), "{what}: {key}");
            assert_eq!(enabled, e["enabled"].as_bool().unwrap(), "{what}: {key} enabled");
        }
        assert_eq!(enabled, expected["preview_enabled"].as_bool().unwrap());
        assert_eq!(state.note(), expected["note"].as_str().unwrap(), "{what}: note");
        assert_eq!(panels_off(edit.settings()), expected["panels_off"], "{what}: panels_off");
        assert_eq!(edit.history().len() as u64, expected["commits"].as_u64().unwrap(), "{what}: commits");
    }
    let labels: Vec<&str> = edit.history().iter().rev().map(String::as_str).collect();
    let expected: Vec<&str> =
        meta["commits"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(labels, expected);
    println!("point colour session: largest difference (sampled rgb) {largest_rgb:e}");
    assert!(largest_rgb <= 2e-5, "{largest_rgb:e}");
}

#[test]
fn samples_and_coverage_match_the_oracle() {
    let case = Case::open("app", "mixer_coverage");
    let meta = case.json();
    let source: Array3<f32> = case.array("source");
    let settings = settings_with(&json!({}));
    let mut map: Map<String, Value> = settings.0.clone();
    for (k, v) in meta["settings"].as_object().unwrap() {
        map.insert(k.clone(), v.clone());
    }
    map.insert("point_colors".into(), meta["points"].clone());
    let settings = normalized(&map);
    let mut largest_sample = 0f64;
    for (p, expected) in meta["sample_points"].as_array().unwrap().iter().zip(meta["samples"].as_array().unwrap()) {
        let rgb = point_color::sample(Source::Linear(&source), &settings.0, [f(&p[0]), f(&p[1])], None).unwrap();
        for c in 0..3 {
            largest_sample = largest_sample.max((f64::from(rgb[c]) - f(&expected[c])).abs());
        }
    }
    println!("sample: largest difference {largest_sample:e}");
    assert!(largest_sample <= 2e-5, "sample {largest_sample:e}");
    for (bad, message) in [[1.2, 0.5], [-0.1, 0.5], [f64::NAN, 0.5]].iter().zip(meta["errors"].as_array().unwrap()) {
        let error = point_color::sample(Source::Linear(&source), &settings.0, *bad, None).unwrap_err();
        assert_eq!(error, message.as_str().unwrap());
    }
    let mut largest = 0f32;
    for index in 0..meta["points"].as_array().unwrap().len() {
        let expected: Array2<f32> = case.array(&format!("coverage_{index}"));
        let actual = point_color::preview(Source::Linear(&source), &settings.0, index, None).unwrap();
        assert_eq!(actual.dim(), expected.dim(), "coverage {index}");
        let (diff, _) = grainy_testkit::max_abs_diff(&actual, &expected);
        println!("preview {index}: largest difference {diff:e}");
        largest = largest.max(diff);
    }
    assert!(largest <= 2e-4, "preview {largest:e}");
}

// ---- the window ------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1300.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn harness<'a>(setup: impl FnOnce(&mut GrainyApp) + 'a) -> Harness<'a, GrainyApp> {
    builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        setup(&mut app);
        app
    })
}

/// The application with the real worker and a generated photo (the garden test input) open.
fn photo_harness<'a>(name: &str, setup: impl FnOnce(&mut GrainyApp) + 'a) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = common::temp_dir(name);
    let path: PathBuf = dir.join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    let open = path.clone();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![open]);
        setup(&mut app);
        app
    });
    settle(&mut harness);
    (harness, path)
}

/// Runs frames until the app has loaded, rendered and shown everything it asked for.
fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle_frame(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

fn press(harness: &mut Harness<'_, GrainyApp>, pos: Pos2) {
    harness.event(Event::PointerMoved(pos));
    harness.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
}

fn release(harness: &mut Harness<'_, GrainyApp>, pos: Pos2) {
    harness.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    harness.step();
}

fn move_to(harness: &mut Harness<'_, GrainyApp>, pos: Pos2) {
    harness.event(Event::PointerMoved(pos));
    harness.step();
}

/// The mixer open with edited colours and two point colours (one of version 1).
fn edited_mixer(app: &mut GrainyApp) {
    app.open_panel("mixer");
    app.open_foldout("point_color");
    let edit = app.edit_state_mut();
    edit.set("mixer_mode", "hsl");
    edit.set(
        "hsl",
        json!([[0, 0, 0], [0, 0, 0], [0, 0, 0], [12.0, -35.0, 20.0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0]]),
    );
    edit.set(
        "point_colors",
        json!([
            {"version": 2, "rgb": [0.31, 0.42, 0.18], "range": 0.12, "saturation_range": 0.6, "lightness_range": 0.5,
             "hue": 25.0, "saturation": -10.0, "lightness": 0},
            {"rgb": [0.6, 0.3, 0.2], "range": 0.1, "hue": 20, "saturation": 10}
        ]),
    );
    edit.commit("보정 변경");
    app.state::<TargetedAdjust>().hsl_color = 3;
}

fn panel_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = harness(edited_mixer);
    harness.run_steps(4);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn mixer_panel_korean() {
    panel_snapshot(Language::Korean, "mixer-panel-ko");
}

#[test]
fn mixer_panel_english() {
    panel_snapshot(Language::English, "mixer-panel-en");
}

/// The engine's own source of the photo at the worker's preview size, with its original size.
fn engine_source(path: &std::path::Path, settings: &Settings) -> (Array3<f32>, (f64, f64)) {
    let loaded = grainy_engine::files::load_photo(path, Some(grainy_app::worker::PREVIEW_SIZE), settings).unwrap();
    let (h, w, _) = loaded.photo.pixels().dim();
    let size = |k: &str, d: usize| loaded.metadata.get(k).and_then(Value::as_u64).unwrap_or(d as u64) as f64;
    let original = (size("width", w), size("height", h));
    (loaded.photo.pixels().clone(), original)
}

/// A pick by a click on the photo (a background job), then the coverage overlay of the new point; the point is the
/// colour `point_color.sample` reads at the clicked frame point. Snapshot with the overlay.
fn pick_in_the_window(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let (mut harness, path) = photo_harness(name, |app| {
        app.open_panel("mixer");
        app.open_foldout("point_color");
    });
    point_color::start(harness.state_mut());
    harness.run_steps(2);
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    let at = Pos2::new(rect.left() + rect.width() * 0.35, rect.top() + rect.height() * 0.3);
    let frame_point = [
        f64::from(at.x - rect.left()) / f64::from(rect.width()),
        f64::from(at.y - rect.top()) / f64::from(rect.height()),
    ];
    let before = harness.state().settings().clone();
    press(&mut harness, at);
    release(&mut harness, at);
    settle(&mut harness);
    let (source, original) = engine_source(&path, &before);
    let expected = point_color::sample(Source::Linear(&source), &before.0, frame_point, Some(original)).unwrap();
    let points = point_color::points(harness.state().settings());
    assert_eq!(points.len(), 1, "{points:?}");
    assert_eq!(points[0], point_color::new_point(expected));
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some(point_color::PICK_LABEL));
    assert!(!harness.state_mut().state::<PointColors>().picking, "the pick ends the tool");

    // A wider hue range and the overlay.
    let mut state = std::mem::take(harness.state_mut().state::<PointColors>());
    state.changed(harness.state_mut().edit_state_mut(), "range", 0.3);
    state.changed(harness.state_mut().edit_state_mut(), "hue", 40.0);
    state.overlay_on = true;
    *harness.state_mut().state::<PointColors>() = state;
    harness.state_mut().edit_state_mut().commit("보정 변경");
    settle(&mut harness);
    assert!(point_color::overlay_target(harness.state()).is_some());
    assert!(point_color::overlay_ready(harness.state()), "the overlay was computed");
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn pick_and_overlay_korean() {
    pick_in_the_window(Language::Korean, "mixer-points-ko");
}

#[test]
fn pick_and_overlay_english() {
    pick_in_the_window(Language::English, "mixer-points-en");
}

/// The mixer's targeted button through the photo view: a press and a drag up of 40 pixels give
/// `drag_hsl(hsl, hsl_weights(sample), 채도, 40 * 0.5)`, select the strongest colour and make one undo step.
#[test]
fn targeted_mixer_in_the_window() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (mut harness, _) = photo_harness("mixer-targeted", |app| app.open_panel("mixer"));
    targeted::toggle(harness.state_mut(), Kind::Hsl, true);
    harness.run_steps(2);
    assert!(targeted::checked(harness.state(), Kind::Hsl));
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    let at = Pos2::new(rect.left() + rect.width() * 0.6, rect.top() + rect.height() * 0.25);
    let frame_point = [
        f64::from(at.x - rect.left()) / f64::from(rect.width()),
        f64::from(at.y - rect.top()) / f64::from(rect.height()),
    ];
    let frame = harness.state().frame().cloned().expect("a frame on screen");
    let before = harness.state().settings().value("hsl").clone();
    press(&mut harness, at);
    for dy in [10.0, 25.0, 40.0] {
        move_to(&mut harness, at - egui::vec2(0.0, dy));
    }
    release(&mut harness, at - egui::vec2(0.0, 40.0));
    let rgb = targeted::sample_preview(&frame.preview, frame_point[0], frame_point[1]);
    let weights = targeted::hsl_weights(rgb);
    let expected = targeted::drag_hsl(&before, &weights, 1, 40.0 * targeted::HSL_PER_PIXEL);
    assert_eq!(harness.state().settings().value("hsl"), &expected);
    assert_ne!(expected, before);
    let strongest = weights.iter().enumerate().fold(0, |b, (i, w)| if *w > weights[b] { i } else { b });
    assert_eq!(harness.state_mut().state::<TargetedAdjust>().hsl_color, strongest);
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some(targeted::HSL_LABEL));
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(!targeted::checked(harness.state(), Kind::Hsl));
}
