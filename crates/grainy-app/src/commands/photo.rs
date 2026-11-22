//! 현상 tools and turns: rotate, the crop and healing tools (luma/app.py `rotate_photo`, `open_tool`).

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::panels::Tool;

pub const COMMANDS: &[Command] = &[
    Command { id: "develop.rotate", label: "90° 회전 (왼쪽 · 오른쪽)", enabled: photo_open, run: rotate },
    Command { id: "develop.crop", label: "크롭", enabled: always_develop, run: crop },
    Command { id: "develop.heal", label: "힐링 브러시", enabled: always_develop, run: heal },
];

fn always_develop(_: &AppContext) -> bool {
    true
}

/// `Ctrl+[` turns counter-clockwise, every other way clockwise.
fn rotate(cx: &mut AppContext, invocation: &Invocation) {
    let clockwise =
        invocation.key != Some("Ctrl+[") && invocation.value.as_ref().and_then(|v| v.as_bool()) != Some(false);
    cx.rotate(clockwise);
}

fn crop(cx: &mut AppContext, _: &Invocation) {
    if cx.tool() == Tool::Crop && !crate::tools::crop::is_open(cx) {
        crate::tools::crop::toggle(cx);
    } else {
        cx.open_tool(Tool::Crop);
    }
}

fn heal(cx: &mut AppContext, _: &Invocation) {
    super::healing::heal_shortcut(cx);
}
