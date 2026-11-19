//! 선택 영역 편집 (세부 편집… of a mask component): port of luma/mask_dialog.py `MaskComponentDialog` — the
//! component's kind, 선택 영역 사용, 이 선택 영역 반전, 결합, and the number fields of its kind (brush: 크기, 칠하는
//! 강도, 최대 농도, 시작점과 비슷한 색 안에서만, 색 경계 허용량; brush and radial: 경계 흐림; luminance range: 최소 /
//! 최대 밝기, 범위 경계 흐림; colour range: 색상 범위 허용량; sky: 민감도, 경계 흐림; gradients: 시작 / 끝 X, Y (%);
//! radial: 타원 회전) — and of `result_component` (the component written back); opened by
//! `StudioTools.edit_component` ([`open`]), whose accepted result replaces the component and ends the tool.
//!
//! The number fields behave as the oracle's `QDoubleSpinBox`: a value is rounded to the field's decimals (Qt's
//! fixed-point text, halves away from zero) and then kept inside its range ([`spin_value`]).

use eframe::egui;
use serde_json::Value;

use super::Dialog;
use crate::context::AppContext;
use crate::masks::layers::{self, Obj};
use crate::masks::{actions, kind_name, Mode, SKY_DEFAULTS};
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The 결합 box: (Korean title, stored value), in the oracle's order.
pub const OPERATIONS: [(&str, &str); 3] = [("더하기", "add"), ("빼기", "subtract"), ("교차", "intersect")];

/// The gradients' points when a component has none (`component.get('points', [[.25, .25], [.75, .75]])`).
const DEFAULT_POINTS: [[f64; 2]; 2] = [[0.25, 0.25], [0.75, 0.75]];

/// Port of `QDoubleSpinBoxPrivate::round`: `value` as Qt's fixed-point text with `decimals` digits, read back.
/// Qt's text rounds an exact half away from zero (`2.125` → `2.13`); other values to the nearest.
pub fn qt_round(value: f64, decimals: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    // The exact decimal expansion far enough to see whether the value is exactly half way.
    let long = format!("{:.*}", decimals + 30, value.abs());
    let (whole, fraction) = long.split_once('.').unwrap_or((long.as_str(), ""));
    let rest = &fraction[decimals.min(fraction.len())..];
    let half = rest.starts_with('5') && rest[1..].bytes().all(|b| b == b'0');
    let magnitude = if half {
        // Increment the kept digits by one unit in the last place.
        let mut digits: Vec<u8> = whole.bytes().chain(fraction[..decimals].bytes()).collect();
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
        let split = digits.len() - decimals;
        let text =
            format!("{}.{}", String::from_utf8_lossy(&digits[..split]), String::from_utf8_lossy(&digits[split..]));
        text.parse::<f64>().unwrap_or(value.abs())
    } else {
        format!("{:.*}", decimals, value.abs()).parse::<f64>().unwrap_or(value.abs())
    };
    if value < 0.0 {
        -magnitude
    } else {
        magnitude
    }
}

/// `QDoubleSpinBox.setValue`: rounded to the decimals, then kept inside the (rounded) range.
pub fn spin_value(value: f64, low: f64, high: f64, decimals: usize) -> f64 {
    let rounded = qt_round(value, decimals);
    let (low, high) = (qt_round(low, decimals), qt_round(high, decimals));
    if rounded < low {
        low
    } else if rounded > high {
        high
    } else {
        rounded
    }
}

/// A number field (`number(key, title, value, low, high, decimals)`).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    /// The Korean label (source text for `tr`).
    pub title: String,
    /// The value as the field shows it.
    pub value: f64,
    pub low: f64,
    pub high: f64,
    pub decimals: usize,
}

/// A row of the form after the four common rows (종류, 선택 영역 사용, 이 선택 영역 반전, 결합).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The number field at this index of [`ComponentForm::fields`].
    Number(usize),
    /// 시작점과 비슷한 색 안에서만 (brushes).
    AutoMask,
}

/// The dialog's model: what the form shows and what the user changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentForm {
    /// The component the dialog was opened with (`self.component`).
    pub component: Obj,
    /// The Korean name of its kind (`NAMES.get(type, '선택 영역')`).
    pub kind_name: &'static str,
    /// 선택 영역 사용.
    pub enabled: bool,
    /// 이 선택 영역 반전.
    pub invert: bool,
    /// The index of 결합 in [`OPERATIONS`].
    pub operation: usize,
    /// 시작점과 비슷한 색 안에서만 (brushes only).
    pub auto_mask: Option<bool>,
    pub fields: Vec<Field>,
    pub rows: Vec<Row>,
}

