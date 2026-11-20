//! Port of `luma/library.py` (everything but `matches`, which lives in `grainy_catalog::query`): duplicate
//! detection, guarded moves, relinking, backup restore. Sidecars are in [`crate::xmp`].
//!
//! Safety: original photos are never overwritten or deleted. A move to an existing target fails, a cross-volume
//! move is a verified copy before the source goes (see [`crate::fsops`]).

use crate::file_access::{access, Access};
use crate::fsops::{file_digest, move_dir, move_file};
use crate::pathx::{self, casefold, is_parent_of, is_root, name, norm_str, parent, resolve, same};
use crate::{profiles, Error, Result};
use grainy_catalog::folders::Platform;
use grainy_catalog::rusqlite::{params, Connection, OpenFlags};
use grainy_catalog::Catalog;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

/// `engine.RAW_EXTENSIONS`.
pub const RAW_EXTENSIONS: &[&str] = &[
    ".arw", ".cr2", ".cr3", ".nef", ".nrw", ".raf", ".rw2", ".orf", ".pef", ".dng", ".srw", ".3fr", ".iiq", ".kdc",
    ".mos",
];
/// `engine.IMAGE_EXTENSIONS` (`RAW_EXTENSIONS` and the plain formats).
pub const IMAGE_EXTENSIONS: &[&str] = &[
    ".arw", ".cr2", ".cr3", ".nef", ".nrw", ".raf", ".rw2", ".orf", ".pef", ".dng", ".srw", ".3fr", ".iiq", ".kdc",
    ".mos", ".jpg", ".jpeg", ".png", ".tif", ".tiff", ".bmp", ".webp",
];

/// Whether a lower-cased extension (with the dot) is one Grainy opens.
pub fn is_image_extension(extension: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&extension)
}

pub(crate) fn str_field<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Port of `library.file_digest`.
pub fn digest(path: &Path) -> Result<String> {
    file_digest(path).map_err(|e| Error::os(&e, path, true))
}

/// Port of `library.duplicate_groups`: groups of catalogue rows (`photos` as `Catalog::photos` gives them)
/// whose files are present and byte-identical. A path that differs only in case counts once.
pub fn duplicate_groups(platform: Platform, photos: &[Value]) -> Result<Vec<Vec<Value>>> {
    let mut sizes: Vec<(u64, Vec<&Value>)> = Vec::new();
    let mut seen = HashSet::new();
    for p in photos {
        let path = norm_str(platform, str_field(p, "path"));
        let key = casefold(&path);
        if seen.contains(&key) || access(Path::new(&path), false) != Access::Present {
            continue;
        }
        seen.insert(key);
        let size = std::fs::metadata(&path).map_err(|e| Error::os(&e, Path::new(&path), false))?.len();
        match sizes.iter_mut().find(|(s, _)| *s == size) {
            Some((_, list)) => list.push(p),
            None => sizes.push((size, vec![p])),
        }
    }
    let mut groups = Vec::new();
    for (_, candidates) in sizes {
        if candidates.len() < 2 {
            continue;
        }
        let mut hashes: Vec<(String, Vec<&Value>)> = Vec::new();
        for p in candidates {
            let hash = digest(Path::new(str_field(p, "path")))?;
            match hashes.iter_mut().find(|(h, _)| *h == hash) {
                Some((_, list)) => list.push(p),
                None => hashes.push((hash, vec![p])),
            }
        }
        groups.extend(
            hashes.into_iter().filter(|(_, rows)| rows.len() > 1).map(|(_, rows)| rows.into_iter().cloned().collect()),
        );
    }
    Ok(groups)
}

/// Port of `library.move_files`: preflight all moves, move the files, then update every row (virtual copies
/// too) that shares the physical source. Any failure moves the completed files back. Returns the number of
/// moved files.
pub fn move_files(catalog: &Catalog, mapping: &[(String, String)]) -> Result<usize> {
    let p = catalog.platform();
    let mut pairs: Vec<(String, String, String)> = Vec::new();
    let mut targets = HashSet::new();
    for (source, destination) in mapping {
        let (src, dst) = (resolve(p, source), resolve(p, destination));
        if same(p, &src, &dst) {
            continue;
        }
        if !Path::new(&src).is_file() {
            return Err(Error::value(format!("원본이 없습니다: {}", name(p, &src))));
        }
        if Path::new(&dst).exists() || targets.contains(&casefold(&dst)) {
            return Err(Error::value(format!("대상 파일이 이미 있습니다: {}", name(p, &dst))));
        }
        if !Path::new(&parent(p, &dst)).is_dir() {
            return Err(Error::value("대상 폴더가 없습니다."));
        }
        targets.insert(casefold(&dst));
        pairs.push((src, dst, p.stored_path(source)));
    }
    let mut completed: Vec<(String, String)> = Vec::new();
    let outcome = (|| -> Result<()> {
        for (src, dst, _) in &pairs {
            move_file(Path::new(src), Path::new(dst))?;
            completed.push((src.clone(), dst.clone()));
        }
        crate::tx(catalog, || {
            for (src, dst, supplied) in &pairs {
                let stored = p.stored_path(dst);
                let stored_name = name(p, &stored);
                catalog.connection().execute(
                    "UPDATE photos SET path=?,name=CASE WHEN virtual_source IS NULL THEN ? ELSE ? END WHERE path=? COLLATE NOCASE OR path=? COLLATE NOCASE",
                    params![stored, stored_name, format!("{stored_name} · 사본"), src, supplied],
                )?;
                catalog
                    .connection()
                    .execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", params![p.stored_path(&parent(p, dst))])?;
            }
            Ok(())
        })?;
        Ok(())
    })();
    if let Err(error) = outcome {
        for (src, dst) in completed.iter().rev() {
            if Path::new(dst).exists() && !Path::new(src).exists() {
                let _ = move_file(Path::new(dst), Path::new(src));
            }
        }
        return Err(error);
    }
    Ok(pairs.len())
}

