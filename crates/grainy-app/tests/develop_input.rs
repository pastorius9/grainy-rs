//! End-to-end input routing in the Develop inspector, including the undo boundary.

use eframe::egui::{self, Id, Key, Modifiers, Pos2};
use egui_kittest::Harness;
use grainy_app::{GrainyApp, Mode};

fn wait_for_photos(app: &mut Harness<'static, GrainyApp>, count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.state().document().len() < count || app.state().document().current().is_none() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {count} photos to open");
        app.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.run_steps(2);
}

fn window() -> (tempfile::TempDir, Harness<'static, GrainyApp>) {
    let files = tempfile::tempdir().unwrap();
    let paths = ["first.png", "second.png"].map(|name| {
        let file = files.path().join(name);
        image::RgbImage::new(8, 8).save(&file).unwrap();
        file
    });
    let mut app = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        app.open_paths(Vec::from(paths));
        app.set_mode(Mode::Develop);
        app
    });
    wait_for_photos(&mut app, 2);
    (files, app)
}

fn slider(key: &str) -> Id {
    Id::unique(("adjustment", key)).with("slider")
}

#[test]
fn track_press_jumps_immediately_then_drags_as_one_edit() {
    let (_files, mut app) = window();
    let track = app.ctx.read_response(slider("contrast")).unwrap().rect.shrink2(egui::vec2(4.0, 0.0));
    let point = |fraction| track.left_center().lerp(track.right_center(), fraction);
    for (start, end, first, last) in [(0.75, 0.0, 50.0, -100.0), (0.25, 1.0, -50.0, 100.0)] {
        app.hover_at(point(start));
        app.run_steps(1);
        app.drag_at(point(start));
        app.run_steps(1);
        assert_eq!(app.state().settings().f64("contrast"), first);
        app.hover_at(point(end));
        app.run_steps(1);
        assert_eq!(app.state().settings().f64("contrast"), last);
        app.drop_at(point(end));
        app.run_steps(2);
        app.state_mut().run("develop.undo");
        app.run_steps(2);
        assert_eq!(app.state().settings().f64("contrast"), 0.0);
    }
}

#[test]
fn a_single_track_click_sets_fractional_exposure() {
    let (_files, mut app) = window();
    let track = app.ctx.read_response(slider("exposure")).unwrap().rect.shrink2(egui::vec2(4.0, 0.0));
    let point = track.left_center().lerp(track.right_center(), 0.625);
    app.hover_at(point);
    app.run_steps(1);
    app.drag_at(point);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("exposure"), 1.25);
    app.drop_at(point);
    app.run_steps(2);
    assert_eq!(app.state().settings().f64("exposure"), 1.25);
    app.state_mut().run("develop.undo");
    app.run_steps(2);
    assert_eq!(app.state().settings().f64("exposure"), 0.0);
}

#[test]
fn basic_focus_hover_cycle_text_and_filmstrip() {
    let (_files, mut app) = window();
    let photo = app.state().document().current_index().unwrap();
    let point = app.ctx.read_response(slider("exposure")).unwrap().rect.center();
    app.hover_at(point);
    app.run_steps(1);
    app.key_press(Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("exposure"), 0.01);
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("exposure"), 0.11);
    assert_eq!(app.state().document().current_index(), Some(photo));

    app.key_press(Key::Period);
    app.run_steps(2);
    assert!(app.ctx.read_response(slider("contrast")).unwrap().has_focus());
    app.key_press(Key::Minus);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("contrast"), -1.0);
    app.key_press(Key::Comma);
    app.run_steps(2);
    assert!(app.ctx.read_response(slider("exposure")).unwrap().has_focus());

    let field = Id::unique(("adjustment", "exposure")).with("value");
    let pos = app.ctx.read_response(field).unwrap().rect.center();
    app.hover_at(pos);
    app.run_steps(1);
    app.drag_at(pos);
    app.run_steps(1);
    app.drop_at(pos);
    app.run_steps(1);
    assert!(app.ctx.text_edit_focused());
    app.event(egui::Event::Text("1.75".into()));
    app.key_press(Key::ArrowDown);
    app.run_steps(1);
    assert_eq!(app.state().document().current_index(), Some(photo));
    assert_eq!(app.state().settings().f64("exposure"), 1.74);
    app.key_press(Key::Enter);
    app.run_steps(2);
    assert_eq!(app.state().settings().f64("exposure"), 1.74);

    app.hover_at(Pos2::new(400.0, 200.0));
    app.run_steps(1);
    app.key_press(Key::ArrowDown);
    app.run_steps(2);
    assert_eq!(app.state().document().current_index(), Some(photo + 1));
}

