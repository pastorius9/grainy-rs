//! The Transform panel (변형): automatic alignment from the photo's straight lines (자동 / 수평 / 수직 / 전체 원근,
//! background jobs), its crop option and status label, 자동 정렬 초기화, the manual transform sliders and guided
//! upright ([`crate::tools::guides`]).
//!
//! Port of luma/studio.py `StudioTools.build_tools`' transform box, `auto_upright` (the job: lens shading, geometry
//! and `upright.estimate` through `grainy_engine::auto::upright_for`; a result applied only to the photo and
//! settings it was measured on), `clear_upright` and the lines of `load_settings` that show the alignment
//! (`upright_label` and the crop panel's `auto_level_note`). The manual sliders are plain settings rows: the geometry
//! stage applies them on top of the stored alignment, as the oracle's does.
//!
//! The alignment's state is [`UprightState`] (`cx.state()`). The job's two halves, [`begin`] and [`finish`], work
//! without a window, so tests drive them directly with the oracle's results.

use eframe::egui::Ui;
use grainy_core::Settings;
use grainy_engine::files::load_photo;
use grainy_engine::{auto, Externals};
use serde_json::Value;

use super::Panel;
use crate::adjustments::Place;
use crate::context::AppContext;
use crate::state::settings_eq;
use crate::text::{tr, tr_with};
use crate::tools::{crop, guides};
use crate::widgets::{self, Button};

/// The alignment buttons: label and `upright.estimate` mode.
pub const MODES: [(&str, &str); 4] = [("자동", "auto"), ("수평", "level"), ("수직", "vertical"), ("전체 원근", "full")];
/// The job kind of the alignment.
pub const JOB: &str = "upright";
/// The develop source the oracle's window measures (`load_image(path, 1800, ...)`).
const SOURCE_SIZE: usize = 1800;

/// The transform panel.
pub struct Transform;

impl Panel for Transform {
    fn key(&self) -> &'static str {
        "transform"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    widgets::muted(ui, &tr("직선 기준 자동 정렬"));
    let idle = buttons_enabled(cx);
    let labels = MODES.map(|(label, _)| tr(label));
    let row: Vec<Button<'_>> = labels.iter().map(|l| Button::new(l).enabled(idle)).collect();
    if let Some(i) = widgets::button_row(ui, &row, width, 4.0) {
        auto_upright(cx, MODES[i].1);
    }
    cx.setting_checkbox(ui, "upright_crop", "자동 원근의 빈 가장자리 맞춤");
    widgets::muted(ui, &label(cx));
    if Button::new(&tr("자동 정렬 초기화")).width(width).show(ui).clicked() {
        clear_upright(cx);
    }
    cx.sliders(ui, Place::Panel("transform"));
    if Button::new(&tr("가이드 원근 · 네 모서리 선택")).checked(guides::is_on(cx)).width(width).show(ui).clicked()
    {
        guides::start(cx);
    }
    widgets::muted(ui, &tr("사각형의 좌상 → 우상 → 우하 → 좌하 모서리를 클릭합니다."));
    if Button::new(&tr("가이드 원근 초기화")).width(width).show(ui).clicked() {
        guides::clear(cx);
    }
}

// ---- the alignment ------------------------------------------------------------------------------------------

/// The alignment's state between frames (`upright_generation`, the buttons, `upright_label` when it shows a
/// message instead of the stored alignment).
#[derive(Debug, Clone, Default)]
pub struct UprightState {
    /// `upright_generation`: a newer analysis makes older results stale.
    pub generation: u64,
    /// The buttons are disabled while an analysis runs (`setEnabled(False)` until a job ends).
    pub running: bool,
    /// The label's message and the photo it was shown for (the analysis note, a failure); `load_controls`
    /// replaces it with the stored alignment.
    note: Option<(u64, String)>,
}

/// One analysis: what [`finish`] checks before applying the result.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub generation: u64,
    pub photo: u64,
    pub mode: &'static str,
    /// The settings when the button was pressed (`saved`).
    pub saved: Settings,
}

/// Whether the alignment buttons (and the crop panel's 자동 수평 맞추기) take clicks.
pub fn buttons_enabled(cx: &AppContext) -> bool {
    !cx.state_ref::<UprightState>().is_some_and(|s| s.running) && !cx.jobs.running("auto_level")
}

/// Python's `f'{v:+.2f}'`.
fn signed(v: f64) -> String {
    format!("{v:+.2}")
}

/// `load_settings`' `upright_label`: the stored alignment (mode, rotation, edge zoom) or 미적용.
pub fn stored_label(settings: &Settings) -> String {
    let upright = settings.value("upright");
    if !grainy_core::settings::truthy(upright) {
        return tr("자동 정렬 미적용");
    }
    let mode = match upright.get("mode").and_then(Value::as_str) {
        Some("level") => "수평",
        Some("vertical") => "수직",
        Some("full") => "전체 원근",
        _ => "자동",
    };
    let rotation = upright.get("rotation").and_then(Value::as_f64).unwrap_or(0.0);
    let scale = upright.get("crop_scale").and_then(Value::as_f64).unwrap_or(1.0);
    tr_with("{0} 적용 · 회전 {1}° · 가장자리 확대 {2}배", &[&tr(mode), &signed(rotation), &format!("{scale:.2}")])
}

