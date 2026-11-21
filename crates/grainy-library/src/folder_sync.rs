//! Port of `luma/folder_sync.py`: automatic import of new photos that appear in registered folders.
//!
//! The oracle checks shortly after start, whenever Grainy becomes the active application and every two minutes.
//! It only adds catalogue entries and never touches the files. Photos and folders the user removed from the
//! catalogue are not imported again, files still being written are left for the next check, and unreadable files
//! are skipped quietly until they change.
//!
//! Here the Qt timers and signals become methods the application calls ([`FolderSync::poll`],
//! [`FolderSync::activated`]) and the clock is injected. The listing itself is a [`ScanJob`] the application runs
//! on a worker thread; its result goes back through [`FolderSync::found`].

use crate::library::is_image_extension;
use crate::pathx::{resolve, splitext_ext};
use crate::{Cancel, Result};
use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use grainy_core::settings::truthy;
use grainy_i18n::tr_with;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

/// Catalogue preference: the feature is on.
pub const PREFERENCE: &str = "auto_sync_folders";
/// Catalogue preference: keys of photos taken out of the catalogue on purpose.
pub const EXCLUDED: &str = "sync_excluded";
/// Interval of the periodic check.
pub const INTERVAL_MS: u64 = 120_000;
/// Delay of the first check after start.
pub const START_MS: u64 = 4_000;
/// Seconds between checks triggered by returning to the app.
pub const ACTIVATE_GAP: f64 = 20.0;
/// Seconds a file must be unchanged (still copying or downloading otherwise).
pub const SETTLE: f64 = 5.0;

static OUTPUTS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Port of `folder_sync.note_output`: a file Grainy wrote itself (an export) is not a new photo.
pub fn note_output(platform: Platform, path: &str) {
    let key = platform.path_key(&resolve(platform, path));
    if let Ok(mut set) = OUTPUTS.lock() {
        set.insert(key);
    }
}

/// Empties the `OUTPUTS` set (the oracle's module global), for tests and for a fresh session.
pub fn clear_outputs() {
    if let Ok(mut set) = OUTPUTS.lock() {
        set.clear();
    }
}

fn outputs() -> BTreeSet<String> {
    OUTPUTS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn mtime_seconds(meta: &std::fs::Metadata) -> Option<f64> {
    let d = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9)
}

/// Seconds since the epoch (`time.time()`).
pub fn now_epoch() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Port of `folder_sync.new_files`: image files under `roots` whose path key is not in `known`, oldest first.
/// `skipped` maps a key to the `(mtime, size)` of a file that failed to import; it is offered again only after it
/// changes. `removed` lists folders taken out of the catalogue: they are not entered (a root inside one of them
/// was registered after the removal and is read like any other root). A set `cancel` returns an empty list.
#[allow(clippy::too_many_arguments)]
pub fn new_files(
    platform: Platform,
    roots: &[String],
    known: &HashSet<String>,
    skipped: &HashMap<String, (f64, u64)>,
    cancel: Option<&Cancel>,
    settle: f64,
    now: f64,
    removed: &[String],
) -> Vec<String> {
    let removed: HashSet<String> = removed.iter().map(|p| platform.path_key(p)).collect();
    let mut found: Vec<(String, (f64, String))> = Vec::new();
    for root in roots {
        if !std::path::Path::new(root).is_dir() {
            continue;
        }
        let mut stack = vec![root.clone()];
        while let Some(folder) = stack.pop() {
            if cancel.is_some_and(Cancel::is_set) {
                return Vec::new();
            }
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            let mut names = Vec::new();
            let mut below = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(kind) = entry.file_type() else { continue };
                let is_dir =
                    kind.is_dir() || (kind.is_symlink() && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()));
                if is_dir {
                    if !kind.is_symlink() {
                        below.push(name);
                    }
                } else {
                    names.push(name);
                }
            }
            let join = |name: &str| crate::pathx::text(&std::path::Path::new(&folder).join(name));
            if !removed.is_empty() {
                below.retain(|name| !removed.contains(&platform.path_key(&join(name))));
            }
            for name in &names {
                if !is_image_extension(&splitext_ext(name).to_lowercase()) {
                    continue;
                }
                let path = join(name);
                if known.contains(&platform.path_key(&path)) {
                    continue;
                }
                let key = platform.path_key(&resolve(platform, &path));
                if known.contains(&key) || found.iter().any(|(k, _)| *k == key) {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                let Some(mtime) = mtime_seconds(&meta) else { continue };
                if now - mtime < settle || skipped.get(&key) == Some(&(mtime, meta.len())) {
                    continue;
                }
                found.push((key, (mtime, path)));
            }
            for name in below.into_iter().rev() {
                stack.push(join(&name));
            }
        }
    }
    let mut values: Vec<(f64, String)> = found.into_iter().map(|(_, v)| v).collect();
    values.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
    values.into_iter().map(|(_, path)| path).collect()
}

