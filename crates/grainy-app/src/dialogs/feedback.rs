//! 피드백 (work package 31): the window that sends what a person types, with the version and a one-line system
//! description, to the feedback form (luma/feedback.py `FeedbackDialog`, `show`). The text of the message, the
//! reference, the form's fields and the records of sent references are `grainy_platform::feedback`.
//!
//! The post goes through a [`Transport`]: the program's start installs `crate::network::HttpTransport` (unless the
//! network is switched off with `--offline` / `GRAINY_OFFLINE=1`); without one it is [`Unavailable`], which fails
//! with "not available in this build". Tests put a fake in [`FeedbackState::transport`] and read what it would
//! have sent. The version field carries `grainy_platform::BUILD_VERSION` (`rs-0.2.1`), so the Rust build's
//! messages are told apart; the local record of a sent reference keeps the oracle's version.

use std::sync::{Arc, Mutex};

use eframe::egui;
use grainy_platform::feedback::{self, Gpu, Outbox, Request, SendFailure, SystemFacts};
use grainy_platform::json::Value as Pref;
use grainy_platform::user_paths::System;
use grainy_platform::{APP_VERSION, BUILD_VERSION};

use super::Dialog;
use crate::commands::settings::preferences;
use crate::context::AppContext;
use crate::startup::{tr, unavailable};
use crate::text::tr_with;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// How a form post reaches the server (`crate::network::HttpTransport` in the program).
pub trait Transport: Send + Sync {
    /// Posts `request` within `timeout` seconds; returns the HTTP status.
    fn post(&self, request: &Request, timeout: u64) -> Result<u16, SendFailure>;
}

/// The transport of this build: nothing can be sent.
pub struct Unavailable;

impl Transport for Unavailable {
    fn post(&self, _request: &Request, _timeout: u64) -> Result<u16, SendFailure> {
        Err(SendFailure::Other(unavailable()))
    }
}

/// What the feedback window uses from outside (`cx.state::<FeedbackState>()`); tests replace the parts.
#[derive(Default)]
pub struct FeedbackState {
    /// The way to post; [`Unavailable`] when none is set.
    pub transport: Option<Arc<dyn Transport>>,
    /// The facts of `system_info` (tests); the machine's own when none.
    pub facts: Option<SystemFacts>,
    /// Today as `YYYY-MM-DD` for the record of a sent reference (tests); the local date when none.
    pub today: Option<String>,
}

/// The facts `system_info` reads from this machine: the system, its version, the architecture and the graphics
/// adapter the renderer reported (the adapter is left out while the GPU path is off or has none).
pub fn system_facts(cx: &AppContext) -> SystemFacts {
    if let Some(facts) = cx.state_ref::<FeedbackState>().and_then(|s| s.facts.clone()) {
        return facts;
    }
    let system = System::current();
    let os_version = sysinfo::System::os_version().unwrap_or_default();
    // `platform.release()` is `11`, `platform.version()` is `10.0.<build>`.
    let release = os_version.split_whitespace().next().unwrap_or("").to_owned();
    let build = sysinfo::System::kernel_version().unwrap_or_default();
    let machine = match (std::env::consts::ARCH, system) {
        ("x86_64", System::Windows) => "AMD64",
        ("aarch64", System::Windows) => "ARM64",
        ("aarch64", _) => "arm64",
        (other, _) => other,
    }
    .to_owned();
    let gpu = cx.renderer.gpu_status().and_then(|status| {
        let named = !(status.text.is_empty() || status.text == "off" || status.text.starts_with("unavailable"));
        named.then(|| Gpu { adapter: status.text.clone(), enabled: status.enabled })
    });
    SystemFacts {
        system,
        mac_version: os_version,
        windows_release: release,
        windows_version: format!("10.0.{build}"),
        machine,
        gpu,
    }
}

#[derive(Default)]
struct Shared {
    sending: bool,
    /// The job's answer: `Ok(reference)` or `Err(reason)`.
    answer: Option<Result<String, String>>,
}

/// The window's model (`FeedbackDialog` without the widgets).
pub struct FeedbackModel {
    /// The text in the box.
    pub text: String,
    /// The line under the box (`self.status`).
    pub status: String,
    outbox: Outbox,
    system: String,
    shared: Arc<Mutex<Shared>>,
}

impl FeedbackModel {
    /// A new window's model; the system line is read now (`self.system = system_info()`).
    pub fn new(cx: &AppContext) -> Self {
        Self {
            text: String::new(),
            status: String::new(),
            outbox: Outbox::default(),
            system: feedback::system_info(&system_facts(cx)),
            shared: Arc::new(Mutex::new(Shared::default())),
        }
    }

    /// The system line sent with a message.
    pub fn system(&self) -> &str {
        &self.system
    }

    /// The note under the box: what goes along with the message.
    pub fn note(&self) -> String {
        tr_with("함께 보내는 정보: Grainy {0} · {1} · 보낼 때 정하는 무작위 접수 번호", &[BUILD_VERSION, &self.system])
    }

    /// Whether a send is in flight (the 보내기 button is off).
    pub fn sending(&self) -> bool {
        self.shared.lock().is_ok_and(|s| s.sending)
    }

