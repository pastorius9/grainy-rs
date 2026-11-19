//! The lens dialog (렌즈 자동 보정): a searchable camera and lens, and the capture conditions the profile is
//! calculated for.
//!
//! Port of luma/lens_dialog.py `LensDialog`. [`LensChoice`] is the dialog's state without a window (the combo
//! boxes' items and texts, the four number boxes with their ranges and decimals, the status line, the result),
//! driven by the same steps as the oracle's widgets: `camera_changed`, `lens_changed`, `apply` and the
//! constructor's pre-selection. [`LensDialog`] draws it.

use std::sync::Arc;

use eframe::egui::{self, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use grainy_core::settings::truthy;
use grainy_lens::{Camera, Conditions, Database, LensRecord};
use serde_json::Value;

use super::Dialog;
use crate::commands::lens::{py_float, tr, LensState};
use crate::context::AppContext;
use crate::text::tr_with;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The id of the dialog.
pub const ID: &str = "lens";

/// Python's `format(value, 'g')`.
fn general(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let scientific = format!("{:.5e}", value);
    let exponent = scientific.split_once('e').and_then(|(_, e)| e.parse::<i32>().ok()).unwrap_or(0);
    if !(-4..6).contains(&exponent) {
        let (mantissa, exp) = scientific.split_once('e').unwrap_or((&scientific, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let mantissa =
            if mantissa.contains('.') { mantissa.trim_end_matches('0').trim_end_matches('.') } else { mantissa };
        return format!("{mantissa}e{exp:+03}");
    }
    let places = (5 - exponent).max(0) as usize;
    let text = format!("{value:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// A `QDoubleSpinBox`: a value kept within its range and rounded to its decimals.
#[derive(Debug, Clone, PartialEq)]
pub struct Spin {
    pub value: f64,
    pub low: f64,
    pub high: f64,
    pub decimals: usize,
}

impl Spin {
    /// The control `number()` makes: the range and decimals, then `float(value)` (or the minimum when that fails).
    pub fn new(low: f64, high: f64, decimals: usize, value: Option<f64>) -> Self {
        let mut spin = Self { value: low, low, high, decimals };
        spin.set_range(low, high);
        spin.set_value(value.unwrap_or(low));
        spin
    }

    fn round(&self, value: f64) -> f64 {
        format!("{value:.*}", self.decimals).parse().unwrap_or(value)
    }

    /// `setValue`: rounded to the decimals, then bounded by the range.
    pub fn set_value(&mut self, value: f64) {
        if value.is_nan() {
            return;
        }
        self.value = self.round(value).clamp(self.low, self.high);
    }

    /// `setRange`: the limits are rounded as values are; the value moves into the range.
    pub fn set_range(&mut self, low: f64, high: f64) {
        self.low = self.round(low);
        self.high = self.round(high);
        self.value = self.value.clamp(self.low, self.high);
    }
}

/// `float(metadata.get(key) or default)` as `number()` receives it: `None` when it cannot be converted.
fn metadata_number(metadata: &Value, key: &str, default: f64) -> Option<f64> {
    match metadata.get(key) {
        Some(v) if truthy(v) => py_float(v).ok(),
        _ => Some(default),
    }
}

/// One item of the lens box: the placeholder, a database lens, or the photo's own saved profile.
#[derive(Debug, Clone)]
pub struct LensItem {
    pub text: String,
    /// `None` for the placeholder (`사용한 렌즈를 선택하세요`).
    pub record: Option<LensRecord>,
}

/// The dialog's state.
pub struct LensChoice {
    database: Arc<Database>,
    metadata: Value,
    /// `camera`: the direct-input entry first, then the cameras.
    pub cameras: Vec<(String, Option<Camera>)>,
    pub camera: usize,
    /// What the editable camera box shows.
    pub camera_text: String,
    pub lenses: Vec<LensItem>,
    pub lens: usize,
    pub lens_text: String,
    pub crop: Spin,
    pub focal: Spin,
    pub aperture: Spin,
    pub distance: Spin,
    /// The status label.
    pub status: String,
    /// `result_profile`: set when `apply` succeeded.
    pub result: Option<Value>,
}

impl LensChoice {
    /// Port of `LensDialog.__init__`: the boxes, the camera of the photo's metadata, its lens when it is
    /// unambiguous, or the photo's current profile.
    pub fn new(database: Arc<Database>, metadata: Value, current: Option<Value>) -> Self {
        let mut cameras = vec![(tr("카메라 직접 선택 · 센서 배율 입력"), None)];
        for camera in database.cameras() {
            let text = if camera.variant.is_empty() {
                camera.model.clone()
            } else {
                format!("{} ({})", camera.model, camera.variant)
            };
            cameras.push((text, Some(camera.clone())));
        }
        let camera_text = cameras[0].0.clone();
        let mut choice = Self {
            crop: Spin::new(0.1, 20.0, 3, Some(1.0)),
            focal: Spin::new(0.1, 5000.0, 2, metadata_number(&metadata, "focal_length", 50.0)),
            aperture: Spin::new(0.0, 128.0, 2, metadata_number(&metadata, "aperture", 0.0)),
            distance: Spin::new(0.01, 100000.0, 2, metadata_number(&metadata, "subject_distance", 1000.0)),
            database,
            metadata,
            cameras,
            camera: 0,
            camera_text,
            lenses: Vec::new(),
            lens: 0,
            lens_text: String::new(),
            status: String::new(),
            result: None,
        };
        let matched = choice.database.match_metadata(&choice.metadata);
        let (camera, candidates, reason) = (
            matched.camera.cloned(),
            matched.candidates.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            matched.reason,
        );
        if let Some(camera) = camera {
            let found = choice
                .cameras
                .iter()
                .position(|(_, c)| c.as_ref().is_some_and(|c| (&c.maker, &c.model) == (&camera.maker, &camera.model)));
            if let Some(i) = found {
                choice.set_camera(i);
            }
        }
        choice.camera_changed();
        if let Some(current) = current {
            choice.select_current(&current);
        } else if candidates.len() == 1 {
            let found = choice.lenses.iter().position(|l| l.record.as_ref().is_some_and(|r| r.id == candidates[0]));
            if let Some(i) = found {
                choice.set_lens(i);
            }
        }
        choice.status =
            if reason.is_empty() { tr("사용할 프로파일과 촬영 조건을 확인하세요.") } else { reason };
        choice
    }

    /// The `current` branch of the constructor: the profile's own lens in the list (or appended as a saved
    /// profile), and its capture conditions in the number boxes.
    fn select_current(&mut self, current: &Value) {
        let xml = current.get("xml").and_then(Value::as_str).unwrap_or("");
        let found = self.lenses.iter().position(|l| l.record.as_ref().is_some_and(|r| r.xml == xml));
        let index = match found {
            Some(i) => i,
            None => {
                let name = current.get("name").and_then(Value::as_str).unwrap_or("");
                let record = LensRecord {
                    id: String::new(),
                    maker: current.get("maker").and_then(Value::as_str).unwrap_or("").to_owned(),
                    model: name.to_owned(),
                    crop: current.get("crop").and_then(Value::as_f64).unwrap_or(1.0),
                    mounts: Vec::new(),
                    xml: xml.to_owned(),
                    file: current.get("database_file").and_then(Value::as_str).unwrap_or("").to_owned(),
                };
                self.lenses
                    .push(LensItem {
                        text: format!("{name}{}", tr(" · 저장된 프로파일")), record: Some(record)
                    });
                self.lenses.len() - 1
            }
        };
        self.set_lens(index);
        for (key, spin) in [
            ("crop", &mut self.crop),
            ("focal", &mut self.focal),
            ("aperture", &mut self.aperture),
            ("distance", &mut self.distance),
        ] {
            if let Some(v) = current.get(key).and_then(Value::as_f64) {
                spin.set_value(v);
            }
        }
    }

    /// `camera.setCurrentIndex(i)`: the box shows the item; a changed index runs `camera_changed`.
    pub fn set_camera(&mut self, index: usize) {
        if index >= self.cameras.len() {
            return;
        }
        self.camera_text = self.cameras[index].0.clone();
        if index != self.camera {
            self.camera = index;
            self.camera_changed();
        }
    }

    /// `lens.setCurrentIndex(i)`.
    pub fn set_lens(&mut self, index: usize) {
        if index >= self.lenses.len() {
            return;
        }
        self.lens_text = self.lenses[index].text.clone();
        if index != self.lens {
            self.lens = index;
            self.lens_changed();
        }
    }

    /// Whether the database has no cameras and no lenses.
    pub fn database_is_empty(&self) -> bool {
        self.database.lens_records().is_empty() && self.database.cameras().is_empty()
    }

    /// The selected camera's data (`camera.currentData()`).
    pub fn camera_data(&self) -> Option<&Camera> {
        self.cameras.get(self.camera).and_then(|(_, c)| c.as_ref())
    }

    /// The selected lens' record (`lens.currentData()`).
    pub fn lens_data(&self) -> Option<&LensRecord> {
        self.lenses.get(self.lens).and_then(|l| l.record.as_ref())
    }

    /// Port of `camera_changed`: the camera's crop factor, and the lenses that fit it (all without a camera).
    pub fn camera_changed(&mut self) {
        if let Some(camera) = self.camera_data() {
            let crop = f64::from(camera.crop_factor);
            self.crop.set_value(crop);
        }
        self.focal.set_range(0.1, 5000.0);
        let camera = self.camera_data().cloned();
        let mut lenses = vec![LensItem { text: tr("사용한 렌즈를 선택하세요"), record: None }];
        for record in self.database.compatible_lenses(camera.as_ref()) {
            let text = tr_with("{0}  ·  배율 {1}", &[&record.model, &general(record.crop)]);
            lenses.push(LensItem { text, record: Some(record.clone()) });
        }
        self.lenses = lenses;
        self.lens = 0;
        self.lens_text = self.lenses[0].text.clone();
    }

    /// Port of `lens_changed`: the lens' focal range, its crop factor when no camera was chosen, and what the
    /// profile can correct.
    pub fn lens_changed(&mut self) {
        let Some(record) = self.lens_data().cloned() else { return };
        let lens = match grainy_lens::profile_lens(&record) {
            Ok(lens) => lens,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        if lens.min_focal != 0.0 {
            self.focal.set_range(f64::from(lens.min_focal), f64::from(lens.max_focal));
        }
        if self.camera_data().is_none() {
            self.crop.set_value(record.crop);
        }
        let capabilities: Vec<String> = [
            ("왜곡", !lens.distortion.is_empty()),
            ("색수차", !lens.tca.is_empty()),
            ("주변광량", !lens.vignetting.is_empty()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| crate::text::tr(name))
        .collect();
        self.status = tr("프로파일 제공 항목: ") + &capabilities.join(" · ");
    }

    /// Port of `apply`: the profile for the chosen lens and conditions, or the message shown in the status line.
    pub fn apply(&mut self) -> bool {
        match self.make() {
            Ok(profile) => {
                self.result = Some(profile);
                true
            }
            Err(message) => {
                self.status = message;
                false
            }
        }
    }

    fn make(&self) -> Result<Value, String> {
        let Some(record) = self.lens_data().filter(|_| self.lens_text == self.lenses[self.lens].text) else {
            return Err("검색 결과에서 실제 렌즈 항목을 선택해 주세요.".to_owned());
        };
        if self.camera_text != self.cameras[self.camera].0 {
            return Err("검색 결과에서 카메라를 선택하거나 직접 입력 항목을 선택해 주세요.".to_owned());
        }
        let orientation = match self.metadata.get("source_orientation") {
            Some(v) if truthy(v) => match v {
                Value::Number(n) => n.as_f64().unwrap_or(1.0) as i64,
                Value::String(s) => {
                    s.trim().parse::<i64>().map_err(|_| format!("invalid literal for int() with base 10: '{s}'"))?
                }
                Value::Bool(_) => 1,
                _ => return Err("int() argument must be a string, a bytes-like object or a real number".to_owned()),
            },
            _ => 1,
        };
        let conditions = Conditions {
            crop: self.crop.value,
            focal: self.focal.value,
            aperture: self.aperture.value,
            distance: self.distance.value,
            orientation,
            camera: self.camera_data().map(|c| c.model.clone()).unwrap_or_default(),
        };
        grainy_lens::make_profile(record, &conditions).map_err(|e| e.to_string())
    }
}

// ---- the window ----------------------------------------------------------------------------------------------

/// What the dialog's controls keep between frames (the open drop-downs, the number fields being typed in).
#[derive(Default)]
struct Fields {
    camera_open: bool,
    lens_open: bool,
    editing: Option<(&'static str, String)>,
}

/// The lens dialog: [`LensChoice`] in a modal window; 보정 적용 applies the profile to the open photo.
pub struct LensDialog {
    choice: LensChoice,
    fields: Fields,
    /// The database is empty: the dialog says where it looked.
    empty: bool,
}

impl LensDialog {
    /// The dialog for the photo's metadata (`info`) and its current lens profile.
    pub fn new(database: Arc<Database>, info: Value, current: Option<Value>) -> Self {
        Self::from_choice(LensChoice::new(database, info, current))
    }

    /// The dialog on a state made before.
    pub fn from_choice(choice: LensChoice) -> Self {
        let empty = choice.database_is_empty();
        Self { choice, fields: Fields::default(), empty }
    }

    /// The state (tests).
    pub fn choice_mut(&mut self) -> &mut LensChoice {
        &mut self.choice
    }
}

impl Dialog for LensDialog {
    fn id(&self) -> &'static str {
        ID
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let notice = self.empty.then(|| {
            let state = cx.state::<LensState>();
            let folder = state.folder.as_ref().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            state
                .database_error()
                .map(str::to_owned)
                .unwrap_or_else(|| tr_with("렌즈 데이터베이스(lensfun)를 찾지 못했습니다. 폴더: {0}", &[&folder]))
        });
        let popup_open = self.fields.camera_open || self.fields.lens_open;
        let mut accepted = false;
        let mut close = false;
        let response = egui::Modal::new(Id::unique("lens_dialog")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.label(egui::RichText::new(tr("렌즈 자동 보정")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            form(ui, &mut self.choice, &mut self.fields);
            ui.add_space(8.0);
            widgets::muted(
                ui,
                &crate::text::tr(
                    "촬영 정보를 먼저 사용합니다. 필름 스캔처럼 정보가 없는 사진은 실제 촬영 조건을 입력하세요.\n거리 정보가 없으면 1,000m(원거리)를 기준으로 계산합니다. 센서 배율은 35mm 전체 화면 기준입니다.",
                ),
            );
            ui.add_space(8.0);
            widgets::label(ui, &self.choice.status, t::TEXT, t::FONT_SIZE);
            if let Some(notice) = notice.as_ref() {
                ui.add_space(6.0);
                widgets::label(ui, notice, t::MUTED, t::FONT_SIZE);
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let cancel = crate::text::tr("취소");
                let response = widgets::button(ui, &cancel);
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &cancel));
                close = response.clicked();
                ui.add_space(6.0);
                let apply = tr("보정 적용");
                let response = Button::new(&apply).kind(Kind::Primary).show(ui);
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &apply));
                accepted = response.clicked();
            });
        });
        if accepted && self.choice.apply() {
            crate::commands::lens::accepted(cx, &mut self.choice);
            return false;
        }
        !(close || (response.should_close() && !accepted && !popup_open))
    }
}

const LABEL_WIDTH: f32 = 130.0;

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(LABEL_WIDTH, 30.0), Sense::hover());
        let g = widgets::galley(ui, label, t::font(t::FONT_SIZE), t::TEXT);
        ui.painter().galley(Pos2::new(rect.left(), rect.center().y - g.size().y / 2.0), g, t::TEXT);
        add(ui);
    });
}