/// A stored number (`component.get(key, default)`); a value that is not a number counts as the default.
fn number(component: &Obj, key: &str, default: f64) -> f64 {
    component.get(key).and_then(Value::as_f64).unwrap_or(default)
}

impl ComponentForm {
    /// Port of `MaskComponentDialog.__init__`: the form of `component`.
    pub fn new(component: &Obj) -> Self {
        let kind = component.get("type").and_then(Value::as_str).unwrap_or("");
        let operation = component.get("operation").and_then(Value::as_str).unwrap_or("add");
        let mut form = Self {
            component: component.clone(),
            kind_name: kind_name(kind),
            enabled: layers::truthy(component.get("enabled"), true),
            invert: layers::truthy(component.get("invert"), false),
            operation: OPERATIONS.iter().position(|(_, key)| *key == operation).unwrap_or(0),
            auto_mask: None,
            fields: Vec::new(),
            rows: Vec::new(),
        };
        let add = |form: &mut Self, key: &str, title: &str, value: f64, low: f64, high: f64, decimals: usize| {
            form.rows.push(Row::Number(form.fields.len()));
            form.fields.push(Field {
                key: key.to_owned(),
                title: title.to_owned(),
                value: spin_value(value, low, high, decimals),
                low: qt_round(low, decimals),
                high: qt_round(high, decimals),
                decimals,
            });
        };
        if kind == "brush" {
            add(&mut form, "radius", "크기", number(component, "radius", 0.04) * 100.0, 0.2, 25.0, 2);
            add(&mut form, "flow", "칠하는 강도", number(component, "flow", 100.0), 0.0, 100.0, 1);
            add(&mut form, "density", "최대 농도", number(component, "density", 100.0), 0.0, 100.0, 1);
            form.auto_mask = Some(layers::truthy(component.get("auto_mask"), false));
            form.rows.push(Row::AutoMask);
            let tolerance = number(component, "auto_tolerance", 0.18);
            add(&mut form, "auto_tolerance", "색 경계 허용량", tolerance, 0.02, 1.0, 2);
        }
        if kind == "brush" || kind == "radial" {
            add(&mut form, "feather", "경계 흐림", number(component, "feather", 70.0), 0.0, 100.0, 1);
        }
        if kind == "luma" {
            let range = component.get("range").and_then(Value::as_array);
            let at = |i: usize, default: f64| range.and_then(|r| r.get(i)).and_then(Value::as_f64).unwrap_or(default);
            let (low, high) = match range {
                Some(r) if r.len() == 2 => (at(0, 0.25), at(1, 0.75)),
                _ => (0.25, 0.75),
            };
            add(&mut form, "low", "최소 밝기", low * 100.0, 0.0, 100.0, 1);
            add(&mut form, "high", "최대 밝기", high * 100.0, 0.0, 100.0, 1);
            add(&mut form, "softness", "범위 경계 흐림", number(component, "softness", 0.1) * 100.0, 0.1, 100.0, 1);
        }
        if kind == "color" {
            add(&mut form, "tolerance", "색상 범위 허용량", number(component, "tolerance", 0.3), 0.02, 1.7, 2);
        }
        if kind == "sky" {
            for (key, title) in [("sensitivity", "민감도"), ("feather", "경계 흐림")] {
                let default = SKY_DEFAULTS.iter().find(|(k, _)| *k == key).map_or(0.0, |(_, v)| *v);
                add(&mut form, key, title, number(component, key, default), 0.0, 100.0, 1);
            }
        }
        if kind == "linear" || kind == "radial" {
            let points = gradient_points(component);
            for (i, point) in [points.first().copied(), points.last().copied()].into_iter().enumerate() {
                let point = point.unwrap_or(DEFAULT_POINTS[i]);
                for (axis, value) in [("x", point[0]), ("y", point[1])] {
                    let title = format!("{} {} (%)", if i == 0 { "시작" } else { "끝" }, axis.to_uppercase());
                    add(&mut form, &format!("{axis}{i}"), &title, value * 100.0, -200.0, 300.0, 1);
                }
            }
        }
        if kind == "radial" {
            add(&mut form, "angle", "타원 회전", number(component, "angle", 0.0), -180.0, 180.0, 1);
        }
        form
    }

