//! Work package 10, the mask fold-out 커브 · 색상 혼합 · 선택 색상 and the component dialog: the dialog's form and
//! `result_component` against `golden/app/masks3_dialog` (the oracle's own `MaskComponentDialog`, headless), the
//! fold-out session of `golden/app/masks3_local` (the oracle's `StudioTools` methods with the real widgets; picks
//! read the oracle's develop of a generated source) replayed through the fold-out's functions, the local coverage
//! and samples of `golden/app/masks3_coverage`, the pick tool and the dialog driven without a window, and
//! snapshots in Korean and English. Made by `tools/golden/gen_app_masks3.py`.

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Modifiers, Pos2, Rect};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::context::AppContext;
use grainy_app::dialogs::mask_component::{qt_round, ComponentForm, MaskComponentDialog, Row, OPERATIONS};
use grainy_app::imagespace::ViewTransform;
use grainy_app::masks::{actions, MaskState, Mode};
use grainy_app::panels::masks_local::{self, LocalPointTool, MaskLocal, HSL_TITLES, POINT_CONTROLS};
use grainy_app::panels::Tool;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::tools::{CanvasTool, PointerEvent, PointerKind};
use grainy_app::GrainyApp;
use grainy_core::{defaults, normalized};
use grainy_engine::Source;
use grainy_i18n::Language;
use grainy_testkit::Case;
use ndarray::{Array2, Array3};
use serde_json::{json, Value};

fn korean() -> std::sync::MutexGuard<'static, ()> {
    let turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    turn
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

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

// ---- the component dialog --------------------------------------------------------------------------------------

#[test]
fn qt_rounding_rounds_halves_away_from_zero() {
    assert_eq!(qt_round(2.125, 2), 2.13);
    assert_eq!(qt_round(-2.125, 2), -2.13);
    assert_eq!(qt_round(70.25, 1), 70.3);
    assert_eq!(qt_round(12.35, 1), 12.3, "12.35 is stored just below the half");
    assert_eq!(qt_round(9.95, 1), 9.9);
    assert_eq!(qt_round(99.95, 1), 100.0);
    assert_eq!(qt_round(0.5, 0), 1.0);
    assert_eq!(qt_round(-0.04, 1), -0.0);
}

