//! Safe, non-destructive folder location repair (work package 22): luma/folder_panel.py `FolderPanel.refresh`
//! (the 5 second rescan and the rename detection of `result['moves']`), `recover` (이름이 바뀐 폴더 자동 찾기),
//! `locate` (폴더 위치 다시 지정…), `apply_many`; and the offline source of luma/app.py `activate` (a photo whose
//! original is missing is developed from its packet `previews/<id>.npz`, `preview_store.load_offline`).
//!
//! The folder scan itself is the library column's (`session_state`); this module acts on what it found. Nothing
//! here moves or deletes a file: a relink rewrites catalogue paths only, after a backup of the catalogue.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use grainy_core::Settings;
use grainy_library::folder_locations::{directory_identity, rebase, relink_plan, Plan};
use grainy_library::folder_recovery::{recover_folders, validate_recovery, Recovery, SmallRgb, Thumbnailer};
use grainy_library::Cancel;
use serde_json::{json, Value};

use crate::commands::files::fail;
use crate::context::AppContext;
use crate::dialogs::confirm::Confirm;
use crate::dialogs::import_errors::Notice;
use crate::dialogs::maintenance::{PathWait, Pick};
use crate::document::catalog::files::{FileOp, FileReply};
use crate::session_state::ColumnState;
use crate::text::{tr, tr_with};

/// `FolderPanel.timer`: the folders are scanned again every 5 seconds.
pub const REFRESH: Duration = Duration::from_secs(5);

/// What the folder panel keeps (`FolderPanel`'s recovery and move fields).
#[derive(Default)]
pub struct FolderPanel {
    last_scan: Option<Instant>,
    /// `failed_moves`: (source, destination, identity) of renames that could not be relinked.
    failed_moves: HashSet<(String, String, Vec<u64>)>,
    /// `recovery_running`.
    pub recovery_running: bool,
    recovery_cancel: Cancel,
    /// `recovery_signature`: the folder state the last automatic search was made for.
    recovery_signature: Option<(u64, Vec<String>)>,
    /// The scan revision the missing folders were last looked at for.
    seen_revision: u64,
    /// The listing whose offline packets were noted.
    offline_listing: Option<(u64, u64, usize)>,
    /// Switched off by tests that must not search on their own.
    pub automatic_off: bool,
}

/// `FolderPanel.busy`: an import, an export, a catalogue job or a maintenance run.
pub fn busy(cx: &AppContext) -> bool {
    cx.jobs.running("import")
        || cx.jobs.running("export")
        || cx.jobs.running("maintenance")
        || cx.state_ref::<crate::commands::files::FilesState>().is_some_and(|s| s.maintenance)
}

/// Once per frame: the 5 second rescan, renamed folders found by the scan, the automatic search for missing
/// folders, and the offline packets of the listed photos whose originals are missing.
pub fn poll(cx: &mut AppContext) {
    if cx.doc.library_folder().is_none() {
        return;
    }
    note_offline(cx);
    let quiet = busy(cx) || cx.modal_open();
    let panel = cx.state::<FolderPanel>();
    if panel.last_scan.is_none_or(|t| t.elapsed() >= REFRESH) {
        panel.last_scan = Some(Instant::now());
        if !quiet {
            cx.state::<ColumnState>().rescan();
        }
    }
    cx.egui.request_repaint_after(REFRESH);
    if quiet {
        return;
    }
    let moves = std::mem::take(&mut cx.state::<ColumnState>().moves);
    if !moves.is_empty() && detect_moves(cx, &moves) {
        return;
    }
    let revision = cx.state::<ColumnState>().scan_revision();
    let missing = !cx.state::<ColumnState>().missing_folders().is_empty();
    let panel = cx.state::<FolderPanel>();
    if revision != panel.seen_revision {
        panel.seen_revision = revision;
        if missing && !panel.automatic_off {
            recover(cx, false);
        }
    }
}

/// The listed photos whose originals are missing and that have a packet in `previews/` are noted for the workers
/// (`worker::register_offline`), once per listing.
fn note_offline(cx: &mut AppContext) {
    let Some(folder) = cx.doc.library_folder() else { return };
    let key = (cx.document_generation, cx.doc.folder_inputs().0, cx.doc.photos().len());
    if cx.state::<FolderPanel>().offline_listing == Some(key) {
        return;
    }
    cx.state::<FolderPanel>().offline_listing = Some(key);
    let photos: Vec<_> = cx.doc.photos().iter().map(|photo| (photo.id, photo.path.clone())).collect();
    cx.spawn_document_job(
        "offline-index",
        "",
        move |_| {
            let entries = match std::fs::read_dir(folder.join("previews")) {
                Ok(entries) => entries,
                Err(_) => return Ok((folder, Vec::new())),
            };
            let packets: HashSet<u64> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str()?.strip_suffix(".npz")?.parse().ok())
                .collect();
            Ok((folder, photos.into_iter().filter(|(id, _)| packets.contains(id)).collect()))
        },
        move |cx, result| {
            if cx.state::<FolderPanel>().offline_listing == Some(key) {
                if let Ok((folder, photos)) = result {
                    for (id, path) in photos {
                        crate::worker::register_offline(&path, &folder, id as i64);
                    }
                }
            }
        },
    );
}

