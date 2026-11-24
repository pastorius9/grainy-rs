//! The application context: everything a feature can use. Commands, panels, tools, dialogs and jobs all work on
//! an `&mut AppContext`:
//!
//! * `doc`: the [`Document`] (photo list, selection, the open photo's [`EditState`], commits);
//! * `jobs`: background work with progress and cancel ([`Jobs`]); `renderer`: the preview and thumbnail lanes;
//! * the view state (mode, zoom, pan, the selected tool, before/after, clipping), the Develop column's state
//!   (expanded panels, fold-outs), the dialog stack, the status line;
//! * helpers for controls: [`AppContext::apply_row`], [`AppContext::set_and_commit`], [`AppContext::run`]…
//!
//! Port of the state the oracle keeps on `MainWindow` (luma/app.py).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use eframe::egui::{self, Vec2};
use grainy_core::{normalized, Settings};
use grainy_i18n::Language;

use crate::commands::{self, Invocation};
use crate::dialogs::native::{self, Chosen, Purpose};
use crate::dialogs::Dialog;
use crate::document::{Document, Kind};
use crate::export::ExportOptions;
use crate::fonts;
use crate::histogram::Bins;
use crate::jobs::Jobs;
use crate::panels::Tool;
use crate::render::{Frame, GpuPreview, PreviewJob, Renderer, Reply, Resolution, ThumbnailJob, Waker};
use crate::session::Session;
use crate::state::EditState;
use crate::text::{tr, tr_with};
use crate::widgets::RowEvent;
use crate::worker::PREVIEW_SIZE;

/// Snapshot tests freeze the render time shown in the status bar, so their pictures do not change from run to run.
/// While it is set the preview also runs on the CPU renderer only (`gpu_worker::Control`): GPU frames may differ
/// from CPU frames by a rounding step between machines.
pub static FROZEN_TIMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The two screens (`stack` index 1 and 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Library,
    Develop,
}

/// The photo view's zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// The whole photo in the view (화면 맞춤).
    Fit,
    /// One photo pixel per screen pixel.
    Actual,
    /// Ctrl+wheel: screen points per original pixel.
    Scale(f32),
}

/// The library filters of the left column (`library_buttons`).
pub const FILTERS: [(&str, &str); 4] = [
    ("all", "모든 사진"),
    ("picked", "선택 표시한 사진"),
    ("rated", "별점 4개 이상"),
    ("rejected", "제외 표시한 사진"),
];

/// A thumbnail on screen.
pub(crate) struct Thumb {
    pub key: u64,
    pub texture: Option<egui::TextureHandle>,
    pub size: Vec2,
    pub error: Option<String>,
    /// The cache's status (`thumb_status`: 오프라인, 원본 없음…), already Korean source text.
    pub status: String,
}

/// What the last preview request asked for: a new one is sent when this changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wanted {
    photo: u64,
    version: u64,
    before: bool,
    resolution: Resolution,
    crop: grainy_engine::Crop,
    /// The monitor profile's revision (`display_color.main.revision`): a new profile makes the picture again.
    display: u64,
    /// The HDR viewing choices' revision (`crate::hdr::revision`).
    hdr: u64,
    /// The fitted view's size the fit preview is developed at (`AppContext::view_need`; `None` for full size).
    view: Option<usize>,
    /// Quantized `[u0, v0, u1, v1]` 1:1 viewport tile ROI (as raw f32 bits for exact `Eq`), `None` for full frame.
    tile: Option<[u32; 4]>,
}

impl Wanted {
    fn same_development(self, base: Self) -> bool {
        Self { resolution: base.resolution, tile: base.tile, view: base.view, ..self } == base
    }
}

/// Native `egui_wgpu` texture for the GPU-rendered clipping overlay.
pub(crate) struct GpuClipTexture {
    pub id: egui::TextureId,
    pub key: (u64, bool, bool),
    pub tile_uv: Option<[f32; 4]>,
    pub _texture: Arc<grainy_gpu::wgpu::Texture>,
    pub _view: Arc<grainy_gpu::wgpu::TextureView>,
}

