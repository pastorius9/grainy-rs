//! The crop tool against the oracle (`golden/app/crop_*`, made by tools/golden/gen_app.py from luma/widgets.py
//! `PhotoView`'s crop mode and luma/app.py's crop functions run on stand-ins): the same pointer presses, drags and
//! releases at the same screen points, keys, buttons, aspects, slider values and job results give the same crop
//! frame, angle, limit, settings, undo labels and size label after every step. Without a window: an
//! [`AppContext`] on an in-memory session, the steps through the crop tool's pointer and the `crop.*` commands.
//! Then snapshots of the frame on a straightened photo and with an aspect, in Korean and English.

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Modifiers, Pos2, Rect};
use egui_kittest::Harness;
use grainy_app::commands::Invocation;
use grainy_app::imagespace::ViewTransform;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::py_eq;
use grainy_app::tools::crop::{self, CropTool};
use grainy_app::tools::{PointerEvent, PointerKind};
use grainy_app::{AppContext, GrainyApp};
use grainy_core::normalized;
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::Value;

const SCRIPTS: [&str; 4] = ["plain", "straightened", "rotated", "upright"];

fn context(original: (u32, u32), changes: &Value) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/GrainyGolden/crop.png")]);
    let id = session.current_id();
    session.set_original(id, original);
    let mut settings = grainy_core::defaults().clone();
    for (k, v) in changes.as_object().unwrap() {
        settings.set(k, v.clone());
    }
    session.edit = grainy_app::state::EditState::new(normalized(&settings.0));
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session))
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn close(a: f64, b: f64, what: &str) -> f64 {
    let e = (a - b).abs();
    assert!(e <= 1e-9, "{what}: {a} vs {b}");
    e
}

