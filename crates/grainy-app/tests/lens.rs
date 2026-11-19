//! Lens correction (work package 21).
//!
//! * Scripted sessions without a window, step by step against the oracle's own `StudioTools.auto_lens`,
//!   `choose_lens`, `apply_lens`, `save_lens`, `load_lens` and the real `LensDialog` widget, driven headless
//!   (`tools/golden/gen_app_lens.py`, `golden/app/lens_*`, on the committed lensfun extract of `grainy-lens`):
//!   after every step the profile in the settings, the five check boxes (state and enabling), the manual values,
//!   both labels, the commits, the status line, the message boxes and the dialog's states.
//! * The number boxes' rounding and bounds, the database folders, the real window: the database read by a job, the
//!   dialog driven with the keyboard and the mouse, a develop with the profile applied; snapshots in Korean and
//!   English.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use grainy_app::commands::lens::{self as lens, LensState};
use grainy_app::dialogs::lens::{LensChoice, Spin};
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::state::EditState;
use grainy_app::{AppContext, GrainyApp};
use grainy_i18n::Language;
use grainy_lens::Database;
use grainy_testkit::Case;
use serde_json::{json, Value};

fn extract() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("grainy-lens").join("tests").join("data")
}

fn database() -> Arc<Database> {
    Arc::new(Database::from_folder(extract()).expect("the committed lensfun extract"))
}

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

/// A context on a session with one photo and the oracle's settings changes.
fn context(changes: &Value) -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("Z:/GrainyGolden/lens.png")]);
    session.select(0);
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    let id = cx.doc.current_id();
    cx.doc.set_original(id, (320, 240));
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

// ---- the dialog's script ----------------------------------------------------------------------------------------

fn spin(s: &Spin) -> Value {
    json!([s.value, s.low, s.high])
}

fn dialog_state(c: &LensChoice, profiles: &[Value]) -> Value {
    let result =
        c.result.as_ref().map(|r| profiles.iter().position(|p| same(p, r)).expect("a profile the oracle also made"));
    json!({
        "camera": c.camera, "camera_text": c.camera_text, "cameras": c.cameras.len(),
        "lens": c.lens, "lens_text": c.lens_text, "lenses": c.lenses.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
        "crop": spin(&c.crop), "focal": spin(&c.focal), "aperture": spin(&c.aperture), "distance": spin(&c.distance),
        "status": c.status, "result": result,
    })
}

fn item(texts: Vec<&String>, needle: &str) -> usize {
    texts.iter().position(|t| t.contains(needle)).unwrap_or_else(|| panic!("no item with {needle}"))
}

/// The oracle's `play`: the user's steps on the dialog; stops when 보정 적용 worked (`accept()` ends `exec()`).
fn play(c: &mut LensChoice, script: &[Value], profiles: &[Value]) -> (bool, Vec<Value>) {
    let mut states = Vec::new();
    let mut accepted = false;
    for step in script {
        let step = step.as_array().unwrap();
        let arg = |i: usize| step[i].clone();
        match step[0].as_str().unwrap() {
            "camera" => {
                let i = item(c.cameras.iter().map(|(t, _)| t).collect(), arg(1).as_str().unwrap());
                c.set_camera(i);
            }
            "camera_text" => c.camera_text = arg(1).as_str().unwrap().to_owned(),
            "lens" => {
                let i = item(c.lenses.iter().map(|l| &l.text).collect(), arg(1).as_str().unwrap());
                c.set_lens(i);
            }
            "lens_text" => c.lens_text = arg(1).as_str().unwrap().to_owned(),
            "set" => {
                let v = arg(2).as_f64().unwrap();
                match arg(1).as_str().unwrap() {
                    "crop" => c.crop.set_value(v),
                    "focal" => c.focal.set_value(v),
                    "aperture" => c.aperture.set_value(v),
                    "distance" => c.distance.set_value(v),
                    other => panic!("{other}"),
                }
            }
            "apply" => {
                accepted = c.apply();
                states.push(dialog_state(c, profiles));
                if accepted {
                    break;
                }
                continue;
            }
            "cancel" => accepted = false,
            other => panic!("unknown dialog step {other}"),
        }
        states.push(dialog_state(c, profiles));
    }
    (accepted, states)
}

