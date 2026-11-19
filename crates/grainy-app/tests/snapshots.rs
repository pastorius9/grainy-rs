//! Renders the window headless (egui_kittest on wgpu) at 1600×1000 in Korean and English and writes the pictures
//! to `tests/snapshots/`. The harness panics on a missing glyph, so a passing run also means every character of
//! the interface was found in the loaded fonts.
//!
//! The shell's own snapshots use the test card; the photo snapshots open generated photos (from the garden test
//! input) through the real path (the background worker and the engine) and compare the photo on screen with
//! `grainy_engine::develop` of the same settings.

mod common;

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::{GrainyApp, Mode, Zoom};
use grainy_core::Settings;
use grainy_i18n::Language;
use serde_json::json;

fn builder() -> HarnessBuilder<GrainyApp> {
    builder_sized(1600.0, 1000.0)
}

fn builder_sized(width: f32, height: f32) -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(width, height)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The real application path (`eframe::App::ui`), as `cargo run` draws it, with the test card.
fn harness<'a>(setup: impl FnOnce(&mut GrainyApp) + 'a) -> Harness<'a, GrainyApp> {
    harness_sized(builder(), setup)
}

fn harness_sized<'a>(
    builder: HarnessBuilder<GrainyApp>,
    setup: impl FnOnce(&mut GrainyApp) + 'a,
) -> Harness<'a, GrainyApp> {
    builder.wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        setup(&mut app);
        app
    })
}

/// The application with the real worker and `paths` opened.
fn photo_harness<'a>(paths: Vec<PathBuf>) -> Harness<'a, GrainyApp> {
    builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(paths);
        app
    })
}

/// Runs frames until the app has loaded, rendered and shown everything it asked for.
fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) -> image::RgbaImage {
    common::snapshot::save(harness, name)
}

fn render(language: Language, name: &str, setup: impl FnOnce(&mut GrainyApp) + 'static) {
    render_sized(language, name, builder(), setup);
}

fn render_sized(
    language: Language,
    name: &str,
    builder: HarnessBuilder<GrainyApp>,
    setup: impl FnOnce(&mut GrainyApp) + 'static,
) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = harness_sized(builder, setup);
    harness.run_steps(4);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

/// The difference between the photo on screen and the engine's render, over a window of `size` screen pixels at
/// the photo's centre: (largest, mean) per channel value.
fn compare(screen: &image::RgbaImage, app: &GrainyApp, expected: &(Vec<u8>, usize, usize), size: f32) -> (u8, f64) {
    let rect = app.photo_rect().expect("the photo was drawn");
    let (rgba, w, h) = expected;
    let (sx, sy) = (rect.width() / *w as f32, rect.height() / *h as f32);
    let centre = rect.center();
    let (mut largest, mut total, mut count) = (0u8, 0u64, 0u64);
    let half = size / 2.0;
    let (x0, x1) = ((centre.x - half).max(rect.left()) as u32, (centre.x + half).min(rect.right()) as u32);
    let (y0, y1) = ((centre.y - half).max(rect.top()) as u32, (centre.y + half).min(rect.bottom()) as u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let px = (((x as f32 + 0.5 - rect.left()) / sx) as usize).min(w - 1);
            let py = (((y as f32 + 0.5 - rect.top()) / sy) as usize).min(h - 1);
            let shown = screen.get_pixel(x, y).0;
            for c in 0..3 {
                let d = shown[c].abs_diff(rgba[(py * w + px) * 4 + c]);
                largest = largest.max(d);
                total += u64::from(d);
                count += 1;
            }
        }
    }
    (largest, total as f64 / count.max(1) as f64)
}

/// One 1440×960 photo (larger than the view at 1:1).
fn one_photo(name: &str) -> PathBuf {
    let dir = common::temp_dir(name);
    let path = dir.join("garden-large.png");
    common::save_png(&common::garden(6, [1.0; 3], false), &path);
    path
}

fn edits() -> Settings {
    common::settings(json!({
        "exposure": 0.7,
        "contrast": 15,
        "film_look": "kodak-gold-200",
        "rotation": 1,
        "crop": [0.1, 0.05, 0.85, 0.9],
        "masks": [{"adjustments": {"exposure": -1.0, "saturation": -60.0},
                   "components": [{"type": "radial", "angle": 25.0, "feather": 60, "points": [[0.3, 0.3], [0.7, 0.8]]}]}]
    }))
}

fn photo_snapshot(
    language: Language,
    name: &str,
    path: &Path,
    setup: impl FnOnce(&mut GrainyApp),
) -> (Harness<'static, GrainyApp>, image::RgbaImage) {
    grainy_i18n::set_language(language);
    let mut harness = photo_harness(vec![path.to_path_buf()]);
    settle(&mut harness);
    setup(harness.state_mut());
    settle(&mut harness);
    let image = save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
    (harness, image)
}

