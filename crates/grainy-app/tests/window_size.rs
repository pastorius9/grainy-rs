//! Any window size is safe: a program can give the window any size (`SetWindowPos`, `MoveWindow`), below the
//! minimum it asks for (1100 × 700) down to 1 × 1 and 0 × 0 (minimised), or larger than the GPU can draw.
//!
//! The release program quit without a message when a script resized its window: a window larger than the GPU's
//! largest texture (32 984 pixels wide on a GPU of 32 768) made `wgpu` refuse the window's surface, an error it turns
//! into a panic by default (window_state.rs, "a window of any size"). These tests draw the window headless at every small size in each view, with the start-up
//! boxes open, at other display scales and with the GPU preview, and fail on any panic; and they make the window
//! larger than the GPU can draw and check that it goes back to a size the GPU can draw (window_state.rs).
//!
//! Every test works on temporary folders and generated photos; no library of the owner is opened.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::library_column::LAUNCH_DISABLED;
use grainy_app::GrainyApp;

/// Widths and heights below the window's minimum (1100 × 700), down to one point.
const WIDTHS: [f32; 11] = [1099.0, 900.0, 700.0, 520.0, 400.0, 300.0, 200.0, 120.0, 60.0, 20.0, 1.0];
const HEIGHTS: [f32; 8] = [699.0, 500.0, 340.0, 200.0, 100.0, 40.0, 10.0, 1.0];

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1280.0, 720.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library in a temporary folder with three generated photos, the first open.
fn window<'a>(name: &str) -> Harness<'a, GrainyApp> {
    let dir = common::temp_dir(name);
    let photos = [dir.join("garden.png"), dir.join("garden two.png"), dir.join("garden three.png")];
    common::save_png(&common::garden(2, [1.0; 3], false), &photos[0]);
    common::save_png(&common::garden(2, [0.8, 1.0, 1.1], true), &photos[1]);
    common::save_png(&common::garden(1, [1.2, 1.0, 0.8], false), &photos[2]);
    let library = dir.join("Library");
    let data = common::data_dir(&dir);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app.open_paths(photos.to_vec());
        app
    });
    common::snapshot::settle(&mut harness);
    harness
}

/// Draws the window at every size of [`WIDTHS`] × [`HEIGHTS`], then 0 × 0, then its usual size again.
fn every_size(harness: &mut Harness<'_, GrainyApp>, screen: &str) {
    for &h in &HEIGHTS {
        for &w in &WIDTHS {
            harness.set_size(egui::vec2(w, h));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                harness.run_steps(2);
                harness.render().expect("render");
            }));
            if let Err(panic) = result {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default();
                panic!("{screen} at {w} x {h}: {message}");
            }
        }
    }
    // Minimised: the system reports a window of no size.
    harness.set_size(egui::vec2(0.0, 0.0));
    harness.run_steps(3);
    harness.set_size(egui::vec2(1280.0, 720.0));
    harness.run_steps(2);
    harness.render().expect("render at the usual size again");
}

#[test]
fn the_window_survives_every_size_in_every_screen() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(grainy_i18n::Language::Korean);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let mut harness = window("window-size");
    let screens: [(&str, &[&str]); 9] = [
        ("Develop, fitted", &["view.develop", "view.fit"]),
        ("Develop, 1:1", &["view.actual"]),
        ("Library grid", &["view.fit", "view.library"]),
        ("compare", &["view.develop", "view.compare"]),
        ("survey", &["view.compare", "view.survey"]),
        ("crop", &["view.survey", "develop.crop"]),
        ("no side columns", &["develop.crop", "view.sides"]),
        ("no bars", &["view.sides", "view.bars"]),
        ("lights out", &["view.bars", "view.lights"]),
    ];
    for (screen, commands) in screens {
        for command in commands {
            harness.state_mut().cx.run(command);
        }
        harness.set_size(egui::vec2(1280.0, 720.0));
        common::snapshot::settle(&mut harness);
        every_size(&mut harness, screen);
    }
}

/// The window as `main` builds it, with the GPU preview sharing the window's `wgpu` device (zero-copy textures,
/// the 1:1 tiles), on an in-memory session of generated photos (`--memory --offline`: nothing is written).
#[test]
fn the_gpu_preview_survives_every_size() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(grainy_i18n::Language::Korean);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let dir = common::temp_dir("window-size-gpu");
    let photos = vec![dir.join("garden.png"), dir.join("garden two.png")];
    common::save_png(&common::garden(8, [1.0; 3], false), &photos[0]);
    common::save_png(&common::garden(8, [0.8, 1.0, 1.1], true), &photos[1]);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let args = grainy_app::paths::Arguments { memory: true, offline: true, paths: photos, ..Default::default() };
        let mut app = GrainyApp::from_arguments_with_wgpu(&cc.egui_ctx, cc.wgpu_render_state.clone(), args);
        app.set_debug_overlay(false);
        app
    });
    common::snapshot::settle(&mut harness);
    let screens: [(&str, &[&str]); 4] = [
        ("Develop, fitted (GPU)", &["view.develop", "view.fit"]),
        ("Develop, 1:1 (GPU tiles)", &["view.actual"]),
        ("compare (GPU)", &["view.compare"]),
        ("Library grid (GPU)", &["view.compare", "view.fit", "view.library"]),
    ];
    for (screen, commands) in screens {
        for command in commands {
            harness.state_mut().cx.run(command);
        }
        harness.set_size(egui::vec2(1280.0, 720.0));
        common::snapshot::settle(&mut harness);
        every_size(&mut harness, screen);
    }
}

