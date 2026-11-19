//! Work package 9, masks by brush, colour and luminance ranges, sky: the session of `golden/app/masks2_*` (made
//! by `tools/golden/gen_app_masks2.py` running `StudioTools`' own methods, `pick_range` sampling the oracle's
//! develop of the `garden` input) replayed through the actions and through the brush's pointer, the `[` / `]`
//! command, the overlay of layers that read the developed image, and snapshots in Korean and English.

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Modifiers, Pos2, Rect};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::Invocation;
use grainy_app::context::AppContext;
use grainy_app::imagespace::ViewTransform;
use grainy_app::masks::{actions, MaskState, Mode, Operation};
use grainy_app::panels::Tool;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::tools::brush::{self, Brush};
use grainy_app::tools::range_pick::{self, RangeState};
use grainy_app::tools::{CanvasTool, PointerEvent, PointerKind};
use grainy_app::GrainyApp;
use grainy_core::{defaults, normalized};
use grainy_engine::files::Photo;
use grainy_engine::Source;
use grainy_i18n::Language;
use serde_json::{json, Value};

fn golden(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../golden/app/masks2_{name}/case.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("golden")).expect("json")
}

/// The source the oracle sampled: the standard `garden` input (scene-linear float32).
fn garden() -> grainy_core::Image {
    grainy_testkit::input("garden")
}

/// A context with one photo of `width` × `height` and the case's settings, the Masks tool open.
fn context(changes: &Value, width: u32, height: u32) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/masks2/photo.png")]);
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

/// JSON equal up to `tolerance` on floats; integers must stay integers.
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

/// The view the scripted pointer works in: the frame drawn at (100, 50), 4 screen points per pixel of the
/// 240 × 160 photo's map (so `p × 239` lands on the same pixel after the screen round trip).
fn view() -> ViewTransform {
    ViewTransform { rect: Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(956.0, 636.0)) }
}

fn event(kind: PointerKind, frame: [f64; 2], alt: bool) -> PointerEvent {
    let v = view();
    let screen = v.to_screen(frame);
    let modifiers = if alt { Modifiers::ALT } else { Modifiers::NONE };
    PointerEvent { kind, screen, frame: v.to_frame(screen), free: v.to_frame_free(screen), modifiers }
}

/// The points the view keeps (a point only when it moved).
fn kept(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    for p in points {
        if out.last() != Some(p) {
            out.push(*p);
        }
    }
    out
}

/// A stroke as the oracle's `press` gives it: through the actions, or through the brush's pointer.
fn press(cx: &mut AppContext, points: &[[f64; 2]], alt: bool, shift: bool, pointer: bool) {
    let points = kept(points);
    let mode = cx.state::<MaskState>().mode;
    if mode.picking() {
        let add = mode == Mode::ColorRangeAdd || shift;
        let source = garden();
        let (h, w, _) = source.dim();
        range_pick::pick_with(cx, &points, mode, add, |pick| {
            range_pick::sample(
                Source::Linear(&source),
                &pick.settings,
                &pick.points,
                pick.layer,
                Some((w as f64, h as f64)),
            )
        });
        return;
    }
    if !pointer {
        for n in 1..=points.len() {
            brush::preview_stroke(cx, &points[..n], alt);
        }
        brush::stroke(cx, &points, alt);
        return;
    }
    let mut tool = Brush;
    if points.len() == 1 {
        assert!(tool.pointer(&event(PointerKind::Click, points[0], alt), cx));
        return;
    }
    assert!(tool.pointer(&event(PointerKind::Press, points[0], alt), cx));
    for p in &points[1..] {
        assert!(tool.pointer(&event(PointerKind::Drag, *p, alt), cx));
    }
    assert!(tool.pointer(&event(PointerKind::Release, *points.last().unwrap(), alt), cx));
}