fn event(kind: PointerKind, x: f64, y: f64, view: &ViewTransform) -> PointerEvent {
    let screen = Pos2::new(x as f32, y as f32);
    PointerEvent {
        kind,
        screen,
        frame: view.to_frame(screen),
        free: view.to_frame_free(screen),
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn crop_steps_match_the_oracle() {
    let _turn = common::language_lock();
    let mut largest = 0.0f64;
    for name in SCRIPTS {
        let case = Case::open("app", &format!("crop_{name}"));
        let meta = case.json();
        let original = (meta["original"][0].as_u64().unwrap() as u32, meta["original"][1].as_u64().unwrap() as u32);
        let mut cx = context(original, &meta["changes"]);
        for key in ["crop", "straighten", "rotation", "flip", "upright"] {
            assert!(py_eq(cx.settings().value(key), &meta["start"][key]), "{name} start {key}");
        }
        let mut tool = CropTool;
        let mut view = ViewTransform { rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(1.0, 1.0)) };
        for (index, step) in meta["steps"].as_array().unwrap().iter().enumerate() {
            let op = step["op"].as_array().unwrap();
            let action = op[0].as_str().unwrap();
            let at = format!("{name} step {index} {op:?}");
            let xy = || (f(&op[1]), f(&op[2]));
            match action {
                "toggle" => {
                    cx.run("crop.toggle");
                }
                "frame" => {
                    let scale = f(&op[1]);
                    let logical = [f(&step["logical"][0]), f(&step["logical"][1])];
                    let size = egui::vec2((logical[0] * scale) as f32, (logical[1] * scale) as f32);
                    view = ViewTransform {
                        rect: Rect::from_min_size(Pos2::new(f(&op[2]) as f32, f(&op[3]) as f32), size),
                    };
                    let scene = crop::scene_for(&cx, view.rect);
                    assert_eq!(scene.image, logical, "{at}");
                    match &step["canvas"] {
                        Value::Null => assert_eq!(scene.canvas, None, "{at}"),
                        c => assert_eq!(scene.canvas, Some([f(&c[0]), f(&c[1]), f(&c[2]), f(&c[3])]), "{at}"),
                    }
                    assert_eq!(scene.scale, scale, "{at}");
                    crop::frame_arrived(&mut cx);
                }
                "press" | "move" | "release" => {
                    let kind = match action {
                        "press" => PointerKind::Press,
                        "move" => PointerKind::Drag,
                        _ => PointerKind::Release,
                    };
                    let (x, y) = xy();
                    tool.pointer_with_view(&event(kind, x, y, &view), &view, &mut cx);
                    if action == "press" {
                        let handle = crop::session(&cx).unwrap().frame.handle.map(|h| h.name());
                        assert_eq!(handle, step["handle"].as_str(), "{at}");
                    }
                }
                "double" => tool.double_click(&mut cx),
                "enter" | "escape" => {
                    let key = if action == "enter" { egui::Key::Enter } else { egui::Key::Escape };
                    grainy_app::tools::CanvasTool::key(&mut tool, key, Modifiers::NONE, &mut cx);
                }
                "apply" => {
                    cx.run("crop.apply");
                }
                "cancel" => {
                    cx.run("crop.cancel");
                }
                "aspect" => {
                    cx.run_with("crop.aspect", &Invocation::value(op[1].as_u64().unwrap()));
                }
                "slider" => crop::straighten_slider(&mut cx, f(&op[1])),
                "commit" => {
                    cx.edit_state_mut().commit("보정 변경");
                }
                "reset" => {
                    cx.run("crop.reset");
                }
                "rotate" => {
                    cx.run("crop.rotate");
                }
                "flip" => {
                    cx.run("crop.flip");
                }
                "auto_level" => crop::auto_level_result(&mut cx, f(&op[1])),
                "border" => {
                    crop::border_begin(&mut cx);
                    let c = &op[1];
                    crop::border_result(&mut cx, [f(&c[0]), f(&c[1]), f(&c[2]), f(&c[3])]);
                }
                other => panic!("unknown step {other}"),
            }
            let s = crop::session(&cx).expect("the crop session");
            for (a, b) in s.frame.crop_values().iter().zip(step["frame"].as_array().unwrap()) {
                largest = largest.max(close(*a, f(b), &format!("{at} frame")));
            }
            assert_eq!(s.frame.open, step["open"].as_bool().unwrap(), "{at} open");
            close(s.frame.straighten_angle, f(&step["angle"]), &format!("{at} angle"));
            let limit = [s.frame.limit.0, s.frame.limit.1, s.frame.limit.2];
            for (a, b) in limit.iter().zip(step["limit"].as_array().unwrap()) {
                close(*a, f(b), &format!("{at} limit"));
            }
            close(s.frame.aspect, f(&step["aspect"]), &format!("{at} aspect"));
            assert_eq!(s.size_text, step["size"].as_str().unwrap(), "{at} size");
            for key in ["straighten", "rotation", "flip", "upright"] {
                assert!(
                    py_eq(cx.settings().value(key), &step["settings"][key]),
                    "{at} {key}: {:?}",
                    cx.settings().value(key)
                );
            }
            match (cx.settings().value("crop"), &step["settings"]["crop"]) {
                (Value::Null, Value::Null) => {}
                (Value::Array(a), Value::Array(b)) => {
                    for (x, y) in a.iter().zip(b) {
                        largest = largest.max(close(f(x), f(y), &format!("{at} settings crop")));
                    }
                }
                (a, b) => panic!("{at} settings crop {a:?} vs {b:?}"),
            }
            let mut labels: Vec<String> = cx.edit_state().history().to_vec();
            labels.reverse();
            let expected: Vec<String> =
                step["commits"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            assert_eq!(labels, expected, "{at} commits");
        }
    }
    println!("crop steps: largest difference {largest:e}");
}

#[test]
fn straighten_maths_match_the_oracle() {
    use grainy_geometry::straighten;
    let case = Case::open("app", "crop_straighten");
    let mut largest = 0.0f64;
    for row in case.json()["rows"].as_array().unwrap() {
        let (angle, w, h) = (f(&row["angle"]), f(&row["width"]), f(&row["height"]));
        let crop = [f(&row["crop"][0]), f(&row["crop"][1]), f(&row["crop"][2]), f(&row["crop"][3])];
        let aspect = row["aspect"].as_f64();
        let four = |v: &Value| [f(&v[0]), f(&v[1]), f(&v[2]), f(&v[3])];
        let refit = straighten::refit(Some(crop), 0.0, angle, w, h, aspect);
        match (&row["refit"], refit) {
            (Value::Null, None) => {}
            (v, Some(r)) => {
                for (a, b) in r.iter().zip(four(v)) {
                    largest = largest.max(close(*a, b, "refit"));
                }
            }
            (v, r) => panic!("refit {v:?} vs {r:?}"),
        }
        for (a, b) in straighten::shrink(crop, angle, w, h).iter().zip(four(&row["shrink"])) {
            largest = largest.max(close(*a, b, "shrink"));
        }
        for (a, b) in straighten::limit(crop, [0.4, 0.4, 0.6, 0.6], angle, w, h).iter().zip(four(&row["limit"])) {
            largest = largest.max(close(*a, b, "limit"));
        }
        let (px, py) = straighten::padding(angle, w as usize, h as usize);
        assert_eq!([px as f64, py as f64], [f(&row["padding"][0]), f(&row["padding"][1])]);
    }
    println!("straighten: largest difference {largest:e}");
}

#[test]
fn leaving_the_tool_applies_and_another_photo_discards() {
    let _turn = common::language_lock();
    let mut cx = context((1500, 1000), &serde_json::json!({}));
    cx.open_tool(grainy_app::panels::Tool::Crop);
    crop::sync(&mut cx);
    assert!(crop::is_open(&cx), "selecting the tool opens the frame");
    crop::choose_aspect(&mut cx, 1);
    let view = ViewTransform { rect: Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(750.0, 500.0)) };
    let mut tool = CropTool;
    crop::frame_arrived(&mut cx);
    let square = crop::session(&cx).unwrap().frame.crop_values();
    assert!((square[2] - square[0]) * 1500.0 - (square[3] - square[1]) * 1000.0 < 1e-6);
    tool.pointer_with_view(&event(PointerKind::Press, 375.0, 250.0, &view), &view, &mut cx);
    tool.pointer_with_view(&event(PointerKind::Release, 375.0, 250.0, &view), &view, &mut cx);
    cx.open_tool(grainy_app::panels::Tool::Edit);
    crop::sync(&mut cx);
    assert!(!crop::is_open(&cx));
    assert!(py_eq(cx.settings().value("crop"), &Value::from(square.to_vec())), "applied on leaving");
    assert_eq!(cx.edit_state().history().first().map(String::as_str), Some("보정 변경"));
    assert_eq!(crop::preview_crop(&mut cx), grainy_engine::Crop::Cropped);
    cx.open_tool(grainy_app::panels::Tool::Crop);
    crop::sync(&mut cx);
    assert_eq!(crop::preview_crop(&mut cx), grainy_engine::Crop::Uncropped);
    crop::straighten_slider(&mut cx, 4.0);
    assert_eq!(crop::preview_crop(&mut cx), grainy_engine::Crop::All);
}

// ---- snapshots ------------------------------------------------------------------------------------------------

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
    common::snapshot::settle(harness);
}

