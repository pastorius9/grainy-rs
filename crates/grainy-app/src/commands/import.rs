//! 추가 도구 → 사진 복사해서 가져오기 (카드·다른 폴더)…, 현재 폴더 자동 가져오기, 자동 가져오기 해제, and 설정 → 등록한
//! 폴더의 새 사진 자동으로 가져오기 (work package 24).
//!
//! Ports luma/import_copy.py `dialog`'s run (the job: `plan`, `execute` with the verified copy and the backup
//! copy, `known_keys` for the duplicates; then `import_paths(copied)`), luma/app.py `apply_import_extras`
//! (keywords and a develop preset for the copies), luma/extras.py `watch` / `unwatch` / `watch_scan` and the
//! switch of luma/folder_sync.py (`FolderSync.set_enabled`). The timers live in `folder_watch.rs`.
//!
//! Data safety: sources are only read; a copy never replaces an existing file (`import_copy._free`, exclusive
//! create of a `.grainy-part` file, SHA-256 check before it takes its name); the import adds catalogue rows only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use grainy_core::Settings;
use grainy_library::{import_copy, pathx, Cancel};
use serde_json::{json, Map, Value};

use super::{writable_library, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::import_copy::{CopyProgress, CopyShared, Extras, Form, ImportCopyDialog};
use crate::dialogs::import_errors::Notice;
use crate::dialogs::maintenance::{PathWait, Pick};
use crate::document::catalog::import;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command {
        id: "import.copy",
        label: "사진 복사해서 가져오기 (카드·다른 폴더)…",
        enabled: writable_library,
        run: open_copy,
    },
    Command { id: "import.watch", label: "현재 폴더 자동 가져오기", enabled: writable_library, run: watch },
    Command { id: "import.unwatch", label: "자동 가져오기 해제", enabled: writable_library, run: unwatch },
    Command {
        id: "import.folder_sync",
        label: "등록한 폴더의 새 사진 자동으로 가져오기",
        enabled: super::always,
        run: toggle_folder_sync,
    },
];

/// The job kind of the copy (the oracle counts it in `import_scans`).
pub const COPY_KIND: &str = "import_copy";

/// `w.import_busy or w.import_scans`: an import or a copy runs.
pub fn importing(cx: &AppContext) -> bool {
    cx.jobs.running("import") || cx.jobs.running(COPY_KIND)
}

// ---- the copy -------------------------------------------------------------------------------------------------

/// 사진 복사해서 가져오기: the dialog with the saved fields, unless an import is running.
fn open_copy(cx: &mut AppContext, _: &Invocation) {
    if importing(cx) {
        cx.status = tr("진행 중인 가져오기가 끝난 뒤 다시 시도하세요.");
        return;
    }
    crate::commands::presets::ensure_loaded(cx);
    let saved = cx.doc.preference("import_copy").unwrap_or(Value::Null);
    cx.open_dialog(Box::new(ImportCopyDialog::new(Form::from_saved(&saved))));
}

/// The names of 현상 프리셋 (`w.all_presets`: the built-in presets, then the library's).
pub fn preset_names(cx: &mut AppContext) -> Vec<String> {
    let user = cx.state::<crate::commands::presets::PresetState>().user.clone();
    crate::commands::presets::all_presets(&user).into_iter().map(|(name, _)| name).collect()
}

/// 복사하고 가져오기: the dialog's checks, then the `import_copy` preference is saved and the copy job starts
/// (its progress box opens). `Err` is the problem the oracle shows in a box (the dialog stays open).
pub fn submit(cx: &mut AppContext, form: &Form) -> Result<(), String> {
    if let Some(problem) = form.problem() {
        return Err(problem);
    }
    let Some(folder) = cx.doc.library_folder() else {
        return Err(tr("읽기 전용 라이브러리에는 사진을 가져올 수 없습니다."));
    };
    if let Err(message) = cx.doc.save_preference("import_copy", &form.saved()) {
        log::warn!("the copy import's fields were not saved: {message}");
    }
    let run = CopyRun {
        source: form.source_folder(),
        destination: form.destination_folder(),
        backup: form.backup_folder(),
        subfolders: form.subfolders.clone(),
        template: form.template.clone(),
        skip: form.skip,
    };
    let extras = form.extras();
    let shared = CopyShared::default();
    let (progress, finish) = (shared.clone(), shared.clone());
    let generation = cx.document_generation;
    let job = cx.jobs.spawn(
        COPY_KIND,
        tr("사진을 복사하는 중…"),
        move |handle| run.run(&folder, handle, &progress),
        move |cx: &mut AppContext, result: Result<Copied, String>| {
            finish.update(|s| s.finished = true); // the box closes
            if generation == cx.document_generation {
                copied(cx, result, &extras);
            }
        },
    );
    cx.open_dialog(Box::new(CopyProgress::new(job, shared)));
    Ok(())
}

