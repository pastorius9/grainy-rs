//! The Colour grading panel (컬러 그레이딩): shadows, midtones and highlights.

use eframe::egui::Ui;
use grainy_i18n::tr;

use super::controls::list_spec;
use super::Panel;
use crate::adjustments::Place;
use crate::context::AppContext;
use crate::widgets::{self};

/// The grading panel.
pub struct Grading;

impl Panel for Grading {
    fn key(&self) -> &'static str {
        "grading"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let ranges = [(0.0, 360.0), (0.0, 100.0), (-100.0, 100.0)];
    for (i, title) in ["그림자 컬러 그레이딩", "중간톤 컬러 그레이딩", "하이라이트 컬러 그레이딩"].iter().enumerate()
    {
        widgets::muted(ui, &tr(title));
        for (j, name) in ["색상", "채도", "명도"].iter().enumerate() {
            let spec = list_spec("grading", ranges[j].0, ranges[j].1);
            cx.list_slider(ui, &spec, name, i, Some(j));
        }
    }
    cx.sliders(ui, Place::Panel("grading"));
}