/// The oracle's `fake_exec` for one dialog (`expected`: what it recorded) and, when accepted, `accepted`.
fn run_dialog(cx: &mut AppContext, mut choice: LensChoice, expected: &Value, profiles: &[Value], at: &str) {
    let script = expected["script"].as_array().unwrap();
    assert!(same(&dialog_state(&choice, profiles), &expected["opened"]), "{at}: the dialog when it opens");
    let (accepted, states) = play(&mut choice, script, profiles);
    let wanted = expected["states"].as_array().unwrap();
    assert_eq!(states.len(), wanted.len(), "{at}: steps");
    for (i, (state, want)) in states.iter().zip(wanted).enumerate() {
        assert!(same(state, want), "{at}: dialog step {i} {:?}\n got {state}\nwant {want}", script[i]);
    }
    assert_eq!(accepted, expected["accepted"].as_bool().unwrap(), "{at}: accepted");
    if accepted {
        lens::accepted(cx, &mut choice);
    }
}

// ---- the scenarios ----------------------------------------------------------------------------------------------

fn check_state(cx: &AppContext, expected: &Value, profiles: &[Value], at: &str) {
    let s = cx.settings();
    match expected["lensfun"].as_u64() {
        Some(i) => assert!(same(s.value("lensfun"), &profiles[i as usize]), "{at}: lensfun {}", s.value("lensfun")),
        None => assert!(!grainy_core::settings::truthy(s.value("lensfun")), "{at}: no profile"),
    }
    assert_eq!(s.bool("lensfun_enabled"), expected["lensfun_enabled"].as_bool().unwrap(), "{at}: enabled");
    for (key, want) in expected["switches"].as_object().unwrap() {
        let got = json!([s.bool(key), lens::switch_enabled(s, key)]);
        assert_eq!(&got, want, "{at}: check box {key}");
    }
    for (key, want) in expected["values"].as_object().unwrap() {
        assert!((s.f64(key) - want.as_f64().unwrap()).abs() < 1e-12, "{at}: {key} {}", s.f64(key));
    }
    let profile = expected["lens_profile"].as_str().unwrap_or("");
    assert_eq!(s.str("lens_profile"), profile, "{at}: lens_profile");
    assert_eq!(lens::auto_label(s), expected["auto_label"].as_str().unwrap(), "{at}: automatic label");
    assert_eq!(lens::manual_label(s), expected["lens_label"].as_str().unwrap(), "{at}: manual label");
    assert_eq!(history(cx), strings(&expected["history"]), "{at}: commits");
    assert!((s.f64("exposure") - expected["exposure"].as_f64().unwrap()).abs() < 1e-12, "{at}: exposure");
    if let Some(status) = expected["status"].as_str() {
        assert_eq!(cx.status(), status, "{at}: status");
    }
    let boxes: Vec<Value> =
        cx.state_ref::<LensState>().map(|st| st.boxes.iter().map(|(a, b)| json!([a, b])).collect()).unwrap_or_default();
    assert_eq!(Value::Array(boxes), expected["boxes"], "{at}: message boxes");
}

fn text_of_file(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn run_scenario(name: &str) -> usize {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let case = Case::open("app", &format!("lens_{name}"));
    let data = case.json();
    let (info, changes) = (&data["info"], &data["changes"]);
    let profiles = data["profiles"].as_array().unwrap().clone();
    let db = database();
    let mut cx = context(changes);
    let folder = common::temp_dir(&format!("lens-files-{name}"));
    let mut dialogs = 0;
    for (index, (item, expected)) in
        data["script"].as_array().unwrap().iter().zip(data["states"].as_array().unwrap()).enumerate()
    {
        let item = item.as_array().unwrap();
        let at = format!("{name} step {index} {}", item[0]);
        let listed = expected.get("dialogs").and_then(Value::as_array).cloned().unwrap_or_default();
        match item[0].as_str().unwrap() {
            "auto" => {
                let opened = lens::auto_for(&mut cx, &db, info);
                assert_eq!(opened.is_some(), !listed.is_empty(), "{at}: the dialog opens when the profile fails");
                if let Some(choice) = opened {
                    dialogs += 1;
                    run_dialog(&mut cx, choice, &listed[0], &profiles, &at);
                }
            }
            "choose" => {
                let choice = lens::choose_for(&cx, &db, info);
                assert_eq!(listed.len(), 1, "{at}");
                dialogs += 1;
                run_dialog(&mut cx, choice, &listed[0], &profiles, &at);
            }
            "switch" => {
                let edit = cx.doc.edit_mut();
                edit.set(item[1].as_str().unwrap(), item[2].as_bool().unwrap());
                edit.commit("보정 변경");
            }
            "set" => cx.doc.edit_mut().set(item[1].as_str().unwrap(), item[2].clone()),
            "commit" => {
                cx.doc.edit_mut().commit("보정 변경");
            }
            "set_profile" => {
                // The profile the oracle's script made by hand: the settings the step left.
                let profile = profiles[expected["lensfun"].as_u64().unwrap() as usize].clone();
                let edit = cx.doc.edit_mut();
                edit.set("lensfun", profile);
                edit.set("lensfun_enabled", true);
                edit.commit("렌즈 자동 보정");
            }
            "save" => {
                let path = folder.join(item[1].as_str().unwrap());
                let text = lens::lens_file_text(&path, cx.settings());
                assert_eq!(text_of_file(&text), text_of_file(expected["file"].as_str().unwrap()), "{at}: the file");
                lens::save_to(&mut cx, path.clone());
                assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "{at}: written as shown");
            }
            "load" => {
                let path = folder.join(item[1].as_str().unwrap());
                std::fs::write(&path, serde_json::to_string(&item[2]).unwrap()).unwrap();
                lens::load_from(&mut cx, path);
                cx.close_dialog("lens_error");
            }
            other => panic!("unknown step {other}"),
        }
        check_state(&cx, expected, &profiles, &at);
    }
    dialogs
}