/// The application context.
pub struct AppContext {
    /// The egui context (textures, viewport commands, repaints).
    pub egui: egui::Context,
    /// The photos.
    pub doc: Box<dyn Document>,
    /// Background jobs.
    pub jobs: Jobs<AppContext>,
    /// The preview and thumbnail lanes.
    pub renderer: Box<dyn Renderer>,
    pub(crate) waker: Option<Waker>,
    wanted: Option<Wanted>,
    base_wanted: Option<Wanted>,
    requests: u64,
    /// Changes even when the new catalogue reuses the old photo ids and edit versions.
    pub(crate) document_generation: u64,
    /// The newest request id answered (a frame or an error).
    answered: u64,
    pub(crate) frame: Option<Arc<Frame>>,
    pub(crate) texture: Option<egui::TextureHandle>,
    /// Shared `eframe` wgpu render state for zero-copy native texture binding (`None` in headless/CPU tests).
    pub(crate) wgpu_render_state: Option<eframe::egui_wgpu::RenderState>,
    /// Zero-copy native `egui_wgpu` texture for the full frame (no CPU readback or upload).
    pub(crate) gpu_texture: Option<(egui::TextureId, Arc<GpuPreview>)>,
    /// Zero-copy native `egui_wgpu` texture for the zoomed 1:1 viewport tile (`[u0, v0, u1, v1]`).
    pub(crate) gpu_tile_texture: Option<(egui::TextureId, [f32; 4], Arc<GpuPreview>)>,
    /// Zero-copy native `egui_wgpu` clipping overlay texture (generated on GPU in < 0.2 ms).
    pub(crate) gpu_clip_texture: Option<GpuClipTexture>,
    /// The clipping overlay texture and the (frame id, shadows, highlights) it shows.
    pub(crate) clip_texture: Option<(egui::TextureHandle, (u64, bool, bool))>,
    pub(crate) picture_size: Vec2,
    pub(crate) bins: Option<Bins>,
    pub(crate) thumbs: HashMap<u64, Thumb>,
    /// Thumbnail keys asked for and not answered.
    thumbs_asked: HashMap<u64, u64>,
    /// Photos whose thumbnails were drawn this frame (only those are asked for).
    pub(crate) thumbs_drawn: HashMap<u64, usize>,
    /// The photos whose thumbnails were drawn in the frame before the last request round (what `idle` waits for).
    thumbs_shown: HashMap<u64, usize>,
    chosen_tx: Sender<Chosen>,
    chosen_rx: Receiver<Chosen>,
    /// Open dialogs, the last on top.
    pub dialogs: Vec<Box<dyn Dialog>>,
    pub(crate) export_options: ExportOptions,
    pub(crate) mode: Mode,
    pub(crate) tool: Tool,
    /// Expanded panels (the oracle starts with Basic open, solo mode on).
    pub(crate) expanded: Vec<&'static str>,
    pub(crate) solo: bool,
    /// Open fold-outs, by a stable name.
    pub(crate) foldouts: HashSet<&'static str>,
    /// Tab: the side columns are shown. Shift+Tab: the bars (header, view bar, filmstrip) too.
    pub(crate) sides: bool,
    pub(crate) bars: bool,
    pub(crate) full_screen: bool,
    pub(crate) zoom: Zoom,
    pub(crate) pan: Vec2,
    pub(crate) drt_advanced: bool,
    /// Shows the RAW white balance as if a RAW file were open (tests of the shell).
    pub(crate) raw_photo: bool,
    pub(crate) before: bool,
    pub(crate) compare: bool,
    pub(crate) wb_picker: bool,
    pub(crate) clip_shadows: bool,
    pub(crate) clip_highlights: bool,
    pub(crate) folder_search: String,
    /// The library search field as typed.
    pub(crate) search: String,
    /// Columns of the library grid at its last drawing (Up/Down move by a row).
    pub(crate) grid_columns: usize,
    /// Photo ID last reached during Shift+Click / Shift+Arrow range selection.
    pub(crate) selection_cursor: Option<u64>,
    /// The filmstrip scrolls to the open photo on its next drawing.
    pub(crate) scroll_to_current: bool,
    /// Where the photo was drawn at the last frame (screen points).
    pub(crate) photo_rect: Option<egui::Rect>,
    /// The fitted photo's long side on screen in device pixels, rounded up (`None` when not fitted): the size the
    /// fit preview is developed at (`Renderer::set_view_size`).
    pub(crate) view_need: Option<usize>,
    /// Visible normalized `[u0, v0, u1, v1]` ROI for true 1:1 GPU tile rendering when zoomed in.
    pub(crate) tile_need: Option<[f32; 4]>,
    pub(crate) debug_overlay: bool,
    /// Soft-proofing state (target output profile + optional gamut warning + BPC).
    pub(crate) soft_proof: crate::render::SoftProof,
    /// Live RGB % (0..100) under the mouse cursor when hovering the photo canvas.
    pub(crate) cursor_rgb: Option<[f32; 3]>,
    /// Cache key `(frame_id, px, py)` to avoid redundant 1x1 GPU pixel readbacks while stationary.
    pub(crate) cursor_sample_key: Option<(u64, u32, u32)>,
    /// Status bar message (already translated).
    pub(crate) status: String,
    /// The note under the automatic buttons (`auto_tone_note`).
    pub(crate) auto_note: String,
    /// Indices of choice boxes that only show (curve channel...).
    pub(crate) choices: HashMap<&'static str, usize>,
    pub(crate) fonts: fonts::Loaded,
    /// The Rust app's data folder (snapshots of read-only libraries).
    pub(crate) data_folder: Option<PathBuf>,
    /// State that features keep for themselves ([`AppContext::state`]), one value per type.
    extensions: HashMap<std::any::TypeId, Box<dyn std::any::Any>>,
}

impl AppContext {
    /// A context drawing with `renderer` on `doc`. Installs the font and the style in `egui`.
    pub fn new(egui: &egui::Context, mut renderer: Box<dyn Renderer>, doc: Box<dyn Document>) -> Self {
        let loaded = fonts::install(egui);
        log::info!("{}", loaded.describe());
        crate::theme::apply(egui);
        let repaint = egui.clone();
        let waker: Waker = Arc::new(move || repaint.request_repaint());
        if let Some(folder) = doc.asset_folder() {
            renderer.set_library(Some(folder));
        }
        renderer.set_waker(waker.clone());
        let mut jobs = Jobs::default();
        jobs.set_waker(Some(waker.clone()));
        let (chosen_tx, chosen_rx) = channel();
        Self {
            egui: egui.clone(),
            doc,
            jobs,
            renderer,
            waker: Some(waker),
            wanted: None,
            base_wanted: None,
            requests: 0,
            document_generation: 0,
            answered: 0,
            frame: None,
            texture: None,
            wgpu_render_state: None,
            gpu_texture: None,
            gpu_tile_texture: None,
            gpu_clip_texture: None,
            clip_texture: None,
            picture_size: Vec2::ZERO,
            bins: None,
            thumbs: HashMap::new(),
            thumbs_asked: HashMap::new(),
            thumbs_drawn: HashMap::new(),
            thumbs_shown: HashMap::new(),
            chosen_tx,
            chosen_rx,
            dialogs: Vec::new(),
            export_options: ExportOptions::default(),
            mode: Mode::Develop,
            tool: Tool::Edit,
            expanded: vec!["basic"],
            solo: true,
            foldouts: HashSet::new(),
            sides: true,
            bars: true,
            full_screen: false,
            zoom: Zoom::Fit,
            pan: Vec2::ZERO,
            drt_advanced: false,
            raw_photo: false,
            before: false,
            compare: false,
            wb_picker: false,
            clip_shadows: false,
            clip_highlights: false,
            folder_search: String::new(),
            search: String::new(),
            grid_columns: 1,
            selection_cursor: None,
            scroll_to_current: false,
            photo_rect: None,
            view_need: None,
            tile_need: None,
            debug_overlay: false,
            soft_proof: crate::render::SoftProof::default(),
            cursor_rgb: None,
            cursor_sample_key: None,
            status: tr("준비됨  ·  사진을 가져오면 시작할 수 있습니다."),
            auto_note: tr("자동톤: 검정·흰색 양끝 3% 여유\n목표 밝기 약 8–247 / 255"),
            choices: HashMap::new(),
            fonts: loaded,
            data_folder: None,
            extensions: HashMap::new(),
        }
    }

