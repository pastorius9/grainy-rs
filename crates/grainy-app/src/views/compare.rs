//! Two pictures side by side in the Develop screen (port of luma/compare_view.py `CompareLayout`): the open photo
//! before and after its edits, or the open photo beside another one. The main view stays what it is (the open
//! photo, with every tool); a second picture is put next to it and follows the main view's zoom and position. In
//! the two-photo mode the open photo carries a white frame and a click on the other picture makes that photo the
//! open one; a double click on either picture ends the comparison and shows that photo.
//!
//! The second picture (and the survey's and reference windows' pictures) come from the render worker's second
//! preview lane ([`crate::render::Renderer::side`]); [`SideLane`] routes its answers to the views that asked.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use grainy_core::Settings;

use crate::context::{AppContext, Mode, Zoom};
use crate::panels::Tool;
use crate::render::SideJob;
use crate::text::tr;
use crate::theme as t;
use crate::tools::CanvasTool;
use crate::widgets::{self, Button};

/// Long side of the second picture until the main view is zoomed in (`SIZE`).
pub const SIZE: usize = 1800;
/// The second preview lane's slot of the compare view's second picture.
pub const SLOT: u64 = 1;
/// The share of a view a fitted picture fills (as the photo view).
const FIT_MARGIN: f32 = 0.96;
/// The closest zoom of a picture view: 16 screen pixels per photo pixel.
const MAX_ZOOM: f32 = 16.0;
/// Height of the compare bar (`setContentsMargins(12,8,12,8)` around 30-pixel controls).
const BAR_HEIGHT: f32 = 46.0;
/// Height of a pane's caption (`setContentsMargins(8,5,8,5)` around a 12-pixel label).
const CAPTION_HEIGHT: f32 = 26.0;

/// The keys `before_settings` keeps: `working_space` and the groups 크롭·회전 and 렌즈·원근 of
/// `manager.EDIT_GROUPS` (a private copy: grainy-service keeps its table private).
const KEPT: [&str; 28] = [
    "working_space",
    "crop",
    "rotation",
    "straighten",
    "flip",
    "distortion",
    "distortion_k2",
    "distortion_k3",
    "lens_vignette",
    "ca_red",
    "ca_blue",
    "defringe",
    "lens_profile",
    "perspective_v",
    "perspective_h",
    "aspect_scale",
    "shift_x",
    "shift_y",
    "transform_scale",
    "guides",
    "lensfun",
    "lensfun_enabled",
    "lensfun_distortion",
    "lensfun_tca",
    "lensfun_vignette",
    "lensfun_scale",
    "upright",
    "upright_crop",
];

/// Port of luma/compare_view.py `before_settings`: the photo as it was before its edits, in the same frame (the
/// crop, the rotation and the lens geometry are kept so both sides show the same part of the picture).
pub fn before_settings(settings: &Settings) -> Settings {
    let mut plain = grainy_core::defaults();
    for key in KEPT {
        if let Some(value) = settings.0.get(key) {
            plain.0.insert(key.to_owned(), value.clone());
        }
    }
    plain
}

/// The comparison's two modes (`MODES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompareMode {
    /// 보정 전·후: the open photo before and after its edits.
    #[default]
    Before,
    /// 여러 이미지: the open photo beside another one.
    Photos,
}

impl CompareMode {
    /// The oracle's key (`before`, `photos`).
    pub fn key(self) -> &'static str {
        match self {
            CompareMode::Before => "before",
            CompareMode::Photos => "photos",
        }
    }
}

/// The modes as the choice box lists them: (title, mode).
pub const MODES: [(&str, CompareMode); 2] = [("보정 전·후", CompareMode::Before), ("여러 이미지", CompareMode::Photos)];

/// The photo seen before the open one (`workflow.previous_id`, set by `MainWindow.activate`).
#[derive(Debug, Default)]
pub struct Previous {
    pub id: Option<u64>,
}

/// Hook of [`AppContext::select_photo`]: another photo was opened while `leaving` was open (`activate`).
pub(crate) fn left(cx: &mut AppContext, leaving: u64) {
    if leaving != 0 && leaving != cx.doc.current_id() {
        cx.state::<Previous>().id = Some(leaving);
    }
}

/// What the second picture is rendered from (the oracle's `shown` key: mode, record, settings, full size).
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub mode: CompareMode,
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub settings: Settings,
    /// Rendered from the full-size file (the main view is zoomed in).
    pub full: bool,
}

