//! Work package 4: the white-balance eyedropper and the film-base picker.
//!
//! * Model tests against goldens made with the oracle's own code (`tools/golden/gen_app_pickers.py`):
//!   the pointer through luma/widgets.py `PhotoView`'s sample mode (`pickers_gesture`), `MainWindow.sample_wb`,
//!   `sample_wb_area` and `sample_negative_base` on a display picture (`pickers_display*`), on camera sources of
//!   DNG files with crops, rotations and straightening (`pickers_raw_*`) and on film scans (`pickers_negative_*`),
//!   and the two buttons (`pickers_toggles`). No window: the model functions of `tools/eyedropper.rs` and an
//!   [`AppContext`] on an in-memory session.
//! * The window headless (egui_kittest on wgpu): `W`, a click and a dragged rectangle on a generated photo,
//!   a RAW photo and a film scan; snapshots in Korean and English (`tests/snapshots/pickers-*.png`).

mod common;

use std::path::PathBuf;

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::Invocation;
use grainy_app::imagespace::{CoordinateMap, ViewTransform};
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::tools::eyedropper::{self, Failure, Gesture, Picture, Sample};
use grainy_app::tools::{CanvasTool, PointerEvent, PointerKind};
use grainy_app::{AppContext, GrainyApp};
use grainy_core::{normalized, Settings};
use grainy_engine::files::{load_photo, Photo};
use grainy_i18n::Language;
use grainy_raw::rawcolor::CameraSource;
use grainy_testkit::Case;
use ndarray::{Array3, Ix3};
use serde_json::Value;

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn settings_with(changes: &Value) -> Settings {
    let mut s = grainy_core::defaults().clone();
    for (k, v) in changes.as_object().unwrap() {
        s.set(k, v.clone());
    }
    normalized(&s.0)
}

fn sample_of(step: &Value) -> Sample {
    match step[0].as_str().unwrap() {
        "click" => Sample::Point([f(&step[1]), f(&step[2])]),
        _ => Sample::Area([f(&step[1]), f(&step[2]), f(&step[3]), f(&step[4])]),
    }
}

fn is_sample(step: &Value) -> bool {
    matches!(step[0].as_str().unwrap(), "click" | "area")
}

/// The numbers of a JSON list.
fn numbers(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(f).collect()
}

/// Compares the settings an oracle step recorded with the edit's, key by key; returns the largest difference.
fn compare_settings(edit: &EditState, expected: &Value, tolerance: f64, what: &str) -> f64 {
    let mut largest = 0f64;
    for (key, want) in expected.as_object().unwrap() {
        let got = edit.settings().value(key);
        match (got, want) {
            (Value::Array(_), Value::Array(_)) => {
                let (a, b) = (numbers(got), numbers(want));
                assert_eq!(a.len(), b.len(), "{what}: {key}");
                for (x, y) in a.iter().zip(&b) {
                    let e = (x - y).abs() / y.abs().max(1.0);
                    assert!(e <= tolerance, "{what}: {key}: {got} vs {want}");
                    largest = largest.max(e);
                }
            }
            (Value::Number(_), Value::Number(_)) => {
                let (x, y) = (f(got), f(want));
                let e = (x - y).abs() / y.abs().max(1.0);
                assert!(e <= tolerance, "{what}: {key}: {x} vs {y}");
                largest = largest.max(e);
            }
            _ => assert_eq!(got, want, "{what}: {key}"),
        }
    }
    largest
}

