//! The Develop column's panels. The panel table (order, titles, switches, which keys each holds: port of
//! luma/develop_panels.py and luma/panel_keys.py) is [`table`]; each panel's body is a [`Panel`] in its own file;
//! [`column`] draws the column with the histogram, the tool strip and the accordion.
//!
//! **Adding a panel:** write `panels/<key>.rs` with a type implementing [`Panel`] (its key is the panel's key in
//! [`PANELS`]) and add one line to [`registry`]. A panel without a registered body shows its slider rows from
//! [`crate::adjustments`] (`detail`, `effects`).

pub mod basic;
pub mod calibration;
pub mod collections;
pub mod column;
pub mod controls;
pub mod crop;
pub mod curve;
pub mod detail;
pub mod film;
pub mod grading;
pub mod healing;
pub mod lens;
pub mod masks;
pub mod masks_local;
pub mod masks_ranges;
pub mod metadata;
pub mod mixer;
pub mod photo_filter;
pub mod presets;
pub mod table;
pub mod transform;

pub use table::*;

use eframe::egui::Ui;

use crate::context::AppContext;

/// The body of one panel of the Develop column.
pub trait Panel {
    /// The panel's key in [`PANELS`] (`basic`, `film`…).
    fn key(&self) -> &'static str;

    /// Draws the expanded body; edits go through `cx` (`cx.slider`, `cx.set_and_commit`, `cx.run`…).
    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext);
}

/// A panel whose editors come in a later work package: a titled placeholder.
pub struct Placeholder {
    pub key: &'static str,
    pub title: &'static str,
    pub height: f32,
}

impl Panel for Placeholder {
    fn key(&self) -> &'static str {
        self.key
    }

    fn ui(&mut self, ui: &mut Ui, _cx: &mut AppContext) {
        controls::placeholder(ui, &grainy_i18n::tr(self.title), self.height);
    }
}

/// The panels with a body, one line each (panels not listed show their slider rows).
pub fn registry() -> Vec<Box<dyn Panel>> {
    vec![
        Box::new(crop::Crop),
        Box::new(healing::Healing),
        Box::new(masks::Masks),
        Box::new(basic::Basic),
        Box::new(film::Film),
        Box::new(curve::Curve),
        Box::new(mixer::Mixer),
        Box::new(grading::Grading),
        Box::new(detail::Detail),
        Box::new(lens::Lens),
        Box::new(transform::Transform),
        Box::new(photo_filter::PhotoFilter),
        Box::new(calibration::Calibration),
        Box::new(metadata::Metadata),
    ]
}
