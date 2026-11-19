//! Collections and filters: the commands behind the library column's 컬렉션 fold-out, the 보기·필터 and 컬렉션
//! menus and the quick collection (luma/manager.py `create_collection`, `add_to_collection`, `remove_collection`,
//! `edit_collection`, `remove_from_collection`, `filter_dialog`, `clear_filter`, `select_collection`,
//! `refresh_timed_collection`; luma/workflow_ui.py `quick_toggle`, `select_collection`, `quick_show`,
//! `containing_collection`).
//!
//! A command opens the oracle's boxes (name, rules, choice) and the box's answer calls the matching `…_named`
//! function; those are public so that tests, and later features, do what the boxes do without a window. The
//! oracle's manager state (`collection`, `rules`) is the document's [`Query`](crate::document::Query).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{always, writable_library, Command, Invocation};
use crate::context::{AppContext, Mode};
use crate::dialogs::filter::{ChoiceBox, FilterDialog, TextBox};
use crate::dialogs::import_errors::Notice;
use crate::document::catalog::collections::{CollectionOp, CollectionReply};
use crate::document::Collection;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "collections.filter", label: "상세 필터", enabled: always, run: filter },
    Command {
        id: "collections.clear_filter", label: "필터·컬렉션 해제", enabled: always, run: clear_filter_command
    },
    Command {
        id: "collections.edit", label: "선택 컬렉션 이름·규칙 변경", enabled: writable_library, run: edit
    },
    Command {
        id: "collections.remove_photos",
        label: "선택 사진을 컬렉션에서 제외",
        enabled: writable_library,
        run: remove_photos,
    },
    Command {
        id: "cull.quick_collection",
        label: "빠른 컬렉션에 추가·제외",
        enabled: writable_library,
        run: quick_toggle_command,
    },
    Command {
        id: "collections.quick_show", label: "빠른 컬렉션 보기", enabled: writable_library, run: quick_show
    },
    Command {
        id: "collections.containing",
        label: "소속 컬렉션으로 이동…",
        enabled: writable_library,
        run: containing,
    },
    Command { id: "collections.new", label: "+", enabled: writable_library, run: new_plain },
    Command { id: "collections.new_smart", label: "스마트", enabled: writable_library, run: new_smart },
    Command { id: "collections.add", label: "추가", enabled: writable_library, run: add },
    Command { id: "collections.delete", label: "삭제", enabled: writable_library, run: delete },
];

/// What the collections fold-out and the 60 second timer keep between frames.
#[derive(Default)]
pub struct CollectionsState {
    /// Collections whose branch the user folded; `refresh_collections` (`expandAll`) opens every branch again.
    pub collapsed: HashSet<i64>,
    /// When the timed refresh last ran.
    pub timer: Option<Instant>,
}

/// The period of `smart_timer`.
pub const TIMER: Duration = Duration::from_secs(60);

/// Port of the box `run()` shows when a manager action raises: `QMessageBox.warning(w, tr('작업을 완료하지 못했습니다'),
/// tr(str(e)))`.
pub fn fail(cx: &mut AppContext, message: &str) {
    let text = tr(message);
    cx.open_dialog(Box::new(Notice {
        id: "collections_error", title: tr("작업을 완료하지 못했습니다"), text
    }));
}

/// Port of `refresh_collections`: every branch of the tree is open again.
fn refresh_collections(cx: &mut AppContext) {
    cx.state::<CollectionsState>().collapsed.clear();
}

/// The ids the oracle's `selected_ids()` gives: the selected photos (the open one when none is selected).
pub fn selected_ids(cx: &AppContext) -> Vec<u64> {
    let photos = cx.doc.photos();
    cx.doc.selected_indices().iter().filter_map(|i| photos.get(*i)).map(|p| p.id).collect()
}

/// `f'{n:,}'`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn op(cx: &mut AppContext, op: CollectionOp) -> Option<CollectionReply> {
    match cx.doc.collection_op(op) {
        Ok(reply) => Some(reply),
        Err(message) => {
            fail(cx, &message);
            None
        }
    }
}

/// The collections as the catalogue has them now, by name.
pub fn list(cx: &mut AppContext) -> Vec<Collection> {
    match op(cx, CollectionOp::List) {
        Some(CollectionReply::Collections(list)) => list,
        _ => Vec::new(),
    }
}

// ---- the filter -------------------------------------------------------------------------------------------

fn filter(cx: &mut AppContext, _: &Invocation) {
    let rules = cx.doc.query().rules;
    match FilterDialog::new("상세 필터", &rules, apply_filter) {
        Ok(dialog) => cx.open_dialog(Box::new(dialog)),
        Err(message) => fail(cx, &message),
    }
}

