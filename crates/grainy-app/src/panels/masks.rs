//! The Masks tool's panel (마스크): port of luma/studio.py `build_masks`, `update_mask_ui`, `load_layers`,
//! `load_layer` and `mask_menu`. In order: 새 마스크 만들기 (the kinds with their keys), the mask list (masks with
//! their check boxes, the selected mask's components), 추가 / 빼기 / …, 반전 and 오버레이 표시 (O), the hint of the
//! tool in use, its options, the radial's 타원 회전, 완료, and the selected mask's adjustments (양, 톤, 색상, 효과,
//! 세부).
//!
//! Work package 9 draws its range and sky editors where `ranges` is called below (`panels/masks_ranges.rs`) and
//! the brush range and automatic mask options; work package 10 replaces the placeholder of the fold-out
//! 커브 · 색상 혼합 · 선택 색상 (`panels/masks_local.rs`) and enables 세부 편집….

use eframe::egui::{self, CornerRadius, Popup, Pos2, Rect, Sense, Ui, Vec2};
use grainy_i18n::shortcuts::MASKS as MASK_KEYS;
use serde_json::Value;

use super::controls::hint;
use super::Panel;
use crate::adjustments::{Place, Reset, Spec};
use crate::context::AppContext;
use crate::masks::{actions, layers, overlay, Mode, Operation, MASK_KINDS};
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button};

/// The Masks panel.
pub struct Masks;

