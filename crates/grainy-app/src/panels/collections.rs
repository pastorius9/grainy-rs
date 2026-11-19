//! The 컬렉션 fold-out of the library column (luma/manager.py `build_collections`, `refresh_collections`,
//! `select_collection`): the collections as a tree (a collection nests under its parent; smart ones carry the
//! mark `◈ `), the chosen one highlighted, and the four buttons + · 스마트 · 추가 · 삭제.

use eframe::egui::{self, CornerRadius, Pos2, Rect, Sense, Ui, Vec2};

use crate::commands::{self, collections::CollectionsState};
use crate::context::AppContext;
use crate::document::Collection;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button};

/// One row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    /// The text of the item (`◈ ` before the name of a smart collection).
    pub text: String,
    pub depth: usize,
    /// Whether it has children (the branch arrow).
    pub parent: bool,
}

/// The tree's rows in the order Qt shows them with every branch open: the top-level collections (those without a
/// parent, or whose parent is not in the list) in the list's order, each followed by its children, depth first.
/// `collapsed` hides the children of the collections in it.
pub fn rows(collections: &[Collection], collapsed: &std::collections::HashSet<i64>) -> Vec<Row> {
    fn visit(
        collections: &[Collection],
        collection: &Collection,
        depth: usize,
        collapsed: &std::collections::HashSet<i64>,
        seen: &mut Vec<i64>,
        out: &mut Vec<Row>,
    ) {
        if seen.contains(&collection.id) {
            return;
        }
        seen.push(collection.id);
        let children: Vec<&Collection> = collections.iter().filter(|c| c.parent == Some(collection.id)).collect();
        let mark = if collection.smart { "◈ " } else { "" };
        out.push(Row {
            id: collection.id,
            text: format!("{mark}{}", collection.name),
            depth,
            parent: !children.is_empty(),
        });
        if !collapsed.contains(&collection.id) {
            for child in children {
                visit(collections, child, depth + 1, collapsed, seen, out);
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for collection in collections {
        let top = match collection.parent {
            Some(parent) => !collections.iter().any(|c| c.id == parent),
            None => true,
        };
        if top {
            visit(collections, collection, 0, collapsed, &mut seen, &mut out);
        }
    }
    out
}

/// The fold-out's body: the tree (192 px high, `QTreeWidget`'s size hint) and the button row. The library column
/// calls it with the column's width.
pub fn body(cx: &mut AppContext, ui: &mut Ui, width: f32) {
    let collections = cx.doc.collections();
    let current = cx.doc.query().collection;
    let collapsed = cx.state::<CollectionsState>().collapsed.clone();
    let rows = rows(&collections, &collapsed);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 192.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_BG);
    let mut chosen = None;
    let mut toggled = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    egui::ScrollArea::vertical().id_salt("collections_tree").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        for row in &rows {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
            let selected = current == Some(row.id);
            if selected {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_SELECTED);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_HOVER);
            }
            let left = 4.0 + row.depth as f32 * 16.0;
            let arrow = Rect::from_min_size(Pos2::new(rect.left() + left, rect.top()), Vec2::new(16.0, 22.0));
            if row.parent {
                widgets::arrow(ui, arrow.center(), !collapsed.contains(&row.id), t::MUTED);
            }
            let text = Rect::from_min_max(Pos2::new(arrow.right() + 2.0, rect.top()), rect.max);
            let color = if selected { t::BRAND } else { t::VALUE };
            widgets::paint_left(ui, text, &row.text, t::font(t::FONT_SIZE), color);
            let on_arrow = row.parent && response.interact_pointer_pos().is_some_and(|p| arrow.contains(p));
            if response.clicked() {
                if on_arrow {
                    toggled = Some(row.id);
                } else if !selected {
                    chosen = Some(row.id);
                }
            }
        }
    });
    if let Some(id) = toggled {
        let collapsed = &mut cx.state::<CollectionsState>().collapsed;
        if !collapsed.remove(&id) {
            collapsed.insert(id);
        }
    }
    if let Some(id) = chosen {
        commands::collections::select(cx, Some(id));
    }
    let buttons = ["+", "스마트", "추가", "삭제"].map(tr);
    let ids = ["collections.new", "collections.new_smart", "collections.add", "collections.delete"];
    let row: Vec<Button<'_>> =
        buttons.iter().zip(ids).map(|(label, id)| Button::new(label).enabled(commands::enabled(cx, id))).collect();
    if let Some(i) = widgets::button_row(ui, &row, width, 6.0) {
        cx.run(ids[i]);
    }
}