/// Port of `filter_dialog` after the box: the rules become the list's rules (`refresh_lists(ensure_current=True)`).
pub fn apply_filter(cx: &mut AppContext, rules: Value) {
    cx.change_query(|q| q.rules = rules);
}

fn clear_filter_command(cx: &mut AppContext, _: &Invocation) {
    clear_filter(cx);
}

/// Port of `clear_filter`: no rules, no collection, and `set_filter('all')` (the 모든 사진 list, no folder).
pub fn clear_filter(cx: &mut AppContext) {
    refresh_collections(cx);
    cx.change_query(|q| {
        q.rules = json!({});
        q.collection = None;
        q.mode = "all".into();
        q.folder = None;
    });
    cx.set_mode(Mode::Library);
}

// ---- the collection list ----------------------------------------------------------------------------------

/// Port of `select_collection` (a click on an item of the tree): the collection lists, the folder is left and
/// the library shows. Choosing the one already chosen does nothing (`currentItemChanged`).
pub fn select(cx: &mut AppContext, collection: Option<i64>) {
    cx.change_query(|q| {
        q.collection = collection;
        q.folder = None;
    });
    cx.set_mode(Mode::Library);
}

/// Port of workflow_ui `select_collection`: the collection alone, no rules, no search, the 모든 사진 list.
pub fn show_collection(cx: &mut AppContext, collection: i64) {
    refresh_collections(cx);
    cx.change_query(|q| {
        q.collection = Some(collection);
        q.rules = json!({});
        q.search.clear();
        q.mode = "all".into();
        q.folder = None;
    });
    cx.set_mode(Mode::Library);
}

fn new_plain(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    let title = tr("컬렉션");
    let label = tr("이름");
    cx.open_dialog(Box::new(TextBox::new("collections_name", title, label, "", move |cx, name| {
        create_named(cx, &name, None, ids);
    })));
}

fn new_smart(cx: &mut AppContext, _: &Invocation) {
    let title = tr("스마트 컬렉션");
    let label = tr("이름");
    cx.open_dialog(Box::new(TextBox::new("collections_name", title, label, "", move |cx, name| {
        if name.trim().is_empty() {
            return;
        }
        match FilterDialog::new("스마트 컬렉션 규칙", &json!({}), move |cx, rules| {
            create_named(cx, &name, Some(rules), Vec::new());
        }) {
            Ok(dialog) => cx.open_dialog(Box::new(dialog)),
            Err(message) => fail(cx, &message),
        }
    })));
}

/// Port of `create_collection` after its boxes: `name.strip()` (nothing for an empty name), the rules of a smart
/// collection (`None` for an ordinary one: it takes `members`), under the selected collection, which it then
/// becomes.
pub fn create_named(cx: &mut AppContext, name: &str, rules: Option<Value>, members: Vec<u64>) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    op(cx, CollectionOp::Create { name: name.to_owned(), rules, members });
    refresh_collections(cx);
}

/// The rows of a choice box: `{id} · {name}`.
pub fn choice_items(collections: &[Collection]) -> Vec<String> {
    collections.iter().map(|c| format!("{} · {}", c.id, c.name)).collect()
}

/// The ordinary collections (`rules is None`), read now: what 추가 offers.
pub fn plain(cx: &mut AppContext) -> Vec<Collection> {
    list(cx).into_iter().filter(|c| !c.smart).collect()
}

/// The collections the photo belongs to, read now (smart ones evaluated): what 소속 컬렉션으로 이동… offers.
pub fn containing_of(cx: &mut AppContext, photo: u64) -> Vec<Collection> {
    match op(cx, CollectionOp::Containing { photo }) {
        Some(CollectionReply::Collections(found)) => found,
        _ => Vec::new(),
    }
}

fn add(cx: &mut AppContext, _: &Invocation) {
    let plain = plain(cx);
    if plain.is_empty() {
        fail(cx, "일반 컬렉션을 먼저 만들어 주세요.");
        return;
    }
    let ids = selected_ids(cx);
    let items = choice_items(&plain);
    cx.open_dialog(Box::new(ChoiceBox::new(
        "collections_add",
        tr("컬렉션에 추가"),
        tr("선택한 사진을 추가할 컬렉션"),
        items,
        move |cx, index| {
            if let Some(collection) = plain.get(index) {
                add_to(cx, collection.id, ids);
            }
        },
    )));
}

/// Port of `add_to_collection` after the choice.
pub fn add_to(cx: &mut AppContext, collection: i64, ids: Vec<u64>) {
    op(cx, CollectionOp::AddMembers { collection, ids });
}

fn delete(cx: &mut AppContext, _: &Invocation) {
    remove_selected(cx);
}