#[test]
fn histogram_edits_only_the_initial_tone_zone_and_undoes_in_one_step() {
    let (_files, mut app) = window();
    let keys = ["blacks", "shadows", "exposure", "highlights", "whites"];
    for key in keys {
        let band = app.ctx.read_response(Id::unique(("histogram_tone", key))).unwrap().rect;
        let start = band.center();
        let direction = if key == "whites" { -1.0 } else { 1.0 };
        let end = start + egui::vec2(band.width() * direction, 0.0);
        app.hover_at(start);
        app.run_steps(1);
        app.drag_at(start);
        app.run_steps(1);
        app.hover_at(start.lerp(end, 0.5));
        app.run_steps(1);
        app.hover_at(end);
        app.run_steps(1);
        app.drop_at(end);
        app.run_steps(2);
        assert!(app.state().settings().f64(key) * f64::from(direction) > 0.0, "{key}");
        for other in keys.into_iter().filter(|other| *other != key) {
            assert_eq!(app.state().settings().f64(other), 0.0, "{key} changed {other}");
        }
        app.state_mut().run("develop.undo");
        app.run_steps(2);
        assert_eq!(app.state().settings().f64(key), 0.0, "one undo restores {key}");
    }
}

fn focus_field(app: &mut Harness<'static, GrainyApp>, key: &str) {
    let field = Id::unique(("adjustment", key)).with("value");
    let pos = app.ctx.read_response(field).unwrap().rect.center();
    app.hover_at(pos);
    app.run_steps(1);
    app.drag_at(pos);
    app.run_steps(1);
    app.drop_at(pos);
    app.run_steps(1);
    assert!(app.ctx.memory(|m| m.has_focus(field)), "field {key} should be focused");
}

#[test]
fn number_field_arrow_keys_fine_adjust_like_lightroom() {
    let (_files, mut app) = window();
    let photo = app.state().document().current_index().unwrap();

    // Exposure numeric box: Up = +0.01, Shift+Up = +0.10, Down = -0.01, Enter commits as one undo step.
    focus_field(&mut app, "exposure");
    app.key_press(Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("exposure"), 0.01);
    assert_eq!(app.state().document().current_index(), Some(photo));
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("exposure"), 0.11);
    app.key_press(Key::ArrowDown);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("exposure"), 0.10);
    assert!(app.ctx.text_edit_focused(), "focus stays in number field across arrow nudges");
    app.key_press(Key::Enter);
    app.run_steps(2);
    assert_eq!(app.state().settings().f64("exposure"), 0.10);
    app.state_mut().run("develop.undo");
    app.run_steps(2);
    assert_eq!(app.state().settings().f64("exposure"), 0.0);

    // Contrast numeric box: Up = +1, Shift+Up = +10, Escape reverts live changes & blurs.
    focus_field(&mut app, "contrast");
    app.key_press(Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("contrast"), 1.0);
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("contrast"), 11.0);
    app.key_press(Key::Escape);
    app.run_steps(2);
    assert!(!app.ctx.text_edit_focused());
    assert_eq!(app.state().settings().f64("contrast"), 0.0);

    // Works in non-Basic panels too (Detail -> sharpen) and clamps at bounds.
    app.state_mut().open_panel("detail");
    app.run_steps(2);
    focus_field(&mut app, "sharpen");
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("sharpen"), 10.0);
    app.key_press(Key::ArrowDown);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("sharpen"), 9.0);
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    app.run_steps(1);
    assert_eq!(app.state().settings().f64("sharpen"), 0.0);
    app.key_press(Key::Enter);
    app.run_steps(2);
}