/// The parts of the main window `FolderSync` looks at (`w.closing`, `w.import_busy` ...), and the import queue
/// it appends to.
#[derive(Clone, Debug, Default)]
pub struct SyncHost {
    /// The window is closing.
    pub closing: bool,
    /// A manual or automatic import is running.
    pub import_busy: bool,
    /// Number of import scans in progress (`w.import_scans`).
    pub import_scans: usize,
    /// An export is running.
    pub export_running: bool,
    /// Catalogue maintenance is running.
    pub maintenance_running: bool,
    /// A command batch is running.
    pub command_running: bool,
    /// The import was cancelled by the person (`w.import_cancel.is_set()`): wait for the next manual import.
    pub import_cancelled: bool,
    /// Files waiting to be imported (`w.import_queue`).
    pub queue: Vec<String>,
    /// Their keys (`w.import_pending_paths`).
    pub pending_paths: BTreeSet<String>,
    /// Errors of the running import (`w.import_errors`).
    pub errors: Vec<String>,
}

impl SyncHost {
    /// Port of `FolderSync.idle`.
    pub fn idle(&self) -> bool {
        !(self.closing
            || self.import_busy
            || self.import_scans > 0
            || self.export_running
            || self.maintenance_running
            || self.command_running
            || self.import_cancelled)
    }
}

/// One folder check for a worker thread (what `FolderSync.scan` hands to `w.spawn`).
#[derive(Clone, Debug)]
pub struct ScanJob {
    platform: Platform,
    thumbnail_cache: String,
    /// Registered folders to read.
    pub roots: Vec<String>,
    /// Folders taken out of the catalogue.
    pub removed: Vec<String>,
    /// Keys already in the catalogue or excluded.
    pub known: HashSet<String>,
    /// Files that failed to import, with their `(mtime, size)`.
    pub skipped: HashMap<String, (f64, u64)>,
    /// Cancel flag (set by [`FolderSync::shutdown`]).
    pub cancel: Cancel,
    /// The folder-removal revision the job was started in; pass it back to [`FolderSync::found`].
    pub revision: u64,
}

impl ScanJob {
    /// Runs `new_files` with the real clock.
    pub fn run(&self) -> Vec<String> {
        self.run_at(now_epoch())
    }
    /// Runs `new_files` as of `now` (seconds since the epoch).
    pub fn run_at(&self, now: f64) -> Vec<String> {
        let cache = resolve(self.platform, &self.thumbnail_cache);
        let roots: Vec<_> = self
            .roots
            .iter()
            .map(|root| resolve(self.platform, root))
            .filter(|root| !self.platform.contains_folder(&cache, root))
            .collect();
        let mut excluded = self.removed.clone();
        excluded.push(cache);
        new_files(self.platform, &roots, &self.known, &self.skipped, Some(&self.cancel), SETTLE, now, &excluded)
    }
}

/// The result of [`FolderSync::found`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// How many files were queued.
    pub added: usize,
    /// The status bar message, when an import should start.
    pub message: Option<String>,
    /// The caller should show the cancel button and run `import_next`.
    pub start_import: bool,
}

/// Port of `folder_sync.FolderSync`.
pub struct FolderSync {
    platform: Platform,
    clock: Box<dyn Fn() -> f64 + Send>,
    /// A check is running.
    pub running: bool,
    /// Set at shutdown: a running listing stops.
    pub cancel: Cancel,
    last: f64,
    revision: u64,
    auto: BTreeSet<String>,
    skipped: HashMap<String, (f64, u64)>,
    excluded: BTreeSet<String>,
    start_at: Option<f64>,
    interval_next: Option<f64>,
}

impl FolderSync {
    /// Port of `FolderSync.__init__`; `clock` is `time.monotonic` in seconds. The first check is due
    /// [`START_MS`] after this call and then every [`INTERVAL_MS`] (see [`FolderSync::poll`]).
    pub fn new(catalog: &Catalog, clock: impl Fn() -> f64 + Send + 'static) -> Result<Self> {
        let excluded = match catalog.preference(EXCLUDED, json!([]))? {
            Value::Array(items) => items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
            _ => BTreeSet::new(),
        };
        let created = clock();
        Ok(Self {
            platform: catalog.platform(),
            clock: Box::new(clock),
            running: false,
            cancel: Cancel::new(),
            last: 0.0,
            revision: 0,
            auto: BTreeSet::new(),
            skipped: HashMap::new(),
            excluded,
            start_at: Some(created + START_MS as f64 / 1000.0),
            interval_next: Some(created + INTERVAL_MS as f64 / 1000.0),
        })
    }

