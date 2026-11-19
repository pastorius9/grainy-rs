//! Work package 8, masks: the mask model and the gradients against the oracle (`golden/app/masks_*`, made by
//! `tools/golden/gen_app.py` running `StudioTools`' own methods), the same session driven through the canvas tool
//! with scripted pointer events (no window), and snapshots of the Masks panel with a radial and a linear gradient
//! and the overlay, in Korean and English.

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Modifiers, Pos2, Rect};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::context::AppContext;
use grainy_app::imagespace::ViewTransform;
use grainy_app::masks::{actions, gradient, overlay, MaskState, Mode, Operation};
use grainy_app::panels::Tool;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::tools::masks::Masks;
use grainy_app::tools::{CanvasTool, PointerEvent, PointerKind};
use grainy_app::GrainyApp;
use grainy_core::{defaults, normalized};
use grainy_i18n::Language;
use serde_json::{json, Value};

fn golden(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../golden/app/masks_{name}/case.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("golden")).expect("json")
}

/// A context with one photo of `width` × `height` and the case's settings, nothing committed yet.
fn context(changes: &Value, width: u32, height: u32) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/masks/photo.png")]);
    let id = session.current_id();
    session.set_original(id, (width, height));
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    let mut map = defaults().0.clone();
    for (k, v) in changes.as_object().unwrap() {
        map.insert(k.clone(), v.clone());
    }
    *cx.edit_state_mut() = EditState::new(normalized(&map));
    cx.open_tool(Tool::Masks);
    cx
}

/// JSON equal up to `tolerance` on floats; integers must stay integers (the oracle stores `feather` 70, not 70.0).
fn close(a: &Value, b: &Value, tolerance: f64, at: &str, worst: &mut f64) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            assert_eq!(x.is_f64(), y.is_f64(), "{at}: {x} vs {y} (integer or float)");
            let d = (x.as_f64().unwrap() - y.as_f64().unwrap()).abs();
            *worst = worst.max(d);
            assert!(d <= tolerance, "{at}: {x} vs {y}");
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{at}: {a} vs {b}");
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                close(p, q, tolerance, &format!("{at}[{i}]"), worst);
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            let mut kx: Vec<&String> = x.keys().collect();
            let mut ky: Vec<&String> = y.keys().collect();
            kx.sort();
            ky.sort();
            assert_eq!(kx, ky, "{at}");
            for (k, v) in x {
                close(v, &y[k], tolerance, &format!("{at}.{k}"), worst);
            }
        }
        _ => assert_eq!(a, b, "{at}"),
    }
}

fn masks_of(cx: &AppContext) -> Value {
    cx.settings().value("masks").clone()
}

