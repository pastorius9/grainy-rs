//! Work package 16, stacks, snapshots and virtual copies, against the oracle (`golden/app/stacks_steps`, built by
//! `tools/golden/gen_app_stacks.py` with the Python Grainy's own `LibraryManager.stack`, `unstack`,
//! `toggle_stacks`, `snapshot`, `restore_snapshot`, `virtual_copy` and `WorkflowTools.stack_cover`,
//! `stack_move` on the stand-in window of its generators):
//!
//! * a scripted session after which the stack orders, the list (stacks open and collapsed), the open photo's
//!   settings and the catalogue tables are the oracle's;
//! * the commands: what they enable, the boxes they open, the refusals;
//! * the window: the grid with a stack, collapsed, and the snapshot boxes (snapshots in Korean and English).
//!
//! Every test works on copies and generated photos in temporary folders.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::collections::selected_ids;
use grainy_app::commands::stacks as st;
use grainy_app::dialogs::snapshots as boxes;
use grainy_app::document::catalog::stacks::{StackOp, StackReply};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::Query;
use grainy_app::render::{PreviewJob, Renderer, Reply, TestCard, ThumbnailJob};
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn now() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-11-13 01:02:03", "%Y-%m-%d %H:%M:%S").unwrap()
}

/// A copy of the oracle's library in a fresh folder, opened with the oracle's clock and token.
fn golden_library(name: &str) -> (PathBuf, CatalogDocument) {
    let meta = Case::open("app", "library").json().clone();
    let dir = common::temp_dir(name);
    std::fs::copy(grainy_testkit::root().join("golden/app/library/start.sqlite"), dir.join("catalog.sqlite"))
        .expect("copy the library");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(&dir, Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.set_clock(Arc::new(now));
    doc.refresh_now().unwrap();
    (dir, doc)
}

fn context(doc: CatalogDocument) -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc))
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn listed(cx: &AppContext) -> Vec<u64> {
    cx.doc.photos().iter().map(|p| p.id).collect()
}

fn ids_of(value: &Value) -> Vec<u64> {
    value.as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect()
}

