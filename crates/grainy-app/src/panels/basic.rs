//! The Basic panel (기본): black and white, the automatic buttons, the negative, white balance, tone, presence and colour (luma/app.py `build_ui`, luma/studio.py `film_balance`, `drt_panel`).

use eframe::egui::Ui;
use grainy_i18n::{tr, tr_with};

use super::controls;
use super::Panel;
use crate::adjustments::Place;
use crate::context::AppContext;
use crate::theme as t;
use crate::tools;
use crate::widgets::{self, Button};

/// DRT looks: (Korean name, values of drt_contrast, drt_toe, drt_shoulder, drt_white). Port of
/// `tone_mapping.LOOKS`.
pub const DRT_LOOKS: [(&str, [f64; 4]); 4] = [
    ("표준", [0.0, 0.0, 0.0, 0.0]),
    ("부드럽게", [-35.0, -25.0, -40.0, 0.0]),
    ("대비 강하게", [40.0, 30.0, 30.0, 0.0]),
    ("필름처럼", [15.0, 35.0, -20.0, -45.0]),
];
const DRT_KEYS: [&str; 4] = ["drt_contrast", "drt_toe", "drt_shoulder", "drt_white"];

/// Film balance conversions: (key, Korean title, wb_reference, kelvin). Port of `FILM_BALANCE` and its buttons.
const FILM_BALANCE: [(&str, &str, f64, f64); 2] =
    [("tungsten", "텅스텐 → 데이라이트", 3200.0, 5500.0), ("daylight", "데이라이트 → 텅스텐", 5500.0, 3200.0)];

/// The look whose values the settings hold, or "" (custom). Port of `tone_mapping.look_of`.
pub fn drt_look(settings: &grainy_core::Settings) -> &'static str {
    let values: Vec<f64> = DRT_KEYS.iter().map(|k| settings.f64(k)).collect();
    DRT_LOOKS.iter().find(|(_, look)| look.iter().zip(&values).all(|(a, b)| a == b)).map(|(n, _)| *n).unwrap_or("")
}

/// The basic panel.
pub struct Basic;

impl Panel for Basic {
    fn key(&self) -> &'static str {
        "basic"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    tools::eyedropper::sync(cx);
    super::calibration::profile_box(ui, cx);
    let width = ui.available_width();
    let gap = 4.0;
    let negative = cx.settings().bool("negative");
    let auto_color_amount = crate::commands::auto::active_auto_color_amount(cx);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let mut mono = cx.settings().bool("monochrome");
        if widgets::checkbox(ui, &mut mono, &tr("흑백")).changed() {
            cx.doc.edit_mut().set("monochrome", mono);
            cx.doc.edit_mut().commit("보정 변경");
        }
        let cell = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
        let tip = tr("스캔한 네거티브 필름을 사진으로 바꿉니다. 필름 베이스 색(컬러 네거티브의 주황색)을 빼고 채널별로 뒤집습니다. 다른 보정은 반전한 사진에 그대로 적용됩니다.");
        if Button::new(&crate::text::tr("네거티브")).checked(negative).width(cell).show(ui).on_hover_text(tip).clicked() {
            cx.run("develop.negative");
        }
        let tone_tip = tr("현재 크롭 영역의 밝기 하위·상위 0.1%를 기준으로 분석합니다. 검정·흰색 양끝에 3% 여유를 두고 약 8–247/255로 보정합니다.");
        if Button::new(&tr("자동 톤")).width(cell).show(ui).on_hover_text(format!("{tone_tip} ({})", controls::hint("Ctrl+U"))).clicked() {
            cx.run("develop.auto_tone");
        }
        let color_tip = tr("어두운 곳·중간·밝은 곳의 무채색 영역을 각각 찾아 색 틀어짐(녹색 끼 등)을 채널 커브로 바로잡습니다. 밝기는 그대로 둡니다. 강한 색 조명만 있는 영역은 건드리지 않습니다.");
        let strength_hint = crate::text::tr("자동 색 세기 슬라이더로 적용 정도(0–100%)를 실시간으로 조절할 수 있습니다.");
        if Button::new(&tr("자동 색"))
            .checked(auto_color_amount.is_some())
            .width(cell)
            .show(ui)
            .on_hover_text(format!("{color_tip} {strength_hint} ({})", controls::hint("Ctrl+Shift+U")))
            .clicked()
        {
            cx.run("develop.auto_color");
        }
    });
    if let Some(amount) = auto_color_amount {
        let spec = crate::adjustments::Spec {
            key: "auto_color_amount",
            label: "자동 색 세기",
            low: 0.0,
            high: 100.0,
            scale: 1.0,
            reset: crate::adjustments::Reset::Value(100.0),
            place: Place::Panel("basic"),
        };
        let event = widgets::adjustment_row(ui, "auto_color_amount", &tr("자동 색 세기"), &spec, amount, true);
        crate::commands::auto::apply_auto_color_amount(cx, event);
    }
    // Additional actions only appear for HDR photos and inverted film scans.
    crate::hdr::button(ui, cx, width);
    if cx.settings().bool("negative") {
        cx.sliders(ui, Place::Negative);
        let labels = [tr("필름 베이스 찍기"), tr("다시 분석")];
        let row = [Button::new(&labels[0]).checked(tools::eyedropper::base_checked(cx)), Button::new(&labels[1])];
        match widgets::button_row(ui, &row, width, gap) {
            Some(0) => {
                cx.run("develop.negative_base");
            }
            Some(_) => {
                cx.run("develop.negative_again");
            }
            None => {}
        }
    }
    // 화이트 밸런스
    let wb_tip = tr_with!(
        "사진에서 중성 회색이나 흰색을 클릭하거나, 드래그해 사각형 영역의 평균으로 맞춥니다. ({0})",
        controls::hint("W")
    );
    ui.horizontal(|ui| {
        widgets::section(ui, &tr("화이트 밸런스"));
        if Button::new(&crate::text::tr("스포이드"))
            .checked(tools::eyedropper::wb_checked(cx))
            .width(ui.available_width())
            .show(ui)
            .on_hover_text(wb_tip)
            .clicked()
        {
            cx.run("develop.white_balance");
        }
    });
    cx.sliders(ui, Place::WhiteBalance);
    if cx.raw_view() {
        super::calibration::raw_white_balance(ui, cx);
    }
    let mut open = cx.foldouts.contains("wb_detail");
    widgets::foldout(ui, &tr("화이트 밸런스 상세"), &mut open);
    cx.set_foldout("wb_detail", open);
    if open {
        let s = cx.settings();
        let checked = FILM_BALANCE.map(|(_, _, reference, light)| {
            s.bool("kelvin_enabled") && s.f64("wb_reference") == reference && s.f64("kelvin") == light
        });
        let labels = FILM_BALANCE.map(|(_, title, _, _)| tr(title));
        let row = [Button::new(&labels[0]).checked(checked[0]), Button::new(&labels[1]).checked(checked[1])];
        if let Some(i) = widgets::button_row(ui, &row, width, gap) {
            film_balance(cx, i, !checked[i]);
        }
        let mut kelvin = cx.settings().bool("kelvin_enabled");
        if widgets::checkbox(ui, &mut kelvin, &tr("켈빈 화이트 밸런스 사용")).changed() {
            cx.doc.edit_mut().set("kelvin_enabled", kelvin);
            cx.doc.edit_mut().commit("보정 변경");
        }
        widgets::muted(ui, &tr("카메라 WB로 현상한 사진의 기준 백색점에 대한 상대 보정입니다."));
        cx.sliders(ui, Place::WhiteBalanceDetail);
    }
    // 톤
    let modes = [("classic", "톤 매핑 · 기본"), ("drt", "톤 매핑 · 부드러운 하이라이트 (DRT)")];
    let current = cx.settings().str("tone_mapping").to_owned();
    let index = modes.iter().position(|(v, _)| *v == current).unwrap_or(0);
    let names: Vec<String> = modes.iter().map(|(_, n)| tr(n)).collect();
    let chosen = ui
        .horizontal(|ui| {
            widgets::section(ui, &tr("톤"));
            controls::choice(ui, "tone_mapping", &tr("톤 매핑"), &names, index, ui.available_width())
        })
        .inner;
    if let Some(chosen) = chosen {
        if modes[chosen].0 != current {
            cx.set_and_commit("tone_mapping", modes[chosen].0, "톤 매핑");
        }
    }
    if current == "drt" {
        drt_controls(ui, cx, width);
    }
    cx.sliders(ui, Place::Tone);
    // The HDR section while the photo is in HDR mode (`hdr_panel`; nothing while the HDR mode is off).
    crate::hdr::panel(ui, cx);
    widgets::section(ui, &tr("존재감"));
    cx.sliders(ui, Place::Presence);
    cx.sliders(ui, Place::Color);
}

