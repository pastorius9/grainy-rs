//! Work package 3: the tone curve editor, the channel curves and the targeted adjustment.
//!
//! * Model tests against goldens made with the oracle's own code (`tools/golden/gen_app.py`, section "work
//!   package 3"): scripted pointer events through luma/widgets.py `ToneCurve` (points, `changed`, `committed`,
//!   the dragged point, the drawn curve) and luma/targeted.py `sample`, `curve_input`, `drag_curve` and a
//!   `TargetedAdjust` session.
//! * The window headless (egui_kittest on wgpu): real pointer events on the curve editor and on the photo, then
//!   snapshots in Korean and English (`tests/snapshots/curve-*.png`).

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::panels::curve::CurveEditors;
use grainy_app::state::EditState;
use grainy_app::tools::targeted::{self, Kind, TargetedAdjust};
use grainy_app::widgets::curve::{points_of, Button, Signal, ToneCurve};
use grainy_app::GrainyApp;
use grainy_core::normalized;
use grainy_i18n::Language;
use grainy_testkit::Case;
use ndarray::Array3;
use serde_json::{json, Value};

fn pair(v: &Value) -> [f64; 2] {
    [v[0].as_f64().expect("x"), v[1].as_f64().expect("y")]
}

fn points(v: &Value) -> Vec<[f64; 2]> {
    v.as_array().expect("points").iter().map(pair).collect()
}

/// The largest difference between two point lists of the same length (panics on a different length).
fn difference(actual: &[[f64; 2]], expected: &[[f64; 2]], what: &str) -> f64 {
    assert_eq!(actual.len(), expected.len(), "{what}: {actual:?} vs {expected:?}");
    actual.iter().zip(expected).map(|(a, e)| (a[0] - e[0]).abs().max((a[1] - e[1]).abs())).fold(0.0, f64::max)
}

// ---- the ToneCurve model ---------------------------------------------------------------------------------------

#[test]
fn tone_curve_pointer_scripts_match_the_oracle() {
    let _turn = common::language_lock();
    let case = Case::open("app", "curve_editor");
    let meta = case.json();
    let size = (meta["width"].as_f64().unwrap(), meta["height"].as_f64().unwrap());
    let mut largest = 0f64;
    for script in meta["scripts"].as_array().unwrap() {
        let name = script["name"].as_str().unwrap();
        let mut model = ToneCurve { size, ..ToneCurve::default() };
        model.set_hdr(script["hdr"].as_bool().unwrap());
        model.set_points(&points(&script["initial"]));
        let mut committed = 0;
        for (step, expected) in script["steps"].as_array().unwrap().iter().zip(script["results"].as_array().unwrap()) {
            let position = [step[2].as_f64().unwrap(), step[3].as_f64().unwrap()];
            let button = if step[1] == "right" { Button::Right } else { Button::Left };
            let signals = match step[0].as_str().unwrap() {
                "press" => model.press(button, position, step[4].as_bool().unwrap()),
                "move" => model.drag(position),
                "release" => model.release(),
                "double" => model.double_click(),
                other => panic!("unknown step {other}"),
            };
            let what = format!("{name} {step}");
            let mut changed = Vec::new();
            for signal in signals {
                match signal {
                    Signal::Changed(p) => changed.push(p),
                    Signal::Committed => committed += 1,
                }
            }
            let expected_changed = expected["changed"].as_array().unwrap();
            assert_eq!(changed.len(), expected_changed.len(), "{what}: changed signals");
            for (a, e) in changed.iter().zip(expected_changed) {
                largest = largest.max(difference(a, &points(e), &what));
            }
            largest = largest.max(difference(&model.points, &points(&expected["points"]), &what));
            assert_eq!(committed, expected["committed"].as_u64().unwrap(), "{what}: committed signals");
            assert_eq!(model.active.map(|a| a as u64), expected["active"].as_u64(), "{what}: dragged point");
        }
        // The axis and the drawn curve (paintEvent: curve_extended of 100 inputs, float32).
        for (i, x) in script["axis_samples"].as_array().unwrap().iter().enumerate() {
            let v = if i == 99 { 1.0 } else { i as f64 * (1.0 / 99.0) };
            largest = largest.max((model.axis_value(v) - x.as_f64().unwrap()).abs());
        }
        let drawn = model.drawn_curve();
        for (d, e) in drawn.iter().zip(script["drawn"].as_array().unwrap()) {
            let error = (d[1] - e.as_f64().unwrap()).abs();
            assert!(error <= 1e-6, "{name}: drawn curve {d:?} vs {e}");
        }
    }
    println!("ToneCurve scripts: largest point difference {largest:e}");
    assert!(largest <= 1e-12, "largest point difference {largest:e}");
}