/// Numbers compare as numbers (`20` and `20.0` are the same setting), everything else as it is.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// FNV-1a (64 bit) of the text, as the generator's `digest`.
fn digest(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        h = (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The tables of `golden/app/stacks_steps`, as the generator reads them.
fn tables(dir: &Path) -> Value {
    let db = Connection::open(dir.join("catalog.sqlite")).unwrap();
    let mut out = Map::new();
    for (table, columns) in [
        ("photos", vec!["id", "name", "stack_id", "virtual_source", "rating", "flag", "label", "imported", "edits"]),
        ("snapshots", vec!["id", "photo_id", "name", "edits", "created"]),
        ("history", vec!["id", "photo_id", "label"]),
    ] {
        let list = columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
        let mut statement = db.prepare(&format!("SELECT {list} FROM {table} ORDER BY id")).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |r| {
                let mut row = Map::new();
                for (i, name) in columns.iter().enumerate() {
                    let value = match r.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(v) => json!(v),
                        ValueRef::Real(v) => json!(v),
                        ValueRef::Text(t) => {
                            let text = String::from_utf8_lossy(t).into_owned();
                            if *name == "edits" {
                                json!(digest(&text))
                            } else {
                                json!(text)
                            }
                        }
                        ValueRef::Blob(_) => json!("blob"),
                    };
                    row.insert((*name).to_owned(), value);
                }
                Ok(Value::Object(row))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        out.insert(table.to_owned(), Value::Array(rows));
    }
    let orders: Option<String> =
        db.query_row("SELECT value FROM preferences WHERE key='stack_orders'", [], |r| r.get(0)).ok();
    out.insert("stack_orders".into(), orders.map_or(Value::Null, Value::String));
    Value::Object(out)
}

fn order_of(cx: &mut AppContext, photo: u64) -> Vec<u64> {
    match cx.doc.stack_op(StackOp::Order { photo }) {
        Ok(StackReply::Order(order)) => order,
        other => panic!("{other:?}"),
    }
}

// ---- the session --------------------------------------------------------------------------------------------

struct Session {
    cx: AppContext,
    selection: Vec<u64>,
}

impl Session {
    fn current(&self) -> Option<u64> {
        self.cx.doc.current().map(|p| p.id)
    }

    fn open(&mut self, id: u64) {
        let index = self.cx.doc.index_of(id).unwrap_or_else(|| panic!("photo {id} is not listed"));
        self.cx.doc.select(index);
        self.selection = vec![id];
    }

    fn ids(&self) -> Vec<u64> {
        if self.selection.is_empty() {
            self.current().into_iter().collect()
        } else {
            self.selection.clone()
        }
    }
}

fn run_step(s: &mut Session, step: &Value, golden: &Value) {
    let action = step[0].as_str().unwrap();
    let args = &step.as_array().unwrap()[1..];
    let at = format!("{step}");
    match action {
        "open" => s.open(args[0].as_u64().unwrap()),
        "select" => {
            s.selection = ids_of(&args[0]);
            // The window's own selection (virtual copies read it): the open photo and the others.
            let current = s.current();
            for id in s.selection.clone() {
                if Some(id) != current {
                    if let Some(index) = s.cx.doc.index_of(id) {
                        s.cx.doc.toggle_selected(index);
                    }
                }
            }
        }
        "set" => s.cx.doc.edit_mut().set(args[0].as_str().unwrap(), args[1].clone()),
        "commit" => {
            s.cx.doc.edit_mut().commit("보정 변경");
        }
        "stack" => {
            let ids = s.ids();
            st::stack(&mut s.cx, ids);
        }
        "unstack" => {
            let ids = s.ids();
            st::unstack(&mut s.cx, ids);
        }
        "toggle" => st::toggle(&mut s.cx),
        "cover" => st::cover(&mut s.cx),
        "move" => st::shift(&mut s.cx, args[0].as_i64().unwrap()),
        "snapshot" => match args[0].as_str() {
            Some(name) => {
                // The box shows the oracle's title and label; its answer is the name.
                assert_eq!(golden["boxes"][0]["title"], "스냅샷", "{at}");
                assert_eq!(golden["boxes"][0]["label"], "이름", "{at}");
                st::save(&mut s.cx, name);
            }
            None => {
                assert!(s.cx.run("snapshots.save"));
                assert!(s.cx.dialog_open(boxes::NAME_BOX), "{at}");
                s.cx.close_dialog(boxes::NAME_BOX);
            }
        },
        "restore" => {
            let rows = st::list(&mut s.cx);
            match golden["error"].as_str() {
                Some(error) => {
                    assert!(rows.is_empty(), "{at}");
                    assert_eq!(error, "저장된 스냅샷이 없습니다.");
                    assert!(s.cx.run("snapshots.restore"));
                    assert!(s.cx.dialog_open("collections_error"), "{at}");
                    s.cx.close_dialog("collections_error");
                }
                None => {
                    let items: Vec<&str> =
                        golden["boxes"][0]["items"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()).collect();
                    assert_eq!(boxes::labels(&rows), items, "{at}: the choices");
                    assert_eq!(golden["boxes"][0]["title"], "스냅샷");
                    assert_eq!(golden["boxes"][0]["label"], "복원할 보정");
                    if let Some(index) = args[0].as_u64() {
                        st::restore(&mut s.cx, &rows[index as usize]);
                    }
                }
            }
        }
        "virtual_copy" => {
            s.cx.doc.virtual_copies().unwrap();
        }
        other => panic!("unknown step {other}"),
    }
}

#[test]
fn the_scripted_stack_session_matches_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let golden = Case::open("app", "stacks_steps").json().clone();
    let script = parse(golden["script"].as_str().unwrap());
    let (dir, doc) = golden_library("stacks-session");
    let mut s = Session { cx: context(doc), selection: Vec::new() };
    s.cx.doc.refresh().unwrap();
    let mut last_tables = tables(&dir);
    let steps = golden["steps"].as_array().unwrap();
    assert_eq!(steps.len(), script.as_array().unwrap().len());
    let mut refreshing = 0;
    for (step, expected) in script.as_array().unwrap().iter().zip(steps) {
        let at = format!("{step}");
        run_step(&mut s, step, expected);
        s.cx.doc.flush().unwrap();
        let asked = s.cx.doc.listing_pending();
        s.cx.doc.refresh().unwrap();
        let oracle_asked = !expected["refreshes"].as_array().unwrap().is_empty();
        if oracle_asked {
            assert!(asked, "{at}: the oracle reads the list again");
            refreshing += 1;
        }
        let action = step[0].as_str().unwrap();
        if !oracle_asked && !matches!(action, "open" | "select") {
            assert!(!asked, "{at}: a refresh the oracle did not ask for");
        }
        assert_eq!(s.cx.doc.query().collapsed, expected["collapsed"].as_bool().unwrap(), "{at}: collapsed");
        assert_eq!(listed(&s.cx), ids_of(&expected["ids"]), "{at}: the list");
        assert_eq!(s.cx.doc.total() as u64, expected["total"].as_u64().unwrap(), "{at}: total");
        // The oracle's stand-in window does not run `ensure_current`: when a list hides the open photo, the real
        // window opens the first listed one. The photo-specific checks then wait for the next `open`.
        let same_photo = s.current() == expected["current"].as_u64();
        if !same_photo && action != "virtual_copy" {
            let hidden = !ids_of(&expected["ids"]).contains(&expected["current"].as_u64().unwrap());
            assert!(hidden, "{at}: the open photo differs although it is listed");
            assert_eq!(s.current(), listed(&s.cx).first().copied(), "{at}: the first listed photo opens");
        }
        if same_photo {
            if let Some(current) = s.current() {
                assert_eq!(order_of(&mut s.cx, current), ids_of(&expected["current_order"]), "{at}: the stack order");
            }
            let changed = Value::Object(s.cx.doc.edit().changed());
            assert!(same(&changed, &expected["changed"]), "{at}: settings {changed} against {}", expected["changed"]);
            let rows = st::list(&mut s.cx);
            let ours: Vec<Value> = rows.iter().map(|r| json!({"id": r.id, "name": r.name})).collect();
            assert_eq!(Value::Array(ours), expected["snapshots"], "{at}: the snapshot list");
        }
        if let Some(t) = expected.get("tables") {
            last_tables = t.clone();
        }
        assert_eq!(tables(&dir), last_tables, "{at}: tables");
    }
    assert!(refreshing >= 20, "the script changes the list often ({refreshing})");
}

// ---- the commands -------------------------------------------------------------------------------------------

/// The stack commands are off without a stacked open photo, snapshots need a library; menus and shortcuts see it.
#[test]
fn the_commands_are_enabled_as_the_context_menu_enables_them() {
    let (_dir, doc) = golden_library("stacks-enabled");
    let mut cx = context(doc);
    cx.doc.refresh().unwrap();
    let on = |cx: &AppContext, id: &str| grainy_app::commands::enabled(cx, id);
    for id in ["stacks.stack", "stacks.unstack", "stacks.toggle", "snapshots.save", "snapshots.restore"] {
        assert!(on(&cx, id), "{id}");
    }
    assert!(!on(&cx, "stacks.cover"), "no photo is open");
    cx.doc.select(cx.doc.index_of(2).unwrap());
    assert!(!on(&cx, "stacks.cover"), "photo 2 is not stacked");
    assert!(!on(&cx, "stacks.forward") && !on(&cx, "stacks.backward"));
    cx.doc.select(cx.doc.index_of(5).unwrap());
    for id in ["stacks.cover", "stacks.forward", "stacks.backward"] {
        assert!(on(&cx, id), "{id}");
    }
    assert_eq!(st::checked(&cx, "stacks.toggle"), Some(false));
    assert_eq!(st::checked(&cx, "stacks.cover"), None);
    assert!(cx.run("stacks.toggle"));
    cx.doc.refresh().unwrap();
    assert_eq!(st::checked(&cx, "stacks.toggle"), Some(true));
    assert_eq!(st::toggle_title(&cx), "스택 펼쳐 보기");
    assert_eq!(listed(&cx), [1, 2, 3, 6, 7, 8], "one photo of the stack of 4, 5 and 6: its cover");
    assert!(cx.run("stacks.toggle"));
    cx.doc.refresh().unwrap();
    assert_eq!(st::toggle_title(&cx), "스택 접어서 보기");
}

/// The stack commands run on the selected photos, as the oracle's `selected_ids()`: the selection, or the open
/// photo when none is selected.
#[test]
fn stack_and_unstack_work_on_the_selection() {
    let (_dir, doc) = golden_library("stacks-selection");
    let mut cx = context(doc);
    cx.doc.refresh().unwrap();
    cx.doc.select(cx.doc.index_of(1).unwrap());
    cx.doc.toggle_selected(cx.doc.index_of(3).unwrap());
    assert_eq!(selected_ids(&cx), [1, 3]);
    assert!(cx.run("stacks.stack"));
    cx.doc.refresh().unwrap();
    assert_eq!(cx.doc.photos().iter().filter(|p| p.stack == Some(1)).count(), 2, "stack id = the smallest id");
    assert!(cx.doc.photos().iter().any(|p| p.id == 1 && p.stack == Some(1)));
    assert!(cx.run("stacks.unstack"));
    cx.doc.refresh().unwrap();
    assert!(cx.doc.photos().iter().all(|p| p.stack != Some(1)));
}

/// A library opened read-only keeps its stacks and snapshots: the commands are off and nothing is written.
#[test]
fn a_read_only_library_is_not_changed() {
    let (dir, doc) = golden_library("stacks-readonly");
    drop(doc);
    let scratch = common::temp_dir("stacks-readonly-scratch");
    let doc = CatalogDocument::open_read_only(&dir, &scratch).unwrap();
    let mut cx = context(doc);
    cx.doc.refresh().unwrap();
    for id in ["stacks.stack", "stacks.unstack", "snapshots.save", "snapshots.restore", "stacks.toggle"] {
        assert!(!grainy_app::commands::enabled(&cx, id), "{id} is off");
    }
    let before = tables(&dir);
    st::stack(&mut cx, vec![1, 2]);
    assert!(cx.dialog_open("collections_error"), "the refusal is shown");
    st::save(&mut cx, "x");
    assert_eq!(tables(&dir), before, "nothing was written");
}

/// A session (no library) has neither stacks nor snapshots.
#[test]
fn a_session_has_no_stacks() {
    let mut cx = AppContext::new(
        &egui::Context::default(),
        Box::new(TestCard::default()),
        Box::new(grainy_app::session::Session::default()),
    );
    for id in ["stacks.stack", "stacks.toggle", "snapshots.save", "snapshots.restore", "stacks.cover"] {
        assert!(!grainy_app::commands::enabled(&cx, id), "{id}");
    }
    assert!(cx.doc.stack_op(StackOp::Snapshots).is_err());
}

/// The restore commits the open photo's pending change as its own step, then replaces the settings with the
/// snapshot's as one step labelled 스냅샷 복원; undo goes back to the settings before.
#[test]
fn restoring_a_snapshot_is_one_undo_step() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (_dir, doc) = golden_library("stacks-restore");
    let mut cx = context(doc);
    cx.doc.refresh().unwrap();
    cx.doc.select(cx.doc.index_of(2).unwrap());
    cx.doc.edit_mut().set("exposure", json!(0.25));
    st::save(&mut cx, "  quarter  ");
    cx.doc.flush().unwrap();
    cx.doc.edit_mut().set("exposure", json!(-0.5));
    let rows = st::list(&mut cx);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "quarter", "the name is stripped");
    st::restore(&mut cx, &rows[0]);
    cx.doc.flush().unwrap();
    assert_eq!(cx.doc.edit().settings().f32("exposure"), 0.25);
    assert_eq!(cx.doc.edit().history()[0], "스냅샷 복원");
    assert_eq!(cx.doc.edit().history()[1], "보정 변경", "the pending change was committed first");
    assert!(cx.doc.edit_mut().undo());
    assert_eq!(cx.doc.edit().settings().f32("exposure"), -0.5);
}