/// `load_settings`' `auto_level_note` (the crop panel's note): `None` is its default text.
pub fn stored_level_note(settings: &Settings) -> Option<String> {
    let upright = settings.value("upright");
    grainy_core::settings::truthy(upright).then(|| {
        let rotation = upright.get("rotation").and_then(Value::as_f64).unwrap_or(0.0);
        tr_with("자동 보정 {0}° · 아래 슬라이더로 미세 조절할 수 있습니다.", &[&signed(rotation)])
    })
}

/// The status label under the buttons.
pub fn label(cx: &AppContext) -> String {
    let photo = cx.doc.current_id();
    match cx.state_ref::<UprightState>().and_then(|s| s.note.as_ref()) {
        Some((id, text)) if *id == photo => text.clone(),
        _ => stored_label(cx.settings()),
    }
}

/// `load_controls`' part: both labels show the stored alignment.
fn load_controls(cx: &mut AppContext) {
    cx.state::<UprightState>().note = None;
    let note = stored_level_note(cx.settings());
    crop::set_level_note(cx, note);
}

/// Both labels show `text`.
fn show(cx: &mut AppContext, text: String) {
    let photo = cx.doc.current_id();
    cx.state::<UprightState>().note = Some((photo, text.clone()));
    crop::set_level_note(cx, Some(text));
}

/// The first half of `auto_upright`: a newer generation, the buttons disabled, the analysis notes. `None` without
/// a photo.
pub fn begin(cx: &mut AppContext, mode: &'static str) -> Option<Ticket> {
    let photo = cx.doc.current()?.id;
    let saved = cx.settings().clone();
    let state = cx.state::<UprightState>();
    state.generation += 1;
    state.running = true;
    let generation = state.generation;
    let note = tr("사진의 직선과 소실점을 분석하고 있습니다…");
    cx.state::<UprightState>().note = Some((photo, note));
    crop::set_level_note(cx, Some(tr("사진의 기준선을 분석하고 있습니다…")));
    Some(Ticket { generation, photo, mode, saved })
}

/// The last line of an error without its exception name (`str(error).strip().splitlines()[-1].split(': ',1)[-1]`).
pub fn short(message: &str) -> String {
    let last = message.trim().lines().last().unwrap_or("");
    last.split_once(": ").map_or(last, |(_, rest)| rest).to_owned()
}

/// The undo label of an alignment.
pub fn commit_label(mode: &str) -> &'static str {
    if mode == "level" {
        "자동 수평 맞추기"
    } else {
        "자동 수평·원근"
    }
}

/// The second half of `auto_upright` (`finished` / `failed`): the estimate (`Estimate::to_json`) stored as
/// `upright` with the angle slider at 0, one undo step; or the error's message in both labels. Nothing is applied
/// when another analysis started, another photo opened or the settings changed meanwhile.
pub fn finish(cx: &mut AppContext, ticket: &Ticket, result: Result<Value, String>) {
    let state = cx.state::<UprightState>();
    state.running = false;
    let fresh = ticket.generation == state.generation
        && cx.doc.current().is_some_and(|p| p.id == ticket.photo)
        && settings_eq(cx.settings(), &ticket.saved);
    match result {
        Ok(_) if !fresh => {
            load_controls(cx);
            cx.set_status(tr("분석 중 사진이나 설정이 바뀌어 결과를 적용하지 않았습니다."));
        }
        Ok(estimate) => {
            let edit = cx.doc.edit_mut();
            edit.commit("보정 변경");
            edit.set("straighten", 0);
            edit.set("upright", estimate);
            edit.commit(commit_label(ticket.mode));
            cx.before = false;
            load_controls(cx);
        }
        Err(_) if !fresh => {}
        Err(message) => {
            load_controls(cx);
            show(cx, tr(&short(&message)));
        }
    }
}

/// Port of `auto_upright` (자동 / 수평 / 수직 / 전체 원근): the analysis as a background job.
pub fn auto_upright(cx: &mut AppContext, mode: &'static str) {
    let Some(path) = cx.doc.current().map(|p| p.path.clone()) else { return };
    let Some(ticket) = begin(cx, mode) else { return };
    let settings = ticket.saved.clone();
    cx.jobs.spawn(
        JOB,
        tr("사진의 직선과 소실점을 분석하고 있습니다…"),
        move |_| {
            let loaded = load_photo(&path, Some(SOURCE_SIZE), &settings).map_err(|e| e.to_string())?;
            auto::upright_for(loaded.photo.source(), &settings, mode, &Externals::default())
                .map(|estimate| estimate.to_json())
                .map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| finish(cx, &ticket, result),
    );
}

/// Port of `clear_upright` (자동 정렬 초기화).
pub fn clear_upright(cx: &mut AppContext) {
    if cx.doc.current().is_none() {
        return;
    }
    cx.doc.edit_mut().set("upright", Value::Null);
    cx.before = false;
    cx.doc.edit_mut().commit("보정 변경");
    load_controls(cx);
}
