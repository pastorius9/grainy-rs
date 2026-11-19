//! The display colour items of 보기·필터 (luma/manager.py: 화면 색상 관리…, 모니터 색상 자동 감지, 모니터 ICC
//! 프로파일 선택, 모니터 ICC 보정 해제) and 사진 배경 (luma/app.py `set_view_background`), as commands.

use std::path::PathBuf;

use super::{always, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::preset_review::FileWait;
use crate::display;
use crate::text::tr;

pub const COMMANDS: &[Command] = &[
    Command { id: "display.dialog", label: "화면 색상 관리…", enabled: always, run: dialog },
    Command { id: "display.auto", label: "모니터 색상 자동 감지", enabled: always, run: auto },
    Command { id: "display.choose", label: "모니터 ICC 프로파일 선택", enabled: always, run: choose },
    Command { id: "display.off", label: "모니터 ICC 보정 해제", enabled: always, run: off },
    Command { id: "view.background", label: "사진 배경", enabled: always, run: background },
];

/// `display_color.show_dialog`.
fn dialog(cx: &mut AppContext, _: &Invocation) {
    display::show_dialog(cx);
}

/// `display_color.configure('auto')`.
fn auto(cx: &mut AppContext, _: &Invocation) {
    let _ = display::configure(cx, "auto", "");
}

/// `Manager.monitor_profile`: the file box, then `configure('manual', path)`.
fn choose(cx: &mut AppContext, _: &Invocation) {
    let filter = ("ICC".to_owned(), vec!["icc", "icm"]);
    cx.open_dialog(Box::new(FileWait::open(tr("모니터 ICC 프로파일 선택"), filter, None, chosen)));
}

/// The answer of the file box of [`choose`].
pub fn chosen(cx: &mut AppContext, path: PathBuf) {
    let _ = display::configure(cx, "manual", &path.to_string_lossy());
}

/// `display_color.configure('off')`.
fn off(cx: &mut AppContext, _: &Invocation) {
    let _ = display::configure(cx, "off", "");
}

/// `set_view_background(key)`: the key comes from the menu item's value.
fn background(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(key) = invocation.value.as_ref().and_then(|v| v.as_str()) {
        display::set_background(cx, key);
    }
}