/// The status text and undo labels an oracle step produced, against what the model did.
fn compare_outcome(statuses: &[&str], labels_added: &[String], expected: &Value, what: &str) {
    let want_status: Vec<&str> = expected["status"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    assert_eq!(statuses, want_status.as_slice(), "{what}: status");
    let want_labels: Vec<&str> =
        expected["commits"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    let got: Vec<&str> = labels_added.iter().map(String::as_str).collect();
    assert_eq!(got, want_labels, "{what}: undo labels");
}

/// History labels added since `before` entries, oldest first.
fn added(edit: &EditState, before: usize) -> Vec<String> {
    let h = edit.history();
    h[..h.len() - before].iter().rev().cloned().collect()
}

// ---- the pointer ---------------------------------------------------------------------------------------------------

/// `PhotoView` in the sample mode: the same presses, moves, releases and Esc at the same screen points give the same
/// clicks, rectangles and cancellations.
#[test]
fn pointer_gestures_match_the_oracle() {
    let _turn = common::language_lock();
    let case = Case::open("app", "pickers_gesture");
    let meta = case.json();
    let r = numbers(&meta["photo_rect"]);
    let view = ViewTransform {
        rect: Rect::from_min_max(Pos2::new(r[0] as f32, r[1] as f32), Pos2::new(r[2] as f32, r[3] as f32)),
    };
    assert_eq!(f(&meta["drag"]) as f32, eyedropper::SAMPLE_DRAG);
    let mut largest = 0f64;
    for script in meta["scripts"].as_array().unwrap() {
        let name = script["name"].as_str().unwrap();
        let (mut mode, mut gesture): (bool, Option<Gesture>) = (true, None);
        for (step, expected) in script["steps"].as_array().unwrap().iter().zip(script["results"].as_array().unwrap()) {
            let what = format!("{name} {step}");
            let screen = Pos2::new(f(&step[1]) as f32, f(&step[2]) as f32);
            let mut emitted: Vec<Vec<f64>> = Vec::new();
            match step[0].as_str().unwrap() {
                "press" if mode => gesture = Gesture::press(screen, view.to_frame_free(screen)),
                "move" if mode => {
                    if let Some(g) = &mut gesture {
                        g.moved(screen, view.to_frame(screen));
                    }
                }
                "release" if mode => {
                    if let Some(g) = gesture.take() {
                        emitted.push(match g.release(screen, view.to_frame(screen)) {
                            Sample::Point(p) => vec![p[0], p[1]],
                            Sample::Area(a) => a.to_vec(),
                        });
                    }
                }
                "key" => {
                    mode = false;
                    gesture = None;
                    emitted.push(vec![]);
                }
                _ => {}
            }
            let want: Vec<Vec<f64>> = expected["emitted"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    if e[0] == "cancelled" {
                        vec![]
                    } else {
                        numbers(&Value::Array(e.as_array().unwrap()[1..].to_vec()))
                    }
                })
                .collect();
            assert_eq!(emitted.len(), want.len(), "{what}: signals");
            for (a, b) in emitted.iter().zip(&want) {
                assert_eq!(a.len(), b.len(), "{what}");
                for (x, y) in a.iter().zip(b) {
                    largest = largest.max((x - y).abs());
                    assert!((x - y).abs() <= 1e-5, "{what}: {a:?} vs {b:?}");
                }
            }
            let area = gesture.and_then(|g| g.area());
            match (&expected["area"], area) {
                (Value::Null, None) => {}
                (want, Some(got)) => {
                    for (x, y) in got.iter().zip(numbers(want)) {
                        assert!((x - y).abs() <= 1e-5, "{what}: area {got:?} vs {want}");
                    }
                }
                (want, got) => panic!("{what}: area {got:?} vs {want}"),
            }
            assert_eq!(gesture.is_some(), expected["pressed"].as_bool().unwrap(), "{what}: pressed");
            assert_eq!(mode, expected["sample_mode"].as_bool().unwrap(), "{what}: sample mode");
        }
    }
    println!("sample gestures: largest difference {largest:e}");
}

// ---- ordinary photos --------------------------------------------------------------------------------------------

/// The display branch of `sample_wb`: clicks and rectangles on a picture (8-bit, as the frame on screen).
#[test]
fn display_branch_matches_the_oracle() {
    let mut largest = 0f64;
    for name in ["pickers_display", "pickers_display_warm", "pickers_display_saturated"] {
        let meta_case = Case::open("app", name);
        let meta = meta_case.json();
        let pixels: Array3<u8> = Case::open("app", "pickers_display").array("picture");
        let (h, w, _) = pixels.dim();
        let pixel = |x: usize, y: usize| [0, 1, 2].map(|c| f32::from(pixels[[y, x, c]]) / 255.0);
        let picture = Picture { width: w, height: h, pixel: &pixel };
        let mut edit = EditState::new(settings_with(&meta["start"]));
        for (step, expected) in meta["steps"].as_array().unwrap().iter().zip(meta["results"].as_array().unwrap()) {
            if !is_sample(step) {
                continue;
            }
            let what = format!("{name} {step}");
            let before = edit.history().len();
            let mut statuses = Vec::new();
            if let Err(text) = eyedropper::apply_display(&mut edit, &picture, &sample_of(step)) {
                statuses.push(text);
            }
            largest = largest.max(compare_settings(&edit, &expected["settings"], 2e-4, &what));
            compare_outcome(&statuses, &added(&edit, before), expected, &what);
        }
    }
    println!("display branch: largest relative difference {largest:e} (tolerance 2e-4)");
}

// ---- RAW photos and negatives ---------------------------------------------------------------------------------

fn dng(file: &str) -> PathBuf {
    grainy_testkit::root().join("golden").join("rawload").join("_files").join(file)
}

/// The DNG's camera source with the picture of the case in its pixels (the calibration is the file's).
fn camera_source(meta: &Value, settings: &Settings) -> CameraSource {
    let loaded =
        load_photo(dng(meta["file"].as_str().unwrap()), Some(eyedropper::SOURCE_SIZE), settings).expect("the DNG");
    let Photo::Camera(source) = loaded.photo else { panic!("not a camera source") };
    let pixels: Array3<f32> = Case::open("app", meta["pixels_case"].as_str().unwrap()).array("pixels");
    assert_eq!(pixels.dim(), source.pixels().dim());
    CameraSource::new(pixels, source.raw_info().clone()).expect("the source")
}

fn map_of(meta: &Value, settings: &Settings) -> CoordinateMap {
    let live = meta["live"].as_array().unwrap();
    CoordinateMap::new(live[1].as_u64().unwrap() as usize, live[0].as_u64().unwrap() as usize, settings).expect("map")
}

const RAW_CASES: [&str; 7] =
    ["triple", "triple_crop_rotation", "triple_straighten_flip", "bayer1", "bayer2_crop", "bayer3", "lineardual"];

/// The RAW branch: `neutral_white` of the camera neutral under clicks and rectangles carried through the geometry.
#[test]
fn raw_branch_matches_the_oracle() {
    let mut largest = 0f64;
    let mut picked = 0;
    for name in RAW_CASES {
        let case = Case::open("app", &format!("pickers_raw_{name}"));
        let meta = case.json();
        let settings = settings_with(&meta["settings"]);
        let source = camera_source(meta, &settings);
        let map = map_of(meta, &settings);
        let mut edit = EditState::new(settings.clone());
        for (step, expected) in meta["steps"].as_array().unwrap().iter().zip(meta["results"].as_array().unwrap()) {
            if !is_sample(step) {
                continue;
            }
            let what = format!("{name} {step}");
            let before = edit.history().len();
            let pick = eyedropper::source_pick(&map, &sample_of(step));
            let mut statuses = Vec::new();
            match eyedropper::raw_white(&source, edit.settings(), &pick) {
                Ok(white) => {
                    eyedropper::apply_raw_white(&mut edit, &white);
                    picked += 1;
                }
                Err(Failure::Message(text)) => statuses.push(text),
                Err(other) => assert_eq!(other, Failure::Silent, "{what}"),
            }
            largest = largest.max(compare_settings(&edit, &expected["settings"], 1e-5, &what));
            compare_outcome(&statuses, &added(&edit, before), expected, &what);
        }
    }
    println!("RAW branch: {picked} picks, largest relative difference {largest:e} (tolerance 1e-5)");
    assert!(picked > 40);
}

/// A negative's photo: the camera source stays, but the picture's branch runs (a negative has no camera white balance).
#[test]
fn a_raw_negative_is_not_read_as_a_camera_white() {
    let case = Case::open("app", "pickers_raw_negative_display");
    let meta = case.json();
    let settings = settings_with(&meta["settings"]);
    assert!(settings.bool("negative"));
    // The branch decision of `sample`: the camera path needs a photo that is not a negative.
    assert!(grainy_raw::rawcolor::enabled(&settings), "a negative is read in the camera's colours");
    let u: Array3<u8> = Case::open("app", "pickers_display").array("picture");
    let pixel = |x: usize, y: usize| [0, 1, 2].map(|c| f32::from(u[[y, x, c]]) / 255.0);
    let (h, w, _) = u.dim();
    let picture = Picture { width: w, height: h, pixel: &pixel };
    let mut edit = EditState::new(settings);
    for (step, expected) in meta["steps"].as_array().unwrap().iter().zip(meta["results"].as_array().unwrap()) {
        if !is_sample(step) {
            continue;
        }
        let before = edit.history().len();
        let mut statuses = Vec::new();
        if let Err(text) = eyedropper::apply_display(&mut edit, &picture, &sample_of(step)) {
            statuses.push(text);
        }
        let what = format!("raw negative {step}");
        compare_settings(&edit, &expected["settings"], 2e-4, &what);
        compare_outcome(&statuses, &added(&edit, before), expected, &what);
    }
}

/// The film base: the median of the 9 × 9 pixels around a click or of a rectangle, refused when too dark, on a plain
/// scan and on camera sources, through crops, rotations and straightening.
#[test]
fn film_base_matches_the_oracle() {
    let mut largest = 0f64;
    let mut picked = 0;
    for name in ["plain", "plain_crop_rotation", "plain_straighten", "raw_bayer0", "raw_triple_crop"] {
        let case = Case::open("app", &format!("pickers_negative_{name}"));
        let meta = case.json();
        let settings = settings_with(&meta["settings"]);
        let photo = if name.starts_with("plain") {
            Photo::Linear(Case::open("app", "pickers_negative_plain").array::<f32, Ix3>("source"))
        } else {
            Photo::Camera(Box::new(camera_source(meta, &settings)))
        };
        let map = map_of(meta, &settings);
        let mut edit = EditState::new(settings.clone());
        let mut armed = false;
        for (step, expected) in meta["steps"].as_array().unwrap().iter().zip(meta["results"].as_array().unwrap()) {
            let what = format!("{name} {step}");
            let before = edit.history().len();
            let mut statuses = Vec::new();
            match step[0].as_str().unwrap() {
                "toggle_base" => armed = step[1].as_bool().unwrap() && edit.settings().bool("negative"),
                "set" => {
                    edit.set(step[1].as_str().unwrap(), step[2].clone());
                    edit.commit("보정 변경");
                }
                _ if armed => {
                    armed = false;
                    let pick = eyedropper::source_pick(&map, &sample_of(step));
                    let scan = eyedropper::scan_of(&photo, edit.settings()).expect("the scan");
                    let peak =
                        edit.settings().value("negative_peak").as_array().map(|l| [f(&l[0]), f(&l[1]), f(&l[2])]);
                    match eyedropper::negative_base(&scan, &pick, peak) {
                        Ok(base) => {
                            eyedropper::apply_negative_base(&mut edit, base);
                            picked += 1;
                        }
                        Err(Failure::Message(text)) => statuses.push(text),
                        Err(other) => assert_eq!(other, Failure::Silent, "{what}"),
                    }
                }
                _ => {
                    // The button is not pressed: the ordinary picker. A camera source that is no longer a negative
                    // takes the RAW white; a plain scan has no frame here.
                    if let Photo::Camera(source) = &photo {
                        if !edit.settings().bool("negative") {
                            let pick = eyedropper::source_pick(&map, &sample_of(step));
                            if let Ok(white) = eyedropper::raw_white(source, edit.settings(), &pick) {
                                eyedropper::apply_raw_white(&mut edit, &white);
                            }
                        }
                    }
                }
            }
            if matches!(step[0].as_str().unwrap(), "toggle_base" | "set") {
                continue;
            }
            // The oracle keeps `negative` and `negative_peak` in its record; so does the edit.
            largest = largest.max(compare_settings(&edit, &expected["settings"], 1e-5, &what));
            compare_outcome(&statuses, &added(&edit, before), expected, &what);
        }
    }
    println!("film base: {picked} picks, largest relative difference {largest:e} (tolerance 1e-5)");
    assert!(picked > 25);
}

// ---- the buttons ------------------------------------------------------------------------------------------------

fn context() -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/GrainyGolden/pickers.png")]);
    let id = session.current_id();
    session.set_original(id, (240, 160));
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session))
}