#[test]
fn dialog_results_match_the_oracle() {
    let _turn = korean();
    let case = Case::open("app", "masks3_dialog");
    let meta = case.json();
    let mut worst = 0.0f64;
    for expected in meta["cases"].as_array().unwrap() {
        let name = expected["name"].as_str().unwrap();
        let component = expected["component"].as_object().unwrap();
        let mut form = ComponentForm::new(component);
        assert_eq!(expected["title"], "선택 영역 편집");
        // The rows as built: 종류, 선택 영역 사용, 이 선택 영역 반전, 결합, then the kind's own.
        let rows = expected["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 4 + form.rows.len(), "{name}: rows");
        assert_eq!(rows[0]["label"], "종류");
        assert_eq!(rows[0]["text"], form.kind_name, "{name}: kind");
        assert_eq!(rows[1]["check"], "선택 영역 사용");
        assert_eq!(rows[1]["checked"], form.enabled, "{name}: enabled");
        assert_eq!(rows[2]["check"], "이 선택 영역 반전");
        assert_eq!(rows[2]["checked"], form.invert, "{name}: invert");
        assert_eq!(rows[3]["label"], "결합");
        assert_eq!(rows[3]["choice"], OPERATIONS[form.operation].1, "{name}: operation");
        let titles: Vec<&str> = OPERATIONS.iter().map(|(t, _)| *t).collect();
        assert_eq!(rows[3]["items"], json!(titles));
        for (row, shown) in form.rows.iter().zip(&rows[4..]) {
            match row {
                Row::Number(i) => {
                    let field = &form.fields[*i];
                    assert_eq!(shown["label"], field.title.as_str(), "{name}: label");
                    assert_eq!(f(&shown["value"]), field.value, "{name}: {} shown", field.key);
                    assert_eq!(f(&shown["low"]), field.low, "{name}: {} low", field.key);
                    assert_eq!(f(&shown["high"]), field.high, "{name}: {} high", field.key);
                    assert_eq!(shown["decimals"].as_u64().unwrap() as usize, field.decimals, "{name}: decimals");
                }
                Row::AutoMask => {
                    assert_eq!(shown["check"], "시작점과 비슷한 색 안에서만", "{name}");
                    assert_eq!(shown["checked"], form.auto_mask.unwrap(), "{name}: auto mask");
                }
            }
        }
        // The user's edits, through the fields' own rounding and range.
        for (key, value) in expected["edits"].as_object().unwrap() {
            form.set(key, f(value));
        }
        let others = &expected["others"];
        if let Some(on) = others["enabled"].as_bool() {
            form.enabled = on;
        }
        if let Some(on) = others["invert"].as_bool() {
            form.invert = on;
        }
        if let Some(op) = others["operation"].as_str() {
            form.set_operation(op);
        }
        if let Some(on) = others["auto_mask"].as_bool() {
            form.auto_mask = Some(on);
        }
        for (key, value) in expected["values"].as_object().unwrap() {
            assert_eq!(form.value(key), Some(f(value)), "{name}: {key} after the edits");
        }
        assert_eq!(form.fields.len(), expected["values"].as_object().unwrap().len(), "{name}: fields");
        match form.result_component() {
            Ok(result) => {
                assert_eq!(expected["error"], "", "{name}: the oracle failed");
                close(&Value::Object(result), &expected["result"], 0.0, name, &mut worst);
            }
            Err(error) => assert!(!expected["error"].as_str().unwrap().is_empty(), "{name}: {error}"),
        }
    }
    println!("masks3_dialog: largest difference {worst:e}");
}

#[test]
fn a_brush_without_points_cannot_take_the_automatic_mask() {
    let component = json!({"type": "brush", "radius": 0.03, "seed": [0.1, 0.2]});
    let mut form = ComponentForm::new(component.as_object().unwrap());
    form.auto_mask = Some(true);
    // `result['points'][0]` is read even with a seed: KeyError, as in the oracle.
    assert!(form.result_component().is_err());
    form.auto_mask = Some(false);
    assert!(form.result_component().is_ok());
}

// ---- the fold-out session --------------------------------------------------------------------------------------

/// A context with one photo of the source's size and the case's settings, the Masks tool open.
fn context(settings: &Value, width: u32, height: u32) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/masks3/photo.png")]);
    let id = session.current_id();
    session.set_original(id, (width, height));
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    let mut map = defaults().0.clone();
    for (k, v) in settings.as_object().unwrap() {
        map.insert(k.clone(), v.clone());
    }
    *cx.edit_state_mut() = EditState::new(normalized(&map));
    cx.open_tool(Tool::Masks);
    cx
}

/// `slider.setValue(round(value * scale))`: the value is set only when the position moves.
fn slide_hsl(cx: &mut AppContext, column: usize, value: f64) {
    let (shown, enabled) = masks_local::hsl_shown(cx);
    if enabled && (value).round() != shown[column].round() {
        masks_local::hsl_changed(cx, column, value.round());
    }
}

fn slide_point(cx: &mut AppContext, key: &str, value: f64) {
    let i = POINT_CONTROLS.iter().position(|c| c.0 == key).unwrap();
    let scale = POINT_CONTROLS[i].4;
    let (shown, enabled) = masks_local::point_shown(cx);
    if enabled && (value * scale).round() != (shown[i] * scale).round() {
        masks_local::point_changed(cx, key, (value * scale).round() / scale);
    }
}

fn commit(cx: &mut AppContext) {
    cx.edit_state_mut().commit("보정 변경");
}

/// What the panel does when it loads a layer (`load_layer`'s fold-out part, every frame here).
fn load(cx: &mut AppContext) {
    actions::sync(cx);
    masks_local::load_points(cx);
}

