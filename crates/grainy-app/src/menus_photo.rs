//! The right-click menu of a photo in the grid and the filmstrip (APP_PLAN package 14; luma/photo_actions.py
//! `build_menu`): every item in the oracle's order with the oracle's disabled and checked states.
//!
//! [`menu`] builds the menu as data (what the oracle's `QMenu` holds: titles, the `setData` key of each action,
//! enabled, checked), [`AppContext::photo_context_menu`] draws it. Items of work packages that are not merged yet
//! stay in place, disabled (`DEFERRED`).

use eframe::egui::{self, Ui};
use serde_json::{json, Value};

use crate::commands::collections::{self, selected_ids};
use crate::commands::{self, Invocation};
use crate::context::AppContext;
use crate::text::tr;

/// An action of the menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The action's data key (`action.setData(key)` in the oracle).
    pub key: String,
    /// The oracle's Korean title (translated when drawn).
    pub title: String,
    /// The command it runs; empty for an item of a package that is not merged yet.
    pub command: &'static str,
    /// The value given to the command (a star count, a label, 흑백 or 컬러).
    pub value: Option<Value>,
    pub enabled: bool,
    /// `Some` for a checkable item (`choice`).
    pub checked: Option<bool>,
    /// The tooltip (Korean source).
    pub tip: Option<&'static str>,
}

/// A submenu.
#[derive(Debug, Clone, PartialEq)]
pub struct Sub {
    pub key: &'static str,
    pub title: &'static str,
    pub enabled: bool,
    pub children: Vec<Node>,
}

/// One entry of the menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Item(Item),
    Sub(Sub),
    Separator,
}

/// The items whose command belongs to a package that is not merged yet: (key, package). They are listed, greyed
/// out, in the oracle's place.
pub const DEFERRED: &[(&str, &str)] = &[];

/// What the menu reads about the selection and the library (the oracle's `photos`, `w.catalog.*`, `manager.*`).
struct Facts {
    ids: Vec<u64>,
    ratings: Vec<i64>,
    flags: Vec<i64>,
    labels: Vec<String>,
    monochrome: Vec<bool>,
    any_oriented: bool,
    any_stacked: bool,
    current_stacked: bool,
    snapshots: bool,
    plain_collection: bool,
    selected_plain_collection: bool,
    copied: bool,
    previous_edits: bool,
    export_running: bool,
    previous_export: bool,
    collapsed: bool,
}

fn facts(cx: &mut AppContext) -> Facts {
    let ids = selected_ids(cx);
    let (mut ratings, mut flags, mut labels, mut monochrome) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut any_oriented, mut any_stacked) = (false, false);
    for id in &ids {
        if let Some(photo) = cx.doc.index_of(*id).and_then(|i| cx.doc.photos().get(i)) {
            ratings.push(photo.rating);
            flags.push(photo.flag);
            labels.push(photo.label.clone());
            any_stacked |= photo.stack.is_some();
        }
        if let Ok(settings) = cx.doc.stored_settings_of(*id) {
            any_oriented |= settings.i64("rotation") != 0 || settings.bool("flip");
            monochrome.push(settings.bool("monochrome"));
        }
    }
    let library = cx.doc.library_folder().is_some();
    let snapshots = library && cx.doc.current().is_some() && !commands::stacks::list(cx).is_empty();
    let collections = if library { collections::list(cx) } else { Vec::new() };
    let selected = cx.doc.query().collection;
    let previous = cx.state_ref::<commands::sync::SyncState>().and_then(|s| s.previous);
    Facts {
        current_stacked: cx.doc.current().is_some_and(|p| p.stack.is_some()),
        snapshots,
        plain_collection: collections.iter().any(|c| !c.smart),
        selected_plain_collection: collections.iter().any(|c| Some(c.id) == selected && !c.smart),
        copied: cx.doc.has_copied(),
        previous_edits: previous.is_some_and(|p| ids.iter().any(|i| *i != p)),
        export_running: cx.jobs.running("export"),
        previous_export: commands::export::previous_available(cx),
        collapsed: cx.doc.query().collapsed,
        ids,
        ratings,
        flags,
        labels,
        monochrome,
        any_oriented,
        any_stacked,
    }
}

/// `values and all(v == target)`.
fn all_equal<T: PartialEq>(values: &[T], target: &T) -> bool {
    !values.is_empty() && values.iter().all(|v| v == target)
}