/// Port of `StudioTools.film_balance`.
fn film_balance(cx: &mut AppContext, index: usize, checked: bool) {
    let (_, title, reference, light) = FILM_BALANCE[index];
    cx.doc.edit_mut().commit("보정 변경");
    if checked {
        cx.doc.edit_mut().set("kelvin_enabled", true);
        cx.doc.edit_mut().set("wb_reference", reference);
        cx.doc.edit_mut().set("kelvin", light);
        cx.doc.edit_mut().commit(title);
    } else {
        cx.doc.edit_mut().set("kelvin_enabled", false);
        cx.doc.edit_mut().commit("필름 색온도 변환 해제");
    }
}

/// The DRT's look choice, its Advanced button and sliders (`drt_panel`).
fn drt_controls(ui: &mut Ui, cx: &mut AppContext, width: f32) {
    let look = drt_look(cx.settings());
    let mut names: Vec<String> = DRT_LOOKS.iter().map(|(n, _)| tr(n)).collect();
    names.push(tr("사용자 정의"));
    let index = DRT_LOOKS.iter().position(|(n, _)| *n == look).unwrap_or(DRT_LOOKS.len());
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        widgets::label(ui, &tr("DRT 룩"), t::TEXT, t::FONT_SIZE);
        let advanced = tr("고급 설정");
        let button_width = Button::new(&advanced).size(ui).x;
        let room = ui.available_width() - button_width - 6.0;
        chosen = controls::choice(ui, "drt_look", &tr("DRT 룩"), &names, index, room);
        if Button::new(&advanced).checked(cx.drt_advanced).show(ui).clicked() {
            cx.drt_advanced = !cx.drt_advanced;
        }
    });
    let _ = width;
    if let Some(i) = chosen.filter(|i| *i < DRT_LOOKS.len()) {
        let values = DRT_LOOKS[i].1;
        let settings = cx.settings();
        if !DRT_KEYS.iter().zip(values).all(|(k, v)| settings.f64(k) == v) {
            cx.doc.edit_mut().commit("보정 변경");
            for (key, value) in DRT_KEYS.iter().zip(values) {
                cx.doc.edit_mut().set(key, value);
            }
            cx.doc.edit_mut().commit("DRT 룩");
        }
    }
    if cx.drt_advanced {
        cx.sliders(ui, Place::Drt);
    }
}
