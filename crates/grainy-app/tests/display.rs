//! Display colour management and the photo background (work package 23).
//!
//! Models against goldens made with the oracle's own code (`tools/golden/gen_app_display.py`, `golden/app/display*`):
//! `DisplayColor` / `DisplayTarget` / `DisplayDialog` (state, button, tooltip, preferences, the legacy
//! `monitor_icc`), the texts of both languages, `set_view_background` and its menu, the frame the renderer makes with
//! a monitor profile (`output_rgb(working, space, profile)`), and the thumbnails through it. The real window
//! (egui_kittest on wgpu) draws the box, the status button, the photo in the monitor's colours and the right-click
//! menu in Korean and English. The monitor's profile comes from the files of the goldens and a fixed fake system:
//! no test reads the real monitor profile.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::settings as screen;
use grainy_app::dialogs::display::DisplayDialog;
use grainy_app::display::{self, DisplayState, BACKGROUNDS};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::{Preview, PreviewJob, Renderer, Reply, Resolution, ThumbnailJob};
use grainy_app::session::Session;
use grainy_app::text::tr;
use grainy_app::worker::Worker;
use grainy_app::{AppContext, GrainyApp};
use grainy_cms::display::SystemProfiles;
use grainy_cms::Profile;
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Value};

use common::snapshot::{pin_status, save, settle_frame};

// ---- helpers ---------------------------------------------------------------------------------------------------

/// The system lookups of the tests: the profile Windows would report for the screen (a settable path).
#[derive(Default)]
struct FakeSystem {
    path: Mutex<Option<String>>,
}

impl SystemProfiles for FakeSystem {
    fn is_mac(&self) -> bool {
        false
    }

    fn windows_profile_path(&self, _screen: &str) -> Result<Option<String>, String> {
        Ok(self.path.lock().unwrap().clone())
    }

    fn mac_profile_data(&self, _screen: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}

fn golden_folder(name: &str) -> PathBuf {
    grainy_testkit::root().join("golden").join("app").join(name)
}

fn inputs(file: &str) -> PathBuf {
    golden_folder("display_inputs").join(file)
}

fn profile(file: &str) -> Profile {
    Profile::from_bytes(std::fs::read(inputs(file)).expect("a profile of display_inputs"))
}

fn context_on(document: Box<dyn grainy_app::document::Document>) -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(grainy_app::render::TestCard::default()), document)
}

/// A context over a library in `folder` (preferences are stored there).
fn library_context(folder: &Path) -> AppContext {
    context_on(Box::new(CatalogDocument::open(&folder.join("Library")).unwrap()))
}

/// Paths of `dir` as `<dir>`, with forward slashes: how the goldens write them.
fn clean(text: &str, dir: &Path) -> String {
    let dir = dir.to_string_lossy().replace('\\', "/");
    text.replace('\\', "/").replace(&dir, "<dir>")
}

fn clean_value(value: &Value, dir: &Path) -> Value {
    match value {
        Value::String(s) => json!(clean(s, dir)),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), clean_value(v, dir))).collect()),
        other => other.clone(),
    }
}

/// The oracle's `snapshot` of a step, from the context.
fn snapshot(cx: &mut AppContext, dir: &Path) -> Value {
    let stored = |cx: &AppContext, key: &str| cx.doc.preference(key).unwrap_or(Value::Null);
    let (display, legacy) = (stored(cx, "display_color"), stored(cx, "monitor_icc"));
    let state = cx.state::<DisplayState>();
    let target = state.target().clone();
    json!({
        "button": state.button_text(),
        "tooltip": clean(&state.tooltip(), dir),
        "mode": state.mode,
        "path": clean(&state.path, dir),
        "state": {
            "mode": target.mode, "screen": target.screen, "path": clean(&target.path, dir),
            "description": target.description, "message": clean(&target.message, dir), "error": target.error,
            "has_profile": target.data.is_some(), "digest": target.digest,
        },
        "revision": state.revision(),
        "stored": {
            "display_color": clean_value(&display, dir),
            "monitor_icc": clean_value(&legacy, dir),
        },
    })
}

