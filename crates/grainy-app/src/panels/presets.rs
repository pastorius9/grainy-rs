//! The Presets fold-out of the library column (luma/app.py `preset_tree`, `refresh_presets`, `preset_clicked`):
//! the film looks in a folder per maker (a folder per kind for makers with many films), then 기타 프리셋 with the
//! built-in presets, the user's presets and the Adobe presets (their report as tooltip). A click on a folder opens
//! it, on a film sets it (`change_film`), on a preset applies it.

use eframe::egui::{self, CornerRadius, Rect, Sense, Ui, Vec2};

use crate::commands::presets::{self, PresetState};
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button};

/// One row of the tree.
enum Row {
    Folder { key: String, title: String, depth: usize },
    Film { id: String, label: String, tip: String, depth: usize },
    Preset { name: String, tip: Option<String>, depth: usize },
}

impl Row {
    fn key(&self) -> String {
        match self {
            Row::Folder { key, .. } => format!("folder:{key}"),
            Row::Film { id, .. } => format!("film:{id}"),
            Row::Preset { name, .. } => format!("preset:{name}"),
        }
    }
}

/// The tree's rows for the folders that are open, as `refresh_presets` builds them.
fn rows(state: &PresetState) -> Vec<Row> {
    let mut rows = Vec::new();
    for (maker, kinds) in grainy_film::by_maker() {
        let total: usize = kinds.iter().map(|(_, group)| group.len()).sum();
        let top = maker.clone();
        let open = state.open.contains(&top);
        rows.push(Row::Folder { key: top, title: format!("{maker} ({total})"), depth: 0 });
        if !open {
            continue;
        }
        for (title, group) in kinds {
            let depth = if title.is_some() { 2 } else { 1 };
            if let Some(title) = title {
                let key = format!("{maker}/{title}");
                let open = state.open.contains(&key);
                rows.push(Row::Folder { key, title: format!("{} ({})", tr(title), group.len()), depth: 1 });
                if !open {
                    continue;
                }
            }
            for entry in group {
                rows.push(Row::Film {
                    id: entry.id.clone(),
                    label: grainy_film::short_name(&entry.maker, &entry.name),
                    tip: presets::film_tooltip(entry),
                    depth,
                });
            }
        }
    }
    let all = presets::all_presets(&state.user);
    let open = state.open.contains("others");
    rows.push(Row::Folder {
        key: "others".into(), title: format!("{} ({})", tr("기타 프리셋"), all.len()), depth: 0
    });
    if open {
        for (name, preset) in all {
            let tip = preset
                .get(presets::KEY)
                .map(|record| record.get("report").and_then(|r| r.as_str()).unwrap_or("Adobe 프리셋").to_owned());
            rows.push(Row::Preset { name, tip, depth: 1 });
        }
    }
    rows
}

/// What a click did.
enum Click {
    Folder(String),
    Film(String),
    Preset(String),
}

/// The fold-out's body: the tree (260 px high) and the save button. The library column calls it with the
/// column's width.
pub fn body(cx: &mut AppContext, ui: &mut Ui, width: f32) {
    presets::ensure_loaded(cx);
    let mut clicked = None;
    let mut chosen = None;
    {
        let state = cx.state::<PresetState>();
        let rows = rows(state);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 260.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_BG);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for row in &rows {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                let selected = state.selected.as_deref() == Some(row.key().as_str());
                if selected {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_SELECTED);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, t::LIST_HOVER);
                }
                let (depth, label, folder_open) = match row {
                    Row::Folder { key, title, depth } => (*depth, title.as_str(), Some(state.open.contains(key))),
                    Row::Film { label, depth, .. } => (*depth, label.as_str(), None),
                    Row::Preset { name, depth, .. } => (*depth, name.as_str(), None),
                };
                let left = 4.0 + depth as f32 * 14.0;
                if let Some(open) = folder_open {
                    widgets::arrow(ui, egui::pos2(rect.left() + left + 5.0, rect.center().y), open, t::VALUE);
                }
                let text = Rect::from_min_max(egui::pos2(rect.left() + left + 14.0, rect.top()), rect.max);
                let color = if selected { t::BRAND } else { t::VALUE };
                widgets::paint_left(ui, text, label, t::font(t::FONT_SIZE), color);
                let tip = match row {
                    Row::Film { tip, .. } => Some(tip.as_str()),
                    Row::Preset { tip, .. } => tip.as_deref(),
                    Row::Folder { .. } => None,
                };
                let response = match tip {
                    Some(tip) => response.on_hover_text(tip),
                    None => response,
                };
                if response.clicked() {
                    clicked = Some(match row {
                        Row::Folder { key, .. } => Click::Folder(key.clone()),
                        Row::Film { id, .. } => Click::Film(id.clone()),
                        Row::Preset { name, .. } => Click::Preset(name.clone()),
                    });
                    // Qt selects the clicked item.
                    chosen = Some(row.key());
                }
            }
        });
    }
    if chosen.is_some() {
        cx.state::<PresetState>().selected = chosen;
    }
    match clicked {
        Some(Click::Folder(key)) => {
            let open = &mut cx.state::<PresetState>().open;
            if !open.remove(&key) {
                open.insert(key);
            }
        }
        Some(Click::Film(id)) => presets::film(cx, &id),
        Some(Click::Preset(name)) => presets::apply(cx, &name),
        None => {}
    }
    if Button::new(&tr("현재 보정을 프리셋으로 저장")).width(width).show(ui).clicked() {
        cx.run("presets.save");
    }
}