fn step(cx: &mut AppContext, name: &str, pointer: bool) {
    fn options(cx: &mut AppContext) -> &mut grainy_app::masks::ToolOptions {
        &mut cx.state::<MaskState>().options
    }
    match name {
        "create_color" | "create_color_2" => actions::create_mask(cx, "color_range"),
        "pick_color_click" => press(cx, &[[0.3, 0.4]], false, false, pointer),
        "add_mode" | "add_mode_refused" => range_pick::range_add_mode(cx),
        "pick_color_add_rect" => press(cx, &[[0.1, 0.1], [0.2, 0.3], [0.55, 0.6]], false, false, pointer),
        "back_to_color" => actions::mode(cx, Mode::ColorRange),
        "pick_color_shift_full" => press(cx, &[[0.7, 0.2], [0.9, 0.5]], false, true, pointer),
        "tolerance" => {
            range_pick::range_changed(cx, "tolerance", 0.55);
            cx.edit_state_mut().commit("보정 변경");
        }
        "remove_sample" => {
            cx.state::<RangeState>().sample = 1;
            range_pick::remove_range_sample(cx);
        }
        "pick_color_shift" => press(cx, &[[0.45, 0.8]], false, true, pointer),
        "tolerance_option" => options(cx).tolerance = 64,
        "pick_color_new" => press(cx, &[[0.8, 0.85], [0.95, 0.95]], false, false, pointer),
        "extend_luma" => actions::extend_mask(cx, "luma_range", Operation::Intersect),
        "pick_luma" => press(cx, &[[0.2, 0.2], [0.6, 0.7]], false, false, pointer),
        "range_low" => range_pick::range_changed(cx, "low", 30.0),
        "range_high_below" => range_pick::range_changed(cx, "high", 20.0),
        "range_softness" => {
            range_pick::range_changed(cx, "softness", 25.0);
            cx.edit_state_mut().commit("보정 변경");
        }
        "layer_exposure" => {
            actions::local_changed(cx, "exposure", 0.8);
            cx.edit_state_mut().commit("보정 변경");
        }
        "create_luma" => actions::create_mask(cx, "luma_range"),
        "pick_luma_after_layer" => press(cx, &[[0.5, 0.5]], false, false, pointer),
        "pick_color_after_layers" => press(cx, &[[0.15, 0.6], [0.4, 0.9]], false, false, pointer),
        "end_pick" | "end_brush" => actions::set_tool(cx, Mode::None),
        "create_sky" => actions::create_mask(cx, "sky"),
        "sky_sensitivity" => range_pick::sky_changed(cx, "sensitivity", 70.0),
        "sky_feather" => {
            range_pick::sky_changed(cx, "feather", 12.0);
            cx.edit_state_mut().commit("보정 변경");
        }
        "create_brush" | "create_brush_ranged" => actions::create_mask(cx, "brush"),
        "brush" => press(cx, &[[0.2, 0.2], [0.3, 0.25], [0.4, 0.3]], false, false, pointer),
        "brush_alt" => press(cx, &[[0.25, 0.22], [0.35, 0.27]], true, false, pointer),
        "brush_options" => {
            let o = options(cx);
            o.radius = 50;
            o.feather = 30;
            o.flow = 40;
            o.density = 60;
        }
        "brush_with_options" => press(cx, &[[0.6, 0.6], [0.62, 0.7], [0.7, 0.75], [0.7, 0.75]], false, false, pointer),
        "range_shadows" => brush::set_brush_range(cx, "shadows"),
        "brush_in_range" => press(cx, &[[0.1, 0.8], [0.2, 0.85]], false, false, pointer),
        "brush_alt_in_range" => press(cx, &[[0.12, 0.8]], true, false, pointer),
        "range_highlights" => brush::set_brush_range(cx, "highlights"),
        "range_all" => brush::set_brush_range(cx, "all"),
        "auto_mask" => {
            let o = options(cx);
            o.auto_mask = true;
            o.auto_tolerance = 25;
        }
        "brush_auto" => press(cx, &[[0.5, 0.3], [0.55, 0.35]], false, false, pointer),
        "auto_mask_off" => options(cx).auto_mask = false,
        "range_midtones_first" => brush::set_brush_range(cx, "midtones"),
        "brush_ranged_first" => press(cx, &[[0.4, 0.4], [0.5, 0.5]], false, false, pointer),
        "brush_ranged_second" => press(cx, &[[0.45, 0.6]], false, false, pointer),
        "subtract_brush" => actions::extend_mask(cx, "brush", Operation::Subtract),
        "brush_subtract_in_range" => press(cx, &[[0.3, 0.3], [0.35, 0.3]], false, false, pointer),
        "bigger" | "bigger_again" => {
            // `[` / `]` run the shortcut table's command.
            assert!(cx.run_with("mask.brush_size", &Invocation::key("]")));
        }
        "smaller" => {
            assert!(cx.run_with("mask.brush_size", &Invocation::key("[")));
        }
        "smaller_many" => {
            for _ in 0..30 {
                brush::resize_brush(cx, false);
            }
        }
        "largest" => {
            for _ in 0..40 {
                brush::resize_brush(cx, true);
            }
        }
        "brush_after_resize" => press(cx, &[[0.8, 0.2]], false, false, pointer),
        "resize_without_brush" => assert!(!brush::resize_brush(cx, true)),
        "add_luma" => range_pick::add_luma(cx, 25, 75),
        other => panic!("unknown step {other}"),
    }
    // What the panel does every frame: the brush range follows the selected mask (`load_layer`).
    brush::sync_brush_range(cx);
}