impl Panel for Masks {
    fn key(&self) -> &'static str {
        "masks"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

/// What a click asked for, run after the drawing.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    Create(&'static str),
    Extend(&'static str, Operation),
    Select(isize, isize),
    Enable(isize, bool),
    Rename,
    DuplicateLayer,
    Invert(bool),
    ToggleInvert,
    MoveLayer(isize),
    RemoveLayer,
    EditComponent,
    DuplicateComponent,
    MoveComponent(isize),
    RemoveComponent,
    Overlay(bool),
    Done,
}

/// One adjustment slider: (key, label, low, high, scale).
pub type LocalSlider = (&'static str, &'static str, f64, f64, f64);

/// The adjustments of a mask: (section, sliders). Port of `build_masks`' list.
pub const LOCAL_SLIDERS: [(&str, &[LocalSlider]); 4] = [
    (
        "톤",
        &[
            ("exposure", "노출", -5.0, 5.0, 100.0),
            ("contrast", "대비", -100.0, 100.0, 1.0),
            ("highlights", "하이라이트", -100.0, 100.0, 1.0),
            ("shadows", "그림자", -100.0, 100.0, 1.0),
            ("whites", "흰색 계열", -100.0, 100.0, 1.0),
            ("blacks", "검정 계열", -100.0, 100.0, 1.0),
        ],
    ),
    (
        "색상",
        &[
            ("temperature", "색온도", -100.0, 100.0, 1.0),
            ("tint", "색조", -100.0, 100.0, 1.0),
            ("hue", "색상", -180.0, 180.0, 1.0),
            ("saturation", "채도", -100.0, 100.0, 1.0),
        ],
    ),
    (
        "효과",
        &[
            ("texture", "텍스처", -100.0, 100.0, 1.0),
            ("clarity", "부분 대비", -100.0, 100.0, 1.0),
            ("dehaze", "안개 제거", -100.0, 100.0, 1.0),
        ],
    ),
    (
        "세부",
        &[
            ("sharpen", "선명하게", 0.0, 100.0, 1.0),
            ("noise_luma", "노이즈", 0.0, 100.0, 1.0),
            ("noise_color", "색상 노이즈", 0.0, 100.0, 1.0),
        ],
    ),
];

const fn spec(key: &'static str, label: &'static str, low: f64, high: f64, scale: f64, reset: f64) -> Spec {
    Spec { key, label, low, high, scale, reset: Reset::Value(reset), place: Place::Panel("masks") }
}

/// A slider row of the panel: (new value, released).
fn row(ui: &mut Ui, spec: &Spec, value: f64, enabled: bool) -> (Option<f64>, bool) {
    let event = widgets::adjustment_row(ui, &format!("masks.{}", spec.key), &tr(spec.label), spec, value, enabled);
    (event.set, event.commit)
}

/// The hint of the tool in use (`update_mask_ui`'s `tool_hint`).
pub fn tool_hint(cx: &mut AppContext) -> String {
    let (mode, operation) = {
        let s = actions::state(cx);
        (s.mode, s.operation)
    };
    let title = match mode {
        Mode::ColorRangeAdd => Some("색상 범위"),
        Mode::None | Mode::EditComponent => None,
        other => MASK_KINDS.iter().find(|(k, _)| *k == Some(other.as_str())).map(|(_, t)| *t),
    };
    if let Some(title) = title {
        let layer = actions::current_layer(cx);
        let name = match &layer {
            Some(layer) => layers::layer_name(layer),
            None => tr("새 마스크"),
        };
        let how = match mode {
            Mode::Object => "사진 안에서 개체를 둘러싸 주세요.",
            Mode::Brush => "사진 위를 칠하세요.",
            Mode::Linear => "사진 위에서 드래그하세요.",
            Mode::Radial => "사진 위에서 드래그해 타원을 그리세요.",
            Mode::LumaRange => "사진에서 밝기를 고를 곳을 클릭하거나 드래그하세요.",
            _ => "사진에서 색을 고를 곳을 클릭하거나 드래그하세요.",
        };
        return tr_with("{0} · {1}: {2}", &[&name, &tr(operation.verb()), &tr(title)]) + "\n" + &tr(how);
    }
    if mode == Mode::EditComponent {
        return tr("핸들을 드래그해 위치와 범위를 바꾸세요.");
    }
    if actions::masks(cx).is_empty() {
        return tr("새 마스크 만들기에서 종류를 고른 뒤 사진 위에 그리세요.");
    }
    String::new()
}

/// The list of kinds (`kind_button`): with `keyed`, the key of each kind written beside it.
fn kinds_menu(ui: &mut Ui, keyed: bool, chosen: &mut Option<&'static str>) {
    for (kind, title) in MASK_KINDS {
        let Some(kind) = kind else {
            ui.separator();
            continue;
        };
        let key = MASK_KEYS.iter().find(|(k, _, _)| *k == kind).map(|(_, key, _)| hint(key)).filter(|_| keyed);
        let button = egui::Button::new(tr(title)).shortcut_text(key.unwrap_or_default());
        if ui.add(button).clicked() {
            *chosen = Some(kind);
            ui.close();
        }
    }
}

/// A menu item that asks for `a`.
fn item(ui: &mut Ui, text: &str, a: Action, action: &mut Option<Action>) {
    if ui.button(tr(text)).clicked() {
        *action = Some(a);
        ui.close();
    }
}

/// The "…" menu of the selected mask and component (`mask_menu`).
fn mask_menu(
    ui: &mut Ui,
    has_layer: bool,
    shown: bool,
    has_component: bool,
    action: &mut Option<Action>,
    layer: isize,
) {
    if !has_layer {
        ui.add_enabled(false, egui::Button::new(tr("먼저 마스크를 만드세요.")));
        return;
    }
    item(ui, "이름 바꾸기…", Action::Rename, action);
    item(ui, "마스크 복제", Action::DuplicateLayer, action);
    item(ui, if shown { "마스크 숨기기" } else { "마스크 표시" }, Action::Enable(layer, !shown), action);
    item(ui, "마스크 반전", Action::ToggleInvert, action);
    let mut intersect = None;
    ui.menu_button(tr("다음과 마스크 교차"), |ui| kinds_menu(ui, false, &mut intersect));
    if let Some(kind) = intersect {
        *action = Some(Action::Extend(kind, Operation::Intersect));
    }
    item(ui, "위로 이동", Action::MoveLayer(-1), action);
    item(ui, "아래로 이동", Action::MoveLayer(1), action);
    item(ui, "마스크 삭제", Action::RemoveLayer, action);
    if has_component {
        ui.separator();
        ui.add_enabled(false, egui::Button::new(tr("선택한 구성 요소")));
        item(ui, "세부 편집…", Action::EditComponent, action);
        item(ui, "구성 요소 복제", Action::DuplicateComponent, action);
        item(ui, "구성 요소 위로", Action::MoveComponent(-1), action);
        item(ui, "구성 요소 아래로", Action::MoveComponent(1), action);
        item(ui, "구성 요소 삭제", Action::RemoveComponent, action);
    }
}

/// One row of the mask list: indent, an optional check box, the text; selected rows are highlighted.
fn tree_row(ui: &mut Ui, indent: f32, check: Option<bool>, text: &str, selected: bool) -> (egui::Response, bool) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, t::CHECKED);
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, t::BUTTON_HOVER);
    }
    let mut x = rect.left() + 6.0 + indent;
    let mut toggled = false;
    if let Some(checked) = check {
        let indicator = Rect::from_center_size(Pos2::new(x + 6.5, rect.center().y), Vec2::splat(13.0));
        let hit = ui.interact(indicator.expand(2.0), response.id.with("check"), Sense::click());
        toggled = hit.clicked();
        let painter = ui.painter();
        painter.rect(
            indicator,
            CornerRadius::ZERO,
            t::INPUT_BG,
            egui::Stroke::new(1.0, t::INPUT_BORDER),
            egui::StrokeKind::Inside,
        );
        if checked {
            let c = indicator.center();
            let stroke = egui::Stroke::new(1.6, t::TEXT);
            painter.line_segment([c + Vec2::new(-3.5, 0.0), c + Vec2::new(-1.0, 2.8)], stroke);
            painter.line_segment([c + Vec2::new(-1.0, 2.8), c + Vec2::new(3.8, -3.0)], stroke);
        }
        x += 20.0;
    }
    let color = if selected { t::CHECKED_TEXT } else { t::TEXT };
    let room = rect.right() - x - 4.0;
    let full = widgets::galley(ui, text, t::font(t::FONT_SIZE), color);
    let galley =
        if full.size().x <= room { full } else { widgets::elided(ui, text, t::font(t::FONT_SIZE), color, room) };
    ui.painter().galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, color);
    (response, toggled)
}

