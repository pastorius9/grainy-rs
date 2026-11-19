//! The Transform panel (work package 19): automatic alignment and guided upright.
//!
//! * Scripted sessions without a window, step by step against the oracle's own `StudioTools` methods run on
//!   stand-ins (`tools/golden/gen_app_transform.py`, `golden/app/transform_*`): the guide corners placed by clicks
//!   and strokes, `guides`, the commits, the status line and the message box; the alignment jobs' start and end
//!   (fed the oracle's own results and errors), `upright`, the angle, the labels, the buttons and the commits.
//! * The guide tool through the canvas-tool interface (pointer events at screen points), and the tool ending when
//!   another tool takes the view.
//! * The real window (egui_kittest on wgpu, the background worker, the engine): the alignment job on a generated
//!   facade, a job that finds no lines, and guide corners being placed; snapshots in Korean and English.

mod common;

use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui::{self, Key, Modifiers, Pos2, Rect};
use egui_kittest::Harness;
use grainy_app::imagespace::ViewTransform;
use grainy_app::panels::transform::{self, Ticket};
use grainy_app::panels::Tool;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::text::tr;
use grainy_app::tools::crop;
use grainy_app::tools::guides::{self, GuideState, Guides};
use grainy_app::tools::{CanvasTool, PointerEvent, PointerKind};
use grainy_app::{AppContext, GrainyApp};
use grainy_i18n::Language;
use grainy_testkit::Case;
use ndarray::Array3;
use serde_json::{json, Value};

/// Numbers equal within `1e-9` (the oracle's corners are float32 values), everything else exactly.
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

fn points(list: &[[f64; 2]]) -> Value {
    Value::Array(list.iter().map(|p| json!([p[0], p[1]])).collect())
}

/// A context on a session with one photo of `width` × `height` and the oracle's settings changes.
fn context(changes: &Value, width: u32, height: u32) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/GrainyGolden/transform.png")]);
    session.select(0);
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    let id = cx.doc.current_id();
    cx.doc.set_original(id, (width, height));
    *cx.doc.edit_mut() = EditState::new(common::settings(changes.clone()));
    cx
}

fn history(cx: &AppContext) -> Vec<String> {
    let mut history: Vec<String> = cx.doc.edit().history().to_vec();
    history.reverse();
    history
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect()
}

// ---- guided upright -----------------------------------------------------------------------------------------

fn guide_step(cx: &mut AppContext, step: &[Value], boxes: &mut usize) {
    let arg = step.get(1);
    match step[0].as_str().unwrap() {
        "start" => guides::start(cx),
        "press" => {
            guides::press(cx, point(arg.unwrap()));
        }
        "move" => {
            guides::move_to(cx, point(arg.unwrap()));
        }
        "release" => {
            guides::release(cx, point(arg.unwrap()));
        }
        "click" => {
            guides::press(cx, point(arg.unwrap()));
            guides::release(cx, point(arg.unwrap()));
        }
        "escape" => {
            guides::key(cx, Key::Escape);
        }
        "clear" => guides::clear(cx),
        other => panic!("unknown step {other}"),
    }
    // The oracle's message box is modal: it is answered before the next step.
    if cx.dialog_open("guides") {
        *boxes += 1;
        cx.close_dialog("guides");
    }
}

fn run_guides(name: &str) -> usize {
    let case = Case::open("app", name);
    let data = case.json();
    let (w, h) = (data["width"].as_u64().unwrap() as u32, data["height"].as_u64().unwrap() as u32);
    let mut cx = context(&data["changes"], w, h);
    let mut boxes = 0;
    let mut corners = 0;
    for (index, (script, expected)) in
        data["script"].as_array().unwrap().iter().zip(data["states"].as_array().unwrap()).enumerate()
    {
        let script = script.as_array().unwrap();
        guide_step(&mut cx, script, &mut boxes);
        let at = format!("{name} step {index} {script:?}");
        let mode = if guides::is_on(&cx) { "guide" } else { "" };
        assert_eq!(mode, expected["mode"].as_str().unwrap(), "{at}: mode");
        let s = cx.state_ref::<GuideState>().expect("the tool's state");
        assert!(same(&points(&s.stroke), &expected["stroke"]), "{at}: stroke {:?}", s.stroke);
        assert!(same(&points(&s.corners), &expected["corners"]), "{at}: corners {:?}", s.corners);
        corners = corners.max(s.corners.len());
        let guides = cx.settings().value("guides");
        assert!(same(guides, &expected["guides"]), "{at}: guides {guides}");
        assert_eq!(history(&cx), strings(&expected["history"]), "{at}: commits");
        if let Some(status) = expected["status"].as_str() {
            assert_eq!(cx.status(), status, "{at}: status");
        }
        let wanted = expected["boxes"].as_array().unwrap();
        assert_eq!(boxes, wanted.len(), "{at}: message boxes");
        if let Some(last) = wanted.last() {
            assert_eq!(tr("가이드"), last[0].as_str().unwrap());
            assert_eq!(tr("네 점이 겹치지 않는 사각형이 되도록 다시 선택해 주세요."), last[1].as_str().unwrap());
        }
    }
    corners
}