#[test]
fn develop_screen_korean() {
    render(Language::Korean, "develop-ko", |_| {});
}

#[test]
fn develop_screen_english() {
    render(Language::English, "develop-en", |_| {});
}

/// Edited values: the dots, a panel switched off and the overlay of changed settings.
#[test]
fn develop_screen_edited() {
    render(Language::Korean, "develop-edited-ko", |app| {
        app.set_debug_overlay(true);
        let edit = app.edit_state_mut();
        edit.set("exposure", 0.75);
        edit.set("contrast", 20.0);
        edit.commit("보정 변경");
        edit.set("vignette", 30.0);
        edit.commit("보정 변경");
        edit.switch_panel("effects", false);
        edit.set("tone_mapping", "drt");
        edit.commit("톤 매핑");
    });
}

#[test]
fn library_screen_korean() {
    render(Language::Korean, "library-ko", |app| app.set_mode(Mode::Library));
}

/// Every panel and fold-out open (solo mode off) in a tall window, to check every row at Korean and English
/// widths; the RAW white balance, the negative's sliders and the DRT's Advanced sliders shown too.
fn all_panels(app: &mut GrainyApp) {
    app.set_debug_overlay(false);
    app.set_solo(false);
    app.set_raw_photo(true);
    app.set_drt_advanced(true);
    for name in ["wb_detail", "lens_manual", "channel_curve", "point_color", "raw_profile", "camera_matrix"] {
        app.open_foldout(name);
    }
    for (key, _) in grainy_app::panels::PANELS {
        if !grainy_app::panels::TOOLS.contains(&key) {
            app.open_panel(key);
        }
    }
    let edit = app.edit_state_mut();
    edit.set("negative", true);
    edit.set("tone_mapping", "drt");
    edit.commit("보정 변경");
}

#[test]
fn all_panels_korean() {
    render_sized(Language::Korean, "all-panels-ko", builder_sized(1600.0, 6400.0), all_panels);
}

#[test]
fn all_panels_english() {
    render_sized(Language::English, "all-panels-en", builder_sized(1600.0, 6400.0), all_panels);
}

/// The Crop tool's panel.
#[test]
fn crop_tool_english() {
    render(Language::English, "crop-en", |app| {
        app.set_debug_overlay(false);
        app.open_tool(grainy_app::panels::Tool::Crop);
    });
}

#[test]
fn shortcuts_switch_screens_and_panels() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut harness = harness(|_| {});
    harness.run_steps(2);
    harness.key_press(egui::Key::G);
    harness.run_steps(2);
    assert_eq!(harness.state().mode(), Mode::Library);
    harness.key_press(egui::Key::D);
    harness.run_steps(2);
    assert_eq!(harness.state().mode(), Mode::Develop);
    harness.key_press(egui::Key::Tab);
    harness.run_steps(2);
    assert!(!harness.state().sides_visible());
    harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    harness.run_steps(2);
    let app = harness.state();
    assert!(!app.sides_visible() && !app.bars_visible(), "Shift+Tab hides everything unless all is hidden");
    harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    harness.run_steps(2);
    let app = harness.state();
    assert!(app.sides_visible() && app.bars_visible(), "a second Shift+Tab shows everything again");
    harness.key_press(egui::Key::Questionmark);
    harness.run_steps(2);
    assert!(harness.state().shortcut_list_open());
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(!harness.state().shortcut_list_open());
}

/// A real photo at the default settings, fitted: the screen shows the engine's develop (scaled by the GPU).
#[test]
fn develop_photo_at_defaults() {
    let _turn = common::language_lock();
    let path = one_photo("defaults");
    let (harness, image) = photo_snapshot(Language::Korean, "develop-photo-ko", &path, |_| {});
    let app = harness.state();
    let frame = app.frame().expect("a frame").clone();
    assert_eq!((frame.preview.width, frame.preview.height), (1440, 960));
    assert!(app.status().contains("1440 × 960"), "the status shows the render: {}", app.status());
    let expected = common::engine_render(&path, Some(2048), &common::settings(json!({})));
    assert!(expected.0 == frame.preview.rgba, "the worker's frame is the engine's develop");
    let (largest, mean) = compare(&image, app, &expected, 300.0);
    println!("fit view at defaults: largest {largest}, mean {mean:.3}");
    assert!(mean < 4.0, "fit view differs from the engine by {mean:.3} on average");
}

