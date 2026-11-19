//! 복사·동기화 항목 (`LibraryManager.choose_copy`): one check box per `manager.EDIT_GROUPS` group, ticked when any
//! key of the group is in the copy set. The HDR group is hidden while the oracle's `engine.HDR_FEATURE` is off but
//! keeps its state.

use std::collections::BTreeSet;

use eframe::egui;

use super::Dialog;
use crate::commands::sync;
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// Port of reading `engine.HDR_FEATURE`: the HDR check box shows only while the HDR mode is on.
pub fn hdr_feature() -> bool {
    crate::hdr::enabled()
}

/// `manager.EDIT_GROUPS`: Grainy's copy/sync groups.
pub const EDIT_GROUPS: [(&str, &[&str]); 16] = [
    ("작업 색공간", &["working_space"]),
    (
        "빛",
        &[
            "exposure",
            "contrast",
            "highlights",
            "shadows",
            "whites",
            "blacks",
            "tone_mapping",
            "drt_contrast",
            "drt_toe",
            "drt_shoulder",
            "drt_white",
        ],
    ),
    ("HDR", &["hdr", "hdr_limit", "hdr_brightness", "sdr_exposure", "sdr_compression"]),
    (
        "화이트 밸런스",
        &[
            "temperature",
            "tint",
            "kelvin",
            "wb_reference",
            "kelvin_enabled",
            "raw_mode",
            "raw_kelvin",
            "raw_tint",
            "raw_neutral",
        ],
    ),
    ("RAW 프로파일", &["dcp_profile", "dcp_huesat", "dcp_look", "dcp_tone", "dcp_exposure"]),
    (
        "색상",
        &[
            "saturation",
            "vibrance",
            "monochrome",
            "mixer_mode",
            "hsl",
            "grading",
            "grading_balance",
            "calibration",
            "point_colors",
            "camera_matrix",
        ],
    ),
    ("필름", &["film_look", "film_strength"]),
    ("컬러 필터", &["photo_filter", "photo_filter_enabled", "photo_filter_density", "photo_filter_luminosity"]),
    ("커브", &["curve", "rgb_curves", "parametric"]),
    (
        "디테일",
        &[
            "clarity",
            "texture",
            "dehaze",
            "sharpen",
            "sharpen_radius",
            "sharpen_detail",
            "sharpen_mask",
            "noise_luma",
            "noise_color",
        ],
    ),
    (
        "효과",
        &[
            "grain",
            "grain_size",
            "grain_roughness",
            "vignette",
            "vignette_midpoint",
            "vignette_roundness",
            "vignette_feather",
            "vignette_highlights",
        ],
    ),
    ("네거티브 반전", &["negative", "negative_base", "negative_peak", "negative_range", "negative_density"]),
    (
        "렌즈·원근",
        &[
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
        ],
    ),
    ("크롭·회전", &["crop", "rotation", "straighten", "flip"]),
    ("마스크", &["masks"]),
    ("복구", &["retouch"]),
];

/// The group of a title (`EDIT_GROUPS[title]`).
pub fn group(title: &str) -> &'static [&'static str] {
    EDIT_GROUPS.iter().find(|(name, _)| *name == title).map_or(&[], |(_, keys)| *keys)
}

/// The group titles ticked for a copy set: `bool(set(keys) & copy_keys)` per group.
pub fn ticked(keys: &BTreeSet<String>) -> Vec<(&'static str, bool)> {
    EDIT_GROUPS.iter().map(|(title, group)| (*title, group.iter().any(|k| keys.contains(*k)))).collect()
}

/// The copy set of the ticked groups: `{k for title, keys in EDIT_GROUPS.items() if checked for k in keys}`.
pub fn keys_of(checks: &[(&'static str, bool)]) -> BTreeSet<String> {
    checks.iter().filter(|(_, on)| *on).flat_map(|(title, _)| group(title).iter().map(|k| (*k).to_owned())).collect()
}

/// The dialog.
pub struct CopyItems {
    checks: Vec<(&'static str, bool)>,
}

impl CopyItems {
    /// The dialog for the current copy set.
    pub fn new(keys: &BTreeSet<String>) -> Self {
        Self { checks: ticked(keys) }
    }

    /// The check boxes as they stand.
    pub fn checks(&self) -> &[(&'static str, bool)] {
        &self.checks
    }

    /// Ticks or clears a group (tests drive the dialog this way).
    pub fn set(&mut self, title: &str, on: bool) {
        if let Some(check) = self.checks.iter_mut().find(|(name, _)| *name == title) {
            check.1 = on;
        }
    }
}

impl Dialog for CopyItems {
    fn id(&self) -> &'static str {
        "copy_items"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut answer = None;
        let response = egui::Modal::new(egui::Id::unique("copy_items")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(300.0);
            ui.label(egui::RichText::new(tr("복사·동기화 항목")).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(6.0);
            for (title, on) in &mut self.checks {
                if *title == "HDR" && !hdr_feature() {
                    continue;
                }
                widgets::checkbox(ui, on, &tr(title));
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, &tr("취소")).clicked() {
                    answer = Some(false);
                }
                ui.add_space(6.0);
                if Button::new(&tr("확인")).kind(Kind::Primary).show(ui).clicked() {
                    answer = Some(true);
                }
            });
        });
        if response.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                sync::choose_done(cx, &self.checks);
                false
            }
            Some(false) => false,
            None => true,
        }
    }
}
