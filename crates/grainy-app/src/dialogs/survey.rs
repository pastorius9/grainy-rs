//! The survey of the selected photos (port of luma/manager.py `LibraryManager.compare`, 선택 사진 비교 / 모아보기):
//! up to twelve photos in a grid of two (up to four) or three columns, each with its name, its picture, 맞춤 and
//! 보기에서 제외. Non-modal; the pictures come from the second preview lane at 1800 pixels.

use std::path::PathBuf;

use eframe::egui::{self, Pos2, Rect, Vec2};
use grainy_core::Settings;

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::views::compare::{self, PictureView, SideLane};
use crate::widgets;

/// How many photos the menu item shows (`compare(12)`).
pub const LIMIT: usize = 12;
/// Long side of each picture (`load_image(path, 1800, …)`).
pub const SIZE: usize = 1800;
/// The second preview lane's first slot of the survey's cells.
const FIRST_SLOT: u64 = 16;

/// One cell of the grid.
pub struct Cell {
    pub id: u64,
    /// The label (`p['name']`, or 원본 / 보정 후 in the before/after form), Korean source for the latter.
    pub label: String,
    pub path: PathBuf,
    pub settings: Settings,
    /// 보기에서 제외 hid it.
    pub removed: bool,
    view: PictureView,
}

/// The survey window.
pub struct Survey {
    pub before: bool,
    pub cells: Vec<Cell>,
    asked: bool,
}

/// Port of `LibraryManager.compare(limit, before)`: commits the open photo, then shows the first `limit` selected
/// photos (with `before`, the open photo twice: the defaults and its edits). Replaces an open survey.
pub fn open(cx: &mut AppContext, limit: usize, before: bool) {
    cx.doc.edit_mut().commit("보정 변경");
    let Some(survey) = Survey::new(cx, limit, before) else { return };
    if let Some(old) = cx.dialogs.iter().position(|d| d.id() == "survey") {
        cx.dialogs.remove(old);
        release(cx);
    }
    cx.open_dialog(Box::new(survey));
}

fn release(cx: &mut AppContext) {
    let lane = cx.state::<SideLane>();
    for slot in FIRST_SLOT..FIRST_SLOT + LIMIT as u64 {
        lane.release(slot);
    }
}

impl Survey {
    /// The cells of `selected_photos()[:limit]`; `None` when nothing is selected.
    pub fn new(cx: &AppContext, limit: usize, before: bool) -> Option<Self> {
        let mut indices: Vec<usize> = cx.doc.selected_indices().into_iter().take(limit).collect();
        if indices.is_empty() {
            return None;
        }
        if before {
            let current = cx.doc.current_index()?;
            indices = vec![current, current];
        }
        let cells = indices
            .iter()
            .enumerate()
            .map(|(n, &index)| {
                let photo = &cx.doc.photos()[index];
                let label =
                    if before { (if n == 0 { "원본" } else { "보정 후" }).to_owned() } else { photo.name.clone() };
                let settings = if before && n == 0 { grainy_core::defaults() } else { cx.doc.settings_of(index) };
                Cell {
                    id: photo.id,
                    label,
                    path: photo.path.clone(),
                    settings,
                    removed: false,
                    view: PictureView::default(),
                }
            })
            .collect();
        Some(Self { before, cells, asked: false })
    }

    /// The window's title.
    pub fn title(&self) -> String {
        tr(if self.before { "전·후 비교" } else { "선택 사진 비교 / 모아보기" })
    }

    /// Columns of the grid (`2 if len(photos)<=4 else 3`).
    pub fn columns(&self) -> usize {
        if self.cells.len() <= 4 {
            2
        } else {
            3
        }
    }

