//! The Camera calibration panel (카메라 보정): working space, the RAW profile fold-out, primaries, the colour matrix;
//! and the RAW controls the oracle puts on the Basic page: the camera profile row above the automatic buttons
//! (`profile_box`) and the RAW white-balance choice with its temperature and tint (`wb_box`).
//!
//! Ports luma/studio.py `build_color`'s calibration and RAW widgets, `load_raw_controls` (what is shown and
//! enabled, the RAW note), `change_working`, `change_raw_mode`, `raw_temperature_changed`, `display_raw_white`;
//! the actions are [`crate::commands::raw`].

use eframe::egui::Ui;
use serde_json::Value;

use super::controls::{self, list_spec};
use super::Panel;
use crate::adjustments::{self, Place};
use crate::commands::raw::{self, RAW_MODES};
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::widgets::{self, Button};

/// The calibration panel.
pub struct Calibration;

impl Panel for Calibration {
    fn key(&self) -> &'static str {
        "calibration"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

/// `usable` of `load_raw_controls`: a RAW photo is open (the window reads it, or its stored preview, in the
/// camera's colours).
fn usable(cx: &AppContext) -> bool {
    cx.raw_view() && cx.doc.current().is_some_and(|p| raw::is_raw_path(&p.path))
}

/// The DCP note (`dcp_note`): the pinned profile's name, camera and copyright, or DCP 프로파일 없음.
pub fn dcp_note(settings: &grainy_core::Settings) -> String {
    match settings.value("dcp_profile") {
        Value::Object(p) if !p.is_empty() => {
            let text = |k: &str| p.get(k).map(crate::commands::metadata::python_str).unwrap_or_default();
            format!("{} · {}\n{}", text("name"), text("camera"), text("copyright"))
        }
        _ => tr("DCP 프로파일 없음"),
    }
}

/// Python's `f"{v:,}"` for the as-shot temperature (an integer in the stored info).
fn grouped(value: &Value) -> String {
    let text = match value.as_f64() {
        Some(v) if v.fract() == 0.0 && v.abs() < 1e15 => format!("{}", v as i64),
        _ => crate::commands::metadata::python_str(value),
    };
    let (sign, digits) = text.strip_prefix('-').map_or(("", text.as_str()), |d| ("-", d));
    let (whole, rest) = digits.split_once('.').map_or((digits, None), |(w, r)| (w, Some(r)));
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}{}", rest.map(|r| format!(".{r}")).unwrap_or_default())
}

/// Python's `f"{v:+g}"`.
fn signed_g(value: &Value) -> String {
    let v = value.as_f64().unwrap_or(0.0);
    let text = if v == v.trunc() && v.abs() < 1e16 {
        format!("{}", v as i64)
    } else {
        let digits = 6 - 1 - v.abs().log10().floor() as i32;
        let rounded = format!("{:.*}", digits.max(0) as usize, v);
        let trimmed = rounded.trim_end_matches('0').trim_end_matches('.').to_owned();
        trimmed
    };
    if v >= 0.0 && !text.starts_with('-') {
        format!("+{text}")
    } else {
        text
    }
}

/// The RAW note of `load_raw_controls` for a photo's stored `info`.
pub fn raw_note(usable: bool, offline: bool, info: &Value) -> String {
    let mut message = if usable {
        tr("RAW 센서 단계에서 적용합니다. 양의 색조는 자홍색 방향 보정입니다.")
    } else {
        tr("RAW 원본을 선택하면 카메라 색온도와 DCP를 사용할 수 있습니다.")
    };
    if info.get("raw_as_shot_kelvin").is_some_and(grainy_core::settings::truthy) {
        message += &tr_with(
            "\n촬영 시 약 {0}K · 색조 {1}",
            &[&grouped(&info["raw_as_shot_kelvin"]), &signed_g(&info["raw_as_shot_tint"])],
        );
    }
    if offline {
        message += &tr("\n저장된 RAW 미리보기로 편집 중입니다.");
    }
    message
}