fn form(ui: &mut egui::Ui, choice: &mut LensChoice, state: &mut Fields) {
    let width = ui.available_width() - LABEL_WIDTH - ui.spacing().item_spacing.x;
    row(ui, &tr("카메라 찾기"), |ui| {
        let texts: Vec<String> = choice.cameras.iter().map(|(t, _)| t.clone()).collect();
        if let Some(i) =
            search_combo(ui, "camera", &texts, &mut choice.camera_text, choice.camera, width, &mut state.camera_open)
        {
            choice.set_camera(i);
        }
    });
    row(ui, &tr("렌즈 찾기"), |ui| {
        let texts: Vec<String> = choice.lenses.iter().map(|l| l.text.clone()).collect();
        if let Some(i) =
            search_combo(ui, "lens", &texts, &mut choice.lens_text, choice.lens, width, &mut state.lens_open)
        {
            choice.set_lens(i);
        }
    });
    let special = tr("정보 없음 · 주변광량 보정 제외");
    for (key, title, special) in [
        ("crop", "센서 배율", None),
        ("focal", "초점 거리 mm", None),
        ("aperture", "조리개 f/", Some(special.as_str())),
        ("distance", "촬영 거리 m", None),
    ] {
        row(ui, &tr(title), |ui| {
            let spin = match key {
                "crop" => &mut choice.crop,
                "focal" => &mut choice.focal,
                "aperture" => &mut choice.aperture,
                _ => &mut choice.distance,
            };
            spin_box(ui, key, spin, special, 280.0, &mut state.editing);
        });
    }
}