/// What the copy is asked to do.
#[derive(Clone, Debug)]
pub struct CopyRun {
    pub source: String,
    pub destination: String,
    pub backup: Option<String>,
    pub subfolders: String,
    pub template: String,
    pub skip: bool,
}

/// What the copy did: `(copied, skipped, errors)` of the oracle's `work`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Copied {
    /// Destination paths of the copies.
    pub copied: Vec<String>,
    /// Sources left out as suspected duplicates.
    pub skipped: Vec<String>,
    /// `"<name>: <error>"` per photo that was not copied.
    pub errors: Vec<String>,
}

/// `sorted(str(p) for p in src.rglob('*') if p.is_file() and p.suffix.lower() in IMAGE_EXTENSIONS)`.
pub fn image_files(source: &str) -> Vec<String> {
    if !Path::new(source).is_dir() {
        return Vec::new();
    }
    let (files, _) = import::scan(&[PathBuf::from(source)], &false.into());
    files.into_iter().map(|file| pathx::text(&file)).collect()
}

/// `engine.read_metadata` for `plan`: the EXIF capture time and camera. A file whose metadata cannot be read
/// gets none (its capture time is then the file's time).
fn read_metadata(path: &str) -> Map<String, Value> {
    grainy_codecs::read_metadata(path).unwrap_or_default()
}

impl CopyRun {
    /// The oracle's `work`: the files of the source, `known_keys` when duplicates are left out, `plan` and
    /// `execute`. Cancelling stops between photos (`cancel.is_set()` before each one).
    pub fn run(&self, library: &Path, handle: &crate::jobs::JobHandle, shared: &CopyShared) -> Result<Copied, String> {
        let platform = Platform::native();
        let (files, _) = import::scan_for_library(&[PathBuf::from(&self.source)], handle.cancel_flag(), library);
        let files: Vec<_> = files.into_iter().map(|file| pathx::text(&file)).collect();
        shared.update(|s| s.total = files.len());
        handle.progress(0, files.len(), "");
        let known = if self.skip {
            let catalog = Catalog::new(library).map_err(|e| e.to_string())?;
            import_copy::known_keys(&catalog).map_err(|e| e.to_string())?
        } else {
            Default::default()
        };
        let cancel = Cancel::new();
        let note_cancel = || {
            if handle.cancelled() {
                cancel.set();
            }
        };
        let mut read = |path: &str| {
            note_cancel();
            Ok(read_metadata(path))
        };
        let (planned, skipped) =
            import_copy::plan(platform, &files, &self.subfolders, &self.template, &known, &mut read)
                .map_err(|e| e.to_string())?;
        for folder in std::iter::once(&self.destination).chain(self.backup.iter()) {
            crate::paths::writable(Path::new(folder)).map_err(|e| e.to_string())?;
            for (_, relative) in &planned {
                let base = Path::new(folder);
                let target = base.join(relative);
                if !grainy_platform::user_paths::resolved_within(&target, base) {
                    return Err("copy destination is outside the selected folder".into());
                }
                crate::paths::writable(&target).map_err(|e| e.to_string())?;
            }
        }
        note_cancel();
        let total = files.len();
        let mut progress = |n: usize| {
            note_cancel();
            shared.update(|s| s.done = n);
            handle.progress(n, total, "");
        };
        let executed = import_copy::execute(
            platform,
            &planned,
            &self.destination,
            self.backup.as_deref(),
            Some(&cancel),
            Some(&mut progress),
        )
        .map_err(|e| e.to_string())?;
        Ok(Copied { copied: executed.copied, skipped, errors: executed.errors })
    }
}

