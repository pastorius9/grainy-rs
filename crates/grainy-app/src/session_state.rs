//! The library column's state and the saved session (work package 1): the folder tree with its missing and
//! unscanned marks, the preferences `folder_panel`, `browser_view` and `develop_panels` (port of
//! `save_folder_state`, `restore_folder_selection`, `change_sort`, `DevelopPanels.save` / `finish` of
//! luma/app.py and luma/develop_panels.py), the metadata refresh after a photo is loaded, neighbour prefetch,
//! the growing list during an import, and the library lock of luma/main.py.
//!
//! [`poll`] runs once per frame (one hook line in `AppContext::poll_background`). Saving compares the values
//! with the last ones written, so a preference is stored when it changes and never otherwise.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use grainy_catalog::folders::Platform;
use grainy_library::folder_locations::{scan_folders, FolderScan, Identity};
use grainy_library::Cancel;
use serde_json::{json, Map, Value};

use crate::context::{AppContext, Mode};
use crate::document::{Kind, Query};
use crate::panels::{PANELS, TOOLS};
use crate::render::PrefetchJob;
use crate::text::tr;

/// Preference keys of the oracle.
pub const FOLDER_PANEL: &str = "folder_panel";
pub const BROWSER_VIEW: &str = "browser_view";
pub const DEVELOP_PANELS: &str = "develop_panels";
const FOLDER_IDENTITIES: &str = "folder_identities";

/// The sort orders `sort_order` offers (`findData` falls back to the first).
const SORTS: [&str; 4] = ["imported", "name", "capture", "rating"];

/// How long the list waits between two reads while an import adds photos (`import_refresh_timer.start(150)`).
const IMPORT_REFRESH: Duration = Duration::from_millis(150);

/// `{'selected', 'include_subfolders', 'expanded', 'current_photo'}` of `save_folder_state`.
pub fn folder_panel_value(query: &Query, expanded: &BTreeSet<String>, current: u64) -> Value {
    json!({
        "selected": query.folder,
        "include_subfolders": query.recursive,
        "expanded": expanded.iter().collect::<Vec<_>>(),
        "current_photo": if current == 0 { Value::Null } else { json!(current) },
    })
}

/// `{'sort', 'descending'}` of `change_sort`.
pub fn browser_view_value(query: &Query) -> Value {
    json!({"sort": query.sort, "descending": query.descending})
}

/// `{'solo', 'expanded'}` of `DevelopPanels.save`: the open panels in the column's order, tools left out.
pub fn develop_panels_value(solo: bool, expanded: &[&'static str]) -> Value {
    let open: Vec<&str> =
        PANELS.iter().map(|(k, _)| *k).filter(|k| !TOOLS.contains(k) && expanded.contains(k)).collect();
    json!({"solo": solo, "expanded": open})
}

/// One folder of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeNode {
    pub key: String,
    pub path: String,
    pub name: String,
    pub drive: bool,
    pub direct: i64,
    pub total: i64,
    pub missing: bool,
    pub unscanned: bool,
    pub children: Vec<TreeNode>,
}

/// The folder rows (path, count) and roots a scan was made for (`FolderPanel.signature`).
type ListingSignature = (Vec<(String, i64)>, Vec<String>);

/// What the last folder scan found (`FolderPanel.children`, `missing`, `scanned`).
#[derive(Debug, Default)]
struct Scan {
    children: Vec<String>,
    missing: HashSet<String>,
    scanned: HashSet<String>,
    revision: u64,
    running: bool,
    wanted: bool,
    /// The signature of the listing the running or last scan was made for.
    signature: Option<ListingSignature>,
    cancel: Cancel,
}

