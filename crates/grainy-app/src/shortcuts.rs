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
            let no_cmd = !modifiers.command && !modifiers.ctrl && !modifiers.alt;
            return (key == Key::Questionmark && no_cmd) || (key == Key::Slash && modifiers.shift && no_cmd);
        }
        let extra_ctrl = !self.command && modifiers.ctrl;
        key == self.key
            && !extra_ctrl
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
        if ctx.input(|i| i.key_pressed(Key::Escape))
            && !ctx.text_edit_focused()
            && !egui::Popup::is_any_open(&ctx)
            && self.dialogs.is_empty()
            && crate::commands::settings::leave(self)
        {
            ctx.input_mut(|i| {
                i.events.retain(|e| !matches!(e, Event::Key { key: Key::Escape, pressed: true, .. }));
            });
        }
        if self.modal_open() {
            return;
        }
        if self.mode == Mode::Develop && self.sides && self.expanded.contains(&"basic") && self.doc.current().is_some()
        {
            crate::widgets::basic_keys::route(&ctx);
        }
        for (row, sequence) in take_pressed(&ctx) {
            commands::run(self, row.command, &Invocation::key(sequence));
        }
        // Multi-selection shortcuts (Ctrl/Cmd+A select all, Ctrl/Cmd+D deselect) and arrow navigation.
        if !ctx.text_edit_focused() && !self.doc.is_empty() {
            let row = if self.mode == Mode::Library { self.grid_columns.max(1) as isize } else { 1 };
            let (select_all, deselect, step, shift) = ctx.input(|i| {
                let mut select_all = false;
                let mut deselect = false;
                let mut step = 0isize;
                let mut shift = false;
                for event in &i.events {
                    let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
                    let cmd_only = modifiers.command && !modifiers.alt && !modifiers.shift;
                    if cmd_only && *key == Key::A {
                        select_all = true;
                    } else if cmd_only && *key == Key::D {
                        deselect = true;
                    } else if !modifiers.command && !modifiers.alt && !modifiers.ctrl && !modifiers.mac_cmd {
                        let delta = match key {
                            Key::ArrowLeft => -1,
                            Key::ArrowRight => 1,
                            Key::ArrowUp => -row,
                            Key::ArrowDown => row,
                            _ => 0,
                        };
                        if delta != 0 {
                            step = delta;
                            shift = modifiers.shift || i.modifiers.shift;
                        }
                    }
                }
                (select_all, deselect, step, shift)
            });
            if select_all {
                self.doc.select_all();
            } else if deselect {
                self.doc.deselect_others();
                self.selection_cursor = self.doc.current_index();
            }
            if step != 0 {
                if shift {
                    if let Some(base) = self.selection_cursor.or_else(|| self.doc.current_index()) {
                        let target = base.saturating_add_signed(step).min(self.doc.len() - 1);
                        self.doc.select_range(target, false);
                        self.selection_cursor = Some(target);
                        self.scroll_to_current = true;
                    }
                } else if let Some(current) = self.doc.current_index() {
                    let target = current.saturating_add_signed(step).min(self.doc.len() - 1);
                    self.select_photo(target);
                }
            }
        }
    }
}