#[test]
fn guide_sessions_equal_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    for name in ["transform_guides_plain", "transform_guides_transformed", "transform_guides_again"] {
        let corners = run_guides(name);
        assert!(corners >= 3, "{name}");
    }
}

#[test]
fn quadrilateral_check() {
    let square = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
    assert!(guides::valid_quad(&square));
    // Crossed (bow tie), collinear corners and a tiny square are refused.
    assert!(!guides::valid_quad(&[[0.1, 0.1], [0.9, 0.9], [0.9, 0.1], [0.1, 0.9]]));
    assert!(!guides::valid_quad(&[[0.1, 0.1], [0.5, 0.1], [0.9, 0.1], [0.1, 0.9]]));
    assert!(!guides::valid_quad(&[[0.5, 0.5], [0.55, 0.5], [0.55, 0.55], [0.5, 0.55]]));
    // The area limit is strict: 0.0729 > .005 passes, 0.07 x 0.07 = 0.0049 does not.
    assert!(guides::valid_quad(&[[0.0, 0.0], [0.27, 0.0], [0.27, 0.27], [0.0, 0.27]]));
}

fn event(kind: PointerKind, view: &ViewTransform, frame: [f64; 2]) -> PointerEvent {
    let screen = view.to_screen(frame);
    PointerEvent {
        kind,
        screen,
        frame: view.to_frame(screen),
        free: view.to_frame_free(screen),
        modifiers: Modifiers::NONE,
    }
}

/// Clicks and a stroke sent to the tool as the photo view sends them give the oracle's guides.
#[test]
fn guide_tool_takes_pointer_events() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let case = Case::open("app", "transform_guides_plain");
    let expected = case.json()["states"].as_array().unwrap().last().unwrap()["guides"].clone();
    let mut cx = context(&json!({}), 97, 65);
    // 97 × 65 photo pixels shown 4× at (100, 50): every click lands on a whole screen point.
    let view = ViewTransform { rect: Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(388.0, 260.0)) };
    let mut tool = Guides;
    assert!(!tool.active(&cx));
    guides::start(&mut cx);
    assert!(tool.active(&cx));
    // Outside the photo the view pans (the tool does not take the press).
    assert!(!tool.pointer(&event(PointerKind::Press, &view, [-0.2, 0.5]), &mut cx));
    let clicks = [[0.1, 0.15], [0.85, 0.1], [0.9, 0.9]];
    for p in clicks {
        assert!(tool.pointer(&event(PointerKind::Click, &view, p), &mut cx));
    }
    // The last corner as a short drag: the press point is the corner.
    assert!(tool.pointer(&event(PointerKind::Press, &view, [0.12, 0.8]), &mut cx));
    assert!(tool.pointer(&event(PointerKind::Drag, &view, [0.13, 0.81]), &mut cx));
    assert!(tool.pointer(&event(PointerKind::Release, &view, [0.13, 0.81]), &mut cx));
    assert!(same(cx.settings().value("guides"), &expected), "{}", cx.settings().value("guides"));
    assert!(!tool.active(&cx));
    assert_eq!(history(&cx), vec!["부분 보정".to_owned()]);
    assert_eq!(cx.status(), "도구를 종료했습니다.");
}

