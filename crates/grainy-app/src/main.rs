//! `cargo run -p grainy-app [-- [--library <folder>] [--memory] [--debug] <file|folder>...]`: opens the Grainy
//! window on the Rust app's own library (`GRAINY_RS_HOME`, else `%LOCALAPPDATA%\GrainyRS\Library`) or on
//! `--library`; `--memory` opens photos in memory only. The photos and folders given are imported (added in
//! place). The Python Grainy's library is only opened read-only (파일 관리 → Grainy 라이브러리 읽기 전용으로 열기).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui;
use grainy_app::paths::Arguments;

fn main() -> eframe::Result {
    grainy_app::logger::init();
    // The new version, started from its staged folder by an update: --finish-update APP --wait-pid PID -- ARGS.
    let raw: Vec<_> = std::env::args_os().skip(1).collect();
    if raw.first().is_some_and(|arg| arg == "--finish-update") {
        let args: Result<Vec<_>, _> = raw.into_iter().map(|arg| arg.into_string()).collect();
        std::process::exit(args.ok().and_then(|args| grainy_app::network::run_finish(&args)).unwrap_or(2));
    }
    let args = match Arguments::parse(raw) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\nusage: grainy-app [--library <folder>] [--memory] [--lensfun <folder>] [--offline] [--smoke <folder>] [--debug] [<photo or folder>...]");
            std::process::exit(2);
        }
    };
    // The HDR editing mode the oracle hides (`engine.HDR_FEATURE = False`) is on in the Rust application.
    grainy_app::hdr::enable();
    if let Some(folder) = args.lensfun.clone() {
        grainy_app::commands::lens::set_folder_override(folder);
    }
    // A second window on the same library is refused (main.py `QLockFile`).
    if let Err(message) = grainy_app::session_state::lock_library(&args) {
        grainy_app::session_state::show_refusal(&message);
        return Ok(());
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title("Grainy")
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size([1100.0, 700.0])
        .with_drag_and_drop(true);
    // The size, position and mode the window had when it closed (the Rust app's own settings file).
    let viewport = if args.memory {
        viewport
    } else {
        grainy_app::window_state::viewport(viewport, &grainy_app::paths::Locations::current().data_folder())
    };
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, ..Default::default() };
    eframe::run_native(
        "Grainy",
        options,
        Box::new(move |cc| {
            #[cfg(windows)]
            if let Some(window) = cc.winit_window() {
                use winit::dpi::LogicalSize;
                use winit::platform::windows::{IconExtWindows, WindowExtWindows};
                use winit::window::Icon;
                let icon_at = |size: u32| {
                    Icon::from_resource(1, Some(LogicalSize::new(size, size).to_physical(window.scale_factor())))
                };
                if let Ok(icon) = icon_at(16) {
                    window.set_window_icon(Some(icon));
                }
                if let Ok(icon) = icon_at(32) {
                    window.set_taskbar_icon(Some(icon));
                }
            }
            Ok(Box::new(grainy_app::GrainyApp::from_arguments(&cc.egui_ctx, args)))
        }),
    )
}
