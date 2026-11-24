//! The centre: the photo view (fit / 1:1 / Ctrl+wheel zoom, pan, clipping overlay, the open canvas tool), the view
//! bar and the
//! filmstrip in Develop; the library toolbar and the grid in Library (luma/app.py `build_ui`,
//! luma/photo_browser.py `PhotoDelegate`).

use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, Panel, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use grainy_i18n::tr_with;

use crate::context::{AppContext, Mode, Zoom};
use crate::imagespace::ViewTransform;
use crate::text::tr;
use crate::theme as t;
use crate::tools::{self, CanvasTool};
use crate::widgets::{self, Button};

/// Filmstrip cell (`setGridSize(QSize(140,111))`) and grid cell (`QSize(228,190)`).
const STRIP_CELL: Vec2 = Vec2::new(140.0, 111.0);
const GRID_CELL: Vec2 = Vec2::new(228.0, 190.0);
/// The share of the view a fitted photo fills.
const FIT_MARGIN: f32 = 0.96;
/// The closest zoom: 16 screen pixels per photo pixel.
const MAX_ZOOM: f32 = 16.0;

impl AppContext {
    pub(crate) fn center(&mut self, ui: &mut Ui, tools: &mut [Box<dyn CanvasTool>]) {
        if self.mode == Mode::Develop && self.bars {
            Panel::bottom("filmstrip")
                .frame(Frame::new().fill(t::BG).inner_margin(Margin::symmetric(3, 3)))
                .show_separator_line(false)
                .resizable(false)
                .exact_size(132.0)
                .show(ui, |ui| self.filmstrip(ui));
        }
        if self.bars {
            Panel::bottom("view_bar")
                .frame(Frame::new().fill(t::BG).inner_margin(Margin { left: 12, right: 12, top: 9, bottom: 9 }))
                .show_separator_line(false)
                .resizable(false)
                .show(ui, |ui| self.view_bar(ui));
        }
        if self.mode == Mode::Library && self.bars {
            Panel::top("library_toolbar")
                .frame(Frame::new().fill(t::BG).inner_margin(Margin { left: 15, right: 15, top: 9, bottom: 8 }))
                .show_separator_line(false)
                .resizable(false)
                .show(ui, |ui| self.library_toolbar(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| match self.mode {
            Mode::Develop => crate::views::compare::photo_area(self, ui, tools),
            Mode::Library => self.grid(ui),
        });
    }

    /// Screen points per original pixel for a zoom in a view of `area` showing `size` original pixels.
    fn scale_for(&self, zoom: Zoom, area: Rect, size: Vec2, ppp: f32) -> f32 {
        let fit = (area.width() / size.x).min(area.height() / size.y) * FIT_MARGIN;
        match zoom {
            Zoom::Fit => fit,
            Zoom::Actual => 1.0 / ppp,
            Zoom::Scale(s) => s,
        }
    }

    pub(crate) fn photo_view(&mut self, ui: &mut Ui, tools: &mut [Box<dyn CanvasTool>]) {
        let area = ui.max_rect();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, crate::display::background(self));
        response.context_menu(|ui| crate::display::background_context_menu(ui, self));
        let ppp = ui.ctx().pixels_per_point();
        let original = self.doc.current().and_then(|p| p.original);
        let thumb_size = self.thumbs.get(&self.doc.current_id()).map_or(Vec2::ZERO, |t| t.size);
        let size = if self.picture_size != Vec2::ZERO {
            self.picture_size
        } else if let (Some((w, h)), true) = (original, thumb_size.x > 0.0) {
            // Until the first frame: the thumbnail, at the photo's size.
            thumb_size * (w.max(h) as f32 / thumb_size.x.max(thumb_size.y))
        } else {
            Vec2::new(1500.0, 1000.0)
        };
        let fit = self.scale_for(Zoom::Fit, area, size, ppp);
        let mut scale = self.scale_for(self.zoom, area, size, ppp);
        // Ctrl+wheel: zoom around the pointer.
        let (zoom_delta, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if response.hovered() && (zoom_delta - 1.0).abs() > 1e-4 {
            if let Some(pointer) = pointer {
                let min_scale = fit.min(1.0 / ppp);
                let new = (scale * zoom_delta).clamp(min_scale, MAX_ZOOM / ppp);
                if zoom_delta < 1.0 && new <= fit + 1e-4 {
                    self.zoom = Zoom::Fit;
                    self.pan = Vec2::ZERO;
                    scale = fit;
                } else {
                    let centre = area.center() + self.pan;
                    let at = (pointer - centre) / scale;
                    self.pan = pointer - at * new - area.center();
                    self.zoom = Zoom::Scale(new);
                    scale = new;
                }
            }
        }
        // macOS: a trackpad scroll moves the photo (the oracle's `wheelEvent` there); pinching zooms (above).
        if cfg!(target_os = "macos") && response.hovered() && self.zoom != Zoom::Fit {
            self.pan += ui.input(|i| trackpad_pan(&i.events));
        }
        let shown = size * scale;
        // The fitted photo's long side in device pixels, rounded up to 128 so small layout changes keep the preview.
        // Previews developed at this size are as sharp on screen as the full fit source and cost a fraction of it.
        self.view_need = (self.zoom == Zoom::Fit && shown.x > 0.0 && shown.y > 0.0).then(|| {
            let long = (shown.x.max(shown.y) * ppp).ceil() as usize;
            #[cfg(any(windows, target_os = "macos"))]
            let long = long.max(crate::native_hdr::fitted_side(self, size));
            long.div_ceil(128).max(4) * 128
        });
        // The open tool takes the pointer first (in frame coordinates); otherwise a drag pans.
        let view = ViewTransform { rect: self.photo_rect.unwrap_or(area) };
        let used = self.doc.current().is_some() && tools::dispatch(ui, &response, &view, self, tools);
        if self.zoom != Zoom::Fit && response.dragged_by(egui::PointerButton::Primary) && !used {
            self.pan += response.drag_delta();
        }
        // Keep the photo covering the view when it is larger, centred when smaller.
        let limit = ((shown - area.size()) / 2.0).max(Vec2::ZERO);
        self.pan = self.pan.clamp(-limit, limit);
        if response.double_clicked_by(egui::PointerButton::Primary) && !used {
            self.run("view.zoom_toggle");
        }
        // Whole device pixels at the top left, so 1:1 shows each photo pixel on one screen pixel.
        let min = area.center() + self.pan - shown / 2.0;
        let min = Pos2::new((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
        let rect = Rect::from_min_size(min, shown);
        self.photo_rect = Some(rect);
        // Visible viewport ROI for true 1:1 GPU tile rendering when zoomed in past the viewport.
        // Wait until a full-frame base texture exists so switching photos while zoomed always
        // populates the base frame and whole-frame histogram first.
        let has_base_frame = self.gpu_texture.is_some() || self.texture.is_some();
        self.tile_need = if has_base_frame
            && self.zoom != Zoom::Fit
            && self.wgpu_render_state.is_some()
            && rect.width() > 1.0
            && rect.height() > 1.0
            && (rect.width() > area.width() * 1.1 || rect.height() > area.height() * 1.1)
        {
            let vis = area.intersect(rect);
            if vis.width() > 1.0 && vis.height() > 1.0 {
                let u0 = ((vis.left() - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let v0 = ((vis.top() - rect.top()) / rect.height()).clamp(0.0, 1.0);
                let u1 = ((vis.right() - rect.left()) / rect.width()).clamp(0.0, 1.0);
                let v1 = ((vis.bottom() - rect.top()) / rect.height()).clamp(0.0, 1.0);
                let pad_u = ((u1 - u0) * 0.2).max(0.03);
                let pad_v = ((v1 - v0) * 0.2).max(0.03);
                let q_lo = |x: f32| ((x * 16.0).floor() / 16.0).clamp(0.0, 1.0);
                let q_hi = |x: f32| ((x * 16.0).ceil() / 16.0).clamp(0.0, 1.0);
                let roi = [q_lo(u0 - pad_u), q_lo(v0 - pad_v), q_hi(u1 + pad_u), q_hi(v1 + pad_v)];
                ((roi[2] - roi[0]) * (roi[3] - roi[1]) < 0.75).then_some(roi)
            } else {
                None
            }
        } else {
            None
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let thumb = self.thumbs.get(&self.doc.current_id());
        let base_id = self
            .gpu_texture
            .as_ref()
            .map(|(id, _)| *id)
            .or_else(|| self.texture.as_ref().map(egui::TextureHandle::id))
            .or_else(|| thumb.and_then(|t| t.texture.as_ref()).map(egui::TextureHandle::id));
        if let Some(id) = base_id {
            painter.image(id, rect, uv, Color32::WHITE);
        } else if self.doc.current().is_some() {
            checkerboard(&painter, rect);
        } else {
            crate::commands::settings::paint_empty(&painter, area);
        }
        if let Some((tile_id, [u0, v0, u1, v1], _)) = &self.gpu_tile_texture {
            let tile_rect = Rect::from_min_max(
                Pos2::new(rect.left() + u0 * rect.width(), rect.top() + v0 * rect.height()),
                Pos2::new(rect.left() + u1 * rect.width(), rect.top() + v1 * rect.height()),
            );
            painter.image(*tile_id, tile_rect, uv, Color32::WHITE);
        }
        self.clipping_overlay(ui, &painter, rect);
        self.update_cursor_rgb(ui, rect, response.hovered() && !self.modal_open());
        if self.soft_proof.enabled && self.proof_available() && self.doc.current().is_some() {
            let label = if self.soft_proof.gamut_warning {
                crate::text::tr_with("소프트 프루핑 · {0} (색역 경고)", &[&self.soft_proof.target])
            } else {
                crate::text::tr_with("소프트 프루핑 · {0}", &[&self.soft_proof.target])
            };
            let galley = painter.layout_no_wrap(label, t::font(t::FONT_SIZE), t::BRAND);
            let badge = Rect::from_min_size(
                Pos2::new(area.right() - galley.size().x - 22.0, area.top() + 10.0),
                galley.size() + Vec2::new(12.0, 6.0),
            );
            painter.rect_filled(badge, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(16, 18, 21, 210));
            painter.rect_stroke(badge, CornerRadius::ZERO, Stroke::new(1.0, t::BRAND), StrokeKind::Inside);
            painter.galley(badge.min + Vec2::new(6.0, 3.0), galley, t::BRAND);
        }
        if self.doc.current().is_some() {
            tools::paint(ui, &ViewTransform { rect }, self, tools);
        }
        if self.debug_overlay {
            self.paint_debug_overlay(ui, area);
        }
    }

    /// Samples the live pixel RGB % (0..100) under the pointer for the Histogram readout.
    fn update_cursor_rgb(&mut self, ui: &Ui, rect: Rect, hovered: bool) {
        let pointer = hovered
            .then(|| ui.input(|i| i.pointer.hover_pos()))
            .flatten()
            .filter(|p| rect.contains(*p) && rect.width() > 0.0 && rect.height() > 0.0);
        let (Some(pos), Some(frame)) = (pointer, self.frame.clone()) else {
            self.cursor_rgb = None;
            self.cursor_sample_key = None;
            return;
        };
        let u = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let v = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let (pw, ph) = (frame.preview.width, frame.preview.height);
        if pw == 0 || ph == 0 {
            self.cursor_rgb = None;
            return;
        }
        let (lu, lv) = match frame.tile_uv {
            Some([u0, v0, u1, v1]) if u >= u0 && u <= u1 && v >= v0 && v <= v1 => {
                (((u - u0) / (u1 - u0).max(1e-6)).clamp(0.0, 1.0), ((v - v0) / (v1 - v0).max(1e-6)).clamp(0.0, 1.0))
            }
            Some(_) => {
                self.cursor_rgb = None;
                self.cursor_sample_key = None;
                return;
            }
            None => (u, v),
        };
        let px = ((lu * (pw - 1) as f32).round() as usize).min(pw - 1);
        let py = ((lv * (ph - 1) as f32).round() as usize).min(ph - 1);
        let key = (frame.id, px as u32, py as u32);
        if let Some((asked, rx)) = &self.cursor_readback {
            match rx.try_recv() {
                Ok(sample) => {
                    let asked = *asked;
                    self.cursor_readback = None;
                    if asked.0 == frame.id {
                        self.show_cursor_sample(asked, sample);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.cursor_readback = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.cursor_sample_key == Some(key) {
            return;
        }
        if !frame.preview.rgba.is_empty() {
            let at = (py * pw + px) * 4;
            self.show_cursor_sample(key, frame.preview.rgba.get(at..at + 3).map(|c| [c[0], c[1], c[2]]));
        } else if let Some(gpu) = &frame.gpu {
            if self.cursor_sample_key.is_none_or(|last| last.0 != frame.id) {
                self.cursor_rgb = None;
            }
            if self.cursor_readback.is_none() {
                let gpu = gpu.clone();
                let ctx = ui.ctx().clone();
                let (tx, rx) = std::sync::mpsc::channel();
                self.cursor_readback = Some((key, rx));
                rayon::spawn(move || {
                    let sample =
                        gpu.gpu.read_pixel_rgba8(&gpu.presented.texture, key.1, key.2).ok().map(|c| [c[0], c[1], c[2]]);
                    let _ = tx.send(sample);
                    ctx.request_repaint();
                });
            }
        } else {
            self.show_cursor_sample(key, None);
        }
    }

    fn show_cursor_sample(&mut self, key: (u64, u32, u32), sample: Option<[u8; 3]>) {
        self.cursor_sample_key = Some(key);
        self.cursor_rgb = sample.map(|[r, g, b]| [r as f32 / 2.55, g as f32 / 2.55, b as f32 / 2.55]);
    }

    /// The clipping overlay over the photo (`drawForeground`): highlights red, shadows blue.
    fn clipping_overlay(&mut self, ui: &Ui, painter: &egui::Painter, rect: Rect) {
        let (shadows, highlights) = (self.clip_shadows, self.clip_highlights);
        let Some(frame) = self.frame.clone().filter(|_| shadows || highlights) else { return };
        let key = (frame.id, shadows, highlights);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        if let (Some(gpu_preview), Some(rs)) = (&frame.gpu, self.wgpu_render_state.clone()) {
            let stale = self.gpu_clip_texture.as_ref().is_none_or(|c| c.key != key);
            if stale {
                if let Ok((tex, view)) = gpu_preview.gpu.render_clip_overlay(
                    &gpu_preview.presented.clip_bits,
                    gpu_preview.presented.width,
                    gpu_preview.presented.height,
                    shadows,
                    highlights,
                ) {
                    let filter = grainy_gpu::wgpu::FilterMode::Nearest;
                    let mut renderer = rs.renderer.write();
                    let id = match &self.gpu_clip_texture {
                        Some(clip) => {
                            renderer.update_egui_texture_from_wgpu_texture(&rs.device, &view, filter, clip.id);
                            clip.id
                        }
                        None => renderer.register_native_texture(&rs.device, &view, filter),
                    };
                    self.gpu_clip_texture = Some(crate::context::GpuClipTexture {
                        id,
                        key,
                        tile_uv: frame.tile_uv,
                        _texture: tex,
                        _view: view,
                    });
                } else if let Some(old) = self.gpu_clip_texture.take() {
                    self.defer_gpu_free(old.id);
                }
            }
            if let Some(clip) = &self.gpu_clip_texture {
                let target = match clip.tile_uv {
                    Some([u0, v0, u1, v1]) => Rect::from_min_max(
                        Pos2::new(rect.left() + u0 * rect.width(), rect.top() + v0 * rect.height()),
                        Pos2::new(rect.left() + u1 * rect.width(), rect.top() + v1 * rect.height()),
                    ),
                    None => rect,
                };
                painter.image(clip.id, target, uv, Color32::WHITE);
            }
            return;
        }
        if self.clip_texture.as_ref().is_none_or(|(_, k)| *k != key) {
            let rgba = crate::histogram::overlay(&frame.clipping, shadows, highlights);
            let image = egui::ColorImage::from_rgba_unmultiplied([frame.preview.width, frame.preview.height], &rgba);
            let texture = ui.ctx().load_texture("clipping", image, egui::TextureOptions::NEAREST);
            self.clip_texture = Some((texture, key));
        }
        if let Some((texture, _)) = &self.clip_texture {
            painter.image(texture.id(), rect, uv, Color32::WHITE);
        }
    }

    /// The settings that differ from the defaults, as JSON, the undo depth and the last render (developer
    /// overlay).
    fn paint_debug_overlay(&self, ui: &Ui, area: Rect) {
        let changed = self.doc.edit().changed();
        let (undo, redo) = self.doc.edit().depth();
        let pv = if self.settings().i64("tone_version") >= 4 { "PV4" } else { "PV3" };
        let mut text = format!("debug · {pv} · undo {undo} · redo {redo} · {} changed", changed.len());
        if let Some(frame) = &self.frame {
            let path = match (frame.gpu.is_some(), frame.tile_uv.is_some()) {
                (true, true) => "GPU Zero-Copy 1:1 Tile",
                (true, false) => "GPU Zero-Copy",
                (false, _) => "CPU",
            };
            text.push('\n');
            text.push_str(&format!(
                "render {:.1} ms · {} × {} ({:?}, {path}) · cache hits {} misses {}",
                frame.millis, frame.preview.width, frame.preview.height, frame.resolution, frame.hits, frame.misses
            ));
        }
        if !changed.is_empty() {
            let json = serde_json::to_string_pretty(&serde_json::Value::Object(changed)).unwrap_or_default();
            text.push('\n');
            text.push_str(&json);
        }
        let font = egui::FontId::monospace(11.0);
        let galley = ui.painter().layout(text, font, Color32::from_rgb(0xc9, 0xf1, 0xf2), 360.0);
        let rect = Rect::from_min_size(area.min + Vec2::new(10.0, 10.0), galley.size() + Vec2::splat(12.0));
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(16, 18, 21, 215));
        painter.galley(rect.min + Vec2::splat(6.0), galley, Color32::WHITE);
    }

    fn view_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let fit_tip = tr_with!("사진 전체를 화면에 맞춥니다. ({0}로 1:1과 번갈아 봅니다)", hint("Z"));
            if widgets::button(ui, &tr("맞춤")).on_hover_text(fit_tip).clicked() {
                self.run("view.fit");
            }
            let actual_tip = tr_with!("원본 100%로 봅니다. ({0}로 화면 맞춤과 번갈아 봅니다)", hint("Z"));
            if widgets::button(ui, "1:1").on_hover_text(actual_tip).clicked() {
                self.run("view.actual");
            }
            let clip_tip = tr_with!("검정이나 흰색으로 날아간 곳을 사진 위에 표시합니다. ({0})", hint("J"));
            let clipping = self.clip_shadows || self.clip_highlights;
            if Button::new(&tr("클리핑")).checked(clipping).show(ui).on_hover_text(clip_tip).clicked() {
                self.run("view.clipping");
            }
            ui.add_space(2.0);
            let zoom = match self.zoom {
                Zoom::Fit => tr("화면 맞춤"),
                Zoom::Actual => "100%".to_owned(),
                Zoom::Scale(s) => {
                    let percent = (s * ui.ctx().pixels_per_point() * 100.0).round();
                    crate::text::tr_with("화면 {0}%", &[&percent.to_string()])
                }
            };
            widgets::label(ui, &zoom, t::MUTED, t::FONT_SIZE);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if widgets::button(ui, &tr("다시 실행")).clicked() {
                    self.run("develop.redo");
                }
                if widgets::button(ui, &tr("실행 취소")).clicked() {
                    self.run("develop.undo");
                }
                let before =
                    Button::new(&tr("원본 비교")).checked(self.before).show(ui).on_hover_text(tr("단축키: \\"));
                if before.clicked() {
                    self.run("view.compare_original");
                }
                let compare = Button::new(&tr("비교"))
                    .checked(self.compare)
                    .show(ui)
                    .on_hover_text(tr("두 장을 나란히 봅니다: 보정 전·후 또는 다른 사진과. 단축키: C"));
                if compare.clicked() {
                    self.run("view.compare");
                }
            });
        });
    }

    fn filmstrip(&mut self, ui: &mut Ui) {
        let current = self.doc.current_index();
        egui::ScrollArea::horizontal().id_salt("filmstrip").auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let count = self.doc.len();
                if count == 0 {
                    return;
                }
                let step = STRIP_CELL + Vec2::new(6.0, 0.0);
                let (area, _) = ui.allocate_exact_size(Vec2::new(step.x * count as f32, step.y), Sense::hover());
                let rect = |index: usize| Rect::from_min_size(area.min + Vec2::new(index as f32 * step.x, 0.0), step);
                if self.scroll_to_current {
                    if let Some(index) = current {
                        ui.scroll_to_rect(rect(index), None);
                        self.scroll_to_current = false;
                    }
                }
                let clip = ui.clip_rect();
                let first = ((clip.left() - area.left()) / step.x).floor().max(0.0) as usize;
                let end = (((clip.right() - area.left()) / step.x).ceil().max(0.0) as usize).min(count);
                for index in first.min(count)..end {
                    let bounds = rect(index);
                    let response = ui.interact(bounds, egui::Id::unique(("filmstrip_cell", index)), Sense::click());
                    let cell = bounds.shrink2(Vec2::new(3.0, 0.0));
                    self.thumbnail(ui, cell, index, response.hovered(), (7.0, 36.0));
                    response.context_menu(|ui| self.photo_context_menu(ui, index));
                    if response.clicked() {
                        self.click_photo(ui, index);
                    }
                }
            });
        });
    }

    /// A click on a thumbnail: opens the photo; Shift selects a range from the anchor;
    /// Ctrl/Cmd toggles one photo (or Ctrl/Cmd+Shift adds a range).
    fn click_photo(&mut self, ui: &Ui, index: usize) {
        let (shift, command) = ui.input(|i| {
            let mut shift = i.modifiers.shift;
            let mut command = i.modifiers.command;
            for e in &i.events {
                if let egui::Event::PointerButton { button: egui::PointerButton::Primary, modifiers, .. } = e {
                    shift |= modifiers.shift;
                    command |= modifiers.command;
                }
            }
            (shift, command)
        });
        if shift {
            self.doc.select_range(index, command);
            self.selection_cursor = self.doc.photos().get(index).map(|p| p.id);
        } else if command {
            self.doc.toggle_selected(index);
            self.selection_cursor = self.doc.photos().get(index).map(|p| p.id);
        } else {
            self.select_photo(index);
        }
    }

    /// Port of `PhotoDelegate.paint`: the frame, the thumbnail, the cache's status badge, the name (in the colour
    /// label's colour) with the pick/reject mark, and the stars.
    fn thumbnail(&mut self, ui: &Ui, cell: Rect, index: usize, hover: bool, inset: (f32, f32)) {
        let photo = &self.doc.photos()[index];
        let id = photo.id;
        let rect = Rect::from_min_max(cell.min + Vec2::new(4.0, 3.0), cell.max - Vec2::new(4.0, 3.0));
        let current = id == self.doc.current_id();
        let selected = current || self.doc.is_selected(id);
        let multi = selected && self.doc.multiple_selected();
        let (border, fill) = if current && multi {
            (t::BRAND, t::LIST_SELECTED)
        } else if selected {
            (t::LIST_SELECTED_BORDER, t::LIST_SELECTED)
        } else if hover {
            (t::BUTTON_BORDER, t::LIST_HOVER)
        } else {
            (t::LIST_BG, t::LIST_BG)
        };
        let painter = ui.painter();
        painter.rect(rect, CornerRadius::ZERO, fill, Stroke::new(1.0, border), StrokeKind::Inside);
        let area = Rect::from_min_max(rect.min + Vec2::splat(inset.0), rect.max - Vec2::new(inset.0, inset.1));
        let thumb = self.thumbs.get(&id);
        match thumb {
            Some(thumb) if thumb.texture.is_some() && thumb.size.x > 0.0 => {
                let scale = (area.width() / thumb.size.x).min(area.height() / thumb.size.y);
                let target = Rect::from_center_size(area.center(), thumb.size * scale);
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                if let Some(texture) = &thumb.texture {
                    painter.image(texture.id(), target, uv, Color32::WHITE);
                }
            }
            Some(thumb) if thumb.error.is_some() => {
                painter.rect_filled(area, CornerRadius::ZERO, t::THUMB_EMPTY);
                painter.text(area.center(), egui::Align2::CENTER_CENTER, "!", t::bold(18.0), t::MUTED);
            }
            _ => {
                painter.rect_filled(area, CornerRadius::ZERO, t::THUMB_EMPTY);
            }
        }
        if let Some(status) = thumb.map(|t| t.status.as_str()).filter(|s| !s.is_empty()) {
            let badge = Rect::from_min_size(area.min, Vec2::new(area.width(), 18.0));
            painter.rect_filled(badge, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(22, 27, 33, 210));
            let galley = widgets::elided(ui, &tr(status), t::font(t::FONT_SIZE), t::BRAND, badge.width());
            painter.galley(badge.center() - galley.size() / 2.0, galley, t::BRAND);
        }
        let color = match photo.label.as_str() {
            "빨강" => Color32::from_rgb(0xec, 0x99, 0x8c),
            "노랑" => Color32::from_rgb(0xee, 0xd0, 0x8e),
            "초록" => Color32::from_rgb(0x95, 0xc9, 0xaa),
            "파랑" => Color32::from_rgb(0x95, 0xbc, 0xe9),
            "보라" => Color32::from_rgb(0xc6, 0xa2, 0xe2),
            _ if current => t::BRAND,
            _ => t::VALUE,
        };
        let (name, stars) = photo.caption();
        let label = Rect::from_min_size(
            Pos2::new(rect.left() + 5.0, rect.bottom() - 33.0),
            Vec2::new(rect.width() - 10.0, 17.0),
        );
        let galley = widgets::elided_middle(ui, &name, t::font(t::FONT_SIZE), color, label.width());
        painter.galley(label.center() - galley.size() / 2.0, galley, color);
        if !stars.is_empty() {
            let line = label.translate(Vec2::new(0.0, 16.0));
            let galley = widgets::galley(ui, stars, t::font(t::FONT_SIZE), color);
            painter.galley(line.center() - galley.size() / 2.0, galley, color);
        }
        self.thumbs_drawn.insert(id, index);
    }

    /// The library toolbar: the source's name and count, search, sort and its direction (`change_sort`).
    fn library_toolbar(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 8.0;
        let query = self.doc.query();
        let source = if let Some(cid) = query.collection {
            self.doc.collections().iter().find(|c| c.id == cid).map(|c| c.name.clone()).unwrap_or_else(|| tr("컬렉션"))
        } else if let Some(folder) = &query.folder {
            std::path::Path::new(folder)
                .file_name()
                .map_or_else(|| folder.clone(), |n| n.to_string_lossy().into_owned())
        } else {
            let filter =
                crate::context::FILTERS.iter().find(|(m, _)| *m == query.mode).map_or("모든 사진", |(_, l)| *l);
            tr(filter)
        };
        let count = if self.doc.listing_pending() {
            tr("불러오는 중…")
        } else {
            crate::text::tr_with("{0}장", &[&self.doc.len().to_string()])
        };
        ui.horizontal(|ui| {
            widgets::label(ui, &source, t::MUTED, t::FONT_SIZE);
            ui.add_space(8.0);
            widgets::label(ui, &count, t::MUTED, t::FONT_SIZE);
        });
        ui.horizontal(|ui| {
            widgets::label(ui, "YOUR LIGHT. YOUR LOOK.", t::MUTED, t::FONT_SIZE);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if Button::new("↓")
                    .checked(query.descending)
                    .width(35.0)
                    .show(ui)
                    .on_hover_text(tr("역순으로 정렬"))
                    .clicked()
                {
                    self.change_query(|q| q.descending = !q.descending);
                }
                let keys = grainy_service::SORTS;
                let sorts: Vec<String> =
                    ["가져온 순서", "파일 이름", "촬영 시간", "별점"].iter().map(|s| tr(s)).collect();
                let index = keys.iter().position(|k| *k == query.sort).unwrap_or(0);
                if let Some(i) = crate::panels::controls::choice(ui, "sort_order", "", &sorts, index, 120.0) {
                    self.change_query(|q| q.sort = keys[i].to_owned());
                }
                ui.add_space(6.0);
                ui.allocate_ui(Vec2::new(230.0, 30.0), |ui| {
                    let mut search = std::mem::take(&mut self.search);
                    let changed =
                        widgets::line_edit(ui, "library_search", &mut search, &tr("파일명 · 키워드 검색")).changed();
                    if changed {
                        let text = search.clone();
                        self.change_query(|q| q.search = text);
                    }
                    self.search = search;
                });
            });
        });
    }

    fn grid(&mut self, ui: &mut Ui) {
        let area = ui.max_rect();
        ui.painter().rect_filled(area, CornerRadius::ZERO, t::BG);
        egui::ScrollArea::vertical().id_salt("grid").auto_shrink([false, false]).show(ui, |ui| {
            // `setGridSize(QSize(228,190))`, `setSpacing(8)`: cells left to right, wrapping.
            let width = ui.available_width();
            let step = GRID_CELL + Vec2::splat(8.0);
            let columns = (((width - 8.0) / step.x).floor() as usize).max(1);
            self.grid_columns = columns;
            let count = self.doc.len();
            let rows = count.div_ceil(columns);
            let (area, _) = ui.allocate_exact_size(Vec2::new(width, 8.0 + rows as f32 * step.y), Sense::hover());
            if std::mem::take(&mut self.scroll_to_current) {
                if let Some(target_idx) = self.selection_cursor_index() {
                    let target_offset = Vec2::new(
                        8.0 + (target_idx % columns) as f32 * step.x,
                        8.0 + (target_idx / columns) as f32 * step.y,
                    );
                    let target_rect = Rect::from_min_size(area.min + target_offset, GRID_CELL);
                    ui.scroll_to_rect(target_rect, None);
                }
            }
            let clip = ui.clip_rect();
            let first = ((clip.top() - area.top() - 8.0) / step.y).floor().max(0.) as usize;
            let end = ((clip.bottom() - area.top() - 8.0) / step.y).floor().max(0.) as usize + 1;
            for index in first.saturating_mul(columns).min(count)..end.saturating_mul(columns).min(count) {
                let offset =
                    Vec2::new(8.0 + (index % columns) as f32 * step.x, 8.0 + (index / columns) as f32 * step.y);
                let cell = Rect::from_min_size(area.min + offset, GRID_CELL);
                let response = ui.interact(cell, egui::Id::unique(("grid_cell", index)), Sense::click());
                self.thumbnail(ui, cell, index, response.hovered(), (7.0, 36.0));
                response.context_menu(|ui| self.photo_context_menu(ui, index));
                if response.double_clicked() {
                    self.select_photo(index);
                    self.set_zoom(Zoom::Fit);
                    self.set_mode(Mode::Develop);
                } else if response.clicked() {
                    self.click_photo(ui, index);
                }
            }
        });
    }
}