/// One step of the oracle's script through the actions (the `StudioTools` methods).
fn action_step(cx: &mut AppContext, step: &str) {
    let drag = |cx: &mut AppContext, index: usize, start: Option<[f64; 2]>, end: [f64; 2]| {
        let handles = cx.state::<MaskState>().handles.clone();
        let start = start.unwrap_or(handles[index.min(1)]);
        let values = gradient::dragged(&handles, index, start, end);
        if !gradient::allclose(&values, &handles) {
            actions::move_component_handles(cx, &values);
        }
    };
    match step {
        "create_linear" => actions::create_mask(cx, "linear"),
        "draw_linear" => actions::stroke(cx, &[[0.2, 0.3], [0.8, 0.6]], false),
        "extend_radial_subtract" => actions::extend_mask(cx, "radial", Operation::Subtract),
        "draw_radial" => actions::stroke(cx, &[[0.3, 0.2], [0.7, 0.8]], false),
        "extend_radial_intersect" => actions::extend_mask(cx, "radial", Operation::Intersect),
        "draw_radial_off_photo" => actions::stroke(cx, &[[-0.1, 0.4], [0.5, 1.2]], false),
        "extend_linear_add" => actions::extend_mask(cx, "linear", Operation::Add),
        "draw_linear_off_photo" => actions::stroke(cx, &[[1.2, -0.1], [0.4, 0.5]], false),
        "draw_linear_click" => actions::stroke(cx, &[[0.5, 0.5], [0.5, 0.5]], false),
        "end_tool" | "end_edit" | "drop_fresh" => actions::set_tool(cx, Mode::None),
        "select_radial" | "select_turned_radial" => actions::tree_selected(cx, 0, 1),
        "drag_radial_corner" => drag(cx, 0, None, [0.25, 0.1]),
        "drag_radial_centre" => drag(cx, 2, Some([0.5, 0.5]), [0.55, 0.42]),
        "turn_radial" => actions::radial_turned(cx, 30.0),
        "drag_turned_corner" => drag(cx, 1, None, [0.8, 0.85]),
        "select_linear" => actions::tree_selected(cx, 0, 0),
        "drag_linear_end" => drag(cx, 1, None, [0.9, 0.2]),
        "drag_linear_centre" => drag(cx, 2, Some([0.5, 0.5]), [0.45, 0.55]),
        "opacity" => {
            actions::pause_overlay(cx);
            actions::layer_property(cx, "opacity", json!(0.5));
            cx.edit_state_mut().commit("보정 변경");
        }
        "invert" => {
            actions::layer_property(cx, "invert", json!(true));
            cx.edit_state_mut().commit("보정 변경");
        }
        "local_exposure" => {
            actions::local_changed(cx, "exposure", 0.75);
            cx.edit_state_mut().commit("보정 변경");
        }
        "local_saturation" => {
            actions::local_changed(cx, "saturation", -40.0);
            cx.edit_state_mut().commit("보정 변경");
        }
        "duplicate_layer" => actions::duplicate_layer(cx),
        "rename_layer" => {
            actions::rename_layer(cx, "하늘 쪽");
            cx.edit_state_mut().commit("보정 변경");
        }
        "move_layer_up" | "move_layer_up_again" => actions::move_layer(cx, -1),
        "hide_layer" => actions::set_layer_enabled(cx, 0, false),
        "select_component_2" => actions::select_component(cx, 2),
        "duplicate_component" => actions::duplicate_component(cx),
        "move_component_up" => actions::move_component(cx, -1),
        "move_component_down" => actions::move_component(cx, 1),
        "remove_component" => actions::remove_component(cx),
        "create_radial_then_end" => actions::create_mask(cx, "radial"),
        "create_sky" => actions::create_mask(cx, "sky"),
        "extend_sky_subtract" => actions::extend_mask(cx, "sky", Operation::Subtract),
        "create_brush_mode" => actions::create_mask(cx, "brush"),
        "reuse_fresh" => actions::mask_shortcut(cx, "linear"),
        "draw_in_fresh" => actions::stroke(cx, &[[0.1, 0.9], [0.3, 0.7]], false),
        "remove_layer" => actions::remove_layer(cx),
        "show_layer" => actions::set_layer_enabled(cx, 0, true),
        other => panic!("unknown step {other}"),
    }
}

/// The view the scripted pointer works in: the frame drawn at (100, 50), 970 × 650 screen points.
fn view() -> ViewTransform {
    ViewTransform { rect: Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(970.0, 650.0)) }
}

fn event(kind: PointerKind, frame: [f64; 2]) -> PointerEvent {
    let v = view();
    let screen = v.to_screen(frame);
    PointerEvent { kind, screen, frame: v.to_frame(screen), free: v.to_frame_free(screen), modifiers: Modifiers::NONE }
}

/// A drag of the pointer from `a` to `b` (frame fractions) through the tool.
fn pointer_drag(tool: &mut Masks, cx: &mut AppContext, a: [f64; 2], b: [f64; 2]) {
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    for (kind, p) in
        [(PointerKind::Press, a), (PointerKind::Drag, mid), (PointerKind::Drag, b), (PointerKind::Release, b)]
    {
        assert!(tool.pointer_at(&event(kind, p), view(), cx), "the tool takes {kind:?}");
    }
}

