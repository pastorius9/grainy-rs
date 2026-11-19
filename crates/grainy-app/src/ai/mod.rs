//! The AI connection and Ask AI (work package 30): the MCP server of `grainy-mcp` started and stopped with the
//! window ([`connection`]), its [`grainy_mcp::Host`] on the application context and the job service ([`host`]),
//! and the Ask AI window's programs ([`programs`], [`discover`], [`auth`], [`install`], [`runner`], [`ask`]).
//!
//! Everything that touches the computer outside Grainy's own folders (the environment, program discovery,
//! starting a program, a console, a web page) goes through a [`system::System`]; the server through a
//! [`connection::Binder`]. [`configure`] replaces both (tests: a temporary home, fake programs, no sockets).
//!
//! Registration: `app.rs` calls [`install`] at start, [`frame`] every frame, [`stop_button`] in the status bar and
//! [`shutdown`] on exit; the commands are in `commands/ai.rs`, the windows in `dialogs/mcp.rs` and
//! `dialogs/ask_ai.rs`.

pub mod ask;
pub mod auth;
pub mod connection;
pub mod discover;
pub mod host;
pub mod install;
pub mod programs;
pub mod runner;
pub mod system;

use std::sync::Arc;

use eframe::egui::Ui;

use crate::context::AppContext;
use crate::text::tr;
use connection::{Connection, Setup};
use system::{HostSystem, System};

/// Adjusts the setup of a new connection (tests: a fake server, fixed keys and ports, their own Pictures).
pub type SetupHook = Box<dyn Fn(Setup) -> Setup>;

/// What the AI features keep in the context.
pub struct AiState {
    /// The computer as the AI features see it.
    pub system: Arc<dyn System>,
    /// Applied to every new connection's setup.
    pub setup: Option<SetupHook>,
    /// [`install`] ran: the connection follows the open library.
    pub installed: bool,
    /// The connection of the open library (a library on disk open for writing only).
    pub connection: Option<Connection>,
    /// The Ask AI window's state (kept while the window is closed, as the oracle keeps its dialog).
    pub ask: Option<ask::AskAi>,
}

impl Default for AiState {
    fn default() -> Self {
        Self { system: Arc::new(HostSystem::new()), setup: None, installed: false, connection: None, ask: None }
    }
}

/// Replaces the system and the connection setup (before [`install`]).
pub fn configure(cx: &mut AppContext, system: Arc<dyn System>, setup: Option<SetupHook>) {
    let state = cx.state::<AiState>();
    state.system = system;
    state.setup = setup;
}

/// Port of `DesktopSession.start`'s `mcp.start_if_enabled()` and `--ask-ai` (`window.show_ai()`).
pub fn install(cx: &mut AppContext, ask_ai: bool) {
    cx.state::<AiState>().installed = true;
    follow_library(cx);
    if ask_ai {
        cx.run("ai.ask");
    }
}

/// The connection of the library on screen: made (and started unless switched off) when a library open for writing
/// is shown, stopped when another one or none is.
fn follow_library(cx: &mut AppContext) {
    let library = if cx.doc.read_only() { None } else { cx.doc.library_folder() };
    let wanted = library.zip(cx.data_folder.clone());
    let current = cx.state_ref::<AiState>().and_then(|s| s.connection.as_ref()).map(|c| c.library().to_path_buf());
    if current.as_ref() == wanted.as_ref().map(|(l, _)| l) {
        return;
    }
    let wake: Arc<dyn Fn() + Send + Sync> = {
        let ctx = cx.egui.clone();
        Arc::new(move || ctx.request_repaint())
    };
    let state = cx.state::<AiState>();
    if let Some(mut old) = state.connection.take() {
        old.service.shutdown_window();
        old.stop();
    }
    if let Some((library, settings)) = wanted {
        let mut setup = Setup::new(settings, library, wake);
        if let Some(hook) = &state.setup {
            setup = hook(setup);
        }
        let mut connection = Connection::new(setup);
        connection.start_if_enabled();
        state.connection = Some(connection);
    }
}

/// Every frame: the connection follows the library, waiting calls and timers run (`GuiCall._run`, `QTimer`).
pub fn frame(cx: &mut AppContext) {
    if !cx.state_ref::<AiState>().is_some_and(|s| s.installed) {
        return;
    }
    follow_library(cx);
    host::pump(cx);
}

/// The running AI task's job, if any.
pub fn running_job(cx: &AppContext) -> Option<Arc<grainy_mcp::service::Job>> {
    let connection = cx.state_ref::<AiState>()?.connection.as_ref()?;
    connection.service.current_job().filter(|j| j.running())
}

/// The status bar's AI 작업 중단 button (`McpWindowState.stop_button`): shown while an AI task runs.
pub fn stop_button(ui: &mut Ui, cx: &mut AppContext) {
    if running_job(cx).is_none() {
        return;
    }
    if crate::widgets::button(ui, &tr("AI 작업 중단")).clicked() {
        cx.run("ai.stop_job");
    }
}

/// Port of `McpWindowState.shutdown` and `AiDialog.shutdown` (the window closes): a running job is cancelled,
/// the Ask AI run stops, the server stops.
pub fn shutdown(cx: &mut AppContext) {
    cx.state::<host::HostState>().closing = true;
    let state = cx.state::<AiState>();
    if let Some(ask) = state.ask.as_mut() {
        ask.shutdown();
    }
    if let Some(mut connection) = state.connection.take() {
        connection.service.shutdown_window();
        connection.stop();
    }
}