/// Port of `library.relink`: points a photo (and every row with the same path) at a new file.
pub fn relink(catalog: &Catalog, photo_id: i64, new_path: &str) -> Result<()> {
    let p = catalog.platform();
    let path = resolve(p, new_path);
    let extension = pathx::stem_suffix(&name(p, &path)).1.to_lowercase();
    if !Path::new(&path).is_file() || !is_image_extension(&extension) {
        return Err(Error::value("지원하는 사진 파일을 선택해 주세요."));
    }
    let photo = catalog.photo(photo_id)?.ok_or_else(|| Error::Type("'NoneType' object is not subscriptable".into()))?;
    let old = str_field(&photo, "path").to_owned();
    catalog
        .connection()
        .execute("UPDATE photos SET path=? WHERE path=? COLLATE NOCASE", params![p.stored_path(&path), old])?;
    catalog.register_folder(&parent(p, &path), false)?;
    Ok(())
}

fn below_join(p: Platform, destination: &str, path: &str, folder: &str) -> Result<String> {
    let relative = p.below(path, folder)?;
    Ok(if relative == "." { norm_str(p, destination) } else { pathx::join(p, destination, &relative) })
}

/// Port of `library.move_folder`: moves a folder on disk and rewrites every catalogue path below it.
pub fn move_folder(catalog: &Catalog, source: &str, destination: &str) -> Result<()> {
    let p = catalog.platform();
    let (src, dst) = (resolve(p, source), resolve(p, destination));
    if !Path::new(&src).is_dir() || is_root(p, &src) {
        return Err(Error::value("사진 폴더를 선택해 주세요."));
    }
    if Path::new(&dst).exists() || same(p, &dst, &src) || is_parent_of(p, &src, &dst) {
        return Err(Error::value("폴더 안쪽이나 기존 폴더로 이동할 수 없습니다."));
    }
    let mut photos: Vec<(String, i64)> = Vec::new();
    let mut roots: Vec<(String, String)> = Vec::new();
    for row in catalog.photos()? {
        let id = row["id"].as_i64().unwrap_or(0);
        if let Ok(new) = below_join(p, &dst, &resolve(p, str_field(&row, "path")), &src) {
            photos.push((p.stored_path(&new), id));
        }
    }
    for root in catalog.folder_roots()? {
        if let Ok(new) = below_join(p, &dst, &resolve(p, &root), &src) {
            roots.push((root, p.stored_path(&new)));
        }
    }
    let mut removed = Vec::new();
    for r in catalog.removed_folders()? {
        removed.push(if p.contains_folder(&src, &r) { p.stored_path(&below_join(p, &dst, &r, &src)?) } else { r });
    }
    move_dir(Path::new(&src), Path::new(&dst))?;
    let outcome = crate::tx(catalog, || {
        let db = catalog.connection();
        for (new, id) in &photos {
            db.execute("UPDATE photos SET path=? WHERE id=?", params![new, id])?;
        }
        for (old, new) in &roots {
            db.execute("DELETE FROM folder_roots WHERE path=?", params![old])?;
            db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", params![new])?;
        }
        db.execute(
            "INSERT OR REPLACE INTO preferences VALUES(?,?)",
            params!["removed_folders", grainy_catalog::json::dumps(&json!(removed), false, false, false)],
        )?;
        Ok(())
    });
    if let Err(error) = outcome {
        let _ = move_dir(Path::new(&dst), Path::new(&src));
        return Err(error);
    }
    Ok(())
}

/// Port of `library.relink_folder`: `relink_plan` then `apply_relink`.
pub fn relink_folder(catalog: &Catalog, source: &str, destination: &str) -> Result<usize> {
    let plan = crate::folder_locations::relink_plan(catalog, source, destination)?;
    crate::folder_locations::apply_relink(catalog, &plan)
}

/// Port of `library.restore_backup`: replaces the whole catalogue by a backup after checking it, keeping a
/// `before-restore-<time>.sqlite` copy of the current one first.
pub fn restore_backup(catalog: &Catalog, path: &Path) -> Result<()> {
    restore_backup_at(catalog, path, &chrono::Local::now().format("%Y%m%d-%H%M%S").to_string())
}

/// [`restore_backup`] with the time stamp of the safety copy given (`%Y%m%d-%H%M%S`).
pub fn restore_backup_at(catalog: &Catalog, path: &Path, stamp: &str) -> Result<()> {
    let p = catalog.platform();
    let source = resolve(p, &pathx::text(path));
    let current = resolve(p, &pathx::text(&catalog.directory().join("catalog.sqlite")));
    if same(p, &source, &current) {
        return Err(Error::value("현재 카탈로그와 같은 파일입니다."));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let db = Connection::open_with_flags(&source, flags)?;
    db.execute_batch("BEGIN")?;
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(Error::value("백업 파일이 손상되었습니다."));
    }
    let tables: HashSet<String> = {
        let mut stmt = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<_, _>>()?;
        rows
    };
    if !["photos", "history", "preferences"].iter().all(|t| tables.contains(*t)) {
        return Err(Error::value("Luma 카탈로그 백업이 아닙니다."));
    }
    profiles::verify_references(&db, &mut || Ok(()))?;
    catalog.backup(catalog.directory().join("backups").join(format!("before-restore-{stamp}.sqlite")))?;
    catalog.restore_from(&db)?;
    Ok(())
}