/// The query the stacks use is part of the document's options: `collapsed` reaches `library_query`.
#[test]
fn collapsed_is_a_list_option() {
    let q = Query { collapsed: true, ..Query::default() };
    assert_eq!(q.options(json!({}))["collapsed"], true);
}

// ---- the window ---------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library with six generated photos (a folder with a fixed name); the first is open.
fn library_harness<'a>(name: &str) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = common::temp_dir(name);
    let folder = dir.join("stacks-photos");
    std::fs::create_dir_all(&folder).unwrap();
    let gains = [[1.0; 3], [1.3, 1.0, 0.8], [0.7, 0.9, 1.2], [1.0, 1.0, 0.6], [0.6; 3], [1.2, 0.8, 1.0]];
    let photos: Vec<PathBuf> = gains
        .iter()
        .enumerate()
        .map(|(i, gain)| {
            let path = folder.join(format!("garden-{}.png", i + 1));
            common::save_png(&common::garden(2, *gain, i % 2 == 1), &path);
            path
        })
        .collect();
    let library = dir.join("Library");
    let data = dir.join("data");
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app.open_paths(photos);
        app
    });
    common::snapshot::settle(&mut harness);
    common::snapshot::open_folders(harness.state_mut(), &["stacks-photos"]);
    common::snapshot::settle(&mut harness);
    (harness, dir)
}

