//! The Ask AI window (port of the widgets of luma/ai_dialog.py `AiDialog`): non-modal, the editor and the AI
//! connection stay usable while it runs. Its behaviour is [`crate::ai::ask::AskAi`], kept in the context while the
//! window is closed; this file draws it and turns clicks into its calls.

use eframe::egui::{self, RichText, Vec2};

use super::Dialog;
use crate::ai::ask::{AskAi, Question, NOTICE};
use crate::ai::install::{Installer, NODE_URL};
use crate::ai::system::Os;
use crate::ai::AiState;
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// A box over the window.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Boxed {
    Question(Question),
    /// 자세히: EXPLAIN and NOTICE.
    Details,
}

/// What a click asked for (done after drawing, with the model and the connection).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Click {
    Details,
    Acknowledge,
    Choice,
    SignIn(usize),
    Install(usize),
    Recheck(usize),
    Enable,
    Send,
    Stop,
    New,
    Undo,
}

/// The window.
#[derive(Default)]
pub struct AskAiDialog {
    boxed: Option<Boxed>,
}

fn confirm_text(installer: &Installer) -> String {
    tr_with(
        "설치할 프로그램: {0}\n제공자: {1}\n공식 안내: {2}\n\n실행할 명령:\n{3}",
        &[&tr(installer.name), installer.vendor, installer.source, installer.command],
    )
}

