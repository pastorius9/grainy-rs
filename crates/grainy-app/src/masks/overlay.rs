//! The red overlay of the selected mask (오버레이 표시, O): port of the mask overlay of luma/app.py `start_preview`
//! (`capture_mask`: the layer's mask as the develop computes it, red `[250, 65, 55]` with alpha `mask * 105`) and
//! of `StudioTools.overlay_layer`.
//!
//! The oracle captures the mask from its develop (`on_mask`). Here the mask is computed on its own background job
//! with grainy-local's [`layer_mask`](grainy_local::layer_mask) through the engine's geometry
//! (`grainy_geometry::geometry` with the crop, on the settings as shown), on a source of the oracle's live size
//! (720 pixels on the long side), so the UI thread only uploads the finished picture. Layers that read the
//! developed image (luminance and colour ranges, sky) need that image: [`OverlayInput::image`] (work package 9).

use std::sync::Arc;

use eframe::egui::{self, Color32, Painter, Pos2, Rect};
use grainy_core::{Image, Settings};
use grainy_geometry::UseCrop;
use ndarray::Array3;
use serde_json::Value;

use super::layers;
use super::{actions, MaskState};
use crate::context::AppContext;
use crate::imagespace::ViewTransform;
use crate::panels::Tool;

/// The live source's long side (`resize_float(source, 720)`).
pub const LIVE_SIZE: usize = 720;

/// The overlay colour (`rgba[...,0..3] = 250, 65, 55`) and its alpha scale (`mask * 105`).
pub const RED: [u8; 3] = [250, 65, 55];
pub const ALPHA: f32 = 105.0;

/// What the overlay shows and what is being computed.
#[derive(Default)]
pub struct OverlayState {
    pub(crate) texture: Option<egui::TextureHandle>,
    /// The key of the picture in `texture`.
    pub(crate) shown: Option<String>,
    /// The key being computed.
    pub(crate) running: Option<String>,
}

/// What one overlay needs: the settings, the layer, the source shape (h, w) and, for layers that read the
/// developed image, that image at the frame's size.
#[derive(Clone)]
pub struct OverlayInput {
    pub settings: Settings,
    pub layer: usize,
    pub source: (usize, usize),
    pub image: Option<Arc<Image>>,
}

/// The overlay picture: RGBA rows, width, height.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayPicture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// `resize_float`'s size rule for the live source: (h, w) of a photo of `original` (w, h) at most
/// [`LIVE_SIZE`] on its long side.
pub fn live_shape(original: (u32, u32)) -> (usize, usize) {
    let (w, h) = (f64::from(original.0.max(1)), f64::from(original.1.max(1)));
    let long = w.max(h);
    if long <= LIVE_SIZE as f64 {
        return (h as usize, w as usize);
    }
    let factor = LIVE_SIZE as f64 / long;
    let size = |v: f64| ((v * factor).round_ties_even() as usize).max(1);
    (size(h), size(w))
}

/// Port of `overlay_layer`: the layer whose overlay shows, if any.
pub fn overlay_layer(cx: &AppContext, state: &MaskState) -> Option<usize> {
    if !state.overlay_shown() || state.overlay_paused || cx.before() || cx.tool() != Tool::Masks {
        return None;
    }
    // The blue overlay of a local point colour shows instead (`if self.point_overlay_target() is not None`).
    if crate::panels::masks_local::overlay_target(cx).is_some() {
        return None;
    }
    let masks = layers::list(cx.settings());
    if layers::at(&masks, state.layer).is_some() {
        return usize::try_from(state.layer).ok();
    }
    // A stroke on a mask that does not exist yet: the layer it makes, the last one while it is drawn.
    state.pending.as_ref().and_then(|_| masks.len().checked_sub(1))
}

/// Whether the layer reads the developed image (a range or the sky).
pub fn needs_image(layer: &layers::Obj) -> bool {
    grainy_local::image_dependent(layer)
        || layers::components(layer)
            .iter()
            .any(|c| c.get("type").and_then(Value::as_str) == Some("sky") && layers::truthy(c.get("enabled"), true))
}