// ---- targeted.py ---------------------------------------------------------------------------------------------

#[test]
fn targeted_functions_match_the_oracle() {
    let case = Case::open("app", "curve_targeted");
    let meta = case.json();
    let picture: Array3<f32> = case.array("picture");
    let view = picture.view();
    let mut largest_sample = 0f32;
    let samples: Vec<[f32; 3]> = meta["points"]
        .as_array()
        .unwrap()
        .iter()
        .zip(meta["samples"].as_array().unwrap())
        .map(|(p, expected)| {
            let [x, y] = pair(p);
            let rgb = targeted::sample(&view, x, y);
            for c in 0..3 {
                largest_sample = largest_sample.max((rgb[c] - expected[c].as_f64().unwrap() as f32).abs());
            }
            rgb
        })
        .collect();
    assert!(largest_sample <= 1e-6, "sample {largest_sample:e}");
    let mut largest_input = 0f64;
    for (curve, expected) in meta["curves"].as_array().unwrap().iter().zip(meta["curve_inputs"].as_array().unwrap()) {
        let curve = points(curve);
        for (rgb, e) in samples.iter().zip(expected.as_array().unwrap()) {
            largest_input = largest_input.max((targeted::curve_input(&curve, *rgb) - e.as_f64().unwrap()).abs());
        }
    }
    assert!(largest_input <= 1e-6, "curve_input {largest_input:e}");
    let curves: Vec<Vec<[f64; 2]>> = meta["curves"].as_array().unwrap().iter().map(points).collect();
    let mut largest_drag = 0f64;
    for (c, expected) in meta["drag_cases"].as_array().unwrap().iter().zip(meta["drags"].as_array().unwrap()) {
        let index = c[0].as_u64().unwrap() as usize;
        let value = targeted::drag_curve(&curves[index], c[1].as_f64().unwrap(), c[2].as_f64().unwrap());
        largest_drag = largest_drag.max(difference(&value, &points(expected), &format!("drag_curve {c}")));
    }
    assert_eq!(largest_drag, 0.0, "drag_curve");
    assert_eq!(meta["per_pixel"].as_f64().unwrap(), targeted::CURVE_PER_PIXEL);
    println!("sample {largest_sample:e}, curve_input {largest_input:e}, drag_curve {largest_drag:e}");
}

#[test]
fn targeted_session_matches_the_oracle() {
    let case = Case::open("app", "curve_targeted");
    let meta = case.json();
    let picture: Array3<f32> = case.array("picture");
    let view = picture.view();
    let mut edit = EditState::new(normalized(&serde_json::Map::new()));
    let mut adjust = TargetedAdjust::default();
    let mut largest = 0f64;
    for (step, expected) in meta["session"].as_array().unwrap().iter().zip(meta["trace"].as_array().unwrap()) {
        match step[0].as_str().unwrap() {
            // `toggle` on the window (status, other buttons) is tested through the window below.
            "toggle" if step[1].as_bool().unwrap() => adjust.kind = Some(Kind::Curve),
            "toggle" => adjust.stop(),
            "started" => {
                let rgb = targeted::sample(&view, step[1].as_f64().unwrap(), step[2].as_f64().unwrap());
                adjust.started_with(&mut edit, rgb);
            }
            "dragged" => adjust.dragged(&mut edit, step[1].as_f64().unwrap()),
            _ => adjust.finished(&mut edit),
        }
        let curve = points_of(edit.settings().value("curve"));
        largest = largest.max(difference(&curve, &points(&expected["curve"]), &format!("{step}")));
        assert_eq!(edit.history().len() as u64, expected["commits"].as_u64().unwrap(), "{step}: commits");
    }
    let commits = meta["commits"].as_array().unwrap();
    let labels: Vec<&str> = edit.history().iter().rev().map(String::as_str).collect();
    let expected: Vec<&str> = commits.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(labels, expected);
    println!("TargetedAdjust session: largest curve difference {largest:e}");
    assert!(largest <= 1e-6, "{largest:e}");
}

