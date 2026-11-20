//! The timers of the automatic imports (work package 24): luma/folder_sync.py `FolderSync` (등록한 폴더의 새 사진
//! 자동으로 가져오기: shortly after start, whenever Grainy becomes the active application and every 120 seconds,
//! the registered folders are read for new photos) and the 20 second timer of luma/extras.py `ExtraTools`
//! (`watch_scan` of the folders of 현재 폴더 자동 가져오기).
//!
//! `grainy_library::folder_sync::FolderSync` holds the oracle's state and decisions; this module calls it from
//! the window's frame ([`poll`]), runs its folder listing as a quiet background job and hands what it found to
//! the import (`commands::import::start_import`). Only folders registered in the catalogue are read; files are
//! never written. The clock is injectable ([`set_clock`]) so tests never wait for the real two minutes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grainy_catalog::Catalog;
use grainy_library::folder_sync::{FolderSync, ScanJob, SyncHost, EXCLUDED, PREFERENCE};
use serde_json::{json, Value};

use crate::commands::import::{self, ImportRun, Imported, COPY_KIND};
use crate::context::AppContext;

/// `ExtraTools.timer`: the watched folders are read every 20 seconds.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(20);

/// The job kind of the folder listing (no label: the status bar does not show it).
pub const SCAN_KIND: &str = "folder_sync";

type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

fn real_clock() -> Clock {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    Arc::new(move || start.elapsed().as_secs_f64())
}

/// What the automatic imports keep between frames.
#[derive(Default)]
pub struct WatchState {
    sync: Option<FolderSync>,
    /// The library `sync` belongs to.
    library: Option<PathBuf>,
    clock: Option<Clock>,
    /// The window had the focus in the last frame.
    focused: Option<bool>,
    /// When the watched folders are read next (`None`: the timer is stopped).
    watch_next: Option<Instant>,
    /// The interval of the watched folders' timer (tests shorten it).
    pub watch_interval: Option<Duration>,
    /// The last time the excluded list was read again from the catalogue.
    reconciled: Option<Instant>,
    /// Switched off by tests that drive every step themselves.
    pub automatic_off: bool,
}

/// Sets the clock of `FolderSync` (seconds, like `time.monotonic`). Call it before the first frame; the next
/// library opened uses it.
pub fn set_clock(cx: &mut AppContext, clock: impl Fn() -> f64 + Send + Sync + 'static) {
    let state = cx.state::<WatchState>();
    state.clock = Some(Arc::new(clock));
    state.sync = None;
    state.library = None;
}

/// `FolderSync.idle`'s view of the window.
fn host(cx: &AppContext) -> SyncHost {
    SyncHost {
        closing: false,
        import_busy: cx.jobs.running("import"),
        import_scans: usize::from(cx.jobs.running(COPY_KIND)),
        export_running: cx.jobs.running("export"),
        maintenance_running: maintenance_running(cx),
        command_running: cx.jobs.running("workflow"),
        import_cancelled: cx.state_ref::<crate::session_state::ColumnState>().is_some_and(|s| s.cancelling),
        ..SyncHost::default()
    }
}

/// `maintenance_running`: catalogue maintenance (optimise, integrity, duplicates…) runs.
pub fn maintenance_running(cx: &AppContext) -> bool {
    cx.jobs.running("maintenance")
        || cx.state_ref::<crate::commands::files::FilesState>().is_some_and(|s| s.maintenance)
}

/// Runs `work` with the state taken out of the context and the open library's catalogue (writable libraries only).
fn with_sync<T>(cx: &mut AppContext, work: impl FnOnce(&mut WatchState, &Catalog, &SyncHost) -> T) -> Option<T> {
    let folder = cx.doc.library_folder()?;
    let mut state = std::mem::take(cx.state::<WatchState>());
    let host = host(cx);
    let result = cx.doc.metadata_catalog().and_then(|catalog| {
        if state.library.as_ref() != Some(&folder) || state.sync.is_none() {
            let clock = state.clock.clone().unwrap_or_else(real_clock);
            state.sync = match FolderSync::new(catalog, move || clock()) {
                Ok(sync) => Some(sync),
                Err(error) => {
                    log::warn!("the automatic folder import cannot start: {error}");
                    None
                }
            };
            state.library = Some(folder.clone());
            state.reconciled = None;
            let watched = catalog.preference("watch_folders", json!([])).ok();
            state.watch_next = watched
                .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
                .map(|_| Instant::now() + state.watch_interval.unwrap_or(WATCH_INTERVAL));
        }
        state.sync.as_ref()?;
        Some(work(&mut state, catalog, &host))
    });
    *cx.state::<WatchState>() = state;
    result
}