/// The compare view's state (`CompareLayout`'s attributes).
#[derive(Debug, Default)]
pub struct Compare {
    pub on: bool,
    pub mode: CompareMode,
    /// 상하: one above the other.
    pub vertical: bool,
    /// 바꾸기: the two pictures change places.
    pub swapped: bool,
    /// The photo beside the open one in the two-photo mode.
    pub other_id: Option<u64>,
    pub shown: Option<Shown>,
    /// The second picture is rendered at full size (`sharpen`).
    pub full: bool,
    /// How many renders of the second picture were asked for (`show` calls).
    pub renders: u64,
    /// The second picture could not be read.
    pub failed: bool,
    /// The main pane's photo area at the last frame (the second picture follows it).
    main_area: Option<Rect>,
}

impl Compare {
    /// Whether the main pane comes first (left or top): before/after reads left to right (the original first);
    /// with two photos the open one comes first. 바꾸기 turns it round.
    pub fn main_first(&self) -> bool {
        (self.mode == CompareMode::Before) == self.swapped
    }

    /// Whether the open photo carries the white frame (the two-photo mode).
    pub fn marked(&self) -> bool {
        self.mode == CompareMode::Photos
    }
}

/// Port of `toggle` (the 비교 button and C): ends the comparison, or starts it with two photos when several are
/// selected, otherwise in the mode used last.
pub fn toggle(cx: &mut AppContext) {
    if cx.state::<Compare>().on {
        stop(cx, false);
    } else {
        let several = cx.doc.selected_indices().len() > 1;
        start(cx, several.then_some(CompareMode::Photos));
    }
}

/// Port of `start`: shows the comparison in `mode`, or in the one used last.
pub fn start(cx: &mut AppContext, mode: Option<CompareMode>) {
    if cx.doc.current().is_none() {
        cx.compare = false;
        return;
    }
    cx.set_mode(Mode::Develop);
    let full = cx.zoom != Zoom::Fit;
    let state = cx.state::<Compare>();
    if let Some(mode) = mode {
        state.mode = mode;
        state.other_id = None;
        state.swapped = false;
    }
    state.on = true;
    state.shown = None;
    state.full = full;
    cx.compare = true;
    sync(cx);
}

/// Port of `stop`: back to one picture, the open photo or, with `show_other`, the other one.
pub fn stop(cx: &mut AppContext, show_other: bool) {
    let state = cx.state::<Compare>();
    if !state.on {
        return;
    }
    let other = if state.mode == CompareMode::Photos && show_other { state.other_id } else { None };
    state.on = false;
    state.shown = None;
    state.full = false;
    state.failed = false;
    cx.state::<SideLane>().clear(SLOT);
    cx.compare = false;
    if let Some(other) = other {
        select(cx, other);
    }
    cx.set_zoom(Zoom::Fit);
}

/// Port of `set_mode` (the 비교 방식 choice).
pub fn set_mode(cx: &mut AppContext, mode: CompareMode) {
    let state = cx.state::<Compare>();
    if mode == state.mode || !state.on {
        return;
    }
    state.mode = mode;
    state.swapped = false;
    state.shown = None;
    sync(cx);
}

/// Port of `set_vertical` (좌우 / 상하).
pub fn set_vertical(cx: &mut AppContext, vertical: bool) {
    cx.state::<Compare>().vertical = vertical;
}

/// Port of `swap` (바꾸기).
pub fn swap(cx: &mut AppContext) {
    let state = cx.state::<Compare>();
    state.swapped = !state.swapped;
}

/// Port of `pick`: the photo beside the open one: the one already there, another selected photo, the photo seen
/// before this one, or its neighbour in the list.
pub fn pick(cx: &mut AppContext) -> Option<u64> {
    let current = cx.doc.current_id();
    let usable = |cx: &AppContext, id: Option<u64>| id.filter(|&i| i != current && cx.doc.index_of(i).is_some());
    let other = cx.state::<Compare>().other_id;
    if let Some(id) = usable(cx, other) {
        return Some(id);
    }
    let ids: Vec<u64> = cx.doc.photos().iter().map(|p| p.id).collect();
    let place = ids.iter().position(|&i| i == current);
    let near = match place {
        Some(p) if p + 1 < ids.len() => Some(ids[p + 1]),
        Some(p) if p > 0 => Some(ids[p - 1]),
        _ => None,
    };
    let selected: Vec<u64> = cx.doc.selected_indices().into_iter().filter_map(|i| ids.get(i).copied()).collect();
    let previous = cx.state::<Previous>().id;
    let candidates = selected.into_iter().map(Some).chain([previous, near]);
    let chosen = candidates.into_iter().find_map(|id| usable(cx, id));
    cx.state::<Compare>().other_id = chosen;
    chosen
}