struct Builder<'a> {
    cx: &'a AppContext,
    ids: bool,
}

impl Builder<'_> {
    /// `add(parent, text, key, callback, enabled)`: enabled only with photos selected (and, in the app, while the
    /// command can run).
    fn item<'l>(
        &self,
        list: &'l mut Vec<Node>,
        title: &str,
        key: &str,
        command: &'static str,
        enabled: bool,
    ) -> &'l mut Item {
        let can = !command.is_empty() && commands::enabled(self.cx, command);
        list.push(Node::Item(Item {
            key: key.to_owned(),
            title: title.to_owned(),
            command,
            value: None,
            enabled: self.ids && enabled && can,
            checked: None,
            tip: None,
        }));
        match list.last_mut() {
            Some(Node::Item(item)) => item,
            _ => unreachable!("an item was just pushed"),
        }
    }

    /// `choice(...)`: a checkable item, checked when every selected photo has the target value.
    fn choice(
        &self,
        list: &mut Vec<Node>,
        title: &str,
        key: &str,
        command: &'static str,
        value: Option<Value>,
        checked: bool,
    ) {
        let item = self.item(list, title, key, command, true);
        item.value = value;
        item.checked = Some(checked);
    }
}

/// The context menu of the selected photos, in the oracle's order (`photo_actions.build_menu`).
pub fn menu(cx: &mut AppContext) -> Vec<Node> {
    let f = facts(cx);
    let b = Builder { cx, ids: !f.ids.is_empty() };
    let many = f.ids.len() > 1;
    let mut m = Vec::new();

    b.item(&mut m, "현상에서 크게 보기", "develop", "photo.develop", true);
    b.item(&mut m, "내보내기…", "export", "file.export", !f.export_running);
    let mut quick = Vec::new();
    for (title, kind, command) in [
        ("JPG (작은 크기 · 2048px)", "small", "export.quick_small"),
        ("JPG (원본 크기)", "large", "export.quick_large"),
        ("원본 + 설정", "original", "export.quick_original"),
    ] {
        let item = b.item(&mut quick, title, &format!("quick-export-{kind}"), command, !f.export_running);
        if kind == "original" {
            item.tip = Some("원본 파일과 Grainy 보정값 XMP를 함께 복사합니다.");
        }
    }
    quick.push(Node::Separator);
    b.item(
        &mut quick,
        "이전 설정으로 내보내기",
        "export-previous",
        "file.quick_export",
        !f.export_running && f.previous_export,
    );
    b.item(&mut quick, "사용자 지정 설정…", "export-custom", "file.export", !f.export_running);
    m.push(Node::Sub(Sub {
        key: "quick-export",
        title: "빠른 내보내기",
        enabled: b.ids && !f.export_running,
        children: quick,
    }));
    b.item(&mut m, "내보내기 기록…", "export-history", "export.history", true);
    m.push(Node::Separator);
    b.item(&mut m, "시계 방향으로 90° 회전", "rotate-right", "photo.rotate_right", true);
    b.item(&mut m, "반시계 방향으로 90° 회전", "rotate-left", "photo.rotate_left", true);
    b.item(&mut m, "좌우 반전", "flip", "photo.flip", true);
    b.item(&mut m, "상하 반전", "flip-vertical", "photo.flip_vertical", true);
    b.item(&mut m, "카메라 방향으로 복원", "camera-orientation", "photo.camera_orientation", f.any_oriented);
    m.push(Node::Separator);
    b.item(&mut m, "보정 복사", "copy", "develop.copy_edits", true);
    b.item(&mut m, "보정 붙여넣기", "paste", "develop.paste_edits", f.copied);

    let mut edits = Vec::new();
    b.item(&mut edits, "복사할 보정 항목 선택…", "copy-options", "sync.choose_items", true);
    b.item(&mut edits, "현재 보정을 선택 사진에 동기화", "sync", "sync.selected", many);
    b.item(&mut edits, "이전 사진 보정 붙이기", "previous-edits", "develop.paste_previous", f.previous_edits);
    b.item(&mut edits, "자동 화이트 밸런스", "auto-white-balance", "workflow.auto_white", true);
    b.item(&mut edits, "현재 사진에 총노출 맞추기", "match-exposure", "workflow.match_exposure", many);
    for (title, value, key) in [("흑백으로 전환", true, "monochrome"), ("컬러로 전환", false, "color")] {
        b.choice(&mut edits, title, key, "photo.monochrome", Some(json!(value)), all_equal(&f.monochrome, &value));
    }
    b.item(&mut edits, "현재 보정 스냅샷 저장…", "snapshot", "snapshots.save", true);
    b.item(&mut edits, "스냅샷 불러오기…", "restore-snapshot", "snapshots.restore", f.snapshots);
    edits.push(Node::Separator);
    b.item(&mut edits, "선택 사진 보정 초기화", "reset", "photo.reset", true);
    m.push(Node::Sub(Sub { key: "", title: "보정 설정", enabled: true, children: edits }));

    b.item(&mut m, "가상 사본 만들기", "virtual-copy", "cull.virtual_copy", true);

    let mut stacks = Vec::new();
    b.item(&mut stacks, "선택 사진을 스택으로 묶기", "stack", "stacks.stack", many);
    b.item(&mut stacks, "선택 사진 스택 해제", "unstack", "stacks.unstack", f.any_stacked);
    let toggle = if f.collapsed { "스택 펼쳐 보기" } else { "스택 접어서 보기" };
    b.item(&mut stacks, toggle, "toggle-stacks", "stacks.toggle", true);
    b.item(&mut stacks, "현재 사진을 스택 대표로", "stack-cover", "stacks.cover", f.current_stacked);
    m.push(Node::Sub(Sub { key: "", title: "스택", enabled: true, children: stacks }));

    let mut rating = Vec::new();
    for value in 0..6i64 {
        let title = if value == 0 { "별점 해제".to_owned() } else { "★".repeat(value as usize) };
        b.choice(
            &mut rating,
            &title,
            &format!("rating-{value}"),
            "cull.rating",
            Some(json!(value)),
            all_equal(&f.ratings, &value),
        );
    }
    m.push(Node::Sub(Sub { key: "", title: "별점", enabled: true, children: rating }));

    let mut flag = Vec::new();
    for (title, value, command) in
        [("선택", 1i64, "cull.pick"), ("제외", -1, "cull.reject"), ("표시 해제", 0, "cull.unflag")]
    {
        b.choice(&mut flag, title, &format!("flag-{value}"), command, None, all_equal(&f.flags, &value));
    }
    m.push(Node::Sub(Sub { key: "", title: "선택 표시", enabled: true, children: flag }));

    let mut label = Vec::new();
    for value in ["", "빨강", "노랑", "초록", "파랑", "보라"] {
        let title = if value.is_empty() { "라벨 해제" } else { value };
        b.choice(
            &mut label,
            title,
            &format!("label-{value}"),
            "cull.color_label",
            Some(json!(value)),
            all_equal(&f.labels, &value.to_owned()),
        );
    }
    m.push(Node::Sub(Sub { key: "", title: "색 라벨", enabled: true, children: label }));

    let mut collection = Vec::new();
    b.item(&mut collection, "빠른 컬렉션에 추가·제외", "quick-collection", "cull.quick_collection", true);
    b.item(&mut collection, "컬렉션에 추가…", "collection-add", "collections.add", f.plain_collection);
    b.item(&mut collection, "선택 사진으로 새 컬렉션…", "collection-new", "collections.new", true);
    b.item(
        &mut collection,
        "현재 컬렉션에서 제외",
        "collection-remove",
        "collections.remove_photos",
        f.selected_plain_collection,
    );
    m.push(Node::Sub(Sub { key: "", title: "컬렉션", enabled: true, children: collection }));

    let mut compare = Vec::new();
    b.item(&mut compare, "선택 사진 비교", "compare", "view.compare_photos", many);
    b.item(&mut compare, "선택 사진 모아보기", "survey", "view.survey", many);
    b.item(&mut compare, "현재 사진 보정 전·후", "before-after", "view.compare_before", true);
    m.push(Node::Sub(Sub { key: "", title: "비교 보기", enabled: true, children: compare }));

    let mut metadata = Vec::new();
    b.item(&mut metadata, "메타데이터·키워드 편집…", "metadata", "metadata.edit", true);
    b.item(&mut metadata, "촬영 시각 일괄 이동…", "capture-time", "metadata.shift_date", true);
    b.item(&mut metadata, "XMP 저장", "write-xmp", "metadata.write_xmp", true);
    b.item(&mut metadata, "XMP 불러오기", "read-xmp", "metadata.read_xmp", true);
    metadata.push(Node::Separator);
    b.item(&mut metadata, "메타데이터 프리셋 적용…", "metadata-preset", "workflow.metadata_apply", true);
    b.item(&mut metadata, "바로가기 키워드 적용", "shortcut-keyword", "workflow.keyword_apply", true);
    m.push(Node::Sub(Sub { key: "", title: "메타데이터", enabled: true, children: metadata }));

    let mut files = Vec::new();
    for (title, key, command) in [
        ("파일 이름 변경…", "rename", "file.rename"),
        ("다른 폴더로 이동…", "move", "file.move"),
        ("누락 원본 다시 연결…", "relink", "file.relink"),
        ("오프라인 편집용 미리보기 만들기", "smart-preview", "file.smart_previews"),
        ("미리보기 다시 만들기", "rebuild-preview", "file.rebuild_previews"),
    ] {
        b.item(&mut files, title, key, command, true);
    }
    m.push(Node::Sub(Sub { key: "", title: "파일 관리", enabled: true, children: files }));

    m.push(Node::Separator);
    let folder = if cfg!(target_os = "macos") { "Finder에서 폴더 열기" } else { "탐색기에서 폴더 열기" };
    b.item(&mut m, folder, "show-folder", "photo.show_folder", true);

    let mut remove = Vec::new();
    b.item(&mut remove, "카탈로그에서만 제거…", "remove", "file.remove", true);
    b.item(&mut remove, "원본을 휴지통으로…", "trash", "file.trash", true);
    m.push(Node::Sub(Sub { key: "", title: "사진 제거", enabled: true, children: remove }));
    m
}