/// `toggle_wb` and `toggle_negative_base`: which button is pressed after each step, and the status text.
#[test]
fn the_buttons_match_the_oracle() {
    let _turn = common::language_lock();
    let case = Case::open("app", "pickers_toggles");
    let meta = case.json();
    let mut cx = context();
    for (step, expected) in meta["steps"].as_array().unwrap().iter().zip(meta["results"].as_array().unwrap()) {
        let what = step.to_string();
        cx.set_status("");
        let on = step.get(1).and_then(Value::as_bool).unwrap_or(false);
        match step[0].as_str().unwrap() {
            "toggle_wb" => eyedropper::toggle_wb(&mut cx, on),
            "toggle_base" => eyedropper::toggle_base(&mut cx, on),
            _ => {
                cx.doc.edit_mut().set(step[1].as_str().unwrap(), step[2].clone());
                cx.doc.edit_mut().commit("보정 변경");
            }
        }
        let statuses = expected["status"].as_array().unwrap();
        match statuses.first() {
            Some(text) => assert_eq!(cx.status(), grainy_i18n::tr(text.as_str().unwrap()), "{what}"),
            None => assert_eq!(cx.status(), "", "{what}"),
        }
        assert_eq!(
            (eyedropper::wb_checked(&cx) || eyedropper::base_checked(&cx)),
            expected["sample_mode"].as_bool().unwrap(),
            "{what}: sample mode"
        );
        assert_eq!(
            eyedropper::wb_checked(&cx),
            expected["wb_checked"].as_bool().unwrap(),
            "{what}: white balance button"
        );
        assert_eq!(
            eyedropper::base_checked(&cx),
            expected["base_checked"].as_bool().unwrap(),
            "{what}: film base button"
        );
    }
}