fn step(cx: &mut AppContext, name: &str, source: &Array3<f32>) {
    let (h, w, _) = source.dim();
    let pick = |cx: &mut AppContext, point: [f64; 2]| {
        masks_local::pick_with(cx, point, |pick| {
            masks_local::sample(
                Source::Linear(source),
                &pick.settings.0,
                pick.point,
                pick.layer as usize,
                Some((w as f64, h as f64)),
            )
        });
    };
    let curve = |cx: &mut AppContext, points: &[[f64; 2]]| masks_local::curve_changed(cx, points);
    match name {
        "load" => load(cx),
        "curve_rgb" => curve(cx, &[[0.0, 0.0], [0.3, 0.4], [1.0, 1.0]]),
        "curve_rgb_more" => curve(cx, &[[0.0, 0.05], [0.3, 0.45], [1.0, 0.95]]),
        "curve_blue" => curve(cx, &[[0.0, 0.0], [0.5, 0.42], [1.0, 1.0]]),
        "layer_2_curve" => curve(cx, &[[0.0, 0.0], [0.6, 0.5], [1.0, 1.0]]),
        "curve_release"
        | "curve_blue_release"
        | "hsl_release"
        | "point_release"
        | "point_lightness_release"
        | "layer_2_release"
        | "layer_2_hsl_release"
        | "layer_2_curve_release" => commit(cx),
        "curve_channel_blue" => masks_local::state(cx).curve_channel = 3,
        "curve_channel_red" => masks_local::state(cx).curve_channel = 1,
        "curve_channel_rgb" => masks_local::state(cx).curve_channel = 0,
        "overlay_back" => actions::state(cx).overlay_paused = false,
        "hsl_channel_green" | "hsl_channel_green_again" => masks_local::state(cx).hsl_channel = 3,
        "hsl_channel_red" => masks_local::state(cx).hsl_channel = 0,
        "hsl_hue" => slide_hsl(cx, 0, 25.0),
        "hsl_lightness" => slide_hsl(cx, 2, -40.0),
        "hsl_saturation" | "hsl_same" => slide_hsl(cx, 1, 10.0),
        "hsl_reset" => {
            slide_hsl(cx, 1, 0.0);
            commit(cx);
        }
        "layer_2_hsl" => slide_hsl(cx, 0, -12.0),
        "pick_start" | "pick_start_refused" => masks_local::start(cx),
        "pick" => pick(cx, [0.2, 0.5]),
        "pick_far" => pick(cx, [0.95, 0.5]),
        "pick_outside_photo" => pick(cx, [1.2, 0.5]),
        "pick_second" => pick(cx, [0.1, 0.2]),
        "layer_2_pick" => pick(cx, [0.8, 0.6]),
        "layer_2_pick_outside_mask" => pick(cx, [0.05, 0.05]),
        "layer_3_pick_disabled" => pick(cx, [0.5, 0.5]),
        "point_range" => slide_point(cx, "range", 0.2),
        "point_hue" => slide_point(cx, "hue", 30.0),
        "point_saturation_range" => slide_point(cx, "saturation_range", 0.35),
        "point_lightness" => slide_point(cx, "lightness", -20.0),
        "point_reset_range" => {
            slide_point(cx, "range", 0.12);
            commit(cx);
        }
        "layer_2_point" => slide_point(cx, "saturation", 15.0),
        "select_first" => masks_local::state(cx).point_index = 0,
        "remove_first" | "layer_3_remove_nothing" | "remove_last" | "remove_none" => masks_local::remove_point(cx),
        "layer_2" => actions::tree_selected(cx, 1, -1),
        "layer_3" => actions::tree_selected(cx, 2, -1),
        "layer_1" => actions::tree_selected(cx, 0, -1),
        "no_layer" => actions::select_layer(cx, 5),
        "masks_gone" => {
            cx.edit_state_mut().set("masks", json!([]));
            commit(cx);
        }
        other => panic!("unknown step {other}"),
    }
    load(cx);
}

/// The steps after which the oracle's curve widget was loaded from the settings (`load_local_curve`); after a
/// `changed` emitted from outside the widget it keeps its own points.
fn curve_loaded(name: &str) -> bool {
    name == "load"
        || name.starts_with("curve_channel")
        || ["layer_1", "layer_2", "layer_3", "no_layer", "masks_gone"].contains(&name)
}