/// The same step through the canvas tool where the oracle's step is a pointer gesture.
fn pointer_step(tool: &mut Masks, cx: &mut AppContext, step: &str) {
    let handle_drag = |tool: &mut Masks, cx: &mut AppContext, index: usize, start: Option<[f64; 2]>, end: [f64; 2]| {
        let handles = cx.state::<MaskState>().handles.clone();
        let hits = gradient::hit_points(&handles);
        let from = hits[index];
        let to = match start {
            Some(start) => [from[0] + end[0] - start[0], from[1] + end[1] - start[1]],
            None => end,
        };
        pointer_drag(tool, cx, from, to);
    };
    match step {
        "draw_linear" => pointer_drag(tool, cx, [0.2, 0.3], [0.8, 0.6]),
        "draw_radial" => pointer_drag(tool, cx, [0.3, 0.2], [0.7, 0.8]),
        "draw_radial_off_photo" => pointer_drag(tool, cx, [-0.1, 0.4], [0.5, 1.2]),
        "draw_linear_off_photo" => pointer_drag(tool, cx, [1.2, -0.1], [0.4, 0.5]),
        "draw_linear_click" => {
            assert!(tool.pointer_at(&event(PointerKind::Click, [0.5, 0.5]), view(), cx));
        }
        "draw_in_fresh" => pointer_drag(tool, cx, [0.1, 0.9], [0.3, 0.7]),
        "drag_radial_corner" => handle_drag(tool, cx, 0, None, [0.25, 0.1]),
        "drag_radial_centre" => handle_drag(tool, cx, 2, Some([0.5, 0.5]), [0.55, 0.42]),
        "drag_turned_corner" => handle_drag(tool, cx, 1, None, [0.8, 0.85]),
        "drag_linear_end" => handle_drag(tool, cx, 1, None, [0.9, 0.2]),
        "drag_linear_centre" => handle_drag(tool, cx, 2, Some([0.5, 0.5]), [0.45, 0.55]),
        _ => action_step(cx, step),
    }
}

fn mode_name(cx: &mut AppContext) -> &'static str {
    cx.state::<MaskState>().mode.as_str()
}

