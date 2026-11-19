//! The display colour box (luma/display_color.py `DisplayDialog`): the mode (auto, a chosen ICC profile, off), the
//! profile path with 파일 선택, the status of the monitor, the note, 다시 확인, 적용 and 닫기. It stays open beside
//! the window (`show`, not `exec`).

use eframe::egui;

use super::Dialog;
use crate::context::AppContext;
use crate::dialogs::preset_review::FileWait;
use crate::display::{self, DisplayState};
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// `heading.setStyleSheet('... color:#e7c9a2')`.
const HEADING: egui::Color32 = egui::Color32::from_rgb(0xe7, 0xc9, 0xa2);

/// The box's state: the fields as typed, and the text of the status label.
pub struct DisplayDialog {
    mode: String,
    path: String,
    /// The text set by 적용 / a refusal, until the monitor's state changes (`update_status`).
    notice: Option<String>,
    seen_changes: u64,
    seen_configurations: u64,
}

impl DisplayDialog {
    /// `DisplayDialog(manager)`: the fields show the manager's choice.
    pub fn new(cx: &mut AppContext) -> Self {
        let state = cx.state::<DisplayState>();
        Self {
            mode: state.mode.clone(),
            path: state.path.clone(),
            notice: None,
            seen_changes: state.changes(),
            seen_configurations: state.configurations(),
        }
    }

    /// The mode of the box (`mode.currentData()`).
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// The path field.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// `mode.setCurrentIndex`.
    pub fn set_mode(&mut self, mode: &str) {
        self.mode = mode.to_owned();
    }

    /// Types into the path field.
    pub fn set_path(&mut self, path: &str) {
        self.path = path.to_owned();
    }

    /// `fields`: the path and 파일 선택 work in the manual mode only.
    pub fn fields_enabled(&self) -> bool {
        self.mode == "manual"
    }

    /// The items of the mode box, in order: (text, key) (`mode.addItem`).
    pub fn mode_items(system: &str) -> [(String, &'static str); 3] {
        let auto = if system == "macOS" {
            "자동 · macOS 모니터 설정 사용"
        } else {
            "자동 · Windows 모니터 설정 사용"
        };
        [(tr(auto), "auto"), (tr("직접 선택 · ICC 프로파일"), "manual"), (tr("앱의 화면 색상 보정 해제"), "off")]
    }

    /// `apply`: a manual choice needs a file; otherwise the choice is configured and the monitor asked again.
    pub fn apply(&mut self, cx: &mut AppContext) {
        let path = self.path.trim().to_owned();
        if self.mode == "manual" && path.is_empty() {
            self.notice = Some(tr("먼저 화면용 RGB ICC 파일을 선택하세요."));
            return;
        }
        if display::configure(cx, &self.mode.clone(), &path).is_ok() {
            // `configurationChanged` runs `sync`: the fields show the choice (the path stripped).
            self.path = path;
            self.seen_configurations = cx.state::<DisplayState>().configurations();
            self.notice = Some(tr("화면 프로파일을 확인하고 있습니다…"));
        }
    }

    /// `refresh` (다시 확인): every monitor is asked again.
    pub fn refresh(&mut self, cx: &mut AppContext) {
        cx.state::<DisplayState>().ask_again();
    }

    /// `sync` and `update_status`: follows a choice made elsewhere (a menu item) and the monitor's new state.
    pub fn follow(&mut self, cx: &mut AppContext) {
        let state = cx.state::<DisplayState>();
        if state.configurations() != self.seen_configurations {
            self.seen_configurations = state.configurations();
            self.mode = state.mode.clone();
            self.path = state.path.clone();
        }
        if state.changes() != self.seen_changes {
            self.seen_changes = state.changes();
            self.notice = None;
        }
        if let Some(path) = state.chosen.take() {
            self.path = path.to_string_lossy().into_owned();
        }
    }

    /// The text of the status label.
    pub fn status(&self, cx: &mut AppContext) -> String {
        self.notice.clone().unwrap_or_else(|| cx.state::<DisplayState>().status_text())
    }
}

/// 파일 선택 (`browse`): the answer is handed to the open box.
fn remember(cx: &mut AppContext, path: std::path::PathBuf) {
    cx.state::<DisplayState>().chosen = Some(path);
}

impl Dialog for DisplayDialog {
    fn id(&self) -> &'static str {
        "display"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        self.follow(cx);
        let system = cx.state::<DisplayState>().system_name();
        let status = self.status(cx);
        let mut open = true;
        let mut action = None;
        let mut browse = false;
        egui::Window::new(tr("화면 색상 관리 — Grainy"))
            .open(&mut open)
            .frame(super::frame().inner_margin(egui::Margin::same(24)))
            .fixed_size(egui::vec2(580.0, 420.0))
            .default_pos(ctx.content_rect().center() - egui::vec2(290.0, 210.0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                ui.label(egui::RichText::new(tr("화면 색상 관리")).font(t::bold(22.0)).color(HEADING));
                let items = Self::mode_items(system);
                let names: Vec<String> = items.iter().map(|(text, _)| text.clone()).collect();
                let index = items.iter().position(|(_, key)| *key == self.mode).unwrap_or(0);
                if let Some(chosen) =
                    crate::panels::controls::choice(ui, "display_mode", "", &names, index, ui.available_width())
                {
                    self.mode = items[chosen].1.to_owned();
                }
                ui.horizontal(|ui| {
                    let enabled = self.fields_enabled();
                    ui.add_enabled_ui(enabled, |ui| {
                        let width = ui.available_width() - 100.0;
                        ui.allocate_ui(egui::vec2(width, 30.0), |ui| {
                            widgets::line_edit(ui, "display_path", &mut self.path, &tr("ICC 프로파일 경로"));
                        });
                        if widgets::button(ui, &tr("파일 선택")).clicked() {
                            browse = true;
                        }
                    });
                });
                let note = tr("편집 화면·사진 목록·비교창의 표시 색에 적용합니다. 보정값과 내보내는 파일은 바뀌지 않습니다.\n자동 모드에서는 각 창이 있는 모니터를 따릅니다. Windows가 별도 프로파일을 제공하지 않으면 sRGB를 사용합니다.")
                    .replace("Windows", system);
                // The status label takes what the note and the buttons leave (stretch 1).
                let note_height = 3.0 * 17.0;
                let room = (ui.available_height() - note_height - 14.0 - 30.0 - 28.0).max(20.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), room), |ui| {
                    let text = egui::RichText::new(status).font(t::font(t::FONT_SIZE)).color(t::TEXT);
                    ui.add(egui::Label::new(text).selectable(true));
                });
                ui.add_space((ui.available_height() - note_height - 14.0 - 30.0 - 14.0).max(0.0));
                widgets::muted(ui, &note);
                ui.horizontal(|ui| {
                    if widgets::button(ui, &tr("다시 확인")).clicked() {
                        action = Some("refresh");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::button(ui, &tr("닫기")).clicked() {
                            action = Some("close");
                        }
                        ui.add_space(6.0);
                        if Button::new(&tr("적용")).kind(Kind::Primary).show(ui).clicked() {
                            action = Some("apply");
                        }
                    });
                });
            });
        if browse {
            let filter = ("ICC".to_owned(), vec!["icc", "icm"]);
            cx.open_dialog(Box::new(FileWait::open(tr("화면 ICC 프로파일 선택"), filter, None, remember)));
        }
        match action {
            Some("apply") => self.apply(cx),
            Some("refresh") => self.refresh(cx),
            Some("close") => open = false,
            _ => {}
        }
        open
    }
}