/// W runs the command; Esc, a new photo and another tool end the mode.
#[test]
fn w_and_the_ways_out() {
    let _turn = common::language_lock();
    let mut cx = context();
    assert!(grainy_app::commands::run(&mut cx, "develop.white_balance", &Invocation::key("W")));
    assert!(eyedropper::wb_checked(&cx));
    assert!(grainy_app::commands::run(&mut cx, "develop.white_balance", &Invocation::key("W")));
    assert!(!eyedropper::wb_checked(&cx));
    // The film base command needs a negative.
    assert!(!grainy_app::commands::enabled(&cx, "develop.negative_base"));
    // Esc leaves the mode.
    eyedropper::toggle_wb(&mut cx, true);
    let mut tool = eyedropper::Eyedropper;
    assert!(tool.active(&cx));
    assert!(tool.key(egui::Key::Escape, Modifiers::NONE, &mut cx));
    assert!(!eyedropper::wb_checked(&cx) && !tool.active(&cx));
    // A press outside the photo does nothing; a drag of three points is still a click.
    eyedropper::toggle_wb(&mut cx, true);
    let outside = PointerEvent {
        kind: PointerKind::Press,
        screen: Pos2::new(1.0, 1.0),
        frame: [0.0, 0.0],
        free: [-0.2, 0.1],
        modifiers: Modifiers::NONE,
    };
    assert!(tool.pointer(&outside, &mut cx), "the view never pans in this mode");
}