/// The library column's state, kept in the context (`cx.state::<ColumnState>()`).
#[derive(Default)]
pub struct ColumnState {
    /// The document the state was restored for.
    doc_key: Option<String>,
    /// Folder keys that are expanded (`expanded_folders`).
    pub expanded: BTreeSet<String>,
    /// `'expanded' in folder_preferences`: new folders with photos expand by themselves when it is missing.
    expanded_preference: bool,
    /// The values last written (or read) of the three preferences.
    saved: [Option<Value>; 3],
    tree_key: Option<(u64, u64)>,
    tree_signature: Vec<(String, Option<String>, i64, i64, bool, bool)>,
    previous: HashSet<String>,
    /// The tree (`refresh_folder_tree`).
    pub nodes: Vec<TreeNode>,
    scan: Scan,
    activated: u64,
    metadata_sent: bool,
    prefetch_sent: bool,
    import_status: Option<String>,
    import_read: Option<Instant>,
    /// The cancel button was pressed (`import_cancel_button.setEnabled(False)`).
    pub cancelling: bool,
    /// The renamed folders the last scan found (`result['moves']`), for the file-management package (`folders`).
    pub moves: Vec<(String, String)>,
}

impl ColumnState {
    /// Asks for the folders to be scanned again (`FolderPanel.refresh`).
    pub fn rescan(&mut self) {
        self.scan.wanted = true;
    }

    /// Keys of the folders the last scan found missing (`FolderPanel.missing`).
    pub fn missing_folders(&self) -> &HashSet<String> {
        &self.scan.missing
    }

    /// Counts the scans that finished (a new value: new `children`, `missing`, `scanned`).
    pub fn scan_revision(&self) -> u64 {
        self.scan.revision
    }

    /// Port of `removable_folder`: the chosen folder, unless it is a drive.
    pub fn removable(query: &Query) -> Option<String> {
        query.folder.clone().filter(|f| Path::new(f).parent().is_some())
    }

    /// The node of a folder path.
    pub fn node(&self, path: &str) -> Option<&TreeNode> {
        fn find<'a>(nodes: &'a [TreeNode], key: &str) -> Option<&'a TreeNode> {
            nodes.iter().find_map(|n| if n.key == key { Some(n) } else { find(&n.children, key) })
        }
        find(&self.nodes, &Platform::native().path_key(path))
    }
}

