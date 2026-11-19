//! Settings › AI 연결 (MCP) (port of luma/mcp_dialog.py `McpDialog`): the switch and its status, how to install
//! Grainy's MCP in each AI program (copied by the person; Grainy never runs it or edits other programs' files),
//! the AI export folder, undoing the last AI task, the recent activity, and under 고급 the key and the port for
//! programs that connect over HTTP. [`McpView`] is what `refresh` shows, without widgets.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText, Vec2};

use super::Dialog;
use crate::ai::connection::{activity_text, install_commands, install_text, standard_folder, Connection, CLIENTS};
use crate::ai::system::{Os, System};
use crate::ai::AiState;
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// `EXPLAIN`: what the AI programs can read and change, and what Grainy sends.
pub const EXPLAIN: &str = "Grainy가 실행 중이면 이 컴퓨터의 AI 프로그램(Claude Code, Codex, Gemini CLI, Claude 데스크톱)이 Grainy를 쓸 수 있습니다. AI에게 시키기 창에서 요청하거나, 그 프로그램에 아래 방법으로 Grainy의 MCP를 설치하고 일을 시켰을 때만, 그 프로그램이 사진 목록과 사진 정보, 보정값, 미리보기 이미지(최대 1568픽셀)를 읽고, 보정값과 별점·선택 표시·색 라벨을 바꾸고, 가상 사본과 스냅샷을 만들고, 그 프로그램이 정한 폴더의 사진을 보관함으로 가져오고, 그 프로그램이 정한 폴더로 사진을 내보내고 밀착 이미지 JPEG(최대 4000픽셀)를 저장합니다. 사진 정보는 파일 이름과 폴더 경로, 파일 형식과 픽셀 크기, 키워드, 별점, 선택·제외 표시, 색 라벨, 카메라와 렌즈, 노출 정보(ISO, 셔터 속도, 조리개, 초점 거리), 촬영 날짜이며 위치 정보는 보내지 않습니다. AI가 읽은 내용은 그 AI 서비스로 전송되고 그 프로그램에도 남을 수 있습니다. Grainy 자체는 사진을 인터넷으로 보내지 않으며, 연결은 이 컴퓨터 안에서만 열립니다. 원본 사진 파일은 바뀌지 않고, AI가 한 보정과 별점·선택 표시·색 라벨 변경은 최근 작업부터 차례로 되돌릴 수 있습니다.";
/// `REGEN`.
pub const REGEN: &str = "새 연결 키를 만들까요? HTTP로 직접 연결한 프로그램은 새 키로 다시 설정해야 합니다. 위 설치 방법으로 설치한 AI 프로그램은 그대로 쓸 수 있습니다.";
const NOTE: &str = "한 번만 설치하면 됩니다. 설치에는 연결 키가 들어가지 않습니다. 설치한 AI 프로그램은 Grainy가 실행 중이고 AI 연결이 켜져 있을 때 Grainy를 쓰며, Grainy보다 먼저 켜 두어도 됩니다. Grainy 업데이트 후에는 AI에서 새 세션을 시작하세요. 기존 세션은 이전 도구 목록을 유지합니다.";
const PORT_NOTE: &str = "포트를 바꿨습니다. HTTP로 직접 연결한 프로그램은 새 주소로 다시 설정해야 합니다. 위 설치 방법으로 설치한 AI 프로그램은 그대로 쓸 수 있습니다.";

/// What the dialog reads of the connection (`McpService` as `McpDialog.refresh` uses it).
pub trait Shown {
    fn enabled(&self) -> bool;
    fn running(&self) -> bool;
    fn error(&self) -> Option<String>;
    fn elsewhere(&self) -> bool;
    fn port_changed(&self) -> bool;
    fn bound_port(&self) -> Option<u16>;
    fn port(&self) -> Option<u16>;
    fn token(&self) -> Option<String>;
    fn export_root(&self) -> PathBuf;
    fn folder(&self) -> PathBuf;
    /// `service.activity`, oldest first.
    fn activity(&self) -> Vec<(String, String, String)>;
}

