//! The compare and reference views (work package 17).
//!
//! * `before_settings` against the oracle's (`golden/app/compare_before_settings`).
//! * Scripted sessions without a window against the oracle's own `CompareLayout` driven on a stand-in window
//!   (`tools/golden/gen_app_compare.py`, `golden/app/compare_session`, `compare_fallback`): after every step the
//!   mode, the order and direction of the panes, the captions, the white frame, the photo beside the open one
//!   (`pick`, `activate_other`, `previous_id`), and the record and size the second picture is rendered from.
//! * The real window (egui_kittest on wgpu, the background worker with its second preview lane and the engine):
//!   before/after side by side and the two-photo mode one above the other, the survey and the reference window, in
//!   Korean and English; the second picture equals the engine's develop of the oracle's settings.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::document::Document;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::views::compare::{self, before_settings, Compare, CompareMode, SideLane};
use grainy_app::{AppContext, GrainyApp, Mode, Zoom};
use grainy_core::{normalized, Settings};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

/// Numbers equal within `1e-9`, everything else exactly.
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

/// The keys of `settings` that differ from the defaults (the goldens store settings this way).
fn changed(settings: &Settings) -> Value {
    let defaults = grainy_core::defaults();
    let mut out = Map::new();
    for (key, value) in &settings.0 {
        if defaults.0.get(key).is_none_or(|d| !same(d, value)) {
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

/// The defaults with `changes`, normalized (the goldens' `normalized({**defaults(), **changes})`).
fn with_defaults(changes: &Value) -> Settings {
    let mut map = grainy_core::defaults().0;
    for (key, value) in changes.as_object().unwrap() {
        map.insert(key.clone(), value.clone());
    }
    normalized(&map)
}

#[test]
fn before_settings_match_the_oracle() {
    let case = Case::open("app", "compare_before_settings");
    let cases = case.json()["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 5);
    let keys: Vec<String> = grainy_core::defaults().0.keys().cloned().collect();
    for item in cases {
        let settings = with_defaults(&item["changes"]);
        assert!(same(&changed(&settings), &item["changes"]), "input {}", item["changes"]);
        let before = before_settings(&settings);
        assert!(
            same(&changed(&before), &item["before"]),
            "before of {}: {} vs {}",
            item["changes"],
            changed(&before),
            item["before"]
        );
        let mut got: Vec<String> = before.0.keys().cloned().collect();
        got.sort();
        let mut want = keys.clone();
        want.sort();
        assert_eq!(got, want, "the same keys as the defaults");
    }
}

/// A context on a session of the golden's photos, `current` open (nothing tracked yet).
fn context(photos: &[Value], current: u64) -> AppContext {
    let mut session = Session::default();
    let ids: Vec<u64> = photos.iter().map(|p| p["id"].as_u64().unwrap()).collect();
    session.add(photos.iter().map(|p| PathBuf::from(p["path"].as_str().unwrap())));
    for (index, photo) in photos.iter().enumerate() {
        session.select(index);
        *Document::edit_mut(&mut session) = EditState::new(with_defaults(&photo["settings"]));
    }
    let at = ids.iter().position(|&i| i == current).unwrap();
    session.select(at);
    let cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    assert_eq!(
        cx.doc.photos().iter().map(|p| p.id).collect::<Vec<_>>(),
        ids,
        "the session numbers the photos as the golden"
    );
    cx
}

/// Makes the selection exactly `ids` (the open photo is among them in the scripts).
fn set_selection(cx: &mut AppContext, ids: &[u64]) {
    for index in 0..cx.doc.len() {
        let id = cx.doc.photos()[index].id;
        if cx.doc.is_selected(id) != ids.contains(&id) {
            cx.doc.toggle_selected(index);
        }
    }
}

fn mode_of(value: &Value) -> Option<CompareMode> {
    match value.as_str() {
        Some("before") => Some(CompareMode::Before),
        Some("photos") => Some(CompareMode::Photos),
        _ => None,
    }
}

fn step(cx: &mut AppContext, step: &[Value]) {
    let arg = step.get(1);
    match step[0].as_str().unwrap() {
        "toggle" => {
            assert!(cx.run("view.compare"));
        }
        "start" => compare::start(cx, mode_of(arg.unwrap())),
        "stop" => compare::stop(cx, arg.unwrap().as_bool().unwrap()),
        "mode" => compare::set_mode(cx, mode_of(arg.unwrap()).unwrap()),
        "vertical" => compare::set_vertical(cx, arg.unwrap().as_bool().unwrap()),
        "swap" => compare::swap(cx),
        "activate_other" => compare::activate_other(cx),
        "set" => cx.doc.edit_mut().set(arg.unwrap().as_str().unwrap(), step[2].clone()),
        "select" => {
            let ids: Vec<u64> = arg.unwrap().as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
            set_selection(cx, &ids);
        }
        "activate" => {
            let index = cx.doc.index_of(arg.unwrap().as_u64().unwrap()).unwrap();
            cx.select_photo(index);
        }
        "zoom" => cx.set_zoom(Zoom::Actual),
        other => panic!("unknown step {other}"),
    }
    compare::sync(cx); // the 400 ms timer
}

fn check(cx: &mut AppContext, want: &Value, what: &str) {
    let previous = cx.state::<compare::Previous>().id;
    let (main_caption, other_caption) = compare::captions(cx);
    let selection: Vec<u64> = cx.doc.selected_indices().iter().map(|&i| cx.doc.photos()[i].id).collect();
    let current = cx.doc.current_id();
    let status = cx.status().to_owned();
    let screen = cx.mode();
    let checked = cx.state::<Compare>().on;
    let s = cx.state::<Compare>();
    let opt = |v: Option<u64>| v.map_or(Value::Null, Value::from);
    assert_eq!(s.on, want["on"].as_bool().unwrap(), "{what}: on");
    assert_eq!(s.mode.key(), want["mode"].as_str().unwrap(), "{what}: mode");
    assert_eq!(s.swapped, want["swapped"].as_bool().unwrap(), "{what}: swapped");
    assert_eq!(s.vertical, want["vertical"].as_bool().unwrap(), "{what}: vertical");
    assert_eq!(opt(s.other_id), want["other_id"], "{what}: other photo");
    assert_eq!(Value::from(current), want["current_id"], "{what}: current photo");
    assert_eq!(opt(previous), want["previous_id"], "{what}: previous photo");
    assert_eq!(json!(selection), want["selection"], "{what}: selection");
    assert_eq!(s.main_first(), want["main_first"].as_bool().unwrap(), "{what}: order");
    assert_eq!(checked, want["checked"].as_bool().unwrap(), "{what}: 비교 button");
    assert_eq!(main_caption, want["main_caption"].as_str().unwrap(), "{what}: main caption");
    assert_eq!(other_caption, want["other_caption"].as_str().unwrap(), "{what}: other caption");
    assert_eq!(s.on && s.marked(), want["main_marked"].as_bool().unwrap(), "{what}: white frame");
    assert_eq!(s.full, want["full"].as_bool().unwrap(), "{what}: full size");
    assert_eq!(s.renders, want["renders"].as_u64().unwrap(), "{what}: renders");
    assert_eq!(status, want["status"].as_str().unwrap(), "{what}: status");
    if want["screen"] == json!(0) && want["on"] == json!(true) {
        assert_eq!(screen, Mode::Develop, "{what}: Develop screen");
    }
    if let Some(shown) = want["shown"].as_object().filter(|_| s.on) {
        let got = s.shown.as_ref().expect("a second picture was asked for");
        assert_eq!(Value::from(got.id), shown["id"], "{what}: rendered photo");
        assert_eq!(got.path, PathBuf::from(shown["path"].as_str().unwrap()), "{what}: path");
        assert_eq!(got.name, shown["name"].as_str().unwrap(), "{what}: name");
        assert!(
            same(&changed(&got.settings), &shown["settings"]),
            "{what}: rendered settings {} vs {}",
            changed(&got.settings),
            shown["settings"]
        );
        assert_eq!(got.full, shown["size"].is_null(), "{what}: size");
    }
}

fn replay(name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let case = Case::open("app", name);
    let golden = case.json();
    let photos = golden["photos"].as_array().unwrap();
    let mut cx = context(photos, golden["current"].as_u64().unwrap());
    cx.set_status("");
    for (n, item) in golden["steps"].as_array().unwrap().iter().enumerate() {
        let script = item["step"].as_array().unwrap();
        step(&mut cx, script);
        check(&mut cx, &item["state"], &format!("{name} step {n} {script:?}"));
    }
}

#[test]
fn compare_session_matches_the_oracle() {
    replay("compare_session");
}

#[test]
fn compare_fallback_matches_the_oracle() {
    replay("compare_fallback");
}

#[test]
fn double_clicks_close_the_comparison() {
    let _turn = common::language_lock();
    let photos: Vec<Value> =
        (1..=3).map(|i| json!({"id": i, "path": format!("Z:/compare/photo-{i}.png"), "settings": {}})).collect();
    let mut cx = context(&photos, 1);
    compare::start(&mut cx, Some(CompareMode::Photos));
    assert_eq!(cx.state::<Compare>().other_id, Some(2));
    // A tool on the main picture keeps the comparison; the second picture always closes it, showing that photo.
    cx.open_tool(grainy_app::panels::Tool::Crop);
    compare::double_click(&mut cx, false);
    assert!(cx.state::<Compare>().on);
    compare::double_click(&mut cx, true);
    assert!(!cx.state::<Compare>().on);
    assert_eq!(cx.doc.current_id(), 2);
    cx.open_tool(grainy_app::panels::Tool::Edit);
    compare::start(&mut cx, None);
    compare::double_click(&mut cx, false);
    assert!(!cx.state::<Compare>().on);
    assert_eq!(cx.doc.current_id(), 2);
    assert_eq!(cx.zoom(), Zoom::Fit);
}

#[test]
fn the_second_picture_follows_the_main_view() {
    use egui::{pos2, vec2, Rect};
    let main_area = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
    let other = Rect::from_min_size(pos2(406.0, 0.0), vec2(400.0, 300.0));
    // Fitted: the whole picture, centred.
    let fit = compare::follow(true, main_area, Rect::NOTHING, other, vec2(1500.0, 1000.0));
    assert!((fit.center() - other.center()).length() < 1e-3);
    assert!((fit.width() - 400.0 * 0.96).abs() < 1e-3);
    // Zoomed in on the main picture's top left quarter: the same part of a picture twice as large.
    let main_rect = Rect::from_min_size(pos2(-200.0, -150.0), vec2(1600.0, 1200.0));
    let rect = compare::follow(false, main_area, main_rect, other, vec2(3000.0, 2000.0));
    assert!((rect.width() - 1600.0).abs() < 1e-3, "the same displayed width");
    let fx = (other.center().x - rect.left()) / rect.width();
    let main_fx = (main_area.center().x - main_rect.left()) / main_rect.width();
    assert!((fx - main_fx).abs() < 1e-5);
    // Smaller than the view: centred.
    let small = compare::follow(
        false,
        main_area,
        Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0)),
        other,
        vec2(20.0, 10.0),
    );
    assert!((small.center() - other.center()).length() < 1e-3);
}

#[test]
fn the_second_lane_answers_each_slot_with_its_newest_request() {
    let dir = common::temp_dir("compare-lane");
    let paths = common::eight_photos(&dir);
    let mut worker = grainy_app::worker::Worker::new();
    use grainy_app::render::{Renderer, SideJob};
    let edits = common::settings(json!({"exposure": 0.7, "crop": [0.1, 0.2, 0.6, 0.7]}));
    for id in 1..=3 {
        worker.side(SideJob { slot: 7, id, path: paths[0].clone(), settings: edits.clone(), max_size: Some(1800) });
    }
    worker.side(SideJob { slot: 8, id: 4, path: paths[1].clone(), settings: edits.clone(), max_size: None });
    let started = Instant::now();
    let mut answers = Vec::new();
    while answers.len() < 2 || !worker.idle() {
        answers.extend(worker.poll_side());
        assert!(started.elapsed() < Duration::from_secs(120), "the lane did not answer");
        std::thread::sleep(Duration::from_millis(5));
    }
    answers.extend(worker.poll_side());
    let ids: Vec<(u64, u64)> = answers.iter().map(|f| (f.slot, f.id)).collect();
    assert!(ids.contains(&(8, 4)), "{ids:?}");
    assert!(ids.contains(&(7, 3)), "the newest of slot 7 is answered: {ids:?}");
    assert!(ids.len() <= 3, "replaced requests are not rendered: {ids:?}");
    let frame = answers.iter().find(|f| f.slot == 7 && f.id == 3).unwrap();
    let preview = frame.result.as_ref().expect("rendered");
    let (rgba, w, h) = common::engine_render(&paths[0], Some(1800), &edits);
    assert_eq!((preview.width, preview.height), (w, h));
    assert_eq!(preview.rgba, rgba, "the second lane renders as the engine");
    assert_eq!(worker.preview_renders(), 0, "the open photo's lane did nothing");
}

// ---- snapshots ----------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn photo_harness<'a>(paths: Vec<PathBuf>) -> Harness<'a, GrainyApp> {
    builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(paths);
        app
    })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) -> image::RgbaImage {
    common::snapshot::save(harness, name)
}

