//! The AI connection's commands (work package 30): Settings › AI 연결 (MCP)…, the header's AI에게 시키기, the status
//! bar's AI 작업 중단 and 마지막 AI 작업 되돌리기 of both AI windows (port of `McpWindowState.open_dialog` /
//! `stop_job`, `MainWindow.show_ai`, `McpDialog.undo_last`).

use grainy_mcp::service::{ready, Ready};
use grainy_mcp::Host;

use super::{always, Command, Invocation};
use crate::ai::host::AppHost;
use crate::ai::AiState;
use crate::context::AppContext;
use crate::text::tr;

pub const COMMANDS: &[Command] = &[
    Command { id: "ai.connection", label: "AI 연결 (MCP)…", enabled: always, run: connection },
    Command { id: "ai.ask", label: "AI에게 시키기", enabled: always, run: ask },
    Command { id: "ai.stop_job", label: "AI 작업 중단", enabled: job_running, run: stop_job },
    Command { id: "ai.undo_last", label: "마지막 AI 작업 되돌리기", enabled: always, run: undo_last_command },
];

fn job_running(cx: &AppContext) -> bool {
    crate::ai::running_job(cx).is_some()
}

/// `McpWindowState.open_dialog`.
fn connection(cx: &mut AppContext, _: &Invocation) {
    cx.open_dialog(Box::new(crate::dialogs::mcp::McpDialog::default()));
}

/// Port of `MainWindow.show_ai`: the Ask AI window (made once, kept while closed). It needs the connection of a
/// library open for writing.
fn ask(cx: &mut AppContext, _: &Invocation) {
    if cx.state_ref::<crate::ai::host::HostState>().is_some_and(|s| s.closing) {
        return;
    }
    let state = cx.state::<AiState>();
    let Some(link) = state.connection.as_ref() else {
        log::info!("Ask AI needs a library open for writing");
        return;
    };
    if state.ask.is_none() {
        let model = crate::ai::ask::AskAi::standard(link, state.system.clone());
        state.ask = Some(model);
    }
    if let Some(model) = state.ask.as_mut() {
        model.shown();
    }
    cx.open_dialog(Box::new(crate::dialogs::ask_ai::AskAiDialog::default()));
}

/// Port of `McpWindowState.stop_job`.
fn stop_job(cx: &mut AppContext, _: &Invocation) {
    if let Some(connection) = cx.state_ref::<AiState>().and_then(|s| s.connection.as_ref()) {
        connection.service.stop_job();
    }
}

fn undo_last_command(cx: &mut AppContext, _: &Invocation) {
    if let Some(message) = undo_last(cx) {
        cx.set_status(message);
    }
}

/// Port of `McpDialog.undo_last`: the person's own undo of the last AI task, whether or not the connection is on.
/// Returns the message the window shows, if any.
pub fn undo_last(cx: &mut AppContext) -> Option<String> {
    let service = cx.state_ref::<AiState>()?.connection.as_ref()?.service.clone();
    let mut host = AppHost::new(cx)?;
    let current: Vec<i64> = host.current_id().into_iter().collect();
    let result = (|| {
        ready(&host, &service, &Ready { write: true, ids: &current, ..Ready::default() })?;
        if !current.is_empty() {
            host.commit()?;
            host.save_keywords()?;
        }
        grainy_service::stack::undo_batch(host.catalog(), None, false)
    })();
    match result {
        Err(grainy_service::Error::Busy(_)) => {
            Some(tr("지금은 되돌릴 수 없습니다. 열린 창이나 진행 중인 작업을 마친 뒤 다시 눌러 주세요."))
        }
        Err(grainy_service::Error::Service(text)) if text.contains("can no longer be undone") => Some(tr(
            "이후에 다시 보정한 사진이 있어 이 AI 작업은 되돌릴 수 없습니다. 사진마다 실행 취소를 사용해 주세요.",
        )),
        Err(_) => None,
        Ok((_, tool, ids)) => {
            if !ids.is_empty() {
                if tool == "set_labels" {
                    host.refresh_lists();
                    if host.current_id().is_some_and(|c| ids.contains(&c)) {
                        host.load_controls();
                    }
                } else {
                    host.refresh_command_edits(&ids, "AI 실행 취소");
                }
            }
            service.note("undo_last_batch", "undone in Grainy");
            None
        }
    }
}

/// The undo line of both AI windows: the last AI batch (`photo_service.last_batch`) and whether a job runs.
pub fn undo_line(cx: &AppContext) -> (String, bool) {
    let closing = cx.state_ref::<crate::ai::host::HostState>().is_some_and(|s| s.closing);
    let entry = if closing {
        None
    } else {
        cx.doc.metadata_catalog().and_then(|c| grainy_service::stack::last_batch(c).ok().flatten())
    };
    crate::ai::ask::undo_line(entry.as_ref(), crate::ai::running_job(cx).is_some())
}

/// What the person sees in the window now, for Ask AI's context.
pub fn seen(cx: &AppContext) -> crate::ai::ask::Seen {
    let query = cx.doc.query();
    let photos = cx.doc.photos();
    crate::ai::ask::Seen {
        folder: query.folder.clone().map(|f| (f, query.recursive, photos.len())),
        selected: if cx.doc.listing_pending() {
            Vec::new()
        } else {
            cx.doc.selected_indices().iter().filter_map(|i| photos.get(*i)).map(|p| p.id as i64).collect()
        },
        current: cx.doc.current().map(|p| (p.id as i64, p.name.clone())),
    }
}
