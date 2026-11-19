//! The Film panel (필름): the stocks of `grainy_film` grouped by kind, film strength and the note (`StudioTools.build_film` / `load_film`).

use eframe::egui::{self, Ui, Vec2};
use grainy_i18n::{tr, tr_with};

use super::Panel;
use crate::adjustments::{self, Place};
use crate::context::AppContext;
use crate::widgets::{self};

/// The film panel.
pub struct Film;

impl Panel for Film {
    fn key(&self) -> &'static str {
        "film"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

/// Port of `StudioTools.build_film` / `load_film`: the stocks of `grainy_film` grouped by kind (a disabled
/// title per group), film strength, and the note.
fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let current = cx.settings().str("film_look").to_owned();
    let entry = grainy_film::entry(&current);
    let shown = match entry {
        Some(e) => e.name.clone(),
        None if current.is_empty() => tr("필름 없음"),
        None => current.clone(),
    };
    let mut chosen: Option<String> = None;
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = Vec2::new(7.0, 7.0);
        ui.spacing_mut().interact_size.y = 30.0;
        let response = egui::ComboBox::from_id_salt("film_look")
            .width(width - 2.0)
            .height(28.0 * 22.0)
            .selected_text(shown.clone())
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_empty(), tr("필름 없음")).clicked() {
                    chosen = Some(String::new());
                }
                for (kind, title) in grainy_film::KINDS {
                    let entries: Vec<&grainy_film::Entry> =
                        grainy_film::catalog().iter().filter(|e| e.group == kind).collect();
                    if entries.is_empty() {
                        continue;
                    }
                    ui.separator();
                    ui.add_enabled(false, egui::Label::new(tr(title)));
                    for e in entries {
                        if ui.selectable_label(e.id == current, &e.name).clicked() {
                            chosen = Some(e.id.clone());
                        }
                    }
                }
            });
        response.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, tr("필름")));
    });
    if let Some(name) = chosen {
        cx.change_film(&name);
    }
    for spec in adjustments::at(Place::Panel("film")) {
        cx.slider(ui, spec, entry.is_some());
    }
    if entry.is_none() && !current.is_empty() {
        widgets::muted(ui, &tr_with!("이 버전에 없는 필름입니다: {0}", current));
    } else if entry.is_none() {
        widgets::muted(
            ui,
            &tr("필름을 고르면 그 필름 제조사가 공개한 기술 자료의 곡선으로 계산한 색과 톤이 적용됩니다."),
        );
    }
}
