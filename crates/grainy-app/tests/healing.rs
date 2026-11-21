//! The Healing tool (work package 11).
//!
//! * Scripted sessions without a window, step by step against the oracle's own `StudioTools` methods run on a
//!   stand-in (`tools/golden/gen_app.py`, `golden/app/healing_*`): the `retouch` operations, the staged marks, the
//!   stroke being drawn, the source, the sizes, the commits, the status line and the red coverage overlay.
//! * The real window (egui_kittest on wgpu, the background worker and the engine): Q, two strokes dragged on the
//!   photo, snapshots of the staged marks in Korean and English, Enter, and the photo after the heal equal to the
//!   engine's develop of the same settings.

mod common;

use std::path::PathBuf;

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::Invocation;
use grainy_app::panels::Tool;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::text::{tr, tr_with};
use grainy_app::tools::healing::{self, HealingState};
use grainy_app::{AppContext, GrainyApp};
use grainy_i18n::Language;
use grainy_testkit::Case;
use ndarray::Array2;
use serde_json::{json, Value};

/// Numbers equal within `1e-9` (the oracle's floats are float32 values), everything else exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() <= 1e-9,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn point(v: &Value) -> [f64; 2] {
    [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()]
}

/// A context on a session with one photo of `width` × `height` and the oracle's settings changes.
fn context(changes: &Value, width: u32, height: u32) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/healing/photo.png")]);
    session.select(0);
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    let id = cx.doc.current_id();
    cx.doc.set_original(id, (width, height));
    *cx.doc.edit_mut() = EditState::new(common::settings(changes.clone()));
    cx
}

fn step(cx: &mut AppContext, step: &[Value]) {
    let name = step[0].as_str().unwrap();
    let arg = step.get(1);
    match name {
        "shortcut" => {
            cx.run("develop.heal");
        }
        "strip" => {
            cx.open_tool(Tool::Healing);
            healing::sync(cx);
        }
        "mode" => {
            cx.run_with("heal.mode", &Invocation::value(arg.unwrap().clone()));
        }
        "press" => {
            healing::press(cx, point(arg.unwrap()), step[2].as_bool().unwrap());
        }
        "move" => {
            healing::move_to(cx, point(arg.unwrap()));
        }
        "release" => {
            healing::release(cx, point(arg.unwrap()));
        }
        "click" => {
            healing::press(cx, point(arg.unwrap()), step[2].as_bool().unwrap());
            healing::release(cx, point(arg.unwrap()));
        }
        "key" => {
            let key = match arg.unwrap().as_str().unwrap() {
                "Escape" => egui::Key::Escape,
                "Return" => egui::Key::Enter,
                "[" => egui::Key::OpenBracket,
                _ => egui::Key::CloseBracket,
            };
            healing::key(cx, key);
        }
        "button" => match arg.unwrap().as_str().unwrap() {
            // The buttons' functions (a disabled button's function too, as the oracle's script calls it).
            "apply" => healing::apply_healing(cx),
            "remove_mark" => healing::remove_healing(cx),
            "clear_marks" => healing::clear_healing(cx),
            "remove_last" => {
                cx.run("heal.remove_last");
            }
            "automatic" => {
                cx.run("heal.automatic_source");
            }
            _ => {
                cx.run("heal.finish");
            }
        },
        "radius" => healing::set_radius_position(cx, arg.unwrap().as_i64().unwrap()),
        "feather" => healing::set_feather(cx, arg.unwrap().as_i64().unwrap()),
        "before" => {
            cx.set_before(arg.unwrap().as_bool().unwrap());
            healing::update_overlay(cx);
        }
        other => panic!("unknown step {other}"),
    }
}