/// Another tool taking the view ends the guide tool (`tab_changed` → `set_tool('')`); Esc during a stroke cancels
/// it, a second Esc ends the tool; a crop frame open when the tool starts is cancelled.
#[test]
fn guide_tool_ends_with_other_tools() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut cx = context(&json!({}), 97, 65);
    let tool = Guides;
    guides::start(&mut cx);
    assert!(tool.active(&cx));
    cx.open_tool(Tool::Masks);
    assert!(!tool.active(&cx));
    cx.open_tool(Tool::Edit);
    assert!(!tool.active(&cx), "the tool stays ended");
    // The crop frame open: starting the guides cancels it without applying.
    crop::toggle(&mut cx);
    assert!(crop::is_open(&cx));
    guides::start(&mut cx);
    assert!(!crop::is_open(&cx));
    assert_eq!(cx.tool(), Tool::Edit);
    assert!(tool.active(&cx));
    assert!(guides::press(&mut cx, [0.5, 0.5]));
    assert!(guides::key(&mut cx, Key::Escape));
    assert!(tool.active(&cx));
    assert!(cx.state_ref::<GuideState>().unwrap().stroke.is_empty());
    assert!(guides::key(&mut cx, Key::Escape));
    assert!(!tool.active(&cx));
    assert!(cx.settings().value("guides").is_null());
}

// ---- automatic alignment -------------------------------------------------------------------------------------

fn mode(name: &str) -> &'static str {
    transform::MODES.iter().find(|(_, m)| *m == name).expect("a mode").1
}

fn run_upright(name: &str) -> usize {
    let case = Case::open("app", name);
    let data = case.json();
    let mut cx = context(&data["changes"], 320, 240);
    let mut jobs: VecDeque<Ticket> = VecDeque::new();
    let mut applied = 0;
    for (index, (script, expected)) in
        data["script"].as_array().unwrap().iter().zip(data["states"].as_array().unwrap()).enumerate()
    {
        let step = script.as_array().unwrap();
        let at = format!("{name} step {index} {step:?}");
        match step[0].as_str().unwrap() {
            "upright" => jobs.push_back(transform::begin(&mut cx, mode(step[1].as_str().unwrap())).expect("a photo")),
            "finish" => {
                let ticket = jobs.pop_front().expect("a job");
                let outcome = &expected["outcome"];
                let result = match outcome.get("result") {
                    Some(result) => Ok(result.clone()),
                    None => Err(outcome["error"].as_str().unwrap().to_owned()),
                };
                transform::finish(&mut cx, &ticket, result);
            }
            "clear" => transform::clear_upright(&mut cx),
            "set" => cx.doc.edit_mut().set(step[1].as_str().unwrap(), step[2].clone()),
            "commit" => {
                cx.doc.edit_mut().commit("보정 변경");
            }
            other => panic!("unknown step {other}"),
        }
        let s = cx.settings();
        assert!(same(s.value("upright"), &expected["upright"]), "{at}: upright {}", s.value("upright"));
        if !s.value("upright").is_null() {
            applied += 1;
        }
        assert!((s.f64("straighten") - expected["straighten"].as_f64().unwrap()).abs() < 1e-12, "{at}: straighten");
        assert_eq!(s.bool("upright_crop"), expected["upright_crop"].as_bool().unwrap(), "{at}: upright_crop");
        assert_eq!(history(&cx), strings(&expected["history"]), "{at}: commits");
        assert_eq!(transform::label(&cx), expected["label"].as_str().unwrap(), "{at}: label");
        let note = crop::session(&cx)
            .and_then(|c| c.level_note.clone())
            .unwrap_or_else(|| tr("직선이 보이는 사진에서 자동 보정한 뒤 아래 슬라이더로 미세 조절하세요."));
        assert_eq!(note, expected["level_note"].as_str().unwrap(), "{at}: level note");
        assert_eq!(transform::buttons_enabled(&cx), expected["enabled"].as_bool().unwrap(), "{at}: buttons");
        assert_eq!(transform::buttons_enabled(&cx), expected["level_enabled"].as_bool().unwrap(), "{at}: level");
        if let Some(status) = expected["status"].as_str() {
            assert_eq!(cx.status(), status, "{at}: status");
        }
    }
    applied
}

#[test]
fn upright_sessions_equal_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    assert!(run_upright("transform_upright_building") > 0);
    assert_eq!(run_upright("transform_upright_empty"), 0);
}

#[test]
fn upright_labels_in_english() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::English);
    let mut s = common::settings(json!({}));
    assert_eq!(transform::stored_label(&s), "Auto alignment not applied");
    s.set("upright", json!({"mode": "vertical", "rotation": -0.005, "crop_scale": 1.125}));
    assert_eq!(transform::stored_label(&s), "Vertical applied · Rotation -0.01° · Edge scale 1.12×");
    grainy_i18n::set_language(Language::Korean);
    assert_eq!(transform::stored_label(&s), "수직 적용 · 회전 -0.01° · 가장자리 확대 1.12배");
    assert_eq!(
        transform::short("Traceback\n  ...\nluma.upright.AlignmentError: 원근 추정의 오차가 큽니다.\n"),
        "원근 추정의 오차가 큽니다."
    );
}