    /// Port of `submit` (보내기): the post runs on a job. An empty text asks for text; a send in flight is left.
    pub fn submit(&mut self, cx: &mut AppContext) {
        let Some((text, tagged, reference)) = self.outbox.prepare(&self.text, &self.system) else {
            self.status = tr_with("내용을 입력해 주세요.", &[]);
            return;
        };
        if self.sending() {
            return;
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.sending = true;
            shared.answer = None;
        }
        self.status = tr_with("보내는 중…", &[]);
        let state = cx.state::<FeedbackState>();
        let transport: Arc<dyn Transport> = state.transport.clone().unwrap_or_else(|| Arc::new(Unavailable));
        let today = state.today.clone().unwrap_or_else(feedback::today);
        let shared = self.shared.clone();
        cx.jobs.spawn(
            "feedback",
            "",
            move |_| {
                let error =
                    feedback::send(&text, &tagged, BUILD_VERSION, &|request, timeout| transport.post(request, timeout));
                Ok(error)
            },
            move |cx: &mut AppContext, result: Result<String, String>| {
                let error = result.unwrap_or_else(|e| e);
                let answer = if error.is_empty() {
                    // The reference is kept even when the window was closed meanwhile (`SENDING`).
                    if let Some(mut prefs) = preferences(cx) {
                        if let Err(e) = feedback::record(&mut prefs, &reference, APP_VERSION, &today) {
                            log::warn!("feedback reference not recorded: {e}");
                        }
                    }
                    Ok(reference)
                } else {
                    Err(error)
                };
                if let Ok(mut shared) = shared.lock() {
                    shared.sending = false;
                    shared.answer = Some(answer);
                }
            },
        );
    }

    /// Port of `check`: takes the answer of the send, if it came.
    pub fn poll(&mut self) {
        let answer = self.shared.lock().ok().and_then(|mut s| s.answer.take());
        match answer {
            Some(Err(error)) => {
                self.status = tr_with("보내지 못했습니다 ({0}). 인터넷 연결을 확인하고 다시 시도해 주세요.", &[&error]);
            }
            Some(Ok(reference)) => {
                self.outbox.sent();
                self.text.clear();
                self.status = tr_with(
                    "보냈습니다. 고맙습니다. 접수 번호는 #{0}입니다. 반영되면 업데이트한 뒤 Grainy가 알려 드립니다.",
                    &[&reference],
                );
            }
            None => {}
        }
    }
}

/// The window (`FeedbackDialog`).
pub struct FeedbackDialog {
    /// The model.
    pub model: FeedbackModel,
}

impl FeedbackDialog {
    /// A window for the context.
    pub fn new(cx: &AppContext) -> Self {
        Self { model: FeedbackModel::new(cx) }
    }
}

impl Dialog for FeedbackDialog {
    fn id(&self) -> &'static str {
        "feedback"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        self.model.poll();
        let mut send = false;
        let mut close = false;
        let response = egui::Modal::new(egui::Id::unique("feedback")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(488.0);
            ui.label(egui::RichText::new(tr("피드백 보내기")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            widgets::label(
                ui,
                &tr("문제점이나 개선 아이디어를 자유롭게 적어 주세요. 사진, 파일 이름과 경로는 보내지 않습니다."),
                t::TEXT,
                t::FONT_SIZE,
            );
            ui.add_space(6.0);
            let frame = egui::Frame::new()
                .fill(t::INPUT_BG)
                .stroke(egui::Stroke::new(1.0, t::INPUT_BORDER))
                .inner_margin(egui::Margin::symmetric(7, 7));
            let hint = tr("어떤 상황에서 무슨 일이 있었는지, 무엇이 있으면 좋을지 적어 주세요.");
            frame.show(ui, |ui| {
                egui::TextEdit::multiline(&mut self.model.text)
                    .id_salt("feedback_text")
                    .hint_text(egui::RichText::new(hint).color(t::MUTED))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(10)
                    .text_color(t::TEXT)
                    .show(ui);
            });
            ui.add_space(6.0);
            widgets::label(ui, &self.model.note(), t::MUTED, t::FONT_SIZE);
            ui.add_space(6.0);
            widgets::label(ui, &self.model.status, t::TEXT, t::FONT_SIZE);
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, &tr("닫기")).clicked() {
                    close = true;
                }
                ui.add_space(6.0);
                if Button::new(&tr("보내기")).kind(Kind::Normal).enabled(!self.model.sending()).show(ui).clicked() {
                    send = true;
                }
            });
        });
        if send {
            self.model.submit(cx);
        }
        if self.model.sending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        !(close || response.should_close())
    }
}

/// The references sent from this computer, newest last (the preference `feedback_sent`).
pub fn sent_references(cx: &AppContext) -> Vec<(String, String, String)> {
    let Some(prefs) = preferences(cx) else { return Vec::new() };
    let Some(Pref::List(items)) = prefs.values.get(feedback::SENT) else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| match item {
            Pref::Object(o) => {
                let text = |key: &str| o.get(key).and_then(Pref::as_str).unwrap_or("").to_owned();
                Some((text("ref"), text("version"), text("sent")))
            }
            _ => None,
        })
        .collect()
}