fn listed_ids(harness: &Harness<'_, GrainyApp>) -> Vec<u64> {
    harness.state().cx.doc.photos().iter().map(|p| p.id).collect()
}

/// The name-dialog fixture shows the newly selected filename with the previous photo's histogram.
/// Hold the real worker's replies until that frame is captured, independently of its speed.
struct HoldReplies {
    worker: Box<dyn Renderer>,
    hold: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Renderer for HoldReplies {
    fn preview(&mut self, job: PreviewJob) {
        self.worker.preview(job);
    }
    fn thumbnail(&mut self, job: ThumbnailJob) {
        self.worker.thumbnail(job);
    }
    fn poll(&mut self) -> Vec<Reply> {
        if self.hold.get() {
            Vec::new()
        } else {
            self.worker.poll()
        }
    }
    fn idle(&self) -> bool {
        self.worker.idle()
    }
}

/// The grid with a stack open and collapsed, the menu commands and the snapshot boxes end to end.
fn window_flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let (mut harness, _dir) = library_harness(&format!("stacks-window-{tag}"));
    common::snapshot::pin_status(harness.state_mut(), "");
    assert_eq!(listed_ids(&harness).len(), 6);
    // Photos 2, 3 and 4 become a stack: select them, then the library menu's command.
    {
        let cx = &mut harness.state_mut().cx;
        cx.doc.select(1);
        cx.doc.toggle_selected(2);
        cx.doc.toggle_selected(3);
        assert!(cx.run("stacks.stack"));
    }
    harness.state_mut().cx.set_mode(grainy_app::context::Mode::Library);
    common::snapshot::settle(&mut harness);
    common::snapshot::pin_status(harness.state_mut(), "");
    let stacked: Vec<Option<i64>> = harness.state().cx.doc.photos().iter().map(|p| p.stack).collect();
    assert_eq!(stacked, [None, Some(2), Some(2), Some(2), None, None]);
    common::snapshot::save(&mut harness, &format!("stacks-grid-open-{tag}"));

