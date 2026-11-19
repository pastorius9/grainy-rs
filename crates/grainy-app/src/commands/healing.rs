//! The Healing tool's actions (luma/studio.py `build_retouch` buttons, `heal_shortcut`): the panel's buttons run
//! them by id; Q (`develop.heal`, registered with the other 현상 tools in [`super::photo`]) calls
//! [`heal_shortcut`].

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::tools::healing;

pub const COMMANDS: &[Command] = &[
    Command { id: "heal.mode", label: "힐링 브러시", enabled: photo_open, run: mode },
    Command { id: "heal.apply", label: "실행 · 표시한 곳 제거", enabled: has_marks, run: apply },
    Command { id: "heal.remove_mark", label: "마지막 표시 취소", enabled: has_marks, run: remove_mark },
    Command { id: "heal.clear_marks", label: "표시 모두 취소", enabled: can_clear, run: clear_marks },
    Command { id: "heal.remove_last", label: "마지막 복구 제거", enabled: photo_open, run: remove_last },
    Command {
        id: "heal.automatic_source", label: "힐링 원본 자동 선택으로 복귀", enabled: photo_open, run: automatic
    },
    Command { id: "heal.finish", label: "도구 끝내기", enabled: photo_open, run: finish },
];

fn has_marks(cx: &AppContext) -> bool {
    cx.state_ref::<healing::HealingState>().is_some_and(|s| !s.staged.is_empty())
}

fn can_clear(cx: &AppContext) -> bool {
    cx.state_ref::<healing::HealingState>().is_some_and(healing::HealingState::can_clear)
}

/// Port of `heal_shortcut` (Q).
pub fn heal_shortcut(cx: &mut AppContext) {
    healing::sync(cx);
    healing::heal_shortcut(cx);
}

/// `mode(kind)`: the value is the kind (`clone`, `heal`, `inpaint`, `red_eye`); none picks the healing brush.
fn mode(cx: &mut AppContext, invocation: &Invocation) {
    healing::sync(cx);
    let wanted = invocation.value.as_ref().and_then(|v| v.as_str()).unwrap_or(healing::HEAL);
    if let Some(kind) = healing::KINDS.iter().find(|k| **k == wanted) {
        healing::set_mode(cx, Some(kind));
    }
}

fn apply(cx: &mut AppContext, _: &Invocation) {
    healing::sync(cx);
    healing::apply_healing(cx);
}

fn remove_mark(cx: &mut AppContext, _: &Invocation) {
    healing::sync(cx);
    healing::remove_healing(cx);
}

fn clear_marks(cx: &mut AppContext, _: &Invocation) {
    healing::sync(cx);
    healing::clear_healing(cx);
}

fn remove_last(cx: &mut AppContext, _: &Invocation) {
    healing::sync(cx);
    healing::remove_retouch(cx);
}

fn automatic(cx: &mut AppContext, _: &Invocation) {
    healing::sync(cx);
    healing::automatic_source(cx);
}

fn finish(cx: &mut AppContext, _: &Invocation) {
    healing::sync(cx);
    healing::set_mode(cx, None);
}