/// Port of `select`: opens photo `id` of the list (it becomes the only selected one).
fn select(cx: &mut AppContext, id: u64) {
    if let Some(index) = cx.doc.index_of(id) {
        cx.select_photo(index);
    }
}

/// Port of `activate_other`: the other photo becomes the open one; both stay where they are on screen.
pub fn activate_other(cx: &mut AppContext) {
    let current = cx.doc.current_id();
    let state = cx.state::<Compare>();
    let Some(chosen) = state.other_id.filter(|_| state.on && state.mode == CompareMode::Photos) else { return };
    state.other_id = Some(current);
    state.swapped = !state.swapped;
    state.shown = None;
    select(cx, chosen);
    sync(cx);
}

/// Port of `sync` (run every frame while the comparison shows; the oracle's 400 ms timer): keeps the second
/// picture and the captions in step with the main view.
pub fn sync(cx: &mut AppContext) {
    if !cx.state::<Compare>().on {
        return;
    }
    let Some(current) = cx.doc.current().cloned() else {
        stop(cx, false);
        return;
    };
    // `sharpen`: once the main view is zoomed in, the second picture is rendered at full size.
    if cx.zoom != Zoom::Fit {
        cx.state::<Compare>().full = true;
    }
    let mut record = None;
    if cx.state::<Compare>().mode == CompareMode::Photos {
        match pick(cx).and_then(|id| cx.doc.index_of(id)) {
            None => {
                cx.state::<Compare>().mode = CompareMode::Before;
                cx.set_status(tr("비교할 다른 사진이 없어 보정 전·후로 표시합니다."));
            }
            Some(index) => {
                let photo = &cx.doc.photos()[index];
                record = Some((photo.id, photo.path.clone(), photo.name.clone(), cx.doc.settings_of(index)));
            }
        }
    }
    let (id, path, name, settings) = match record {
        Some(record) => record,
        None => (current.id, current.path.clone(), current.name.clone(), before_settings(cx.settings())),
    };
    let state = cx.state::<Compare>();
    let key = Shown { mode: state.mode, id, path, name, settings, full: state.full };
    if state.shown.as_ref() != Some(&key) {
        show(cx, key);
    }
}

/// Port of `show`: asks the second preview lane for the picture.
fn show(cx: &mut AppContext, key: Shown) {
    let state = cx.state::<Compare>();
    let other_record = state.shown.as_ref().is_none_or(|s| s.id != key.id);
    state.renders += 1;
    state.failed = false;
    let max_size = if key.full { None } else { Some(SIZE) };
    let (path, settings) = (key.path.clone(), key.settings.clone());
    state.shown = Some(key);
    if other_record {
        cx.state::<SideLane>().clear(SLOT);
    }
    request(cx, SLOT, path, settings, max_size);
}

/// The captions of the main and the other picture (`arrange`): 보정 후 / 원본, or the photos' names.
pub fn captions(cx: &AppContext) -> (String, String) {
    let Some(state) = cx.state_ref::<Compare>() else { return (String::new(), String::new()) };
    if !state.on {
        return (String::new(), String::new());
    }
    let current = cx.doc.current().map(|p| p.name.clone()).unwrap_or_default();
    let (main, other) = match state.mode {
        CompareMode::Before => (tr("보정 후"), tr("원본")),
        CompareMode::Photos => (current, state.shown.as_ref().map(|s| s.name.clone()).unwrap_or_default()),
    };
    let other = if state.failed {
        let name = state.shown.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        format!("{name}  ·  {}", tr("원본을 읽을 수 없습니다."))
    } else {
        other
    };
    (main, other)
}

/// Port of `eventFilter`'s double click: on the second picture it ends the comparison showing that photo; on the
/// main picture it ends it unless a tool uses the pointer there.
pub fn double_click(cx: &mut AppContext, second: bool) {
    if !cx.state::<Compare>().on {
        return;
    }
    if second || (cx.tool() == Tool::Edit && !cx.wb_picker) {
        stop(cx, second);
    }
}

// ---- the second preview lane's client -------------------------------------------------------------------------

/// A picture of the second lane on screen.
pub struct Picture {
    pub texture: egui::TextureHandle,
    /// Its size in original pixels.
    pub size: Vec2,
    /// Its size in picture pixels.
    pub pixels: [usize; 2],
}

