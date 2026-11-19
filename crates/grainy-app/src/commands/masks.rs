//! 마스크: the `mask.*` rows of the shortcut table (luma/shortcut_list.py `MASKS` → `StudioTools.mask_shortcut`,
//! O → `toggle_overlay`). `mask.brush_size` ([ / ]) is shared with the healing brushes and comes with the brush
//! (work package 9).

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::masks::actions;

pub const COMMANDS: &[Command] = &[
    Command { id: "mask.new_brush", label: "새 브러시 마스크", enabled: photo_open, run: new_brush },
    Command {
        id: "mask.new_linear", label: "새 선형 그레이디언트 마스크", enabled: photo_open, run: new_linear
    },
    Command {
        id: "mask.new_radial", label: "새 방사형 그레이디언트 마스크", enabled: photo_open, run: new_radial
    },
    Command { id: "mask.overlay", label: "마스크 오버레이 켜기·끄기", enabled: photo_open, run: overlay },
    Command {
        id: "mask.brush_size",
        label: "브러시 크기 줄이기 · 키우기",
        enabled: photo_open,
        run: crate::tools::brush::brush_size,
    },
];

fn new_brush(cx: &mut AppContext, _: &Invocation) {
    actions::mask_shortcut(cx, "brush");
}

fn new_linear(cx: &mut AppContext, _: &Invocation) {
    actions::mask_shortcut(cx, "linear");
}

fn new_radial(cx: &mut AppContext, _: &Invocation) {
    actions::mask_shortcut(cx, "radial");
}

fn overlay(cx: &mut AppContext, _: &Invocation) {
    actions::toggle_overlay(cx);
}