// ---- the window ------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
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
fn photo_harness<'a>(name: &str, setup: impl FnOnce(&mut GrainyApp) + 'a) -> Harness<'a, GrainyApp> {
    let dir = common::temp_dir(name);
    let path: PathBuf = dir.join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![path]);
        setup(&mut app);
        app
    });
    settle(&mut harness);
    harness
}

/// Runs frames until the app has loaded, rendered and shown everything it asked for.
fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle_frame(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

fn press(harness: &mut Harness<'_, GrainyApp>, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
    harness.event(Event::PointerMoved(pos));
    harness.event(Event::PointerButton { pos, button, pressed: true, modifiers });
    harness.step();
}

fn release(harness: &mut Harness<'_, GrainyApp>, pos: Pos2, button: PointerButton) {
    harness.event(Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
    harness.step();
    harness.step();
}

fn move_to(harness: &mut Harness<'_, GrainyApp>, pos: Pos2) {
    harness.event(Event::PointerMoved(pos));
    harness.step();
}

/// The curve panel open with an edited tone curve and channel curve.
fn edited_curves(app: &mut GrainyApp) {
    app.open_panel("curve");
    app.open_foldout("channel_curve");
    let edit = app.edit_state_mut();
    edit.set("curve", json!([[0.0, 0.06], [0.25, 0.2], [0.5, 0.5], [0.72, 0.82], [1.0, 0.97]]));
    edit.set("rgb_curves", json!([[[0, 0], [0.4, 0.5], [1, 1]], [[0, 0], [1, 1]], [[0, 0.1], [1, 0.9]]]));
    edit.set("parametric", json!([20.0, 0.0, -15.0, 0.0]));
    edit.commit("보정 변경");
}

fn snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = harness(edited_curves);
    // Tall enough for the channel curve under the tone curve.
    harness.set_size(egui::vec2(1600.0, 1300.0));
    harness.run_steps(4);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn curve_panel_korean() {
    snapshot(Language::Korean, "curve-panel-ko");
}

#[test]
fn curve_panel_english() {
    snapshot(Language::English, "curve-panel-en");
}

/// Ctrl+click adds a point, a drag moves it (one undo step), a right-click deletes it, on the real editor; the
/// settings follow the model of the oracle's widget.
#[test]
fn curve_editor_in_the_window() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut harness = photo_harness("curve-editor", |app| {
        app.open_panel("curve");
        app.open_foldout("channel_curve");
    });
    // Tall enough for the channel curve under the tone curve.
    harness.set_size(egui::vec2(1600.0, 1400.0));
    harness.run_steps(3);
    let rect = harness.state_mut().state::<CurveEditors>().main.rect.expect("the tone curve was drawn");
    let mut model = ToneCurve { size: (f64::from(rect.width()), f64::from(rect.height())), ..ToneCurve::default() };
    let local = |p: Pos2| [f64::from(p.x - rect.left()), f64::from(p.y - rect.top())];
    let undo_before = harness.state().edit_state().depth().0;

    // Ctrl+click on an empty place: a point is added there; the release commits.
    let at = rect.left_top() + egui::vec2(40.0, 30.0);
    press(&mut harness, at, PointerButton::Primary, Modifiers::COMMAND);
    model.press(Button::Left, local(at), true);
    let to = at + egui::vec2(6.0, 10.0);
    move_to(&mut harness, to);
    model.drag(local(to));
    release(&mut harness, to, PointerButton::Primary);
    model.release();
    let curve = points_of(harness.state().settings().value("curve"));
    assert_eq!(curve, model.points);
    assert_eq!(curve.len(), 6);
    assert_eq!(harness.state().edit_state().depth().0, undo_before + 1, "one undo step");
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some("보정 변경"));

    // Right-click on it: deleted, one more undo step.
    press(&mut harness, to, PointerButton::Secondary, Modifiers::NONE);
    release(&mut harness, to, PointerButton::Secondary);
    model.press(Button::Right, local(to), false);
    let curve = points_of(harness.state().settings().value("curve"));
    assert_eq!(curve, model.points);
    assert_eq!(curve.len(), 5);
    assert_eq!(harness.state().edit_state().depth().0, undo_before + 2);

    // The channel curve (빨강 R) edits rgb_curves[0] only.
    let rect = harness.state_mut().state::<CurveEditors>().channel.rect.expect("the channel curve was drawn");
    let point = rect.left_top() + egui::vec2(12.0 + (rect.width() - 24.0) * 0.5, 12.0 + (rect.height() - 24.0) * 0.5);
    press(&mut harness, point, PointerButton::Primary, Modifiers::NONE);
    move_to(&mut harness, point - egui::vec2(0.0, 20.0));
    release(&mut harness, point - egui::vec2(0.0, 20.0), PointerButton::Primary);
    let curves = harness.state().settings().value("rgb_curves").clone();
    let red = points_of(&curves[0]);
    assert!((red[2][1] - (0.5 + 20.0 / f64::from(rect.height() - 24.0))).abs() < 1e-9, "{red:?}");
    assert_eq!(
        points_of(&curves[1]),
        points_of(&json!([[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [0.75, 0.75], [1.0, 1.0]]))
    );
    grainy_i18n::set_language(Language::Korean);
}

/// The targeted adjustment through the photo view: the button, a press on the photo and a drag up of 60 pixels
/// give `drag_curve(curve, curve_input(curve, sample), 60 / 300)` and one undo step 사진에서 톤 커브 조정.
fn targeted_in_the_window(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = photo_harness(name, |app| app.open_panel("curve"));
    targeted::toggle(harness.state_mut(), Kind::Curve, true);
    harness.run_steps(2);
    assert!(targeted::checked(harness.state(), Kind::Curve));
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    let at = Pos2::new(rect.left() + rect.width() * 0.3, rect.top() + rect.height() * 0.68);
    let frame_point = [
        f64::from(at.x - rect.left()) / f64::from(rect.width()),
        f64::from(at.y - rect.top()) / f64::from(rect.height()),
    ];
    let frame = harness.state().frame().cloned().expect("a frame on screen");
    let before = points_of(harness.state().settings().value("curve"));
    press(&mut harness, at, PointerButton::Primary, Modifiers::NONE);
    for dy in [10.0, 30.0, 60.0] {
        move_to(&mut harness, at - egui::vec2(0.0, dy));
    }
    release(&mut harness, at - egui::vec2(0.0, 60.0), PointerButton::Primary);
    let rgb = targeted::sample_preview(&frame.preview, frame_point[0], frame_point[1]);
    let expected = targeted::drag_curve(&before, targeted::curve_input(&before, rgb), 60.0 * targeted::CURVE_PER_PIXEL);
    let curve = points_of(harness.state().settings().value("curve"));
    assert_eq!(curve, expected);
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some(targeted::CURVE_LABEL));
    assert!(!harness.state().edit_state().dirty());
    settle(&mut harness);
    save(&mut harness, name);
    // Esc leaves the tool.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(!targeted::checked(harness.state(), Kind::Curve));
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn targeted_adjustment_korean() {
    targeted_in_the_window(Language::Korean, "curve-targeted-ko");
}

#[test]
fn targeted_adjustment_english() {
    targeted_in_the_window(Language::English, "curve-targeted-en");
}