/// One view's request and answer.
#[derive(Default)]
pub struct Slot {
    /// The newest request id (answers to older ones are dropped).
    pub asked: u64,
    pub picture: Option<Picture>,
    pub error: Option<String>,
    /// The newest request was answered.
    pub answered: bool,
}

/// The second preview lane's answers by slot (the compare view, the survey's cells, the reference windows).
#[derive(Default)]
pub struct SideLane {
    requests: u64,
    slots: HashMap<u64, Slot>,
}

impl SideLane {
    /// The view of `slot`, when it asked for something.
    pub fn slot(&self, slot: u64) -> Option<&Slot> {
        self.slots.get(&slot)
    }

    /// Clears the picture of `slot` (a different photo is coming: `clear_photo(loading=True)`).
    pub fn clear(&mut self, slot: u64) {
        if let Some(entry) = self.slots.get_mut(&slot) {
            entry.picture = None;
            entry.error = None;
        }
    }

    /// Forgets `slot` (its view closed); its answers still on the way are dropped.
    pub fn release(&mut self, slot: u64) {
        self.slots.remove(&slot);
    }
}

/// Asks the second preview lane for `path` developed with `settings` for `slot`; returns the request id.
pub fn request(cx: &mut AppContext, slot: u64, path: PathBuf, settings: Settings, max_size: Option<usize>) -> u64 {
    let lane = cx.state::<SideLane>();
    lane.requests += 1;
    let id = lane.requests;
    let entry = lane.slots.entry(slot).or_default();
    entry.asked = id;
    entry.answered = false;
    cx.renderer.side(SideJob { slot, id, path, settings, max_size });
    id
}

/// Collects the second lane's answers into their slots (every view calls it before drawing).
pub fn pump(cx: &mut AppContext) {
    for frame in cx.renderer.poll_side() {
        let wanted = cx.state::<SideLane>().slots.get(&frame.slot).is_some_and(|s| s.asked == frame.id);
        if !wanted {
            continue;
        }
        let (picture, error) = match frame.result {
            Ok(preview) if preview.rgba.len() == preview.width * preview.height * 4 => {
                let image = egui::ColorImage::from_rgba_unmultiplied([preview.width, preview.height], &preview.rgba);
                let name = format!("side-{}", frame.slot);
                let texture = cx.egui.load_texture(name, image, egui::TextureOptions::LINEAR);
                let size = Vec2::new(preview.width as f32, preview.height as f32) * frame.scale as f32;
                (Some(Picture { texture, size, pixels: [preview.width, preview.height] }), None)
            }
            Ok(_) => (None, Some("wrong picture size".to_owned())),
            Err(message) => (None, Some(message)),
        };
        let entry = cx.state::<SideLane>().slots.entry(frame.slot).or_default();
        entry.answered = true;
        if picture.is_some() {
            entry.picture = picture;
        }
        entry.error = error;
        if frame.slot == SLOT && entry.error.is_some() {
            cx.state::<Compare>().failed = true;
        }
    }
}

// ---- drawing ---------------------------------------------------------------------------------------------------

/// Where a picture of `size` original pixels is drawn in `area` to show the same part, at the same relative size,
/// as the main view (`follow`): `main_area` is the main view, `main_rect` where its photo was drawn.
pub fn follow(fit: bool, main_area: Rect, main_rect: Rect, area: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return Rect::from_center_size(area.center(), Vec2::ZERO);
    }
    if fit || main_rect.width() <= 0.0 || main_rect.height() <= 0.0 {
        let scale = (area.width() / size.x).min(area.height() / size.y) * FIT_MARGIN;
        return Rect::from_center_size(area.center(), size * scale);
    }
    let shown = Vec2::new(main_rect.width(), main_rect.width() * size.y / size.x);
    let centre = main_area.center();
    let fx = (centre.x - main_rect.left()) / main_rect.width();
    let fy = (centre.y - main_rect.top()) / main_rect.height();
    let place = |centre: f32, low: f32, high: f32, length: f32, fraction: f32| {
        if length > high - low {
            (centre - fraction * length).clamp(high - length, low)
        } else {
            centre - length / 2.0
        }
    };
    let x = place(area.center().x, area.left(), area.right(), shown.x, fx);
    let y = place(area.center().y, area.top(), area.bottom(), shown.y, fy);
    Rect::from_min_size(Pos2::new(x, y), shown)
}