#[test]
fn local_session_matches_the_oracle() {
    let _turn = korean();
    let case = Case::open("app", "masks3_local");
    let meta = case.json();
    let source: Array3<f32> = case.array("source");
    let (h, w, _) = source.dim();
    let mut cx = context(&meta["settings"], w as u32, h as u32);
    cx.set_status("");
    let mut worst = 0.0f64;
    for expected in meta["steps"].as_array().unwrap() {
        let name = expected["step"].as_str().unwrap();
        step(&mut cx, name, &source);
        close(cx.settings().value("masks"), &expected["masks"], 2e-5, name, &mut worst);
        let mut history: Vec<String> = cx.edit_state().history().to_vec();
        history.reverse();
        let labels: Vec<String> =
            expected["history"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        assert_eq!(history, labels, "{name}: history");
        assert_eq!(actions::state(&mut cx).layer as i64, expected["layer"].as_i64().unwrap(), "{name}: layer");
        let picking = cx.state::<MaskLocal>().picking;
        let mode = if picking { "local_point_color" } else { actions::state(&mut cx).mode.as_str() };
        assert_eq!(mode, expected["mode"].as_str().unwrap(), "{name}: mode");
        assert_eq!(cx.status(), expected["message"].as_str().unwrap(), "{name}: status");
        assert_eq!(actions::state(&mut cx).overlay_paused, expected["overlay_paused"].as_bool().unwrap(), "{name}");
        let local = cx.state::<MaskLocal>();
        assert_eq!(local.curve_channel as u64, expected["curve_channel"].as_u64().unwrap(), "{name}: curve channel");
        assert_eq!(local.hsl_channel as u64, expected["hsl_channel"].as_u64().unwrap(), "{name}: colour");
        assert_eq!(local.point_index as i64, expected["index"].as_i64().unwrap(), "{name}: point index");
        assert_eq!(local.point_index as i64, expected["current"].as_i64().unwrap(), "{name}: point box");
        assert_eq!(local.note(), expected["note"].as_str().unwrap(), "{name}: note");
        if curve_loaded(name) {
            let points: Vec<Value> = masks_local::curve_points(&mut cx).iter().map(|p| json!([p[0], p[1]])).collect();
            close(&Value::Array(points), &expected["curve"], 0.0, &format!("{name} curve"), &mut worst);
        }
        let (hsl, enabled) = masks_local::hsl_shown(&mut cx);
        for (i, shown) in expected["hsl"].as_array().unwrap().iter().enumerate() {
            assert_eq!(hsl[i], f(&shown["value"]), "{name}: {}", HSL_TITLES[i]);
            assert_eq!(enabled, shown["enabled"].as_bool().unwrap(), "{name}: {} enabled", HSL_TITLES[i]);
        }
        let count = masks_local::point_values(&mut cx).len();
        let items: Vec<String> = (1..=count).map(|i| format!("부분 선택 색상 {i}")).collect();
        assert_eq!(json!(items), expected["items"], "{name}: points");
        let (values, enabled) = masks_local::point_shown(&mut cx);
        for (i, (key, ..)) in POINT_CONTROLS.iter().enumerate() {
            let shown = &expected["points"][key];
            assert!(
                (values[i] - f(&shown["value"])).abs() < 1e-12,
                "{name}: {key} {} vs {}",
                values[i],
                shown["value"]
            );
            assert_eq!(enabled, shown["enabled"].as_bool().unwrap(), "{name}: {key} enabled");
        }
        assert_eq!(enabled, expected["remove_enabled"].as_bool().unwrap(), "{name}: remove");
        assert_eq!(enabled, expected["preview_enabled"].as_bool().unwrap(), "{name}: preview");
        let has_layer = actions::current_layer(&mut cx).is_some();
        assert_eq!(has_layer, expected["add_enabled"].as_bool().unwrap(), "{name}: pick button");
    }
    println!("masks3_local: largest difference {worst:e}");
}

#[test]
fn local_coverage_and_samples_match_the_oracle() {
    let source: Array3<f32> = Case::open("app", "masks3_local").array("source");
    let case = Case::open("app", "masks3_coverage");
    let meta = case.json();
    let settings = meta["settings"].as_object().unwrap().clone();
    let mut largest_sample = 0f64;
    for expected in meta["samples"].as_array().unwrap() {
        let layer = expected["layer"].as_u64().unwrap() as usize;
        let p = &expected["point"];
        let result = masks_local::sample(Source::Linear(&source), &settings, [f(&p[0]), f(&p[1])], layer, None);
        match (result, expected["rgb"].as_array()) {
            (Ok(rgb), Some(want)) => {
                for c in 0..3 {
                    largest_sample = largest_sample.max((f64::from(rgb[c]) - f(&want[c])).abs());
                }
            }
            (Err(error), None) => assert_eq!(error, expected["error"].as_str().unwrap()),
            (other, _) => panic!("sample {expected}: {other:?}"),
        }
    }
    println!("sample: largest difference {largest_sample:e}");
    assert!(largest_sample <= 2e-5, "sample {largest_sample:e}");
    let mut largest = 0f32;
    for target in meta["targets"].as_array().unwrap() {
        let (layer, index) = (target[0].as_u64().unwrap() as usize, target[1].as_u64().unwrap() as usize);
        let expected: Array2<f32> = case.array(&format!("coverage_{layer}_{index}"));
        let actual = masks_local::preview(Source::Linear(&source), &settings, layer, index, None).unwrap();
        assert_eq!(actual.dim(), expected.dim(), "coverage {layer} {index}");
        let (diff, _) = grainy_testkit::max_abs_diff(&actual, &expected);
        println!("preview {layer} {index}: largest difference {diff:e}");
        largest = largest.max(diff);
    }
    assert!(largest <= 2e-4, "coverage {largest:e}");
    for (bad, message) in
        [([1.2, 0.5], "사진 안쪽의 색을 골라 주세요."), ([f64::NAN, 0.5], "사진 안쪽의 색을 골라 주세요.")]
    {
        assert_eq!(masks_local::sample(Source::Linear(&source), &settings, bad, 0, None).unwrap_err(), message);
    }
    assert!(masks_local::sample(Source::Linear(&source), &settings, [0.5, 0.5], 9, None).is_err());
}

// ---- driven without a window ------------------------------------------------------------------------------------

fn view() -> ViewTransform {
    ViewTransform { rect: Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(480.0, 320.0)) }
}

