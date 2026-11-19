//! Port of `luma/folder_locations.py`: read-only directory discovery and transactional catalogue path repair.

use crate::file_access::denied;
use crate::pathx::{self, absolute, is_root, join, name, norm_str, parents, parts_len};
use crate::{Cancel, Error, Result};
use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use grainy_core::settings::truthy;
use grainy_i18n::tr;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// `[st_dev, st_ino, birth time in ns]` of a folder (`directory_identity`).
pub type Identity = [u64; 3];

/// Port of `folder_locations.directory_identity`: `None` for a missing path, a non-folder or an id of 0.
pub fn directory_identity(path: &Path) -> Option<Identity> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let (device, inode) = ids(path, &meta)?;
    if inode == 0 {
        return None;
    }
    // Creation time guards against reused file IDs on Windows. POSIX ctime changes during a rename and must
    // not be used as a creation timestamp. The oracle reads `st_birthtime_ns`, which Python has on Windows only
    // (macOS has the float `st_birthtime` alone): 0 on macOS too, so identities match the oracle's there.
    let birth = if cfg!(windows) {
        meta.created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| u64::try_from(d.as_nanos()).ok())
            .unwrap_or(0)
    } else {
        0
    };
    Some([device, inode, birth])
}

#[cfg(windows)]
fn ids(path: &Path, _meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    match file_id::get_file_id(path).ok()? {
        file_id::FileId::HighRes { volume_serial_number, file_id } => {
            Some((volume_serial_number, u64::try_from(file_id).ok()?))
        }
        file_id::FileId::LowRes { volume_serial_number, file_index } => {
            Some((u64::from(volume_serial_number), file_index))
        }
        file_id::FileId::Inode { device_id, inode_number } => Some((device_id, inode_number)),
    }
}

#[cfg(not(windows))]
fn ids(_path: &Path, meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

/// Port of `folder_locations.rebase`.
pub fn rebase(platform: Platform, path: &str, source: &str, destination: &str) -> Result<String> {
    if platform.contains_folder(source, path) {
        Ok(join_below(platform, destination, path, source)?)
    } else {
        Ok(path.to_owned())
    }
}

pub(crate) fn join_below(p: Platform, destination: &str, path: &str, folder: &str) -> Result<String> {
    let relative = p.below(path, folder)?;
    Ok(if relative == "." { norm_str(p, destination) } else { join(p, destination, &relative) })
}

/// A folder seen by [`scan_folders`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The folder as the catalogue spells it.
    pub path: String,
    /// Its identity.
    pub id: Identity,
}

/// What [`scan_folders`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderScan {
    /// `identities`: path key to the observed identity, in the order the folders were looked at.
    pub identities: Vec<(String, Observed)>,
    /// Keys of folders that are gone (and not merely refused by the system).
    pub missing: Vec<String>,
    /// `(old path, new path)` of folders found renamed within their parent.
    pub moves: Vec<(String, String)>,
    /// Direct subfolders of the scanned folders, sorted.
    pub children: Vec<String>,
    /// Keys of the folders whose children were listed.
    pub scanned: Vec<String>,
    /// The time or entry budget ran out, or the scan was cancelled.
    pub incomplete: bool,
}

fn monotonic_seconds() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// Port of `folder_locations.scan_folders`: scans only catalogue folders, their ancestors and expanded direct
/// children (no image opens, recursive walks, junction traversal or filename-based guesses). Unknown old
/// locations are deliberately left for manual reconnection. `identities` maps a path key to the identity saved
/// earlier; `expanded` holds path keys.
pub fn scan_folders(
    platform: Platform,
    paths: &[String],
    roots: &[String],
    expanded: &HashSet<String>,
    identities: &HashMap<String, Identity>,
    cancel: &Cancel,
) -> FolderScan {
    scan_folders_with(platform, paths, roots, expanded, identities, cancel, &monotonic_seconds)
}

