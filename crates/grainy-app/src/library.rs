//! The left column: the library filters, the folder tree, presets, the history and collections (luma/app.py
//! `build_ui`, `refresh_folder_tree`, `refresh_history`, `restore_history`; the collections fold-out of
//! luma/manager.py). The filters, the folder and the collection change the document's [`Query`]; the list is read
//! again in the background.

use std::collections::BTreeSet;

use eframe::egui::{self, CornerRadius, Frame, Margin, Panel, Pos2, Rect, Sense, Ui, Vec2};
use grainy_catalog::folders::Platform;
use grainy_i18n::{tr, tr_with};

use crate::commands::{self, Invocation};
use crate::context::{AppContext, Mode, FILTERS};
use crate::document::Query;
use crate::session_state::{ColumnState, TreeNode};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

impl AppContext {
    pub(crate) fn library_column(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
        // The lower part (fold-outs, help, footer) sits at the bottom; the folder tree takes the rest.
        Panel::bottom("library_lower")
            .frame(Frame::new().inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 0 }))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| self.library_lower(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.library_upper(ui));
    }

    /// Changes the list options (the document reads the list again).
    pub(crate) fn change_query(&mut self, change: impl FnOnce(&mut Query)) {
        let mut query = self.doc.query();
        change(&mut query);
        if let Err(message) = self.doc.set_query(query) {
            self.status = crate::text::tr_with("처리하지 못했습니다: {0}", &[&message]);
        }
    }

    fn library_upper(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
        widgets::section(ui, "LIBRARY");
        widgets::muted(ui, &tr_with!("{0}장의 사진", self.doc.total()));
        let width = ui.available_width();
        let query = self.doc.query();
        for (mode, label) in FILTERS {
            let checked =
                query.mode == mode && (mode != "all" || (query.folder.is_none() && query.collection.is_none()));
            if Button::new(&tr(label)).kind(Kind::Nav).checked(checked).width(width).show(ui).clicked() {
                // Port of `set_filter`: "all" also leaves the folder and collection.
                self.change_query(|q| {
                    q.mode = mode.to_owned();
                    if mode == "all" {
                        q.folder = None;
                        q.collection = None;
                    }
                });
                self.set_mode(Mode::Library);
            }
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let title = widgets::spaced(ui, &tr("폴더"), t::bold(10.0), t::SECTION, 2.0);
            let room = ui.available_width() - 60.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(room, title.size().y + 18.0), Sense::hover());
            ui.painter().galley(egui::pos2(rect.left(), rect.top() + 12.0), title, t::SECTION);
            let removable =
                ColumnState::removable(&query).is_some() && commands::enabled(self, "library.remove_folder");
            let remove = Button::new("−").width(30.0).enabled(removable).show(ui);
            if remove.on_hover_text(tr("선택한 폴더를 카탈로그에서 제거 · 원본 파일은 그대로 둡니다")).clicked()
            {
                self.run("library.remove_folder");
            }
            if Button::new("+").width(30.0).show(ui).on_hover_text(tr("사진 폴더 가져오기")).clicked() {
                self.run("file.import_folder");
            }
        });
        let mut search = std::mem::take(&mut self.folder_search);
        widgets::line_edit(ui, "folder_search", &mut search, &tr("폴더 찾기"));
        self.folder_search = search;
        let mut recursive = query.recursive;
        if widgets::checkbox(ui, &mut recursive, &tr("하위 폴더 사진 포함")).changed() {
            self.change_query(|q| q.recursive = recursive);
        }
        let nodes = self.state::<ColumnState>().nodes.clone();
        let note = (nodes.is_empty()).then(|| {
            widgets::wrapped(ui, tr("사진 폴더를 가져오면 여기에 표시됩니다."), t::font(t::FONT_SIZE), t::MUTED, width)
        });
        let note_height = note.as_ref().map_or(0.0, |n| n.size().y + 10.0);
        let tree_height = (ui.available_height() - note_height).clamp(80.0, 500.0);
        let (tree, background) = ui.allocate_exact_size(Vec2::new(width, tree_height), Sense::click());
        ui.painter().rect_filled(tree, CornerRadius::ZERO, t::LIST_BG);
        background.context_menu(|ui| self.folder_menu(ui, None));
        if !nodes.is_empty() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(tree.shrink(4.0)));
            egui::ScrollArea::vertical().id_salt("folder_tree").auto_shrink([false, false]).show(&mut child, |ui| {
                let expanded = self.state::<ColumnState>().expanded.clone();
                let rows = visible_rows(&nodes, &self.folder_search, &expanded);
                for row in &rows {
                    self.folder_row(ui, row, &query);
                }
            });
        }
        if let Some(note) = note {
            let (rect, _) = ui.allocate_exact_size(note.size(), Sense::hover());
            ui.painter().galley(rect.min, note, t::MUTED);
        }
    }

    /// One folder of the tree: the arrow, the name (`? ` for a folder that is gone) and the count; a click lists
    /// the folder (`folder_current_changed` → `select_folder`), the arrow or a double click expands it
    /// (`folder_expansion_changed`), the right button opens the menu (`folder_context_menu`).
    fn folder_row(&mut self, ui: &mut Ui, row: &Row, query: &Query) {
        let node = &row.node;
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click());
        let platform = Platform::native();
        let selected = query.folder.as_deref().is_some_and(|f| platform.path_key(f) == node.key);
        if selected {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_SELECTED);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_HOVER);
        }
        let left = rect.left() + 4.0 + row.depth as f32 * 16.0;
        let arrow = Rect::from_min_size(Pos2::new(left, rect.top()), Vec2::new(16.0, 22.0));
        if row.arrow {
            widgets::arrow(ui, arrow.center(), row.open, t::MUTED);
        }
        let color = if selected { t::BRAND } else { t::VALUE };
        let number = if query.recursive || node.drive { node.total } else { node.direct };
        let count = widgets::galley(ui, number.to_string(), t::font(t::FONT_SIZE), t::MUTED);
        let count_rect = Rect::from_min_size(
            Pos2::new(rect.right() - 8.0 - count.size().x, rect.center().y - count.size().y / 2.0),
            count.size(),
        );
        ui.painter().galley(count_rect.min, count, t::MUTED);
        let name_rect = Rect::from_min_max(
            Pos2::new(arrow.right() + 2.0, rect.top()),
            Pos2::new(count_rect.left() - 6.0, rect.bottom()),
        );
        let name = format!("{}{}", if node.missing { "? " } else { "" }, node.name);
        widgets::paint_left(ui, name_rect, &name, t::font(t::FONT_SIZE), color);
        let note = if node.missing {
            tr("\n폴더를 찾을 수 없습니다. 우클릭하여 새 위치를 지정하세요.")
        } else {
            tr_with("\n등록된 사진 {0}장 · 새 사진은 폴더 동기화로 가져오세요.", &[&node.total.to_string()])
        };
        // The oracle's count tooltip is an untranslated f-string.
        ui.interact(
            count_rect.expand2(Vec2::new(4.0, 4.0)),
            egui::Id::unique(("folder_count", node.key.clone())),
            Sense::hover(),
        )
        .on_hover_text(format!("이 폴더 {}장 · 하위 폴더 포함 {}장", node.direct, node.total));
        let response = response.on_hover_text(format!("{}{}", node.path, note));
        response.context_menu(|ui| self.folder_menu(ui, Some(&node.path)));
        let on_arrow = row.arrow && response.interact_pointer_pos().is_some_and(|p| arrow.contains(p));
        let toggle = (response.clicked() && on_arrow) || response.double_clicked();
        if toggle && self.folder_search.is_empty() {
            let key = node.key.clone();
            let st = self.state::<ColumnState>();
            if !row.open {
                st.expanded.insert(key);
                st.rescan();
            } else {
                st.expanded.remove(&key);
            }
        }
        if response.clicked() && !on_arrow {
            let path = node.path.clone();
            self.change_query(|q| {
                q.folder = Some(path);
                q.collection = None;
            });
            self.set_mode(Mode::Library);
        }
    }

    /// The tree's context menu (`folder_context_menu`); `path` is the folder under the pointer.
    fn folder_menu(&mut self, ui: &mut Ui, path: Option<&str>) {
        let item = |cx: &mut AppContext, ui: &mut Ui, id: &str, label: &str, value: Option<&str>| {
            let enabled = commands::enabled(cx, id);
            if ui.add_enabled(enabled, egui::Button::new(tr(label))).clicked() {
                match value {
                    Some(v) => cx.run_with(id, &Invocation::value(v)),
                    None => cx.run(id),
                };
                ui.close();
            }
        };
        item(self, ui, "file.import_folder", "폴더 가져오기…", None);
        item(self, ui, "library.refresh_folders", "폴더 목록 새로 고침", None);
        item(self, ui, "library.recover_folders", crate::folders::recover_label(self), None);
        if let Some(path) = path {
            ui.separator();
            item(self, ui, "library.sync_folder", "폴더 동기화 · 새 사진 가져오기", Some(path));
            item(self, ui, "library.open_folder", crate::system_open::SHOW_IN_FILE_MANAGER, Some(path));
            if std::path::Path::new(path).parent().is_some() {
                item(self, ui, "library.show_parent", "상위 폴더 표시", Some(path));
                item(self, ui, "library.locate_folder", "폴더 위치 다시 지정…", Some(path));
                ui.separator();
                item(self, ui, "library.remove_folder", "카탈로그에서 폴더 제거…", Some(path));
            }
        }
    }

    /// The status bar's 가져오기 중단 button, shown while an import runs (`import_cancel_button`).
    pub(crate) fn import_cancel_button(&mut self, ui: &mut Ui) {
        if !self.jobs.running("import") {
            return;
        }
        let cancelling = self.state::<ColumnState>().cancelling;
        if Button::new(&tr("가져오기 중단")).enabled(!cancelling).show(ui).clicked() {
            self.run("library.cancel_import");
        }
    }

    fn library_lower(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
        let width = ui.available_width();
        let mut open = self.foldouts.contains("presets");
        widgets::foldout(ui, &tr("프리셋"), &mut open);
        self.set_foldout("presets", open);
        if open {
            crate::panels::presets::body(self, ui, width);
        }
        let mut open = self.foldouts.contains("history");
        widgets::foldout(ui, &tr("보정 기록"), &mut open);
        self.set_foldout("history", open);
        if open {
            self.history_list(ui, width);
        }
        if Button::new(&tr("사용법 / 구현 범위")).width(width).show(ui).clicked() {
            self.run("settings.help");
        }
        widgets::muted(ui, &tr("LOCAL FIRST\n원본 보존 · 보정값 자동 저장"));
        let mut open = self.foldouts.contains("collections");
        widgets::foldout(ui, &tr("컬렉션"), &mut open);
        self.set_foldout("collections", open);
        if open {
            crate::panels::collections::body(self, ui, width);
        }
    }

    /// The History list (`refresh_history`): a double click restores the state before that edit
    /// (`restore_history`).
    fn history_list(&mut self, ui: &mut Ui, width: f32) {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 135.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_BG);
        let mut restore = None;
        let mut y = rect.top() + 4.0;
        for (index, entry) in self.doc.history().iter().take(6).enumerate() {
            let row = Rect::from_min_size(egui::pos2(rect.left() + 4.0, y), Vec2::new(width - 8.0, 21.0));
            let hit = ui.interact(row, egui::Id::unique(("history", index)), Sense::click());
            if hit.hovered() {
                ui.painter().rect_filled(row, CornerRadius::ZERO, t::LIST_HOVER);
            }
            let label = if entry.settings.is_some() { entry.label.clone() } else { tr(&entry.label) };
            widgets::paint_left(ui, row.shrink2(Vec2::new(5.0, 0.0)), &label, t::font(t::FONT_SIZE), t::VALUE);
            let hit = if entry.created.is_empty() { hit } else { hit.on_hover_text(&entry.created) };
            if hit.double_clicked() {
                restore = Some(index);
            }
            y += 22.0;
        }
        response.on_hover_text(tr("더블클릭하여 해당 보정 이전 상태로 복원"));
        if let Some(index) = restore {
            self.doc.restore_history(index);
        }
    }
}