    /// Port of `FolderSync.enabled`.
    pub fn enabled(&self, catalog: &Catalog) -> Result<bool> {
        Ok(truthy(&catalog.preference(PREFERENCE, json!(true))?))
    }

    /// Port of `FolderSync.set_enabled`: saves the switch and, when it is turned on, checks at once.
    pub fn set_enabled(&mut self, catalog: &Catalog, host: &SyncHost, value: bool) -> Result<Option<ScanJob>> {
        catalog.save_preference(PREFERENCE, &json!(value))?;
        if value {
            self.scan(catalog, host)
        } else {
            Ok(None)
        }
    }

    /// The two Qt timers of the oracle (the single shot after [`START_MS`] and the repeating one): call this
    /// from the application's tick; it starts a check when one is due.
    pub fn poll(&mut self, catalog: &Catalog, host: &SyncHost) -> Result<Option<ScanJob>> {
        let now = (self.clock)();
        let mut due = false;
        if self.start_at.is_some_and(|t| now >= t) {
            self.start_at = None;
            due = true;
        }
        if let Some(mut next) = self.interval_next {
            if now >= next {
                while next <= now {
                    next += INTERVAL_MS as f64 / 1000.0;
                }
                self.interval_next = Some(next);
                due = true;
            }
        }
        if due {
            self.scan(catalog, host)
        } else {
            Ok(None)
        }
    }

    /// Port of `FolderSync.activated`: the application became the active one (`active`) or not.
    pub fn activated(&mut self, catalog: &Catalog, host: &SyncHost, active: bool) -> Result<Option<ScanJob>> {
        if active && (self.clock)() - self.last > ACTIVATE_GAP {
            self.scan(catalog, host)
        } else {
            Ok(None)
        }
    }

    /// Port of `FolderSync.scan`: `None` while another check runs, the window is busy or the feature is off.
    pub fn scan(&mut self, catalog: &Catalog, host: &SyncHost) -> Result<Option<ScanJob>> {
        if self.running || !host.idle() || !self.enabled(catalog)? {
            return Ok(None);
        }
        self.keep_outputs(catalog)?;
        let roots = catalog.folder_roots()?;
        let removed = catalog.removed_folders()?;
        let mut known: HashSet<String> = catalog.paths()?.iter().map(|p| self.platform.path_key(p)).collect();
        known.extend(self.excluded.iter().cloned());
        self.running = true;
        self.last = (self.clock)();
        Ok(Some(ScanJob {
            platform: self.platform,
            thumbnail_cache: crate::pathx::text(&catalog.directory().join("thumbnails")),
            roots,
            removed,
            known,
            skipped: self.skipped.clone(),
            cancel: self.cancel.clone(),
            revision: self.revision,
        }))
    }

    /// Port of `FolderSync.failed`: the listing failed.
    pub fn failed(&mut self) {
        self.running = false;
    }

    /// Port of `FolderSync.found`: a listing arrived. New files are appended to `host.queue`.
    pub fn found(
        &mut self,
        files: &[String],
        revision: Option<u64>,
        catalog: &Catalog,
        host: &mut SyncHost,
    ) -> Result<Found> {
        self.running = false;
        if revision.is_some_and(|r| r != self.revision) {
            return Ok(Found::default()); // listed before a folder was removed
        }
        if files.is_empty() || !host.idle() || !self.enabled(catalog)? {
            return Ok(Found::default());
        }
        let mut known: HashSet<String> = catalog.paths()?.iter().map(|p| self.platform.path_key(p)).collect();
        known.extend(host.pending_paths.iter().cloned());
        known.extend(self.excluded.iter().cloned());
        self.auto = self.auto.intersection(&host.pending_paths).cloned().collect();
        let mut added = 0;
        for file in files {
            let key = self.platform.path_key(&resolve(self.platform, file));
            if known.contains(&key) {
                continue;
            }
            host.queue.push(file.clone());
            host.pending_paths.insert(key.clone());
            known.insert(key.clone());
            self.auto.insert(key);
            added += 1;
        }
        let mut result = Found { added, ..Found::default() };
        if added > 0 && !host.import_busy {
            host.errors.clear();
            result.message = Some(tr_with("등록한 폴더에서 새 사진 {0}장을 찾았습니다.", &[&added.to_string()]));
            result.start_import = true;
        }
        Ok(result)
    }

