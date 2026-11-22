//! End-to-end input routing in the Develop inspector, including the undo boundary.

use eframe::egui::{self, Id, Key, Modifiers, Pos2};
use egui_kittest::Harness;
use grainy_app::{GrainyApp, Mode};

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
    app.run_steps(4);
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
