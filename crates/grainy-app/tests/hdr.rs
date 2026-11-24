//! The HDR editing mode and the video rule (APP_PLAN package 32).
//!
//! The oracle hides the mode (`engine.HDR_FEATURE = False`); its goldens (`tools/golden/gen_app_hdr.py`,
//! `golden/app_hdr/`) are made with the switch on inside the generator, and every test here switches it on in this
//! test process (`grainy_app::hdr::enable`, as `main.rs` does for the application). The other test files run with
//! the switch off, as the oracle ships.
//!
//! * `normalized` keeps `hdr` with the switch on (and the edit's tone version follows).
//! * The preview path of an HDR photo (`render`'s `work()`): the limited working pixels from the engine, then the
//!   screen picture, the HDR histogram, the clipping channels, the screen ranges and the clipping overlay, for the
//!   default view, the SDR preview, the HDR range view and the ProPhoto working space.
//! * The export dialog's format list with the HDR TIFF and the fields it enables.
//! * The render worker answers an HDR photo with the HDR frame; the HDR button is one undo step.
//! * Snapshots `hdr-*` in Korean and English: the HDR section and histogram, the SDR preview with the HDR range view.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::export::{self, ExportOptions, FORMATS};
use grainy_app::hdr::{self, View};
use grainy_app::render::{PreviewJob, Renderer, Reply, Resolution};
use grainy_app::worker::Worker;
use grainy_app::GrainyApp;
use grainy_core::{normalized, Image, Settings};
use grainy_engine::{develop, Crop, Options, Output};
use grainy_i18n::Language;
use grainy_testkit::Case;
use ndarray::{Array2, Array3};
use serde_json::{json, Map, Value};

fn on() {
    hdr::enable();
}