fn replay(pointer: bool) {
    for name in ["plain", "straighten"] {
        let case = golden(name);
        let (w, h) = (case["width"].as_u64().unwrap() as u32, case["height"].as_u64().unwrap() as u32);
        let mut cx = context(&case["changes"], w, h);
        let mut worst = 0.0f64;
        for expected in case["steps"].as_array().unwrap() {
            let name_of = expected["step"].as_str().unwrap();
            let label = format!("{name} {name_of}{}", if pointer { " (pointer)" } else { "" });
            step(&mut cx, name_of, pointer);
            close(cx.settings().value("masks"), &expected["masks"], 1e-5, &label, &mut worst);
            let state = cx.state::<MaskState>();
            assert_eq!(state.layer as i64, expected["layer"].as_i64().unwrap(), "{label} layer");
            assert_eq!(state.component as i64, expected["component"].as_i64().unwrap(), "{label} component");
            assert_eq!(state.mode.as_str(), expected["mode"].as_str().unwrap(), "{label} mode");
            assert_eq!(state.options.brush_range, expected["brush_range"].as_str().unwrap(), "{label} range");
            assert_eq!(state.options.radius, expected["radius"].as_i64().unwrap(), "{label} radius");
            assert!(state.pending.is_none(), "{label}: no stroke left pending");
            let radius = brush::view_radius(&cx);
            assert!((radius - expected["view_radius"].as_f64().unwrap()).abs() < 1e-12, "{label} view radius");
            assert_eq!(cx.state::<RangeState>().note, expected["note"].as_str().unwrap(), "{label} note");
            let mut history: Vec<String> = cx.edit_state().history().to_vec();
            history.reverse();
            let labels: Vec<String> =
                expected["history"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            assert_eq!(history, labels, "{label} history");
        }
        println!("masks2_{name}{}: largest difference {worst:e}", if pointer { " (pointer)" } else { "" });
    }
}

#[test]
fn session_matches_the_oracle() {
    let _turn = common::language_lock();
    replay(false);
}

#[test]
fn brush_strokes_through_the_pointer_match_the_oracle() {
    let _turn = common::language_lock();
    replay(true);
}

#[test]
fn a_brush_stroke_shows_live_and_esc_cancels() {
    let _turn = common::language_lock();
    let mut cx = context(&json!({}), 240, 160);
    actions::mask_shortcut(&mut cx, "brush");
    let mut tool = Brush;
    assert!(tool.active(&cx));
    let before = cx.settings().value("masks").clone();
    assert!(tool.pointer(&event(PointerKind::Press, [0.3, 0.3], false), &mut cx));
    assert!(tool.pointer(&event(PointerKind::Drag, [0.4, 0.35], false), &mut cx));
    let live = cx.settings().value("masks").clone();
    assert_eq!(live[0]["components"][0]["points"].as_array().unwrap().len(), 2, "the stroke shows while painted");
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert_eq!(cx.settings().value("masks"), &before, "Esc puts the masks back");
    assert_eq!(cx.state::<MaskState>().mode, Mode::Brush);
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert_eq!(cx.state::<MaskState>().mode, Mode::None);
    assert!(!tool.active(&cx));
    // A press off the photo is the view's (pan), not a stroke.
    actions::mask_shortcut(&mut cx, "brush");
    assert!(!tool.pointer(&event(PointerKind::Press, [1.2, 0.5], false), &mut cx));
}

#[test]
fn a_range_pick_through_the_pointer_starts_its_job() {
    let _turn = common::language_lock();
    let mut cx = context(&json!({}), 240, 160);
    actions::create_mask(&mut cx, "color_range");
    let mut tool = range_pick::RangePick;
    assert!(tool.active(&cx));
    assert!(tool.pointer(&event(PointerKind::Press, [0.2, 0.2], false), &mut cx));
    assert!(tool.pointer(&event(PointerKind::Drag, [0.4, 0.5], false), &mut cx));
    assert_eq!(cx.state::<MaskState>().points.len(), 2, "the rectangle is drawn");
    assert!(tool.pointer(&event(PointerKind::Release, [0.4, 0.5], false), &mut cx));
    assert!(cx.jobs.running("range_pick"), "the sampling runs on a job");
    assert_eq!(cx.state::<RangeState>().note, "마스크를 적용하기 전 색과 밝기를 읽는 중…");
    assert!(cx.state::<MaskState>().points.is_empty());
    // Esc ends the tool.
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert_eq!(cx.state::<MaskState>().mode, Mode::None);
}

#[test]
fn brush_size_keys_go_to_the_healing_brush_in_the_healing_tool() {
    let _turn = common::language_lock();
    let mut cx = context(&json!({}), 240, 160);
    actions::mask_shortcut(&mut cx, "brush");
    assert!(cx.run_with("mask.brush_size", &Invocation::key("]")));
    assert_eq!(cx.state::<MaskState>().options.radius, 30);
    cx.open_tool(Tool::Healing);
    grainy_app::tools::healing::sync(&mut cx);
    let healing_before = cx.state::<grainy_app::tools::healing::HealingState>().radius;
    assert!(cx.run_with("mask.brush_size", &Invocation::key("]")));
    assert_eq!(cx.state::<MaskState>().options.radius, 30, "the mask brush keeps its size");
    assert_ne!(cx.state::<grainy_app::tools::healing::HealingState>().radius, healing_before);
}

#[test]
fn image_overlays_read_the_developed_layer_input() {
    let source = Photo::Linear(garden());
    let mut s = normalized(&defaults().0);
    s.set(
        "masks",
        json!([{"name": "마스크 1", "enabled": true, "opacity": 1, "adjustments": {}, "components": [
            {"type": "luma", "range": [0.6, 1.0], "softness": 0.1, "operation": "add"}]}]),
    );
    let picture = range_pick::compute_image_overlay(&source, &s, 0, None).unwrap().expect("a range overlay");
    let (h, w, _) = garden().dim();
    assert_eq!((picture.width, picture.height), (w, h));
    // The overlay is the range's coverage of the developed picture: bright where the picture is bright.
    let image = range_pick::layer_input(source.source(), &s, 0, None).unwrap();
    let luma = |x: usize, y: usize| 0.2126 * image[[y, x, 0]] + 0.7152 * image[[y, x, 1]] + 0.0722 * image[[y, x, 2]];
    let alpha = |x: usize, y: usize| picture.rgba[(y * w + x) * 4 + 3];
    let (mut bright, mut dark) = ((0, 0), (0, 0));
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            if luma(x, y) > 0.8 {
                bright = (bright.0 + u32::from(alpha(x, y)), bright.1 + 1);
            } else if luma(x, y) < 0.4 {
                dark = (dark.0 + u32::from(alpha(x, y)), dark.1 + 1);
            }
        }
    }
    assert!(bright.1 > 0 && dark.1 > 0);
    assert_eq!(dark.0, 0, "no coverage below the range");
    assert!(bright.0 / bright.1 > 90, "full coverage above it: {}", bright.0 / bright.1);
    // A sky layer is detected on the colour source; a later layer starts from the earlier layers' result.
    s.set(
        "masks",
        json!([{"enabled": true, "opacity": 1, "adjustments": {"exposure": 1.0}, "components": [{"type": "sky", "sensitivity": 50.0, "feather": 30.0}]},
               {"enabled": true, "opacity": 1, "adjustments": {}, "components": [{"type": "color", "rgb": [0.5, 0.5, 0.5], "samples": [[0.5, 0.5, 0.5]], "tolerance": 0.3}]}]),
    );
    assert!(range_pick::compute_image_overlay(&source, &s, 0, None).unwrap().is_some());
    let first = range_pick::layer_input(source.source(), &s, 0, None).unwrap();
    let second = range_pick::layer_input(source.source(), &s, 1, None).unwrap();
    assert_ne!(first, second, "layer 1 reads layer 0's exposure");
    assert!(range_pick::compute_image_overlay(&source, &s, 1, None).unwrap().is_some());
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
    let dir = common::temp_dir("masks2-snapshot");
    let path = dir.join("garden-masks2.png");
    common::save_png(&common::garden(6, [1.0; 3], false), &path);
    path
}