/// The copy's `done`: the copies are imported with the keywords and preset; the summary goes to the status line,
/// or into a box with the photos that were not copied.
fn copied(cx: &mut AppContext, result: Result<Copied, String>, extras: &Extras) {
    match result {
        Ok(result) => {
            let platform = Platform::native();
            let extras: BTreeMap<String, Extras> = result
                .copied
                .iter()
                .map(|path| (platform.path_key(&pathx::resolve(platform, path)), extras.clone()))
                .collect();
            if !result.copied.is_empty() {
                start_import(cx, ImportRun::copies(result.copied.iter().map(PathBuf::from).collect(), extras));
            }
            let summary = tr_with(
                "{0}장 복사 · 중복으로 건너뜀 {1}장",
                &[&result.copied.len().to_string(), &result.skipped.len().to_string()],
            );
            if result.errors.is_empty() {
                cx.status = summary;
            } else {
                let lines: Vec<String> = result.errors.iter().take(10).cloned().collect();
                cx.open_dialog(Box::new(Notice {
                    id: "import_copy_errors",
                    title: tr("복사하지 못한 사진"),
                    text: format!("{summary}\n{}", lines.join("\n")),
                }));
            }
        }
        Err(message) => crate::dialogs::preset_review::show_error(cx, &message),
    }
}

// ---- the import of copies, watched folders and new files ------------------------------------------------------

/// One import (`import_paths` or the queue `FolderSync.found` fills).
#[derive(Clone, Debug, Default)]
pub struct ImportRun {
    /// Files or folders.
    pub paths: Vec<PathBuf>,
    /// The folders below `paths` are looked through (`import_paths`); otherwise `paths` are the files (the queue).
    pub scan: bool,
    /// `restore`: the person chose the paths, so folders removed from the catalogue below them come back. An
    /// automatic re-read of a watched folder passes false and leaves them out.
    pub restore: bool,
    /// Keywords and preset by path key (`import_extras`).
    pub extras: BTreeMap<String, Extras>,
    /// The files were queued by the automatic folder import: failures are not reported
    /// (`FolderSync.import_failed`).
    pub automatic: bool,
}

impl ImportRun {
    /// `import_paths(copied)` with the copies' extras.
    pub fn copies(paths: Vec<PathBuf>, extras: BTreeMap<String, Extras>) -> Self {
        Self { paths, scan: true, restore: true, extras, automatic: false }
    }

    /// `import_paths(paths, restore=False)` of `watch_scan`.
    pub fn watched(paths: Vec<PathBuf>) -> Self {
        Self { paths, scan: true, restore: false, ..Self::default() }
    }

    /// The files `FolderSync.found` queued.
    pub fn automatic(files: Vec<PathBuf>) -> Self {
        Self { paths: files, scan: false, restore: false, automatic: true, ..Self::default() }
    }
}

/// What an import run did, for its `done`.
pub struct Imported {
    pub report: import::Report,
    /// Every file that was to be imported, and whether it is in the catalogue now.
    pub files: Vec<(String, bool)>,
    /// The import was cancelled.
    pub cancelled: bool,
}

/// Runs a catalogue write again while SQLite says the database is locked (the window's connection may be
/// writing at the same moment).
fn retrying<T>(mut write: impl FnMut() -> grainy_catalog::Result<T>) -> grainy_catalog::Result<T> {
    let mut tries = 0;
    loop {
        match write() {
            Err(error) if tries < 100 && error.to_string().contains("database is locked") => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            other => return other,
        }
    }
}