impl Shown for Connection {
    fn enabled(&self) -> bool {
        Connection::enabled(self)
    }
    fn running(&self) -> bool {
        self.running
    }
    fn error(&self) -> Option<String> {
        self.error.clone()
    }
    fn elsewhere(&self) -> bool {
        self.elsewhere
    }
    fn port_changed(&self) -> bool {
        self.port_changed
    }
    fn bound_port(&self) -> Option<u16> {
        Connection::bound_port(self)
    }
    fn port(&self) -> Option<u16> {
        Connection::port(self)
    }
    fn token(&self) -> Option<String> {
        Connection::token(self)
    }
    fn export_root(&self) -> PathBuf {
        Connection::export_root(self)
    }
    fn folder(&self) -> PathBuf {
        Connection::folder(self).to_path_buf()
    }
    fn activity(&self) -> Vec<(String, String, String)> {
        self.service.activity()
    }
}

/// The dialog's state between refreshes (`key_visible`, `opened_for`, the 고급 toggle).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpState {
    pub key_visible: bool,
    pub opened_for: Option<String>,
    pub advanced: bool,
}

/// What `McpDialog.refresh` shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpView {
    pub enabled: bool,
    pub status: String,
    pub port_button: bool,
    pub port_note: bool,
    pub http: String,
    pub key: String,
    pub key_shown: bool,
    pub show_key: String,
    pub key_enabled: bool,
    pub export_folder: String,
    /// Per [`CLIENTS`] tab: the instruction and the text to copy.
    pub setups: Vec<(String, String)>,
    pub undo_label: String,
    pub undo_enabled: bool,
    /// Newest first.
    pub activity: Vec<String>,
}

/// The activity's time as the dialog shows it: grainy-mcp's service clock gives `%Y-%m-%d %H:%M`, the oracle shows
/// the time of day.
fn shown_time(when: &str) -> &str {
    when.split_once(' ').map_or(when, |(_, time)| time)
}

/// Port of `McpDialog.refresh` (and `setup_text`): `state.advanced` opens once when the connection should be on
/// but did not start.
pub fn refresh(state: &mut McpState, shown: &dyn Shown, sys: &dyn System, undo: (String, bool)) -> McpView {
    let running = shown.running();
    let status = if running {
        tr_with("켜짐 · 포트 {0}", &[&shown.bound_port().map(|p| p.to_string()).unwrap_or_else(|| "None".into())])
    } else if let Some(error) = shown.error() {
        error
    } else {
        tr("꺼짐")
    };
    let problem = shown.error().filter(|_| shown.enabled() && !running && !shown.elsewhere());
    if problem.is_some() && problem != state.opened_for {
        state.advanced = true;
    }
    state.opened_for = problem;
    let port = shown.bound_port().or(shown.port()).unwrap_or(grainy_mcp::service::DEFAULT_PORT);
    let address = format!("http://127.0.0.1:{port}/mcp");
    let http = tr_with(
        "HTTP로 직접 연결하는 프로그램용입니다. 주소 {0}, 요청 헤더 Authorization: Bearer <연결 키>. 새 키를 만들거나 포트를 바꾸면 그 프로그램은 다시 설정해야 합니다. 위 설치 방법으로 설치했다면 필요 없습니다.",
        &[&address],
    );
    let windows = sys.os() == Os::Windows;
    let shell = if windows { "PowerShell".to_owned() } else { tr("터미널") };
    let instructions = [
        tr_with("{0}에서 아래 한 줄을 한 번 실행한 뒤 Claude Code를 다시 시작하세요. 확인은 claude mcp list. 이미 있다고 나오면 먼저 claude mcp remove grainy --scope user를 실행하세요.", &[&shell]),
        tr_with("{0}에서 아래 한 줄을 한 번 실행한 뒤 Codex를 다시 시작하세요. 확인은 codex mcp list.", &[&shell]),
        tr("Claude 데스크톱에서 설정 › 개발자 › 구성 편집을 눌러 claude_desktop_config.json을 열고, \"mcpServers\" 안에 아래 \"grainy\" 항목을 넣은 뒤(파일이 비어 있으면 아래 내용을 그대로 붙여 넣으세요) Claude 데스크톱을 다시 시작하세요."),
    ];
    let standard = standard_folder(sys, windows);
    let commands = install_commands(windows, Some(&shown.folder()), standard.as_deref());
    let setups = CLIENTS
        .iter()
        .zip(instructions)
        .map(|((key, _), instruction)| (instruction, install_text(&commands[*key])))
        .collect();
    let activity = shown
        .activity()
        .iter()
        .rev()
        .map(|(when, name, outcome)| format!("{} · {}", shown_time(when), activity_text(name, outcome)))
        .collect();
    McpView {
        enabled: shown.enabled(),
        status,
        port_button: shown.enabled() && !shown.elsewhere(),
        port_note: shown.port_changed(),
        http,
        key: shown.token().unwrap_or_default(),
        key_shown: state.key_visible,
        show_key: if state.key_visible { tr("숨기기") } else { tr("보기") },
        key_enabled: running,
        export_folder: shown.export_root().to_string_lossy().into_owned(),
        setups,
        undo_label: undo.0,
        undo_enabled: undo.1,
        activity,
    }
}