/// The Develop screen with the Masks tool open on a generated photo; `setup` runs once the photo shows, `after`
/// once what it started (picks, overlays) has finished.
fn snapshot(language: Language, name: &str, setup: impl FnOnce(&mut AppContext), after: impl FnOnce(&mut AppContext)) {
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
    after(harness.state_mut());
    settle(&mut harness);
    let image = harness.render().expect("render the frame");
    common::snapshot::check(&image, name);
    grainy_i18n::set_language(Language::Korean);
}

fn painted(cx: &mut AppContext) {
    actions::create_mask(cx, "brush");
    cx.state::<MaskState>().options.radius = 60;
    brush::set_brush_range(cx, "shadows");
    actions::local_changed(cx, "exposure", 1.2);
    for stroke in [[[0.15, 0.3], [0.45, 0.35], [0.7, 0.3]], [[0.2, 0.6], [0.5, 0.65], [0.8, 0.6]]] {
        for n in 1..=stroke.len() {
            brush::preview_stroke(cx, &stroke[..n], false);
        }
        brush::stroke(cx, &stroke, false);
    }
}

#[test]
fn snapshot_brush() {
    for (language, name) in [(Language::Korean, "masks2-brush-ko"), (Language::English, "masks2-brush-en")] {
        snapshot(language, name, painted, |_| {});
    }
}

fn picked(cx: &mut AppContext) {
    actions::create_mask(cx, "color_range");
    range_pick::stroke(cx, &[[0.15, 0.12]], false);
}

fn shift_pick(cx: &mut AppContext) {
    range_pick::stroke(cx, &[[0.62, 0.76]], true);
}

#[test]
fn snapshot_color_range() {
    for (language, name) in [(Language::Korean, "masks2-range-ko"), (Language::English, "masks2-range-en")] {
        snapshot(language, name, picked, shift_pick);
    }
}

#[test]
fn snapshot_sky() {
    for (language, name) in [(Language::Korean, "masks2-sky-ko"), (Language::English, "masks2-sky-en")] {
        snapshot(
            language,
            name,
            |cx| {
                actions::create_mask(cx, "sky");
                range_pick::sky_changed(cx, "sensitivity", 65.0);
                cx.edit_state_mut().commit("보정 변경");
            },
            |_| {},
        );
    }
}