/// Port of `apply_import_extras`: keywords and a develop preset chosen in the copy import. Returns the new
/// settings, if any; a preset that does not fit the photo adds `"<name>: <reason>"` to `errors`.
pub fn apply_import_extras(
    catalog: &Catalog,
    id: i64,
    extras: &Extras,
    presets: &[(String, Value)],
    errors: &mut Vec<String>,
) -> Result<Option<Settings>, String> {
    let text = |e: grainy_catalog::Error| e.to_string();
    let Some(record) = catalog.photo(id).map_err(text)? else { return Ok(None) };
    if !extras.keywords.is_empty() {
        let existing: Vec<String> = record["keywords"]
            .as_str()
            .unwrap_or("")
            .split(',')
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .collect();
        // Words already on the photo are not added again; a word given twice is (as the oracle does).
        let added: Vec<String> = extras
            .keywords
            .split(',')
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty() && !existing.contains(k))
            .collect();
        let joined = existing.into_iter().chain(added).collect::<Vec<_>>().join(", ");
        let mut fields = Map::new();
        fields.insert("keywords".into(), json!(joined));
        retrying(|| catalog.update(id, &fields)).map_err(text)?;
    }
    if extras.preset.is_empty() {
        return Ok(None);
    }
    let Some((_, preset)) = presets.iter().find(|(name, _)| *name == extras.preset) else { return Ok(None) };
    let base = Settings(record["settings"].as_object().cloned().unwrap_or_default());
    let path = record["path"].as_str().unwrap_or("").to_owned();
    let value = match crate::commands::presets::applied(preset, &base, Path::new(&path), &record["info"]) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("{}: {error}", pathx::name(Platform::native(), &path)));
            return Ok(None);
        }
    };
    retrying(|| catalog.set_settings(id, &value)).map_err(text)?;
    Ok(Some(value))
}

/// `import_paths(paths, restore=False)`'s `skipped`: the folders removed from the catalogue that lie inside one
/// of the paths (their files are left out), as path-key prefixes.
fn removed_below(catalog: &Catalog, paths: &[PathBuf]) -> Vec<String> {
    let platform = catalog.platform();
    let sep = if platform == Platform::Windows { '\\' } else { '/' };
    let removed = catalog.removed_folders().unwrap_or_default();
    removed
        .iter()
        .filter(|folder| paths.iter().any(|p| platform.contains_folder(&pathx::text(p), folder)))
        .map(|folder| format!("{}{sep}", platform.path_key(folder)))
        .collect()
}

impl ImportRun {
    /// The work of the import job (on its own connection).
    pub fn run(
        &self,
        library: &Path,
        cancel: &std::sync::atomic::AtomicBool,
        progress: &dyn Fn(usize, usize, &str),
    ) -> Result<Imported, String> {
        let catalog = Catalog::new(library).map_err(|e| e.to_string())?;
        let platform = catalog.platform();
        let (mut files, roots) = if self.scan {
            import::scan_for_library(&self.paths, cancel, library)
        } else {
            (self.paths.clone(), vec![])
        };
        let roots = if self.restore {
            roots
        } else {
            let skipped = removed_below(&catalog, &self.paths);
            if !skipped.is_empty() {
                files.retain(|f| {
                    let key = platform.path_key(&pathx::text(f));
                    !skipped.iter().any(|s| key.starts_with(s.as_str()))
                });
            }
            for root in &roots {
                retrying(|| catalog.register_folder(&pathx::text(root), false)).map_err(|e| e.to_string())?;
            }
            Vec::new()
        };
        let mut report = import::import_files(&catalog, &files, &roots, cancel, progress)?;
        let all_presets = if self.extras.values().any(|e| !e.preset.is_empty()) {
            let user = crate::commands::presets::Store::open(Some(library), &[])?.list()?;
            crate::commands::presets::all_presets(&user)
        } else {
            Vec::new()
        };
        let mut errors = Vec::new();
        for &id in &report.added {
            let Ok(Some(row)) = catalog.photo(id) else { continue };
            let key = platform.path_key(row["path"].as_str().unwrap_or(""));
            if let Some(extras) = self.extras.get(&key) {
                apply_import_extras(&catalog, id, extras, &all_presets, &mut errors)?;
            }
        }
        report.errors.extend(errors);
        let present: std::collections::HashSet<String> =
            catalog.paths().map_err(|e| e.to_string())?.iter().map(|p| platform.path_key(p)).collect();
        let files = files
            .iter()
            .map(|f| {
                let text = pathx::text(f);
                let found = present.contains(&platform.path_key(&pathx::resolve(platform, &text)));
                (text, found)
            })
            .collect();
        Ok(Imported { report, files, cancelled: cancel.load(std::sync::atomic::Ordering::Relaxed) })
    }
}