fn event(kind: PointerKind, frame: [f64; 2]) -> PointerEvent {
    let v = view();
    let screen = v.to_screen(frame);
    PointerEvent { kind, screen, frame: v.to_frame(screen), free: v.to_frame_free(screen), modifiers: Modifiers::NONE }
}

fn one_mask() -> Value {
    json!({"masks": [{"name": "마스크 1", "enabled": true, "opacity": 1, "adjustments": {}, "components": [
        {"type": "linear", "points": [[0.0, 0.5], [0.7, 0.5]], "radius": 0.025, "feather": 70, "operation": "add"},
        {"type": "radial", "points": [[0.2, 0.2], [0.6, 0.7]], "radius": 0.025, "feather": 50, "operation": "add"}]}]})
}

#[test]
fn the_pick_tool_takes_the_photo_until_esc() {
    let _turn = korean();
    let mut cx = context(&json!({}), 240, 160);
    let mut tool = LocalPointTool;
    // Without a mask the pick is refused.
    masks_local::start(&mut cx);
    assert!(!tool.active(&cx));
    assert_eq!(cx.status(), "먼저 마스크와 선택 영역을 추가하세요.");
    let mut cx = context(&one_mask(), 240, 160);
    masks_local::start(&mut cx);
    assert!(tool.active(&cx));
    assert_eq!(cx.status(), "선택한 마스크 안에서 보정할 색을 클릭하세요.");
    // A click off the photo is nothing; a press on it and its release start the reading job.
    assert!(tool.pointer(&event(PointerKind::Click, [1.3, 0.5]), &mut cx));
    assert!(!cx.jobs.running("local_point_color"));
    assert!(tool.pointer(&event(PointerKind::Press, [0.2, 0.5]), &mut cx));
    assert!(tool.pointer(&event(PointerKind::Release, [0.2, 0.5]), &mut cx));
    assert!(cx.jobs.running("local_point_color"), "the colour is read on a job");
    assert_eq!(cx.state::<MaskLocal>().note(), "보정 단계의 색을 읽는 중…");
    assert!(!tool.active(&cx), "the pick ends the tool");
    // A mask tool takes the view from the pick; Esc ends it.
    masks_local::start(&mut cx);
    actions::mode(&mut cx, Mode::Brush);
    masks_local::sync(&mut cx);
    assert!(!cx.state::<MaskLocal>().picking);
    actions::mode(&mut cx, Mode::None);
    masks_local::start(&mut cx);
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert!(!tool.active(&cx));
    // Another tool ends it too.
    masks_local::start(&mut cx);
    cx.open_tool(Tool::Edit);
    assert!(!tool.active(&cx));
}