    // The third photo becomes the cover; the stack folds to it.
    harness.state_mut().cx.doc.select(2);
    assert!(harness.state_mut().run("stacks.cover"));
    common::snapshot::settle(&mut harness);
    common::snapshot::pin_status(harness.state_mut(), "");
    assert_eq!(listed_ids(&harness), [1, 3, 2, 4, 5, 6]);
    assert!(harness.state_mut().run("stacks.toggle"));
    common::snapshot::settle(&mut harness);
    common::snapshot::pin_status(harness.state_mut(), "");
    assert_eq!(listed_ids(&harness), [1, 3, 5, 6], "a collapsed stack shows its cover");
    common::snapshot::save(&mut harness, &format!("stacks-grid-collapsed-{tag}"));
    assert!(harness.state_mut().run("stacks.toggle"));
    common::snapshot::settle(&mut harness);

    // The snapshot boxes.
    let hold = std::rc::Rc::new(std::cell::Cell::new(true));
    let renderer = &mut harness.state_mut().cx.renderer;
    let mut worker: Box<dyn Renderer> = Box::<TestCard>::default();
    std::mem::swap(renderer, &mut worker);
    *renderer = Box::new(HoldReplies { worker, hold: hold.clone() });
    harness.state_mut().cx.doc.select(0);
    assert!(harness.state_mut().run("snapshots.save"));
    harness.run_steps(3);
    common::snapshot::pin_status(harness.state_mut(), "");
    assert!(harness.state().dialog_open(boxes::NAME_BOX));
    common::snapshot::save(&mut harness, &format!("stacks-snapshot-name-{tag}"));
    hold.set(false);
    harness.state_mut().close_dialog(boxes::NAME_BOX);
    {
        let cx = &mut harness.state_mut().cx;
        cx.doc.edit_mut().set("exposure", json!(0.4));
        st::save(cx, "Warm");
        cx.doc.edit_mut().set("exposure", json!(-0.3));
        st::save(cx, "Dark");
    }
    common::snapshot::settle(&mut harness);
    assert!(harness.state_mut().run("snapshots.restore"));
    harness.run_steps(3);
    common::snapshot::pin_status(harness.state_mut(), "");
    assert!(harness.state().dialog_open(boxes::CHOICE_BOX));
    common::snapshot::save(&mut harness, &format!("stacks-snapshot-choice-{tag}"));
    harness.state_mut().close_dialog(boxes::CHOICE_BOX);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_window_shows_stacks_and_snapshots_korean() {
    window_flow(Language::Korean, "ko");
}

#[test]
fn the_window_shows_stacks_and_snapshots_english() {
    window_flow(Language::English, "en");
}