// ---- the real window --------------------------------------------------------------------------------------------

fn photo_harness(path: PathBuf) -> Harness<'static, GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(|ctx| {
            grainy_app::fonts::install(ctx);
            grainy_app::theme::apply(ctx);
        })
        .wgpu()
        .build_eframe(move |cc| {
            let mut app = GrainyApp::new(&cc.egui_ctx);
            app.set_debug_overlay(false);
            app.open_paths(vec![path]);
            app
        })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle_frame(harness);
}

/// A facade turned by 4 degrees (the oracle's `scene('building')` drawn larger): light uprights and floor lines on
/// grey, scene-linear.
fn facade() -> Array3<f32> {
    let (h, w) = (480usize, 640usize);
    let turn = 4f64.to_radians();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    Array3::from_shape_fn((h, w, 3), |(y, x, _)| {
        // The canvas point shown at (x, y) (the photo turned counter-clockwise by 4 degrees).
        let (dx, dy) = (x as f64 - cx, y as f64 - cy);
        let u = dx * turn.cos() - dy * turn.sin() + cx * 2.0;
        let v = dx * turn.sin() + dy * turn.cos() + cy * 2.0;
        let column = (u - 40.0).rem_euclid(100.0) < 44.0;
        let floor = (v - 60.0).rem_euclid(140.0) < 6.0;
        if column {
            0.6
        } else if floor {
            0.25
        } else {
            0.06
        }
    })
}

fn snapshot(
    language: Language,
    name: &str,
    image: &Array3<f32>,
    setup: impl Fn(&mut GrainyApp),
    check: impl Fn(&GrainyApp),
) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(name);
    let path = dir.join("photo.png");
    common::save_png(image, &path);
    let mut harness = photo_harness(path);
    settle(&mut harness);
    harness.state_mut().open_panel("transform");
    setup(harness.state_mut());
    settle(&mut harness);
    let shot = harness.render().expect("render the frame");
    common::snapshot::check(&shot, name);
    check(harness.state());
    grainy_i18n::set_language(Language::Korean);
}

fn full(app: &mut GrainyApp) {
    transform::auto_upright(app, "full");
    assert!(!transform::buttons_enabled(app));
}

fn level(app: &mut GrainyApp) {
    transform::auto_upright(app, "level");
}

/// Two corners placed, a third being pressed.
fn corners(app: &mut GrainyApp) {
    guides::start(app);
    guides::click(app, [0.2, 0.2]);
    guides::click(app, [0.8, 0.15]);
    guides::press(app, [0.85, 0.85]);
}

#[test]
fn upright_job_snapshots() {
    for (language, name) in [(Language::Korean, "transform-upright-ko"), (Language::English, "transform-upright-en")] {
        snapshot(language, name, &facade(), full, |app| {
            let upright = app.settings().value("upright").clone();
            assert_eq!(upright["mode"], "full", "{upright}");
            let rotation = upright["rotation"].as_f64().unwrap();
            assert!((rotation.abs() - 4.0).abs() < 0.5, "the facade is turned by 4 degrees: {rotation}");
            assert_eq!(app.edit_state().history().first().map(String::as_str), Some("자동 수평·원근"));
            assert!(transform::buttons_enabled(app));
        });
    }
}

#[test]
fn upright_no_lines_snapshots() {
    let flat = Array3::from_elem((240, 320, 3), 0.05f32);
    for (language, name) in [(Language::Korean, "transform-nolines-ko"), (Language::English, "transform-nolines-en")] {
        snapshot(language, name, &flat, level, |app| {
            assert!(app.settings().value("upright").is_null());
            assert!(app.edit_state().history().is_empty());
            assert!(transform::buttons_enabled(app));
            assert_ne!(transform::label(app), tr("자동 정렬 미적용"), "the failure is shown");
        });
    }
}

#[test]
fn guide_corner_snapshots() {
    for (language, name) in [(Language::Korean, "transform-guides-ko"), (Language::English, "transform-guides-en")] {
        snapshot(language, name, &common::garden(4, [1.0; 3], false), corners, |app| {
            let s = app.state_ref::<GuideState>().expect("the tool's state");
            assert_eq!(s.corners.len(), 2);
            assert_eq!(s.stroke.len(), 1);
            assert!(guides::is_on(app));
        });
    }
}