/// The camera profile row of the Basic page (`profile_box`): 카메라 프로파일 찾아보기 and the DCP note, for RAW
/// photos only.
pub fn profile_box(ui: &mut Ui, cx: &mut AppContext) {
    if !usable(cx) {
        return;
    }
    if Button::new(&tr("카메라 프로파일 찾아보기")).width(ui.available_width()).show(ui).clicked() {
        cx.run("raw.browse_dcp");
    }
    widgets::label(ui, &dcp_note(cx.settings()), crate::theme::TEXT, crate::theme::FONT_SIZE);
}

/// The RAW white balance of the Basic page: the mode (`change_raw_mode`), the temperature and tint
/// (`raw_temperature_changed`), which show the resolved camera white in the camera modes (`display_raw_white`).
pub fn raw_white_balance(ui: &mut Ui, cx: &mut AppContext) {
    let camera = raw::camera_info(cx);
    let current = cx.settings().str("raw_mode").to_owned();
    let index = RAW_MODES.iter().position(|(v, _)| *v == current).unwrap_or(0);
    let names: Vec<String> = RAW_MODES.iter().map(|(_, n)| tr(n)).collect();
    if let Some(chosen) = controls::choice(ui, "raw_mode", "", &names, index, ui.available_width()) {
        let mode = RAW_MODES[chosen].0;
        // 스포이드 can be chosen only once a neutral was sampled.
        let allowed = mode != "sample" || grainy_core::settings::truthy(cx.settings().value("raw_neutral"));
        if mode != current && allowed {
            let settings = cx.settings().clone();
            let info = raw::source_info(&settings, camera.as_ref());
            raw::change_raw_mode(cx.doc.edit_mut(), mode, info);
            cx.before = false;
        }
    }
    let enabled = current != "legacy";
    // Settings versions are local to each photo. Camera metadata may arrive after the first frame.
    #[derive(Default)]
    struct WhiteDisplay {
        key: Option<(u64, u64, u64, bool)>,
        shown: Option<(f64, f64)>,
    }
    let key = (cx.document_generation, cx.doc.current_id(), cx.doc.edit().version(), camera.is_some());
    if cx.state::<WhiteDisplay>().key != Some(key) {
        let shown = raw::display_raw_white(cx.settings(), raw::source_info(cx.settings(), camera.as_ref()));
        *cx.state::<WhiteDisplay>() = WhiteDisplay { key: Some(key), shown };
    }
    let shown = cx.state::<WhiteDisplay>().shown;
    let displayed = shown.unwrap_or((cx.settings().f64("raw_kelvin"), cx.settings().f64("raw_tint")));
    for spec in adjustments::at(Place::Raw) {
        let value = if spec.key == "raw_kelvin" { displayed.0 } else { displayed.1 };
        let event = widgets::adjustment(ui, spec, value, enabled);
        if let Some(new) = event.set {
            // The other slider keeps what it shows (`control.slider.value()` of both).
            let (kelvin, tint) = if spec.key == "raw_kelvin" { (new, displayed.1) } else { (displayed.0, new) };
            raw::raw_temperature_changed(cx.doc.edit_mut(), kelvin, tint);
            cx.before = false;
        }
        if event.commit {
            cx.doc.edit_mut().commit("보정 변경");
        }
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let spaces = [("sRGB", "작업 색공간 · sRGB"), ("ProPhoto", "작업 색공간 · ProPhoto 광색역")];
    let current = cx.settings().str("working_space").to_owned();
    let index = spaces.iter().position(|(v, _)| *v == current).unwrap_or(0);
    let names: Vec<String> = spaces.iter().map(|(_, n)| tr(n)).collect();
    if let Some(chosen) = controls::choice(ui, "working_space", "", &names, index, width) {
        if spaces[chosen].0 != current && cx.doc.current().is_some() {
            raw::change_working(cx.doc.edit_mut(), spaces[chosen].0);
        }
    }
    let mut open = cx.foldouts.contains("raw_profile");
    widgets::foldout(ui, &tr("RAW 프로파일 상세"), &mut open);
    cx.set_foldout("raw_profile", open);
    if open {
        raw_profile(ui, cx, width);
    }
    for (i, title) in ["빨강 원색 보정", "초록 원색 보정", "파랑 원색 보정"].iter().enumerate() {
        widgets::muted(ui, &tr(title));
        for (j, name) in ["색상", "채도"].iter().enumerate() {
            let spec = list_spec("calibration", -100.0, 100.0);
            cx.list_slider(ui, &spec, name, i, Some(j));
        }
    }
    let mut open = cx.foldouts.contains("camera_matrix");
    widgets::foldout(ui, &tr("색 보정 행렬"), &mut open);
    cx.set_foldout("camera_matrix", open);
    if open {
        if Button::new(&tr("색 보정 행렬 프로파일 불러오기")).width(width).show(ui).clicked() {
            cx.run("raw.camera_matrix");
        }
        if Button::new(&tr("색 보정 행렬 해제")).width(width).show(ui).clicked() {
            cx.set_and_commit("camera_matrix", Value::Null, "보정 변경");
        }
        widgets::muted(
            ui,
            &tr("행렬 프로파일: 현상된 RGB에 적용하는 3×3 JSON 행렬. RAW 카메라 DCP는 위에서 선택합니다."),
        );
    }
}

/// The RAW 프로파일 상세 fold-out (`load_raw_controls`'s note, buttons and switches).
fn raw_profile(ui: &mut Ui, cx: &mut AppContext, width: f32) {
    #[derive(Default)]
    struct Details {
        key: Option<(u64, u64, u64, std::path::PathBuf)>,
        info: Value,
        offline: bool,
        checked: Option<std::time::Instant>,
    }
    let key = cx.doc.current().map(|p| (cx.document_generation, p.id, cx.doc.folder_inputs().0, p.path.clone()));
    if cx.state::<Details>().key != key {
        let info = key.as_ref().map(|(_, id, _, _)| cx.doc.photo_info(*id)).unwrap_or(Value::Null);
        *cx.state::<Details>() = Details { key: key.clone(), info, ..Default::default() };
    }
    let mut usable = usable(cx);
    if usable
        && cx.state::<Details>().checked.is_none_or(|t| t.elapsed() >= crate::folders::REFRESH)
        && !cx.jobs.running("raw-availability")
    {
        if let Some((_, _, _, path)) = &key {
            let path = path.clone();
            cx.state::<Details>().checked = Some(std::time::Instant::now());
            cx.spawn_document_job(
                "raw-availability",
                "",
                move |_| Ok(!path.exists()),
                move |cx, result| {
                    if cx.state::<Details>().key == key {
                        cx.state::<Details>().offline = result.unwrap_or(false);
                    }
                },
            );
        }
    }
    let details = cx.state::<Details>();
    let offline = usable && details.offline;
    let info = &details.info;
    let preview =
        ["raw_native", "raw_dual_preview"].iter().any(|k| info.get(*k).is_some_and(grainy_core::settings::truthy));
    if offline && !preview {
        usable = false;
    }
    widgets::muted(ui, &raw_note(usable, offline, info));
    let has_profile = grainy_core::settings::truthy(cx.settings().value("dcp_profile"));
    let buttons = [
        ("DCP 파일 직접 불러오기", "raw.load_dcp", usable),
        ("DCP 프로파일 해제", "raw.clear_dcp", has_profile),
        ("RAW 가져오기 기본 보정", "raw.defaults", true),
    ];
    for (text, id, enabled) in buttons {
        if Button::new(&tr(text)).width(width).enabled(enabled).show(ui).clicked() {
            cx.run(id);
        }
    }
    for (key, text) in [
        ("dcp_huesat", "프로파일 색상 보정"),
        ("dcp_look", "프로파일 룩"),
        ("dcp_tone", "프로파일 톤 곡선"),
        ("dcp_exposure", "프로파일 기준 노출"),
    ] {
        ui.add_enabled_ui(usable && has_profile, |ui| cx.setting_checkbox(ui, key, text));
    }
}