#[test]
fn a_stale_pick_is_cancelled() {
    let _turn = korean();
    let mut cx = context(&one_mask(), 240, 160);
    let pick = masks_local::prepare(&mut cx, [0.2, 0.5]).unwrap();
    // The settings changed meanwhile: nothing is added and the note says so.
    actions::local_changed(&mut cx, "exposure", 0.5);
    masks_local::finish(&mut cx, &pick, Ok([0.5, 0.4, 0.3]));
    assert!(masks_local::point_values(&mut cx).is_empty());
    assert_eq!(cx.state::<MaskLocal>().note(), "대상이 바뀌어 색 선택을 취소했습니다. 다시 선택하세요.");
    let pick = masks_local::prepare(&mut cx, [0.2, 0.5]).unwrap();
    masks_local::finish(&mut cx, &pick, Ok([0.5, 0.4, 0.3]));
    assert_eq!(masks_local::point_values(&mut cx).len(), 1);
    assert_eq!(cx.edit_state().history()[0], "부분 선택 색상 추가");
}

#[test]
fn the_dialog_replaces_the_selected_component() {
    let _turn = korean();
    let mut cx = context(&one_mask(), 240, 160);
    // No component selected: 세부 편집… opens nothing.
    actions::edit_component(&mut cx);
    assert!(!cx.dialog_open("mask_component"));
    actions::tree_selected(&mut cx, 0, 1);
    actions::edit_component(&mut cx);
    assert!(cx.dialog_open("mask_component"), "세부 편집… opens the dialog");
    let mut dialog = MaskComponentDialog::for_selection(&mut cx).unwrap();
    assert_eq!(dialog.form.kind_name, "방사형 그레이디언트");
    dialog.form.set("angle", 30.04);
    dialog.form.set("feather", 20.0);
    dialog.form.set_operation("subtract");
    let before = cx.edit_state().history().len();
    dialog.accept(&mut cx);
    let component = &cx.settings().value("masks")[0]["components"][1];
    assert_eq!(component["angle"], json!(30.0));
    assert_eq!(component["feather"], json!(20.0));
    assert_eq!(component["operation"], "subtract");
    assert_eq!(cx.edit_state().history().len(), before + 1);
    assert_eq!(cx.edit_state().history()[0], "보정 변경");
    assert_eq!(actions::state(&mut cx).mode, Mode::None, "the tool ends");
    // A dialog whose component is no longer selected changes nothing.
    let stale = MaskComponentDialog::for_selection(&mut cx).unwrap();
    actions::tree_selected(&mut cx, 0, 0);
    let masks = cx.settings().value("masks").clone();
    stale.accept(&mut cx);
    assert_eq!(cx.settings().value("masks"), &masks);
}

