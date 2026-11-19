//! Keyboard shortcuts from the oracle's table (`grainy_i18n::shortcuts::TABLE`, port of luma/shortcut_list.py):
//! each row runs its [`crate::commands`] command with the key that was pressed.

use eframe::egui::{self, Event, Key, Modifiers};
use grainy_i18n::shortcuts::{Shortcut, TABLE};

use crate::commands::{self, Invocation};
use crate::context::{AppContext, Mode};

/// A key sequence of the table in Qt notation, parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Parses `Ctrl+Shift+Z`, `Tab`, `Shift+/`, `\`, `Ctrl+'`. `Ctrl` is Command on macOS.
pub fn parse(sequence: &str) -> Option<Chord> {
    let (mods, name) = match sequence.rfind('+') {
        Some(at) if at + 1 < sequence.len() => (&sequence[..at], &sequence[at + 1..]),
        _ => ("", sequence),
    };
    let mut chord = Chord { key: Key::Escape, command: false, shift: false, alt: false };
    for part in mods.split('+').filter(|p| !p.is_empty()) {
        match part {
            "Ctrl" => chord.command = true,
            "Shift" => chord.shift = true,
            "Alt" => chord.alt = true,
            _ => return None,
        }
    }
    chord.key = match name {
        "\\" => Key::Backslash,
        "/" => Key::Slash,
        "[" => Key::OpenBracket,
        "]" => Key::CloseBracket,
        "'" => Key::Quote,
        other => Key::from_name(other)?,
    };
    Some(chord)
}

impl Chord {
    fn matches(&self, key: Key, modifiers: Modifiers) -> bool {
        if self.key == Key::Slash && self.shift && !self.command {
            // "?" arrives as Questionmark on most layouts, or as Shift+/.
            return (key == Key::Questionmark && !modifiers.command)
                || (key == Key::Slash && modifiers.shift && !modifiers.command);
        }
        key == self.key
            && modifiers.command == self.command
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
    }
}

/// The rows of the table whose key was pressed this frame, removed from the input so egui does not also use them.
/// Rows that do not repeat ignore key repeats. Text fields keep their keys: nothing is taken while one has the
/// focus (`safe_shortcut`).
pub fn take_pressed(ctx: &egui::Context) -> Vec<(&'static Shortcut, &'static str)> {
    if ctx.text_edit_focused() {
        return Vec::new();
    }
    let mut bindings: Vec<(Chord, &'static Shortcut, &'static str)> =
        TABLE.iter().flat_map(|row| row.keys.iter().filter_map(move |k| parse(k).map(|c| (c, row, *k)))).collect();
    // Most specific first (Shift+Tab before Tab).
    bindings.sort_by_key(|(c, _, _)| std::cmp::Reverse(u8::from(c.command) + u8::from(c.shift) + u8::from(c.alt)));
    let mut found = Vec::new();
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            let Event::Key { key, pressed: true, repeat, modifiers, .. } = event else { return true };
            match bindings.iter().find(|(c, _, _)| c.matches(*key, *modifiers)) {
                Some((_, row, sequence)) => {
                    if !*repeat || row.repeat {
                        found.push((*row, *sequence));
                    }
                    false
                }
                None => true,
            }
        });
    });
    found
}

impl AppContext {
    /// The keys of this frame: the shortcut table's rows run their commands, the arrow keys step through the
    /// list. Port of `safe_shortcut` and `ScreenModes.leave` (Esc leaves full screen).
    pub(crate) fn handle_shortcuts(&mut self) {
        let ctx = self.egui.clone();
        if ctx.input(|i| i.key_pressed(Key::Escape)) && !ctx.text_edit_focused() && self.dialogs.is_empty() {
            crate::commands::settings::leave(self);
        }
        if self.modal_open() {
            return;
        }
        for (row, sequence) in take_pressed(&ctx) {
            commands::run(self, row.command, &Invocation::key(sequence));
        }
        // Arrow keys: the previous or next photo (a row up or down in the grid).
        if !ctx.text_edit_focused() && !self.doc.is_empty() {
            let row = if self.mode == Mode::Library { self.grid_columns.max(1) as isize } else { 1 };
            let step = ctx.input(|i| {
                if i.key_pressed(Key::ArrowLeft) {
                    -1
                } else if i.key_pressed(Key::ArrowRight) {
                    1
                } else if i.key_pressed(Key::ArrowUp) {
                    -row
                } else if i.key_pressed(Key::ArrowDown) {
                    row
                } else {
                    0
                }
            });
            if step != 0 {
                if let Some(current) = self.doc.current_index() {
                    let target = current.saturating_add_signed(step).min(self.doc.len() - 1);
                    self.select_photo(target);
                }
            }
        }
    }
}