#[test]
fn filmstrip_supports_shift_click_ctrl_click_shift_arrow_and_select_all() {
    let files = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..5)
        .map(|i| {
            let file = files.path().join(format!("photo_{i}.png"));
            image::RgbImage::new(8, 8).save(&file).unwrap();
            file
        })
        .collect();
    let mut app = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        app.open_paths(paths);
        app.set_mode(Mode::Develop);
        app
    });
    wait_for_photos(&mut app, 5);
    assert_eq!(app.state().document().selected_indices(), vec![0]);

    let click_cell = |app: &mut Harness<'static, GrainyApp>, index: usize, modifiers: Modifiers| {
        let id = Id::unique(("filmstrip_cell", index));
        let pos = app.ctx.read_response(id).expect("filmstrip cell visible").rect.center();
        app.hover_at(pos);
        app.run_steps(1);
        app.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers });
        app.run_steps(1);
        app.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers });
        app.run_steps(1);
    };

    // Shift+Click on cell 3 selects range 0..=3 while keeping photo 0 active.
    click_cell(&mut app, 3, Modifiers::SHIFT);
    assert_eq!(app.state().document().current_index(), Some(0));
    assert_eq!(app.state().document().selected_indices(), vec![0, 1, 2, 3]);

    // Shift+Click on cell 1 shrinks range to 0..=1 from the same anchor.
    click_cell(&mut app, 1, Modifiers::SHIFT);
    assert_eq!(app.state().document().selected_indices(), vec![0, 1]);

    // Ctrl/Cmd+Click toggles cell 4 into the selection.
    click_cell(&mut app, 4, Modifiers::COMMAND);
    assert_eq!(app.state().document().selected_indices(), vec![0, 1, 4]);

    // Plain click on cell 1 resets selection to [1].
    click_cell(&mut app, 1, Modifiers::NONE);
    assert_eq!(app.state().document().current_index(), Some(1));
    assert_eq!(app.state().document().selected_indices(), vec![1]);

    // Shift+Right extends range to [1, 2], then [1, 2, 3].
    app.hover_at(Pos2::new(400.0, 200.0));
    app.run_steps(1);
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    app.run_steps(1);
    assert_eq!(app.state().document().selected_indices(), vec![1, 2]);
    app.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    app.run_steps(1);
    assert_eq!(app.state().document().selected_indices(), vec![1, 2, 3]);

    // Ctrl/Cmd+A selects all 5 photos; Ctrl/Cmd+D deselects back to [1].
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.run_steps(1);
    assert_eq!(app.state().document().selected_indices(), vec![0, 1, 2, 3, 4]);
    app.key_press_modifiers(Modifiers::COMMAND, Key::D);
    app.run_steps(1);
    assert_eq!(app.state().document().selected_indices(), vec![1]);
}

#[test]
fn auto_color_strength_slider_appears_and_adjusts_live() {
    let files = tempfile::tempdir().unwrap();
    let file = files.path().join("green_cast.png");
    // Construct a 64x64 image with a clear green cast across shadows, midtones, and highlights.
    let mut img = image::RgbImage::new(64, 64);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let base = if y < 22 {
            40u8
        } else if y < 44 {
            120u8
        } else {
            200u8
        };
        let tweak = (x % 3) as u8;
        *px = image::Rgb([base.saturating_sub(14) + tweak, base.saturating_add(16), base.saturating_sub(8)]);
    }
    img.save(&file).unwrap();

    let mut app = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        app.open_paths(vec![file]);
        app.set_mode(Mode::Develop);
        app
    });
    wait_for_photos(&mut app, 1);

    // Before Auto Color, the strength slider is hidden to preserve Basic panel density.
    assert!(app.ctx.read_response(slider("auto_color_amount")).is_none());

    // Run Auto Color and wait for background analysis to apply.
    assert!(app.state_mut().run("develop.auto_color"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.ctx.read_response(slider("auto_color_amount")).is_none() {
        assert!(std::time::Instant::now() < deadline, "auto_color_amount slider did not appear");
        app.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let curves_100 = app.state().settings().value("rgb_curves").clone();

    // Drag the Auto Color Strength slider to 50% and verify RGB curves blend smoothly in real time.
    let track = app.ctx.read_response(slider("auto_color_amount")).unwrap().rect.shrink2(egui::vec2(4.0, 0.0));
    let mid = track.left_center().lerp(track.right_center(), 0.5);
    app.hover_at(mid);
    app.run_steps(1);
    app.drag_at(mid);
    app.run_steps(1);
    app.drop_at(mid);
    app.run_steps(2);
    let curves_50 = app.state().settings().value("rgb_curves").clone();
    assert_ne!(curves_50, curves_100, "50% strength must soften the auto-color curves");
    assert!(app.state().status().contains("50%"), "status reflects 50%: {}", app.state().status());

    // Undo restores 100% strength while keeping the slider active; second Undo removes Auto Color and hides slider.
    app.state_mut().run("develop.undo");
    app.run_steps(2);
    assert_eq!(app.state().settings().value("rgb_curves"), &curves_100);
    assert!(app.ctx.read_response(slider("auto_color_amount")).is_some());

    app.state_mut().run("develop.undo");
    app.run_steps(2);
    assert!(app.ctx.read_response(slider("auto_color_amount")).is_none());
}
