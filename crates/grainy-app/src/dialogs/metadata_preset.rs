//! The boxes of the metadata presets and the shortcut keyword (`WorkflowTools.save_metadata`, `choose_metadata`,
//! `set_keyword`): the form that saves a preset (a name, then a check box and a value for each of the eleven
//! metadata fields), the list that picks a saved preset (`QInputDialog.getItem`, for 적용 and 삭제) and the box
//! that asks for the keyword (`QInputDialog.getText`). Each box hands its answer to `commands::workflow`.

use eframe::egui;
use serde_json::Value;

use super::Dialog;
use crate::commands::metadata::python_str;
use crate::commands::workflow::{self, Choice, Then, FIELDS};
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The buttons: the primary one and 취소. `Some(true)` for the primary, `Some(false)` for cancel.
fn buttons(ui: &mut egui::Ui, primary: &str) -> Option<bool> {
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if widgets::button(ui, &tr("취소")).clicked() {
            answer = Some(false);
        }
        ui.add_space(6.0);
        if Button::new(primary).kind(Kind::Primary).show(ui).clicked() {
            answer = Some(true);
        }
    });
    answer
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(t::bold(14.0)).color(t::BRAND));
    ui.add_space(8.0);
}

/// One row of the save form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The metadata key.
    pub key: &'static str,
    /// The oracle's Korean title of the check box.
    pub title: &'static str,
    /// Whether the field goes into the preset.
    pub checked: bool,
    /// The value in the text field.
    pub text: String,
}

/// 메타데이터 프리셋 저장: the form of `save_metadata`.
pub struct PresetSave {
    /// 프리셋 이름.
    pub name: String,
    /// The eleven fields, filled from the open photo.
    pub rows: Vec<Row>,
}

impl PresetSave {
    /// The form for the open photo's catalogue row: `{**info, **user_metadata, 'keywords': keywords}`, with 저작권
    /// and 촬영자 ticked when they have a value.
    pub fn new(photo: &Value) -> Self {
        let text_of = |key: &str| -> String {
            if key == "keywords" {
                return python_str(&photo["keywords"]);
            }
            match photo["user_metadata"].get(key).or_else(|| photo["info"].get(key)) {
                Some(value) => python_str(value),
                None => String::new(),
            }
        };
        let rows = FIELDS
            .iter()
            .map(|(key, title)| {
                let text = text_of(key);
                Row { key, title, checked: matches!(*key, "creator" | "copyright") && !text.is_empty(), text }
            })
            .collect();
        Self { name: String::new(), rows }
    }
}

impl Dialog for PresetSave {
    fn id(&self) -> &'static str {
        "workflow_preset_save"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response =
            egui::Modal::new(egui::Id::unique("workflow_preset_save")).frame(super::frame()).show(ctx, |ui| {
                ui.set_width(520.0);
                title(ui, &tr("메타데이터 프리셋 저장"));
                egui::Grid::new("workflow_preset_form").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(150.0, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new(tr("프리셋 이름")).font(t::font(t::FONT_SIZE)).color(t::TEXT));
                        },
                    );
                    ui.allocate_ui(egui::vec2(340.0, 28.0), |ui| {
                        widgets::line_edit(ui, "workflow_preset_name", &mut self.name, "");
                    });
                    ui.end_row();
                    for row in &mut self.rows {
                        ui.allocate_ui_with_layout(
                            egui::vec2(150.0, 28.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                widgets::checkbox(ui, &mut row.checked, &tr(row.title));
                            },
                        );
                        ui.allocate_ui(egui::vec2(340.0, 28.0), |ui| {
                            widgets::line_edit(ui, &format!("workflow_preset_{}", row.key), &mut row.text, "");
                        });
                        ui.end_row();
                    }
                });
                ui.add_space(10.0);
                answer = buttons(ui, &tr("저장"));
            });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                workflow::save_preset_done(cx, &self.name, &self.rows);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

/// 메타데이터 프리셋 / 프리셋 선택: a saved preset to apply or delete.
pub struct PresetChoose {
    what: Choice,
    /// The saved names, in the order the preference has them.
    pub names: Vec<String>,
    /// The chosen entry.
    pub selected: usize,
}

impl PresetChoose {
    /// The list for `names` (not empty) and what to do with the choice.
    pub fn new(what: Choice, names: Vec<String>) -> Self {
        Self { what, names, selected: 0 }
    }
}

impl Dialog for PresetChoose {
    fn id(&self) -> &'static str {
        "workflow_preset_choose"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response =
            egui::Modal::new(egui::Id::unique("workflow_preset_choose")).frame(super::frame()).show(ctx, |ui| {
                ui.set_width(380.0);
                title(ui, &tr("메타데이터 프리셋"));
                widgets::label(ui, &tr("프리셋 선택"), t::TEXT, t::FONT_SIZE);
                ui.add_space(4.0);
                let shown = self.names.get(self.selected).cloned().unwrap_or_default();
                egui::ComboBox::from_id_salt("workflow_preset_list").width(340.0).selected_text(shown).show_ui(
                    ui,
                    |ui| {
                        for (i, name) in self.names.iter().enumerate() {
                            ui.selectable_value(&mut self.selected, i, name);
                        }
                    },
                );
                ui.add_space(10.0);
                answer = buttons(ui, &tr("확인"));
            });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some(name) = self.names.get(self.selected).cloned() {
                    workflow::choose_done(cx, self.what, &name);
                }
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}

/// 바로가기 키워드: the keyword the shortcut adds.
pub struct KeywordInput {
    then: Then,
    /// The text in the box.
    pub text: String,
}

impl KeywordInput {
    /// The box with the saved keyword in it; `then` says what follows an answer.
    pub fn new(text: String, then: Then) -> Self {
        Self { then, text }
    }
}

impl Dialog for KeywordInput {
    fn id(&self) -> &'static str {
        "workflow_keyword"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("workflow_keyword")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(380.0);
            title(ui, &tr("바로가기 키워드"));
            widgets::label(ui, &tr("키워드 하나 · 계층은 |로 구분"), t::TEXT, t::FONT_SIZE);
            ui.add_space(4.0);
            widgets::line_edit(ui, "workflow_keyword_text", &mut self.text, "");
            ui.add_space(10.0);
            answer = buttons(ui, &tr("확인"));
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                workflow::keyword_done(cx, self.then, &self.text);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}
