//! XMP 저장 / XMP 불러오기 for the selected photos and the GPS map (luma/manager.py `write_xmp` / `read_xmp`,
//! luma/extras.py `ExtraTools.map`).
//!
//! The sidecar of a photo is its own file next to it: `name.xmp`, or `name.luma-copy-{id}.xmp` for a virtual copy
//! ([`sidecar_path`]). Writing goes through `grainy_library::xmp::write_sidecar` (the standard rating, label and
//! keywords plus the explicit Grainy settings; foreign descriptions of an existing sidecar are kept) and touches
//! no other file than that sidecar. Reading brings the file's settings in as an edit (one history row
//! `XMP 불러오기` and one undo step per changed photo), its metadata into the photo's extras and its rating, label
//! and keywords into the catalogue.
//!
//! The file work runs as a job ([`write_sidecars`] and [`read_sidecars`] are plain functions: the commands, the
//! tests and the job call them); applying a read to the catalogue is [`apply_sidecars`].

use std::path::{Path, PathBuf};

use grainy_catalog::Catalog;
use grainy_library::pathx::with_suffix;
use grainy_library::xmp::{self, Sidecar};
use grainy_library::Platform;
use serde_json::{Map, Value};

use super::metadata::{changed, selected_ids, set_keywords};
use super::{always, has_photos, writable_library, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::map::{self, MapPoint};
use crate::document::Document;
use crate::panels::metadata as panel;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "metadata.write_xmp", label: "XMP 저장", enabled: can_xmp, run: write },
    Command { id: "metadata.read_xmp", label: "XMP 불러오기", enabled: can_xmp, run: read },
    Command { id: "extras.map", label: "GPS 지도", enabled: always, run: show_map },
];

/// The history label of a read (`edit_photo(p['id'], data['settings'], 'XMP 불러오기')`).
pub const READ_LABEL: &str = "XMP 불러오기";

fn can_xmp(cx: &AppContext) -> bool {
    writable_library(cx) && has_photos(cx)
}

fn failed(cx: &mut AppContext, message: &str) {
    cx.status = tr_with("처리하지 못했습니다: {0}", &[&tr(message)]);
}

/// `Path(p['path']).with_suffix(f'.luma-copy-{p["id"]}.xmp' if p['virtual_source'] is not None else '.xmp')` for a
/// catalogue row (`Catalog::photo`).
pub fn sidecar_path(photo: &Value) -> PathBuf {
    let suffix = match photo.get("virtual_source") {
        Some(Value::Null) | None => ".xmp".to_owned(),
        Some(_) => format!(".luma-copy-{}.xmp", photo["id"].as_i64().unwrap_or(0)),
    };
    PathBuf::from(with_suffix(Platform::native(), photo["path"].as_str().unwrap_or(""), &suffix))
}

