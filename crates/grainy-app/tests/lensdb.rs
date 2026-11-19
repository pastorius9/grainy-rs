//! The lens database built into the program (HANDOFF2 section C).
//!
//! * The built-in database is the oracle's: its automatic choice (`optics.match_metadata`, `automatic_profile`)
//!   for the CC0 samples' cameras and lenses and for real camera/lens pairs over the makers and mounts of the
//!   database equals the Rust choice, the profile numbers included (`tools/golden/gen_app_lensdb.py`,
//!   `golden/app/lensdb_pairs`).
//! * Where the database is read from: setting or command line, then the data folder's `lensfun`, then the
//!   built-in one; a bare program works with no setup.
//! * The real window with no folder anywhere: the dialog lists the built-in cameras and lenses; snapshots
//!   `lensdb-*` in Korean and English. The About box carries the Lensfun attribution (tests/settings.rs).

mod common;

use std::path::PathBuf;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use grainy_app::commands::lens::{self as lens, LensState, Source};
use grainy_app::paths::Locations;
use grainy_app::GrainyApp;
use grainy_i18n::Language;
use grainy_lens::Database;
use grainy_testkit::Case;
use serde_json::Value;

use common::snapshot::{save, settle};

/// Locations whose data folder is `home` (never this machine's real one).
fn locations(home: &std::path::Path) -> Locations {
    let mut locations = Locations::current();
    locations.home_override = Some(home.to_path_buf());
    locations
}

#[test]
fn the_database_is_in_the_program() {
    let db = Database::builtin().expect("the embedded database parses");
    let case = Case::open("app", "lensdb_pairs");
    let g = case.json();
    assert_eq!(Database::builtin_file_count() as u64, g["database_files"].as_u64().unwrap());
    assert_eq!(db.cameras().len() as u64, g["cameras"].as_u64().unwrap());
    assert_eq!(db.lens_records().len() as u64, g["lenses"].as_u64().unwrap());
    // Unmodified data under its licence: the notice sits next to the files.
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("grainy-lens").join("data").join("lensfun");
    let notice = std::fs::read_to_string(folder.join("NOTICE")).expect("the notice file");
    assert!(notice.contains("CC BY-SA 3.0") && notice.contains("unmodified"));
    // The embedded database equals the folder it came from.
    let from_folder = Database::from_folder(&folder).unwrap();
    assert_eq!(from_folder.cameras().len(), db.cameras().len());
    assert_eq!(
        from_folder.lens_records().iter().map(|r| &r.id).collect::<Vec<_>>(),
        db.lens_records().iter().map(|r| &r.id).collect::<Vec<_>>()
    );
}

fn close(a: &Value, b: &Value, at: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!((x - y).abs() <= 1e-9 * (1.0 + y.abs()), "{at}: {x} != {y}");
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{at}: length");
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                close(p, q, &format!("{at}[{i}]"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            let (mut kx, mut ky): (Vec<_>, Vec<_>) = (x.keys().collect(), y.keys().collect());
            kx.sort();
            ky.sort();
            assert_eq!(kx, ky, "{at}: keys");
            for (k, v) in x {
                close(v, &y[k], &format!("{at}.{k}"));
            }
        }
        _ => assert_eq!(a, b, "{at}"),
    }
}

fn terms(c: Option<grainy_lens::Calibration>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => serde_json::json!({
            "model": c.model,
            "terms": c.terms.iter().map(|t| f64::from(*t)).collect::<Vec<_>>()
        }),
    }
}