/// [`scan_folders`] with the clock (`time.monotonic`, in seconds) given, so the three-second budget can be tested.
pub fn scan_folders_with(
    platform: Platform,
    paths: &[String],
    roots: &[String],
    expanded: &HashSet<String>,
    identities: &HashMap<String, Identity>,
    cancel: &Cancel,
    clock: &dyn Fn() -> f64,
) -> FolderScan {
    let deadline = clock() + 3.0;
    let mut budget: i64 = 10000;
    let stopped = |cancel: &Cancel| cancel.is_set() || clock() > deadline;
    let key_of = |p: &str| platform.path_key(p);
    // known = {path_key(p): str(Path(p)) for p in paths + roots}
    let mut known: Vec<(String, String)> = Vec::new();
    for p in paths.iter().chain(roots.iter()) {
        let (key, value) = (key_of(p), norm_str(platform, p));
        match known.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => known.push((key, value)),
        }
    }
    let mut tracked = known.clone();
    for (_, value) in &known {
        for parent in parents(platform, value) {
            let key = key_of(&parent);
            if !tracked.iter().any(|(k, _)| *k == key) {
                tracked.push((key, parent));
            }
        }
    }
    let mut observed: Vec<(String, Observed)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut moves: Vec<(String, String)> = Vec::new();
    let mut listing: HashMap<String, Vec<String>> = HashMap::new();
    let mut children: Vec<String> = Vec::new();
    let mut scanned: Vec<String> = Vec::new();

    let mut dirs = |parent: &str, budget: &mut i64| -> Vec<String> {
        let key = key_of(parent);
        if let Some(found) = listing.get(&key) {
            return found.clone();
        }
        let mut result = Vec::new();
        let Ok(entries) = std::fs::read_dir(parent) else { return Vec::new() };
        for entry in entries {
            if stopped(cancel) || *budget <= 0 {
                break;
            }
            *budget -= 1;
            let Ok(entry) = entry else { return Vec::new() };
            // Do not traverse symlinks or junctions. Cloud directories may carry other reparse tags and remain
            // browsable.
            let Ok(kind) = entry.file_type() else { return Vec::new() };
            if kind.is_dir() && !kind.is_symlink() {
                result.push(pathx::text(&entry.path()));
            }
        }
        listing.insert(key, result.clone());
        result
    };

    for (key, path) in &tracked {
        if stopped(cancel) {
            break;
        }
        match directory_identity(Path::new(path)) {
            Some(id) => observed.push((key.clone(), Observed { path: path.clone(), id })),
            None => {
                if !denied(Path::new(path)) {
                    missing.push(key.clone());
                }
            }
        }
    }
    let mut order: Vec<&String> = missing.iter().collect();
    order.sort_by_key(|k| parts_len(platform, k));
    let order: Vec<String> = order.into_iter().cloned().collect();
    for key in order {
        if stopped(cancel) || moves.iter().any(|(old, _)| platform.contains_folder(old, &key)) {
            continue;
        }
        let Some(saved) = identities.get(&key) else { continue };
        let Some((_, tracked_path)) = tracked.iter().find(|(k, _)| *k == key) else { continue };
        let path = tracked_path.clone();
        let up = pathx::parent(platform, &path);
        if is_root(platform, &path) || directory_identity(Path::new(&up)).is_none() {
            continue;
        }
        let candidates: Vec<String> = dirs(&up, &mut budget)
            .into_iter()
            .filter(|p| directory_identity(Path::new(p)).as_ref() == Some(saved))
            .collect();
        if candidates.len() == 1 && key_of(&candidates[0]) != key {
            moves.push((path, candidates[0].clone()));
        }
    }
    let root_keys: HashSet<String> = roots.iter().map(|p| key_of(p)).collect();
    for (key, path) in &known {
        if stopped(cancel) {
            break;
        }
        if !observed.iter().any(|(k, _)| k == key) || (!expanded.contains(key) && !root_keys.contains(key)) {
            continue;
        }
        let values = dirs(path, &mut budget);
        for v in values {
            if !children.contains(&v) {
                children.push(v);
            }
        }
        if !scanned.contains(key) {
            scanned.push(key.clone());
        }
    }
    children.sort();
    let incomplete = stopped(cancel) || budget <= 0;
    FolderScan { identities: observed, missing, moves, children, scanned, incomplete }
}

/// What [`relink_plan`] decided: the photo paths to rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The old folder.
    pub source: String,
    /// The new folder.
    pub destination: String,
    /// `(new path, photo id)` for every photo below the old folder.
    pub updates: Vec<(String, i64)>,
    /// How many of the new paths are files that exist.
    pub found: usize,
    /// How many are not.
    pub missing: usize,
}

/// Port of `folder_locations.relink_plan`.
pub fn relink_plan(catalog: &Catalog, source: &str, destination: &str) -> Result<Plan> {
    let p = catalog.platform();
    let (source, destination) = (absolute(p, source), absolute(p, destination));
    if is_root(p, &source) {
        return Err(Error::value(tr("드라이브 전체를 다시 연결할 수 없습니다.")));
    }
    if p.path_key(&source) == p.path_key(&destination) {
        return Err(Error::value(tr("기존 위치와 다른 폴더를 선택하세요.")));
    }
    if p.contains_folder(&source, &destination) || p.contains_folder(&destination, &source) {
        return Err(Error::value(tr("기존 폴더의 상위·하위 폴더로 다시 연결할 수 없습니다.")));
    }
    if !Path::new(&destination).is_dir() {
        return Err(Error::value(tr("다시 연결할 폴더가 없습니다.")));
    }
    let rows = catalog.rows("SELECT id,path,virtual_source FROM photos", [])?;
    let path_of = |r: &Value| r["path"].as_str().unwrap_or("").to_owned();
    let affected: Vec<&Value> = rows.iter().filter(|r| p.in_folder(&path_of(r), &source, true)).collect();
    let outside: HashSet<String> =
        rows.iter().filter(|r| !p.in_folder(&path_of(r), &source, true)).map(|r| p.path_key(&path_of(r))).collect();
    let mut updates = Vec::new();
    for r in &affected {
        updates.push((join_below(p, &destination, &path_of(r), &source)?, r["id"].as_i64().unwrap_or(0)));
    }
    if updates.iter().any(|(target, _)| outside.contains(&p.path_key(target))) {
        return Err(Error::value(tr("새 위치의 사진이 이미 라이브러리에 있습니다. 중복 연결은 적용하지 않았습니다.")));
    }
    let found = updates.iter().filter(|(target, _)| Path::new(target).is_file()).count();
    if !affected.is_empty() && found == 0 {
        return Err(Error::value(tr(
            "선택한 폴더에서 기존 사진을 찾지 못했습니다. 파일 이름과 하위 폴더 구조를 확인하세요.",
        )));
    }
    let missing = updates.len() - found;
    Ok(Plan { source, destination, updates, found, missing })
}