#[test]
fn a_local_point_overlay_hides_the_red_one() {
    let _turn = korean();
    let mut settings = one_mask();
    settings["masks"][0]["adjustments"]["point_colors"] = json!([{"version": 2, "rgb": [0.5, 0.4, 0.3]}]);
    let mut cx = context(&settings, 240, 160);
    actions::sync(&mut cx);
    masks_local::load_points(&mut cx);
    let red = grainy_app::masks::overlay::overlay_layer(&cx, cx.state_ref::<MaskState>().unwrap());
    assert_eq!(red, Some(0));
    cx.state::<MaskLocal>().preview_on = true;
    assert_eq!(masks_local::overlay_target(&cx), Some((0, 0)));
    assert_eq!(grainy_app::masks::overlay::overlay_layer(&cx, cx.state_ref::<MaskState>().unwrap()), None);
    cx.set_before(true);
    assert_eq!(masks_local::overlay_target(&cx), None);
}

// ---- snapshots ---------------------------------------------------------------------------------------------

fn builder(height: f32) -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, height)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

/// The generated photo, in a folder with a fixed name (nothing that changes between runs shows).
fn photo() -> PathBuf {
    let dir = common::temp_dir("masks3-snapshot");
    let path = dir.join("garden-masks3.png");
    common::save_png(&common::garden(6, [1.0; 3], false), &path);
    path
}

/// The Develop screen with the Masks tool open on a generated photo; `setup` runs once the photo shows, `after`
/// once what it started (picks, overlays) has finished.
fn snapshot(
    language: Language,
    name: &str,
    height: f32,
    setup: impl FnOnce(&mut AppContext),
    after: impl FnOnce(&mut AppContext),
) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let path = photo();
    let mut harness = builder(height).wgpu().build_eframe(move |cc| {
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
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    common::snapshot::save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

fn local_edits(cx: &mut AppContext) {
    actions::create_mask(cx, "radial");
    actions::state(cx).points = vec![[0.25, 0.2]];
    actions::preview_stroke(cx, &[[0.25, 0.2]]);
    actions::preview_stroke(cx, &[[0.25, 0.2], [0.8, 0.85]]);
    actions::stroke(cx, &[[0.25, 0.2], [0.8, 0.85]], false);
    actions::set_tool(cx, Mode::None);
    cx.set_foldout("mask_local", true);
    masks_local::curve_changed(cx, &[[0.0, 0.0], [0.3, 0.42], [1.0, 1.0]]);
    masks_local::state(cx).hsl_channel = 3;
    masks_local::hsl_changed(cx, 0, 25.0);
    masks_local::hsl_changed(cx, 2, -30.0);
    cx.edit_state_mut().commit("보정 변경");
    actions::state(cx).overlay_paused = false;
    masks_local::pick(cx, [0.5, 0.5]);
}

fn show_range(cx: &mut AppContext) {
    masks_local::point_changed(cx, "hue", 35.0);
    cx.edit_state_mut().commit("보정 변경");
    masks_local::state(cx).preview_on = true;
}

#[test]
fn snapshot_local_foldout() {
    for (language, name) in [(Language::Korean, "masks3-local-ko"), (Language::English, "masks3-local-en")] {
        snapshot(language, name, 2250.0, local_edits, show_range);
    }
}

fn component_dialog(cx: &mut AppContext) {
    actions::create_mask(cx, "linear");
    actions::preview_stroke(cx, &[[0.1, 0.3]]);
    actions::preview_stroke(cx, &[[0.1, 0.3], [0.6, 0.7]]);
    actions::stroke(cx, &[[0.1, 0.3], [0.6, 0.7]], false);
    actions::set_tool(cx, Mode::None);
    actions::extend_mask(cx, "radial", grainy_app::masks::Operation::Subtract);
    actions::preview_stroke(cx, &[[0.4, 0.2]]);
    actions::preview_stroke(cx, &[[0.4, 0.2], [0.9, 0.8]]);
    actions::stroke(cx, &[[0.4, 0.2], [0.9, 0.8]], false);
    actions::set_tool(cx, Mode::None);
    actions::edit_component(cx);
}

#[test]
fn snapshot_component_dialog() {
    for (language, name) in [(Language::Korean, "masks3-dialog-ko"), (Language::English, "masks3-dialog-en")] {
        snapshot(language, name, 1000.0, component_dialog, |_| {});
    }
}