/// Three photos opened, the first edited (geometry and tone), settled.
fn three_photos(name: &str) -> (Harness<'static, GrainyApp>, Vec<PathBuf>) {
    let dir = common::temp_dir(name);
    let paths: Vec<PathBuf> = common::eight_photos(&dir).into_iter().take(3).collect();
    let mut harness = photo_harness(paths.clone());
    settle(&mut harness);
    harness.state_mut().select_photo(0);
    let edits = common::settings(json!({"exposure": 0.8, "contrast": 30, "saturation": 40, "straighten": 4.0}));
    harness.state_mut().edit_state_mut().replace(edits, "보정 변경");
    settle(&mut harness);
    (harness, paths)
}

#[test]
fn compare_before_after_side_by_side() {
    let _turn = common::language_lock();
    for (language, suffix) in [(Language::Korean, "ko"), (Language::English, "en")] {
        grainy_i18n::set_language(language);
        let (mut harness, paths) = three_photos(&format!("compare-before-{suffix}"));
        harness.key_press(egui::Key::C);
        settle(&mut harness);
        {
            let app = harness.state_mut();
            let state = app.state::<Compare>();
            assert!(state.on && state.mode == CompareMode::Before && !state.main_first());
        }
        save(&mut harness, &format!("compare-before-{suffix}"));
        // The second picture is the engine's develop of `before_settings` at 1800 pixels (the lane's pixels are
        // compared with the engine's in `the_second_lane_answers_each_slot_with_its_newest_request`).
        let settings = before_settings(harness.state().settings());
        let (_, w, h) = common::engine_render(&paths[0], Some(compare::SIZE), &settings);
        let app = harness.state();
        let lane = app.state_ref::<SideLane>().unwrap();
        let picture = lane.slot(compare::SLOT).and_then(|s| s.picture.as_ref()).expect("the second picture");
        assert_eq!(picture.pixels, [w, h]);
    }
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn compare_two_photos_stacked() {
    let _turn = common::language_lock();
    for (language, suffix) in [(Language::Korean, "ko"), (Language::English, "en")] {
        grainy_i18n::set_language(language);
        let (mut harness, _) = three_photos(&format!("compare-photos-{suffix}"));
        harness.state_mut().document_mut().toggle_selected(1);
        harness.key_press(egui::Key::C);
        settle(&mut harness);
        compare::set_vertical(harness.state_mut(), true);
        settle(&mut harness);
        {
            let app = harness.state_mut();
            let state = app.state::<Compare>();
            assert!(state.on && state.mode == CompareMode::Photos && state.other_id == Some(2) && state.main_first());
        }
        save(&mut harness, &format!("compare-photos-{suffix}"));
    }
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn survey_and_reference_windows() {
    let _turn = common::language_lock();
    for (language, suffix) in [(Language::Korean, "ko"), (Language::English, "en")] {
        grainy_i18n::set_language(language);
        let (mut harness, _) = three_photos(&format!("compare-survey-{suffix}"));
        for index in [1, 2] {
            harness.state_mut().document_mut().toggle_selected(index);
        }
        assert!(harness.state_mut().run("view.survey"));
        settle(&mut harness);
        save(&mut harness, &format!("compare-survey-{suffix}"));
        harness.state_mut().close_dialog("survey");
        assert!(harness.state_mut().run("workflow.reference"));
        settle(&mut harness);
        // The reference window's picture comes from the second lane (slot 2 for 참조 사진, 3 for the window that follows the open photo); on a busy
        // machine its answer can land after the window looked idle: wait for it, then let the window size itself.
        let started = std::time::Instant::now();
        while !harness
            .state()
            .cx
            .state_ref::<grainy_app::views::compare::SideLane>()
            .is_some_and(|lane| [2, 3].iter().filter_map(|&n| lane.slot(n)).any(|slot| slot.answered))
        {
            assert!(started.elapsed() < std::time::Duration::from_secs(120), "the reference picture did not arrive");
            harness.step();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        settle(&mut harness);
        save(&mut harness, &format!("compare-reference-{suffix}"));
    }
    grainy_i18n::set_language(Language::Korean);
}