/// The golden step reduced to what the Rust app keeps: the oracle's `unset` and `null` are both "no preference".
fn golden_step(step: &Value) -> Value {
    let mut step = step.clone();
    let stored = step["stored"].as_object_mut().unwrap();
    for value in stored.values_mut() {
        if value == "__unset__" {
            *value = Value::Null;
        }
    }
    json!({
        "button": step["button"], "tooltip": normalised(&step["tooltip"]), "mode": step["mode"],
        "path": normalised(&step["path"]), "state": normalised_state(&step["state"]), "revision": step["revision"],
        "stored": {
            "display_color": normalised(&step["stored"]["display_color"]),
            "monitor_icc": normalised(&step["stored"]["monitor_icc"]),
        },
    })
}

fn normalised(value: &Value) -> Value {
    match value {
        Value::String(s) => json!(s.replace('\\', "/")),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), normalised(v))).collect()),
        other => other.clone(),
    }
}

fn normalised_state(state: &Value) -> Value {
    normalised(state)
}

/// Runs until the profile is settled.
fn settle(cx: &mut AppContext) {
    display::settle(cx);
}

fn language(language: Language) {
    grainy_i18n::set_language(language);
}

// ---- the state against the oracle's DisplayColor ----------------------------------------------------------------------

#[test]
fn the_state_follows_the_oracles_display_color() {
    let _turn = common::language_lock();
    language(Language::Korean);
    let g = Case::open("app", "display_state").json().clone();
    let dir = common::temp_dir("display-state");
    let p3 = dir.join("p3.icc");
    let adobe = dir.join("adobe.icc");
    let notes = dir.join("notes.icc");
    std::fs::copy(inputs("Display_P3.icc"), &p3).unwrap();
    std::fs::copy(inputs("Adobe_RGB.icc"), &adobe).unwrap();
    std::fs::write(&notes, "this is not an ICC profile").unwrap();
    let (p3, adobe, notes) =
        (p3.to_string_lossy().into_owned(), adobe.to_string_lossy().into_owned(), notes.to_string_lossy().into_owned());

    let fake = Arc::new(FakeSystem::default());
    let mut cx = library_context(&dir);
    cx.state::<DisplayState>().set_system(fake.clone());
    let steps = g["steps"].as_array().unwrap();
    let check = |cx: &mut AppContext, index: usize| {
        let name = steps[index]["step"].as_str().unwrap();
        assert_eq!(snapshot(cx, &dir), golden_step(&steps[index]), "step {index}: {name}");
    };

    check(&mut cx, 0);
    settle(&mut cx);
    check(&mut cx, 1);
    *fake.path.lock().unwrap() = Some(p3.clone());
    cx.state::<DisplayState>().ask_again();
    settle(&mut cx);
    check(&mut cx, 2);
    cx.state::<DisplayState>().ask_again();
    settle(&mut cx);
    check(&mut cx, 3);
    *fake.path.lock().unwrap() = None;
    for (index, mode, path) in [
        (4, "manual", adobe.as_str()),
        (5, "manual", ""),
        (6, "manual", notes.as_str()),
        (7, "off", ""),
        (8, "auto", ""),
    ] {
        display::configure(&mut cx, mode, path).unwrap();
        settle(&mut cx);
        check(&mut cx, index);
    }
    let refusal = display::configure(&mut cx, "sepia", "").unwrap_err();
    assert_eq!(Some(refusal.as_str()), g["refusal"].as_str());
    check(&mut cx, 9);

    // The box: the oracle's steps on the same state.
    display::configure(&mut cx, "manual", &adobe).unwrap();
    settle(&mut cx);
    let mut dialog = DisplayDialog::new(&mut cx);
    let dialog_steps = g["dialog"].as_array().unwrap();
    let box_state = |dialog: &mut DisplayDialog, cx: &mut AppContext| {
        dialog.follow(cx);
        json!({
            "mode": dialog.mode(), "path": clean(dialog.path(), &dir), "path_enabled": dialog.fields_enabled(),
            "choose_enabled": dialog.fields_enabled(), "status": clean(&dialog.status(cx), &dir),
        })
    };
    let expect = |index: usize| {
        let mut want = dialog_steps[index].clone();
        want.as_object_mut().unwrap().remove("step");
        normalised(&want)
    };
    let name = |index: usize| dialog_steps[index]["step"].as_str().unwrap().to_owned();
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(0), "{}", name(0));
    dialog.set_mode("auto");
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(1), "{}", name(1));
    dialog.set_mode("manual");
    dialog.set_path("");
    dialog.apply(&mut cx);
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(2), "{}", name(2));
    dialog.set_path(&format!("  {p3}  "));
    dialog.apply(&mut cx);
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(3), "{}", name(3));
    settle(&mut cx);
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(4), "{}", name(4));
    display::configure(&mut cx, "off", "").unwrap();
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(5), "{}", name(5));
    settle(&mut cx);
    assert_eq!(normalised(&box_state(&mut dialog, &mut cx)), expect(6), "{}", name(6));
    assert_eq!(snapshot(&mut cx, &dir)["stored"], golden_step(&g["final"])["stored"], "{}", g["final"]["step"]);
    assert_eq!(snapshot(&mut cx, &dir)["button"], g["final"]["button"]);
}

