//! The Colour filter panel (컬러 필터).

use eframe::egui::Ui;
use grainy_i18n::tr;

use super::controls::{self};
use super::Panel;
use crate::adjustments::Place;
use crate::context::AppContext;
use crate::widgets::{self};

/// Colour filter presets: (key, Korean title). Port of `photo_filter.FILTERS` (names only).
const PHOTO_FILTERS: [(&str, &str); 18] = [
    ("warming85", "(85) 웜 필터"),
    ("warming81", "(81) 웜 필터"),
    ("cooling80", "(80) 쿨 필터"),
    ("cooling82", "(82) 쿨 필터"),
    ("red", "레드"),
    ("orange", "오렌지"),
    ("yellow", "옐로"),
    ("green", "그린"),
    ("cyan", "시안"),
    ("blue", "블루"),
    ("violet", "바이올렛"),
    ("magenta", "마젠타"),
    ("dark_red", "다크 레드"),
    ("navy_blue", "네이비 블루"),
    ("emerald_green", "에메랄드 그린"),
    ("chrome_yellow", "크롬 옐로"),
    ("marine_blue", "마린 블루"),
    ("", ""),
];

/// The photo_filter panel.
pub struct PhotoFilter;

impl Panel for PhotoFilter {
    fn key(&self) -> &'static str {
        "photo_filter"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        body(ui, cx);
    }
}

fn body(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let current = cx.settings().str("photo_filter").to_owned();
    let filters = &PHOTO_FILTERS[..PHOTO_FILTERS.len() - 1];
    let index = filters.iter().position(|(k, _)| *k == current).unwrap_or(0);
    let names: Vec<String> = filters.iter().map(|(_, n)| tr(n)).collect();
    if let Some(chosen) = controls::choice(ui, "photo_filter", &tr("필터 프리셋"), &names, index, width) {
        if filters[chosen].0 != current {
            cx.set_and_commit("photo_filter", filters[chosen].0, "컬러 필터");
        }
    }
    cx.sliders(ui, Place::Panel("photo_filter"));
    let mut keep = cx.settings().bool("photo_filter_luminosity");
    if widgets::checkbox(ui, &mut keep, &tr("밝기 유지")).changed() {
        cx.set_and_commit("photo_filter_luminosity", keep, "컬러 필터");
    }
    widgets::muted(ui, &tr("필터 선택 후 강도를 조절하세요. 흑백 사진에서는 색에 따른 밝기 차이를 조절합니다."));
}