#[test]
fn mask_session_matches_the_oracle() {
    let _turn = common::language_lock();
    for name in ["plain", "rotation_1", "straighten_crop"] {
        let case = golden(name);
        let mut cx = context(&case["changes"], 97, 65);
        let mut worst = 0.0f64;
        for step in case["steps"].as_array().unwrap() {
            let label = format!("{name} {}", step["step"].as_str().unwrap());
            action_step(&mut cx, step["step"].as_str().unwrap());
            close(&masks_of(&cx), &step["masks"], 1e-9, &label, &mut worst);
            let state = cx.state::<MaskState>();
            assert_eq!(
                (state.layer as i64, state.component as i64),
                (step["layer"].as_i64().unwrap(), step["component"].as_i64().unwrap()),
                "{label}"
            );
            let handles: Vec<Value> = state.handles.iter().map(|p| json!(p)).collect();
            close(&Value::Array(handles), &step["handles"], 1e-9, &format!("{label} handles"), &mut worst);
            assert!(
                (state.handle_angle - step["angle"].as_f64().unwrap()).abs() < 1e-9 || state.handles.is_empty(),
                "{label} angle"
            );
            assert_eq!(mode_name(&mut cx), step["mode"].as_str().unwrap(), "{label} mode");
            let mut history: Vec<String> = cx.edit_state().history().to_vec();
            history.reverse();
            let expected: Vec<String> =
                step["history"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            assert_eq!(history, expected, "{label} history");
        }
        println!("masks_{name}: largest difference {worst:e}");
    }
}

#[test]
fn pointer_gestures_give_the_oracles_components() {
    let _turn = common::language_lock();
    for name in ["plain", "rotation_1", "straighten_crop"] {
        let case = golden(name);
        let mut cx = context(&case["changes"], 97, 65);
        let mut tool = Masks::default();
        let mut worst = 0.0f64;
        for step in case["steps"].as_array().unwrap() {
            let label = format!("{name} {} (pointer)", step["step"].as_str().unwrap());
            pointer_step(&mut tool, &mut cx, step["step"].as_str().unwrap());
            close(&masks_of(&cx), &step["masks"], 1e-5, &label, &mut worst);
            let state = cx.state::<MaskState>();
            assert_eq!(state.layer as i64, step["layer"].as_i64().unwrap(), "{label}");
            assert_eq!(state.component as i64, step["component"].as_i64().unwrap(), "{label}");
            assert!(state.pending.is_none() && state.handle_drag.is_none(), "{label}");
        }
        println!("masks_{name} through the pointer: largest difference {worst:e}");
    }
}

#[test]
fn a_drag_shows_live_and_esc_cancels() {
    let _turn = common::language_lock();
    let mut cx = context(&json!({}), 97, 65);
    let mut tool = Masks::default();
    actions::mask_shortcut(&mut cx, "radial");
    assert_eq!(cx.state::<MaskState>().mode, Mode::Radial);
    let before = masks_of(&cx);
    assert!(tool.pointer_at(&event(PointerKind::Press, [0.3, 0.3]), view(), &mut cx));
    assert!(tool.pointer_at(&event(PointerKind::Drag, [0.6, 0.7]), view(), &mut cx));
    let live = masks_of(&cx);
    assert_eq!(live[0]["components"].as_array().unwrap().len(), 1, "the gradient shows while it is drawn");
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert_eq!(masks_of(&cx), before, "Esc puts the masks back");
    assert_eq!(cx.state::<MaskState>().mode, Mode::Radial, "the first Esc ends the stroke only");
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert_eq!(cx.state::<MaskState>().mode, Mode::None);
    assert!(masks_of(&cx).as_array().unwrap().is_empty(), "the unused new mask is dropped");
    // Undo after a drawn gradient goes back to no mask in one or two steps, never leaving a half stroke.
    actions::mask_shortcut(&mut cx, "linear");
    for (kind, p) in
        [(PointerKind::Press, [0.1, 0.1]), (PointerKind::Drag, [0.9, 0.9]), (PointerKind::Release, [0.9, 0.9])]
    {
        tool.pointer_at(&event(kind, p), view(), &mut cx);
    }
    assert_eq!(cx.edit_state().history()[0], "부분 보정");
    // In no mode the tool leaves the pointer to the view (pan).
    actions::set_tool(&mut cx, Mode::None);
    assert!(!tool.pointer_at(&event(PointerKind::Press, [0.5, 0.5]), view(), &mut cx));
}

#[test]
fn handles_hit_within_fourteen_pixels() {
    let handles = vec![[0.2, 0.3], [0.8, 0.6]];
    let v = view();
    let to = |p| v.to_screen(p);
    let at = |dx: f32, dy: f32, p: [f64; 2]| Pos2::new(to(p).x + dx, to(p).y + dy);
    assert_eq!(gradient::hit_handle(&handles, at(7.0, 7.0, [0.2, 0.3]), to), Some(0));
    assert_eq!(gradient::hit_handle(&handles, at(8.0, 7.0, [0.2, 0.3]), to), None);
    assert_eq!(gradient::hit_handle(&handles, at(-3.0, 2.0, [0.5, 0.45]), to), Some(2));
    assert_eq!(gradient::hit_handle(&handles, at(0.0, 0.0, [0.8, 0.6]), to), Some(1));
    assert_eq!(gradient::dragged(&handles, 2, [0.5, 0.5], [0.6, 0.4]), vec![[0.3, 0.2], [0.9, 0.5]]);
}

#[test]
fn overlay_is_the_layer_mask_through_the_geometry() {
    let mut s = defaults().clone();
    s.set("rotation", 1);
    s.set(
        "masks",
        json!([{"name": "마스크 1", "enabled": true, "opacity": 0.5, "components": [
        {"type": "linear", "points": [[0.0, 0.0], [1.0, 0.0]], "operation": "add"}], "adjustments": {}}]),
    );
    let input = overlay::OverlayInput { settings: s, layer: 0, source: (65, 97), image: None };
    let picture = overlay::compute(&input).unwrap().expect("a shape layer has an overlay");
    // A quarter turn: the frame is 65 wide and 97 high; the source's left→right ramp runs top→bottom.
    assert_eq!((picture.width, picture.height), (65, 97));
    let alpha = |x: usize, y: usize| picture.rgba[(y * picture.width + x) * 4 + 3];
    assert_eq!(&picture.rgba[..3], &overlay::RED);
    assert_eq!(alpha(30, 0), 0);
    assert_eq!(alpha(30, 96), 52, "opacity 0.5 × 105, truncated");
    assert!(alpha(30, 48) > 20 && alpha(30, 48) < 30);
    // A radial centred across with a vertical linear subtracted: the coverage is symmetric left and right.
    let mut s = defaults().clone();
    s.set(
        "masks",
        json!([{"opacity": 1, "components": [
        {"type": "radial", "points": [[0.25, 0.2], [0.75, 0.75]], "feather": 70},
        {"type": "linear", "points": [[0.5, 0.55], [0.5, 0.95]], "operation": "subtract"}]}]),
    );
    let input = overlay::OverlayInput { settings: s, layer: 0, source: (480, 720), image: None };
    let p = overlay::compute(&input).unwrap().unwrap();
    let a = |x: usize, y: usize| i32::from(p.rgba[(y * p.width + x) * 4 + 3]);
    for y in [100, 200, 260, 300, 340] {
        for d in [20, 80, 150] {
            assert!((a(359 - d, y) - a(360 + d, y)).abs() <= 1, "row {y}: {} vs {}", a(359 - d, y), a(360 + d, y));
        }
    }
    assert!(a(360, 200) > 60 && a(360, 400) == 0, "{} {}", a(360, 200), a(360, 400));
    // A range layer needs the developed image: no overlay without it.
    let mut s = defaults().clone();
    s.set("masks", json!([{"components": [{"type": "luma", "range": [0.2, 0.6]}]}]));
    let input = overlay::OverlayInput { settings: s, layer: 0, source: (65, 97), image: None };
    assert_eq!(overlay::compute(&input).unwrap(), None);
}

// ---- snapshots ---------------------------------------------------------------------------------------------

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

fn photo() -> PathBuf {
    let dir = common::temp_dir("masks-snapshot");
    let path = dir.join("garden-masks.png");
    common::save_png(&common::garden(6, [1.0; 3], false), &path);
    path
}

/// The Develop screen with the Masks tool open on a generated photo; `setup` runs once the photo shows.
fn masks_snapshot(language: Language, name: &str, setup: impl FnOnce(&mut AppContext)) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let path = photo();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![path]);
        app
    });
    settle(&mut harness);
    harness.state_mut().open_tool(Tool::Masks);
    settle(&mut harness);
    setup(harness.state_mut());
    settle(&mut harness);
    let image = harness.render().expect("render the frame");
    common::snapshot::check(&image, name);
    grainy_i18n::set_language(Language::Korean);
}