/// A new library: the choice is read from `display_color`, else the legacy `monitor_icc` (a stored `null` of the
/// oracle's, "off", cannot be told from an unset one by the preference store and reads as auto).
#[test]
fn an_older_librarys_choice_is_read_like_the_oracle() {
    let _turn = common::language_lock();
    let g = Case::open("app", "display_state").json().clone();
    let adobe_file = inputs("Adobe_RGB.icc");
    for (index, row) in g["legacy"].as_array().unwrap().iter().enumerate() {
        let label = row["stored"].as_str().unwrap();
        if label == "null" {
            continue;
        }
        let dir = common::temp_dir(&format!("display-legacy{index}"));
        let adobe = dir.join("adobe.icc");
        std::fs::copy(&adobe_file, &adobe).unwrap();
        let adobe = adobe.to_string_lossy().into_owned();
        let mut document = CatalogDocument::open(&dir.join("Library")).unwrap();
        use grainy_app::document::Document;
        match label {
            "a path" => document.save_preference("monitor_icc", &json!(adobe)).unwrap(),
            "empty" => document.save_preference("monitor_icc", &json!("")).unwrap(),
            "a broken mode" => document.save_preference("display_color", &json!({"mode": "x", "path": "p"})).unwrap(),
            "the new choice wins" => {
                document.save_preference("display_color", &json!({"mode": "off", "path": ""})).unwrap();
                document.save_preference("monitor_icc", &json!(adobe)).unwrap();
            }
            _ => {}
        }
        let mut cx = context_on(Box::new(document));
        display::poll(&mut cx);
        let state = cx.state::<DisplayState>();
        assert_eq!(state.mode, row["mode"].as_str().unwrap(), "{label}");
        let want = row["path"].as_str().unwrap();
        let got = Path::new(&state.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        assert_eq!(got, want, "{label}");
        display::settle(&mut cx);
    }
}

#[test]
fn the_choice_is_saved_and_comes_back_in_the_next_window() {
    let dir = common::temp_dir("display-saved");
    let p3 = inputs("Display_P3.icc").to_string_lossy().into_owned();
    {
        let mut cx = library_context(&dir);
        display::configure(&mut cx, "manual", &p3).unwrap();
        settle(&mut cx);
        assert_eq!(cx.state::<DisplayState>().target().description, "RGB built-in");
        assert_eq!(cx.doc.preference("display_color"), Some(json!({"mode": "manual", "path": p3})));
        assert_eq!(cx.doc.preference("monitor_icc"), Some(json!(p3)));
    }
    let mut cx = library_context(&dir);
    display::poll(&mut cx);
    assert_eq!(cx.state::<DisplayState>().mode, "manual");
    assert_eq!(cx.state::<DisplayState>().path, p3);
    settle(&mut cx);
    assert!(display::monitor(&cx).is_some(), "the profile is used again");
    assert_eq!(display::revision(&cx), 1);
}

#[test]
fn the_texts_of_both_languages_are_the_oracles() {
    let _turn = common::language_lock();
    for (code, which) in [("ko", Language::Korean), ("en", Language::English)] {
        language(which);
        let g = Case::open("app", &format!("display_texts_{code}")).json().clone();
        let dir = common::temp_dir(&format!("display-texts-{code}"));
        let mut cx = library_context(&dir);
        let text = |key: &str| g[key].as_str().unwrap().to_owned();
        assert_eq!(cx.state::<DisplayState>().button_text(), text("button_initial"), "{code}");
        let items = DisplayDialog::mode_items("Windows");
        let names: Vec<&str> = g["items"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(items.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), names, "{code}");
        assert_eq!(tr("화면 색상 관리 — Grainy"), text("title"));
        let labels = g["labels"].as_array().unwrap();
        assert_eq!(tr("화면 색상 관리"), labels[0].as_str().unwrap());
        let note = tr("편집 화면·사진 목록·비교창의 표시 색에 적용합니다. 보정값과 내보내는 파일은 바뀌지 않습니다.\n자동 모드에서는 각 창이 있는 모니터를 따릅니다. Windows가 별도 프로파일을 제공하지 않으면 sRGB를 사용합니다.");
        assert_eq!(note, labels[2].as_str().unwrap(), "{code}");
        assert_eq!(tr("ICC 프로파일 경로"), text("placeholder"));
        let buttons: Vec<String> =
            g["buttons"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        assert_eq!(buttons, [tr("파일 선택"), tr("다시 확인"), tr("닫기"), tr("적용")], "{code}");
        // The status label before anything: monitor, profile.
        assert_eq!(cx.state::<DisplayState>().status_text(), "모니터: 기본 화면\n적용 프로파일: sRGB");
        assert_eq!(cx.state::<DisplayState>().status_text(), labels[1].as_str().unwrap());
        // The button after each choice.
        for (mode, key) in [("auto", "button_auto"), ("off", "button_off")] {
            display::configure(&mut cx, mode, "").unwrap();
            settle(&mut cx);
            assert_eq!(cx.state::<DisplayState>().button_text(), text(key), "{code} {mode}");
        }
        display::configure(&mut cx, "manual", "").unwrap();
        settle(&mut cx);
        assert_eq!(cx.state::<DisplayState>().button_text(), text("button_error"), "{code}");
        assert_eq!(cx.state::<DisplayState>().status_text(), text("status_error"), "{code}");
        let mut dialog = DisplayDialog::new(&mut cx);
        dialog.set_mode("manual");
        dialog.set_path("");
        dialog.apply(&mut cx);
        assert_eq!(dialog.status(&mut cx), text("status_no_file"), "{code}");
        dialog.set_path("x.icc");
        dialog.apply(&mut cx);
        assert_eq!(dialog.status(&mut cx), text("status_applying"), "{code}");
        settle(&mut cx);
    }
    language(Language::Korean);
}

// ---- the background ------------------------------------------------------------------------------------------------

fn hex(colour: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", colour.r(), colour.g(), colour.b())
}

#[test]
fn the_photo_background_follows_set_view_background() {
    let _turn = common::language_lock();
    let g = Case::open("app", "display_background").json().clone();
    let dir = common::temp_dir("display-background");
    let mut cx = library_context(&dir);
    display::poll(&mut cx);
    let keys: Vec<&str> = g["keys"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(BACKGROUNDS.iter().map(|(_, k, _)| *k).collect::<Vec<_>>(), keys);
    for (key, colour) in g["colours"].as_object().unwrap() {
        let ours = BACKGROUNDS.iter().find(|(_, k, _)| k == key).unwrap().2;
        assert_eq!(hex(ours), colour.as_str().unwrap(), "{key}");
    }
    assert_eq!(hex(display::colour_of("nothing")), g["default"].as_str().unwrap());
    let steps = g["steps"].as_array().unwrap();
    let checked = |cx: &AppContext| -> Vec<bool> {
        let current = display::background_key(cx);
        keys.iter().map(|k| *k == current).collect()
    };
    // Built: black is the stored default; the oracle's menu shows it checked.
    assert_eq!(
        checked(&cx),
        steps[0]["context_checked"].as_array().unwrap().iter().map(|v| v.as_bool().unwrap()).collect::<Vec<_>>()
    );
    for step in &steps[1..] {
        let key = match step["step"].as_str().unwrap() {
            "white" | "white again" => "white",
            "mid grey" => "mid_grey",
            "unknown key" => "bogus",
            "black" => "black",
            "dark grey" => "dark_grey",
            other => panic!("{other}"),
        };
        display::set_background(&mut cx, key);
        let want_view = step["view"].as_array().unwrap().last().unwrap().as_str().unwrap().to_owned();
        assert_eq!(hex(display::background(&cx)), want_view, "{}", step["step"]);
        let flags: Vec<bool> = step["menu_checked"].as_array().unwrap().iter().map(|v| v.as_bool().unwrap()).collect();
        assert_eq!(checked(&cx), flags, "{}", step["step"]);
        let saved = step["saved"].as_array().unwrap().last().unwrap()[1].as_str().unwrap().to_owned();
        assert_eq!(cx.doc.preference("view_background"), Some(json!(saved)), "{}", step["step"]);
    }
    // The next window starts with the stored background.
    drop(cx);
    let mut cx = library_context(&dir);
    display::poll(&mut cx);
    assert_eq!(display::background_key(&cx), "dark_grey");
    assert_eq!(hex(display::background(&cx)), "#2b2e33");
}

#[test]
fn the_menus_and_the_lights() {
    let _turn = common::language_lock();
    let g = Case::open("app", "display_background").json().clone();
    for (code, which) in [("ko", Language::Korean), ("en", Language::English)] {
        language(which);
        let items = g["menus"][code].as_array().unwrap();
        for ((label, _, _), item) in BACKGROUNDS.iter().zip(items) {
            assert_eq!(tr(label), item["text"].as_str().unwrap(), "{code}");
            assert_eq!(item["checkable"], json!(true));
        }
    }
    language(Language::Korean);
    // The View menu items are commands with the oracle's text.
    for (label, id) in [
        ("화면 색상 관리…", "display.dialog"),
        ("모니터 색상 자동 감지", "display.auto"),
        ("모니터 ICC 프로파일 선택", "display.choose"),
        ("모니터 ICC 보정 해제", "display.off"),
        ("사진 배경", "view.background"),
    ] {
        assert_eq!(grainy_app::commands::find(id).unwrap_or_else(|| panic!("{id}")).label, label);
    }
    // Lights out: the window's black behind the photo, the choice back afterwards (`toggle_lights`).
    let mut cx = context_on(Box::new(Session::default()));
    cx.run_with("view.background", &grainy_app::commands::Invocation { key: None, value: Some(json!("white")) });
    assert_eq!(hex(display::background(&cx)), "#ffffff");
    screen::toggle_lights(&mut cx);
    assert_eq!(hex(display::background(&cx)), "#101215", "the window's black, whatever was chosen");
    screen::toggle_lights(&mut cx);
    assert_eq!(hex(display::background(&cx)), "#ffffff");
}

// ---- the picture through the monitor profile ---------------------------------------------------------------------

fn wait(worker: &mut Worker) -> Vec<Reply> {
    let started = Instant::now();
    let mut replies = Vec::new();
    loop {
        replies.extend(worker.poll());
        if worker.idle() {
            replies.extend(worker.poll());
            return replies;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn frame(worker: &mut Worker, id: u64, settings: grainy_core::Settings) -> Arc<grainy_app::render::Frame> {
    worker.preview(PreviewJob {
        id,
        photo: 1,
        path: inputs("photo.png"),
        settings,
        resolution: Resolution::Fit,
        crop: grainy_engine::Crop::Cropped,
    });
    wait(worker)
        .into_iter()
        .find_map(|r| match r {
            Reply::Frame(f) => Some(f),
            _ => None,
        })
        .expect("a frame")
}

/// The oracle's `normalized({**defaults(), **changes})`: the defaults with the changes laid over them.
fn changed(changes: Value) -> grainy_core::Settings {
    let mut map = grainy_core::normalized(&serde_json::Map::new()).0;
    map.extend(changes.as_object().unwrap().clone());
    grainy_core::normalized(&map)
}

/// How many bytes of the frame differ from the oracle's quantised `display` and by how much at most.
fn compare(rgba: &[u8], expected: &ndarray::Array3<u8>) -> (usize, u8) {
    let (height, width, _) = expected.dim();
    assert_eq!(rgba.len(), width * height * 4);
    let (mut count, mut largest) = (0, 0u8);
    for y in 0..height {
        for x in 0..width {
            for c in 0..3 {
                let d = rgba[(y * width + x) * 4 + c].abs_diff(expected[[y, x, c]]);
                largest = largest.max(d);
                if d > 0 {
                    count += 1;
                }
            }
        }
    }
    (count, largest)
}

/// The frame the oracle shows with a monitor profile: `develop(output_space=None)` then `output_rgb(working, space,
/// profile)`. The histogram and the clipping stay those of the sRGB frame.
#[test]
fn a_frame_with_a_monitor_profile_equals_the_oracles_output_rgb() {
    for name in ["p3_exposure", "adobe_prophoto", "srgb_profile"] {
        let case = Case::open("app", &format!("display_frame_{name}"));
        let expected: ndarray::Array3<u8> = case.array("display");
        let settings = changed(case.json()["settings"].clone());
        let monitor = profile(&format!("{}.icc", case.json()["profile"].as_str().unwrap().replace(' ', "_")));
        let mut worker = Worker::new();
        let plain = frame(&mut worker, 1, settings.clone());
        worker.set_monitor(Some(monitor));
        let shown = frame(&mut worker, 2, settings);
        let (count, largest) = compare(&shown.preview.rgba, &expected);
        println!("{name}: {count} of {} bytes differ, largest {largest}", expected.len());
        assert!(largest <= 1, "{name}: largest difference {largest} levels, {count} bytes");
        // Not the sRGB picture (unless the profile is sRGB itself).
        if case.json()["profile"] != "sRGB" {
            assert_ne!(plain.preview.rgba, shown.preview.rgba, "{name}: the profile changed the picture");
        }
        // The analysis is that of the sRGB frame.
        assert_eq!(plain.bins, shown.bins, "{name}: histogram");
        assert_eq!(plain.channels, shown.channels, "{name}: clipping channels");
        assert_eq!(plain.clipping, shown.clipping, "{name}: clipping mask");
        // And the profile can be taken away again: the frame is byte-identical to the one before it.
        worker.set_monitor(None);
        let again = frame(&mut worker, 3, changed(case.json()["settings"].clone()));
        assert_eq!(again.preview.rgba, plain.preview.rgba, "{name}: back to sRGB");
    }
}

#[test]
fn without_a_profile_frames_are_what_they_were() {
    // The default settings on the stored photo, with and without a worker that was told `None`.
    let mut worker = Worker::new();
    let plain = frame(&mut worker, 1, common::settings(json!({})));
    worker.set_monitor(None);
    let again = frame(&mut worker, 2, common::settings(json!({})));
    assert_eq!(plain.preview.rgba, again.preview.rgba);
    let (rgba, width, height) = common::engine_render(&inputs("photo.png"), Some(2048), &common::settings(json!({})));
    assert_eq!((plain.preview.width, plain.preview.height), (width, height));
    assert_eq!(plain.preview.rgba, rgba, "the same bytes as the engine's own render");
}

#[test]
fn thumbnails_are_converted_like_the_oracles() {
    for (name, file) in [("p3", "Display_P3.icc"), ("adobe", "Adobe_RGB.icc")] {
        let case = Case::open("app", &format!("display_thumbnail_{name}"));
        let input: ndarray::Array3<u8> = case.array("input");
        let expected: ndarray::Array3<u8> = case.array("output");
        let (height, width, _) = input.dim();
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[input[[y, x, 0]], input[[y, x, 1]], input[[y, x, 2]], 255]);
            }
        }
        let converted = display::monitor_preview(Preview { width, height, rgba }, &profile(file)).unwrap();
        let (count, largest) = compare(&converted.rgba, &expected);
        println!("thumbnail {name}: {count} bytes differ, largest {largest}");
        assert!(largest <= 1, "{name}: {largest}");
        assert!(converted.rgba.chunks(4).all(|p| p[3] == 255));
    }
}

/// The thumbnail lane applies the profile it was given: a thumbnail made with the monitor profile is the plain one
/// converted, and one made again without it is the plain one.
#[test]
fn the_thumbnail_lane_uses_the_monitor_profile() {
    let job = |key: u64| ThumbnailJob {
        photo: 1,
        path: inputs("photo.png"),
        settings: common::settings(json!({})),
        key,
        cache: None,
    };
    let thumb = |worker: &mut Worker, key: u64| {
        worker.thumbnail(job(key));
        wait(worker)
            .into_iter()
            .find_map(|r| match r {
                Reply::Thumbnail(t) => t.result.ok(),
                _ => None,
            })
            .expect("a thumbnail")
    };
    let mut worker = Worker::new();
    let plain = thumb(&mut worker, 1);
    let monitor = profile("Display_P3.icc");
    worker.set_monitor(Some(monitor.clone()));
    let shown = thumb(&mut worker, 2);
    let expected = display::monitor_preview(plain.clone(), &monitor).unwrap();
    assert_eq!(shown.rgba, expected.rgba);
    assert_ne!(shown.rgba, plain.rgba);
    worker.set_monitor(None);
    assert_eq!(thumb(&mut worker, 3).rgba, plain.rgba);
}

// ---- the window ----------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The application with the real worker and a generated photo open, `setup` run before the first frame.
fn photo_harness<'a>(setup: impl FnOnce(&mut GrainyApp) + 'a) -> Harness<'a, GrainyApp> {
    let dir = common::temp_dir("display");
    let path: PathBuf = dir.join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![path]);
        setup(&mut app);
        app
    });
    settle_frame(&mut harness);
    pin_status(harness.state_mut(), "");
    harness
}