fn object(value: Option<Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn doc_key(cx: &AppContext) -> Option<String> {
    match cx.doc.kind() {
        Kind::Session => None,
        Kind::Catalog { folder, read_only } => Some(format!("{}|{read_only}", folder.display())),
    }
}

/// Once per frame: restore a newly opened library's session, save what changed, rebuild the folder tree, run
/// the folder scan, refresh the metadata, prefetch neighbours and keep the list growing during an import.
pub fn poll(cx: &mut AppContext) {
    let key = doc_key(cx);
    if cx.state::<ColumnState>().doc_key != key {
        restore(cx, key);
    }
    if cx.state::<ColumnState>().doc_key.is_none() {
        return;
    }
    rebuild_tree(cx);
    start_scan(cx);
    import_growth(cx);
    if cx.doc.listing_pending() {
        return;
    }
    save(cx);
    after_load(cx);
}

/// Port of `restore_folder_selection` and of the saved parts read at start (`folder_preferences`,
/// `browser_preferences`, `DevelopPanels.finish`).
fn restore(cx: &mut AppContext, key: Option<String>) {
    let folder = object(cx.doc.preference(FOLDER_PANEL));
    let browser = object(cx.doc.preference(BROWSER_VIEW));
    let panels = cx.doc.preference(DEVELOP_PANELS);
    {
        let st = cx.state::<ColumnState>();
        st.scan.cancel.set();
        *st = ColumnState { doc_key: key.clone(), ..ColumnState::default() };
    }
    if key.is_none() {
        return;
    }
    let mut query = cx.doc.query();
    query.folder = folder.get("selected").and_then(Value::as_str).map(str::to_owned);
    query.recursive = folder.get("include_subfolders").and_then(Value::as_bool).unwrap_or(true);
    let sort = browser.get("sort").and_then(Value::as_str).unwrap_or("imported");
    query.sort = if SORTS.contains(&sort) { sort.to_owned() } else { SORTS[0].to_owned() };
    query.descending = browser.get("descending").and_then(Value::as_bool).unwrap_or(false);
    let expanded: BTreeSet<String> = folder
        .get("expanded")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let current = folder.get("current_photo").and_then(Value::as_u64);
    let saved_folder =
        if folder.is_empty() { folder_panel_value(&query, &expanded, 0) } else { Value::Object(folder.clone()) };
    let saved_browser = if browser.is_empty() { browser_view_value(&query) } else { Value::Object(browser.clone()) };

    // `DevelopPanels.finish`: solo, and the open panels (the first one only in solo mode).
    let saved_panels = match &panels {
        Some(Value::Object(map)) => {
            let solo = map.get("solo").and_then(Value::as_bool).unwrap_or(true);
            let asked: Vec<String> = match map.get("expanded").and_then(Value::as_array) {
                Some(a) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                None => vec!["basic".to_owned()],
            };
            let mut open: Vec<&'static str> = asked
                .iter()
                .filter_map(|k| PANELS.iter().map(|(p, _)| *p).find(|p| p == k))
                .filter(|k| !TOOLS.contains(k))
                .collect();
            if solo {
                open.truncate(1);
            }
            cx.solo = solo;
            cx.expanded = open;
            panels.clone()
        }
        _ => None,
    };
    let saved_panels = saved_panels.unwrap_or_else(|| develop_panels_value(cx.solo, &cx.expanded));

    if let Err(message) = cx.doc.set_query(query) {
        cx.status = crate::text::tr_with("처리하지 못했습니다: {0}", &[&message]);
    }
    let has_folder = !folder.is_empty();
    if has_folder {
        let safe = crate::startup::safe_mode(cx);
        cx.doc.restore_photo(if safe { None } else { current });
        cx.doc.request_refresh(!safe);
        if cx.doc.query().folder.is_some() {
            cx.set_mode(Mode::Library);
        }
    }
    let st = cx.state::<ColumnState>();
    st.expanded = expanded;
    st.expanded_preference = folder.contains_key("expanded");
    st.saved = [Some(saved_folder), Some(saved_browser), Some(saved_panels)];
}

/// Stores `folder_panel`, `browser_view` and `develop_panels` when they differ from what was written last.
fn save(cx: &mut AppContext) {
    // The import writes through its own connection: the window waits until it is over (the values stay due).
    if cx.jobs.running("import") {
        return;
    }
    let query = cx.doc.query();
    let current = cx.doc.current_id();
    let expanded = cx.state::<ColumnState>().expanded.clone();
    let wanted = [
        (FOLDER_PANEL, folder_panel_value(&query, &expanded, current)),
        (BROWSER_VIEW, browser_view_value(&query)),
        (DEVELOP_PANELS, develop_panels_value(cx.solo, &cx.expanded)),
    ];
    for (index, (name, value)) in wanted.into_iter().enumerate() {
        if cx.state::<ColumnState>().saved[index].as_ref() == Some(&value) {
            continue;
        }
        match cx.doc.save_preference(name, &value) {
            Ok(()) => cx.state::<ColumnState>().saved[index] = Some(value),
            Err(message) => log::warn!("cannot save {name}: {message}"),
        }
    }
}

/// Port of `FolderPanel.nodes`: the flat folder list (depth, key order) of the query's folders with the scanned
/// subfolders under registered roots that were not left out, each with its parent's key and the `missing` and
/// `unscanned` marks. `children`, `missing` and `scanned` are the last scan's.
pub fn flat_nodes(
    rows: &[Value],
    roots: &[String],
    removed: &[String],
    children: &[String],
    missing: &HashSet<String>,
    scanned: &HashSet<String>,
) -> Vec<(Option<String>, TreeNode)> {
    let platform = Platform::native();
    let extra = children
        .iter()
        .filter(|c| roots.iter().any(|r| platform.contains_folder(r, c)) && !platform.left_out(c, roots, removed))
        .cloned();
    let registered: Vec<String> = roots.iter().cloned().chain(extra).collect();
    let flat = platform.folder_nodes(rows, &registered).unwrap_or_default();
    flat.iter()
        .map(|n| {
            let text = |k: &str| n[k].as_str().unwrap_or("").to_owned();
            let key = text("key");
            let drive = n["drive"].as_bool().unwrap_or(false);
            (
                n["parent"].as_str().map(str::to_owned),
                TreeNode {
                    missing: missing.contains(&key),
                    unscanned: !drive && !scanned.contains(&key),
                    key,
                    path: text("path"),
                    name: text("name"),
                    drive,
                    direct: n["direct"].as_i64().unwrap_or(0),
                    total: n["total"].as_i64().unwrap_or(0),
                    children: Vec::new(),
                },
            )
        })
        .collect()
}

/// Port of `refresh_folder_tree`'s tree: `FolderPanel.nodes` (scanned subfolders under registered roots that
/// were not left out), the marks, and the automatic expansion of new folders with photos.
fn rebuild_tree(cx: &mut AppContext) {
    let (generation, rows, roots, removed) = {
        let (generation, inputs) = cx.doc.folder_inputs();
        (generation, inputs.rows.clone(), inputs.roots.clone(), inputs.removed.clone())
    };
    let tree_key = (generation, cx.state::<ColumnState>().scan.revision);
    if cx.state::<ColumnState>().tree_key == Some(tree_key) {
        return;
    }
    let signature: ListingSignature = (
        rows.iter()
            .map(|r| (r["path"].as_str().unwrap_or("").to_owned(), r.get("count").and_then(Value::as_i64).unwrap_or(1)))
            .collect(),
        roots.clone(),
    );
    let st = cx.state::<ColumnState>();
    st.tree_key = Some(tree_key);
    if st.scan.signature.as_ref() != Some(&signature) {
        st.scan.signature = Some(signature);
        st.scan.wanted = true;
    }
    let nodes = flat_nodes(&rows, &roots, &removed, &st.scan.children, &st.scan.missing, &st.scan.scanned);
    let signature: Vec<_> =
        nodes.iter().map(|(p, n)| (n.key.clone(), p.clone(), n.direct, n.total, n.missing, n.unscanned)).collect();
    if signature != st.tree_signature {
        for (_, n) in &nodes {
            if n.total > 0 && !st.previous.contains(&n.key) && !st.expanded_preference {
                st.expanded.insert(n.key.clone());
            }
        }
        st.previous = nodes.iter().map(|(_, n)| n.key.clone()).collect();
        st.tree_signature = signature;
    }
    // Children attach to their parent in the flat order (depth, key): the oracle's insertion order.
    let mut roots_out: Vec<TreeNode> = Vec::new();
    let order: Vec<String> = nodes.iter().map(|(_, n)| n.key.clone()).collect();
    let parents: HashMap<String, Option<String>> = nodes.iter().map(|(p, n)| (n.key.clone(), p.clone())).collect();
    let mut by_key: HashMap<String, TreeNode> = nodes.into_iter().map(|(_, n)| (n.key.clone(), n)).collect();
    for key in order.iter().rev() {
        let node = by_key.remove(key).expect("listed once");
        match parents[key].as_ref().and_then(|p| by_key.get_mut(p)) {
            Some(parent) => parent.children.insert(0, node),
            None => roots_out.insert(0, node),
        }
    }
    st.nodes = roots_out;
}

fn busy(cx: &AppContext) -> bool {
    cx.jobs.running("import") || cx.jobs.running("export")
}

/// Port of `FolderPanel.refresh`: a scan of the catalogue's folders, their parents and the expanded folders'
/// children, on a job. Renamed folders (`result['moves']`) are the file-management package's.
fn start_scan(cx: &mut AppContext) {
    let wanted = {
        let st = cx.state::<ColumnState>();
        st.scan.wanted && !st.scan.running
    };
    if !wanted || busy(cx) || cx.doc.listing_pending() || cx.modal_open() {
        return;
    }
    let platform = Platform::native();
    let (mut paths, roots): (BTreeSet<String>, Vec<String>) = {
        let (_, inputs) = cx.doc.folder_inputs();
        (inputs.rows.iter().map(|r| platform.parent(r["path"].as_str().unwrap_or(""))).collect(), inputs.roots.clone())
    };
    let st = cx.state::<ColumnState>();
    paths.extend(st.scan.children.iter().cloned());
    let expanded: HashSet<String> = st.expanded.iter().cloned().collect();
    let signature = st.scan.signature.clone();
    st.scan.running = true;
    st.scan.wanted = false;
    st.scan.cancel = Cancel::new();
    let cancel = st.scan.cancel.clone();
    let saved = object(cx.doc.preference(FOLDER_IDENTITIES));
    let identities: HashMap<String, Identity> = saved
        .iter()
        .filter_map(|(k, v)| {
            let id = v.get("id")?.as_array()?;
            Some((k.clone(), [id.first()?.as_u64()?, id.get(1)?.as_u64()?, id.get(2)?.as_u64()?]))
        })
        .collect();
    let paths: Vec<String> = paths.into_iter().collect();
    let token = cancel.clone();
    cx.jobs.spawn(
        "folders",
        "",
        move |_| Ok(scan_folders(platform, &paths, &roots, &expanded, &identities, &cancel)),
        move |cx: &mut AppContext, result: Result<FolderScan, String>| {
            cx.state::<ColumnState>().scan.running = false;
            match result {
                Ok(scan) => {
                    if token.is_set() {
                        return;
                    }
                    if busy(cx) || cx.state::<ColumnState>().scan.signature != signature {
                        cx.state::<ColumnState>().scan.wanted = true;
                        return;
                    }
                    let mut updated = saved.clone();
                    for (key, seen) in &scan.identities {
                        updated.insert(key.clone(), json!({"path": seen.path, "id": seen.id}));
                    }
                    if updated != saved {
                        if let Err(message) = cx.doc.save_preference(FOLDER_IDENTITIES, &Value::Object(updated)) {
                            log::warn!("cannot save the folder identities: {message}");
                        }
                    }
                    let st = cx.state::<ColumnState>();
                    st.moves = scan.moves;
                    st.scan.children = scan.children;
                    st.scan.missing = scan.missing.into_iter().collect();
                    st.scan.scanned = scan.scanned.into_iter().collect();
                    st.scan.revision += 1;
                }
                Err(_) => cx.status = tr("폴더 목록을 갱신하지 못했습니다. 드라이브 연결을 확인하세요."),
            }
        },
    );
}

/// Port of `import_refresh_timer`: while an import adds photos the list is read again every 150 ms.
fn import_growth(cx: &mut AppContext) {
    if !cx.jobs.running("import") {
        let st = cx.state::<ColumnState>();
        st.import_status = None;
        st.import_read = None;
        return;
    }
    let status = cx.jobs.status();
    let waiting = cx.doc.listing_pending();
    let ensure = cx.doc.current().is_none();
    let st = cx.state::<ColumnState>();
    let due = st.import_read.is_none_or(|t| t.elapsed() >= IMPORT_REFRESH);
    if due && !waiting && st.import_status != status {
        st.import_status = status;
        st.import_read = Some(Instant::now());
        cx.doc.request_refresh(ensure);
    }
    cx.egui.request_repaint_after(IMPORT_REFRESH);
}

/// What `activate` does after the photo is loaded: the metadata read replaces the stored one, and the photos
/// next to it are read ahead (`prefetch_neighbors`).
fn after_load(cx: &mut AppContext) {
    let id = cx.doc.current_id();
    let loaded = cx.frame.as_ref().is_some_and(|f| f.photo == id);
    let st = cx.state::<ColumnState>();
    if st.activated != id {
        st.activated = id;
        st.metadata_sent = false;
        st.prefetch_sent = false;
    }
    if id == 0 || !loaded {
        return;
    }
    // The metadata is stored once the import is over (its own connection writes meanwhile).
    let importing = cx.jobs.running("import");
    let st = cx.state::<ColumnState>();
    if !importing && !std::mem::replace(&mut st.metadata_sent, true) && !cx.doc.read_only() {
        if let Some(index) = cx.doc.current_index() {
            let path = cx.doc.photos()[index].path.clone();
            let settings = cx.doc.settings_of(index);
            cx.spawn_document_job(
                "metadata",
                "",
                move |_| {
                    grainy_engine::files::load_photo(&path, Some(64), &settings)
                        .map(|loaded| loaded.metadata)
                        .map_err(|e| e.to_string())
                },
                move |cx: &mut AppContext, result: Result<Map<String, Value>, String>| {
                    // `if version!=self.load_version: return`: the photo is still the open one.
                    if cx.doc.current_id() != id {
                        return;
                    }
                    match result {
                        Ok(info) => {
                            if let Err(message) = cx.doc.store_metadata(id, &Value::Object(info)) {
                                log::warn!("cannot store the metadata: {message}");
                            }
                        }
                        Err(message) => log::info!("metadata of photo {id} not read: {message}"),
                    }
                },
            );
        }
    }
    if cx.state::<ColumnState>().prefetch_sent {
        return;
    }
    if let Some(position) = cx.doc.current_index() {
        let jobs: Vec<PrefetchJob> = [position.checked_add(1), position.checked_sub(1)]
            .into_iter()
            .flatten()
            .filter(|i| *i < cx.doc.len() && !crate::video::is_video(&cx.doc.photos()[*i].path))
            .map(|i| PrefetchJob { path: cx.doc.photos()[i].path.clone(), settings: cx.doc.settings_of(i) })
            .collect();
        // While an import adds photos the neighbours may not be listed yet: ask again later.
        if !jobs.is_empty() || !importing {
            cx.state::<ColumnState>().prefetch_sent = true;
        }
        if !jobs.is_empty() {
            cx.renderer.prefetch(jobs);
        }
    }
}

/// Port of `import_reason`: one line for the "unreadable photos" list: the error's last line, or for a RAW file
/// the decoder does not know, what the person can do about it.
pub fn import_reason(path: &Path, error: &str) -> String {
    let last = error.trim().lines().last().unwrap_or("").to_owned();
    if !last.contains("LibRawFileUnsupportedError") {
        return last;
    }
    let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    if extension == ".nef" || extension == ".nrw" {
        return tr("이 압축 방식은 지원하지 않습니다. 니콘의 고효율(HE) RAW라면 카메라에서 \"무손실 압축\"으로 찍거나 DNG로 변환해 주세요.");
    }
    tr("지원하지 않는 RAW 형식입니다.")
}

// ---- the library lock (main.py `QLockFile`) ------------------------------------------------------------------

/// What the second window says (main.py).
pub const ALREADY_OPEN: &str = "This photo library is already open.\n이 사진 라이브러리가 이미 열려 있습니다.";

/// The lock of an open library: `luma.lock` in the library folder, held with the system's file lock for as long
/// as this value lives (a crashed run leaves no stale lock).
#[derive(Debug)]
pub struct LibraryLock {
    _file: File,
}

impl LibraryLock {
    /// Locks the library in `folder` (created when missing). Refused with [`ALREADY_OPEN`] when another window
    /// holds it.
    pub fn acquire(folder: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(folder.join("luma.lock"))
            .map_err(|e| e.to_string())?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(ALREADY_OPEN.to_owned()),
            Err(TryLockError::Error(e)) => Err(e.to_string()),
        }
    }
}

