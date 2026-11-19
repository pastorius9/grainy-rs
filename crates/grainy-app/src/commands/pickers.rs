//! The two pickers of the Basic panel as commands: 화이트 밸런스 스포이드 (`develop.white_balance`, W) and
//! 필름 베이스 찍기 (`develop.negative_base`). See [`crate::tools::eyedropper`].

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::tools::eyedropper;

pub const COMMANDS: &[Command] = &[
    Command {
        id: "develop.white_balance", label: "화이트 밸런스 스포이드", enabled: photo_open, run: white_balance
    },
    Command { id: "develop.negative_base", label: "필름 베이스 찍기", enabled: negative_on, run: negative_base },
];

fn negative_on(cx: &AppContext) -> bool {
    photo_open(cx) && cx.settings().bool("negative")
}

/// `wb_shortcut`: the eyedropper on or off.
fn white_balance(cx: &mut AppContext, _: &Invocation) {
    eyedropper::shortcut(cx);
}

/// The film base button: the picker on or off.
fn negative_base(cx: &mut AppContext, _: &Invocation) {
    eyedropper::sync(cx);
    let checked = eyedropper::base_checked(cx);
    eyedropper::toggle_base(cx, !checked);
}