/// The oracle's automatic choice with its full database equals the Rust choice with the built-in one.
#[test]
fn the_automatic_choice_equals_the_oracles() {
    let db = Database::builtin().unwrap();
    let case = Case::open("app", "lensdb_pairs");
    let cases = case.json()["cases"].as_array().unwrap();
    let (mut with_profile, mut samples, mut pairs, mut makers) = (0, 0, 0, std::collections::BTreeSet::new());
    for entry in cases {
        let origin = entry["origin"].as_str().unwrap();
        let at = format!("{origin} {}", entry["metadata"]);
        let m = db.match_metadata(&entry["metadata"]);
        assert_eq!(serde_json::to_value(m.camera.map(|c| &c.model)).unwrap(), entry["camera"], "{at}: camera");
        if let Some(camera) = m.camera {
            assert_eq!(camera.variant, entry["camera_variant"].as_str().unwrap(), "{at}: variant");
        }
        assert_eq!(
            serde_json::to_value(m.candidates.iter().map(|r| &r.id).collect::<Vec<_>>()).unwrap(),
            entry["candidates"],
            "{at}: candidates"
        );
        assert_eq!(m.reason, entry["reason"].as_str().unwrap(), "{at}: reason");
        let (profile, why) = db.automatic_profile(&entry["metadata"]);
        assert_eq!(why, entry["profile_reason"].as_str().unwrap(), "{at}: profile reason");
        match (&profile, &entry["profile"]) {
            (None, Value::Null) => {}
            (Some(p), expected) => {
                with_profile += 1;
                let mut got = p.clone();
                let xml = got.as_object_mut().unwrap().remove("xml").unwrap();
                assert_eq!(
                    xml.as_str().unwrap().chars().count() as u64,
                    expected["xml_length"].as_u64().unwrap(),
                    "{at}: xml"
                );
                let lens = grainy_lens::saved_lens(xml.as_str().unwrap()).unwrap();
                let focal = p["focal"].as_f64().unwrap() as f32;
                let aperture = p["aperture"].as_f64().unwrap() as f32;
                let distance = p["distance"].as_f64().unwrap() as f32;
                let mut expected = expected.clone();
                let object = expected.as_object_mut().unwrap();
                object.remove("xml_sha256");
                object.remove("xml_length");
                let (d, t, v) = (object.remove("distortion"), object.remove("tca"), object.remove("vignetting"));
                close(&got, &expected, &at);
                // The numbers the profile's lens gives at its conditions (interpolated terms, f32 as lensfun).
                let vignetting = if aperture > 0.0 {
                    terms(lens.interpolate_vignetting(focal, aperture, distance))
                } else {
                    Value::Null
                };
                for (name, actual, wanted) in [
                    ("distortion", terms(lens.interpolate_distortion(focal)), d),
                    ("tca", terms(lens.interpolate_tca(focal)), t),
                    ("vignetting", vignetting, v),
                ] {
                    let wanted = wanted.unwrap();
                    assert_eq!(actual.is_null(), wanted.is_null(), "{at}: {name} availability");
                    if !actual.is_null() {
                        assert_eq!(actual["model"], wanted["model"], "{at}: {name} model");
                        let (a, b) = (actual["terms"].as_array().unwrap(), wanted["terms"].as_array().unwrap());
                        for (a, b) in a.iter().zip(b) {
                            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
                            assert!((a - b).abs() <= 1e-5, "{at}: {name} term {a} != {b}");
                        }
                    }
                }
                assert_eq!(grainy_lens::description(p).unwrap(), entry["description"].as_str().unwrap(), "{at}");
            }
            (None, expected) => panic!("{at}: no profile, the oracle has {expected}"),
        }
        if origin.starts_with("sample") {
            samples += 1;
        }
        if origin.starts_with("pair:") {
            pairs += 1;
            makers.insert(entry["metadata"]["make"].as_str().unwrap().to_owned());
        }
    }
    println!(
        "{} cases: {samples} sample ones, {pairs} database pairs over {} makers, {with_profile} with a profile",
        cases.len(),
        makers.len()
    );
    assert!(samples >= 12, "the CC0 samples' cameras: {samples}");
    assert!(pairs >= 30, "thirty or more pairs: {pairs}");
    assert!(makers.len() >= 15, "spread over makers: {}", makers.len());
}