/// A picture view of the survey and the reference windows (a `PhotoView`): fitted, Ctrl+wheel zooms around the
/// pointer, a drag moves a zoomed picture, 맞춤 fits it again.
#[derive(Debug, Default, Clone, Copy)]
pub struct PictureView {
    /// Screen points per original pixel; `None` fits the picture.
    pub zoom: Option<f32>,
    pub pan: Vec2,
}

impl PictureView {
    /// Port of `fit_photo`.
    pub fn fit(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }

    /// Draws `picture` in `rect` (the window background where there is none yet).
    pub fn show(&mut self, ui: &mut Ui, id: egui::Id, rect: Rect, picture: Option<&Picture>) -> egui::Response {
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, t::WINDOW);
        let Some(picture) = picture else { return response };
        let size = picture.size;
        let fit = (rect.width() / size.x).min(rect.height() / size.y) * FIT_MARGIN;
        let mut scale = self.zoom.unwrap_or(fit);
        let (zoom_delta, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if response.hovered() && (zoom_delta - 1.0).abs() > 1e-4 {
            if let Some(pointer) = pointer {
                let ppp = ui.ctx().pixels_per_point();
                let new = (scale * zoom_delta).clamp(fit.min(1.0 / ppp), MAX_ZOOM / ppp);
                let centre = rect.center() + self.pan;
                let at = (pointer - centre) / scale;
                self.pan = pointer - at * new - rect.center();
                self.zoom = Some(new);
                scale = new;
            }
        }
        if self.zoom.is_some() && response.dragged() {
            self.pan += response.drag_delta();
        }
        let shown = size * scale;
        let limit = ((shown - rect.size()) / 2.0).max(Vec2::ZERO);
        self.pan = self.pan.clamp(-limit, limit);
        let target = Rect::from_center_size(rect.center() + self.pan, shown);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(picture.texture.id(), target, uv, Color32::WHITE);
        response
    }
}

/// The centre of the Develop screen: the photo view, or the comparison (the bar and two panes) while it is on.
pub(crate) fn photo_area(cx: &mut AppContext, ui: &mut Ui, tools: &mut [Box<dyn CanvasTool>]) {
    if cx.state_ref::<Compare>().is_none_or(|s| !s.on) {
        cx.photo_view(ui, tools);
        return;
    }
    pump(cx);
    sync(cx);
    if !cx.state::<Compare>().on {
        cx.photo_view(ui, tools);
        return;
    }
    let area = ui.max_rect();
    ui.painter().rect_filled(area, CornerRadius::ZERO, t::BG);
    let mut panes = area;
    if cx.bars {
        let bar = Rect::from_min_size(area.min, Vec2::new(area.width(), BAR_HEIGHT));
        panes.min.y = bar.bottom();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(Vec2::new(12.0, 8.0))));
        compare_bar(&mut child, cx);
    }
    let state = cx.state::<Compare>();
    let (vertical, main_first, marked) = (state.vertical, state.main_first(), state.marked());
    let spacing = 6.0;
    let (first, second) = if vertical {
        let h = (panes.height() - spacing) / 2.0;
        (
            Rect::from_min_size(panes.min, Vec2::new(panes.width(), h)),
            Rect::from_min_max(Pos2::new(panes.left(), panes.top() + h + spacing), panes.max),
        )
    } else {
        let w = (panes.width() - spacing) / 2.0;
        (
            Rect::from_min_size(panes.min, Vec2::new(w, panes.height())),
            Rect::from_min_max(Pos2::new(panes.left() + w + spacing, panes.top()), panes.max),
        )
    };
    let (main_pane, other_pane) = if main_first { (first, second) } else { (second, first) };
    let (main_caption, other_caption) = captions(cx);
    let main_view = pane(ui, main_pane, &main_caption, marked);
    let other_view = pane(ui, other_pane, &other_caption, false);
    // The main pane: the ordinary photo view with its tools.
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(main_view));
    child.set_clip_rect(main_view);
    cx.photo_view(&mut child, tools);
    cx.state::<Compare>().main_area = Some(main_view);
    let main_double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
        && ui.rect_contains_pointer(main_view);
    // The second picture takes every pointer event (`eventFilter`): a click activates it, a double click closes.
    let response = ui.interact(other_view, egui::Id::unique("compare_other"), Sense::click());
    let painter = ui.painter_at(other_view);
    painter.rect_filled(other_view, CornerRadius::ZERO, crate::display::background(cx));
    let fit = cx.zoom == Zoom::Fit;
    let main_rect = cx.photo_rect().unwrap_or(main_view);
    if let Some(picture) = cx.state_ref::<SideLane>().and_then(|l| l.slot(SLOT)).and_then(|s| s.picture.as_ref()) {
        let target = follow(fit, main_view, main_rect, other_view, picture.size);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(picture.texture.id(), target, uv, Color32::WHITE);
    }
    if response.double_clicked() {
        double_click(cx, true);
    } else if response.clicked() {
        activate_other(cx);
    } else if main_double {
        double_click(cx, false);
    }
}