    /// The label shown above cell `n`: its name, or the error when its picture cannot be read.
    pub fn label(&self, cx: &AppContext, n: usize) -> String {
        let cell = &self.cells[n];
        let failed = cx
            .state_ref::<SideLane>()
            .and_then(|l| l.slot(FIRST_SLOT + n as u64))
            .is_some_and(|s| s.answered && s.error.is_some());
        if failed {
            tr("원본을 읽을 수 없습니다.")
        } else if self.before {
            tr(&cell.label)
        } else {
            cell.label.clone()
        }
    }

    fn ask(&mut self, cx: &mut AppContext) {
        if self.asked {
            return;
        }
        self.asked = true;
        for (n, cell) in self.cells.iter().enumerate() {
            compare::request(cx, FIRST_SLOT + n as u64, cell.path.clone(), cell.settings.clone(), Some(SIZE));
        }
    }
}

impl Dialog for Survey {
    fn id(&self) -> &'static str {
        "survey"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        compare::pump(cx);
        self.ask(cx);
        let labels: Vec<String> = (0..self.cells.len()).map(|n| self.label(cx, n)).collect();
        let columns = self.columns();
        let mut open = true;
        let mut fit = None;
        let mut remove = None;
        egui::Window::new(self.title())
            .id(egui::Id::unique("survey_window"))
            .open(&mut open)
            .frame(super::frame().inner_margin(egui::Margin::same(10)))
            .default_size(Vec2::new(1200.0, 750.0))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let area = ui.available_rect_before_wrap();
                ui.allocate_rect(area, egui::Sense::hover());
                // A hidden cell takes no room: rows and columns without a visible cell close up (QGridLayout).
                let place = |n: usize| (n / columns, n % columns);
                let visible: Vec<usize> = (0..self.cells.len()).filter(|&n| !self.cells[n].removed).collect();
                let mut rows: Vec<usize> = visible.iter().map(|&n| place(n).0).collect();
                let mut cols: Vec<usize> = visible.iter().map(|&n| place(n).1).collect();
                rows.dedup();
                cols.sort_unstable();
                cols.dedup();
                let gap = 8.0;
                let cell_w = (area.width() - gap * (cols.len().max(1) - 1) as f32) / cols.len().max(1) as f32;
                let cell_h = (area.height() - gap * (rows.len().max(1) - 1) as f32) / rows.len().max(1) as f32;
                for &n in &visible {
                    let (row, col) = place(n);
                    let (Some(r), Some(c)) = (rows.iter().position(|&x| x == row), cols.iter().position(|&x| x == col))
                    else {
                        continue;
                    };
                    let min = area.min + Vec2::new(c as f32 * (cell_w + gap), r as f32 * (cell_h + gap));
                    let rect = Rect::from_min_size(min, Vec2::new(cell_w, cell_h));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    widgets::label(&mut child, &labels[n], t::TEXT, t::FONT_SIZE);
                    let top = child.cursor().top() + 4.0;
                    let picture_rect =
                        Rect::from_min_max(Pos2::new(rect.left(), top), Pos2::new(rect.right(), rect.bottom() - 38.0));
                    let slot = FIRST_SLOT + n as u64;
                    let picture =
                        cx.state_ref::<SideLane>().and_then(|l| l.slot(slot)).and_then(|s| s.picture.as_ref());
                    self.cells[n].view.show(&mut child, egui::Id::unique(("survey_cell", n)), picture_rect, picture);
                    let buttons = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 32.0), rect.max);
                    let mut row = child.new_child(
                        egui::UiBuilder::new()
                            .max_rect(buttons)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    row.spacing_mut().item_spacing.x = 0.0;
                    if widgets::button(&mut row, &tr("맞춤")).clicked() {
                        fit = Some(n);
                    }
                    if widgets::button(&mut row, &tr("보기에서 제외")).clicked() {
                        remove = Some(n);
                    }
                }
            });
        if let Some(n) = fit {
            self.cells[n].view.fit();
        }
        if let Some(n) = remove {
            self.cells[n].removed = true;
        }
        if !open {
            release(cx);
        }
        open
    }
}