/// Port of `remove_collection`: the selected collection goes; nothing happens when none is selected.
pub fn remove_selected(cx: &mut AppContext) {
    op(cx, CollectionOp::RemoveSelected);
    refresh_collections(cx);
}

fn edit(cx: &mut AppContext, _: &Invocation) {
    let Some(id) = cx.doc.query().collection else { return };
    let Some(CollectionReply::Info { name, rules }) = op(cx, CollectionOp::Info { collection: id }) else { return };
    cx.open_dialog(Box::new(TextBox::new(
        "collections_edit",
        tr("컬렉션 편집"),
        tr("이름"),
        &name,
        move |cx, name| {
            if name.trim().is_empty() {
                return;
            }
            match rules {
                None => edit_named(cx, &name, None),
                Some(rules) => {
                    match FilterDialog::new("스마트 컬렉션 규칙", &rules, move |cx, rules| {
                        edit_named(cx, &name, Some(rules));
                    }) {
                        Ok(dialog) => cx.open_dialog(Box::new(dialog)),
                        Err(message) => fail(cx, &message),
                    }
                }
            }
        },
    )));
}

/// Port of `edit_collection` after its boxes: the selected collection's name (stripped) and, for a smart one, its
/// rules.
pub fn edit_named(cx: &mut AppContext, name: &str, rules: Option<Value>) {
    op(cx, CollectionOp::Edit { name: name.trim().to_owned(), rules });
    refresh_collections(cx);
}

fn remove_photos(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    remove_from(cx, ids);
}

/// Port of `remove_from_collection`: the photos leave the selected collection (nothing when none is selected).
pub fn remove_from(cx: &mut AppContext, ids: Vec<u64>) {
    op(cx, CollectionOp::RemoveMembers { ids });
}

// ---- the quick collection ---------------------------------------------------------------------------------

fn quick_toggle_command(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    quick_toggle(cx, &ids);
}

/// Port of `quick_toggle` (the B key): the selected photos join the quick collection, or leave it when they all
/// are in it; the status line says which.
pub fn quick_toggle(cx: &mut AppContext, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    let label = tr("빠른 컬렉션");
    let Some(CollectionReply::Toggled { added, .. }) = op(cx, CollectionOp::ToggleQuick { ids: ids.to_vec(), label })
    else {
        return;
    };
    refresh_collections(cx);
    let count = thousands(ids.len());
    let text = if added { "빠른 컬렉션에 {0}장 추가" } else { "빠른 컬렉션에서 {0}장 제외" };
    cx.set_status(tr_with(text, &[&count]));
}

fn quick_show(cx: &mut AppContext, _: &Invocation) {
    let label = tr("빠른 컬렉션");
    if let Some(CollectionReply::Id(id)) = op(cx, CollectionOp::Quick { label }) {
        show_collection(cx, id);
    }
}

fn containing(cx: &mut AppContext, _: &Invocation) {
    let photo = cx.doc.current_id();
    let found = containing_of(cx, photo);
    if found.is_empty() {
        fail(cx, "이 사진이 속한 컬렉션이 없습니다.");
        return;
    }
    let items = choice_items(&found);
    cx.open_dialog(Box::new(ChoiceBox::new(
        "collections_containing",
        tr("소속 컬렉션"),
        tr("이동할 컬렉션"),
        items,
        move |cx, index| {
            if let Some(collection) = found.get(index) {
                show_collection(cx, collection.id);
            }
        },
    )));
}

// ---- the timer ----------------------------------------------------------------------------------------------

/// Port of `refresh_timed_collection`: when the chosen collection or the filter holds a relative date rule ("in the
/// last 7 days"), its list is read again. Skipped while the list loads or an import runs.
pub fn refresh_timed_collection(cx: &mut AppContext) {
    if cx.doc.listing_pending() || cx.jobs.running("import") {
        return;
    }
    let query = cx.doc.query();
    let mut rules = vec![query.rules.clone()];
    if let Some(collection) = query.collection {
        if let Ok(CollectionReply::Info { rules: Some(stored), .. }) =
            cx.doc.collection_op(CollectionOp::Info { collection })
        {
            rules.push(stored);
        }
    }
    if rules.iter().any(|r| grainy_adobe::smart::time_dependent(r).unwrap_or(false)) {
        cx.doc.request_refresh(false);
    }
}

/// Once per frame: `smart_timer`, every 60 seconds.
pub fn timed_refresh(cx: &mut AppContext) {
    let now = Instant::now();
    let due = match cx.state::<CollectionsState>().timer {
        Some(last) => now.duration_since(last) >= TIMER,
        None => {
            cx.state::<CollectionsState>().timer = Some(now);
            false
        }
    };
    if due {
        cx.state::<CollectionsState>().timer = Some(now);
        refresh_timed_collection(cx);
    }
}
