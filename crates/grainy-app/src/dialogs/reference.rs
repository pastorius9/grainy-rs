//! Secondary photo windows (port of luma/reference_view.py `ReferenceWindow`): 참조 사진 keeps the open photo as it
//! was when the window opened; 두 번째 화면 follows the open photo, showing its edits once they are committed.
//! Both are non-modal and render through the second preview lane (`render_photo`).

use std::path::PathBuf;

use eframe::egui::{self, Rect, Vec2};
use grainy_core::Settings;

use super::Dialog;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::views::compare::{self, PictureView, SideLane};
use crate::widgets;

/// `PREVIEW_SIZE` of luma/reference_view.py.
pub const PREVIEW_SIZE: usize = 1800;
/// The second preview lane's slots of the two windows.
const FIXED_SLOT: u64 = 2;
const FOLLOW_SLOT: u64 = 3;

/// A photo as the window renders it (a catalogue record: id, path, name, settings).
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub settings: Settings,
}

/// The window.
pub struct ReferenceWindow {
    /// The snapshot shown (참조 사진), or `None` for the window that follows the open photo.
    reference: Option<Record>,
    /// The record rendered last (`shown_key`).
    shown: Option<Record>,
    view: PictureView,
}

impl ReferenceWindow {
    /// 참조 사진: the open photo with its settings now (`reference=deepcopy(catalog.photo(current_id))`).
    pub fn fixed(cx: &AppContext) -> Option<Self> {
        let record = current_record(cx)?;
        Some(Self { reference: Some(record), shown: None, view: PictureView::default() })
    }

    /// 두 번째 화면: follows the open photo.
    pub fn following() -> Self {
        Self { reference: None, shown: None, view: PictureView::default() }
    }

    /// The dialog id of this kind of window.
    pub fn id_str(&self) -> &'static str {
        if self.reference.is_some() {
            "reference"
        } else {
            "second_window"
        }
    }

    fn slot(&self) -> u64 {
        if self.reference.is_some() {
            FIXED_SLOT
        } else {
            FOLLOW_SLOT
        }
    }

    /// The window's title (`setWindowTitle`).
    pub fn title(&self) -> String {
        match &self.reference {
            Some(record) => format!("{} · {}", tr("참조 사진"), record.name),
            None => tr("두 번째 화면"),
        }
    }

    /// Port of `sync` and `show_record`: asks for the picture when the record changed.
    pub fn sync(&mut self, cx: &mut AppContext) {
        let record = match &self.reference {
            Some(record) => Some(record.clone()),
            // The committed state: a value being dragged shows once it is committed.
            None if cx.doc.edit().dirty() => self.shown.clone(),
            None => current_record(cx),
        };
        let Some(record) = record else {
            self.shown = None;
            return;
        };
        if self.shown.as_ref() != Some(&record) {
            compare::request(cx, self.slot(), record.path.clone(), record.settings.clone(), Some(PREVIEW_SIZE));
            self.shown = Some(record);
        }
    }

    /// The label above the picture: the name, with 불러오는 중… or the error while there is no picture.
    pub fn label(&self, cx: &AppContext) -> String {
        let Some(record) = &self.shown else { return tr("표시할 사진이 없습니다.") };
        let slot = cx.state_ref::<SideLane>().and_then(|l| l.slot(self.slot()));
        match slot {
            Some(s) if s.answered && s.error.is_some() => {
                format!("{}  ·  {}", record.name, tr("원본을 읽을 수 없습니다."))
            }
            Some(s) if s.answered => record.name.clone(),
            _ => format!("{}  ·  {}", record.name, tr("불러오는 중…")),
        }
    }
}

/// `catalog.photo(current_id)`: the open photo with its settings.
fn current_record(cx: &AppContext) -> Option<Record> {
    let index = cx.doc.current_index()?;
    let photo = &cx.doc.photos()[index];
    Some(Record {
        id: photo.id,
        path: photo.path.clone(),
        name: photo.name.clone(),
        settings: cx.doc.settings_of(index),
    })
}

impl Dialog for ReferenceWindow {
    fn id(&self) -> &'static str {
        self.id_str()
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        compare::pump(cx);
        self.sync(cx);
        let label = self.label(cx);
        let fit_text = tr("맞춤");
        let title = self.title();
        let mut open = true;
        let mut fit = false;
        let slot = self.slot();
        let draw_body = |ui: &mut egui::Ui, view: &mut PictureView, cx: &mut AppContext, fit: &mut bool| {
            widgets::label(ui, &label, t::TEXT, t::FONT_SIZE);
            // Below the label, one rectangle of exactly the size that is left: the picture, then the button
            // row at its bottom.
            let area = Rect::from_min_size(ui.cursor().min, ui.available_size());
            ui.allocate_rect(area, egui::Sense::hover());
            let fit_button = widgets::Button::new(&fit_text);
            let row = fit_button.size(ui).y;
            let bottom = (area.max.y - row - 6.0).max(area.min.y + 80.0);
            let picture_rect = Rect::from_min_max(area.min, egui::pos2(area.max.x, bottom));
            let picture = cx.state_ref::<SideLane>().and_then(|l| l.slot(slot)).and_then(|s| s.picture.as_ref());
            view.show(ui, egui::Id::unique(("reference_view", slot)), picture_rect, picture);
            let row_rect = Rect::from_min_max(egui::pos2(area.min.x, area.max.y - row), area.max);
            let mut row_ui = ui.new_child(
                egui::UiBuilder::new().max_rect(row_rect).layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            *fit = fit_button.show(&mut row_ui).clicked();
        };

        // Native OS secondary window (draggable across dual monitors on Windows & macOS);
        // falls back to an embedded egui::Window in headless kittest snapshots.
        let use_native_viewport = !crate::context::FROZEN_TIMING.load(std::sync::atomic::Ordering::Relaxed);
        if use_native_viewport {
            let viewport_id = egui::ViewportId::from_hash_of(("reference_viewport", slot));
            let builder = egui::ViewportBuilder::default()
                .with_title(title.clone())
                .with_inner_size(Vec2::new(1000.0, 720.0))
                .with_resizable(true);
            ctx.show_viewport_immediate(viewport_id, builder, |vctx, class| {
                if class == egui::ViewportClass::Immediate {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(t::BG).inner_margin(egui::Margin::same(10)))
                        .show(vctx, |ui| draw_body(ui, &mut self.view, cx, &mut fit));
                    if vctx.input(|i| i.viewport().close_requested()) {
                        open = false;
                    }
                } else {
                    egui::Window::new(&title)
                        .id(egui::Id::unique(("reference_window", slot)))
                        .open(&mut open)
                        .frame(super::frame().inner_margin(egui::Margin::same(10)))
                        .default_size(Vec2::new(1000.0, 720.0))
                        .resizable(true)
                        .collapsible(false)
                        .show(vctx, |ui| draw_body(ui, &mut self.view, cx, &mut fit));
                }
            });
        } else {
            egui::Window::new(title)
                .id(egui::Id::unique(("reference_window", slot)))
                .open(&mut open)
                .frame(super::frame().inner_margin(egui::Margin::same(10)))
                .default_size(Vec2::new(1000.0, 720.0))
                .resizable(true)
                .collapsible(false)
                .show(ctx, |ui| draw_body(ui, &mut self.view, cx, &mut fit));
        }
        if fit {
            self.view.fit();
        }
        if !open {
            cx.state::<SideLane>().release(slot);
        }
        open
    }
}