/// Notes the packets again on the next frame (after 오프라인 편집용 미리보기 만들기).
pub fn offline_changed(cx: &mut AppContext) {
    cx.state::<FolderPanel>().offline_listing = None;
}

/// `refresh`'s `finished` for `result['moves']`: a folder renamed within its parent (the old path gone, the new
/// one with the saved identity) is relinked; returns whether one was.
fn detect_moves(cx: &mut AppContext, moves: &[(String, String)]) -> bool {
    let platform = grainy_catalog::folders::Platform::native();
    let saved = cx.doc.preference("folder_identities").unwrap_or(json!({}));
    for (source, destination) in moves {
        let identity: Option<Vec<u64>> = saved
            .get(platform.path_key(source))
            .and_then(|v| v.get("id"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect());
        let token = (source.clone(), destination.clone(), identity.clone().unwrap_or_default());
        if cx.state::<FolderPanel>().failed_moves.contains(&token) {
            continue;
        }
        let current = directory_identity(Path::new(destination)).map(|i| i.to_vec());
        if Path::new(source).exists() || current != identity {
            continue;
        }
        let plan = match cx.doc.metadata_catalog().map(|c| relink_plan(c, source, destination)) {
            Some(Ok(plan)) => plan,
            _ => {
                cx.state::<FolderPanel>().failed_moves.insert(token);
                cx.status = tr("폴더 위치를 확인해 주세요. 폴더 우클릭 → 폴더 위치 다시 지정…");
                continue;
            }
        };
        match apply_many(cx, vec![plan]) {
            Ok(_) => {
                let name = grainy_library::pathx::name(platform, destination);
                cx.status = tr_with("폴더 이름 변경을 감지해 다시 연결했습니다: {0}", &[&name]);
                return true;
            }
            Err(_) => {
                cx.state::<FolderPanel>().failed_moves.insert(token);
                cx.status = tr("폴더 위치를 확인해 주세요. 폴더 우클릭 → 폴더 위치 다시 지정…");
            }
        }
    }
    false
}

/// Port of `FolderPanel.apply_many`: the catalogue is backed up and relinked (one transaction), and the tree's
/// chosen folder and expanded folders follow the moves. Returns how many photo paths changed.
pub fn apply_many(cx: &mut AppContext, plans: Vec<Plan>) -> Result<usize, String> {
    let had = cx.doc.current_id();
    let moves: Vec<(String, String)> = plans.iter().map(|p| (p.source.clone(), p.destination.clone())).collect();
    let result = cx.doc.file_op(FileOp::ApplyRelinks { plans });
    if cx.doc.current_id() != had {
        cx.photo_changed();
    }
    let count = match result? {
        FileReply::Count(n) => n,
        FileReply::Done => 0,
    };
    let platform = grainy_catalog::folders::Platform::native();
    let mut folder = cx.doc.query().folder;
    for (source, destination) in &moves {
        if let Some(f) = &folder {
            folder = Some(rebase(platform, f, source, destination).unwrap_or_else(|_| f.clone()));
        }
        let st = cx.state::<ColumnState>();
        st.expanded = st
            .expanded
            .iter()
            .map(|p| platform.path_key(&rebase(platform, p, source, destination).unwrap_or_else(|_| p.clone())))
            .collect();
    }
    if folder != cx.doc.query().folder {
        cx.change_query(|q| q.folder = folder);
    }
    cx.state::<ColumnState>().rescan();
    Ok(count)
}

// ---- 폴더 위치 다시 지정… -----------------------------------------------------------------------------------------

/// Port of `FolderPanel.locate`: refused while busy; the folder dialog for the new place, then [`locate_to`].
pub fn locate(cx: &mut AppContext, source: &str) {
    if busy(cx) {
        cx.open_dialog(Box::new(Notice {
            id: "folder_locate_busy",
            title: tr("폴더 위치"),
            text: tr("사진 가져오기·내보내기와 카탈로그 작업을 마친 뒤 다시 시도하세요."),
        }));
        return;
    }
    let start = Path::new(source).parent().map(Path::to_path_buf);
    let source = source.to_owned();
    cx.open_dialog(Box::new(PathWait::open(
        tr("이 폴더의 새 위치 선택"),
        Pick::Folder,
        start,
        move |cx, folder| {
            locate_to(cx, &source, &folder);
        },
    )));
}

/// The question of `locate` for the chosen place.
pub fn locate_text(plan: &Plan) -> String {
    tr_with(
        "기존 위치: {0}\n새 위치: {1}\n\n사진·가상 사본 {2}개를 다시 연결합니다. 찾은 원본 참조 {3}개, 찾지 못한 참조 {4}개.\n보정·별점·컬렉션은 유지하며 원본 파일을 이동하거나 삭제하지 않습니다. 변경 전 카탈로그를 자동 백업합니다.\n\n이 위치로 연결할까요?",
        &[
            &plan.source,
            &plan.destination,
            &plan.updates.len().to_string(),
            &plan.found.to_string(),
            &plan.missing.to_string(),
        ],
    )
}

/// `locate` after the folder dialog: the plan (`relink_plan`), the question, and on 예 the relink.
pub fn locate_to(cx: &mut AppContext, source: &str, destination: &Path) {
    let destination = grainy_library::pathx::text(destination);
    let plan = match cx.doc.metadata_catalog().map(|c| relink_plan(c, source, &destination)) {
        Some(Ok(plan)) => plan,
        Some(Err(error)) => return fail(cx, &error.to_string()),
        None => return,
    };
    let text = locate_text(&plan);
    cx.open_dialog(Box::new(Confirm::new(
        "folder_locate",
        tr("폴더 위치 다시 지정"),
        text,
        move |cx: &mut AppContext| {
            locate_now(cx, plan);
        },
    )));
}

/// What 예 of `locate` does.
pub fn locate_now(cx: &mut AppContext, plan: Plan) {
    match apply_many(cx, vec![plan]) {
        Ok(count) => cx.status = tr_with("{0}개 원본 참조를 다시 연결했습니다.", &[&count.to_string()]),
        Err(message) => fail(cx, &message),
    }
}

// ---- 이름이 바뀐 폴더 자동 찾기 -------------------------------------------------------------------------------------

/// The folder menu's item: 이름이 바뀐 폴더 자동 찾기, or 폴더 자동 찾기 중단 while a search runs.
pub fn recover_label(cx: &AppContext) -> &'static str {
    if cx.state_ref::<FolderPanel>().is_some_and(|p| p.recovery_running) {
        "폴더 자동 찾기 중단"
    } else {
        "이름이 바뀐 폴더 자동 찾기"
    }
}