/// What the open menu keeps between frames: the menu is built when it opens (it reads the catalogue) and again
/// when the selection or an edit changes.
#[derive(Default)]
struct Cache {
    key: Vec<(u64, u64)>,
    frame: u64,
    nodes: Vec<Node>,
}

/// The menu for this frame: the cached one while the same menu stays open on the same selection.
fn current(cx: &mut AppContext) -> Vec<Node> {
    let frame = cx.egui.cumulative_pass_nr();
    let key: Vec<(u64, u64)> = cx
        .doc
        .selected_indices()
        .iter()
        .map(|i| (cx.doc.photos().get(*i).map_or(0, |p| p.id), cx.doc.version_of(*i)))
        .collect();
    let stale = {
        let cache = cx.state::<Cache>();
        cache.nodes.is_empty() || cache.key != key || frame > cache.frame + 2
    };
    if stale {
        let nodes = menu(cx);
        let cache = cx.state::<Cache>();
        cache.key = key;
        cache.nodes = nodes;
    }
    let cache = cx.state::<Cache>();
    cache.frame = frame;
    cache.nodes.clone()
}

fn draw(ui: &mut Ui, cx: &mut AppContext, nodes: &[Node]) {
    for node in nodes {
        match node {
            Node::Separator => {
                ui.separator();
            }
            Node::Sub(sub) => {
                ui.add_enabled_ui(sub.enabled, |ui| {
                    ui.menu_button(tr(sub.title), |ui| draw(ui, cx, &sub.children));
                });
            }
            Node::Item(item) => {
                let mut button = egui::Button::new(tr(&item.title));
                if let Some(checked) = item.checked {
                    button = button.selected(checked);
                }
                let mut response = ui.add_enabled(item.enabled, button);
                if let Some(tip) = item.tip {
                    response = response.on_hover_text(tr(tip));
                }
                if response.clicked() {
                    let invocation = Invocation { key: None, value: item.value.clone() };
                    cx.run_with(item.command, &invocation);
                    ui.close();
                }
            }
        }
    }
}

impl AppContext {
    /// The photo's context menu (`MainWindow.photo_context_menu` and `build_menu`). A photo that is not in the
    /// selection is selected alone first; one that is becomes the open photo with the selection kept.
    pub(crate) fn photo_context_menu(&mut self, ui: &mut Ui, index: usize) {
        let Some(id) = self.doc.photos().get(index).map(|p| p.id) else { return };
        if !self.doc.is_selected(id) {
            self.select_photo(index);
        } else if id != self.doc.current_id() {
            let others: Vec<usize> = self.doc.selected_indices().into_iter().filter(|i| *i != index).collect();
            self.select_photo(index);
            for other in others {
                self.doc.toggle_selected(other);
            }
        }
        let nodes = current(self);
        draw(ui, self, &nodes);
    }
}
