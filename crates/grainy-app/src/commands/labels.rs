//! 분류: flags, ratings, colour labels and virtual copies of the selected photos (luma/app.py `set_rating`,
//! `set_flag`; luma/manager.py `set_label`, `toggle_label`, `virtual_copy`).

use serde_json::{json, Value};

use super::{has_photos, Command, Invocation};
use crate::context::AppContext;
use crate::document::LabelField;
use crate::text::tr_with;

pub const COMMANDS: &[Command] = &[
    Command { id: "cull.pick", label: "선택 표시", enabled: has_photos, run: pick },
    Command { id: "cull.reject", label: "제외 표시", enabled: has_photos, run: reject },
    Command { id: "cull.unflag", label: "표시 해제", enabled: has_photos, run: unflag },
    Command { id: "cull.rating", label: "별점", enabled: has_photos, run: rating },
    Command { id: "cull.color_label", label: "색 라벨", enabled: has_photos, run: color_label },
    Command { id: "cull.virtual_copy", label: "가상 사본 만들기", enabled: has_photos, run: virtual_copy },
];

fn set(cx: &mut AppContext, field: LabelField, value: Value) {
    if let Err(message) = cx.doc.set_label_field(field, value) {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
    }
}

fn pick(cx: &mut AppContext, _: &Invocation) {
    set(cx, LabelField::Flag, json!(1));
}

fn reject(cx: &mut AppContext, _: &Invocation) {
    set(cx, LabelField::Flag, json!(-1));
}

fn unflag(cx: &mut AppContext, _: &Invocation) {
    set(cx, LabelField::Flag, json!(0));
}

/// The keys 0–5, or a menu's value.
fn rating(cx: &mut AppContext, invocation: &Invocation) {
    let stars = invocation
        .value
        .as_ref()
        .and_then(Value::as_i64)
        .or_else(|| invocation.key.and_then(|k| k.parse::<i64>().ok()))
        .unwrap_or(0)
        .clamp(0, 5);
    set(cx, LabelField::Rating, json!(stars));
}

/// The keys 6–9 (`toggle_label`: the label on the selected photos, or none when they all have it already), or a
/// menu's value (`set_label`, "" clears).
fn color_label(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(value) = invocation.value.as_ref().and_then(Value::as_str) {
        set(cx, LabelField::Label, json!(value));
        return;
    }
    let Some(key) = invocation.key else { return };
    let Some((_, label)) = grainy_i18n::shortcuts::LABELS.iter().find(|(k, _)| *k == key) else { return };
    let selected = cx.doc.selected_indices();
    let all = !selected.is_empty() && selected.iter().all(|i| cx.doc.photos()[*i].label == *label);
    set(cx, LabelField::Label, json!(if all { "" } else { *label }));
}

/// Port of `manager.virtual_copy`.
fn virtual_copy(cx: &mut AppContext, _: &Invocation) {
    if let Err(message) = cx.doc.virtual_copies() {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
    }
}