/// Runs one golden session; returns the largest overlay alpha difference.
fn run_case(name: &str) -> u8 {
    let case = Case::open("app", name);
    let data = case.json();
    let (w, h) = (data["width"].as_u64().unwrap() as u32, data["height"].as_u64().unwrap() as u32);
    let mut cx = context(&data["changes"], w, h);
    let mut largest = 0u8;
    for (index, (script, expected)) in
        data["script"].as_array().unwrap().iter().zip(data["states"].as_array().unwrap()).enumerate()
    {
        let script = script.as_array().unwrap();
        step(&mut cx, script);
        let at = format!("{name} step {index} {script:?}");
        let s = cx.state_ref::<HealingState>().expect("the tool's state");
        assert_eq!(s.mode.unwrap_or(""), expected["mode"].as_str().unwrap(), "{at}: mode");
        let points: Vec<Value> = s.points.iter().map(|p| json!([p[0], p[1]])).collect();
        assert!(same(&Value::Array(points), &expected["points"]), "{at}: points");
        // A clone, inpaint or red eye stroke being drawn is shown live: the oracle draws it through
        // `preview_settings` on top of its settings, the port sets it as a live (uncommitted) value.
        let mut shown = expected["retouch"].as_array().unwrap().clone();
        if !expected["pending"].is_null() && !expected["pending_staged"].as_bool().unwrap() {
            shown.push(expected["pending"].clone());
        }
        let retouch = cx.settings().value("retouch");
        assert!(same(retouch, &Value::Array(shown)), "{at}: retouch {retouch}");
        assert!(same(cx.edit_state().last_saved().value("retouch"), &expected["retouch"]), "{at}: saved retouch");
        assert!(same(&Value::Array(s.staged.clone()), &expected["staged"]), "{at}: staged");
        let pending = s.pending.as_ref().map_or(Value::Null, |p| p.component.clone());
        assert!(same(&pending, &expected["pending"]), "{at}: pending {pending}");
        assert_eq!(s.pending.as_ref().is_some_and(|p| p.staged), expected["pending_staged"].as_bool().unwrap(), "{at}");
        let donor = s.donor.map_or(Value::Null, |d| json!([d[0], d[1]]));
        assert!(same(&donor, &expected["donor"]), "{at}: donor");
        assert_eq!(tr(s.source_note()), expected["note"].as_str().unwrap(), "{at}: note");
        let waiting = tr_with("실행 대기: {0}개", &[&s.staged.len().to_string()]);
        assert_eq!(waiting, expected["waiting"].as_str().unwrap(), "{at}: waiting");
        assert_eq!(!s.staged.is_empty(), expected["apply"].as_bool().unwrap(), "{at}: apply");
        assert_eq!(!s.staged.is_empty(), expected["remove"].as_bool().unwrap(), "{at}: remove");
        assert_eq!(s.can_clear(), expected["clear"].as_bool().unwrap(), "{at}: clear");
        assert_eq!(s.radius, expected["radius"].as_i64().unwrap(), "{at}: radius");
        assert_eq!(s.feather, expected["feather"].as_i64().unwrap(), "{at}: feather");
        if s.mode.is_some() {
            assert!((s.brush_radius - expected["brush_radius"].as_f64().unwrap()).abs() < 1e-12, "{at}: cursor");
            assert!((s.brush_feather - expected["brush_feather"].as_f64().unwrap()).abs() < 1e-12, "{at}: cursor");
        }
        assert_eq!(cx.before(), expected["before"].as_bool().unwrap(), "{at}: before");
        let mut history: Vec<String> = cx.doc.edit().history().to_vec();
        history.reverse();
        let wanted: Vec<String> =
            expected["history"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        assert_eq!(history, wanted, "{at}: commits");
        if let Some(status) = expected["status"].as_str() {
            assert_eq!(cx.status(), status, "{at}: status");
        }
        assert_eq!(s.overlay().is_some(), expected["overlay"].as_bool().unwrap(), "{at}: overlay");
        let file = format!("overlay_{index}");
        if case.has(&file) {
            let want: Array2<u8> = case.array(&file);
            let got = s.overlay().expect("an overlay");
            assert_eq!((got.height, got.width), want.dim(), "{at}: overlay size");
            for (a, b) in got.alpha.iter().zip(want.iter()) {
                largest = largest.max(a.abs_diff(*b));
            }
        }
    }
    largest
}

#[test]
fn healing_sessions_equal_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    for name in ["healing_heal", "healing_clone", "healing_straighten"] {
        let largest = run_case(name);
        println!("{name}: overlay alpha largest difference {largest}");
        // Straightened frames: the inverse map and the clamp of source_point differ in the last float bits.
        assert!(largest <= 2, "{name}: the overlay differs by {largest}");
    }
}

#[test]
fn live_source_size() {
    assert_eq!(healing::live_size(1440, 960), (720, 480));
    assert_eq!(healing::live_size(97, 65), (97, 65));
    assert_eq!(healing::live_size(1000, 3000), (240, 720));
    // round(1001 * 0.24) = round(240.24) = 240; round(2.5) = 2 (half to even).
    assert_eq!(healing::live_size(1001, 3000), (240, 720));
}