/// Exposure, a film look, a crop and a rotation and a radial mask from JSON settings.
#[test]
fn develop_photo_with_edits() {
    let _turn = common::language_lock();
    let path = one_photo("edits");
    for (language, name) in
        [(Language::English, "develop-photo-edited-en"), (Language::Korean, "develop-photo-edited-ko")]
    {
        let (harness, image) = photo_snapshot(language, name, &path, |app| {
            app.edit_state_mut().replace(edits(), "보정 변경");
            app.open_panel("film");
        });
        let app = harness.state();
        let frame = app.frame().expect("a frame").clone();
        let expected = common::engine_render(&path, Some(2048), &edits());
        assert_eq!((frame.preview.width, frame.preview.height), (expected.1, expected.2));
        assert!(frame.preview.height > frame.preview.width, "rotated a quarter turn");
        assert!(expected.0 == frame.preview.rgba, "the worker's frame is the engine's develop");
        let (largest, mean) = compare(&image, app, &expected, 300.0);
        println!("{name}: largest {largest}, mean {mean:.3}");
        assert!(mean < 4.0, "edited view differs from the engine by {mean:.3} on average");
    }
}

/// 1:1: each photo pixel on one screen pixel, exactly the engine's bytes.
#[test]
fn actual_size_is_exact() {
    let _turn = common::language_lock();
    let path = one_photo("actual");
    let (harness, image) = photo_snapshot(Language::Korean, "actual-size-ko", &path, |app| app.set_zoom(Zoom::Actual));
    let app = harness.state();
    let rect = app.photo_rect().expect("drawn");
    assert_eq!((rect.width(), rect.height()), (1440.0, 960.0));
    let expected = common::engine_render(&path, None, &common::settings(json!({})));
    let (largest, mean) = compare(&image, app, &expected, 400.0);
    println!("1:1: largest {largest}, mean {mean:.4}");
    assert_eq!(largest, 0, "1:1 shows the engine's pixels exactly");
}

/// `\`: the undeveloped default render of an edited photo.
#[test]
fn before_shows_the_default_render() {
    let _turn = common::language_lock();
    let path = one_photo("before");
    let (harness, image) = photo_snapshot(Language::Korean, "before-ko", &path, |app| {
        app.edit_state_mut().replace(edits(), "보정 변경");
        app.set_before(true);
    });
    let app = harness.state();
    let expected = common::engine_render(&path, Some(2048), &common::settings(json!({})));
    assert!(app.frame().is_some_and(|f| f.preview.rgba == expected.0), "the before view is the default develop");
    let (_, mean) = compare(&image, app, &expected, 300.0);
    assert!(mean < 4.0);
    assert_eq!(app.edit_state().settings().f64("exposure"), 0.7, "the edits stay");
}

/// The clipping overlay (J) on a bright photo.
#[test]
fn clipping_overlay() {
    let _turn = common::language_lock();
    let path = one_photo("clipping");
    let (harness, _) = photo_snapshot(Language::Korean, "clipping-ko", &path, |app| {
        app.edit_state_mut().replace(common::settings(json!({"exposure": 1.5, "blacks": -60})), "보정 변경");
        app.set_clipping(true);
    });
    let frame = harness.state().frame().expect("frame").clone();
    assert!(frame.channels.1.iter().any(|v| *v), "highlights clip at +1.5 EV");
}

/// Eight photos in the Library grid, with thumbnails made with each photo's settings.
#[test]
fn library_grid_with_eight_photos() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("library");
    common::eight_photos(&dir);
    let mut harness = photo_harness(vec![dir]);
    settle(&mut harness);
    assert_eq!(harness.state().document().len(), 8);
    // The second photo gets its own edit; its thumbnail follows.
    harness.state_mut().select_photo(1);
    harness.state_mut().edit_state_mut().replace(common::settings(json!({"monochrome": true})), "보정 변경");
    harness.state_mut().set_mode(Mode::Library);
    settle(&mut harness);
    save(&mut harness, "library-photos-ko");
    let app = harness.state();
    assert!(app.status().is_empty() || !app.status().contains("처리하지"), "{}", app.status());
}

/// Arrow keys open the next and previous photos; each keeps its edits.
#[test]
fn arrow_keys_switch_photos() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("arrows");
    let paths = common::eight_photos(&dir);
    let mut harness = photo_harness(paths[..3].to_vec());
    settle(&mut harness);
    harness.state_mut().edit_state_mut().set("exposure", 1.0);
    harness.key_press(egui::Key::ArrowRight);
    settle(&mut harness);
    assert_eq!(harness.state().document().current_index(), Some(1));
    assert_eq!(harness.state().edit_state().settings().f64("exposure"), 0.0);
    harness.key_press(egui::Key::ArrowLeft);
    settle(&mut harness);
    assert_eq!(harness.state().document().current_index(), Some(0));
    assert_eq!(harness.state().edit_state().settings().f64("exposure"), 1.0);
    assert_eq!(harness.state().frame().map(|f| f.photo), Some(harness.state().document().current_id()));
}

