//! Keyboard routing for the Basic panel. Register the rows that were actually laid out so
//! RAW, negative and advanced controls participate only when the corresponding rows exist.

use eframe::egui::{self, Context, Event, Id, Key, Response};

use super::RowEvent;
use crate::adjustments::{Place, Spec};

#[derive(Clone, Default)]
struct Rows {
    pass: u64,
    ids: Vec<Id>,
}

fn list_id() -> Id {
    Id::unique("develop_basic_keys")
}

/// Runs inside the widget so keyboard and pointer input go through the same edit code,
/// including camera-relative RAW white balance.
pub(super) fn on_row(response: &Response, spec: &Spec, value: f64, event: &mut RowEvent) {
    if matches!(spec.place, Place::Panel(_) | Place::LensManual | Place::Crop) {
        return;
    }
    let ctx = &response.ctx;
    let pass = ctx.cumulative_pass_nr();
    let id = response.id;
    let (scroll, steps) = ctx.data_mut(|d| {
        let rows = d.get_temp_mut_or_default::<Rows>(list_id());
        if rows.pass != pass {
            rows.ids.clear();
            rows.pass = pass;
        }
        rows.ids.push(id);
        (d.remove_temp::<bool>(id.with("scroll")).unwrap_or(false), d.remove_temp::<i64>(id.with("nudge")))
    });
    if scroll {
        response.request_focus();
        response.scroll_to_me(None);
    }
    if response.is_pointer_button_down_on() {
        response.request_focus();
    }
    if let Some(steps) = steps {
        let step = match spec.key {
            "kelvin" | "wb_reference" | "raw_kelvin" => 50,
            _ => 1,
        };
        let (low, high) = spec.steps();
        let target = (spec.position(value) + steps * step).clamp(low, high);
        if target != spec.position(value) {
            event.set = Some(spec.value_at(target));
            event.commit = true;
        }
    }
}

fn adjustment_key(event: &Event) -> Option<(Key, bool)> {
    let Event::Key { key, pressed: true, modifiers, .. } = event else { return None };
    if modifiers.command || modifiers.ctrl || modifiers.alt || modifiers.mac_cmd {
        return None;
    }
    match key {
        Key::ArrowUp | Key::ArrowDown | Key::Plus | Key::Minus => Some((*key, modifiers.shift)),
        Key::Period | Key::Comma if !modifiers.shift => Some((*key, false)),
        _ => None,
    }
}

/// Consume keys only if a Basic slider can handle them. Unclaimed arrow keys remain available
/// to the filmstrip, and text fields retain their own navigation and editing keys.
pub(crate) fn route(ctx: &Context) {
    if ctx.text_edit_focused()
        || egui::Popup::is_any_open(ctx)
        || !ctx.input(|i| i.events.iter().any(|e| adjustment_key(e).is_some()))
    {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    let Some(rows) = ctx.data(|d| d.get_temp::<Rows>(list_id())).filter(|r| r.pass + 1 == pass) else { return };
    if rows.ids.is_empty() {
        return;
    }
    let focused = ctx.memory(|m| m.focused());
    let mut selected = rows
        .ids
        .iter()
        .position(|id| Some(*id) == focused)
        .or_else(|| rows.ids.iter().position(|id| ctx.read_response(*id).is_some_and(|r| r.hovered())));
    let mut focus = None;
    let mut nudges = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let Some((key, shift)) = adjustment_key(e) else { return true };
            match key {
                Key::Period | Key::Comma => {
                    let len = rows.ids.len();
                    let next = if key == Key::Period {
                        selected.map_or(0, |n| (n + 1) % len)
                    } else {
                        selected.map_or(len - 1, |n| (n + len - 1) % len)
                    };
                    selected = Some(next);
                    focus = Some(rows.ids[next]);
                }
                _ => {
                    let Some(n) = selected else { return true };
                    let direction = if matches!(key, Key::ArrowUp | Key::Plus) { 1 } else { -1 };
                    nudges.push((rows.ids[n], direction * if shift { 10 } else { 1 }));
                }
            }
            false
        })
    });
    if !nudges.is_empty() || focus.is_some() {
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        ctx.data_mut(|d| {
            for (id, delta) in nudges {
                *d.get_temp_mut_or_default::<i64>(id.with("nudge")) += delta;
            }
            if let Some(id) = focus {
                d.insert_temp(id.with("scroll"), true);
            }
        });
    }
}