/// The command of that item (`recover(explicit=True)`).
pub fn recover_command(cx: &mut AppContext, _: &crate::commands::Invocation) {
    recover(cx, true);
}

/// Port of `FolderPanel.recover`: a search for the missing folders (renamed or moved) by the photos' file facts
/// and saved previews, on a job; what it proves is relinked through [`apply_many`]. An explicit run while one is
/// running stops it.
pub fn recover(cx: &mut AppContext, explicit: bool) {
    let panel = cx.state::<FolderPanel>();
    if panel.recovery_running {
        if explicit {
            panel.recovery_cancel.set();
        }
        return;
    }
    if busy(cx) || cx.modal_open() {
        return;
    }
    let Some(folder) = cx.doc.library_folder() else { return };
    let mut missing: Vec<String> = cx.state::<ColumnState>().missing_folders().iter().cloned().collect();
    missing.sort();
    let key = (cx.doc.folder_inputs().0, missing);
    let panel = cx.state::<FolderPanel>();
    if !explicit && panel.recovery_signature.as_ref() == Some(&key) {
        return;
    }
    panel.recovery_signature = Some(key);
    panel.recovery_running = true;
    panel.recovery_cancel = Cancel::new();
    let cancel = panel.recovery_cancel.clone();
    cx.status = tr("이름이 바뀐 폴더를 사진 정보·저장된 미리보기로 찾는 중…");
    let token = cancel.clone();
    cx.jobs.spawn(
        "folder_recovery",
        "",
        move |_| recover_folders(&folder, Some(&cancel), None, &AppThumbnails).map_err(|e| e.to_string()),
        move |cx: &mut AppContext, result: Result<Recovery, String>| {
            cx.state::<FolderPanel>().recovery_running = false;
            match result {
                Ok(recovery) => recovered(cx, recovery, explicit, &token),
                Err(_) => cx.status = tr("폴더 자동 찾기를 완료하지 못했습니다. 우클릭 메뉴에서 다시 시도하세요."),
            }
        },
    );
}