fn two_gradients(cx: &mut AppContext) {
    actions::create_mask(cx, "radial");
    actions::stroke(cx, &[[0.25, 0.2], [0.7, 0.75]], false);
    actions::extend_mask(cx, "linear", Operation::Subtract);
    actions::stroke(cx, &[[0.5, 0.55], [0.5, 0.95]], false);
    actions::local_changed(cx, "exposure", 1.2);
    cx.edit_state_mut().commit("보정 변경");
    // 완료: the drawing tool ends, so selecting a gradient shows its handles.
    actions::mode(cx, Mode::None);
}

#[test]
fn snapshot_radial_with_overlay() {
    masks_snapshot(Language::Korean, "masks-radial-ko", |cx| {
        two_gradients(cx);
        actions::tree_selected(cx, 0, 0);
    });
    masks_snapshot(Language::English, "masks-radial-en", |cx| {
        two_gradients(cx);
        actions::tree_selected(cx, 0, 0);
    });
}

#[test]
fn snapshot_linear_handles() {
    masks_snapshot(Language::Korean, "masks-linear-ko", |cx| {
        two_gradients(cx);
        actions::tree_selected(cx, 0, 1);
    });
    masks_snapshot(Language::English, "masks-linear-en", |cx| {
        two_gradients(cx);
        actions::tree_selected(cx, 0, 1);
    });
}

#[test]
fn snapshot_empty_panel() {
    masks_snapshot(Language::Korean, "masks-empty-ko", |_| {});
}