/// The mask list (`mask_tree`): the masks, the selected one expanded with its components.
fn tree(ui: &mut Ui, cx: &mut AppContext, action: &mut Option<Action>) {
    let list = actions::masks(cx);
    let (layer, component) = {
        let s = actions::state(cx);
        (s.layer, s.component)
    };
    if list.is_empty() {
        return;
    }
    let shown = layers::at(&list, layer).is_some_and(|l| layers::truthy(l.get("enabled"), true));
    let has_component = layers::component_at(&list, layer, component).is_some();
    let rows = list.len() + layers::at(&list, layer).map_or(0, |l| layers::components(l).len());
    let height = (10.0 + 34.0 * rows.max(1) as f32).min(230.0);
    egui::Frame::new().fill(t::INPUT_BG).stroke(egui::Stroke::new(1.0, t::INPUT_BORDER)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("mask_tree").max_height(height).auto_shrink([false, true]).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for (i, value) in list.iter().enumerate() {
                    let i = i as isize;
                    let empty = serde_json::Map::new();
                    let object = value.as_object().unwrap_or(&empty);
                    let enabled = layers::truthy(object.get("enabled"), true);
                    let selected = i == layer && component < 0;
                    let (response, toggled) = tree_row(ui, 0.0, Some(enabled), &layers::layer_name(object), selected);
                    if toggled {
                        *action = Some(Action::Enable(i, !enabled));
                    } else if response.double_clicked() {
                        *action = Some(Action::Rename);
                    } else if response.clicked() {
                        *action = Some(Action::Select(i, -1));
                    }
                    // A right click selects the row (Qt makes it current) and opens the mask's menu.
                    if response.secondary_clicked() && i != layer {
                        *action = Some(Action::Select(i, -1));
                    }
                    if i == layer {
                        Popup::context_menu(&response).show(|ui| {
                            mask_menu(ui, true, shown, has_component, action, layer);
                        });
                    }
                    if i != layer {
                        continue;
                    }
                    for (j, name) in layers::component_names(object).iter().enumerate() {
                        let j = j as isize;
                        let (response, _) = tree_row(ui, 14.0, None, name, j == component);
                        if response.double_clicked() {
                            *action = Some(Action::EditComponent);
                        } else if response.clicked() {
                            *action = Some(Action::Select(i, j));
                        }
                        Popup::context_menu(&response).show(|ui| {
                            mask_menu(ui, true, shown, j == component, action, layer);
                        });
                    }
                }
            },
        );
    });
}