/// `recover`'s `done`.
fn recovered(cx: &mut AppContext, result: Recovery, explicit: bool, cancel: &Cancel) {
    if cancel.is_set() {
        return;
    }
    if busy(cx) || cx.modal_open() {
        cx.state::<FolderPanel>().recovery_signature = None;
        return;
    }
    let plans = result.plans;
    if !plans.is_empty() {
        let valid = cx.doc.metadata_catalog().map(|c| validate_recovery(c, &plans));
        if !matches!(valid, Some(Ok(true))) {
            cx.state::<FolderPanel>().recovery_signature = None;
            cx.status = tr("검색 중 폴더 정보가 바뀌어 다시 확인합니다.");
            return;
        }
        let moves: Vec<Value> = plans
            .iter()
            .map(|p| {
                json!({"source": p.plan.source, "destination": p.plan.destination, "proof": p.proof,
                       "physical": p.physical, "anchors": p.anchors})
            })
            .collect();
        let unresolved: Vec<Value> =
            result.unresolved.iter().map(|u| json!({"source": u.source, "reason": u.reason})).collect();
        let folders = plans.len();
        match apply_many(cx, plans.into_iter().map(|p| p.plan).collect()) {
            Ok(count) => {
                let report = json!({"created": chrono::Local::now().naive_local().format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
                    "photos": count, "moves": moves, "unresolved": unresolved, "incomplete": result.incomplete});
                if let Err(message) = cx.doc.save_preference("folder_recovery_last", &report) {
                    log::warn!("cannot save the recovery report: {message}");
                }
                cx.status = tr_with(
                    "이름이 바뀐 폴더 {0}개 · 사진 {1}개를 자동으로 다시 연결했습니다.",
                    &[&folders.to_string(), &count.to_string()],
                );
            }
            Err(message) => fail(cx, &message),
        }
    } else if explicit {
        cx.status = tr("자동 연결할 폴더를 찾지 못했습니다. 후보가 중복되거나 사진 일치 근거가 부족합니다.");
    }
}

// ---- the pictures the search compares ----------------------------------------------------------------------

/// The images `folder_recovery` compares (Pillow in the oracle): saved previews decoded, candidates rendered with
/// their settings or read as thumbnails, each reduced to 64 × 64 with a Lanczos filter.
pub struct AppThumbnails;

fn small(width: usize, height: usize, rgb: Vec<u8>) -> Option<SmallRgb> {
    let image = image::RgbImage::from_raw(width as u32, height as u32, rgb)?;
    let reduced = image::imageops::resize(&image, 64, 64, image::imageops::FilterType::Lanczos3);
    let pixels = ndarray::Array3::from_shape_vec((64, 64, 3), reduced.into_raw()).ok()?;
    Some(SmallRgb { pixels, width: width as u32, height: height as u32 })
}

impl Thumbnailer for AppThumbnails {
    fn evidence_image(&self, jpeg: &[u8]) -> Option<SmallRgb> {
        let (width, height, rgba) = crate::thumbjpeg::decode(jpeg).ok()?;
        if width.max(height) > 2048 {
            return None;
        }
        let rgb = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        small(width, height, rgb)
    }

    fn candidate_image(
        &self,
        path: &Path,
        settings: &Settings,
        rendered: bool,
        color_space: &str,
    ) -> grainy_library::Result<SmallRgb> {
        let fail = |e: String| grainy_library::Error::value(e);
        let pixels: ndarray::Array3<u8> = if rendered {
            let loaded =
                grainy_engine::files::load_photo(path, Some(1800), settings).map_err(|e| fail(e.to_string()))?;
            let (h, w, _) = loaded.photo.pixels().dim();
            let size = |k: &str, f: usize| loaded.metadata.get(k).and_then(Value::as_f64).unwrap_or(f as f64);
            let space = if color_space == "ProPhoto RGB" { "ProPhoto RGB" } else { "sRGB" };
            let options = grainy_engine::Options {
                output: grainy_engine::Output::Named(space),
                original_size: Some((size("width", w), size("height", h))),
                ..grainy_engine::Options::default()
            };
            let image = grainy_engine::develop(loaded.photo.source(), &settings.0, &options)
                .map_err(|e| fail(e.to_string()))?;
            image.mapv(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        } else {
            crate::document::catalog::import::preview(path, &[]).map_err(fail)?.0
        };
        let (h, w, _) = pixels.dim();
        let rgb = pixels.as_standard_layout().iter().copied().collect();
        small(w, h, rgb).ok_or_else(|| fail("not an image".into()))
    }
}