/// The profile file the pictures use, relative to the crate (a fixed text in the picture).
const SNAPSHOT_PROFILE: &str = "../../golden/app/display_inputs/Display_P3.icc";

fn both_languages(mut each: impl FnMut(Language, &str)) {
    let _turn = common::language_lock();
    for (which, suffix) in [(Language::Korean, "ko"), (Language::English, "en")] {
        language(which);
        each(which, suffix);
    }
    language(Language::Korean);
}

#[test]
fn display_dialog_with_a_monitor_profile() {
    both_languages(|_, suffix| {
        let mut harness = photo_harness(|app| {
            display::configure(&mut app.cx, "manual", SNAPSHOT_PROFILE).unwrap();
        });
        // The picture is in the monitor's colours before the box is opened.
        assert_eq!(harness.state_mut().cx.state::<DisplayState>().target().description, "RGB built-in");
        harness.state_mut().cx.run("display.dialog");
        harness.run_steps(3);
        settle_frame(&mut harness);
        pin_status(harness.state_mut(), "");
        harness.run_steps(2);
        save(&mut harness, &format!("display-dialog-{suffix}"));
    });
}

#[test]
fn display_error_state_in_the_status_button() {
    both_languages(|_, suffix| {
        let mut harness = photo_harness(|app| {
            display::configure(&mut app.cx, "manual", "").unwrap();
        });
        assert!(harness.state_mut().cx.state::<DisplayState>().target().error);
        harness.state_mut().cx.run("display.dialog");
        harness.run_steps(3);
        settle_frame(&mut harness);
        pin_status(harness.state_mut(), "");
        harness.run_steps(2);
        save(&mut harness, &format!("display-error-{suffix}"));
    });
}