// ---- the window ------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The application with the real worker and the photo at `path` open.
fn photo_harness<'a>(path: PathBuf, setup: impl FnOnce(&mut GrainyApp) + 'a) -> Harness<'a, GrainyApp> {
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

fn garden_photo(name: &str) -> PathBuf {
    let path = common::temp_dir(name).join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    path
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

fn point_in(rect: Rect, fx: f32, fy: f32) -> Pos2 {
    Pos2::new(rect.left() + rect.width() * fx, rect.top() + rect.height() * fy)
}

fn fractions(rect: Rect, p: Pos2) -> [f64; 2] {
    [f64::from(p.x - rect.left()) / f64::from(rect.width()), f64::from(p.y - rect.top()) / f64::from(rect.height())]
}

/// W, a click on the photo: temperature and tint follow the oracle's formula on the frame on screen, the picker goes
/// off, the edit is one undo step; then a dragged rectangle averages its pixels. Snapshots: the loupe over the photo
/// and the rectangle being dragged.
fn white_balance_in_the_window(language: Language, suffix: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = photo_harness(garden_photo("pickers-wb"), |app| app.open_panel("basic"));
    harness.key_press(egui::Key::W);
    harness.run_steps(2);
    assert!(eyedropper::wb_checked(harness.state()), "W switches the eyedropper on");
    assert_eq!(harness.state().status(), grainy_i18n::tr(eyedropper::HINT_WHITE));
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    let frame = harness.state().frame().cloned().expect("a frame on screen");

    // The loupe over the photo.
    move_to(&mut harness, point_in(rect, 0.4, 0.6));
    harness.run_steps(2);
    save(&mut harness, &format!("pickers-loupe-{suffix}"));

    // A click.
    let at = point_in(rect, 0.4, 0.6);
    let pixel = eyedropper::preview_pixel(&frame.preview);
    let picture = Picture { width: frame.preview.width, height: frame.preview.height, pixel: &pixel };
    let mut expected = EditState::new(harness.state().settings().clone());
    let p = fractions(rect, at);
    eyedropper::apply_display(&mut expected, &picture, &Sample::Point(p)).expect("colour");
    let undo = harness.state().edit_state().depth().0;
    press(&mut harness, at);
    release(&mut harness, at);
    let settings = harness.state().settings();
    assert_eq!(settings.f64("temperature"), expected.settings().f64("temperature"));
    assert_eq!(settings.f64("tint"), expected.settings().f64("tint"));
    assert_ne!(settings.f64("temperature"), 0.0);
    assert!(!eyedropper::wb_checked(harness.state()), "the click switches the picker off");
    assert_eq!(harness.state().edit_state().depth().0, undo + 1, "one undo step");
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some("보정 변경"));
    settle(&mut harness);

    // A rectangle: dragged, shown with its dashes, averaged on release.
    harness.key_press(egui::Key::W);
    harness.run_steps(2);
    assert!(eyedropper::wb_checked(harness.state()));
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    let frame = harness.state().frame().cloned().expect("a frame on screen");
    let (from, to) = (point_in(rect, 0.25, 0.3), point_in(rect, 0.55, 0.7));
    let mut after_click = EditState::new(harness.state().settings().clone());
    press(&mut harness, from);
    move_to(&mut harness, point_in(rect, 0.4, 0.5));
    move_to(&mut harness, to);
    save(&mut harness, &format!("pickers-area-{suffix}"));
    release(&mut harness, to);
    let pixel = eyedropper::preview_pixel(&frame.preview);
    let picture = Picture { width: frame.preview.width, height: frame.preview.height, pixel: &pixel };
    let (a, b) = (fractions(rect, from), fractions(rect, to));
    eyedropper::apply_display(&mut after_click, &picture, &Sample::Area([a[0], a[1], b[0], b[1]])).expect("colour");
    let settings = harness.state().settings();
    assert_eq!(settings.f64("temperature"), after_click.settings().f64("temperature"));
    assert_eq!(settings.f64("tint"), after_click.settings().f64("tint"));
    assert!(!eyedropper::wb_checked(harness.state()), "the rectangle switches the picker off");
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn white_balance_in_the_window_korean() {
    white_balance_in_the_window(Language::Korean, "ko");
}

#[test]
fn white_balance_in_the_window_english() {
    white_balance_in_the_window(Language::English, "en");
}

/// Esc ends the mode in the window (the key reaches the tool through the photo view).
#[test]
fn escape_in_the_window() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut harness = photo_harness(garden_photo("pickers-esc"), |_| {});
    harness.key_press(egui::Key::W);
    harness.run_steps(2);
    assert!(eyedropper::wb_checked(harness.state()));
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(!eyedropper::wb_checked(harness.state()), "Esc ends the mode");
    // W again switches it on and a second W off.
    harness.key_press(egui::Key::W);
    harness.run_steps(2);
    assert!(eyedropper::wb_checked(harness.state()));
    harness.key_press(egui::Key::W);
    harness.run_steps(2);
    assert!(!eyedropper::wb_checked(harness.state()));
}