/// Computes the overlay picture (off the UI thread). `None` when the layer does not exist in the settings as
/// shown (a switched-off Masks panel shows none, as the oracle's develop does) or needs an image not given.
pub fn compute(input: &OverlayInput) -> Result<Option<OverlayPicture>, String> {
    let s = grainy_core::effective(&input.settings);
    let masks = layers::list(&s);
    let Some(layer) = layers::at(&masks, input.layer as isize) else { return Ok(None) };
    if needs_image(layer) && input.image.is_none() {
        return Ok(None);
    }
    let transform = |mask: &Image| -> grainy_local::Result<Image> {
        grainy_geometry::geometry(mask, &s, UseCrop::Yes, None)
            .map(std::borrow::Cow::into_owned)
            .map_err(|error| grainy_local::Error::Invalid(error.to_string()))
    };
    let (h, w) = input.source;
    let frame = match &input.image {
        Some(image) => image.as_ref().clone(),
        None => {
            let shape = transform(&Array3::zeros((h, w, 1))).map_err(|e| e.to_string())?;
            let (fh, fw, _) = shape.dim();
            Array3::zeros((fh, fw, 3))
        }
    };
    let mask = grainy_local::layer_mask(&frame, (h, w), layer, &transform, None, None).map_err(|e| e.to_string())?;
    let (height, width) = mask.dim();
    let mut rgba = Vec::with_capacity(width * height * 4);
    for &m in &mask {
        rgba.extend_from_slice(&RED);
        // np.uint8(small * 105): float32, truncated.
        rgba.push((m * ALPHA) as u8);
    }
    Ok(Some(OverlayPicture { width, height, rgba }))
}

/// The key of an overlay: the layer, its value and the geometry, and the source.
fn key(input: &OverlayInput) -> String {
    let s = grainy_core::effective(&input.settings);
    let masks = layers::list(&s);
    let layer = masks.get(input.layer).map(Value::to_string).unwrap_or_default();
    format!(
        "{}|{:?}|{}|{}",
        input.layer,
        input.source,
        layer,
        grainy_engine::settings_key(&s, &grainy_engine::GEOMETRY_KEYS)
    )
}

/// Brings the overlay up to date: starts its job when what it shows changed (one job at a time; the newest
/// wish is computed when it ends). Called every frame the Masks panel is drawn.
pub fn refresh(cx: &mut AppContext) {
    actions::state(cx);
    let wanted = {
        let state = cx.state_ref::<MaskState>();
        let layer = state.and_then(|s| overlay_layer(cx, s));
        let source = cx.doc.current().and_then(|p| p.original).map(live_shape);
        match (layer, source) {
            (Some(layer), Some(source)) => {
                Some(OverlayInput { settings: cx.settings().clone(), layer, source, image: None })
            }
            _ => None,
        }
    };
    let Some(input) = wanted else {
        let state = actions::state(cx);
        state.overlay.texture = None;
        state.overlay.shown = None;
        return;
    };
    // Work package 9: layers that read the developed image (ranges, sky, automatic brush masks).
    if crate::tools::range_pick::image_overlay(cx, &input) {
        return;
    }
    let wanted_key = format!("{:?}|{}", cx.doc.current_id(), key(&input));
    let state = actions::state(cx);
    if state.overlay.shown.as_ref() == Some(&wanted_key) || state.overlay.running.is_some() {
        return;
    }
    state.overlay.running = Some(wanted_key.clone());
    let photo = cx.doc.current_id();
    cx.spawn_document_job(
        "mask_overlay",
        "",
        move |_| compute(&input),
        move |cx: &mut AppContext, result| {
            if cx.doc.current_id() != photo {
                return;
            }
            if actions::state(cx).overlay.running.as_ref() != Some(&wanted_key) {
                return;
            }
            let picture = match result {
                Ok(picture) => picture,
                Err(message) => {
                    log::warn!("mask overlay: {message}");
                    None
                }
            };
            let texture = picture.map(|p| {
                let image = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                cx.egui.load_texture("mask_overlay", image, egui::TextureOptions::LINEAR)
            });
            let state = actions::state(cx);
            state.overlay.running = None;
            state.overlay.texture = texture;
            state.overlay.shown = Some(wanted_key);
            cx.egui.request_repaint();
        },
    );
}

/// Draws the overlay over the photo (`painter.drawImage(self.image_rect, self.tool_overlay)`), when it is meant
/// to show now.
pub fn paint(painter: &Painter, view: &ViewTransform, cx: &AppContext) {
    let Some(state) = cx.state_ref::<MaskState>() else { return };
    if overlay_layer(cx, state).is_none() {
        return;
    }
    if let Some(texture) = &state.overlay.texture {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture.id(), view.rect, uv, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_size_rule() {
        assert_eq!(live_shape((97, 65)), (65, 97));
        assert_eq!(live_shape((1440, 960)), (480, 720));
        assert_eq!(live_shape((1000, 3000)), (720, 240));
    }
}
