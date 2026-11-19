//! The window of 중복 파일 검사 (luma/manager.py `duplicates`): a tree with one branch per group of files whose
//! contents are the same (`그룹 {n} · {count}개`), each file with its name and path, every branch open
//! (`expandAll`). It is not modal (`dialog.show()`); 720 × 480 like the oracle's.

use eframe::egui;
use serde_json::Value;

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets;

/// One group: the rows of `Catalog.photos()` whose files are byte-identical.
pub type Group = Vec<Value>;

/// The duplicates window.
pub struct DuplicatesDialog {
    pub groups: Vec<Group>,
    open: bool,
}

impl DuplicatesDialog {
    pub fn new(groups: Vec<Group>) -> Self {
        Self { groups, open: true }
    }

    /// The branch text of group `n` (from 1): `f'그룹 {n} · {len(rows)}개'`.
    pub fn group_text(n: usize, count: usize) -> String {
        crate::commands::files::tr_files_with("그룹 {0} · {1}개", &[&n.to_string(), &count.to_string()])
    }

    /// The rows as the tree shows them: (branch text, [(name, path)]).
    pub fn rows(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.groups
            .iter()
            .enumerate()
            .map(|(i, rows)| {
                let files = rows
                    .iter()
                    .map(|p| {
                        let text = |k: &str| p[k].as_str().unwrap_or("").to_owned();
                        (text("name"), text("path"))
                    })
                    .collect();
                (Self::group_text(i + 1, rows.len()), files)
            })
            .collect()
    }
}

impl Dialog for DuplicatesDialog {
    fn id(&self) -> &'static str {
        "duplicates"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        let rows = self.rows();
        let mut open = self.open;
        egui::Window::new(tr("중복 파일 · 내용 해시 일치"))
            .id(egui::Id::unique("duplicates"))
            .open(&mut open)
            .frame(super::frame())
            .default_size([720.0, 480.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.set_min_size(egui::vec2(688.0, 420.0));
                let name_width = 220.0;
                ui.horizontal(|ui| {
                    ui.add_sized([name_width, 18.0], egui::Label::new(egui::RichText::new(tr("파일")).color(t::MUTED)));
                    ui.label(egui::RichText::new(tr("경로")).color(t::MUTED));
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (branch, files) in &rows {
                        widgets::label(ui, branch, t::TEXT, t::FONT_SIZE);
                        for (name, path) in files {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.add_sized(
                                    [name_width - 18.0, 16.0],
                                    egui::Label::new(egui::RichText::new(name).color(t::TEXT)).truncate(),
                                );
                                ui.add(egui::Label::new(egui::RichText::new(path).color(t::VALUE)).truncate());
                            });
                        }
                    }
                });
            });
        self.open = open;
        self.open
    }
}