fn map(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn normalized_keeps_hdr_when_the_mode_is_on() {
    on();
    let case = Case::open("app_hdr", "normalized");
    let golden = case.json();
    let ours = normalized(&map(&golden["stored"]));
    for (key, value) in golden["on"].as_object().unwrap() {
        assert_eq!(ours.0.get(key), Some(value), "{key}");
    }
    assert!(ours.bool("hdr"));
    assert_eq!(golden["off"]["hdr"], json!(false));
    assert_ne!(golden["on"]["tone_version"], golden["off"]["tone_version"], "the case tells the two apart");
}

fn bytes_max_diff(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

/// The preview path of the oracle's `render` for HDR photos, against `golden/app_hdr/present_*`.
#[test]
fn the_hdr_preview_matches_the_oracle() {
    on();
    let names =
        ["wide_default", "garden_limit", "colours_prophoto", "garden_sdr", "colours_visualize", "wide_low_limit"];
    for name in names {
        let case = Case::open("app_hdr", &format!("present_{name}"));
        let golden = case.json();
        let source: Image = case.array("source");
        let stored = map(&golden["settings"]);
        let settings = normalized(&stored);
        assert!(settings.bool("hdr"), "{name}: the mode keeps hdr");
        // The engine's limited working pixels (`develop(..., output_space=None)`).
        let expected_working: Image = case.array("working");
        let options = Options { use_crop: Crop::Cropped, output: Output::Working, ..Options::default() };
        let working = develop(&source, &stored, &options).expect("develop");
        let (error, _) = grainy_testkit::max_abs_diff(&working, &expected_working);
        // Measured: 2.4e-7 (float32 rounding order in the engine's stages).
        assert!(error <= 2e-4, "{name}: working pixels differ by {error}");
        // From the oracle's working pixels on: the display path of this package.
        let expected_hdr: Image = case.array("hdr_pixels");
        let hdr_pixels = hdr::to_srgb_extended(&expected_working, settings.str("working_space")).unwrap();
        let (error, _) = grainy_testkit::max_abs_diff(&hdr_pixels, &expected_hdr);
        // Measured: 0 (sRGB) and 4.8e-7 (ProPhoto through LittleCMS).
        assert!(error <= 1e-5, "{name}: to_srgb_extended differs by {error}");
        let view = View {
            sdr_preview: golden["sdr_preview"].as_bool().unwrap(),
            visualize: golden["visualize"].as_bool().unwrap(),
            ..View::default()
        };
        let shown = hdr::present(&expected_working, &settings, view, None).unwrap();
        let display: Array3<u8> = case.array("display");
        let (h, w, _) = display.dim();
        assert_eq!((shown.height, shown.width), (h, w), "{name}");
        let rgb: Vec<u8> = shown.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let diff = bytes_max_diff(&rgb, display.as_standard_layout().as_slice().unwrap());
        assert!(diff <= 1, "{name}: the screen picture differs by {diff} levels");
        let bins: Array2<i64> = case.array("bins");
        let mut off = 0;
        for c in 0..3 {
            for b in 0..128 {
                off += (i64::from(shown.bins[c][b]) - bins[[c, b]]).abs();
            }
        }
        // log2 of float32 may round across a bin edge in a rare pixel; measured: 0.
        assert!(off <= 2, "{name}: the histogram differs by {off} counts");
        let flags = |key: &str| -> [bool; 3] {
            let v = golden[key].as_array().unwrap();
            [v[0].as_bool().unwrap(), v[1].as_bool().unwrap(), v[2].as_bool().unwrap()]
        };
        assert_eq!(shown.channels, (flags("shadows"), flags("highlights")), "{name}: clipping channels");
        assert_eq!(shown.info.inside, golden["inside"].as_bool().unwrap(), "{name}");
        assert_eq!(shown.info.outside, golden["outside"].as_bool().unwrap(), "{name}");
        assert_eq!(shown.info.preview, view.sdr_preview);
        assert!((hdr::encoded_ceiling(settings.f64("hdr_limit")) - golden["ceiling"].as_f64().unwrap()).abs() < 1e-12);
        let highlights: Array2<u8> = case.array("highlight_mask");
        let shadows: Array2<u8> = case.array("shadow_mask");
        for (i, bits) in shown.mask.iter().enumerate() {
            let (y, x) = (i / w, i % w);
            assert_eq!(bits & grainy_app::histogram::HIGHLIGHT != 0, highlights[[y, x]] != 0, "{name} at {y},{x}");
            assert_eq!(bits & grainy_app::histogram::SHADOW != 0, shadows[[y, x]] != 0, "{name} at {y},{x}");
        }
    }
}

#[test]
fn the_export_dialog_lists_the_hdr_tiff() {
    on();
    let case = Case::open("app_hdr", "formats");
    let golden = case.json();
    let listed: Vec<&str> = export::listed_formats().iter().map(|i| FORMATS[*i]).collect();
    let mut expected: Vec<&str> = golden["listed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    expected.push("AVIF HDR 10-bit");
    assert_eq!(listed, expected);
    let mut pq = ExportOptions::default();
    pq.set_format(FORMATS.iter().position(|f| *f == "AVIF HDR 10-bit").unwrap());
    assert!(pq.fields().quality && pq.fields().keep_metadata);
    assert!(!pq.fields().color_space && !pq.fields().dither && !pq.fields().grayscale);
    for row in golden["rows"].as_array().unwrap() {
        let name = row["format"].as_str().unwrap();
        let mut options = ExportOptions { color_space: 1, ..ExportOptions::default() };
        options.set_format(FORMATS.iter().position(|f| *f == name).unwrap());
        let fields = options.fields();
        let enabled = &row["enabled"];
        for (key, value) in [
            ("quality", fields.quality),
            ("color_space", fields.color_space),
            ("longest", fields.longest),
            ("keep_metadata", fields.keep_metadata),
            ("grayscale", fields.grayscale),
            ("dither", fields.dither),
        ] {
            // The legacy dialog exposed a quality control for the lossless JPEG XL encoder.
            let expected = enabled[key].as_bool().unwrap() && !(name == "JPEG XL" && key == "quality");
            assert_eq!(value, expected, "{name} {key}");
        }
    }
    // A saved HDR TIFF export is valid while the mode is on (`quick_export.previous`).
    let saved = json!({"folder": "C:/out", "format": "TIFF HDR 32-bit", "quality": 95, "longest": 0,
        "color_space": "sRGB", "keep_metadata": true, "naming": "{stem}_grainy"});
    assert!(export::previous(Some(&saved)).is_some());
}

#[test]
fn videos_are_recognised_by_their_extension() {
    for (name, video) in
        [("a.mp4", true), ("b.MOV", true), ("c.avi", true), ("d.mkv", true), ("e.m4v", true), ("f.jpg", false)]
    {
        assert_eq!(grainy_app::video::is_video(std::path::Path::new(name)), video, "{name}");
    }
    assert!(!grainy_app::video::is_video(std::path::Path::new("mp4")));
}

/// A bright test photo (the garden input, 8-bit sRGB PNG) in a fixed folder.
fn garden_photo(name: &str) -> PathBuf {
    let path = common::temp_dir(name).join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    path
}

fn hdr_settings() -> Settings {
    on();
    common::settings(json!({"hdr": true, "exposure": 1.5, "hdr_limit": 3.0}))
}

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

/// The worker answers an HDR photo through the HDR path: the frame equals `present` of the engine's working pixels,
/// for the default view and the SDR preview.
#[test]
fn the_worker_renders_hdr_frames() {
    let settings = hdr_settings();
    let path = garden_photo("hdr-worker");
    let loaded = grainy_engine::files::load_photo(&path, None, &settings).expect("load");
    let (h, w, _) = loaded.photo.pixels().dim();
    let options = Options { output: Output::Working, original_size: Some((w as f64, h as f64)), ..Options::default() };
    let working = develop(loaded.photo.source(), &settings.0, &options).expect("develop");
    let mut worker = Worker::new();
    for (id, view) in [(1, View::default()), (2, View { sdr_preview: true, ..View::default() })] {
        worker.set_hdr_view(view);
        worker.preview(PreviewJob {
            id,
            photo: 1,
            path: path.clone(),
            settings: settings.clone(),
            resolution: Resolution::Fit,
            crop: Crop::Cropped,
        });
        let frame = wait(&mut worker)
            .into_iter()
            .find_map(|r| match r {
                Reply::Frame(frame) => Some(frame),
                _ => None,
            })
            .expect("a frame");
        let expected = hdr::present(&working, &settings, view, None).unwrap();
        let info = frame.hdr.expect("an HDR frame");
        assert_eq!(info, expected.info);
        assert!(info.outside, "exposure +1.5 puts the highlights above white");
        assert_eq!(frame.preview.rgba, expected.rgba, "view {view:?}");
        assert_eq!(frame.bins, expected.bins);
        assert_eq!(frame.clipping, expected.mask);
    }
}

// ---- the window ------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn photo_harness<'a>(path: PathBuf) -> Harness<'a, GrainyApp> {
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![path]);
        app.open_panel("basic");
        app
    });
    common::snapshot::settle_frame(&mut harness);
    harness
}