/// The picture is made again when the profile arrives, in the monitor's colours (the frame of the window equals the
/// worker's), and again in sRGB when it is taken away.
#[test]
fn the_window_shows_the_picture_in_the_monitor_profile() {
    let _turn = common::language_lock();
    language(Language::Korean);
    let mut harness = photo_harness(|_| {});
    let plain = harness.state().frame().expect("a frame").preview.rgba.clone();
    harness.state_mut().cx.run("display.auto");
    settle_frame(&mut harness);
    assert_eq!(harness.state().frame().unwrap().preview.rgba, plain, "auto without a system profile is sRGB");

    let profile_path = std::fs::canonicalize(SNAPSHOT_PROFILE).unwrap().to_string_lossy().into_owned();
    display::configure(&mut harness.state_mut().cx, "manual", &profile_path).unwrap();
    // The new profile makes a new request.
    let started = Instant::now();
    loop {
        harness.step();
        if harness.state().frame().is_some_and(|f| f.preview.rgba != plain) && harness.state().idle() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the picture did not change");
        std::thread::sleep(Duration::from_millis(10));
    }
    let shown = harness.state().frame().unwrap().preview.rgba.clone();
    let mut worker = Worker::new();
    worker.set_monitor(Some(profile("Display_P3.icc")));
    let path = harness.state().cx.doc.current().map(|p| p.path.clone()).expect("a photo");
    worker.preview(PreviewJob {
        id: 1,
        photo: 1,
        path,
        settings: harness.state().settings().clone(),
        resolution: Resolution::Fit,
        crop: grainy_engine::Crop::Cropped,
    });
    let direct = wait(&mut worker)
        .into_iter()
        .find_map(|r| match r {
            Reply::Frame(f) => Some(f),
            _ => None,
        })
        .unwrap();
    assert_eq!(shown, direct.preview.rgba, "the window's frame is the worker's frame with the profile");

    // Off: sRGB again, the bytes of the first frame.
    harness.state_mut().cx.run("display.off");
    let started = Instant::now();
    loop {
        harness.step();
        if harness.state().frame().is_some_and(|f| f.preview.rgba == plain) && harness.state().idle() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the picture did not come back");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Right-click in the photo view: the four backgrounds, the chosen one checked; choosing one paints the view and is
/// remembered.
#[test]
fn display_background_menu() {
    both_languages(|_, suffix| {
        let mut harness = photo_harness(|app| {
            app.cx.run_with(
                "view.background",
                &grainy_app::commands::Invocation { key: None, value: Some(json!("mid_grey")) },
            );
        });
        // Right-click in a corner of the view, away from the photo.
        let at = Pos2::new(560.0, 150.0);
        harness.event(Event::PointerMoved(at));
        harness.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        harness.step();
        harness.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(4);
        save(&mut harness, &format!("display-background-menu-{suffix}"));
    });
}