/// Port of `folder_locations.apply_relink`: retains photo IDs and every edit, replaces the whole folder prefix
/// atomically.
pub fn apply_relink(catalog: &Catalog, plan: &Plan) -> Result<usize> {
    apply_relinks(catalog, std::slice::from_ref(plan))
}

fn strings(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(|| Error::Type("a folder path must be a string".into())))
            .collect(),
        _ => Err(Error::Type("a list of folders is expected".into())),
    }
}

/// Port of `folder_locations.apply_relinks`: a batch of locations is one transaction, including all dependent
/// paths (registered folders, the folder panel, watched folders, identities, removed folders).
pub fn apply_relinks(catalog: &Catalog, plans: &[Plan]) -> Result<usize> {
    let p = catalog.platform();
    let mut roots = catalog.folder_roots()?;
    let mut panel = catalog.preference("folder_panel", json!({}))?;
    let mut watches = strings(&catalog.preference("watch_folders", json!([]))?)?;
    let mut identities = catalog.preference("folder_identities", json!({}))?;
    let mut removed = catalog.removed_folders()?;
    let Some(panel_map) = panel.as_object_mut() else {
        return Err(Error::Type("'folder_panel' is not a dict".into()));
    };
    let Some(identity_map) = identities.as_object_mut() else {
        return Err(Error::Type("'folder_identities' is not a dict".into()));
    };
    crate::tx(catalog, || {
        let db = catalog.connection();
        for plan in plans {
            // A location found by listing the disk arrives as the disk spells it (decomposed on macOS).
            let source = plan.source.clone();
            let destination = p.stored_path(&plan.destination);
            for (path, id) in &plan.updates {
                db.execute(
                    "UPDATE photos SET path=? WHERE id=?",
                    grainy_catalog::rusqlite::params![p.stored_path(path), id],
                )?;
            }
            for old in roots.clone() {
                if !p.contains_folder(&source, &old) {
                    continue;
                }
                let new = rebase(p, &old, &source, &destination)?;
                db.execute("DELETE FROM folder_roots WHERE path=?", [&old])?;
                db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", [&new])?;
                if let Some(i) = roots.iter().position(|r| *r == old) {
                    roots.remove(i);
                }
                roots.push(new);
            }
            if !roots.iter().any(|root| p.contains_folder(root, &destination)) {
                db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", [&destination])?;
                roots.push(destination.clone());
            }
            if panel_map.get("selected").is_some_and(truthy) {
                let selected = panel_map["selected"].as_str().unwrap_or("").to_owned();
                panel_map.insert("selected".into(), Value::String(rebase(p, &selected, &source, &destination)?));
            }
            let expanded = match panel_map.get("expanded") {
                Some(v) => strings(v)?,
                None => Vec::new(),
            };
            let expanded: Result<Vec<Value>> =
                expanded.iter().map(|e| Ok(Value::String(rebase(p, e, &source, &destination)?))).collect();
            panel_map.insert("expanded".into(), Value::Array(expanded?));
            watches = watches.iter().map(|w| rebase(p, w, &source, &destination)).collect::<Result<_>>()?;
            let kept: Map<String, Value> = identity_map
                .iter()
                .filter(|(k, _)| !p.contains_folder(&source, k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            *identity_map = kept;
            // Removed folders move along with their parent; a folder that receives photos is in use again.
            let moved: Vec<String> =
                removed.iter().map(|r| rebase(p, r, &source, &destination)).collect::<Result<_>>()?;
            removed = moved.into_iter().filter(|r| !p.contains_folder(r, &destination)).collect();
        }
        let values = [
            ("folder_panel", Value::Object(panel_map.clone())),
            ("watch_folders", json!(watches)),
            ("folder_identities", Value::Object(identity_map.clone())),
            ("removed_folders", json!(removed)),
        ];
        for (key, value) in values {
            db.execute(
                "INSERT OR REPLACE INTO preferences VALUES(?,?)",
                grainy_catalog::rusqlite::params![key, grainy_catalog::json::dumps(&value, false, false, false)],
            )?;
        }
        Ok(())
    })?;
    Ok(plans.iter().map(|plan| plan.updates.len()).sum())
}

/// The final folder name of a path (a convenience for callers listing scan results).
pub fn folder_name(platform: Platform, path: &str) -> String {
    name(platform, path)
}
