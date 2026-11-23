//! `cargo run -p grainy-app [-- [--library <folder>] [--memory] [--debug] <file|folder>...]`: opens the Grainy
//! window on the Rust app's own library (`GRAINY_RS_HOME`, else `%LOCALAPPDATA%\GrainyRS\Library`) or on
//! `--library`; `--memory` opens photos in memory only. The photos and folders given are imported (added in
//! place). The Python Grainy's library is only opened read-only (파일 관리 → Grainy 라이브러리 읽기 전용으로 열기).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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
    let viewport = grainy_app::window_state::default_viewport();
    // The size, position and mode the window had when it closed (the Rust app's own settings file).
    let viewport = if args.memory {
        viewport
    } else {
        grainy_app::window_state::viewport(viewport, &grainy_app::paths::Locations::current().data_folder())
    };
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.power_preference = eframe::wgpu::PowerPreference::HighPerformance;
    setup.device_descriptor = std::sync::Arc::new(|adapter| {
        let limits = adapter.limits();
        eframe::wgpu::DeviceDescriptor {
            label: Some("Grainy shared wgpu device"),
            required_limits: limits,
            ..Default::default()
        }
    });
    let wgpu_options = eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup),
        // A window whose picture could not be set up (a size the GPU cannot draw) sets it up again.
        on_surface_status: grainy_app::window_state::on_surface_status(),
        ..Default::default()
    };
    let options =
        eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, wgpu_options, ..Default::default() };
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
                        .or_else(|_| Icon::from_resource(1, None))
                };
                if let Ok(icon) = icon_at(16) {
                    window.set_window_icon(Some(icon));
                }
                if let Ok(icon) = icon_at(32) {
                    window.set_taskbar_icon(Some(icon));
                }
            }
            Ok(Box::new(grainy_app::GrainyApp::from_arguments_with_wgpu(
                &cc.egui_ctx,
                cc.wgpu_render_state.clone(),
                args,
            )))
        }),
    )
}