/// The catalogue rows of the photos (`selected_photos`); an id that is no longer there is left out.
pub fn rows_of(catalog: &Catalog, ids: &[i64]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for id in ids {
        if let Some(row) = catalog.photo(*id).map_err(|e| e.to_string())? {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Port of the loop of `LibraryManager.write_xmp`: a sidecar for each row, the first failure stops it.
/// Returns the files written.
pub fn write_sidecars(rows: &[Value]) -> Result<Vec<PathBuf>, String> {
    let mut written = Vec::new();
    for row in rows {
        let path = sidecar_path(row);
        crate::paths::writable(&path).map_err(|e| e.to_string())?;
        xmp::write_sidecar(row, &path).map_err(|e| e.to_string())?;
        written.push(path);
    }
    Ok(written)
}

/// What a sidecar read gave for a photo: `None` when the photo has no sidecar file.
pub type Read = (i64, Option<Result<Sidecar, String>>);

/// Port of the first half of the loop of `LibraryManager.read_xmp`: each photo's sidecar read (a photo without
/// the file is skipped). The first file that cannot be read ends the list, as it ends the oracle's loop.
pub fn read_sidecars(rows: &[Value]) -> Vec<Read> {
    let mut reads = Vec::new();
    for row in rows {
        let id = row["id"].as_i64().unwrap_or(0);
        let path = sidecar_path(row);
        if !Path::new(&path).exists() {
            reads.push((id, None));
            continue;
        }
        let read = xmp::read_sidecar(&path).map_err(|e| e.to_string());
        let stop = read.is_err();
        reads.push((id, Some(read)));
        if stop {
            break;
        }
    }
    reads
}

/// Port of the second half of the loop of `LibraryManager.read_xmp`, per photo in order: the settings become an
/// edit (`edit_photo`: fitted to the file's RAW state, nothing written when they do not change), the metadata is
/// merged into the extras (`set_user_metadata`) and rating, label and keywords are stored (`catalog.update`).
/// Stops at the first failed read and returns its message; the photos before it stay changed, as in the oracle.
/// Returns how many photos had a sidecar.
pub fn apply_sidecars(doc: &mut dyn Document, reads: Vec<Read>) -> Result<usize, String> {
    let mut count = 0;
    for (id, read) in reads {
        let Some(read) = read else { continue };
        let data = read?;
        count += 1;
        if let Some(settings) = &data.settings {
            doc.edit_photos(&[id as u64], std::slice::from_ref(settings), READ_LABEL)?;
        }
        let catalog = doc.metadata_catalog().ok_or("no library is open")?;
        if let Some(meta) = &data.user_metadata {
            let Value::Object(values) = meta else {
                return Err("the metadata of a sidecar must be an object".into());
            };
            catalog.set_user_metadata(id, values).map_err(|e| e.to_string())?;
        }
        let mut fields = Map::new();
        if let Some(rating) = data.rating {
            fields.insert("rating".into(), Value::from(rating));
        }
        if let Some(label) = data.label {
            fields.insert("label".into(), Value::String(label));
        }
        if let Some(keywords) = data.keywords {
            fields.insert("keywords".into(), Value::String(keywords));
        }
        catalog.update(id, &fields).map_err(|e| e.to_string())?;
    }
    Ok(count)
}

/// `self.w.commit(); self.w.save_keywords()`: the open photo's pending edit and keywords field are stored first.
fn commit(cx: &mut AppContext) {
    cx.doc.edit_mut().commit("보정 변경");
    if let Err(message) = cx.doc.flush() {
        log::warn!("cannot store the edit journal: {message}");
    }
    if cx.doc.current().is_none() {
        return;
    }
    panel::sync(cx);
    let id = cx.doc.current_id() as i64;
    let text = cx.state::<panel::MetadataState>().keywords.clone();
    if let Some(catalog) = cx.doc.metadata_catalog() {
        if let Err(message) = set_keywords(catalog, id, &text) {
            log::warn!("cannot store the keywords: {message}");
        }
    }
}

/// XMP 저장: a sidecar for each selected photo.
fn write(cx: &mut AppContext, _: &Invocation) {
    commit(cx);
    let ids = selected_ids(cx);
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    let rows = match rows_of(catalog, &ids) {
        Ok(rows) => rows,
        Err(message) => return failed(cx, &message),
    };
    cx.jobs.spawn(
        "xmp_write",
        tr("XMP 저장"),
        move |handle| {
            handle.progress(0, 1, tr("XMP 저장"));
            write_sidecars(&rows)
        },
        |cx: &mut AppContext, result: Result<Vec<PathBuf>, String>| match result {
            Ok(_) => cx.status = tr("XMP에 별점·키워드·Grainy 보정값을 저장했습니다."),
            Err(message) => failed(cx, &message),
        },
    );
}

/// XMP 불러오기: the selected photos' sidecars are read (a job) and applied.
fn read(cx: &mut AppContext, _: &Invocation) {
    commit(cx);
    let ids = selected_ids(cx);
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    let rows = match rows_of(catalog, &ids) {
        Ok(rows) => rows,
        Err(message) => return failed(cx, &message),
    };
    cx.spawn_document_job(
        "xmp_read",
        tr("XMP 불러오기"),
        move |handle| {
            handle.progress(0, 1, tr("XMP 불러오기"));
            Ok(read_sidecars(&rows))
        },
        move |cx: &mut AppContext, result: Result<Vec<Read>, String>| {
            let reads = match result {
                Ok(reads) => reads,
                Err(message) => return failed(cx, &message),
            };
            let outcome = apply_sidecars(&mut *cx.doc, reads);
            // `reload_current`: the open photo and the list are read again.
            let ids: Vec<u64> = ids.iter().map(|id| *id as u64).collect();
            if let Err(message) = cx.doc.refresh_command_edits(&ids) {
                log::warn!("cannot read the photos again: {message}");
            }
            changed(cx);
            if let Err(message) = outcome {
                failed(cx, &message);
            }
        },
    );
}

/// 추가 도구 → GPS 지도: the geotagged photos of the list on the offline map (a job reads their rows).
fn show_map(cx: &mut AppContext, _: &Invocation) {
    let photos: Vec<(i64, String)> = cx.doc.photos().iter().map(|p| (p.id as i64, p.name.clone())).collect();
    // The Python Grainy's library is read through its reader on the window's thread (a snapshot copy, no writes).
    let Some(folder) = cx.doc.library_folder() else {
        let points = match cx.doc.metadata_catalog() {
            Some(catalog) => map::gps_points(catalog, &photos).unwrap_or_default(),
            None => Vec::new(),
        };
        cx.open_dialog(Box::new(map::MapDialog::new(points, map::World::load())));
        return;
    };
    cx.spawn_document_job(
        "gps_map",
        tr("GPS 지도"),
        move |handle| {
            handle.progress(0, 1, tr("GPS 지도"));
            let catalog = Catalog::open_reader(&folder).map_err(|e| e.to_string())?;
            let points = map::gps_points(&catalog, &photos)?;
            Ok((points, map::World::load()))
        },
        |cx: &mut AppContext, result: Result<(Vec<MapPoint>, std::sync::Arc<map::World>), String>| match result {
            Ok((points, world)) => cx.open_dialog(Box::new(map::MapDialog::new(points, world))),
            Err(message) => failed(cx, &message),
        },
    );
}