    /// The value of field `key`, if the form has it.
    pub fn value(&self, key: &str) -> Option<f64> {
        self.fields.iter().find(|f| f.key == key).map(|f| f.value)
    }

    /// `field.setValue(value)`: rounded and kept in range as the spin box does.
    pub fn set(&mut self, key: &str, value: f64) {
        if let Some(field) = self.fields.iter_mut().find(|f| f.key == key) {
            field.value = spin_value(value, field.low, field.high, field.decimals);
        }
    }

    /// Sets 결합 by its stored value (`setCurrentIndex(findData(key))`); an unknown value leaves it as it is.
    pub fn set_operation(&mut self, key: &str) {
        if let Some(index) = OPERATIONS.iter().position(|(_, k)| *k == key) {
            self.operation = index;
        }
    }

    /// Port of `result_component`: the component with the form's values. Errors are the oracle's exceptions (a
    /// brush with 시작점과 비슷한 색 안에서만 needs a first point).
    pub fn result_component(&self) -> Result<Obj, String> {
        let mut result = self.component.clone();
        let value = |key: &str| self.value(key);
        result.insert("enabled".into(), Value::Bool(self.enabled));
        result.insert("invert".into(), Value::Bool(self.invert));
        result.insert("operation".into(), Value::from(OPERATIONS[self.operation].1));
        for key in ["feather", "flow", "density", "tolerance", "sensitivity"] {
            if let Some(v) = value(key) {
                result.insert(key.into(), Value::from(v));
            }
        }
        if let Some(radius) = value("radius") {
            result.insert("radius".into(), Value::from(radius / 100.0));
            if self.auto_mask == Some(true) {
                // `result.get('seed', result['points'][0])`: the first point is read even when a seed exists.
                let first = match result.get("points") {
                    None => return Err("KeyError: 'points'".to_owned()),
                    Some(Value::Array(points)) => {
                        points.first().cloned().ok_or_else(|| "IndexError: list index out of range".to_owned())?
                    }
                    Some(other) => return Err(format!("TypeError: points {other}")),
                };
                let tolerance = value("auto_tolerance").unwrap_or(0.18);
                result.insert("auto_mask".into(), Value::Bool(true));
                result.insert("auto_tolerance".into(), Value::from(tolerance));
                let seed = result.get("seed").cloned().unwrap_or(first);
                result.insert("seed".into(), seed);
            } else {
                result.remove("auto_mask");
            }
            // Preserve legacy stroke blending unless flow/density actually changed.
            let changed = ["flow", "density"].iter().any(|key| {
                let stored = self.component.get(*key);
                let stored = match stored {
                    None => Some(100.0),
                    Some(v) => v.as_f64(),
                };
                stored != value(key)
            });
            if changed {
                result.insert("paint_version".into(), Value::from(2));
            }
        }
        if let (Some(low), Some(high), Some(softness)) = (value("low"), value("high"), value("softness")) {
            let (a, b) = (low / 100.0, high / 100.0);
            let (a, b) = if b < a { (b, a) } else { (a, b) };
            result.insert("range".into(), Value::from(vec![a, b]));
            result.insert("softness".into(), Value::from(softness / 100.0));
        }
        if let (Some(x0), Some(y0), Some(x1), Some(y1)) = (value("x0"), value("y0"), value("x1"), value("y1")) {
            let points = vec![Value::from(vec![x0 / 100.0, y0 / 100.0]), Value::from(vec![x1 / 100.0, y1 / 100.0])];
            result.insert("points".into(), Value::Array(points));
        }
        if let Some(angle) = value("angle") {
            result.insert("angle".into(), Value::from(angle));
        }
        Ok(result)
    }
}

/// The points of a gradient as numbers (missing or empty: the dialog's default).
fn gradient_points(component: &Obj) -> Vec<[f64; 2]> {
    let points: Vec<[f64; 2]> = component
        .get("points")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|p| {
                    let at = |i: usize| p.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                    [at(0), at(1)]
                })
                .collect()
        })
        .unwrap_or_default();
    if points.is_empty() {
        DEFAULT_POINTS.to_vec()
    } else {
        points
    }
}

