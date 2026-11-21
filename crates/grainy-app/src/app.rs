//! The main window. Port of the layout of `MainWindow.build_ui` (luma/app.py): menu bar, header, the library
//! column on the left, the photo view with its bar and the filmstrip in the centre, the Develop column on the
//! right, and the status bar. The window owns the registries (panels, canvas tools) and the [`AppContext`];
//! everything else lives in the context (`GrainyApp` dereferences to it).

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

use eframe::egui::{self, CornerRadius, Frame as EguiFrame, Margin, Panel, Stroke, Ui};

use crate::context::{memory_document, AppContext, Mode};
use crate::document::catalog::CatalogDocument;
use crate::document::Document;
use crate::histogram::Bins;
use crate::panels;
use crate::paths::{Arguments, Locations};
use crate::render::{Renderer, TestCard};
use crate::text::tr;
use crate::theme as t;
use crate::tools::{self, CanvasTool};
use crate::widgets::{Button, Kind};
use crate::worker::Worker;

/// The application window.
pub struct GrainyApp {
    pub cx: AppContext,
    panels: Vec<Box<dyn panels::Panel>>,
    tools: Vec<Box<dyn CanvasTool>>,
}

impl Deref for GrainyApp {
    type Target = AppContext;
    fn deref(&self) -> &AppContext {
        &self.cx
    }
}

impl DerefMut for GrainyApp {
    fn deref_mut(&mut self) -> &mut AppContext {
        &mut self.cx
    }
}