    /// A feature's own state, created on first use: a feature keeps what it needs in a type of its own file
    /// (`cx.state::<CropDraft>()`) instead of adding fields here.
    pub fn state<T: Default + 'static>(&mut self) -> &mut T {
        self.extensions
            .entry(std::any::TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut::<T>()
            .expect("one value per type")
    }

    /// A feature's own state if it was created.
    pub fn state_ref<T: 'static>(&self) -> Option<&T> {
        self.extensions.get(&std::any::TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    // ---- what tests and other features read ----------------------------------------------------------------

    /// The open photo's edit state (settings, undo).
    pub fn edit_state(&self) -> &EditState {
        self.doc.edit()
    }

    /// Mutable edit state of the open photo.
    pub fn edit_state_mut(&mut self) -> &mut EditState {
        self.doc.edit_mut()
    }

    /// The open photo's settings.
    pub fn settings(&self) -> &Settings {
        self.doc.edit().settings()
    }

    /// The document.
    pub fn document(&self) -> &dyn Document {
        self.doc.as_ref()
    }

    /// The document, to change.
    pub fn document_mut(&mut self) -> &mut dyn Document {
        self.doc.as_mut()
    }

    /// Applies a background result only to the document that requested it.
    pub(crate) fn spawn_document_job<T: Send + 'static>(
        &mut self,
        kind: &'static str,
        label: impl Into<String>,
        work: impl FnOnce(&crate::jobs::JobHandle) -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<T, String>) + 'static,
    ) -> crate::jobs::JobId {
        let generation = self.document_generation;
        self.jobs.spawn(kind, label, work, move |cx, result| {
            if cx.document_generation == generation {
                done(cx, result);
            }
        })
    }

    /// Replaces the document (opening a library): the picture and the thumbnails start again.
    pub fn set_document(&mut self, doc: Box<dyn Document>) -> bool {
        if let Err(message) = self.commit_document() {
            self.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
            return false;
        }
        crate::folder_watch::shutdown(self);
        self.renderer.set_library(doc.asset_folder());
        self.doc = doc;
        self.document_generation += 1;
        self.requests += 1;
        self.answered = self.requests;
        self.thumbs.clear();
        self.thumbs_asked.clear();
        self.wanted = None;
        self.photo_rect = None;
        self.selection_cursor = None;
        if let Some(sync) = self.state_ref::<crate::commands::sync::SyncState>() {
            let _ = sync;
            self.state::<crate::commands::sync::SyncState>().reset_document();
        }
        if self.state_ref::<crate::views::compare::Compare>().is_some_and(|c| c.on) {
            *self.state::<crate::views::compare::Compare>() = Default::default();
        }
        self.photo_changed();
        true
    }

    /// Whether the open document is the Python Grainy's library opened read-only (the "읽기 전용" badge).
    pub fn read_only_library(&self) -> bool {
        matches!(self.doc.kind(), Kind::Catalog { read_only: true, .. })
    }

    /// The interface font files that were loaded.
    pub fn loaded_fonts(&self) -> &fonts::Loaded {
        &self.fonts
    }

    /// The frame on screen.
    pub fn frame(&self) -> Option<&Arc<Frame>> {
        self.frame.as_ref()
    }

    /// Where the photo was drawn at the last frame, in screen points (tests compare it with the engine).
    pub fn photo_rect(&self) -> Option<egui::Rect> {
        self.photo_rect
    }

    /// The status bar text.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Sets the status bar text (already translated).
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
    }

    /// Hides or shows the debug overlay of changed settings (off by default; `--debug` or 설정 → 디버그 정보).
    pub fn set_debug_overlay(&mut self, on: bool) {
        self.debug_overlay = on;
    }

    /// Whether the debug overlay shows.
    pub fn debug_overlay(&self) -> bool {
        self.debug_overlay
    }

    /// Expands a panel as a click on its header would.
    pub fn open_panel(&mut self, key: &'static str) {
        self.toggle_panel(key, true);
    }

    /// Solo mode (`단일 패널 모드`): opening a panel closes the others.
    pub fn set_solo(&mut self, solo: bool) {
        self.solo = solo;
    }

    /// Whether the RAW controls show: a RAW file is open (or the shell's tests ask for them).
    pub fn raw_view(&self) -> bool {
        self.raw_photo || self.doc.current().is_some_and(|p| p.is_raw())
    }

    /// Opens a fold-out by name (`wb_detail`, `lens_manual`, `channel_curve`, `point_color`, `raw_profile`,
    /// `camera_matrix`, `presets`, `history`, `collections`).
    pub fn open_foldout(&mut self, name: &'static str) {
        self.foldouts.insert(name);
    }

    /// Opens or closes a fold-out.
    pub fn set_foldout(&mut self, name: &'static str, open: bool) {
        if open {
            self.foldouts.insert(name);
        } else {
            self.foldouts.remove(name);
        }
    }

    /// Shows the controls of a RAW photo (the RAW white balance) as if one were open.
    pub fn set_raw_photo(&mut self, raw: bool) {
        self.raw_photo = raw;
    }

    /// Shows the DRT's Advanced sliders.
    pub fn set_drt_advanced(&mut self, on: bool) {
        self.drt_advanced = on;
    }

    /// The current screen.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Port of `set_mode`.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode == Mode::Develop && mode != Mode::Develop && self.tool == Tool::Masks {
            self.leave_masks_mode();
        }
        self.mode = mode;
    }

    /// The tool selected in the strip.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// The photo view's zoom.
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// Sets the zoom (the Fit and 1:1 buttons).
    pub fn set_zoom(&mut self, zoom: Zoom) {
        self.zoom = zoom;
        self.pan = Vec2::ZERO;
    }

    /// Shows the undeveloped photo (`\`) or the developed one.
    pub fn set_before(&mut self, before: bool) {
        self.before = before;
    }

    /// Whether the original is shown.
    pub fn before(&self) -> bool {
        self.before
    }

    /// The clipping overlay (J): shadows and highlights together.
    pub fn set_clipping(&mut self, on: bool) {
        self.clip_shadows = on;
        self.clip_highlights = on;
    }

    /// Whether the side columns are shown.
    pub fn sides_visible(&self) -> bool {
        self.sides
    }

    /// Whether the header and the bars are shown.
    pub fn bars_visible(&self) -> bool {
        self.bars
    }

    /// Whether the shortcut list is open.
    pub fn shortcut_list_open(&self) -> bool {
        self.dialog_open("shortcuts")
    }

    /// Expanded panels.
    pub fn expanded_panels(&self) -> &[&'static str] {
        &self.expanded
    }

    /// Switches the interface language; takes effect on the next frame.
    pub fn set_language(&mut self, language: Language) {
        grainy_i18n::set_language(language);
        log::info!("language: {}", language.code());
    }

    /// Whether nothing is loading, rendering or exporting and the screen shows the newest answers (tests wait
    /// for this before taking a snapshot).
    pub fn idle(&self) -> bool {
        self.renderer.idle()
            && self.jobs.jobs_idle()
            && self.thumbs_asked.is_empty()
            && self.thumbnails_current()
            && self.answered == self.requests
            && !self.doc.listing_pending()
            && !self.doc.edit().has_journal()
    }

    // ---- commands, dialogs ----------------------------------------------------------------------------------

    /// Runs command `id` (menus, buttons, keys).
    pub fn run(&mut self, id: &str) -> bool {
        commands::run(self, id, &Invocation::default())
    }

    /// Runs command `id` with a value.
    pub fn run_with(&mut self, id: &str, invocation: &Invocation) -> bool {
        commands::run(self, id, invocation)
    }

    /// Opens a dialog on top of the others (one per id).
    pub fn open_dialog(&mut self, dialog: Box<dyn Dialog>) {
        if !self.dialog_open(dialog.id()) {
            self.dialogs.push(dialog);
        }
    }

    /// Whether a dialog with this id is open.
    pub fn dialog_open(&self, id: &str) -> bool {
        self.dialogs.iter().any(|d| d.id() == id)
    }

    /// Closes the dialog with this id.
    pub fn close_dialog(&mut self, id: &str) {
        self.dialogs.retain(|d| d.id() != id);
    }

    /// Whether a modal dialog is open (shortcuts and arrow keys wait).
    pub fn modal_open(&self) -> bool {
        self.dialogs.iter().any(|d| d.modal())
    }

    /// Opens a native file dialog for `purpose`; the answer comes back on a later frame.
    pub fn choose(&mut self, purpose: Purpose) {
        let title = match purpose {
            Purpose::ImportFiles => tr("사진 가져오기"),
            Purpose::ImportFolder => tr("사진 폴더 가져오기 (하위 폴더 포함)"),
            Purpose::ExportFolder => tr("내보낼 폴더 선택"),
            Purpose::OpenLibrary => tr("라이브러리 폴더 열기"),
        };
        let start = match purpose {
            Purpose::ExportFolder => Some(PathBuf::from(self.export_options.folder.trim())),
            _ => self.doc.current().and_then(|p| p.path.parent().map(PathBuf::from)),
        };
        native::open(purpose, title, start, self.chosen_tx.clone(), self.waker.clone());
    }

    // ---- editing ---------------------------------------------------------------------------------------------

    /// Release any native zero-copy `egui_wgpu` textures registered with `eframe`.
    pub(crate) fn free_gpu_textures(&mut self) {
        if let Some(rs) = &self.wgpu_render_state {
            let mut renderer = rs.renderer.write();
            if let Some((id, _)) = self.gpu_texture.take() {
                renderer.free_texture(&id);
            }
            if let Some((id, _, _)) = self.gpu_tile_texture.take() {
                renderer.free_texture(&id);
            }
            if let Some(clip) = self.gpu_clip_texture.take() {
                renderer.free_texture(&clip.id);
            }
        } else {
            self.gpu_texture = None;
            self.gpu_tile_texture = None;
            self.gpu_clip_texture = None;
        }
    }

    /// What changes when another photo opens.
    pub(crate) fn photo_changed(&mut self) {
        crate::commands::auto_source::clear(self);
        self.base_wanted = None;
        self.texture = None;
        self.free_gpu_textures();
        self.clip_texture = None;
        self.frame = None;
        self.bins = None;
        self.picture_size = Vec2::ZERO;
        self.pan = Vec2::ZERO;
        self.tile_need = None;
        self.before = false;
        self.auto_note = tr("자동톤: 검정·흰색 양끝 3% 여유\n목표 밝기 약 8–247 / 255");
    }

    pub(crate) fn selection_cursor_index(&self) -> Option<usize> {
        self.selection_cursor.and_then(|id| self.doc.index_of(id)).or_else(|| self.doc.current_index())
    }

    pub(crate) fn proof_available(&self) -> bool {
        !(crate::hdr::enabled() && self.settings().bool("hdr"))
    }

    /// Applies a soft-proofing configuration change and refreshes the preview.
    pub(crate) fn set_soft_proof(&mut self, proof: crate::render::SoftProof) {
        self.base_wanted = None;
        self.soft_proof = proof.clone();
        self.renderer.set_soft_proof(proof);
        self.wanted = None;
    }

    /// Opens photo `index` of the list (filmstrip click, arrow keys).
    pub fn select_photo(&mut self, index: usize) {
        let leaving = self.doc.current_id();
        if self.doc.select(index) {
            crate::views::compare::left(self, leaving);
            self.photo_changed();
        }
        self.selection_cursor = self.doc.photos().get(index).map(|p| p.id);
        self.scroll_to_current = true;
    }

    /// Applies what a slider row did: a live value, then a commit (one undo step per drag).
    pub(crate) fn apply_row(&mut self, key: &str, event: RowEvent) {
        if let Some(value) = event.set {
            self.doc.edit_mut().set(key, value);
            self.before = false;
        }
        if event.commit {
            self.doc.edit_mut().commit("보정 변경");
        }
    }

    /// Sets a value and commits it as one undo step with `label` (buttons, check boxes, choices).
    pub(crate) fn set_and_commit(&mut self, key: &str, value: impl Into<serde_json::Value>, label: &str) {
        let edit = self.doc.edit_mut();
        edit.commit("보정 변경");
        edit.set(key, value);
        edit.commit(label);
        self.before = false;
    }

    /// A button that is not wired yet: logs it.
    pub(crate) fn stub(&mut self, what: &str) {
        log::info!("not implemented yet: {what}");
    }

    /// Port of `copy_edits`.
    pub fn copy_edits(&mut self) {
        self.run("develop.copy_edits");
    }

    /// Port of `paste_edits` for the selected photos.
    pub fn paste_edits(&mut self) {
        self.run("develop.paste_edits");
    }

    /// Port of `rotate_photo` (clockwise) and of Ctrl+[ (counter-clockwise): the crop is cleared.
    pub fn rotate(&mut self, clockwise: bool) {
        if self.doc.current().is_none() {
            return;
        }
        let edit = self.doc.edit_mut();
        edit.commit("보정 변경");
        let rotation = edit.settings().i64("rotation").rem_euclid(4);
        edit.set("crop", serde_json::Value::Null);
        edit.set("rotation", (rotation + if clockwise { 1 } else { 3 }) % 4);
        edit.commit("보정 변경");
        self.before = false;
        self.pan = Vec2::ZERO;
    }

    /// Port of `StudioTools.change_film`: the stock with its grain (`film_look.with_film`), one undo step.
    pub fn change_film(&mut self, name: &str) {
        let settings = self.settings().clone();
        if settings.str("film_look") == name {
            return;
        }
        let edit = self.doc.edit_mut();
        edit.commit("보정 변경");
        let film = grainy_film::with_film(&settings, name, &[]);
        for (key, value) in &film.0 {
            if !crate::state::py_eq(settings.value(key), value) {
                edit.set(key, value.clone());
            }
        }
        edit.commit("필름");
        self.before = false;
    }

    /// The largest centred crop with `ratio` (width / height) on the rotated photo (what `set_aspect` makes of a
    /// full frame, without the interactive frame of a later run): one undo step.
    pub fn crop_to_aspect(&mut self, ratio: f64) {
        let Some((w, h)) = self.doc.current().and_then(|p| p.original) else { return };
        let rotation = self.settings().i64("rotation").rem_euclid(4);
        let (w, h) = if rotation % 2 == 1 { (f64::from(h), f64::from(w)) } else { (f64::from(w), f64::from(h)) };
        if let Some(crop) = crate::panels::crop::centred_crop(w, h, ratio) {
            self.set_and_commit("crop", serde_json::Value::from(crop.to_vec()), "보정 변경");
        }
    }

    fn leave_masks_mode(&mut self) {
        if self.state_ref::<crate::masks::MaskState>().is_some_and(|s| s.mode != crate::masks::Mode::None) {
            crate::masks::actions::set_tool(self, crate::masks::Mode::None);
        }
    }

    /// Port of `DevelopPanels.select_tool`: a second click on an open tool goes back to the panels.
    pub(crate) fn select_tool(&mut self, tool: Tool) {
        self.wb_picker = false;
        let next = if tool == self.tool { Tool::Edit } else { tool };
        if self.tool == Tool::Masks && next != Tool::Masks {
            self.leave_masks_mode();
        }
        self.tool = next;
    }

    /// A tool's key: opens the tool (`open_tool`), in the Develop screen.
    pub fn open_tool(&mut self, tool: Tool) {
        self.set_mode(Mode::Develop);
        if self.tool == Tool::Masks && tool != Tool::Masks {
            self.leave_masks_mode();
        }
        self.tool = tool;
    }

    /// Port of `DevelopPanels.toggle_panel`.
    pub(crate) fn toggle_panel(&mut self, key: &'static str, expanded: bool) {
        if crate::panels::TOOLS.contains(&key) {
            if !expanded {
                if self.tool == Tool::Masks {
                    self.leave_masks_mode();
                }
                self.tool = Tool::Edit;
            }
            return;
        }
        if expanded {
            if self.tool == Tool::Masks {
                self.leave_masks_mode();
            }
            self.tool = Tool::Edit;
            if self.solo {
                self.expanded.clear();
            }
            if !self.expanded.contains(&key) {
                self.expanded.push(key);
            }
        } else {
            self.expanded.retain(|k| *k != key);
        }
    }

    // ---- opening and exporting -------------------------------------------------------------------------------

    /// Opens files and folders (with their subfolders): into the session, or imported into the library (added in
    /// place). The folders are read and the files added on a background job.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>) {
        crate::commands::file::import_paths(self, paths);
    }

    /// Opens the export dialog (내보내기) for the selected photos.
    pub fn open_export_dialog(&mut self) {
        self.run("file.export");
    }

    /// Port of `start_export`: the selected photos with their settings, exported on a background job.
    pub fn start_export(&mut self, options: ExportOptions) {
        crate::commands::export::start_export(self, options);
    }

    /// Whether an export is running.
    pub fn exporting(&self) -> bool {
        self.jobs.running("export")
    }

    // ---- every frame -----------------------------------------------------------------------------------------

    /// Finished jobs, the document's listing, file dialogs, dropped files.
    pub(crate) fn poll_background(&mut self) {
        crate::session_state::poll(self);
        crate::folders::poll(self);
        crate::commands::settings::poll(self);
        crate::commands::collections::timed_refresh(self);
        crate::display::poll(self);
        crate::hdr::poll(self);
        crate::folder_watch::poll(self);
        for finished in self.jobs.poll() {
            finished(self);
        }
        if let Some(job) = self.doc.listing_job() {
            let generation = self.document_generation;
            self.jobs.spawn(
                "listing",
                "",
                move |_| job(),
                move |cx: &mut AppContext, result| match result {
                    _ if cx.document_generation != generation => {}
                    Ok(listing) => {
                        let had = cx.doc.current_id();
                        cx.doc.apply_listing(listing);
                        if cx.doc.current_id() != had {
                            cx.photo_changed();
                        }
                    }
                    Err(message) => {
                        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
                    }
                },
            );
        }
        while let Ok(chosen) = self.chosen_rx.try_recv() {
            match chosen.purpose {
                Purpose::ImportFiles | Purpose::ImportFolder => self.open_paths(chosen.paths),
                Purpose::OpenLibrary => {
                    if let Some(folder) = chosen.paths.into_iter().next() {
                        crate::commands::file::open_library(self, &folder);
                    }
                }
                Purpose::ExportFolder => {
                    for dialog in &mut self.dialogs {
                        dialog.receive(&chosen);
                    }
                }
            }
        }
        let dropped: Vec<PathBuf> =
            self.egui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.open_paths(dropped);
        }
        self.jobs.set_render_busy(!self.renderer.idle());
    }

    /// Commit an active gesture and persist the journal before a library switch, update or window close.
    pub(crate) fn commit_document(&mut self) -> Result<(), String> {
        self.doc.edit_mut().commit("보정 변경");
        self.doc.flush()
    }

    /// Stores the edits recorded this frame.
    pub(crate) fn flush(&mut self) {
        if let Err(message) = self.doc.flush() {
            self.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
        }
        crate::commands::sync::after_flush(self);
    }

    /// Collects the renderer's replies and sends the requests the screen needs now.
    pub fn refresh_picture(&mut self) {
        self.refresh_picture_inner(false);
    }

    pub(crate) fn refresh_picture_inner(&mut self, preserve_status: bool) {
        for reply in self.renderer.poll() {
            match reply {
                Reply::Frame(frame) => {
                    self.answered = self.answered.max(frame.id);
                    if frame.photo != self.doc.current_id() || frame.id != self.requests {
                        continue;
                    }
                    self.doc.set_original(frame.photo, frame.original);
                    if !preserve_status {
                        self.status = if self.before {
                            tr("원본 비교 중")
                        } else {
                            let source = match frame.resolution {
                                Resolution::Fit => "미리보기 {0} × {1} · {2} ms",
                                Resolution::Full => "원본 해상도 {0} × {1} · {2} ms",
                            };
                            let values = [
                                frame.preview.width.to_string(),
                                frame.preview.height.to_string(),
                                if FROZEN_TIMING.load(std::sync::atomic::Ordering::Relaxed) {
                                    "0".to_owned()
                                } else {
                                    format!("{:.0}", frame.millis)
                                },
                            ];
                            tr_with(source, &[&values[0], &values[1], &values[2]])
                        };
                    }
                    if frame.tile_uv.is_none() {
                        self.base_wanted = self.wanted;
                    }
                    self.show_frame(frame);
                    if self.renderer.prepares_analysis() {
                        crate::commands::auto_source::prepare(self);
                    }
                }
                Reply::Failed { id, photo, message } => {
                    self.answered = self.answered.max(id);
                    if photo == self.doc.current_id() && id == self.requests {
                        // The last good frame stays on screen.
                        self.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
                    }
                }
                Reply::Thumbnail(thumb) => {
                    if self.thumbs_asked.get(&thumb.photo) != Some(&thumb.key) {
                        continue;
                    }
                    self.thumbs_asked.remove(&thumb.photo);
                    if let Some(original) = thumb.original {
                        self.doc.set_original(thumb.photo, original);
                    }
                    let entry = match thumb.result {
                        Ok(preview) => {
                            let image = egui::ColorImage::from_rgba_unmultiplied(
                                [preview.width, preview.height],
                                &preview.rgba,
                            );
                            let name = format!("thumb-{}", thumb.photo);
                            Thumb {
                                key: thumb.key,
                                texture: Some(self.egui.load_texture(name, image, egui::TextureOptions::LINEAR)),
                                size: Vec2::new(preview.width as f32, preview.height as f32),
                                error: None,
                                status: thumb.status,
                            }
                        }
                        Err(message) => Thumb {
                            key: thumb.key,
                            texture: None,
                            size: Vec2::ZERO,
                            error: Some(message),
                            status: thumb.status,
                        },
                    };
                    self.thumbs.insert(thumb.photo, entry);
                }
            }
        }
        self.request_preview();
        self.request_thumbnails();
    }

    /// The resolution the view needs: the full-size source when zoomed in past the preview source.
    /// With zero-copy GPU 1:1 viewport tiling active, stays at `Resolution::Full` even during
    /// slider drags (developing only the visible ~2 MP tile at 60 fps instead of blurring to Fit).
    fn resolution(&self) -> Resolution {
        let long = self.doc.current().and_then(|p| p.original).map_or(0, |(w, h)| w.max(h) as usize);
        let tiled_gpu = self.tile_need.is_some()
            && self.wgpu_render_state.is_some()
            && self.renderer.gpu_status().is_some_and(|s| s.usable);
        if self.zoom != Zoom::Fit && long > PREVIEW_SIZE && (tiled_gpu || !self.doc.edit().dirty()) {
            Resolution::Full
        } else {
            Resolution::Fit
        }
    }

    pub(crate) fn request_preview(&mut self) {
        let crop = crate::tools::crop::preview_crop(self);
        let Some(photo) = self.doc.current() else { return };
        let resolution = self.resolution();
        let tile = self.tile_need.filter(|_| resolution == Resolution::Full);
        let mut wanted = Wanted {
            crop,
            photo: photo.id,
            version: self.doc.edit().version(),
            before: self.before,
            resolution,
            display: crate::display::revision(self),
            hdr: crate::hdr::revision(self),
            view: self.view_need.filter(|_| resolution == Resolution::Fit),
            tile: tile.map(|r| r.map(f32::to_bits)),
        };
        // Refresh the reduced whole-image background and its histogram before painting
        // a tile developed with a new edit, crop or display configuration.
        if wanted.tile.is_some() && !self.base_wanted.is_some_and(|base| wanted.same_development(base)) {
            wanted.resolution = Resolution::Fit;
            wanted.view = self.view_need;
            wanted.tile = None;
        }
        if self.wanted == Some(wanted) {
            return;
        }
        // Let one drag frame reach the screen. Submitting every intermediate value would make
        // every reply stale during continuous movement. The reply wakes us to submit the latest.
        if self.answered < self.requests
            && self.doc.edit().dirty()
            && self.wanted.is_some_and(|w| Wanted { version: wanted.version, ..w } == wanted)
        {
            return;
        }
        let path = photo.path.clone();
        self.wanted = Some(wanted);
        self.requests += 1;
        let settings = if self.before { before_settings(self.settings()) } else { self.settings().clone() };
        let job =
            PreviewJob { id: self.requests, photo: wanted.photo, path, settings, resolution: wanted.resolution, crop };
        self.renderer.set_view_size(wanted.view);
        self.renderer.set_tile_roi(wanted.tile.map(|t| t.map(f32::from_bits)));
        self.renderer.preview(job);
    }

    /// Whether every thumbnail on screen shows the photo's current version from its current source. A stored
    /// thumbnail replaces the first, freshly rendered one when the library reports it, and the two differ by the
    /// JPEG's rounding, so a picture taken before that second request would differ from run to run.
    fn thumbnails_current(&self) -> bool {
        self.thumbs_shown.iter().all(|(&id, &index)| {
            let Some(photo) = self.doc.photos().get(index) else { return true };
            if photo.id != id {
                return true;
            }
            let key = thumb_key(photo.id, self.doc.version_of(index), self.doc.thumb_cache(index).is_some())
                ^ crate::display::thumb_salt(self)
                ^ self.document_generation.wrapping_mul(0x1656_67b1_9e37_79f9);
            self.thumbs.get(&photo.id).is_some_and(|t| t.key == key)
        })
    }

    fn request_thumbnails(&mut self) {
        let current = self.doc.current_index();
        let dragging = self.doc.edit().dirty();
        let drawn = std::mem::take(&mut self.thumbs_drawn);
        // Drop offscreen textures in batches; preserve the complete visible set even on large displays.
        if self.thumbs.len() > 256 && !drawn.is_empty() {
            self.thumbs.retain(|id, _| drawn.contains_key(id));
        }
        self.thumbs_shown.clone_from(&drawn);
        let mut jobs = Vec::new();
        let mut visible: Vec<_> = drawn.into_iter().collect();
        visible.sort_unstable_by_key(|&(_, index)| index);
        for (id, index) in visible {
            let Some(photo) = self.doc.photos().get(index) else { continue };
            if photo.id != id || (current == Some(index) && dragging) {
                continue;
            }
            let cache = self.doc.thumb_cache(index);
            let key = thumb_key(photo.id, self.doc.version_of(index), cache.is_some())
                ^ crate::display::thumb_salt(self)
                ^ self.document_generation.wrapping_mul(0x1656_67b1_9e37_79f9);
            if self.thumbs.get(&photo.id).is_some_and(|t| t.key == key)
                || self.thumbs_asked.get(&photo.id) == Some(&key)
            {
                continue;
            }
            let settings =
                if cache.is_some() { normalized(&serde_json::Map::new()) } else { self.doc.settings_of(index) };
            jobs.push(ThumbnailJob { photo: photo.id, path: photo.path.clone(), settings, key, cache });
        }
        for job in jobs {
            self.thumbs_asked.insert(job.photo, job.key);
            self.renderer.thumbnail(job);
        }
    }

    fn show_frame(&mut self, frame: Arc<Frame>) {
        if let (Some(gpu_preview), Some(rs)) = (frame.gpu.clone(), self.wgpu_render_state.clone()) {
            let filter = grainy_gpu::wgpu::FilterMode::Linear;
            if let Some(tile_uv) = frame.tile_uv {
                let mut renderer = rs.renderer.write();
                let id = match &self.gpu_tile_texture {
                    Some((id, _, _)) => {
                        renderer.update_egui_texture_from_wgpu_texture(
                            &rs.device,
                            &gpu_preview.presented.view,
                            filter,
                            *id,
                        );
                        *id
                    }
                    None => renderer.register_native_texture(&rs.device, &gpu_preview.presented.view, filter),
                };
                self.gpu_tile_texture = Some((id, tile_uv, gpu_preview));
            } else {
                let mut renderer = rs.renderer.write();
                let id = match &self.gpu_texture {
                    Some((id, _)) => {
                        renderer.update_egui_texture_from_wgpu_texture(
                            &rs.device,
                            &gpu_preview.presented.view,
                            filter,
                            *id,
                        );
                        *id
                    }
                    None => renderer.register_native_texture(&rs.device, &gpu_preview.presented.view, filter),
                };
                if let Some((tile_id, _, _)) = self.gpu_tile_texture.take() {
                    renderer.free_texture(&tile_id);
                }
                self.gpu_texture = Some((id, gpu_preview));
                self.texture = None;
            }
            let (w, h) = frame.logical_size();
            self.picture_size = Vec2::new(w as f32, h as f32);
            self.bins = Some(frame.bins);
            self.clip_texture = None;
            self.frame = Some(frame);
            return;
        }
        let preview = &frame.preview;
        if preview.rgba.len() != preview.width * preview.height * 4 {
            log::warn!("renderer returned a picture of the wrong size");
            return;
        }
        let size = [preview.width, preview.height];
        let image = if preview.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
            egui::ColorImage::from_rgba_premultiplied(size, &preview.rgba)
        } else {
            egui::ColorImage::from_rgba_unmultiplied(size, &preview.rgba)
        };
        let options = egui::TextureOptions::LINEAR;
        match &mut self.texture {
            Some(texture) => texture.set(image, options),
            None => self.texture = Some(self.egui.load_texture("photo", image, options)),
        }
        self.free_gpu_textures();
        let (w, h) = frame.logical_size();
        self.picture_size = Vec2::new(w as f32, h as f32);
        self.bins = Some(frame.bins);
        self.clip_texture = None;
        self.frame = Some(frame);
    }
}