/// Scrolls the Develop column down so the HDR section is on screen.
fn scroll_column(harness: &mut Harness<'_, GrainyApp>) {
    harness.event(egui::Event::PointerMoved(egui::pos2(1420.0, 600.0)));
    harness.run_steps(1);
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -420.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(4);
    // The pointer leaves the column, so no tooltip covers the picture.
    harness.event(egui::Event::PointerMoved(egui::pos2(700.0, 960.0)));
    harness.run_steps(4);
}

/// The HDR button, the HDR section and the HDR histogram; then the SDR preview with the HDR range view.
fn hdr_in_the_window(language: Language, suffix: &str) {
    let _turn = common::language_lock();
    on();
    grainy_i18n::set_language(language);
    let mut harness = photo_harness(garden_photo(&format!("hdr-window-{suffix}")));
    {
        let cx = &mut harness.state_mut().cx;
        cx.doc.edit_mut().set("exposure", 1.5);
        cx.doc.edit_mut().commit("보정 변경");
    }
    common::snapshot::settle(&mut harness);
    assert!(harness.state().frame().is_some_and(|f| f.hdr.is_none()), "an ordinary photo has no HDR frame");
    let undo = harness.state().edit_state().depth().0;
    harness.state_mut().cx.run("develop.hdr");
    assert!(harness.state().settings().bool("hdr"));
    assert_eq!(harness.state().edit_state().depth().0, undo + 1, "the HDR button is one undo step");
    common::snapshot::settle(&mut harness);
    let info = harness.state().frame().and_then(|f| f.hdr).expect("an HDR frame on screen");
    assert!(info.outside && !info.preview);
    scroll_column(&mut harness);
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    common::snapshot::save(&mut harness, &format!("hdr-panel-{suffix}"));

    hdr::set_view(&mut harness.state_mut().cx, View { sdr_preview: true, visualize: true, ..View::default() });
    common::snapshot::settle(&mut harness);
    let info = harness.state().frame().and_then(|f| f.hdr).expect("an HDR frame on screen");
    assert!(info.preview);
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    common::snapshot::save(&mut harness, &format!("hdr-preview-{suffix}"));

    // HDR off again: the section goes away, the frame is an ordinary one.
    harness.state_mut().cx.run("develop.hdr");
    common::snapshot::settle(&mut harness);
    assert!(!harness.state().settings().bool("hdr"));
    assert!(harness.state().frame().is_some_and(|f| f.hdr.is_none()));
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn hdr_in_the_window_korean() {
    hdr_in_the_window(Language::Korean, "ko");
}

#[test]
fn hdr_in_the_window_english() {
    hdr_in_the_window(Language::English, "en");
}

#[test]
fn hdr_photo_disables_the_soft_proof_menu() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let _turn = common::language_lock();
    on();
    let mut harness = photo_harness(garden_photo("hdr-proof-menu"));
    harness.state_mut().cx.run("develop.hdr");
    common::snapshot::settle(&mut harness);
    harness.get_by_label(&grainy_i18n::tr("보기·필터")).click();
    harness.run_steps(4);
    assert!(harness.get_by_label(&grainy_i18n::tr("소프트 프루핑")).accesskit_node().is_disabled());
}