/// The dialog: the form of the selected component of the selected mask of one photo.
pub struct MaskComponentDialog {
    /// The form; public so tests can fill it.
    pub form: ComponentForm,
    /// (photo, layer, component) the form belongs to.
    target: (u64, isize, isize),
}

impl MaskComponentDialog {
    /// The dialog for the selected component (`None` when no component is selected).
    pub fn for_selection(cx: &mut AppContext) -> Option<Self> {
        let component = actions::current_component(cx)?;
        let (layer, index) = {
            let s = actions::state(cx);
            (s.layer, s.component)
        };
        Some(Self { form: ComponentForm::new(&component), target: (cx.doc.current_id(), layer, index) })
    }

    /// OK: the component is replaced by `result_component()` (one undo step) and the tool ends
    /// (`replace_component(...)`, `view.set_tool('')`). Nothing happens when the selection is no longer the
    /// dialog's or the result fails.
    pub fn accept(&self, cx: &mut AppContext) {
        let here = {
            let photo = cx.doc.current_id();
            let s = actions::state(cx);
            (photo, s.layer, s.component)
        };
        if here != self.target || actions::current_component(cx).is_none() {
            return;
        }
        match self.form.result_component() {
            Ok(component) => {
                actions::replace_component(cx, component);
                actions::set_tool(cx, Mode::None);
            }
            Err(error) => log::warn!("mask component: {error}"),
        }
    }
}

/// Port of `edit_component` (세부 편집…): the dialog for the selected component, if one is selected.
pub fn open(cx: &mut AppContext) {
    if let Some(dialog) = MaskComponentDialog::for_selection(cx) {
        cx.open_dialog(Box::new(dialog));
    }
}

/// The label column of a row.
fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(egui::vec2(130.0, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new(text).font(t::font(t::FONT_SIZE)).color(t::TEXT));
    });
}

fn number_row(ui: &mut egui::Ui, field: &mut Field) {
    row_label(ui, &tr(&field.title));
    let mut value = field.value;
    let speed = 10f64.powi(-(field.decimals as i32));
    let response = ui.add_sized(
        egui::vec2(160.0, 24.0),
        egui::DragValue::new(&mut value).range(field.low..=field.high).fixed_decimals(field.decimals).speed(speed),
    );
    if response.changed() {
        field.value = spin_value(value, field.low, field.high, field.decimals);
    }
    ui.end_row();
}

impl Dialog for MaskComponentDialog {
    fn id(&self) -> &'static str {
        "mask_component"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let form = &mut self.form;
        let response = egui::Modal::new(egui::Id::unique("mask_component")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(egui::RichText::new(tr("선택 영역 편집")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            egui::Grid::new("mask_component_form").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                row_label(ui, &tr("종류"));
                ui.label(egui::RichText::new(tr(form.kind_name)).font(t::font(t::FONT_SIZE)).color(t::TEXT));
                ui.end_row();
                ui.label("");
                widgets::checkbox(ui, &mut form.enabled, &tr("선택 영역 사용"));
                ui.end_row();
                ui.label("");
                widgets::checkbox(ui, &mut form.invert, &tr("이 선택 영역 반전"));
                ui.end_row();
                row_label(ui, &tr("결합"));
                let names: Vec<String> = OPERATIONS.iter().map(|(title, _)| tr(title)).collect();
                if let Some(chosen) = crate::panels::controls::choice(
                    ui,
                    "mask_component_operation",
                    &tr("결합"),
                    &names,
                    form.operation,
                    160.0,
                ) {
                    form.operation = chosen;
                }
                ui.end_row();
                for row in form.rows.clone() {
                    match row {
                        Row::Number(index) => number_row(ui, &mut form.fields[index]),
                        Row::AutoMask => {
                            ui.label("");
                            let mut on = form.auto_mask.unwrap_or(false);
                            if widgets::checkbox(ui, &mut on, &tr("시작점과 비슷한 색 안에서만")).changed()
                            {
                                form.auto_mask = Some(on);
                            }
                            ui.end_row();
                        }
                    }
                }
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, &tr("취소")).clicked() {
                    answer = Some(false);
                }
                ui.add_space(6.0);
                if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                    answer = Some(true);
                }
            });
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                self.accept(cx);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}