/// The settings of the undeveloped view (`\`): the defaults (the oracle's `toggle_before` renders `defaults()`;
/// the compare view's `before_settings`, which keeps the geometry, is the compare work package's).
fn before_settings(_settings: &Settings) -> Settings {
    normalized(&serde_json::Map::new())
}

/// A thumbnail's key: the photo, its settings' version and where it comes from.
fn thumb_key(id: u64, version: u64, cached: bool) -> u64 {
    id.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ version.wrapping_mul(0xc2b2_ae3d_27d4_eb4f) ^ u64::from(cached)
}

/// The session document of the shell (`--memory`, tests).
pub fn memory_document() -> Box<dyn Document> {
    Box::new(Session::default())
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::render::TestCard;

    #[test]
    fn continuous_slider_edits_show_inflight_frames_and_then_the_latest_value() {
        let mut doc = Session::default();
        doc.add([PathBuf::from("generated.png")]);
        let mut cx = AppContext::new(&Default::default(), Box::<TestCard>::default(), Box::new(doc));
        cx.request_preview();
        let first = cx.requests;
        for ev in 1..=8 {
            cx.edit_state_mut().set("exposure", ev as f64 / 10.);
            cx.request_preview();
        }
        assert_eq!(cx.requests, first);
        cx.refresh_picture();
        assert_eq!(cx.frame().unwrap().id, first);
        assert!(!cx.jobs.running("prepare-auto"), "a synthetic renderer must not read its document paths");
        assert_eq!(cx.requests, first + 1);
        assert_eq!(cx.wanted.unwrap().version, cx.edit_state().version());
        cx.refresh_picture();
        assert_eq!(cx.frame().unwrap().id, first + 1);
        cx.pan = Vec2::new(30., 60.);
        cx.refresh_picture();
        assert_eq!(cx.requests, first + 1);
    }

    #[test]
    fn switching_documents_rejects_queued_frames_and_thumbnails() {
        let mut cx = AppContext::new(&egui::Context::default(), Box::<TestCard>::default(), memory_document());
        cx.refresh_picture();
        assert!(cx.frame.is_some());
        cx.renderer.preview(PreviewJob {
            id: 0,
            photo: 0,
            path: PathBuf::new(),
            settings: cx.settings().clone(),
            resolution: Resolution::Fit,
            crop: grainy_engine::Crop::Cropped,
        });
        cx.renderer.thumbnail(ThumbnailJob {
            photo: 0,
            path: PathBuf::new(),
            settings: cx.settings().clone(),
            key: 7,
            cache: None,
        });
        cx.set_document(memory_document());
        cx.thumbs_asked.insert(0, 8);
        cx.refresh_picture();
        assert!(cx.frame.is_none(), "an old library's photo id can equal the new library's id");
        assert!(cx.thumbs.is_empty());
        assert_eq!(cx.thumbs_asked.get(&0), Some(&8));
    }

    #[test]
    fn a_completed_document_job_is_discarded_after_a_switch() {
        let mut cx = AppContext::new(&egui::Context::default(), Box::<TestCard>::default(), memory_document());
        let (tx, rx) = std::sync::mpsc::channel();
        cx.jobs.set_waker(Some(Arc::new(move || {
            let _ = tx.send(());
        })));
        cx.spawn_document_job("xmp_read", "", |_| Ok(()), |_, _| panic!("old result applied"));
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let callbacks = cx.jobs.poll();
        assert_eq!(callbacks.len(), 1);
        cx.set_document(memory_document());
        for done in callbacks {
            done(&mut cx);
        }
    }

    #[test]
    fn an_old_listing_cannot_replace_a_new_library() {
        use crate::document::catalog::CatalogDocument;
        use std::time::{Duration, Instant};
        let root = std::env::temp_dir().join(format!("grainy-review-listing-{}", std::process::id()));
        let old = grainy_catalog::Catalog::new(root.join("old")).unwrap();
        old.add(root.join("generated.png").to_str().unwrap(), None, None, true).unwrap();
        let doc = CatalogDocument::with_catalog(old).unwrap();
        let mut cx = AppContext::new(&egui::Context::default(), Box::<TestCard>::default(), Box::new(doc));
        cx.poll_background();
        let start = Instant::now();
        let finished = loop {
            let finished = cx.jobs.poll();
            if !finished.is_empty() {
                break finished;
            }
            assert!(start.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(2));
        };
        cx.set_document(Box::new(CatalogDocument::open(&root.join("new")).unwrap()));
        for done in finished {
            done(&mut cx);
        }
        assert!(cx.doc.photos().is_empty());
        drop(cx);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn tiles_require_a_whole_image_with_matching_edits() {
    let base = Wanted {
        photo: 1,
        version: 2,
        before: false,
        resolution: Resolution::Fit,
        crop: grainy_engine::Crop::Cropped,
        display: 0,
        hdr: 0,
        view: None,
        tile: None,
    };
    let tile = Wanted { resolution: Resolution::Full, tile: Some([0; 4]), ..base };
    assert!(tile.same_development(base));
    assert!(!Wanted { version: 3, ..tile }.same_development(base));
    assert!(!Wanted { before: true, ..tile }.same_development(base));
}
