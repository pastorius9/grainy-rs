//! 보기: screens, panels hidden, full screen, zoom, clipping, before/after, compare, the shortcut list, the debug
//! overlay (luma/screen_modes.py, luma/app.py view actions).

use eframe::egui;

use super::{always, photo_open, Command, Invocation};
use crate::context::{AppContext, Mode, Zoom};
use crate::dialogs::shortcuts::ShortcutList;

pub const COMMANDS: &[Command] = &[
    Command { id: "view.develop", label: "현상", enabled: always, run: develop },
    Command { id: "view.library", label: "라이브러리", enabled: always, run: library },
    Command { id: "view.sides", label: "양옆 패널 숨기기·보이기", enabled: always, run: sides },
    Command { id: "view.bars", label: "모든 패널 숨기기·보이기", enabled: always, run: bars },
    Command { id: "view.full_screen", label: "전체 화면", enabled: always, run: full_screen },
    Command { id: "view.shortcuts", label: "단축키 목록…", enabled: always, run: shortcuts },
    Command {
        id: "view.zoom_toggle", label: "1:1과 화면 맞춤 번갈아 보기", enabled: always, run: zoom_toggle
    },
    Command { id: "view.fit", label: "맞춤", enabled: always, run: fit },
    Command { id: "view.actual", label: "1:1", enabled: always, run: actual },
    Command { id: "view.clipping", label: "클리핑", enabled: always, run: clipping },
    Command { id: "view.compare_original", label: "원본 비교", enabled: photo_open, run: compare_original },
    Command { id: "view.debug", label: "디버그 정보 표시", enabled: always, run: debug },
];

fn develop(cx: &mut AppContext, _: &Invocation) {
    cx.set_mode(Mode::Develop);
}

fn library(cx: &mut AppContext, _: &Invocation) {
    cx.set_mode(Mode::Library);
}

/// Port of `ScreenModes.toggle_sides`.
fn sides(cx: &mut AppContext, _: &Invocation) {
    cx.sides = !cx.sides;
}

/// Port of `ScreenModes.toggle_bars`: everything hidden, or everything shown again when the sides and the bars
/// are both hidden already.
fn bars(cx: &mut AppContext, _: &Invocation) {
    let hidden = !cx.sides && !cx.bars;
    cx.sides = hidden;
    cx.bars = hidden;
}

/// Port of `ScreenModes.toggle_full_screen`.
pub fn full_screen(cx: &mut AppContext, _: &Invocation) {
    cx.full_screen = !cx.full_screen;
    cx.egui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(cx.full_screen));
}

fn shortcuts(cx: &mut AppContext, _: &Invocation) {
    if cx.dialog_open("shortcuts") {
        cx.close_dialog("shortcuts");
    } else {
        cx.open_dialog(Box::new(ShortcutList));
    }
}

fn zoom_toggle(cx: &mut AppContext, _: &Invocation) {
    cx.set_zoom(if cx.zoom == Zoom::Fit { Zoom::Actual } else { Zoom::Fit });
}

fn fit(cx: &mut AppContext, _: &Invocation) {
    cx.set_zoom(Zoom::Fit);
}

fn actual(cx: &mut AppContext, _: &Invocation) {
    cx.set_zoom(Zoom::Actual);
}

fn clipping(cx: &mut AppContext, _: &Invocation) {
    let on = cx.clip_shadows || cx.clip_highlights;
    cx.set_clipping(!on);
}

fn compare_original(cx: &mut AppContext, _: &Invocation) {
    cx.before = !cx.before;
}

fn debug(cx: &mut AppContext, _: &Invocation) {
    cx.debug_overlay = !cx.debug_overlay;
}