impl GrainyApp {
    /// A window developing real photos on the background [`Worker`], with photos opened in memory (nothing is
    /// written: the shell's tests and `--memory`).
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_document(ctx, Box::new(Worker::new()), memory_document())
    }

    /// A window showing the fixed test card (no engine): the deterministic snapshots of the shell.
    pub fn with_test_card(ctx: &egui::Context) -> Self {
        Self::with_renderer(ctx, Box::new(TestCard::default()))
    }

    /// A window drawing with `renderer` on an in-memory session.
    pub fn with_renderer(ctx: &egui::Context, renderer: Box<dyn Renderer>) -> Self {
        Self::with_document(ctx, renderer, memory_document())
    }

    /// A window on `doc` drawing with `renderer`. Installs the font and the style in `ctx`.
    pub fn with_document(ctx: &egui::Context, renderer: Box<dyn Renderer>, doc: Box<dyn Document>) -> Self {
        Self { cx: AppContext::new(ctx, renderer, doc), panels: panels::registry(), tools: tools::registry() }
    }

    /// A window on the library folder `folder` (created when missing) with the real worker. The Python Grainy's
    /// folders are refused for writing and open read-only.
    pub fn with_library(ctx: &egui::Context, folder: &std::path::Path, data: Option<PathBuf>) -> Self {
        let locations = Locations::current();
        let read_only = crate::paths::is_oracle_folder(folder, &locations);
        let mut app = Self::new(ctx);
        if data.as_ref().is_some_and(|path| crate::paths::is_oracle_folder(path, &locations)) {
            app.cx.status = "The Python Grainy's data folder cannot be used for Rust settings.".into();
            return app;
        }
        app.cx.set_data_folder(data.clone());
        let opened = if read_only {
            let scratch = data.unwrap_or_else(|| locations.data_folder()).join("read-only snapshot");
            CatalogDocument::open_read_only(folder, &scratch)
        } else {
            CatalogDocument::open(folder)
        };
        match opened {
            Ok(doc) => {
                app.cx.set_document(Box::new(doc));
                app.cx.set_mode(Mode::Library);
                if read_only {
                    app.cx.status = tr("읽기 전용 · 보정은 이 창에서만 유지되고 저장되지 않습니다.");
                }
            }
            Err(message) => app.cx.status = crate::text::tr_with("처리하지 못했습니다: {0}", &[&message]),
        }
        app
    }

    /// The window `cargo run` opens: the library of `--library` or of the data folder (`GRAINY_RS_HOME`, else
    /// `%LOCALAPPDATA%\GrainyRS`), or an in-memory session with `--memory`; the paths given are imported.
    pub fn from_arguments(ctx: &egui::Context, args: Arguments) -> Self {
        let locations = Locations::current();
        if !args.memory && !crate::paths::is_oracle_folder(&locations.data_folder(), &locations) {
            crate::commands::settings::apply_stored_language(&locations.data_folder());
        }
        let mut app = if args.memory {
            Self::new(ctx)
        } else {
            let folder = args.library.clone().unwrap_or_else(|| locations.library_folder());
            Self::with_library(ctx, &folder, Some(locations.data_folder()))
        };
        app.cx.debug_overlay = args.debug;
        crate::gpu_worker::install(&mut app.cx);
        crate::network::install(&mut app.cx, args.offline);
        crate::startup::begin(&mut app.cx);
        crate::commands::settings::startup(&mut app.cx);
        crate::ai::install(&mut app.cx, args.ask_ai);
        if let Some(folder) = args.smoke {
            crate::smoke::install(&mut app.cx, folder);
        }
        app.cx.open_paths(args.paths);
        app
    }

    /// The registered panels (tests).
    pub fn panel_keys(&self) -> Vec<&'static str> {
        self.panels.iter().map(|p| p.key()).collect()
    }

    /// Draws the whole window into `ui` (the root of the viewport).
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let cx = &mut self.cx;
        cx.poll_background();
        crate::ai::frame(cx);
        crate::startup::frame(cx);
        cx.handle_shortcuts();
        cx.flush();
        cx.refresh_picture();
        // Lights out (L): the columns, the bars, the menus and the status bar are not drawn.
        let lights = crate::commands::settings::begin_frame(cx);

        if lights.is_none() {
            Panel::top("menu_bar")
                .frame(EguiFrame::new().fill(t::BG).inner_margin(Margin { left: 6, right: 6, top: 0, bottom: 0 }))
                .show_separator_line(false)
                .resizable(false)
                .show(ui, |ui| cx.menu_bar(ui));
        }
        if cx.bars {
            let vertical = if cx.mode() == Mode::Develop { 5 } else { 13 };
            Panel::top("header")
                .frame(EguiFrame::new().fill(t::BG).inner_margin(Margin {
                    left: 24,
                    right: 20,
                    top: vertical,
                    bottom: vertical,
                }))
                .show_separator_line(false)
                .resizable(false)
                .show(ui, |ui| header(ui, cx));
        }
        if lights.is_none() {
            Panel::bottom("status_bar")
                .frame(EguiFrame::new().fill(t::STATUS_BG).inner_margin(Margin {
                    left: 6,
                    right: 24,
                    top: 3,
                    bottom: 3,
                }))
                .show_separator_line(false)
                .resizable(false)
                .show(ui, |ui| status_bar(ui, cx));
        }
        if cx.sides {
            let (library_width, develop_width) =
                (crate::window_state::library_width(cx), crate::window_state::develop_width(cx));
            let library = Panel::left("library_column")
                .frame(EguiFrame::new().fill(t::BG).inner_margin(Margin { left: 12, right: 12, top: 8, bottom: 12 }))
                .show_separator_line(false)
                .resizable(true)
                .default_size(library_width)
                .size_range(230.0..=360.0)
                .show(ui, |ui| cx.library_column(ui));
            let panels = &mut self.panels;
            let develop = Panel::right("develop_column")
                .frame(EguiFrame::new().fill(t::BG).inner_margin(Margin { left: 8, right: 8, top: 4, bottom: 6 }))
                .show_separator_line(false)
                .resizable(true)
                .default_size(develop_width)
                .size_range(290.0..=360.0)
                .show(ui, |ui| panels::column::develop_column(ui, cx, panels));
            crate::window_state::note_widths(
                cx,
                Some(library.response.rect.width()),
                Some(develop.response.rect.width()),
            );
        }
        let tools = &mut self.tools;
        egui::CentralPanel::no_frame()
            .frame(EguiFrame::new().fill(t::BG).stroke(Stroke::NONE).corner_radius(CornerRadius::ZERO))
            .show(ui, |ui| cx.center(ui, tools));
        crate::dialogs::show_all(&ctx, cx);
        cx.flush();
        cx.request_preview();
        crate::commands::settings::end_frame(cx, lights);
    }
}