/// A question or message box over the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Boxed {
    /// 새 키 만들기: REGEN, 예 / 아니요.
    Regenerate,
    /// A warning (`save_failed`).
    Warning(String),
}

/// The window.
#[derive(Default)]
pub struct McpDialog {
    state: McpState,
    tab: usize,
    undo_message: Option<String>,
    boxed: Option<Boxed>,
    chooser: Option<Receiver<Option<PathBuf>>>,
}

fn mono(ui: &mut egui::Ui, text: &str) {
    let mut shown = text.to_owned();
    egui::ScrollArea::vertical().id_salt("setup").max_height(120.0).show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut shown)
                .font(egui::FontId::monospace(12.0))
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .interactive(false),
        );
    });
}

impl McpDialog {
    fn connection(cx: &mut AppContext) -> Option<&mut Connection> {
        cx.state::<AiState>().connection.as_mut()
    }

    fn save_failed(&mut self) {
        self.boxed = Some(Boxed::Warning(tr("AI 연결 설정을 저장하지 못했습니다.")));
    }

    fn body(&mut self, ui: &mut egui::Ui, cx: &mut AppContext) {
        let undo = crate::commands::ai::undo_line(cx);
        let system = cx.state::<AiState>().system.clone();
        let Some(connection) = Self::connection(cx) else {
            widgets::muted(ui, &tr(EXPLAIN));
            return;
        };
        let view = refresh(&mut self.state, connection, system.as_ref(), undo);
        let mut toggled = None;
        let mut action: Option<&'static str> = None;
        widgets::muted(ui, &tr(EXPLAIN));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let mut on = view.enabled;
            if widgets::checkbox(ui, &mut on, &tr("AI 연결 켜기")).changed() {
                toggled = Some(on);
            }
            ui.add_space(8.0);
            widgets::label(ui, &view.status, t::TEXT, t::FONT_SIZE);
        });
        ui.add_space(6.0);
        widgets::label(ui, &tr("AI 프로그램에 설치하기"), t::TEXT, t::FONT_SIZE);
        ui.horizontal(|ui| {
            for (i, (_, title)) in CLIENTS.iter().enumerate() {
                if Button::new(&tr(title)).checked(self.tab == i).show(ui).clicked() {
                    self.tab = i;
                }
            }
        });
        let (instruction, text) = view.setups[self.tab].clone();
        widgets::label(ui, &instruction, t::TEXT, t::FONT_SIZE);
        mono(ui, &text);
        if widgets::button(ui, &tr("복사")).clicked() {
            ui.ctx().copy_text(text);
        }
        ui.add_space(6.0);
        widgets::muted(ui, &tr(NOTE));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            widgets::label(ui, &tr("AI 내보내기 폴더"), t::TEXT, t::FONT_SIZE);
            ui.add_space(8.0);
            let mut folder = view.export_folder.clone();
            ui.add(egui::TextEdit::singleline(&mut folder).desired_width(380.0).interactive(false));
            if widgets::button(ui, &tr("바꾸기…")).clicked() {
                action = Some("folder");
            }
        });
        ui.horizontal(|ui| {
            widgets::label(ui, &view.undo_label, t::TEXT, t::FONT_SIZE);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Button::new(&tr("마지막 AI 작업 되돌리기")).enabled(view.undo_enabled).show(ui).clicked() {
                    action = Some("undo");
                }
            });
        });
        if let Some(message) = &self.undo_message {
            widgets::label(ui, message, t::TEXT, t::FONT_SIZE);
        }
        ui.add_space(6.0);
        widgets::label(ui, &tr("최근 AI 작업"), t::TEXT, t::FONT_SIZE);
        egui::Frame::new().stroke(egui::Stroke::new(1.0, t::MENU_BORDER)).inner_margin(6.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("activity").max_height(130.0).min_scrolled_height(130.0).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    for line in &view.activity {
                        widgets::label(ui, line, t::TEXT, 11.0);
                    }
                },
            );
        });
        ui.add_space(4.0);
        widgets::foldout(ui, &tr("고급"), &mut self.state.advanced);
        if self.state.advanced {
            widgets::muted(ui, &view.http);
            ui.horizontal(|ui| {
                widgets::label(ui, &tr("연결 키"), t::TEXT, t::FONT_SIZE);
                ui.add_space(8.0);
                let mut key = view.key.clone();
                ui.add_enabled(
                    view.key_enabled,
                    egui::TextEdit::singleline(&mut key)
                        .password(!view.key_shown)
                        .desired_width(300.0)
                        .interactive(false),
                );
                if Button::new(&view.show_key).enabled(view.key_enabled).show(ui).clicked() {
                    action = Some("show");
                }
                if Button::new(&tr("복사")).enabled(view.key_enabled).show(ui).clicked() {
                    ui.ctx().copy_text(view.key.clone());
                }
                if Button::new(&tr("새 키 만들기")).enabled(view.key_enabled).show(ui).clicked() {
                    action = Some("regen");
                }
            });
            if Button::new(&tr("다른 포트로 바꾸기")).enabled(view.port_button).show(ui).clicked() {
                action = Some("port");
            }
            if view.port_note {
                widgets::label(ui, &tr(PORT_NOTE), t::TEXT, t::FONT_SIZE);
            }
        }
        if let Some(on) = toggled {
            if let Some(connection) = Self::connection(cx) {
                connection.set_enabled(on);
            }
        }
        match action {
            Some("folder") => {
                let start = view.export_folder.clone();
                let (tx, rx) = channel();
                let title = tr("AI 내보내기 폴더");
                // Started here, on the window's thread; only the answer is awaited elsewhere (`native`).
                let answer = rfd::AsyncFileDialog::new().set_title(title).set_directory(start).pick_folder();
                let send = move |chosen: Option<rfd::FileHandle>| {
                    let _ = tx.send(chosen.map(super::native::path_of));
                };
                if super::native::await_answer(answer, send) {
                    self.chooser = Some(rx);
                }
            }
            Some("undo") => self.undo_message = crate::commands::ai::undo_last(cx),
            Some("show") => self.state.key_visible = !self.state.key_visible,
            Some("regen") => self.boxed = Some(Boxed::Regenerate),
            Some("port") => {
                if let Some(connection) = Self::connection(cx) {
                    connection.change_port();
                }
            }
            _ => {}
        }
    }

    fn receive_folder(&mut self, cx: &mut AppContext) {
        let Some(rx) = &self.chooser else { return };
        let Ok(chosen) = rx.try_recv() else { return };
        self.chooser = None;
        if let Some(folder) = chosen {
            let saved = Self::connection(cx).map(|c| {
                c.save("mcp_export_folder", grainy_platform::json::Value::from(folder.to_string_lossy().as_ref()))
            });
            if matches!(saved, Some(Err(_))) {
                self.save_failed();
            }
        }
    }

    fn boxes(&mut self, ctx: &egui::Context, cx: &mut AppContext) {
        let Some(boxed) = self.boxed.clone() else { return };
        let (title, text, question) = match &boxed {
            Boxed::Regenerate => (tr("새 키 만들기"), tr(REGEN), true),
            Boxed::Warning(text) => (tr("AI 연결 (MCP)"), text.clone(), false),
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("mcp_box")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(&title).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            widgets::label(ui, &text, t::TEXT, t::FONT_SIZE);
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if question {
                    if Button::new(&tr("아니요")).show(ui).clicked() {
                        answer = Some(false);
                    }
                    if Button::new(&tr("예")).kind(Kind::Primary).show(ui).clicked() {
                        answer = Some(true);
                    }
                } else if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            self.boxed = None;
            if yes && boxed == Boxed::Regenerate {
                let failed = Self::connection(cx).is_some_and(|c| c.regenerate_token().is_err());
                if failed {
                    self.save_failed();
                }
            }
        }
    }
}

impl Dialog for McpDialog {
    fn id(&self) -> &'static str {
        "mcp"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        self.receive_folder(cx);
        let mut open = true;
        let mut close = false;
        egui::Window::new(tr("AI 연결 (MCP)"))
            .id(egui::Id::unique("mcp_window"))
            .open(&mut open)
            .frame(super::frame())
            .default_size(Vec2::new(720.0, 760.0))
            .default_pos(ctx.content_rect().center() - Vec2::new(360.0, 380.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.set_width(688.0);
                self.body(ui, cx);
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if widgets::button(ui, &tr("닫기")).clicked() {
                        close = true;
                    }
                });
            });
        self.boxes(ctx, cx);
        // The window refreshes every second while it is open (`QTimer(1000)`).
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        open && !close
    }
}