/// The rename box (`QInputDialog.getText(tr('마스크 이름'), tr('이름'))`).
fn rename_box(ui: &mut Ui, cx: &mut AppContext) {
    let Some(mut text) = actions::state(cx).rename.clone() else { return };
    let mut answer = None;
    let response =
        egui::Modal::new(egui::Id::unique("mask_rename")).frame(crate::dialogs::frame()).show(ui.ctx(), |ui| {
            ui.set_width(320.0);
            ui.label(egui::RichText::new(tr("마스크 이름")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            widgets::label(ui, &tr("이름"), t::TEXT, t::FONT_SIZE);
            let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                answer = Some(true);
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, &tr("취소")).clicked() {
                    answer = Some(false);
                }
                ui.add_space(6.0);
                if Button::new(&tr("확인")).kind(widgets::Kind::Primary).show(ui).clicked() {
                    answer = Some(true);
                }
            });
        });
    if response.should_close() && answer.is_none() {
        answer = Some(false);
    }
    match answer {
        Some(true) => {
            actions::state(cx).rename = None;
            actions::rename_layer(cx, &text);
            actions::state(cx).rename = None;
        }
        Some(false) => actions::state(cx).rename = None,
        None => actions::state(cx).rename = Some(text),
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    // Leaving the tool ends its mode (`tab_changed` → `set_tool('')`): noticed when the panel shows again.
    let pass = cx.egui.cumulative_pass_nr();
    let gap = actions::state(cx).seen + 1 < pass;
    actions::state(cx).seen = pass;
    actions::sync(cx);
    if gap && actions::state(cx).mode != Mode::None {
        actions::set_tool(cx, Mode::None);
    }
    overlay::refresh(cx);
    let width = ui.available_width();
    let photo = cx.doc.current().is_some();
    let mut action: Option<Action> = None;

    // 새 마스크 만들기
    let tip = {
        let keys: Vec<String> = MASK_KEYS.iter().map(|(_, key, _)| hint(key)).collect();
        tr_with("새 마스크 만들기 · 브러시 {0} · 선형 {1} · 방사형 {2}", &[&keys[0], &keys[1], &keys[2]])
    };
    let create = Button::new(&tr("새 마스크 만들기")).enabled(photo).width(width).show(ui).on_hover_text(tip);
    let mut kind = None;
    Popup::menu(&create).show(|ui| kinds_menu(ui, true, &mut kind));
    if let Some(kind) = kind {
        action = Some(Action::Create(kind));
    }
    ui.add_space(4.0);

    tree(ui, cx, &mut action);
    ui.add_space(4.0);

    // 추가 / 빼기 / …
    let layer = actions::current_layer(cx);
    let has = layer.is_some();
    let component = actions::current_component(cx);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let each = ((width - 44.0 - 8.0) / 2.0).floor();
        for (text, operation) in [("추가", Operation::Add), ("빼기", Operation::Subtract)] {
            let response = Button::new(&tr(text)).enabled(has).width(each).show(ui);
            let mut kind = None;
            Popup::menu(&response).show(|ui| kinds_menu(ui, false, &mut kind));
            if let Some(kind) = kind {
                action = Some(Action::Extend(kind, operation));
            }
        }
        let more =
            Button::new("…").enabled(has).width(44.0).show(ui).on_hover_text(tr("이름 바꾸기, 복제, 교차, 순서, 삭제"));
        let shown = layer.as_ref().is_some_and(|l| layers::truthy(l.get("enabled"), true));
        let index = actions::state(cx).layer;
        Popup::menu(&more).show(|ui| mask_menu(ui, has, shown, component.is_some(), &mut action, index));
    });

    // 반전, 오버레이 표시 (O)
    ui.horizontal(|ui| {
        let mut invert = layer.as_ref().is_some_and(|l| layers::truthy(l.get("invert"), false));
        ui.add_enabled_ui(has, |ui| {
            if widgets::checkbox(ui, &mut invert, &tr("반전")).changed() {
                action = Some(Action::Invert(invert));
            }
        });
        ui.add_space(12.0);
        let mut shown = actions::state(cx).overlay_shown();
        if widgets::checkbox(ui, &mut shown, &tr("오버레이 표시 (O)")).changed() {
            action = Some(Action::Overlay(shown));
        }
    });

    let hint_text = tool_hint(cx);
    if !hint_text.is_empty() {
        widgets::muted(ui, &hint_text);
    }

    // 도구 옵션: the brush's four (and the radial's feather).
    let mode = actions::state(cx).mode;
    if matches!(mode, Mode::Brush | Mode::Radial) || mode.picking() {
        widgets::section(ui, &tr("도구 옵션"));
        let options = actions::state(cx).options.clone();
        let brush = mode == Mode::Brush;
        let sliders: Vec<(Spec, i64)> = [
            (spec("radius", "크기", 0.002, 0.25, 1000.0, 0.025), options.radius, brush),
            (spec("feather", "페더", 0.0, 100.0, 1.0, 70.0), options.feather, brush || mode == Mode::Radial),
            (spec("flow", "플로우", 1.0, 100.0, 1.0, 100.0), options.flow, brush),
            (spec("density", "밀도", 1.0, 100.0, 1.0, 100.0), options.density, brush),
        ]
        .into_iter()
        .filter(|(_, _, show)| *show)
        .map(|(s, v, _)| (s, v))
        .collect();
        for (spec, position) in sliders {
            let (set, _) = row(ui, &spec, position as f64 / spec.scale, true);
            if let Some(value) = set {
                let position = (value * spec.scale).round() as i64;
                let o = &mut actions::state(cx).options;
                match spec.key {
                    "radius" => o.radius = position,
                    "feather" => o.feather = position,
                    "flow" => o.flow = position,
                    _ => o.density = position,
                }
            }
        }
        super::masks_ranges::brush_options(ui, cx, mode);
        if brush {
            widgets::muted(ui, &tr("Alt: 지우기 · [ / ]: 크기 · Esc: 끝내기"));
        }
        super::masks_ranges::pick_options(ui, cx, mode);
    }

    // The range editor (범위 다듬기) and the sky editor (하늘 다듬기).
    super::masks_ranges::editors(ui, cx);

    // 타원 회전 of the selected radial.
    if let Some(angle) = actions::radial_angle(cx) {
        let angle_spec = spec("angle", "타원 회전", -180.0, 180.0, 1.0, 0.0);
        let (set, commit) = row(ui, &angle_spec, angle.round(), true);
        if let Some(value) = set {
            actions::radial_turned(cx, value);
        }
        if commit {
            cx.doc.edit_mut().commit("보정 변경");
        }
        widgets::muted(ui, &tr("방사형 그레이디언트는 사진 밖에서 시작하거나 사진 밖까지 그릴 수 있습니다."));
    }

    if mode.drawing() && Button::new(&tr("완료")).width(width).show(ui).clicked() {
        action = Some(Action::Done);
    }

    // The selected mask's adjustments.
    if let Some(layer) = &layer {
        let opacity = layer.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
        let (set, commit) = row(ui, &spec("opacity", "양", 0.0, 100.0, 1.0, 0.0), opacity * 100.0, true);
        if let Some(value) = set {
            actions::pause_overlay(cx);
            actions::layer_property(cx, "opacity", Value::from(value / 100.0));
        }
        if commit {
            cx.doc.edit_mut().commit("보정 변경");
        }
        let adjustments = layer.get("adjustments").and_then(Value::as_object).cloned().unwrap_or_default();
        for (title, sliders) in LOCAL_SLIDERS {
            widgets::section(ui, &tr(title));
            for (key, label, low, high, scale) in sliders {
                let value = adjustments.get(*key).and_then(Value::as_f64).unwrap_or(0.0);
                let (set, commit) = row(ui, &spec(key, label, *low, *high, *scale, 0.0), value, true);
                if let Some(value) = set {
                    actions::local_changed(cx, key, value);
                }
                if commit {
                    cx.doc.edit_mut().commit("보정 변경");
                }
            }
        }
        let mut open = cx.foldouts.contains("mask_local");
        widgets::foldout(ui, &tr("커브 · 색상 혼합 · 선택 색상"), &mut open);
        cx.set_foldout("mask_local", open);
        if open {
            super::masks_local::body(ui, cx);
        } else {
            super::masks_local::refresh(cx);
        }
    }

    rename_box(ui, cx);
    if let Some(action) = action {
        run(cx, action);
    }
}

