//! The shortcut list (`ShortcutDialog`, port of luma/shortcut_list.py): the table in two columns of groups.

use eframe::egui::{self, Ui};
use grainy_i18n::shortcuts::{column_split, group_rows, written, Shortcut, GROUPS};
use grainy_i18n::{tr, Platform};

use super::Dialog;
use crate::context::AppContext;
use crate::theme as t;
use crate::widgets;

/// The shortcut list.
pub struct ShortcutList;

impl Dialog for ShortcutList {
    fn id(&self) -> &'static str {
        "shortcuts"
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        let response = egui::Modal::new(egui::Id::unique("shortcut_list")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(760.0);
            ui.label(egui::RichText::new(tr("단축키")).font(t::bold(14.0)).color(t::BRAND));
            let sizes: Vec<usize> = GROUPS.iter().map(|g| group_rows(g).count()).collect();
            let split = column_split(&sizes);
            ui.columns(2, |columns| {
                for (index, column) in columns.iter_mut().enumerate() {
                    let range = if index == 0 { 0..split } else { split..GROUPS.len() };
                    for group in &GROUPS[range] {
                        widgets::section(column, &tr(group));
                        for row in group_rows(group) {
                            shortcut_row(column, row);
                        }
                    }
                }
            });
            ui.add_space(8.0);
            widgets::muted(
                ui,
                &tr("글자를 입력하는 칸에서는 단축키가 동작하지 않습니다. 도구를 쓰는 중에는 Esc로 끝냅니다."),
            );
            ui.add_space(6.0);
            let mut close = false;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                close = widgets::button(ui, &tr("닫기")).clicked();
            });
            close
        });
        !(response.inner || response.should_close())
    }
}

fn shortcut_row(ui: &mut Ui, row: &Shortcut) {
    let key = written(row, Platform::current());
    let text = tr(row.text);
    let key_galley = widgets::galley(ui, key, t::bold(t::FONT_SIZE), egui::Color32::from_rgb(0xf7, 0xdf, 0xbc));
    let width = ui.available_width();
    let text_galley = widgets::wrapped(ui, text, t::font(t::FONT_SIZE), t::TEXT, (width - 150.0).max(60.0));
    let height = key_galley.size().y.max(text_galley.size().y) + 4.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().galley(rect.min + egui::vec2(0.0, 2.0), key_galley, t::TEXT);
    ui.painter().galley(rect.min + egui::vec2(150.0, 2.0), text_galley, t::TEXT);
}