// main reserves the startup library before creating a window. Transfer that reservation
// to its document, so switching away releases it just like a library opened from the menu.
static STARTUP_LOCK: Mutex<Option<(std::path::PathBuf, LibraryLock)>> = Mutex::new(None);

pub(crate) fn document_lock(folder: &Path) -> Result<LibraryLock, String> {
    let mut pending = STARTUP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if pending.as_ref().is_some_and(|(path, _)| path == folder) {
        return Ok(pending.take().unwrap().1);
    }
    LibraryLock::acquire(folder)
}

/// Locks the library the arguments name for the life of the process. Nothing is locked for `--memory` and for
/// the Python Grainy's folders (they only open read-only). `Err` carries the message to show before exiting.
pub fn lock_library(args: &crate::paths::Arguments) -> Result<(), String> {
    if args.memory {
        return Ok(());
    }
    let locations = crate::paths::Locations::current();
    let folder = args.library.clone().unwrap_or_else(|| locations.library_folder());
    if crate::paths::is_oracle_folder(&folder, &locations) {
        return Ok(());
    }
    let lock = LibraryLock::acquire(&folder)?;
    *STARTUP_LOCK.lock().unwrap_or_else(|e| e.into_inner()) = Some((folder, lock));
    Ok(())
}

/// Shows `message` in a box titled Grainy (`QMessageBox.information(None, 'Grainy', ...)`). Called by `main` on the
/// main thread before the window exists, the one place where rfd's blocking box is right on every system (macOS
/// shows it from the main thread; there is no window to keep drawing).
pub fn show_refusal(message: &str) {
    rfd::MessageDialog::new().set_title("Grainy").set_description(message).set_level(rfd::MessageLevel::Info).show();
}