/// A grey checkerboard where no picture has been rendered yet.
fn checkerboard(painter: &egui::Painter, rect: Rect) {
    let cell = 16.0;
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_gray(58));
    let columns = (rect.width() / cell).ceil().min(400.0) as i32;
    let rows = (rect.height() / cell).ceil().min(400.0) as i32;
    for y in 0..rows {
        for x in 0..columns {
            if (x + y) % 2 == 0 {
                let min = rect.min + Vec2::new(x as f32 * cell, y as f32 * cell);
                let square = Rect::from_min_size(min, Vec2::splat(cell)).intersect(rect);
                painter.rect_filled(square, CornerRadius::ZERO, Color32::from_gray(82));
            }
        }
    }
}

fn hint(key: &str) -> String {
    grainy_i18n::shortcuts::key_hint(key, grainy_i18n::Platform::current()).unwrap_or_else(|| key.to_owned())
}

/// What a trackpad (or Magic Mouse) scroll moves the photo by this frame, on macOS: luma/widgets.py `wheelEvent`
/// with `MAC` (scrolls in pixels, many events per swipe, move the photo as everywhere on macOS:
/// `scrollBar.setValue(value - delta)`). Scrolls in points only, momentum included; a scroll with ⌘ is egui's zoom.
pub(crate) fn trackpad_pan(events: &[egui::Event]) -> Vec2 {
    events
        .iter()
        .filter_map(|event| match event {
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers, .. }
                if !modifiers.command =>
            {
                Some(*delta)
            }
            _ => None,
        })
        .fold(Vec2::ZERO, |sum, delta| sum + delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trackpad_scroll_pans_and_a_wheel_or_command_scroll_does_not() {
        let wheel = |unit, delta: Vec2, modifiers| egui::Event::MouseWheel {
            unit,
            delta,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Trackpad,
            modifiers,
        };
        let none = egui::Modifiers::NONE;
        let events = [
            wheel(egui::MouseWheelUnit::Point, Vec2::new(3.0, -4.0), none),
            wheel(egui::MouseWheelUnit::Point, Vec2::new(1.0, 2.0), egui::Modifiers::SHIFT),
            wheel(egui::MouseWheelUnit::Point, Vec2::new(50.0, 50.0), egui::Modifiers::COMMAND),
            wheel(egui::MouseWheelUnit::Line, Vec2::new(0.0, 1.0), none),
            egui::Event::Zoom(1.1),
        ];
        assert_eq!(trackpad_pan(&events), Vec2::new(4.0, -2.0));
        assert_eq!(trackpad_pan(&[]), Vec2::ZERO);
    }
}