#[test]
fn commands_follow_the_marks() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut cx = context(&json!({}), 300, 200);
    assert!(!grainy_app::commands::enabled(&cx, "heal.apply"));
    cx.run("develop.heal");
    assert_eq!(cx.tool(), Tool::Healing);
    healing::press(&mut cx, [0.5, 0.5], false);
    assert!(grainy_app::commands::enabled(&cx, "heal.clear_marks"), "a staged stroke being drawn can be cleared");
    healing::release(&mut cx, [0.55, 0.5]);
    assert!(grainy_app::commands::enabled(&cx, "heal.apply"));
    assert!(cx.run("heal.apply"));
    let retouch = cx.settings().value("retouch").as_array().unwrap().clone();
    assert_eq!(retouch.len(), 1);
    assert_eq!(retouch[0]["type"], "heal");
    assert_eq!(retouch[0]["version"], 2);
    assert_eq!(retouch[0]["feather"], 60);
    assert_eq!(cx.doc.edit().history()[0], "힐링 1개 실행");
    // Undo takes the whole batch back.
    cx.run("develop.undo");
    assert!(cx.settings().value("retouch").as_array().unwrap().is_empty());
    // Another photo: staged marks are dropped.
    healing::press(&mut cx, [0.2, 0.2], false);
    healing::release(&mut cx, [0.2, 0.2]);
    assert_eq!(cx.state_ref::<HealingState>().unwrap().staged.len(), 1);
    cx.doc.set_original(cx.doc.current_id(), (300, 200));
    let id = cx.doc.current_id();
    cx.state::<HealingState>().context = Some(id + 1);
    healing::sync(&mut cx);
    assert!(cx.state_ref::<HealingState>().unwrap().staged.is_empty());
}

// ---- the real window ------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

/// A drag on the photo through the window's pointer: frame points from `path[0]` to the last.
fn drag(harness: &mut Harness<'_, GrainyApp>, path: &[[f32; 2]]) {
    let rect = harness.state().photo_rect().expect("the photo is drawn");
    let at = |p: [f32; 2]| egui::pos2(rect.left() + p[0] * rect.width(), rect.top() + p[1] * rect.height());
    harness.hover_at(at(path[0]));
    harness.step();
    harness.drag_at(at(path[0]));
    harness.step();
    for p in &path[1..] {
        harness.hover_at(at(*p));
        harness.step();
    }
    harness.drop_at(at(path[path.len() - 1]));
    harness.step();
}

#[test]
fn staged_heal_on_a_photo() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("healing");
    let path = dir.join("garden-large.png");
    common::save_png(&common::garden(6, [1.0; 3], false), &path);
    let mut harness = builder().wgpu().build_eframe({
        let path = path.clone();
        move |cc| {
            let mut app = GrainyApp::new(&cc.egui_ctx);
            app.set_debug_overlay(false);
            app.open_paths(vec![path]);
            app
        }
    });
    settle(&mut harness);
    harness.key_press(egui::Key::Q);
    settle(&mut harness);
    assert_eq!(harness.state().tool(), Tool::Healing);
    assert_eq!(harness.state().state_ref::<HealingState>().and_then(|s| s.mode), Some("heal"));
    drag(&mut harness, &[[0.3, 0.35], [0.36, 0.38], [0.42, 0.4], [0.46, 0.44]]);
    drag(&mut harness, &[[0.7, 0.6], [0.72, 0.66], [0.73, 0.7]]);
    harness.hover_at(egui::pos2(800.0, 500.0));
    settle(&mut harness);
    let state = harness.state().state_ref::<HealingState>().expect("state");
    assert_eq!(state.staged.len(), 2, "two staged marks");
    assert!(state.overlay().is_some(), "the red coverage shows");
    assert!(harness.state().settings().value("retouch").as_array().unwrap().is_empty(), "nothing applied yet");
    save(&mut harness, "healing-staged-ko");
    grainy_i18n::set_language(Language::English);
    harness.run_steps(3);
    save(&mut harness, "healing-staged-en");
    grainy_i18n::set_language(Language::Korean);

    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    let app = harness.state();
    let retouch = app.settings().value("retouch").as_array().unwrap().clone();
    assert_eq!(retouch.len(), 2);
    assert!(retouch.iter().all(|op| op["type"] == "heal" && op["version"] == 2 && op["source"].is_null()));
    assert_eq!(app.edit_state().history()[0], "힐링 2개 실행");
    // The photo on screen is the engine's develop with the same operations (as an export gives).
    let frame = app.frame().expect("a frame").clone();
    let expected = common::engine_render_for_frame(&path, &frame, app.settings());
    assert!(expected.0 == frame.preview.rgba, "the healed frame is the engine's develop");
    let untouched = common::engine_render_for_frame(&path, &frame, &common::settings(json!({})));
    assert!(untouched.0 != frame.preview.rgba, "the heal changed the picture");
    save(&mut harness, "healing-applied-ko");
}
