//! The boxes of the collections and filters (luma/manager.py): the 상세 필터 / 스마트 컬렉션 규칙 dialog
//! (`FilterDialog`), the name box (`QInputDialog.getText`) and the choice box (`QInputDialog.getItem`) of the
//! collection commands.
//!
//! [`FilterForm`] is the dialog without a window: what it shows for given rules (`__init__`) and the rules it
//! answers (`rules`, with `validate`'s error texts: the oracle shows Python's own `ValueError` messages of `int()`
//! and `datetime.strptime`, kept here word for word).

use eframe::egui;
use serde_json::{Map, Value};

use super::import_errors::Notice;
use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The line edits of the dialog, in the oracle's order: (rule key, label).
pub const FIELDS: [(&str, &str); 14] = [
    ("name", "파일 이름"),
    ("keywords", "키워드 · 계층은 | 로 구분"),
    ("folder", "폴더 경로"),
    ("camera", "카메라"),
    ("lens", "렌즈"),
    ("extension", "파일 확장자"),
    ("rating_min", "최소 별점"),
    ("rating_max", "최대 별점"),
    ("flag", "선택 1 / 제외 -1 / 해제 0"),
    ("label", "색 라벨"),
    ("iso_min", "최소 ISO"),
    ("iso_max", "최대 ISO"),
    ("date_from", "시작 날짜 YYYY-MM-DD"),
    ("date_to", "종료 날짜 YYYY-MM-DD"),
];

/// The orientation choices: (label, value); the first is 전체 (no rule).
pub const ORIENTATIONS: [(&str, &str); 4] =
    [("전체", ""), ("가로", "landscape"), ("세로", "portrait"), ("정사각", "square")];

/// The rule keys that are integers once the dialog answers.
const INTEGERS: [&str; 5] = ["rating_min", "rating_max", "flag", "iso_min", "iso_max"];

/// The key of a stored Lightroom search (`smart_rules.KEY`).
const ADOBE: &str = grainy_adobe::smart::KEY;

// ---- Python's own texts -------------------------------------------------------------------------------------