/// One pane (`Pane.dress`): the caption above the picture and the 2-pixel frame, white around the open photo.
/// Returns the picture's area.
fn pane(ui: &Ui, rect: Rect, caption: &str, marked: bool) -> Rect {
    let inner = rect.shrink(2.0);
    let painter = ui.painter_at(rect);
    if marked {
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(2.0, Color32::from_rgb(0xf2, 0xf2, 0xf2)),
            StrokeKind::Inside,
        );
    }
    let caption_rect = Rect::from_min_size(inner.min, Vec2::new(inner.width(), CAPTION_HEIGHT));
    let room = (caption_rect.width() - 16.0).max(10.0);
    let full = widgets::galley(ui, caption, t::font(t::FONT_SIZE), t::MUTED);
    let galley =
        if full.size().x <= room { full } else { widgets::elided(ui, caption, t::font(t::FONT_SIZE), t::MUTED, room) };
    let at = Pos2::new(caption_rect.left() + 8.0, caption_rect.center().y - galley.size().y / 2.0);
    painter.galley(at, galley, t::MUTED);
    Rect::from_min_max(Pos2::new(inner.left(), caption_rect.bottom()), inner.max)
}

/// What a control of the compare bar does once the bar is drawn.
type BarAction = Box<dyn FnOnce(&mut AppContext)>;

/// The compare bar: 비교 방식, 방향 (좌우 / 상하), 바꾸기 in the middle, 비교 닫기 on the right.
fn compare_bar(ui: &mut Ui, cx: &mut AppContext) {
    let state = cx.state::<Compare>();
    let (mode, vertical) = (state.mode, state.vertical);
    let names: Vec<String> = MODES.iter().map(|(title, _)| tr(title)).collect();
    let index = MODES.iter().position(|(_, m)| *m == mode).unwrap_or(0);
    let direction = tr("방향");
    let (side_text, stack_text, swap_text) = (tr("좌우"), tr("상하"), tr("바꾸기"));
    let label = widgets::galley(ui, direction.clone(), t::font(t::FONT_SIZE), t::TEXT);
    let side = Button::new(&side_text).checked(!vertical);
    let stack = Button::new(&stack_text).checked(vertical);
    let swap_button = Button::new(&swap_text);
    let combo = 140.0;
    let width =
        combo + 18.0 + label.size().x + 6.0 + side.size(ui).x + stack.size(ui).x + 18.0 + swap_button.size(ui).x;
    let rect = ui.max_rect();
    let mut action: Option<BarAction> = None;
    let group =
        Rect::from_min_size(Pos2::new(rect.center().x - width / 2.0, rect.top()), Vec2::new(width, rect.height()));
    let mut middle =
        ui.new_child(egui::UiBuilder::new().max_rect(group).layout(egui::Layout::left_to_right(egui::Align::Center)));
    middle.spacing_mut().item_spacing.x = 0.0;
    if let Some(i) =
        crate::panels::controls::choice(&mut middle, "compare_mode", &tr("비교 방식"), &names, index, combo)
    {
        let chosen = MODES[i].1;
        action = Some(Box::new(move |cx| set_mode(cx, chosen)));
    }
    middle.add_space(18.0);
    widgets::label(&mut middle, &direction, t::TEXT, t::FONT_SIZE);
    middle.add_space(6.0);
    if side.show(&mut middle).clicked() {
        action = Some(Box::new(|cx| set_vertical(cx, false)));
    }
    if stack.show(&mut middle).clicked() {
        action = Some(Box::new(|cx| set_vertical(cx, true)));
    }
    middle.add_space(18.0);
    if swap_button.show(&mut middle).on_hover_text(tr("두 사진의 자리를 서로 바꿉니다.")).clicked() {
        action = Some(Box::new(swap));
    }
    let mut right =
        ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if widgets::button(&mut right, &tr("비교 닫기")).on_hover_text(tr("사진을 더블클릭해도 닫힙니다.")).clicked()
    {
        action = Some(Box::new(|cx| stop(cx, false)));
    }
    if let Some(action) = action {
        action(cx);
    }
}