/// A develop the engine cannot do yet: the message in the status bar, the last frame stays.
#[test]
fn an_error_keeps_the_last_frame() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let path = one_photo("error");
    let mut harness = photo_harness(vec![path]);
    settle(&mut harness);
    let before = harness.state().frame().expect("a frame").id;
    let lens = json!({"lensfun_enabled": true, "lensfun": {"crop": 1.0, "focal": 50.0, "xml": "<lens/>"}});
    harness.state_mut().edit_state_mut().replace(common::settings(lens), "보정 변경");
    settle(&mut harness);
    let app = harness.state();
    assert!(app.status().contains("렌즈"), "{}", app.status());
    assert_eq!(app.frame().map(|f| f.id), Some(before), "the last good frame stays");
}

/// The export dialog in both languages.
#[test]
fn export_dialog() {
    let _turn = common::language_lock();
    let path = one_photo("export_dialog");
    for (language, name) in [(Language::Korean, "export-dialog-ko"), (Language::English, "export-dialog-en")] {
        photo_snapshot(language, name, &path, |app| app.open_export_dialog());
    }
}

/// A library on disk (temporary): a folder imported in place on the import job, thumbnails from the library's
/// cache, a rating key and the automatic tone as a background job, both stored in the catalogue.
#[test]
fn library_on_disk() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("catalog-ui");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).expect("folder");
    common::eight_photos(&photos);
    let library = dir.join("Library");
    let mut harness = builder().wgpu().build_eframe(|cc| {
        let doc = grainy_app::document::catalog::CatalogDocument::open(&library).expect("library");
        let mut app =
            GrainyApp::with_document(&cc.egui_ctx, Box::new(grainy_app::worker::Worker::new()), Box::new(doc));
        app.open_paths(vec![photos.clone()]);
        app.set_mode(Mode::Library);
        app
    });
    settle(&mut harness);
    assert_eq!(harness.state().document().len(), 8, "{}", harness.state().status());
    // Which folders of the tree are open depends on when the listing grew: open the drives only.
    common::snapshot::open_folders(harness.state_mut(), &[]);
    settle(&mut harness);
    save(&mut harness, "library-catalog-ko");
    let id = harness.state().document().current_id() as i64;
    assert!(id > 0);
    // Rating key 3 on the open photo, stored.
    harness.key_press(egui::Key::Num3);
    settle(&mut harness);
    let library = dir.join("Library");
    let catalog = grainy_catalog::Catalog::open_reader(&library).expect("reader");
    assert_eq!(catalog.photo(id).unwrap().unwrap()["rating"], json!(3));
    drop(catalog);
    // Auto tone runs on a job and commits "자동톤".
    harness.state_mut().set_mode(Mode::Develop);
    assert!(harness.state_mut().run("develop.auto_tone"));
    settle(&mut harness);
    let catalog = grainy_catalog::Catalog::open_reader(&library).expect("reader");
    let history = catalog.histories(id).unwrap();
    assert_eq!(history.first().and_then(|h| h["label"].as_str()), Some("자동톤"), "{}", harness.state().status());
    assert!(catalog.photo(id).unwrap().unwrap()["settings"]["exposure"].as_f64().unwrap() != 0.0);
    drop(catalog);
    save(&mut harness, "develop-catalog-ko");
}

/// The Python Grainy's library format opened read-only (a copy of the oracle-built golden library): the
/// "읽기 전용" badge, edits kept in memory, the catalogue file unchanged.
#[test]
fn read_only_library_badge() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("read-only-ui");
    let library = dir.join("Library");
    std::fs::create_dir_all(&library).expect("folder");
    let start = grainy_testkit::root().join("golden/app/library/start.sqlite");
    std::fs::copy(&start, library.join("catalog.sqlite")).expect("copy");
    let scratch = dir.join("scratch");
    let opened = library.clone();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let doc = grainy_app::document::catalog::CatalogDocument::open_read_only(&opened, &scratch).expect("library");
        let mut app =
            GrainyApp::with_document(&cc.egui_ctx, Box::new(grainy_app::worker::Worker::new()), Box::new(doc));
        app.set_mode(Mode::Library);
        app
    });
    settle(&mut harness);
    assert!(harness.state().read_only_library());
    assert_eq!(harness.state().document().len(), 8);
    harness.state_mut().edit_state_mut().set("exposure", 1.0);
    harness.state_mut().edit_state_mut().commit("보정 변경");
    harness.key_press(egui::Key::Num5);
    settle(&mut harness);
    save(&mut harness, "library-readonly-ko");
    assert!(
        std::fs::read(library.join("catalog.sqlite")).unwrap() == std::fs::read(&start).unwrap(),
        "nothing written"
    );
}