/// Runs what a click asked for (the oracle's slots).
fn run(cx: &mut AppContext, action: Action) {
    match action {
        Action::Create(kind) => actions::create_mask(cx, kind),
        Action::Extend(kind, operation) => actions::extend_mask(cx, kind, operation),
        Action::Select(layer, component) => actions::tree_selected(cx, layer, component),
        Action::Enable(index, enabled) => actions::set_layer_enabled(cx, index, enabled),
        Action::Rename => {
            if let Some(layer) = actions::current_layer(cx) {
                actions::state(cx).rename = Some(layers::layer_name(&layer));
            }
        }
        Action::DuplicateLayer => actions::duplicate_layer(cx),
        Action::Invert(on) => {
            actions::layer_property(cx, "invert", Value::Bool(on));
            cx.doc.edit_mut().commit("보정 변경");
        }
        Action::ToggleInvert => {
            // 마스크 반전 toggles the 반전 check box.
            if let Some(layer) = actions::current_layer(cx) {
                let on = !layers::truthy(layer.get("invert"), false);
                actions::layer_property(cx, "invert", Value::Bool(on));
                cx.doc.edit_mut().commit("보정 변경");
            }
        }
        Action::MoveLayer(delta) => actions::move_layer(cx, delta),
        Action::RemoveLayer => actions::remove_layer(cx),
        Action::EditComponent => actions::edit_component(cx),
        Action::DuplicateComponent => actions::duplicate_component(cx),
        Action::MoveComponent(delta) => actions::move_component(cx, delta),
        Action::RemoveComponent => actions::remove_component(cx),
        Action::Overlay(shown) => {
            let s = actions::state(cx);
            s.overlay_off = !shown;
            s.overlay_paused = false;
        }
        Action::Done => actions::mode(cx, Mode::None),
    }
}
