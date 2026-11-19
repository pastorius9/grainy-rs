//! 계층형 키워드: the library's keywords as a tree (`LibraryManager.keyword_tree`). Keywords are joined with `|`
//! (`여행|일본`); a node shows its last part and how many photos are under it. A double click filters the list to
//! that keyword. A window beside the main one (it does not block it).

use std::collections::BTreeSet;

use eframe::egui::{self, CornerRadius, Rect, Sense, Vec2};

use super::Dialog;
use crate::commands::metadata as commands;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets;

/// One node: the full `|` path, how many photos are under it and its depth.
struct Node {
    path: String,
    count: usize,
    depth: usize,
}

/// The keyword tree window.
pub struct KeywordTree {
    nodes: Vec<Node>,
    /// The nodes folded shut (the oracle expands all of them at first).
    folded: BTreeSet<String>,
    selected: Option<String>,
    open: bool,
}

impl KeywordTree {
    /// The tree of `groups` (every path with its photo count: `commands::keyword_groups`). The rows come in the
    /// tree's order: a keyword, then what is under it, siblings by name (the oracle adds sorted paths to a
    /// `QTreeWidget` under their parents).
    pub fn new(mut groups: Vec<(String, usize)>) -> Self {
        groups.sort_by(|a, b| a.0.split('|').cmp(b.0.split('|')));
        let nodes =
            groups.into_iter().map(|(path, count)| Node { depth: path.matches('|').count(), path, count }).collect();
        Self { nodes, folded: BTreeSet::new(), selected: None, open: true }
    }

    /// The rows shown (folded keywords hide what is under them): (path, name, photos, depth).
    pub fn rows(&self) -> Vec<(String, String, usize, usize)> {
        self.nodes
            .iter()
            .filter(|n| !self.hidden(&n.path))
            .map(|n| (n.path.clone(), n.path.rsplit('|').next().unwrap_or("").to_owned(), n.count, n.depth))
            .collect()
    }

    /// Whether a folded ancestor hides `path`.
    fn hidden(&self, path: &str) -> bool {
        self.folded
            .iter()
            .any(|f| path.len() > f.len() && path.starts_with(f.as_str()) && path[f.len()..].starts_with('|'))
    }

    fn has_children(&self, path: &str) -> bool {
        self.nodes
            .iter()
            .any(|n| n.path.len() > path.len() && n.path.starts_with(path) && n.path[path.len()..].starts_with('|'))
    }
}

impl Dialog for KeywordTree {
    fn id(&self) -> &'static str {
        "keyword_tree"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut chosen = None;
        let mut toggled = None;
        let mut open = self.open;
        egui::Window::new(tr("계층형 키워드"))
            .open(&mut open)
            .frame(super::frame())
            .default_size(Vec2::new(480.0, 580.0))
            .default_pos(ctx.content_rect().center() - Vec2::new(240.0, 290.0))
            .show(ctx, |ui| {
                let width = ui.available_width();
                // The header: 키워드 | 사진.
                let (head, _) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
                widgets::paint_left(
                    ui,
                    head.shrink2(Vec2::new(6.0, 0.0)),
                    &tr("키워드"),
                    t::bold(t::FONT_SIZE),
                    t::MUTED,
                );
                ui.painter().text(
                    egui::pos2(head.right() - 8.0, head.center().y),
                    egui::Align2::RIGHT_CENTER,
                    tr("사진"),
                    t::bold(t::FONT_SIZE),
                    t::MUTED,
                );
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(480.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let visible: Vec<usize> =
                        (0..self.nodes.len()).filter(|i| !self.hidden(&self.nodes[*i].path)).collect();
                    for index in visible {
                        let node = &self.nodes[index];
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                        let selected = self.selected.as_deref() == Some(node.path.as_str());
                        if selected {
                            ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_SELECTED);
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_HOVER);
                        }
                        let left = 6.0 + node.depth as f32 * 16.0;
                        if self.has_children(&node.path) {
                            let folded = self.folded.contains(&node.path);
                            widgets::arrow(
                                ui,
                                egui::pos2(rect.left() + left + 5.0, rect.center().y),
                                !folded,
                                t::VALUE,
                            );
                            let arrow = Rect::from_min_size(
                                egui::pos2(rect.left() + left - 2.0, rect.top()),
                                Vec2::new(16.0, rect.height()),
                            );
                            if response.clicked() && response.interact_pointer_pos().is_some_and(|p| arrow.contains(p))
                            {
                                toggled = Some(node.path.clone());
                            }
                        }
                        let name = node.path.rsplit('|').next().unwrap_or("");
                        let text = Rect::from_min_max(egui::pos2(rect.left() + left + 16.0, rect.top()), rect.max);
                        let color = if selected { t::BRAND } else { t::VALUE };
                        widgets::paint_left(ui, text, name, t::font(t::FONT_SIZE), color);
                        ui.painter().text(
                            egui::pos2(rect.right() - 8.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            node.count.to_string(),
                            t::font(t::FONT_SIZE),
                            color,
                        );
                        if response.clicked() {
                            self.selected = Some(node.path.clone());
                        }
                        if response.double_clicked() {
                            chosen = Some(node.path.clone());
                        }
                    }
                });
                ui.add_space(8.0);
                widgets::muted(ui, &tr("더블클릭으로 사진 필터 · 사진 관리 → 메타데이터 편집에서 키워드 수정"));
            });
        self.open = open;
        if let Some(path) = toggled {
            if !self.folded.remove(&path) {
                self.folded.insert(path);
            }
        }
        if let Some(path) = chosen {
            commands::filter_keyword(cx, &path);
        }
        self.open
    }
}
