//! Commands: every action of the window (menu items, buttons, shortcuts, later the AI connection) is a
//! [`Command`] with an id, a Korean label (shown through `tr`), an enabled-state function and a run function over
//! the [`AppContext`]. Menus, the shortcut table and buttons call [`run`] with an id; nothing calls private
//! methods of the window.
//!
//! Ids are those of `grainy_i18n::shortcuts::TABLE` where the oracle has a key for the action (`develop.undo`,
//! `cull.rating`…); the keys come from that table, so a command never lists its own keys.
//!
//! **Adding commands:** write `commands/<area>.rs` with `pub const COMMANDS: &[Command] = &[...]` and add one line
//! to [`REGISTRY`].

pub mod ai;
pub mod auto;
pub(crate) mod auto_source;
pub mod collections;
pub mod compare;
pub mod crop;
pub mod display;
pub mod dust;
pub mod edit;
pub mod export;
pub mod feedback;
pub mod file;
pub mod files;
pub mod healing;
pub mod import;
pub mod labels;
pub mod lens;
pub mod library_column;
pub mod lightroom;
pub mod masks;
pub mod metadata;
pub mod photo;
pub mod photo_actions;
pub mod pickers;
pub mod presets;
pub mod raw;
pub mod settings;
pub mod stacks;
pub mod sync;
pub mod view;
pub mod workflow;
pub mod xmp;

use serde_json::Value;

use crate::context::AppContext;

/// How a command was asked for: the key sequence that ran it (`Ctrl+[` and `Ctrl+]` run `develop.rotate` in two
/// directions, `3` runs `cull.rating` with three stars), or a value from a menu or a button.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Invocation {
    /// The key sequence of the shortcut table, in Qt notation.
    pub key: Option<&'static str>,
    /// A value given by a menu item or a button (a rating, a label name…).
    pub value: Option<Value>,
}

impl Invocation {
    /// A run from a key.
    pub fn key(key: &'static str) -> Self {
        Self { key: Some(key), value: None }
    }

    /// A run with a value.
    pub fn value(value: impl Into<Value>) -> Self {
        Self { key: None, value: Some(value.into()) }
    }
}

/// One action.
pub struct Command {
    /// Stable id (`develop.undo`).
    pub id: &'static str,
    /// The oracle's Korean text (menu item or button); pass through `tr`.
    pub label: &'static str,
    /// Whether it can run now (menus grey it out; shortcuts skip it).
    pub enabled: fn(&AppContext) -> bool,
    /// Runs it.
    pub run: fn(&mut AppContext, &Invocation),
}

/// Always enabled.
pub fn always(_: &AppContext) -> bool {
    true
}

/// Enabled while a photo is open.
pub fn photo_open(cx: &AppContext) -> bool {
    cx.doc.current().is_some()
}

/// Enabled while the list has photos.
pub fn has_photos(cx: &AppContext) -> bool {
    !cx.doc.is_empty()
}

/// Enabled when the open document stores its edits (a writable library).
pub fn writable_library(cx: &AppContext) -> bool {
    cx.doc.library_folder().is_some()
}

/// Every command, one line per feature area.
pub const REGISTRY: &[&[Command]] = &[
    edit::COMMANDS,
    view::COMMANDS,
    file::COMMANDS,
    labels::COMMANDS,
    photo::COMMANDS,
    auto::COMMANDS,
    presets::COMMANDS,
    healing::COMMANDS,
    crop::COMMANDS,
    masks::COMMANDS,
    pickers::COMMANDS,
    library_column::COMMANDS,
    compare::COMMANDS,
    collections::COMMANDS,
    sync::COMMANDS,
    metadata::COMMANDS,
    raw::COMMANDS,
    stacks::COMMANDS,
    lens::COMMANDS,
    settings::COMMANDS,
    dust::COMMANDS,
    ai::COMMANDS,
    export::COMMANDS,
    workflow::COMMANDS,
    xmp::COMMANDS,
    files::COMMANDS,
    display::COMMANDS,
    photo_actions::COMMANDS,
    import::COMMANDS,
    lightroom::COMMANDS,
    feedback::COMMANDS,
    crate::hdr::COMMANDS,
];

/// All commands.
pub fn all() -> impl Iterator<Item = &'static Command> {
    REGISTRY.iter().flat_map(|c| c.iter())
}

/// The command with this id.
pub fn find(id: &str) -> Option<&'static Command> {
    all().find(|c| c.id == id)
}

/// Whether command `id` can run now.
pub fn enabled(cx: &AppContext, id: &str) -> bool {
    find(id).is_some_and(|c| (c.enabled)(cx))
}

/// Runs command `id` if it exists and is enabled. Returns whether it ran.
pub fn run(cx: &mut AppContext, id: &str, invocation: &Invocation) -> bool {
    let Some(command) = find(id) else {
        log::info!("not implemented yet: {id}");
        return false;
    };
    if !(command.enabled)(cx) {
        return false;
    }
    (command.run)(cx, invocation);
    true
}

/// The keys of command `id` in the shortcut table (the first is the one menus show).
pub fn keys(id: &str) -> &'static [&'static str] {
    grainy_i18n::shortcuts::TABLE.iter().find(|row| row.command == id).map_or(&[], |row| row.keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_labels_translate() {
        let mut ids: Vec<&str> = all().map(|c| c.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "command ids are unique");
        for c in all() {
            assert!(!c.label.is_empty(), "{}", c.id);
        }
    }

    #[test]
    fn shortcut_rows_have_commands() {
        // Rows of the oracle's table that have a command in this run (the others are later work packages).
        for id in [
            "view.compare_original",
            "view.develop",
            "view.library",
            "view.clipping",
            "view.zoom_toggle",
            "view.full_screen",
            "view.sides",
            "view.bars",
            "view.shortcuts",
            "develop.crop",
            "develop.heal",
            "develop.undo",
            "develop.redo",
            "develop.auto_tone",
            "develop.auto_color",
            "develop.rotate",
            "develop.copy_edits",
            "develop.paste_edits",
            "cull.pick",
            "cull.reject",
            "cull.unflag",
            "cull.rating",
            "cull.color_label",
            "cull.virtual_copy",
            "file.import",
            "file.export",
        ] {
            assert!(find(id).is_some(), "{id}");
            assert!(!keys(id).is_empty(), "{id} has keys");
        }
    }
}