/// A RAW photo: the click is carried to the camera source (1800 pixels) on a job and sets the RAW white balance as
/// `raw_white` gives it for the same point.
#[test]
fn raw_pick_in_the_window() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("pickers-raw");
    let path = dir.join("triple.dng");
    std::fs::copy(dng("triple.dng"), &path).expect("copy the DNG");
    let mut harness = photo_harness(path.clone(), |_| {});
    harness.state_mut().edit_state_mut().set("raw_mode", "as_shot");
    harness.state_mut().edit_state_mut().commit("보정 변경");
    settle(&mut harness);
    harness.key_press(egui::Key::W);
    harness.run_steps(2);
    assert!(eyedropper::wb_checked(harness.state()));
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    let frame = harness.state().frame().cloned().expect("a frame on screen");
    let at = point_in(rect, 0.31, 0.62);
    let settings = harness.state().settings().clone();
    let loaded = load_photo(&path, Some(eyedropper::SOURCE_SIZE), &settings).expect("the DNG");
    let Photo::Camera(source) = &loaded.photo else { panic!("not a camera source") };
    let (w, h) = frame.original;
    let (lw, lh) = grainy_app::tools::healing::live_size(w as usize, h as usize);
    let map = CoordinateMap::new(lw, lh, &settings).expect("map");
    let pick = eyedropper::source_pick(&map, &Sample::Point(fractions(rect, at)));
    let expected = eyedropper::raw_white(source, &settings, &pick).expect("a white");
    press(&mut harness, at);
    release(&mut harness, at);
    settle(&mut harness);
    let s = harness.state().settings();
    assert_eq!(s.str("raw_mode"), "sample");
    assert_eq!(numbers(s.value("raw_neutral")), expected.neutral.to_vec());
    assert_eq!(s.f64("raw_kelvin"), expected.kelvin);
    assert_eq!(s.f64("raw_tint"), expected.tint);
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some(eyedropper::RAW_LABEL));
    assert!(!eyedropper::wb_checked(harness.state()), "the pick switches the picker off");
}