impl AskAiDialog {
    fn body(&mut self, ui: &mut egui::Ui, cx: &mut AppContext) -> Vec<Click> {
        let undo = crate::commands::ai::undo_line(cx);
        let mut clicks = Vec::new();
        let state = cx.state::<AiState>();
        let (Some(model), Some(link)) = (state.ask.as_mut(), state.connection.as_ref()) else { return clicks };
        let view = model.view(link, undo);
        if view.notice {
            egui::Frame::new().stroke(egui::Stroke::new(1.0, t::MENU_BORDER)).inner_margin(10.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::label(ui, &tr(NOTICE), t::TEXT, t::FONT_SIZE);
                ui.horizontal(|ui| {
                    if widgets::button(ui, &tr("자세히")).clicked() {
                        clicks.push(Click::Details);
                    }
                    if widgets::button(ui, &tr("확인")).clicked() {
                        clicks.push(Click::Acknowledge);
                    }
                });
            });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            widgets::label(ui, &tr("AI 프로그램"), t::TEXT, t::FONT_SIZE);
            ui.add_space(6.0);
            let names: Vec<String> = model.profiles.iter().map(|p| tr(p.name)).collect();
            let selected = model.program.and_then(|i| names.get(i).cloned()).unwrap_or_default();
            ui.add_enabled_ui(view.program_enabled, |ui| {
                egui::ComboBox::from_id_salt("ai_program").selected_text(selected).width(200.0).show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(model.program == Some(i), name).clicked() && model.program != Some(i) {
                            model.program = Some(i);
                            clicks.push(Click::Choice);
                        }
                    }
                });
            });
            ui.add_space(12.0);
            widgets::label(ui, &tr("모델 (선택)"), t::TEXT, t::FONT_SIZE);
            ui.add_space(6.0);
            let field = ui.add_enabled(
                view.program_enabled,
                egui::TextEdit::singleline(&mut model.model)
                    .hint_text(tr("비워 두면 프로그램 기본 모델"))
                    .desired_width(220.0),
            );
            if field.lost_focus() {
                clicks.push(Click::Choice);
            }
        });
        if view.hints {
            widgets::label(
                ui,
                &tr("프로그램이 없으면 설치…를 누르세요. 실행 전에 공식 설치 명령을 보여 줍니다."),
                t::MUTED,
                t::FONT_SIZE,
            );
        }
        for (i, label) in model.auth_labels.iter().enumerate() {
            ui.horizontal(|ui| {
                widgets::label(ui, label, t::TEXT, t::FONT_SIZE);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Button::new(&tr("다시 확인")).enabled(view.recheck_enabled[i]).show(ui).clicked() {
                        clicks.push(Click::Recheck(i));
                    }
                    if view.install_visible[i]
                        && Button::new(&tr("설치…")).enabled(view.install_enabled[i]).show(ui).clicked()
                    {
                        clicks.push(Click::Install(i));
                    }
                    if view.login_visible[i] {
                        let kind = if view.login_bold[i] { Kind::Primary } else { Kind::Normal };
                        if Button::new(&tr("로그인…")).kind(kind).enabled(view.login_enabled[i]).show(ui).clicked()
                        {
                            clicks.push(Click::SignIn(i));
                        }
                    }
                });
            });
        }
        widgets::label(
            ui,
            &tr("로그인은 프로그램의 공식 창에서 진행합니다. Grainy는 비밀번호나 키를 받지 않습니다. 표시된 상태와 실제 인증 유효성은 다를 수 있습니다. Gemini는 인증 파일·환경변수의 존재만 확인합니다."),
            t::MUTED,
            t::FONT_SIZE,
        );
        if let Some(help) = &model.login_help {
            widgets::label(ui, help, t::TEXT, t::FONT_SIZE);
        }
        ui.horizontal(|ui| {
            widgets::label(ui, &view.connection, t::TEXT, t::FONT_SIZE);
            if view.enable_visible {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, &tr("AI 연결 켜기")).clicked() {
                        clicks.push(Click::Enable);
                    }
                });
            }
        });
        ui.add_space(4.0);
        let height = (ui.available_height() - 210.0).max(120.0);
        egui::Frame::new().fill(t::STATUS_BG).stroke(egui::Stroke::new(1.0, t::MENU_BORDER)).inner_margin(6.0).show(
            ui,
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("transcript")
                    .stick_to_bottom(true)
                    .min_scrolled_height(height)
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::label(ui, &model.transcript, t::TEXT, t::FONT_SIZE);
                    });
            },
        );
        ui.add_space(4.0);
        let input = ui.add(
            egui::TextEdit::multiline(&mut model.input)
                // The key is Command+Enter on macOS (`modifiers.command` below): written ⌘Enter there, as every
                // other shortcut text (`keys`); the oracle left this one as "Ctrl+Enter".
                .hint_text(grainy_i18n::keys_here(&tr("Grainy에 시킬 일을 입력하세요. Ctrl+Enter로 보냅니다.")))
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        if input.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            clicks.push(Click::Send);
        }
        ui.horizontal(|ui| {
            if Button::new(&tr("보내기")).kind(Kind::Primary).enabled(view.send).show(ui).clicked() {
                clicks.push(Click::Send);
            }
            if Button::new(&tr("중지")).enabled(view.stop).show(ui).clicked() {
                clicks.push(Click::Stop);
            }
            if Button::new(&tr("새 대화")).enabled(view.new).show(ui).clicked() {
                clicks.push(Click::New);
            }
            ui.add_space(8.0);
            widgets::label(ui, &model.status, t::TEXT, t::FONT_SIZE);
        });
        ui.horizontal(|ui| {
            widgets::label(ui, &view.undo_label, t::TEXT, t::FONT_SIZE);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Button::new(&tr("마지막 AI 작업 되돌리기")).enabled(view.undo_enabled).show(ui).clicked() {
                    clicks.push(Click::Undo);
                }
            });
        });
        if let Some(message) = &model.undo_message {
            widgets::label(ui, message, t::TEXT, t::FONT_SIZE);
        }
        clicks
    }

    fn act(&mut self, clicks: Vec<Click>, cx: &mut AppContext) {
        for click in clicks {
            if click == Click::Undo {
                let message = crate::commands::ai::undo_last(cx);
                if let Some(model) = cx.state::<AiState>().ask.as_mut() {
                    model.undo_message = message;
                }
                continue;
            }
            if click == Click::Send {
                let undo = crate::commands::ai::undo_line(cx);
                let seen = crate::commands::ai::seen(cx);
                let state = cx.state::<AiState>();
                if let (Some(model), Some(link)) = (state.ask.as_mut(), state.connection.as_mut()) {
                    model.send(link, &seen, undo);
                }
                continue;
            }
            let folder = cx.data_folder.clone().unwrap_or_default();
            let state = cx.state::<AiState>();
            let (Some(model), Some(link)) = (state.ask.as_mut(), state.connection.as_mut()) else { return };
            match click {
                Click::Details => self.boxed = Some(Boxed::Details),
                Click::Acknowledge => model.acknowledge(link),
                Click::Choice => model.choice_changed(link),
                Click::SignIn(i) => model.sign_in(i, &folder),
                Click::Install(i) => self.boxed = model.install_program(i).map(Boxed::Question),
                Click::Recheck(i) => model.detect_program(i),
                Click::Enable => model.enable_connection(link),
                Click::Stop => model.stop(),
                Click::New => model.new_conversation(),
                Click::Send | Click::Undo => {}
            }
        }
    }

    fn boxes(&mut self, ctx: &egui::Context, cx: &mut AppContext) {
        let Some(boxed) = self.boxed.clone() else { return };
        let os = cx.state::<AiState>().system.os();
        let mut done = false;
        let mut next: Option<Boxed> = None;
        let mut install: Option<(usize, Installer)> = None;
        let mut node: Option<(usize, bool, bool)> = None;
        egui::Modal::new(egui::Id::unique("ask_ai_box")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(480.0);
            match &boxed {
                Boxed::Details => {
                    ui.label(RichText::new(tr("AI 연결 (MCP)")).font(t::bold(14.0)).color(t::BRAND));
                    widgets::label(ui, &format!("{}\n\n{}", tr(super::mcp::EXPLAIN), tr(NOTICE)), t::TEXT, t::FONT_SIZE);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                            done = true;
                        }
                    });
                }
                Boxed::Question(Question::Install { index, installer }) => {
                    ui.label(RichText::new(tr("설치 확인")).font(t::bold(14.0)).color(t::BRAND));
                    widgets::label(ui, &confirm_text(installer), t::TEXT, t::FONT_SIZE);
                    ui.add_space(6.0);
                    widgets::muted(ui, &tr("새 콘솔/Terminal 창에서 설치 과정을 볼 수 있습니다. 그 창에서 추가 확인을 요청할 수 있습니다. 결과를 읽고 아무 키나 누르면 설치 실행이 끝납니다."));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if Button::new(&tr("취소")).show(ui).clicked() {
                            done = true;
                        }
                        if Button::new(&tr("설치")).kind(Kind::Primary).show(ui).clicked() {
                            install = Some((*index, *installer));
                            done = true;
                        }
                    });
                }
                Boxed::Question(Question::Node { index }) => {
                    ui.label(RichText::new(tr("Node.js가 필요합니다")).font(t::bold(14.0)).color(t::BRAND));
                    widgets::label(ui, &tr("npm으로 설치하려면 Node.js와 npm이 먼저 필요합니다. 공식 Node.js LTS를 설치한 뒤 다시 확인하고 설치…를 눌러 주세요. AI 프로그램은 자동으로 이어서 설치하지 않습니다."), t::TEXT, t::FONT_SIZE);
                    widgets::muted(ui, NODE_URL);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if Button::new(&tr("취소")).show(ui).clicked() {
                            done = true;
                        }
                        if os == Os::Windows && Button::new(&tr("Node.js 설치… (winget)")).show(ui).clicked() {
                            node = Some((*index, false, true));
                        }
                        if Button::new(&tr("Node.js 공식 페이지 열기")).show(ui).clicked() {
                            node = Some((*index, true, false));
                        }
                    });
                }
            }
        });
        let folder = cx.data_folder.clone().unwrap_or_default();
        let Some(model) = cx.state::<AiState>().ask.as_mut() else { return };
        if let Some((index, website, winget)) = node {
            next = model.node_answer(index, website, winget).map(Boxed::Question);
            done = true;
        }
        if let Some((index, installer)) = install {
            model.install_confirmed(index, &installer, &folder);
        }
        if done {
            self.boxed = next;
        }
    }
}

impl Dialog for AskAiDialog {
    fn id(&self) -> &'static str {
        "ask_ai"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        if let Some(model) = cx.state::<AiState>().ask.as_mut() {
            model.tick();
        }
        let mut open = true;
        let mut clicks = Vec::new();
        egui::Window::new(tr("AI에게 시키기"))
            .id(egui::Id::unique("ask_ai_window"))
            .open(&mut open)
            .frame(super::frame())
            .default_size(Vec2::new(760.0, 660.0))
            .default_pos(ctx.content_rect().center() - Vec2::new(380.0, 330.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.set_width(728.0);
                ui.set_min_height(600.0);
                clicks = self.body(ui, cx);
            });
        self.act(clicks, cx);
        self.boxes(ctx, cx);
        // `QTimer(500)`: the run and the checks are looked at while the window is open.
        if cx.state_ref::<AiState>().and_then(|s| s.ask.as_ref()).is_some_and(AskAi::busy) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if !open {
            // `closeEvent`: the run stops.
            if let Some(model) = cx.state::<AiState>().ask.as_mut() {
                model.shutdown();
            }
        }
        open
    }
}