#[test]
fn automatic_profile_and_check_boxes() {
    assert_eq!(run_scenario("auto"), 0, "the metadata names the lens: no dialog");
}

#[test]
fn a_failed_automatic_profile_opens_the_dialog() {
    assert_eq!(run_scenario("no_lens"), 2);
}

#[test]
fn choosing_a_lens_by_hand() {
    assert_eq!(run_scenario("by_hand"), 4);
}

#[test]
fn capabilities_enable_the_check_boxes() {
    run_scenario("capabilities");
}

#[test]
fn the_dialog_opens_on_the_current_profile() {
    assert_eq!(run_scenario("current"), 3);
}

#[test]
fn a_copied_profile_of_another_photo() {
    run_scenario("film_current");
}

#[test]
fn the_manual_lens_file() {
    run_scenario("manual");
}

#[test]
fn the_number_boxes_round_and_bound() {
    let case = Case::open("app", "lens_spin");
    for row in case.json()["rows"].as_array().unwrap() {
        let (low, high) = (row["low"].as_f64().unwrap(), row["high"].as_f64().unwrap());
        let decimals = row["decimals"].as_u64().unwrap() as usize;
        let mut spin = Spin::new(low, high, decimals, None);
        for pair in row["values"].as_array().unwrap() {
            let (input, want) = (pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap());
            spin.set_value(input);
            assert_eq!(spin.value, want, "{low}..{high} ({decimals}): {input}");
        }
        // One box, as the oracle's: each range is set on what the one before left.
        for r in row["ranges"].as_array().unwrap() {
            spin.set_value(50.0);
            spin.set_range(r[0].as_f64().unwrap(), r[1].as_f64().unwrap());
            assert_eq!(
                (spin.low, spin.high, spin.value),
                (r[2].as_f64().unwrap(), r[3].as_f64().unwrap(), r[4].as_f64().unwrap())
            );
        }
    }
}

#[test]
fn the_database_folders() {
    let mut locations = grainy_app::paths::Locations::current();
    locations.home_override = Some(PathBuf::from("Z:/GrainyGolden/home"));
    let list =
        lens::candidates(Some(Path::new("Z:/a")), Some(Path::new("Z:/b")), &locations, Some(Path::new("Z:/program")));
    assert_eq!(
        list,
        vec![
            PathBuf::from("Z:/a"),
            PathBuf::from("Z:/b"),
            PathBuf::from("Z:/GrainyGolden/home/lensfun"),
            PathBuf::from("Z:/program/lensfun"),
        ]
    );
    // An existing explicit folder is taken first.
    let folder = common::temp_dir("lens-folder");
    assert_eq!(lens::database_folder(Some(&folder)), folder);
}

