//! The automatic monitor profile (HANDOFF2 B): the window's monitor is looked up when the window moves (or every
//! `SCREEN_CHECK`), and a different monitor asks for its own profile (`DisplayTarget.bind` / `screenChanged` ->
//! `schedule` of luma/display_color.py). The operating-system calls themselves are tested in grainy-platform
//! (`monitor::tests`, against the oracle's `windows_profile_path`, plus a smoke test of the real calls); here the
//! system is a fake that gives each monitor its own profile, so no test reads this machine's monitor profile.

mod common;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::display::{self, DisplayState, ScreenWatch, SCREEN_CHECK};
use grainy_app::session::Session;
use grainy_app::{AppContext, GrainyApp};
use grainy_cms::display::SystemProfiles;
use grainy_i18n::Language;

use common::snapshot::{pin_status, save, settle_frame};

/// The second monitor's profile, relative to the crate (a fixed text in the picture).
const SECOND_PROFILE: &str = "../../golden/app/display_inputs/Display_P3.icc";
const FIRST: &str = r"\\.\DISPLAY1";
const SECOND: &str = r"\\.\DISPLAY2";

/// Windows as the tests see it: each monitor's profile (none: sRGB), and the screens asked for, in order.
#[derive(Default)]
struct Monitors {
    profiles: HashMap<String, String>,
    asked: Mutex<Vec<String>>,
}