/// The excluded list of the catalogue (`sync_excluded`) is the truth: photos taken out of the catalogue by other
/// features (카탈로그에서만 제거) are added there by their own `FolderSync`, and an import that brings a photo back
/// removes it. The in-memory list follows it before this module's `FolderSync` writes or reads it.
fn reconcile(sync: &mut FolderSync, catalog: &Catalog) {
    let stored: std::collections::BTreeSet<String> = match catalog.preference(EXCLUDED, json!([])) {
        Ok(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        _ => return,
    };
    if &stored == sync.excluded() {
        return;
    }
    let gone: Vec<String> = sync.excluded().difference(&stored).cloned().collect();
    for key in gone {
        if let Err(error) = sync.imported(catalog, &key, true) {
            log::warn!("sync_excluded: {error}");
        }
    }
    let new: Vec<String> = stored.difference(sync.excluded()).cloned().collect();
    if !new.is_empty() {
        if let Err(error) = sync.exclude(catalog, &new) {
            log::warn!("sync_excluded: {error}");
        }
    }
}

/// Once per frame: the start and interval timers of `FolderSync`, the check when the window becomes active, and
/// the 20 second timer of the watched folders.
pub fn poll(cx: &mut AppContext) {
    if cx.doc.library_folder().is_none() || cx.state::<WatchState>().automatic_off {
        return;
    }
    let focused = cx.egui.input(|i| i.focused);
    let job = with_sync(cx, |state, catalog, host| {
        let activated = state.focused == Some(false) && focused;
        state.focused = Some(focused);
        let sync = state.sync.as_mut()?;
        if state.reconciled.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            state.reconciled = Some(Instant::now());
            reconcile(sync, catalog);
        }
        let job = if activated { sync.activated(catalog, host, true) } else { sync.poll(catalog, host) };
        job.unwrap_or_else(|error| {
            log::warn!("the automatic folder import: {error}");
            None
        })
    })
    .flatten();
    if let Some(job) = job {
        start_scan(cx, job);
    }
    let watch = cx.state::<WatchState>();
    if watch.watch_next.is_some_and(|t| Instant::now() >= t) {
        watch.watch_next = Some(Instant::now() + watch.watch_interval.unwrap_or(WATCH_INTERVAL));
        import::watch_scan(cx);
    }
    cx.egui.request_repaint_after(Duration::from_secs(1));
}

/// `FolderSync.scan` now (the 120 second timer's tick; tests call it instead of waiting).
pub fn scan_now(cx: &mut AppContext) -> bool {
    let job = with_sync(cx, |state, catalog, host| {
        let sync = state.sync.as_mut()?;
        reconcile(sync, catalog);
        sync.scan(catalog, host).ok().flatten()
    })
    .flatten();
    match job {
        Some(job) => {
            start_scan(cx, job);
            true
        }
        None => false,
    }
}

/// The listing runs on a worker thread; `FolderSync.found` gets its result.
fn start_scan(cx: &mut AppContext, job: ScanJob) {
    let (revision, removed) = (job.revision, job.removed.clone());
    cx.spawn_document_job(
        SCAN_KIND,
        "",
        move |_| Ok(job.run()),
        move |cx: &mut AppContext, result: Result<Vec<String>, String>| {
            let files = match result {
                Ok(files) => files,
                Err(_) => {
                    with_sync(cx, |state, _, _| state.sync.as_mut().map(FolderSync::failed));
                    return;
                }
            };
            let found = with_sync(cx, |state, catalog, host| {
                let sync = state.sync.as_mut()?;
                // `folder_removed`: a folder left the catalogue while the listing ran; its files are not new.
                if catalog.removed_folders().ok().is_some_and(|now| now != removed) {
                    sync.folder_removed();
                }
                reconcile(sync, catalog);
                let mut host = host.clone();
                match sync.found(&files, Some(revision), catalog, &mut host) {
                    Ok(found) => Some((found, host.queue)),
                    Err(error) => {
                        log::warn!("the automatic folder import: {error}");
                        None
                    }
                }
            })
            .flatten();
            if let Some((found, queue)) = found {
                if found.start_import {
                    import::start_import(cx, ImportRun::automatic(queue.into_iter().map(PathBuf::from).collect()));
                    if let Some(message) = found.message {
                        cx.status = message;
                    }
                }
            }
        },
    );
}