/// Python `repr(str)`.
fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 32 || c as u32 == 127 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr(float)`.
fn repr_float(v: f64) -> String {
    let text = format!("{v:?}");
    match text.split_once('e') {
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', exponent),
            };
            format!("{mantissa}e{sign}{digits:0>2}")
        }
        None => text,
    }
}

/// Python `repr(value)` of a JSON value.
fn repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(v) => if *v { "True" } else { "False" }.into(),
        Value::Number(n) => match n.as_f64().filter(|_| n.is_f64()) {
            Some(f) => repr_float(f),
            None => n.to_string(),
        },
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => {
            format!(
                "{{{}}}",
                m.iter().map(|(k, v)| format!("{}: {}", repr_str(k), repr(v))).collect::<Vec<_>>().join(", ")
            )
        }
    }
}

/// Python `str(value)` of a JSON value (what `str(rules.get(key, ''))` puts in a field).
fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

/// Python `int(text)` for text that is already stripped.
fn py_int(text: &str) -> Result<Value, String> {
    let bad = || format!("invalid literal for int() with base 10: {}", repr_str(text));
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let well_formed = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.chars().all(|c| c == '_' || c.is_ascii_digit());
    if !well_formed {
        return Err(bad());
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let number: i64 = clean.parse().map_err(|_| bad())?;
    Ok(Value::from(if negative { -number } else { number }))
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `datetime.strptime(text, '%Y-%m-%d')`: `Ok` when it parses, else Python's `ValueError` text.
fn py_date(text: &str) -> Result<(), String> {
    let chars: Vec<char> = text.chars().collect();
    let digit = |i: usize| chars.get(i).is_some_and(|c| c.is_ascii_digit());
    let at = |i: usize| chars.get(i).copied();
    let no_match = || format!("time data {} does not match format '%Y-%m-%d'", repr_str(text));
    // %Y: four digits, '-'.
    if !(0..4).all(digit) || at(4) != Some('-') {
        return Err(no_match());
    }
    let year: i64 = chars[..4].iter().collect::<String>().parse().unwrap_or(0);
    // %m: `1[0-2]|0[1-9]|[1-9]`, '-', %d: `3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9]`, tried in order with backtracking.
    let two = |i: usize| -> Option<i64> {
        (digit(i) && digit(i + 1)).then(|| chars[i..i + 2].iter().collect::<String>().parse().unwrap_or(0))
    };
    let mut months: Vec<(i64, usize)> = Vec::new();
    if let Some(v @ 10..=12) = two(5) {
        months.push((v, 2));
    }
    if let Some(v @ 1..=9) = two(5) {
        months.push((v, 2));
    }
    if digit(5) && at(5) != Some('0') {
        months.push((i64::from(at(5).unwrap_or('0').to_digit(10).unwrap_or(0)), 1));
    }
    for (month, length) in months {
        let dash = 5 + length;
        if at(dash) != Some('-') {
            continue;
        }
        let start = dash + 1;
        let mut days: Vec<(i64, usize)> = Vec::new();
        if let Some(v @ 30..=31) = two(start) {
            days.push((v, 2));
        }
        if let Some(v @ 10..=29) = two(start) {
            days.push((v, 2));
        }
        if let Some(v @ 1..=9) = two(start) {
            days.push((v, 2));
        }
        if digit(start) && at(start) != Some('0') {
            days.push((i64::from(at(start).unwrap_or('0').to_digit(10).unwrap_or(0)), 1));
        }
        if at(start) == Some(' ') && digit(start + 1) && at(start + 1) != Some('0') {
            days.push((i64::from(at(start + 1).unwrap_or('0').to_digit(10).unwrap_or(0)), 2));
        }
        let Some((day, length)) = days.first().copied() else { continue };
        let end = start + length;
        if end != chars.len() {
            return Err(format!("unconverted data remains: {}", chars[end..].iter().collect::<String>()));
        }
        if year < 1 {
            return Err(format!("year {year} is out of range"));
        }
        if day > days_in_month(year, month) {
            return Err("day is out of range for month".into());
        }
        return Ok(());
    }
    Err(no_match())
}

// ---- the dialog without a window ---------------------------------------------------------------------------

/// What `FilterDialog` holds: its fields, check boxes, orientation and the rules it does not edit.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterForm {
    /// The line edits, in [`FIELDS`] order.
    pub fields: Vec<String>,
    /// 누락 원본만.
    pub missing: bool,
    /// 가상 사본만.
    pub virtual_only: bool,
    /// The orientation value (`""`, `landscape`, `portrait`, `square`).
    pub orientation: String,
    /// The rules of other kinds (complex searches), kept when `keep_complex`.
    pub extra: Map<String, Value>,
    /// 기존 복합 조건 유지 · 위 조건을 추가로 적용.
    pub keep_complex: bool,
}

impl FilterForm {
    /// Port of `FilterDialog.__init__` for `rules` (an object; anything else counts as no rules).
    pub fn new(rules: &Value) -> Self {
        let empty = Map::new();
        let rules = rules.as_object().unwrap_or(&empty);
        let fields = FIELDS.iter().map(|(key, _)| rules.get(*key).map(py_str).unwrap_or_default()).collect();
        let flag = |key: &str| rules.get(key).is_some_and(grainy_core::settings::truthy);
        let wanted = rules.get("orientation").map(py_str).unwrap_or_default();
        let orientation = ORIENTATIONS
            .iter()
            .find(|(_, value)| *value == wanted)
            .map_or(String::new(), |(_, value)| (*value).to_owned());
        let extra = rules
            .iter()
            .filter(|(key, _)| {
                !FIELDS.iter().any(|(k, _)| k == key) && !["missing", "virtual", "orientation"].contains(&key.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self { fields, missing: flag("missing"), virtual_only: flag("virtual"), orientation, extra, keep_complex: true }
    }

    /// The read-only text under 기존 복합 검색 조건: `smart_rules.describe` of a stored Lightroom search, else the
    /// rules as JSON. `None` when the dialog has no other rules; `Err` is the exception text that makes the
    /// oracle's dialog fail to open.
    pub fn details(&self) -> Result<Option<String>, String> {
        if self.extra.is_empty() {
            return Ok(None);
        }
        match self.extra.get(ADOBE) {
            Some(node) => grainy_adobe::smart::describe(node, 0).map(Some).map_err(|e| e.to_string()),
            None => Ok(Some(serde_json::to_string_pretty(&self.extra).unwrap_or_default())),
        }
    }

    /// Port of `FilterDialog.rules`: the rules the fields make, or the text of the box `validate` shows.
    pub fn rules(&self) -> Result<Value, String> {
        let mut rules: Map<String, Value> = if self.keep_complex { self.extra.clone() } else { Map::new() };
        for ((key, _), text) in FIELDS.iter().zip(&self.fields) {
            let text = text.trim();
            if !text.is_empty() {
                rules.insert((*key).to_owned(), Value::String(text.to_owned()));
            }
        }
        for key in INTEGERS {
            if let Some(Value::String(text)) = rules.get(key) {
                let number = py_int(text)?;
                rules.insert(key.to_owned(), number);
            }
        }
        for key in ["date_from", "date_to"] {
            if let Some(Value::String(text)) = rules.get(key) {
                py_date(text)?;
            }
        }
        if self.missing {
            rules.insert("missing".into(), Value::Bool(true));
        }
        if self.virtual_only {
            rules.insert("virtual".into(), Value::Bool(true));
        }
        if !self.orientation.is_empty() {
            rules.insert("orientation".into(), Value::String(self.orientation.clone()));
        }
        Ok(Value::Object(rules))
    }
}

// ---- the windows -------------------------------------------------------------------------------------------

type OnRules = Box<dyn FnOnce(&mut AppContext, Value)>;

/// The 상세 필터 / 스마트 컬렉션 규칙 dialog. 확인 runs `on_accept` with the rules; a field that does not parse
/// shows `QMessageBox.information(self, tr('필터'), str(e))` and the dialog stays.
pub struct FilterDialog {
    title: String,
    form: FilterForm,
    details: Option<String>,
    on_accept: Option<OnRules>,
    focused: bool,
}

impl FilterDialog {
    /// The dialog for `rules`; `title` is the Korean source (`상세 필터` or `스마트 컬렉션 규칙`). `Err` when the
    /// existing complex rules cannot be described (the oracle's dialog raises while it is built).
    pub fn new(
        title: &str,
        rules: &Value,
        on_accept: impl FnOnce(&mut AppContext, Value) + 'static,
    ) -> Result<Self, String> {
        let form = FilterForm::new(rules);
        let details = form.details()?;
        Ok(Self { title: tr(title), form, details, on_accept: Some(Box::new(on_accept)), focused: false })
    }

    /// The form (tests drive it).
    pub fn form_mut(&mut self) -> &mut FilterForm {
        &mut self.form
    }
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(t::bold(14.0)).color(t::BRAND));
    ui.add_space(6.0);
}

/// 확인 / 취소: `Some(true)` for the primary button.
fn buttons(ui: &mut egui::Ui) -> Option<bool> {
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if widgets::button(ui, &tr("취소")).clicked() {
            answer = Some(false);
        }
        ui.add_space(6.0);
        if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
            answer = Some(true);
        }
    });
    answer
}

impl Dialog for FilterDialog {
    fn id(&self) -> &'static str {
        "filter"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let room = (ctx.content_rect().height() - 150.0).clamp(160.0, 840.0);
        let response = egui::Modal::new(egui::Id::unique("filter")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(480.0);
            title(ui, &self.title);
            egui::ScrollArea::vertical().max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 2.0);
                for (index, (key, label)) in FIELDS.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(190.0, 28.0), egui::Sense::hover());
                        let text = tr(label);
                        widgets::paint_left(ui, rect, &text, t::font(t::FONT_SIZE), t::TEXT);
                        let field = widgets::line_edit(ui, &format!("filter_{key}"), &mut self.form.fields[index], "");
                        if !self.focused && index == 0 {
                            field.request_focus();
                            self.focused = true;
                        }
                    });
                }
                widgets::checkbox(ui, &mut self.form.missing, &tr("누락 원본만"));
                widgets::checkbox(ui, &mut self.form.virtual_only, &tr("가상 사본만"));
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(190.0, 28.0), egui::Sense::hover());
                    widgets::paint_left(ui, rect, &tr("방향"), t::font(t::FONT_SIZE), t::TEXT);
                    let shown = ORIENTATIONS
                        .iter()
                        .find(|(_, value)| *value == self.form.orientation)
                        .map_or(String::new(), |(label, _)| tr(label));
                    egui::ComboBox::from_id_salt("filter_orientation").selected_text(shown).show_ui(ui, |ui| {
                        for (label, value) in ORIENTATIONS {
                            ui.selectable_value(&mut self.form.orientation, value.to_owned(), tr(label));
                        }
                    });
                });
                if let Some(text) = &self.details {
                    widgets::checkbox(
                        ui,
                        &mut self.form.keep_complex,
                        &tr("기존 복합 조건 유지 · 위 조건을 추가로 적용"),
                    );
                    ui.label(
                        egui::RichText::new(tr("기존 복합 검색 조건")).font(t::font(t::FONT_SIZE)).color(t::MUTED),
                    );
                    let mut view: &str = text;
                    egui::ScrollArea::vertical().max_height(110.0).id_salt("filter_details").show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut view).desired_width(f32::INFINITY).text_color(t::TEXT));
                    });
                }
            });
            ui.add_space(10.0);
            answer = buttons(ui);
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => match self.form.rules() {
                Ok(rules) => {
                    if let Some(action) = self.on_accept.take() {
                        action(cx, rules);
                    }
                    false
                }
                Err(message) => {
                    cx.open_dialog(Box::new(Notice { id: "filter_error", title: tr("필터"), text: message }));
                    true
                }
            },
            Some(false) => false,
            None => true,
        }
    }
}

type OnText = Box<dyn FnOnce(&mut AppContext, String)>;

/// `QInputDialog.getText(self.w, title, label, text=initial)`: `on_accept` gets the text on 확인.
pub struct TextBox {
    id: &'static str,
    title: String,
    label: String,
    /// The text in the field.
    pub text: String,
    on_accept: Option<OnText>,
    focused: bool,
}

impl TextBox {
    /// A name box; `title` and `label` are already translated.
    pub fn new(
        id: &'static str,
        title: String,
        label: String,
        initial: &str,
        on_accept: impl FnOnce(&mut AppContext, String) + 'static,
    ) -> Self {
        Self { id, title, label, text: initial.to_owned(), on_accept: Some(Box::new(on_accept)), focused: false }
    }
}

impl Dialog for TextBox {
    fn id(&self) -> &'static str {
        self.id
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response =
            egui::Modal::new(egui::Id::unique(("collections_text", self.id))).frame(super::frame()).show(ctx, |ui| {
                ui.set_width(380.0);
                title(ui, &self.title);
                widgets::label(ui, &self.label, t::TEXT, t::FONT_SIZE);
                ui.add_space(4.0);
                let field = widgets::line_edit(ui, "collections_text_field", &mut self.text, "");
                if !self.focused {
                    field.request_focus();
                    self.focused = true;
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(10.0);
                answer = buttons(ui);
                if enter {
                    answer = Some(true);
                }
            });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some(action) = self.on_accept.take() {
                    action(cx, std::mem::take(&mut self.text));
                }
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

type OnChoice = Box<dyn FnOnce(&mut AppContext, usize)>;

/// `QInputDialog.getItem(self.w, title, label, items, editable=False)`: `on_accept` gets the index on 확인.
pub struct ChoiceBox {
    id: &'static str,
    title: String,
    label: String,
    /// The rows (`{id} · {name}`).
    pub items: Vec<String>,
    /// The chosen row.
    pub chosen: usize,
    on_accept: Option<OnChoice>,
}

impl ChoiceBox {
    /// A choice box; `title` and `label` are already translated.
    pub fn new(
        id: &'static str,
        title: String,
        label: String,
        items: Vec<String>,
        on_accept: impl FnOnce(&mut AppContext, usize) + 'static,
    ) -> Self {
        Self { id, title, label, items, chosen: 0, on_accept: Some(Box::new(on_accept)) }
    }
}

impl Dialog for ChoiceBox {
    fn id(&self) -> &'static str {
        self.id
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response =
            egui::Modal::new(egui::Id::unique(("collections_choice", self.id))).frame(super::frame()).show(ctx, |ui| {
                ui.set_width(380.0);
                title(ui, &self.title);
                widgets::label(ui, &self.label, t::TEXT, t::FONT_SIZE);
                ui.add_space(4.0);
                egui::ComboBox::from_id_salt(("collections_choice_box", self.id))
                    .width(ui.available_width())
                    .selected_text(self.items.get(self.chosen).cloned().unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (index, item) in self.items.iter().enumerate() {
                            ui.selectable_value(&mut self.chosen, index, item);
                        }
                    });
                ui.add_space(10.0);
                answer = buttons(ui);
            });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some(action) = self.on_accept.take() {
                    action(cx, self.chosen);
                }
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}
