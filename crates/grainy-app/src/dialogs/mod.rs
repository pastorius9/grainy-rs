//! Dialogs and windows: a [`Dialog`] is drawn every frame while it is on the context's stack
//! (`AppContext::dialogs`, the last on top) and closes itself by returning `false`. Native file dialogs run on
//! their own threads ([`native`]); their answers reach the open dialogs through [`Dialog::receive`].
//!
//! **Adding a dialog:** write `dialogs/<name>.rs` with a type implementing [`Dialog`], and open it from a command
//! with `cx.open_dialog(Box::new(...))`; the module gets one line below.

pub mod about;
pub mod ask_ai;
pub mod cloud_backup;
pub mod confirm;
pub mod copy_items;
pub mod display;
pub mod duplicates;
pub mod dust;
pub mod dust_batch;
pub mod export;
pub mod export_history;
pub mod feedback;
pub mod filter;
pub mod import_copy;
pub mod import_errors;
pub mod keyword_tree;
pub mod language;
pub mod lens;
pub mod lightroom;
pub mod maintenance;
pub mod map;
pub mod mask_component;
pub mod mcp;
pub mod metadata;
pub mod metadata_preset;
pub mod native;
pub mod preset_review;
pub mod profiles;
pub mod progress;
pub mod raw_defaults;
pub mod reference;
pub mod shortcuts;
pub mod snapshots;
pub mod survey;
pub mod update;
pub mod whats_new;

use eframe::egui;

use crate::context::AppContext;

/// A dialog or window.
pub trait Dialog {
    /// A stable id (one dialog per id is open at a time).
    fn id(&self) -> &'static str;

    /// Draws the dialog; returns `false` when it closed.
    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool;

    /// Whether the window behind it waits (shortcuts and arrow keys do not run).
    fn modal(&self) -> bool {
        true
    }

    /// A native file dialog answered.
    fn receive(&mut self, _chosen: &native::Chosen) {}
}

/// Draws the dialogs of the stack; closed ones leave it, dialogs opened meanwhile join it.
pub fn show_all(ctx: &egui::Context, cx: &mut AppContext) {
    let mut stack = std::mem::take(&mut cx.dialogs);
    stack.retain_mut(|dialog| dialog.show(ctx, cx));
    stack.append(&mut cx.dialogs);
    cx.dialogs = stack;
}

/// The frame of the oracle's dialogs.
pub fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(crate::theme::BG)
        .stroke(egui::Stroke::new(1.0, crate::theme::MENU_BORDER))
        .inner_margin(egui::Margin::same(16))
}