const NEGATIVE: [(&str, [f64; 3]); 2] =
    [("negative_base", [0.8, 0.55, 0.35]), ("negative_peak", [0.003, 0.002, 0.001])];

fn negative_setup(harness: &mut Harness<'_, GrainyApp>) {
    harness.state_mut().open_panel("basic");
    let edit = harness.state_mut().edit_state_mut();
    edit.set("negative", true);
    for (key, value) in NEGATIVE {
        edit.set(key, value.to_vec());
    }
    edit.commit("보정 변경");
    settle(harness);
}

/// The film base picker: the button, a click, and `negative_base` is the median around the point on the scan. The
/// Basic panel shows the pressed button and the hint; a snapshot of it in both languages.
fn film_base_in_the_window(language: Language, suffix: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let path = garden_photo("pickers-base");
    let mut harness = photo_harness(path.clone(), |_| {});
    negative_setup(&mut harness);
    assert!(grainy_app::commands::enabled(harness.state(), "develop.negative_base"));
    grainy_app::commands::run(harness.state_mut(), "develop.negative_base", &Invocation::default());
    harness.run_steps(2);
    assert!(eyedropper::base_checked(harness.state()) && !eyedropper::wb_checked(harness.state()));
    assert_eq!(harness.state().status(), grainy_i18n::tr(eyedropper::HINT_BASE));
    let rect = harness.state().photo_rect().expect("the photo was drawn");
    move_to(&mut harness, point_in(rect, 0.2, 0.3));
    harness.run_steps(2);
    save(&mut harness, &format!("pickers-negative-{suffix}"));

    let at = point_in(rect, 0.2, 0.3);
    let frame = harness.state().frame().cloned().expect("a frame on screen");
    let settings = harness.state().settings().clone();
    let loaded = load_photo(&path, Some(eyedropper::SOURCE_SIZE), &settings).expect("the photo");
    let scan = eyedropper::scan_of(&loaded.photo, &settings).expect("the scan");
    let (w, h) = frame.original;
    let (lw, lh) = grainy_app::tools::healing::live_size(w as usize, h as usize);
    let map = CoordinateMap::new(lw, lh, &settings).expect("map");
    let pick = eyedropper::source_pick(&map, &Sample::Point(fractions(rect, at)));
    let peak = [0.003, 0.002, 0.001];
    let expected = eyedropper::negative_base(&scan, &pick, Some(peak)).expect("a base");
    press(&mut harness, at);
    release(&mut harness, at);
    settle(&mut harness);
    assert_eq!(numbers(harness.state().settings().value("negative_base")), expected.to_vec());
    assert_eq!(harness.state().edit_state().history().first().map(String::as_str), Some(eyedropper::BASE_LABEL));
    assert!(!eyedropper::base_checked(harness.state()), "the click switches the picker off");
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn film_base_in_the_window_korean() {
    film_base_in_the_window(Language::Korean, "ko");
}

#[test]
fn film_base_in_the_window_english() {
    film_base_in_the_window(Language::English, "en");
}
