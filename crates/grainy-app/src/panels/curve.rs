//! The Tone curve panel (톤 곡선): the curve editor (luma/app.py: `ToneCurve` on `curve`, `set_hdr` from the HDR
//! mode), the targeted adjustment button (`TargetedAdjust.toggle('curve')`), the four region sliders
//! (`parametric`) and the 채널 커브 fold-out with the R/G/B curve (luma/studio.py `rgb_curve_changed`, the channel
//! choice loading `rgb_curves[i]`). The oracle's curve display shows only the points: the region sliders are
//! applied by the engine and are not drawn on it, and no histogram is drawn behind it.

use eframe::egui::Ui;
use grainy_i18n::tr;
use serde_json::Value;

use super::controls::{self, list_spec};
use super::Panel;
use crate::context::AppContext;
use crate::tools::targeted::{self, Kind};
use crate::widgets::curve::{self, points_of, points_value, CurveEditor, Signal};
use crate::widgets::{self, Button};

/// The curve panel.
pub struct Curve;

impl Panel for Curve {
    fn key(&self) -> &'static str {
        "curve"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

/// The two editors of the panel, kept between frames.
#[derive(Debug, Default)]
pub struct CurveEditors {
    /// The tone curve (`curve`).
    pub main: CurveEditor,
    /// The channel curve (`rgb_curves[channel]`).
    pub channel: CurveEditor,
}

/// Where a curve's signals go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `curve.changed` → `set_setting('curve', points)`.
    Tone,
    /// `rgb_curve_changed`: `rgb_curves[index] = points`.
    Channel(usize),
}

/// Applies a curve editor's signals in order: `changed` sets the value live, `committed` is `finish_interaction`'s
/// commit (one undo step labelled 보정 변경). Nothing is set without an open photo (`set_setting` returns).
pub fn apply(cx: &mut AppContext, target: Target, signals: Vec<Signal>) {
    if cx.doc.current().is_none() {
        return;
    }
    for signal in signals {
        match signal {
            Signal::Changed(points) => {
                let edit = cx.doc.edit_mut();
                match target {
                    Target::Tone => edit.set("curve", points_value(&points)),
                    Target::Channel(index) => {
                        let mut values = edit.settings().value("rgb_curves").clone();
                        if let Some(slot) = values.as_array_mut().and_then(|list| list.get_mut(index)) {
                            *slot = points_value(&points);
                            edit.set("rgb_curves", values);
                        }
                    }
                }
                cx.before = false;
            }
            Signal::Committed => {
                cx.doc.edit_mut().commit("보정 변경");
            }
        }
    }
}

/// The points of `rgb_curves[index]`.
fn channel_points(cx: &AppContext, index: usize) -> Vec<[f64; 2]> {
    points_of(cx.settings().value("rgb_curves").get(index).unwrap_or(&Value::Null))
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    targeted::sync(cx);
    let points = points_of(cx.settings().value("curve"));
    let hdr = cx.settings().bool("hdr");
    let signals = curve::show(ui, &mut cx.state::<CurveEditors>().main, &points, hdr);
    apply(cx, Target::Tone, signals);
    let on = targeted::checked(cx, Kind::Curve);
    let response = Button::new(&tr("사진에서 드래그로 조정"))
        .width(width)
        .checked(on)
        .show(ui)
        .on_hover_text(tr("사진의 한 곳을 누른 채 위아래로 끌면 그 밝기의 커브가 올라가거나 내려갑니다."));
    if response.clicked() {
        targeted::toggle(cx, Kind::Curve, !on);
    }
    widgets::muted(ui, &tr("구간별 조정 · 곡선의 네 구간을 각각 올리거나 내립니다."));
    let spec = list_spec("parametric", -100.0, 100.0);
    for (i, name) in ["암부 곡선", "어두운 영역 곡선", "밝은 영역 곡선", "명부 곡선"].iter().enumerate()
    {
        cx.list_slider(ui, &spec, name, i, None);
    }
    let mut open = cx.foldouts.contains("channel_curve");
    widgets::foldout(ui, &tr("채널 커브"), &mut open);
    cx.set_foldout("channel_curve", open);
    if open {
        widgets::muted(
            ui,
            &grainy_i18n::keys_here(&tr("색상 채널 커브 · Ctrl+클릭: 점 추가 / 우클릭: 점 삭제 / 더블클릭: 초기화")),
        );
        let channels = [tr("빨강 R"), tr("초록 G"), tr("파랑 B")];
        let current = *cx.choices.get("channel").unwrap_or(&0);
        if let Some(i) = controls::choice(ui, "channel", "", &channels, current, width) {
            cx.choices.insert("channel", i);
        }
        let index = *cx.choices.get("channel").unwrap_or(&0);
        let points = channel_points(cx, index);
        // The channel curve is never on the HDR axis (only the tone curve gets `set_hdr`).
        let signals = curve::show(ui, &mut cx.state::<CurveEditors>().channel, &points, false);
        apply(cx, Target::Channel(index), signals);
    }
}