/// A window larger than the GPU's largest texture: the window asks to go back to the largest size the GPU can
/// draw, at every display scale, and is drawn there.
#[test]
fn a_window_larger_than_the_gpu_can_draw_goes_back_to_a_drawable_size() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(grainy_i18n::Language::Korean);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let dir = common::temp_dir("window-size-too-large");
    let photos = vec![dir.join("garden.png")];
    common::save_png(&common::garden(2, [1.0; 3], false), &photos[0]);
    let max_side = Arc::new(AtomicU32::new(0));
    let seen = max_side.clone();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let state = cc.wgpu_render_state.clone().expect("the window's wgpu device");
        seen.store(state.device.limits().max_texture_dimension_2d, Ordering::Relaxed);
        let args = grainy_app::paths::Arguments { memory: true, offline: true, paths: photos, ..Default::default() };
        let mut app = GrainyApp::from_arguments_with_wgpu(&cc.egui_ctx, Some(state), args);
        app.set_debug_overlay(false);
        app
    });
    common::snapshot::settle(&mut harness);
    let max = max_side.load(Ordering::Relaxed) as f32;
    assert!(max >= 2048.0, "the device's largest texture: {max}");
    for ppp in [1.0, 1.5, 2.0] {
        // The harness takes no new size in the frame its scale changes.
        harness.set_pixels_per_point(ppp);
        harness.run_steps(2);
        // Three times wider than the GPU can draw, and a few points high (a small picture to render).
        harness.set_size(egui::vec2(3.0 * max / ppp, 12.0));
        harness.run_steps(3);
        let size = harness.ctx.viewport_rect().size();
        // The largest whole number of points the GPU draws, the height kept.
        assert_eq!(size, egui::vec2((max / ppp).floor(), 12.0), "at {ppp}x, largest texture {max}");
        assert!(size.x * ppp <= max);
        harness.render().expect("drawn at the size the window went back to");
        // Sizes the GPU can draw are left alone.
        harness.set_size(egui::vec2(1280.0, 720.0));
        harness.run_steps(2);
        assert_eq!(harness.ctx.viewport_rect().size(), egui::vec2(1280.0, 720.0), "at {ppp}x");
    }
}

#[test]
fn the_start_up_boxes_survive_every_size() {
    use grainy_app::dialogs::about::about_box;
    use grainy_app::dialogs::language::LanguageDialog;
    use grainy_app::dialogs::shortcuts::ShortcutList;
    use grainy_app::dialogs::update;
    use grainy_app::dialogs::whats_new::WhatsNew;
    use grainy_app::dialogs::Dialog;
    let _turn = common::language_lock();
    grainy_i18n::set_language(grainy_i18n::Language::Korean);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let mut harness = window("window-size-boxes");
    let boxes: Vec<(&str, Box<dyn Dialog>)> = vec![
        ("language", Box::new(LanguageDialog::new(None, true, false))),
        ("what's new", Box::new(WhatsNew::this_version())),
        ("after a crash", Box::new(update::crash_box())),
        ("first run", Box::new(update::first_run_box())),
        ("folder guide", Box::new(grainy_app::startup::guide_box("D:\\Photos", false))),
        ("about", Box::new(about_box())),
        ("shortcuts", Box::new(ShortcutList)),
    ];
    for (name, dialog) in boxes {
        harness.set_size(egui::vec2(1280.0, 720.0));
        let id = dialog.id();
        harness.state_mut().cx.open_dialog(dialog);
        common::snapshot::settle(&mut harness);
        every_size(&mut harness, name);
        harness.state_mut().cx.close_dialog(id);
    }
}

#[test]
fn every_size_at_other_display_scales() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(grainy_i18n::Language::Korean);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let mut harness = window("window-size-scales");
    for ppp in [1.25, 1.5, 1.75, 2.0, 3.0] {
        harness.set_pixels_per_point(ppp);
        for (screen, command) in [("Develop", "view.develop"), ("Library", "view.library")] {
            harness.state_mut().cx.run(command);
            harness.set_size(egui::vec2(1280.0, 720.0));
            common::snapshot::settle(&mut harness);
            every_size(&mut harness, &format!("{screen} at {ppp}x"));
        }
    }
}