fn field_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(t::INPUT_BG)
        .stroke(Stroke::new(1.0, t::INPUT_BORDER))
        .inner_margin(egui::Margin::symmetric(7, 6))
}

/// A `QDoubleSpinBox`: typing sets the value when the field is left or Enter is pressed; the up and down arrow keys
/// step by one unit of the last decimal place times ten.
fn spin_box(
    ui: &mut egui::Ui,
    key: &'static str,
    spin: &mut Spin,
    special: Option<&str>,
    width: f32,
    editing: &mut Option<(&'static str, String)>,
) {
    let shown = match special {
        Some(text) if spin.value == spin.low => text.to_owned(),
        _ => format!("{:.*}", spin.decimals, spin.value),
    };
    let mut text = match editing {
        Some((k, buffer)) if *k == key => buffer.clone(),
        _ => shown,
    };
    let output = egui::TextEdit::singleline(&mut text)
        .id_salt(("lens_spin", key))
        .frame(field_frame())
        .desired_width(width)
        .text_color(t::TEXT)
        .show(ui);
    let response = output.response;
    if response.has_focus() {
        *editing = Some((key, text.clone()));
        let step = 10f64.powi(1 - spin.decimals as i32);
        let up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
        let down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
        if up || down {
            let value = spin.value + if up { step } else { -step };
            spin.set_value(value);
            *editing = Some((key, format!("{:.*}", spin.decimals, spin.value)));
        }
    }
    let enter = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if response.lost_focus() || enter {
        if let Some((k, buffer)) = editing.take() {
            if k == key {
                if let Ok(value) = buffer.trim().parse::<f64>() {
                    spin.set_value(value);
                }
            } else {
                *editing = Some((k, buffer));
            }
        }
    }
}

/// An editable box with a drop-down list that narrows to the items containing the typed text (a `QComboBox` with a
/// contains-completer). Returns the item chosen this frame.
fn search_combo(
    ui: &mut egui::Ui,
    id: &str,
    texts: &[String],
    text: &mut String,
    index: usize,
    width: f32,
    open: &mut bool,
) -> Option<usize> {
    let mut chosen = None;
    let arrow = 24.0;
    let output = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let edit = egui::TextEdit::singleline(text)
                .id_salt(("lens_combo", id))
                .frame(field_frame())
                .desired_width(width - arrow)
                .text_color(t::TEXT)
                .show(ui)
                .response;
            let (rect, response) = ui.allocate_exact_size(Vec2::new(arrow, edit.rect.height()), Sense::click());
            let fill = if response.hovered() { t::BUTTON_HOVER } else { t::BUTTON };
            ui.painter().rect(rect, 0.0, fill, Stroke::new(1.0, t::INPUT_BORDER), egui::StrokeKind::Inside);
            widgets::arrow(ui, rect.center(), true, t::TEXT);
            (edit, response)
        })
        .inner;
    let (edit, button) = output;
    if edit.changed() || edit.gained_focus() {
        *open = true;
    }
    if button.clicked() {
        *open = !*open;
    }
    if !*open {
        return None;
    }
    let query = text.to_lowercase();
    let narrowed = text.as_str() != texts.get(index).map_or("", String::as_str);
    let rows: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, t)| !narrowed || query.is_empty() || t.to_lowercase().contains(&query))
        .map(|(i, _)| i)
        .collect();
    let top_left = Pos2::new(edit.rect.left(), edit.rect.bottom() + 1.0);
    let row_height = 24.0;
    let area = egui::Area::new(Id::unique(("lens_popup", id))).order(Order::Tooltip).fixed_pos(top_left).show(
        ui.ctx(),
        |ui| {
            egui::Frame::new().fill(t::LIST_BG).stroke(Stroke::new(1.0, t::MENU_BORDER)).show(ui, |ui| {
                ui.set_width(width - 2.0);
                if rows.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("–").color(t::MUTED));
                    });
                    ui.add_space(4.0);
                    return;
                }
                egui::ScrollArea::vertical().max_height(row_height * 9.0).auto_shrink([false, true]).show_rows(
                    ui,
                    row_height,
                    rows.len(),
                    |ui, range| {
                        for r in range {
                            let i = rows[r];
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::new(width - 2.0, row_height), Sense::click());
                            let fill = if i == index {
                                t::LIST_SELECTED
                            } else if response.hovered() {
                                t::LIST_HOVER
                            } else {
                                t::LIST_BG
                            };
                            let name = texts[i].clone();
                            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListBoxOption, true, &name));
                            ui.painter().rect_filled(rect, 0.0, fill);
                            let galley = widgets::galley(ui, texts[i].clone(), t::font(t::FONT_SIZE), t::TEXT);
                            ui.painter().with_clip_rect(rect).galley(
                                Pos2::new(rect.left() + 7.0, rect.center().y - galley.size().y / 2.0),
                                galley,
                                t::TEXT,
                            );
                            if response.clicked() {
                                chosen = Some(i);
                            }
                        }
                    },
                );
            });
        },
    );
    let area_rect: Rect = area.response.rect;
    let pressed_outside = ui.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !area_rect.contains(p) && !edit.rect.contains(p) && !button.rect.contains(p))
    });
    let leave = ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter));
    if chosen.is_some() || pressed_outside || (leave && edit.has_focus()) {
        *open = false;
    }
    chosen
}
