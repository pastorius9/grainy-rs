//! The Lens correction panel (렌즈 교정): 렌즈 자동 선택 and 카메라·렌즈 선택, the five profile check boxes (enabled
//! as `StudioTools.load_settings` enables them: there is a profile, and it can do what the box switches), the
//! profile's description, and the manual lens (the seven sliders, saving and loading a lens file).
//!
//! Port of luma/studio.py `StudioTools.build_tools`' lens box. The commands behind the buttons, the labels and the
//! enabling are in [`crate::commands::lens`].

use eframe::egui::{self, Ui};
use grainy_i18n::tr;

use super::Panel;
use crate::adjustments::Place;
use crate::commands::lens as lens_commands;
use crate::context::AppContext;
use crate::widgets::{self, Button};

/// The lens panel.
pub struct Lens;

impl Panel for Lens {
    fn key(&self) -> &'static str {
        "lens"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

/// The check boxes: settings key and the oracle's text.
pub const SWITCHES: [(&str, &str); 5] = [
    ("lensfun_enabled", "렌즈 프로파일 사용"),
    ("lensfun_distortion", "프로파일 왜곡 보정"),
    ("lensfun_tca", "프로파일 색수차 보정"),
    ("lensfun_vignette", "프로파일 주변광량 보정"),
    ("lensfun_scale", "렌즈 보정의 빈 가장자리 맞춤"),
];

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let labels = [tr("렌즈 자동 선택"), tr("카메라·렌즈 선택")];
    let ids = ["lens.auto", "lens.choose"];
    // `button_row`, with the tooltip of the first button.
    let each = ((width - 4.0) / 2.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (label, id) in labels.iter().zip(ids) {
            let response = Button::new(label).width(each).show(ui);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            let response = if id == "lens.auto" {
                response.on_hover_text(tr("촬영 정보의 카메라·렌즈·초점 거리로 프로파일을 선택합니다."))
            } else {
                response
            };
            if response.clicked() {
                cx.run(id);
            }
        }
    });
    for (key, text) in SWITCHES {
        switch(ui, cx, key, text);
    }
    let auto = lens_commands::auto_label(cx.settings());
    widgets::muted(ui, &auto);
    let mut open = cx.foldouts.contains("lens_manual");
    widgets::foldout(ui, &tr("수동 렌즈 교정"), &mut open);
    cx.set_foldout("lens_manual", open);
    if open {
        cx.sliders(ui, Place::LensManual);
        for (text, id) in [("현재 렌즈 프로파일 저장", "lens.save"), ("렌즈 프로파일 불러오기", "lens.load")]
        {
            let label = tr(text);
            let response = Button::new(&label).width(width).show(ui);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            if response.clicked() {
                cx.run(id);
            }
        }
        let manual = lens_commands::manual_label(cx.settings());
        widgets::muted(ui, &manual);
    }
}

/// One check box: it shows the setting, takes clicks while [`lens_commands::switch_enabled`], and a click is one
/// undo step (`set_setting`, then the commit).
fn switch(ui: &mut Ui, cx: &mut AppContext, key: &str, text: &str) {
    let enabled = lens_commands::switch_enabled(cx.settings(), key);
    let mut value = cx.settings().bool(key);
    let changed = ui.add_enabled_ui(enabled, |ui| widgets::checkbox(ui, &mut value, &tr(text)).changed()).inner;
    if changed {
        let edit = cx.doc.edit_mut();
        edit.set(key, value);
        edit.commit("보정 변경");
        cx.before = false;
    }
}