    /// Port of `FolderSync.import_failed`: true when the failed file was queued automatically (the caller then
    /// reports nothing).
    pub fn import_failed(&mut self, path: &str) -> bool {
        let key = self.platform.path_key(&resolve(self.platform, path));
        if !self.auto.remove(&key) {
            return false;
        }
        if let Ok(meta) = std::fs::metadata(path) {
            if let Some(mtime) = mtime_seconds(&meta) {
                self.skipped.insert(key, (mtime, meta.len()));
            }
        }
        true
    }

    /// Port of `FolderSync.imported`.
    pub fn imported(&mut self, catalog: &Catalog, path: &str, resolved: bool) -> Result<()> {
        let key = self.platform.path_key(&if resolved { path.to_owned() } else { resolve(self.platform, path) });
        self.auto.remove(&key);
        if self.excluded.remove(&key) {
            catalog.save_preference(EXCLUDED, &json!(self.excluded.iter().collect::<Vec<_>>()))?;
        }
        Ok(())
    }

    /// Port of `FolderSync.exclude`: photos removed from the catalogue while the files stay are not brought
    /// back automatically.
    pub fn exclude(&mut self, catalog: &Catalog, paths: &[String]) -> Result<()> {
        for p in paths {
            self.excluded.insert(self.platform.path_key(&resolve(self.platform, p)));
        }
        catalog.save_preference(EXCLUDED, &json!(self.excluded.iter().collect::<Vec<_>>()))?;
        Ok(())
    }

    /// Port of `FolderSync.folder_removed`: a check that is still running lists the removed folder's files as
    /// new, so its result is dropped.
    pub fn folder_removed(&mut self) {
        self.revision += 1;
    }

    /// Port of `FolderSync.release`: closing the window drops an import that only this class queued.
    pub fn release(&mut self, host: &mut SyncHost) {
        if host.import_scans > 0 || host.pending_paths.is_empty() || !host.pending_paths.is_subset(&self.auto) {
            return;
        }
        host.queue.clear();
        host.pending_paths.clear();
        self.auto.clear();
        host.import_busy = false;
    }

    /// Port of `FolderSync.keep_outputs`: remembers exports across restarts so they are not imported as new
    /// photos later.
    pub fn keep_outputs(&mut self, catalog: &Catalog) -> Result<()> {
        let known: HashSet<String> = catalog.paths()?.iter().map(|p| self.platform.path_key(p)).collect();
        let sep = if self.platform == Platform::Windows { '\\' } else { '/' };
        let roots: Vec<String> =
            catalog.folder_roots()?.iter().map(|r| format!("{}{sep}", self.platform.path_key(r))).collect();
        let new: Vec<String> = outputs()
            .into_iter()
            .filter(|k| {
                !self.excluded.contains(k) && !known.contains(k) && roots.iter().any(|r| k.starts_with(r.as_str()))
            })
            .collect();
        if !new.is_empty() {
            self.excluded.extend(new);
            catalog.save_preference(EXCLUDED, &json!(self.excluded.iter().collect::<Vec<_>>()))?;
        }
        Ok(())
    }

    /// Port of `FolderSync.shutdown`: stops the timers (the application stops calling [`FolderSync::poll`]),
    /// cancels a running listing and keeps the exports.
    pub fn shutdown(&mut self, catalog: &Catalog) -> Result<()> {
        self.start_at = None;
        self.interval_next = None;
        self.cancel.set();
        self.keep_outputs(catalog)
    }

    /// The keys excluded from automatic import (`FolderSync.excluded`).
    pub fn excluded(&self) -> &BTreeSet<String> {
        &self.excluded
    }

    /// The keys queued by this class (`FolderSync.auto`).
    pub fn auto(&self) -> &BTreeSet<String> {
        &self.auto
    }

    /// The failed files with their `(mtime, size)` (`FolderSync.skipped`).
    pub fn skipped(&self) -> &HashMap<String, (f64, u64)> {
        &self.skipped
    }

    /// The monotonic time of the last check (`FolderSync.last`, 0 before the first).
    pub fn last(&self) -> f64 {
        self.last
    }

    /// The folder-removal revision (`FolderSync.revision`).
    pub fn revision(&self) -> u64 {
        self.revision
    }
}