#[test]
fn a_lens_file_reads_like_the_oracles() {
    let path = Path::new("D:/lenses/My lens.json");
    let text = r#"{"type":"luma-lens","version":1,"settings":{"distortion":"2.5","defringe":10,"ca_red":true}}"#;
    let file = lens::read_lens_file(text, path).unwrap();
    assert_eq!(file.name, "My lens", "no name: the file's stem");
    assert_eq!(file.values[0], ("distortion", 2.5));
    assert_eq!(file.values[2], ("distortion_k3", 0.0));
    assert_eq!(file.values[5], ("ca_blue", 0.0));
    assert_eq!(file.values[4], ("ca_red", 1.0));
    assert!(lens::read_lens_file("[1]", path).is_err());
    assert!(lens::read_lens_file("not json", path).is_err());
    assert!(lens::read_lens_file(r#"{"type":"luma-lens","settings":[1]}"#, path).is_err());
    // The text a save writes reads back.
    let mut cx = context(&json!({"distortion": 7.0, "ca_red": -2.0, "defringe": 30}));
    let written = lens::lens_file_text(path, cx.settings());
    let back = lens::read_lens_file(&written, path).unwrap();
    assert_eq!(back.name, "My lens");
    lens::apply_lens_file(&mut cx, &back);
    assert_eq!(cx.settings().str("lens_profile"), "My lens");
    assert_eq!(history(&cx), vec!["렌즈 프로파일".to_owned()]);
}

#[test]
fn a_develop_with_the_applied_profile_equals_the_oracles() {
    for name in ["canon", "canon_no_scale", "sony"] {
        let case = Case::open("app", &format!("lens_develop_{name}"));
        let data = case.json();
        let db = database();
        let mut cx = context(&json!({}));
        // 렌즈 자동 선택 on the photo's metadata, then the check boxes and sliders the oracle's case set.
        assert!(lens::auto_for(&mut cx, &db, &data["info"]).is_none(), "{name}: the profile is found");
        for (key, value) in data["extra"].as_object().unwrap() {
            cx.doc.edit_mut().set(key, value.clone());
        }
        let settings = cx.settings().clone();
        let expected: ndarray::Array3<f32> = case.array("develop");
        let out = grainy_engine::develop(&grainy_testkit::input("colours"), &settings.0, &Default::default())
            .expect("develop with the lens profile");
        let (worst, _) = grainy_testkit::max_abs_diff(&out, &expected);
        println!("lens develop {name}: largest error {worst:.3e}");
        assert!(worst < 1e-5, "{name}: {worst}");
        // The profile really changes the picture.
        let plain = grainy_engine::develop(
            &grainy_testkit::input("colours"),
            &common::settings(json!({})).0,
            &Default::default(),
        )
        .unwrap();
        assert!(grainy_testkit::max_abs_diff(&plain, &expected).0 > 1e-3, "{name}: the lens correction acts");
    }
}

// ---- the real window ----------------------------------------------------------------------------------------------

fn builder() -> egui_kittest::HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

use common::snapshot::{save, settle};

fn quiet(harness: &mut Harness<'_, GrainyApp>) {
    // No blinking caret in the pictures.
    harness.ctx.all_styles_mut(|style| style.visuals.text_cursor.blink = false);
}

/// The text fields of the lens dialog (the last six text inputs of the window): camera, lens, crop, focal length,
/// aperture, distance.
fn field(harness: &Harness<'_, GrainyApp>, index: usize) -> egui::Rect {
    let inputs: Vec<_> = harness.get_all_by_role(egui::Role::TextInput).collect();
    inputs[inputs.len() - 6 + index].rect()
}

fn click_at(harness: &mut Harness<'_, GrainyApp>, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        harness.run_steps(2);
    }
    harness.run_steps(2);
}

/// Click in a field, replace its text.
fn type_into(harness: &mut Harness<'_, GrainyApp>, index: usize, text: &str) {
    let rect = field(harness, index);
    click_at(harness, egui::pos2(rect.left() + 12.0, rect.center().y));
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(2);
    harness.event(egui::Event::Text(text.to_owned()));
    harness.run_steps(3);
}

fn press(harness: &mut Harness<'_, GrainyApp>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(3);
}

fn flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(&format!("lens-window-{tag}"));
    let photo = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &photo);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.cx.state::<LensState>().folder = Some(extract());
        app.open_paths(vec![photo]);
        app
    });
    quiet(&mut harness);
    settle(&mut harness);
    harness.state_mut().open_panel("lens");
    harness.run_steps(3);

    // 렌즈 자동 선택 on a photo without camera data: the database is read by a job, the reason is the status
    // message and the dialog opens.
    press(&mut harness, &grainy_app::text::tr("렌즈 자동 선택"));
    settle(&mut harness);
    assert!(harness.state().cx.dialog_open("lens"), "the lens dialog is open");
    assert_eq!(harness.state().status(), "카메라를 선택해 주세요.");
    assert!(harness.state_mut().cx.state::<LensState>().database().is_some(), "the database was read");
    save(&mut harness, &format!("lens-dialog-{tag}"));

    // The camera: its box lists the cameras; typing narrows them; a click chooses.
    type_into(&mut harness, 0, "5D");
    harness.get_by_label("Canon EOS 5D Mark II").click();
    harness.run_steps(3);
    // The lens: only those that fit the camera, narrowed by what is typed.
    type_into(&mut harness, 1, "70-200");
    save(&mut harness, &format!("lens-dialog-search-{tag}"));
    let first = harness.get_all_by_label_contains("70-200").next().expect("the lens in the list");
    first.click();
    harness.run_steps(3);
    // The aperture, typed.
    type_into(&mut harness, 4, "4");
    harness.key_press(egui::Key::Enter);
    harness.run_steps(3);
    save(&mut harness, &format!("lens-dialog-chosen-{tag}"));
    press(&mut harness, &lens::tr("보정 적용"));
    settle(&mut harness);
    assert!(!harness.state().cx.dialog_open("lens"), "보정 적용 closes the dialog");
    {
        let app = harness.state();
        let profile = app.cx.settings().value("lensfun").clone();
        assert_eq!(profile["name"], "Canon EF 70-200mm f/2.8L IS USM");
        assert_eq!(profile["camera"], "Canon EOS 5D Mark II");
        assert_eq!(profile["focal"], 70.0, "the focal length was moved into the lens' range");
        assert_eq!(profile["aperture"], 4.0);
        assert!(app.cx.settings().bool("lensfun_enabled"));
        assert_eq!(app.cx.doc.edit().history().first().map(String::as_str), Some("렌즈 자동 보정"));
    }

    // The manual lens, loaded from a file, with the panel's manual section open.
    {
        let path = dir.join("Wide fix.json");
        std::fs::write(&path, r#"{"type":"luma-lens","version":1,"settings":{"distortion":12.5,"ca_red":1.25}}"#)
            .unwrap();
        lens::load_from(&mut harness.state_mut().cx, path);
    }
    harness.state_mut().cx.set_foldout("lens_manual", true);
    // One of the five check boxes off, as a click would leave it.
    harness.state_mut().cx.doc.edit_mut().set("lensfun_tca", false);
    harness.state_mut().cx.doc.edit_mut().commit("보정 변경");
    settle(&mut harness);
    save(&mut harness, &format!("lens-panel-{tag}"));
    grainy_i18n::set_language(Language::Korean);
}