#[test]
fn where_the_database_comes_from() {
    let root = common::temp_dir("lensdb-source");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let here = locations(&home);
    // Nothing anywhere: the built-in database.
    assert_eq!(lens::database_source(None, Some(&here)), Source::Builtin);
    // A `lensfun` folder without database files does not count.
    std::fs::create_dir_all(home.join("lensfun")).unwrap();
    std::fs::write(home.join("lensfun").join("readme.txt"), "x").unwrap();
    assert_eq!(lens::database_source(None, Some(&here)), Source::Builtin);
    // The person's own folder in the data folder beats the built-in one.
    std::fs::write(home.join("lensfun").join("mine.xml"), "<lensdatabase version=\"1\"></lensdatabase>").unwrap();
    assert_eq!(lens::database_source(None, Some(&here)), Source::Folder(home.join("lensfun")));
    // A setting or `--lensfun` is taken as given, even when it is not there.
    let explicit = root.join("elsewhere");
    assert_eq!(lens::database_source(Some(&explicit), Some(&here)), Source::Folder(explicit));
    // Among folders: the first that holds files, else the built-in one.
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("x.xml"), "<lensdatabase version=\"1\"></lensdatabase>").unwrap();
    assert_eq!(lens::source_among(&[a.clone(), b.clone()]), Source::Folder(b));
    assert_eq!(lens::source_among(&[a]), Source::Builtin);
    // The person's folder is read like the oracle's: its files only.
    let mine = Database::from_folder(home.join("lensfun")).unwrap();
    assert!(mine.cameras().is_empty() && mine.lens_records().is_empty());
}

fn builder() -> egui_kittest::HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn quiet(harness: &mut Harness<'_, GrainyApp>) {
    harness.ctx.all_styles_mut(|style| style.visuals.text_cursor.blink = false);
}

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

/// No folder anywhere: the dialog works from the built-in database.
fn window(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(&format!("lensdb-window-{tag}"));
    let photo = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &photo);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.cx.state::<LensState>().locations = Some(locations(&home));
        app.open_paths(vec![photo]);
        app
    });
    quiet(&mut harness);
    settle(&mut harness);
    harness.state_mut().open_panel("lens");
    harness.run_steps(3);
    press(&mut harness, &grainy_app::text::tr("렌즈 자동 선택"));
    settle(&mut harness);
    assert!(harness.state().cx.dialog_open("lens"), "the lens dialog is open");
    {
        let state = harness.state_mut().cx.state::<LensState>();
        let database = state.database().expect("the database was read").clone();
        assert!(state.database_error().is_none(), "no 'database not found' note");
        assert!(database.cameras().len() > 900 && database.lens_records().len() > 1000, "the full database");
    }
    type_into(&mut harness, 0, "D750");
    harness.get_by_label("Nikon D750").click();
    harness.run_steps(3);
    type_into(&mut harness, 1, "Nikkor AI-S 85mm f/2.0");
    save(&mut harness, &format!("lensdb-dialog-{tag}"));
    harness.get_all_by_label_contains("Nikkor AI-S 85mm f/2.0").next().expect("the lens in the list").click();
    harness.run_steps(3);
    type_into(&mut harness, 4, "2");
    harness.key_press(egui::Key::Enter);
    harness.run_steps(3);
    press(&mut harness, &lens::tr("보정 적용"));
    settle(&mut harness);
    assert!(!harness.state().cx.dialog_open("lens"));
    let profile = harness.state().cx.settings().value("lensfun").clone();
    assert_eq!(profile["name"], "Nikkor AI-S 85mm f/2.0");
    assert_eq!(profile["camera"], "Nikon D750");
    assert!(harness.state().cx.settings().bool("lensfun_enabled"));
    save(&mut harness, &format!("lensdb-panel-{tag}"));
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_window_in_korean() {
    window(Language::Korean, "ko");
}

#[test]
fn the_window_in_english() {
    window(Language::English, "en");
}