/// Persist watched-folder export exclusions while the owning library is still attached.
pub(crate) fn shutdown(cx: &mut AppContext) {
    with_sync(cx, |state, catalog, _| {
        if let Some(sync) = &mut state.sync {
            reconcile(sync, catalog);
            if let Err(e) = sync.shutdown(catalog) {
                log::warn!("folder exclusions: {e}");
            }
        }
        state.sync = None;
        state.library = None;
    });
}

/// An import finished: `FolderSync.imported` for the photos added, `import_failed` for the files the automatic
/// import queued that could not be read (they are offered again only after they change).
pub fn imported(cx: &mut AppContext, result: &Imported, automatic: bool) {
    with_sync(cx, |state, catalog, _| {
        let sync = state.sync.as_mut()?;
        reconcile(sync, catalog);
        for (path, added) in &result.files {
            if *added {
                if let Err(error) = sync.imported(catalog, path, false) {
                    log::warn!("sync_excluded: {error}");
                }
            } else if automatic && !result.cancelled {
                sync.import_failed(path);
            }
        }
        Some(())
    });
}

/// Whether 등록한 폴더의 새 사진 자동으로 가져오기 is on (`FolderSync.enabled`; on when never set).
pub fn sync_enabled(cx: &AppContext) -> bool {
    cx.doc.library_folder().is_some()
        && cx.doc.metadata_catalog().is_some_and(|c| {
            c.preference(PREFERENCE, json!(true)).map(|v| grainy_core::settings::truthy(&v)).unwrap_or(true)
        })
}

/// `FolderSync.set_enabled`: saves the switch; turning it on checks the folders at once.
pub fn set_sync_enabled(cx: &mut AppContext, on: bool) {
    let job = with_sync(cx, |state, catalog, host| {
        let sync = state.sync.as_mut()?;
        sync.set_enabled(catalog, host, on).unwrap_or_else(|error| {
            log::warn!("{error}");
            None
        })
    })
    .flatten();
    if let Some(job) = job {
        start_scan(cx, job);
    }
}

/// Makes sure the automatic import is set up for the open library (tests, and the commands).
pub fn ensure(cx: &mut AppContext) -> bool {
    with_sync(cx, |_, _, _| ()).is_some()
}

/// The keys excluded from the automatic import and the keys it queued (`FolderSync.excluded`, `auto`), after the
/// catalogue's list was read again.
pub fn keys(cx: &mut AppContext) -> (Vec<String>, Vec<String>) {
    with_sync(cx, |state, catalog, _| {
        let sync = state.sync.as_mut()?;
        reconcile(sync, catalog);
        Some((sync.excluded().iter().cloned().collect(), sync.auto().iter().cloned().collect()))
    })
    .flatten()
    .unwrap_or_default()
}

/// `self.timer.start()` of `ExtraTools.watch`.
pub fn start_watch_timer(cx: &mut AppContext) {
    let state = cx.state::<WatchState>();
    state.watch_next = Some(Instant::now() + state.watch_interval.unwrap_or(WATCH_INTERVAL));
}

/// `self.timer.stop()` of `ExtraTools.unwatch`.
pub fn stop_watch_timer(cx: &mut AppContext) {
    cx.state::<WatchState>().watch_next = None;
}

/// Whether the watched folders' timer runs.
pub fn watch_timer_running(cx: &mut AppContext) -> bool {
    cx.state::<WatchState>().watch_next.is_some()
}