/// No lensfun folder: the oracle's "no profile" texts, the dialog that says where the database should be.
fn without_database(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(&format!("lens-nodb-{tag}"));
    let photo = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &photo);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        // A folder that is not there, with a fixed name.
        app.cx.state::<LensState>().folder = Some(PathBuf::from("Z:/GrainyGolden/lensfun"));
        app.open_paths(vec![photo]);
        app
    });
    quiet(&mut harness);
    settle(&mut harness);
    harness.state_mut().open_panel("lens");
    harness.run_steps(3);
    {
        let app = harness.state();
        assert_eq!(lens::auto_label(app.cx.settings()), grainy_i18n::tr("자동 렌즈 프로파일 없음"));
        assert_eq!(lens::manual_label(app.cx.settings()), grainy_i18n::tr("렌즈 프로파일 없음"));
        for (key, _) in grainy_app::panels::lens::SWITCHES {
            assert!(!lens::switch_enabled(app.cx.settings(), key), "{key} waits for a profile");
        }
    }
    press(&mut harness, &grainy_app::text::tr("카메라·렌즈 선택"));
    settle(&mut harness);
    assert!(harness.state().cx.dialog_open("lens"));
    let error = harness.state_mut().cx.state::<LensState>().database_error().map(str::to_owned);
    assert!(error.is_some_and(|e| e.contains("Z:/GrainyGolden/lensfun")), "the folder is named");
    save(&mut harness, &format!("lens-nodb-{tag}"));
    press(&mut harness, &grainy_app::text::tr("취소"));
    assert!(!harness.state().cx.dialog_open("lens"));
    assert!(harness.state().cx.doc.edit().history().is_empty(), "nothing was changed");
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn no_database_in_korean() {
    without_database(Language::Korean, "ko");
}

#[test]
fn no_database_in_english() {
    without_database(Language::English, "en");
}

#[test]
fn the_window_in_korean() {
    flow(Language::Korean, "ko");
}

#[test]
fn the_window_in_english() {
    flow(Language::English, "en");
}