fn header(ui: &mut Ui, cx: &mut AppContext) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let brand = crate::widgets::spaced(ui, "Grainy", t::bold(28.0), t::BRAND, 3.0);
        let (rect, _) = ui.allocate_exact_size(brand.size(), egui::Sense::hover());
        ui.painter().galley(rect.min, brand, t::BRAND);
        ui.add_space(15.0);
        let subtitle = format!("PHOTO STUDIO  /  {}", env!("CARGO_PKG_VERSION"));
        crate::widgets::label(ui, &subtitle, t::MUTED, t::FONT_SIZE);
        if cx.read_only_library() {
            ui.add_space(12.0);
            Button::new(&tr("읽기 전용"))
                .kind(Kind::Feedback)
                .show(ui)
                .on_hover_text(tr("Python Grainy의 라이브러리를 읽기 전용으로 열었습니다. 보정은 저장되지 않습니다."));
        }
        ui.add_space(18.0);
        let feedback = Button::new(&tr("피드백")).kind(Kind::Feedback).show(ui);
        if feedback.clone().on_hover_text(tr("문제점이나 개선 아이디어 보내기")).clicked() {
            cx.run("feedback.open");
        }
        ui.add_space(8.0);
        if crate::widgets::button(ui, &tr("AI에게 시키기")).clicked() {
            cx.run("ai.ask");
        }
        crate::dialogs::update::header_button(ui, cx);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let export = Button::new(&tr("내보내기"))
                .kind(Kind::Primary)
                .enabled(crate::commands::enabled(cx, "file.export"))
                .show(ui);
            if export.clicked() {
                cx.run("file.export");
            }
            if crate::widgets::button(ui, &tr("사진 가져오기")).clicked() {
                cx.run("file.import");
            }
            ui.add_space(20.0);
            if Button::new(&tr("현상")).checked(cx.mode == Mode::Develop).show(ui).clicked() {
                cx.run("view.develop");
            }
            if Button::new(&tr("라이브러리")).checked(cx.mode == Mode::Library).show(ui).clicked() {
                cx.run("view.library");
            }
        });
    });
}

fn status_bar(ui: &mut Ui, cx: &mut AppContext) {
    ui.horizontal(|ui| {
        ui.set_min_height(31.0);
        let text = cx.jobs.status().unwrap_or_else(|| cx.status.clone());
        crate::widgets::label(ui, &text, t::STATUS_TEXT, 11.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::display::status_button(ui, cx);
            cx.import_cancel_button(ui);
            crate::commands::files::preview_cancel_button(cx, ui);
            crate::ai::stop_button(ui, cx);
        });
    });
}

impl eframe::App for GrainyApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        crate::display::watch_window(&mut self.cx, frame);
        self.show(ui);
        if ui.input(|i| i.viewport().close_requested()) {
            if let Err(reason) = self.cx.commit_document() {
                self.cx.status = crate::text::tr_with("처리하지 못했습니다: {0}", &[&reason]);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        crate::smoke::frame(&mut self.cx, ui.ctx());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        t::WINDOW.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        crate::ai::shutdown(&mut self.cx);
        self.cx.doc.edit_mut().commit("보정 변경");
        self.cx.flush();
        crate::folder_watch::shutdown(&mut self.cx);
        crate::window_state::flush(&mut self.cx);
        crate::startup::shutdown(&mut self.cx);
    }
}

/// 128 bins per channel of a preview's bytes (`histogram_bins` of luma/widgets.py).
pub fn histogram_bins(preview: &crate::render::Preview) -> Bins {
    crate::histogram::bins(&preview.rgba, preview.width, preview.height)
}