fn snapshot(language: Language, name: &str, setup: impl Fn(&mut GrainyApp)) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(name);
    let path = dir.join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    let mut harness = photo_harness(path);
    settle(&mut harness);
    setup(harness.state_mut());
    settle(&mut harness);
    let image = harness.render().expect("render the frame");
    common::snapshot::check(&image, name);
    grainy_i18n::set_language(Language::Korean);
}

/// The frame on a straightened photo: the enlarged canvas, the frame refitted inside the turned photo.
fn straightened(app: &mut GrainyApp) {
    crop::straighten_slider(app, 8.0);
    app.edit_state_mut().commit("보정 변경");
    assert!(crop::is_open(app));
}

/// The frame opened with the 2 : 3 aspect.
fn aspect(app: &mut GrainyApp) {
    app.run("crop.toggle");
    crop::frame_arrived(app);
    app.run_with("crop.aspect", &Invocation::value(5));
}

#[test]
fn crop_straightened_snapshots() {
    snapshot(Language::Korean, "crop-straightened-ko", straightened);
    snapshot(Language::English, "crop-straightened-en", straightened);
}

#[test]
fn crop_aspect_snapshots() {
    snapshot(Language::Korean, "crop-aspect-ko", aspect);
    snapshot(Language::English, "crop-aspect-en", aspect);
}
