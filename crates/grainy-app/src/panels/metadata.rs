//! The 사진 정보·관리 panel (luma/app.py, the 관리 page): ORGANIZE (star rating, flag, colour label and the
//! keywords of the open photo), BATCH EDIT (copy the edits, paste them on the selection, with or without crop and
//! rotation) and FILE (the stored file facts, 파일 위치 열기).
//!
//! The rating, flag and label go to the selected photos through the `cull.*` commands; the keywords text is saved
//! when the field loses focus (`editingFinished`) through `metadata.keywords`.

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Ui};
use serde_json::Value;

use super::{controls, Panel};
use crate::commands::{metadata as commands, writable_library, Invocation};
use crate::context::AppContext;
use crate::text::tr;
use crate::theme as t;
use crate::widgets::{self, Button};

/// 붙여넣기에 크롭·회전도 포함 (`copy_crop`): the crop, rotation, straighten and flip join the pasted items
/// (`copy_crop_changed` adds `EDIT_GROUPS['크롭·회전']` to the copy keys). The window has one.
static PASTE_CROP: AtomicBool = AtomicBool::new(false);

/// Whether pasting includes crop and rotation.
pub fn paste_crop() -> bool {
    PASTE_CROP.load(Ordering::Relaxed)
}

/// Sets the 붙여넣기에 크롭·회전도 포함 box.
pub fn set_paste_crop(on: bool) {
    PASTE_CROP.store(on, Ordering::Relaxed);
}

/// The colour labels of the menu, in order.
pub const LABELS: [&str; 5] = ["빨강", "노랑", "초록", "파랑", "보라"];

/// What the panel keeps between frames.
#[derive(Default)]
pub struct MetadataState {
    /// Bumped by every command that changed catalogue rows: the panel reads them again.
    pub revision: u64,
    /// The photo and revision the texts below were read for.
    shown: Option<(u64, u64)>,
    /// The keywords field's text (what is typed, saved on focus loss).
    pub keywords: String,
    /// The keywords as stored.
    stored: String,
    /// The FILE section's text.
    pub info: String,
    /// Whether the open photo's metadata job was running (its end brings the stored info).
    reading: bool,
}

/// The panel.
pub struct Metadata;

impl Panel for Metadata {
    fn key(&self) -> &'static str {
        "metadata"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut AppContext) {
        sync(cx);
        let width = ui.available_width();
        let open = cx.doc.current().is_some();
        let writable = writable_library(cx);
        let (rating, flag, label) =
            cx.doc.current().map(|p| (p.rating, p.flag, p.label.clone())).unwrap_or((0, 0, String::new()));

        widgets::section(ui, "ORGANIZE");
        ui.add_enabled_ui(open, |ui| {
            let stars: Vec<String> = [tr("별점 없음")].into_iter().chain((1..=5).map(|n| "★".repeat(n))).collect();
            if let Some(i) = controls::choice(ui, "metadata_rating", "", &stars, rating.clamp(0, 5) as usize, width) {
                cx.run_with("cull.rating", &Invocation::value(i as i64));
            }
            ui.add_space(4.0);
            let flags = [tr("표시 없음"), tr("선택 표시"), tr("제외 표시")];
            let at = match flag {
                1 => 1,
                -1 => 2,
                _ => 0,
            };
            if let Some(i) = controls::choice(ui, "metadata_flag", "", &flags, at, width) {
                cx.run(["cull.unflag", "cull.pick", "cull.reject"][i]);
            }
            ui.add_space(4.0);
            let labels: Vec<String> = [tr("색 라벨")].into_iter().chain(LABELS.iter().map(|l| tr(l))).collect();
            let at = LABELS.iter().position(|l| *l == label).map_or(0, |i| i + 1);
            if let Some(i) = controls::choice(ui, "metadata_label", "", &labels, at, width) {
                let name = if i == 0 { "" } else { LABELS[i - 1] };
                cx.run_with("cull.color_label", &Invocation::value(name));
            }
            ui.add_space(4.0);
            ui.add_enabled_ui(writable, |ui| {
                let mut text = std::mem::take(&mut cx.state::<MetadataState>().keywords);
                let response = widgets::line_edit(ui, "metadata_keywords", &mut text, &tr("키워드: 여행, 인물, 필름…"));
                let state = cx.state::<MetadataState>();
                state.keywords = text.clone();
                if response.lost_focus() && text != state.stored {
                    cx.run_with("metadata.keywords", &Invocation::value(Value::String(text)));
                }
            });
        });

        widgets::section(ui, "BATCH EDIT");
        widgets::label(
            ui,
            &grainy_i18n::keys_here(&tr("Ctrl / Shift로 여러 사진을 선택하세요.")),
            t::TEXT,
            t::FONT_SIZE,
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(open, |ui| {
            if Button::new(&tr("현재 보정 복사")).width(width).show(ui).clicked() {
                cx.run("develop.copy_edits");
            }
            let mut crop = cx.state::<crate::commands::sync::SyncState>().copy_crop;
            if widgets::checkbox(ui, &mut crop, &tr("붙여넣기에 크롭·회전도 포함")).changed() {
                set_paste_crop(crop);
                cx.run_with("sync.copy_crop", &Invocation::value(crop));
            }
            if Button::new(&tr("선택한 사진에 보정 붙여넣기")).width(width).show(ui).clicked() {
                cx.run("develop.paste_edits");
            }
        });

        widgets::section(ui, "FILE");
        let info = cx.state::<MetadataState>().info.clone();
        ui.add(
            egui::Label::new(egui::RichText::new(info).font(t::font(t::FONT_SIZE)).color(t::TEXT))
                .wrap()
                .selectable(true),
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(open, |ui| {
            if Button::new(&tr("파일 위치 열기")).width(width).show(ui).clicked() {
                cx.run("metadata.show_file");
            }
        });
    }
}

/// Reads the open photo's keywords and file facts again when the photo or the catalogue rows changed (`load_controls`),
/// and the file facts when the photo's metadata job ended (the oracle fills them when the load is done).
pub fn sync(cx: &mut AppContext) {
    let id = cx.doc.current_id();
    let reading = cx.jobs.running("metadata");
    let state = cx.state::<MetadataState>();
    let key = (id, state.revision);
    let finished = state.reading && !reading;
    state.reading = reading;
    let same = state.shown == Some(key);
    if same && !finished {
        return;
    }
    state.shown = Some(key);
    let Some(photo) = cx.doc.current() else {
        let state = cx.state::<MetadataState>();
        state.keywords.clear();
        state.stored.clear();
        state.info.clear();
        return;
    };
    let path = photo.path.to_string_lossy().into_owned();
    let (keywords, info) = match cx.doc.metadata_catalog().and_then(|c| c.photo(id as i64).ok().flatten()) {
        Some(row) => (row["keywords"].as_str().unwrap_or("").to_owned(), row["info"].clone()),
        None => (String::new(), Value::Null),
    };
    let state = cx.state::<MetadataState>();
    state.info = commands::info_text(&path, &info);
    if !same {
        state.keywords = keywords.clone();
    }
    state.stored = keywords;
}
