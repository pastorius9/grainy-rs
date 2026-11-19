//! The Crop tool's panel (`tab_page('크롭')` of luma/app.py): 크롭 편집, 적용 / 취소, the crop's size, the aspect
//! box, 크롭 초기화, 스캔 테두리 자동 자르기, 시계 방향으로 90° 회전, 좌우 반전, 자동 수평 맞추기 and the angle
//! slider (`set_straighten`: the frame opens and follows the angle). The actions are the `crop.*` commands.

use eframe::egui::Ui;
use grainy_i18n::tr;

use super::controls;
use super::Panel;
use crate::adjustments;
use crate::commands::Invocation;
use crate::context::AppContext;
use crate::tools::crop::{self, ASPECTS};
use crate::widgets::{self, Button};

/// The crop panel.
pub struct Crop;

impl Panel for Crop {
    fn key(&self) -> &'static str {
        "crop"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let (open, size, aspect, note) = match crop::session(cx) {
        Some(s) => (s.frame.open, s.size_text.clone(), s.aspect_index, s.level_note.clone()),
        None => (false, String::new(), 0, None),
    };
    widgets::section(ui, "FRAME");
    widgets::muted(ui, &tr("모서리·변을 드래그해 크기를 조절하고,\n안쪽을 드래그해 위치를 옮기세요.\n틀 바깥을 드래그하면 사진이 회전합니다.\nEnter: 적용 / Esc: 취소"));
    if Button::new(&tr("크롭 편집")).checked(open).width(width).show(ui).clicked() {
        cx.run("crop.toggle");
    }
    let labels = [tr("적용"), tr("취소")];
    let row =
        [Button::new(&labels[0]).kind(widgets::Kind::Primary).enabled(open), Button::new(&labels[1]).enabled(open)];
    match widgets::button_row(ui, &row, width, 4.0) {
        Some(0) => {
            cx.run("crop.apply");
        }
        Some(1) => {
            cx.run("crop.cancel");
        }
        _ => {}
    }
    if !size.is_empty() {
        widgets::muted(ui, &size);
    }
    let names: Vec<String> = ASPECTS.iter().map(|(label, _)| tr(label)).collect();
    if let Some(i) = controls::choice(ui, "aspect", "", &names, aspect, width) {
        cx.run_with("crop.aspect", &Invocation::value(i));
    }
    if Button::new(&tr("크롭 초기화")).width(width).show(ui).clicked() {
        cx.run("crop.reset");
    }
    let border = Button::new(&tr("스캔 테두리 자동 자르기"))
        .width(width)
        .enabled(crate::commands::enabled(cx, "crop.border"))
        .show(ui)
        .on_hover_text(tr(
            "필름 스캔 가장자리의 검은 필름 테두리를 찾아 크롭합니다. 크롭 초기화나 실행 취소로 되돌릴 수 있습니다.",
        ));
    if border.clicked() {
        cx.run("crop.border");
    }
    let key = grainy_i18n::shortcuts::key_hint("Ctrl+[", grainy_i18n::Platform::current())
        .unwrap_or_else(|| "Ctrl+[".to_owned());
    let rotate = Button::new(&tr("시계 방향으로 90° 회전"))
        .width(width)
        .show(ui)
        .on_hover_text(crate::text::tr_with("{0}: 선택한 사진을 왼쪽 · 오른쪽으로 90° 돌립니다.", &[&key]));
    if rotate.clicked() {
        cx.run("crop.rotate");
    }
    if Button::new(&tr("좌우 반전")).width(width).show(ui).clicked() {
        cx.run("crop.flip");
    }
    let level = Button::new(&tr("자동 수평 맞추기"))
        .width(width)
        .enabled(crate::commands::enabled(cx, "crop.auto_level"))
        .show(ui)
        .on_hover_text(tr("사진의 수평·수직 직선을 분석해 기울기를 보정합니다."));
    if level.clicked() {
        cx.run("crop.auto_level");
    }
    let note = note.unwrap_or_else(|| tr("직선이 보이는 사진에서 자동 보정한 뒤 아래 슬라이더로 미세 조절하세요."));
    widgets::muted(ui, &note);
    // The angle slider: `set_straighten` (the frame opens, the crop follows); released, one undo step.
    if let Some(spec) = adjustments::spec("straighten") {
        let value = cx.settings().f64("straighten");
        let event = widgets::adjustment(ui, spec, value, true);
        if let Some(value) = event.set {
            crop::straighten_slider(cx, value);
        }
        if event.commit {
            cx.doc.edit_mut().commit("보정 변경");
        }
    }
    widgets::muted(
        ui,
        &tr("각도를 바꾸면 크롭 틀이 사진 안쪽에 맞게 자동으로 조정됩니다. 틀 바깥은 적용할 때 잘립니다."),
    );
}

/// "3 : 2" → 1.5; "자유 비율" → None.
pub fn aspect_ratio(text: &str) -> Option<f64> {
    let text = text.split('·').next()?.trim();
    let (a, b) = text.split_once(':')?;
    let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a > 0.0 && b > 0.0).then_some(a / b)
}

/// The largest crop `[left, top, right, bottom]` (fractions) with `ratio` = width / height, centred in a
/// `w` × `h` frame.
pub fn centred_crop(w: f64, h: f64, ratio: f64) -> Option<[f64; 4]> {
    if !(w > 0.0 && h > 0.0 && ratio > 0.0) {
        return None;
    }
    let (fw, fh) = if w / h > ratio { (ratio * h / w, 1.0) } else { (1.0, w / (ratio * h)) };
    Some([(1.0 - fw) / 2.0, (1.0 - fh) / 2.0, (1.0 + fw) / 2.0, (1.0 + fh) / 2.0])
}