/// Starts an import job (kind `import`: the status bar shows it with 가져오기 중단).
pub fn start_import(cx: &mut AppContext, run: ImportRun) {
    let Some(folder) = cx.doc.library_folder() else { return };
    if run.paths.is_empty() {
        return;
    }
    if run.scan {
        cx.status = tr("폴더를 찾는 중…");
    }
    cx.spawn_document_job(
        "import",
        tr("사진 가져오기"),
        move |handle| {
            let progress = |done: usize, total: usize, name: &str| {
                let text = if name.is_empty() {
                    String::new()
                } else {
                    tr_with("가져오는 중: {0}  ·  남은 사진 {1}장", &[name, &(total - done).to_string()])
                };
                handle.progress(done, total, text);
            };
            let imported = run.run(&folder, handle.cancel_flag(), &progress)?;
            Ok((imported, run.automatic))
        },
        |cx: &mut AppContext, result: Result<(Imported, bool), String>| match result {
            Ok((mut imported, automatic)) => {
                crate::folder_watch::imported(cx, &imported, automatic);
                if automatic {
                    imported.report.errors.clear();
                }
                super::library_column::import_done(cx, Ok(imported.report));
            }
            Err(message) => super::library_column::import_done(cx, Err(message)),
        },
    );
}

// ---- watched folders (extras.py) -------------------------------------------------------------------------------

/// The `watch_folders` preference (`ExtraTools.watch_paths`).
pub fn watch_paths(cx: &AppContext) -> Vec<String> {
    cx.doc
        .preference("watch_folders")
        .and_then(|v| v.as_array().cloned())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// 현재 폴더 자동 가져오기 (`ExtraTools.watch`): the folder chosen in the tree, else one asked for.
fn watch(cx: &mut AppContext, _: &Invocation) {
    if let Some(folder) = cx.doc.query().folder.filter(|f| !f.is_empty()) {
        add_watch(cx, &folder);
        return;
    }
    cx.open_dialog(Box::new(PathWait::open(
        tr("자동으로 새 사진을 가져올 폴더"),
        Pick::Folder,
        None,
        |cx, folder| {
            add_watch(cx, &pathx::text(&folder));
        },
    )));
}

/// The rest of `ExtraTools.watch` once the folder is known: a folder not watched yet is restored in the
/// catalogue, added to `watch_folders`, the 20 second timer starts and the folder is read at once.
pub fn add_watch(cx: &mut AppContext, folder: &str) {
    let mut paths = watch_paths(cx);
    if folder.is_empty() || paths.iter().any(|p| p == folder) {
        return;
    }
    if let Some(catalog) = cx.doc.metadata_catalog() {
        if let Err(error) = retrying(|| catalog.restore_folder(folder)) {
            log::warn!("restore_folder({folder}): {error}");
        }
    }
    paths.push(folder.to_owned());
    if let Err(message) = cx.doc.save_preference("watch_folders", &json!(paths)) {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
        return;
    }
    crate::folder_watch::start_watch_timer(cx);
    watch_scan(cx);
}

/// 자동 가져오기 해제 (`ExtraTools.unwatch`).
fn unwatch(cx: &mut AppContext, _: &Invocation) {
    crate::folder_watch::stop_watch_timer(cx);
    if let Err(message) = cx.doc.save_preference("watch_folders", &json!([])) {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
    }
}

/// `ExtraTools.watch_scan`: the watched folders that exist are imported again (`restore=False`), unless an
/// import, a copy, an export or catalogue maintenance runs.
pub fn watch_scan(cx: &mut AppContext) {
    if importing(cx) || cx.jobs.running("export") || crate::folder_watch::maintenance_running(cx) {
        return;
    }
    let paths: Vec<PathBuf> =
        watch_paths(cx).into_iter().filter(|p| Path::new(p).is_dir()).map(PathBuf::from).collect();
    if !paths.is_empty() {
        start_import(cx, ImportRun::watched(paths));
    }
}

/// 등록한 폴더의 새 사진 자동으로 가져오기 (`FolderSync.set_enabled(not enabled)`).
fn toggle_folder_sync(cx: &mut AppContext, _: &Invocation) {
    let on = crate::folder_watch::sync_enabled(cx);
    crate::folder_watch::set_sync_enabled(cx, !on);
}