/// A visible row of the tree.
struct Row {
    node: TreeNode,
    depth: usize,
    /// Whether the arrow shows (children, or a folder not scanned yet: `ShowIndicator`).
    arrow: bool,
    open: bool,
}

/// Port of `filter_folder_tree`: the rows that show. A folder shows when its name matches, an ancestor's name
/// matches or a descendant shows; while searching the matching branches are open, otherwise the saved
/// expansion decides.
fn visible_rows(nodes: &[TreeNode], search: &str, expanded: &BTreeSet<String>) -> Vec<Row> {
    fn visit(
        node: &TreeNode,
        depth: usize,
        query: &str,
        ancestor: bool,
        expanded: &BTreeSet<String>,
        out: &mut Vec<Row>,
    ) -> bool {
        let matches = query.is_empty() || node.name.to_lowercase().contains(query) || ancestor;
        let mut below = Vec::new();
        let mut any = false;
        for child in &node.children {
            any |= visit(child, depth + 1, query, matches, expanded, &mut below);
        }
        let visible = matches || any;
        if visible {
            let has_children = !node.children.is_empty();
            let open = has_children && ((!query.is_empty() && visible) || expanded.contains(&node.key));
            out.push(Row { node: node.clone(), depth, arrow: has_children || node.unscanned, open });
            if open {
                out.append(&mut below);
            }
        }
        visible
    }
    let query = search.trim().to_lowercase();
    let mut out = Vec::new();
    for node in nodes {
        visit(node, 0, &query, false, expanded, &mut out);
    }
    out
}

#[allow(dead_code)]
/// An empty list box (`QTreeWidget` / `QListWidget` background) standing in for a list of a later work package.
fn stub_list(ui: &mut Ui, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_BG);
}