impl Monitors {
    fn two() -> Arc<Self> {
        let mut profiles = HashMap::new();
        profiles.insert(SECOND.to_owned(), SECOND_PROFILE.to_owned());
        Arc::new(Self { profiles, asked: Mutex::default() })
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl SystemProfiles for Monitors {
    fn is_mac(&self) -> bool {
        false
    }

    fn windows_profile_path(&self, screen: &str) -> Result<Option<String>, String> {
        self.asked.lock().unwrap().push(screen.to_owned());
        Ok(self.profiles.get(screen).cloned())
    }

    fn mac_profile_data(&self, _screen: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}

fn context() -> AppContext {
    AppContext::new(
        &egui::Context::default(),
        Box::new(grainy_app::render::TestCard::default()),
        Box::new(Session::default()),
    )
}

#[test]
fn the_monitor_is_looked_up_when_the_window_moves_and_every_few_seconds() {
    let mut watch = ScreenWatch::default();
    let start = Instant::now();
    let place = [1, 2, 3, 4, 5, 6, 7];
    assert!(watch.due(place, start), "the first frame looks");
    assert!(!watch.due(place, start + Duration::from_millis(16)), "a window that stays is not looked up every frame");
    let moved = [9, 2, 3, 4, 5, 6, 7];
    assert!(watch.due(moved, start + Duration::from_millis(32)), "a moved window is looked up at once");
    assert!(!watch.due(moved, start + Duration::from_millis(48)));
    let scaled = [9, 2, 3, 4, 5, 6, 8];
    assert!(watch.due(scaled, start + Duration::from_millis(64)), "a changed scale is looked up at once");
    let later = start + Duration::from_millis(64) + SCREEN_CHECK;
    assert!(watch.due(scaled, later), "a window that stays is looked up again after SCREEN_CHECK");
    assert!(!watch.due(scaled, later + Duration::from_millis(16)));
    assert!(SCREEN_CHECK < display::POLL, "the cheap monitor check is more frequent than the profile poll");
}

#[test]
fn a_window_on_another_monitor_gets_that_monitors_profile() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let monitors = Monitors::two();
    let mut cx = context();
    cx.state::<DisplayState>().set_system(monitors.clone());
    assert!(cx.state::<DisplayState>().screen_seen(FIRST));
    display::settle(&mut cx);
    let first = cx.state::<DisplayState>().target().clone();
    assert_eq!((first.screen.as_str(), first.digest.as_str(), first.error), (FIRST, "srgb", false));
    assert_eq!(first.message, "Windows 기본 색상(sRGB)을 사용합니다.");
    let revision = display::revision(&cx);
    assert!(display::monitor(&cx).is_none());

    // The same monitor again (the window moved on it): nothing is asked.
    let asked = monitors.asked().len();
    assert!(!cx.state::<DisplayState>().screen_seen(FIRST));
    display::settle(&mut cx);
    assert_eq!(monitors.asked().len(), asked);

    // Moved to the second monitor: its profile, a new revision (the preview and the thumbnails are made again).
    assert!(cx.state::<DisplayState>().screen_seen(SECOND));
    display::settle(&mut cx);
    let second = cx.state::<DisplayState>().target().clone();
    assert_eq!(monitors.asked().last().map(String::as_str), Some(SECOND));
    assert_eq!((second.screen.as_str(), second.path.as_str(), second.error), (SECOND, SECOND_PROFILE, false));
    assert_eq!(second.message, "Windows가 현재 모니터에 제공한 프로파일입니다.");
    assert_eq!(display::revision(&cx), revision + 1);
    assert!(display::monitor(&cx).is_some());
    assert_eq!(cx.state::<DisplayState>().button_text(), "색상 · 자동");
    assert!(cx.state::<DisplayState>().tooltip().starts_with(SECOND));

    // And back: sRGB again.
    assert!(cx.state::<DisplayState>().screen_seen(FIRST));
    display::settle(&mut cx);
    assert_eq!(cx.state::<DisplayState>().target().digest, "srgb");
    assert_eq!(display::revision(&cx), revision + 2);
    assert!(display::monitor(&cx).is_none());

    // A manual profile does not follow the monitor (the screen is still recorded, as the oracle's state does).
    display::configure(&mut cx, "manual", SECOND_PROFILE).unwrap();
    display::settle(&mut cx);
    let manual = cx.state::<DisplayState>().target().clone();
    assert!(cx.state::<DisplayState>().screen_seen(SECOND));
    display::settle(&mut cx);
    let moved = cx.state::<DisplayState>().target();
    assert_eq!((moved.screen.as_str(), moved.digest.as_str()), (SECOND, manual.digest.as_str()));
}

/// A screen change while the first answer is still on its way: the late answer is dropped, the new monitor wins.
#[test]
fn a_late_answer_for_the_old_monitor_is_dropped() {
    let monitors = Monitors::two();
    let mut cx = context();
    cx.state::<DisplayState>().set_system(monitors.clone());
    cx.state::<DisplayState>().screen_seen(FIRST);
    display::poll(&mut cx);
    cx.state::<DisplayState>().screen_seen(SECOND);
    display::settle(&mut cx);
    let state = cx.state::<DisplayState>().target();
    assert_eq!((state.screen.as_str(), state.path.as_str()), (SECOND, SECOND_PROFILE));
}

/// macOS as the tests see it: the system hands over each display's profile data (`mac_profile_data`), never a file.
struct MacDisplays {
    second: Vec<u8>,
    asked: Mutex<Vec<String>>,
}

impl SystemProfiles for MacDisplays {
    fn is_mac(&self) -> bool {
        true
    }

    fn windows_profile_path(&self, _screen: &str) -> Result<Option<String>, String> {
        panic!("macOS never asks for a Windows profile path");
    }

    fn mac_profile_data(&self, screen: &str) -> Result<Option<Vec<u8>>, String> {
        self.asked.lock().unwrap().push(screen.to_owned());
        Ok((grainy_platform::monitor::display_number(screen) == Some(7)).then(|| self.second.clone()))
    }
}

/// The macOS screen names of grainy-platform: the window's display gets the profile the system gives it, named
/// after the display (`resolve`'s `screen.rsplit(' (', 1)[0]`).
#[test]
fn a_window_on_a_macos_display_gets_the_profile_data_of_that_display() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let displays = [
        grainy_platform::monitor::DisplayBounds { id: 1, x: 0.0, y: 0.0, width: 1512.0, height: 982.0, builtin: true },
        grainy_platform::monitor::DisplayBounds {
            id: 7,
            x: 1512.0,
            y: -200.0,
            width: 2560.0,
            height: 1440.0,
            builtin: false,
        },
    ];
    let second = std::fs::read(SECOND_PROFILE).unwrap();
    let mac = Arc::new(MacDisplays { second: second.clone(), asked: Mutex::default() });
    let mut cx = context();
    cx.state::<DisplayState>().set_system(mac.clone());
    let screen_of = |window: [f64; 4], monitor: [f64; 2]| {
        let id = grainy_platform::monitor::display_of_window(&displays, window, Some(monitor)).unwrap();
        grainy_platform::monitor::mac_screen_name(displays.iter().find(|d| d.id == id).unwrap())
    };
    let built_in = screen_of([100.0, 100.0, 900.0, 700.0], [1512.0, 982.0]);
    assert!(cx.state::<DisplayState>().screen_seen(&built_in));
    display::settle(&mut cx);
    let first = cx.state::<DisplayState>().target().clone();
    assert_eq!((first.screen.as_str(), first.digest.as_str()), ("Built-in Display (1)", "srgb"));
    assert_eq!(first.message, "macOS 기본 색상(sRGB)을 사용합니다.");
    let external = screen_of([1600.0, 0.0, 3000.0, 900.0], [2560.0, 1440.0]);
    assert!(cx.state::<DisplayState>().screen_seen(&external));
    display::settle(&mut cx);
    let state = cx.state::<DisplayState>().target().clone();
    assert_eq!(
        (state.screen.as_str(), state.description.as_str(), state.path.as_str()),
        ("Display (7)", "Display", "")
    );
    assert_eq!(state.message, "macOS가 현재 모니터에 제공한 프로파일입니다.");
    assert_eq!(display::monitor(&cx).map(|p| p.bytes().to_vec()), Some(second));
    assert_eq!(mac.asked.lock().unwrap().clone(), vec![built_in, external]);
}

/// egui's points times the zoom factor are the system's points the macOS lookup works in.
#[test]
fn the_window_place_is_read_in_the_systems_points() {
    let ctx = egui::Context::default();
    assert_eq!(display::system_place(&ctx), None, "no place before the toolkit tells it");
    ctx.set_zoom_factor(1.25);
    let mut input = egui::RawInput::default();
    let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
    info.outer_rect = Some(egui::Rect::from_min_max(egui::pos2(80.0, 40.0), egui::pos2(880.0, 640.0)));
    info.monitor_size = Some(egui::vec2(1209.6, 785.6));
    for _ in 0..2 {
        // The textures of a pass must be handled: none are wanted here.
        ctx.run_ui(input.clone(), |_| {}).textures_delta.clear();
    }
    let (window, monitor) = display::system_place(&ctx).unwrap();
    assert_eq!(window, [100.0, 50.0, 1100.0, 800.0]);
    let monitor = monitor.unwrap();
    assert!((monitor[0] - 1512.0).abs() < 1e-3 && (monitor[1] - 982.0).abs() < 1e-3, "{monitor:?}");
}

// ---- the window ----------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on the second monitor with the 화면 색상 box open: the monitor's name, its profile and the oracle's
/// "the system gave this monitor" message. The test window has no native handle, so the screen set here stays.
#[test]
fn monitor_dialog_on_the_second_monitor() {
    let _turn = common::language_lock();
    for (language, suffix) in [(Language::Korean, "ko"), (Language::English, "en")] {
        grainy_i18n::set_language(language);
        let dir = common::temp_dir("monitor");
        let path = dir.join("garden.png");
        common::save_png(&common::garden(4, [1.0; 3], false), &path);
        let mut harness = builder().wgpu().build_eframe(move |cc| {
            let mut app = GrainyApp::new(&cc.egui_ctx);
            app.set_debug_overlay(false);
            app.open_paths(vec![path]);
            app.cx.state::<DisplayState>().set_system(Monitors::two());
            app.cx.state::<DisplayState>().screen_seen(SECOND);
            app
        });
        settle_frame(&mut harness);
        assert_eq!(harness.state_mut().cx.state::<DisplayState>().target().path, SECOND_PROFILE);
        assert_eq!(harness.state_mut().cx.state::<DisplayState>().screen(), SECOND);
        harness.state_mut().cx.run("display.dialog");
        harness.run_steps(3);
        settle_frame(&mut harness);
        pin_status(harness.state_mut(), "");
        harness.run_steps(2);
        save(&mut harness, &format!("monitor-dialog-{suffix}"));
    }
    grainy_i18n::set_language(Language::Korean);
}
