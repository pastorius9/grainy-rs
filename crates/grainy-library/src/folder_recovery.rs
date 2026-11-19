//! Port of `luma/folder_recovery.py`: recover pre-existing missing folders from locally stored photo evidence.
//!
//! The search is limited to siblings of the highest missing directory. A folder name is never evidence.
//! Ambiguous, incomplete and already-catalogued matches are left untouched. Files are only read; edits are never
//! recalculated or replaced here.
//!
//! Comparing small image descriptors needs a decoder and a resampler; they are injected through [`Thumbnailer`]
//! (the oracle uses Pillow: a decoded image resized to 64 x 64 with Lanczos).

use crate::folder_locations::{directory_identity, join_below, Identity, Plan};
use crate::npx::{gaussian_blur5, mean_f32, quantile_f32, std_f32};
use crate::pathx::{casefold, name, parent};
use crate::{profiles, Cancel, Result};
use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use grainy_core::Settings;
use ndarray::Array3;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// The side of the descriptor image.
pub const DESCRIPTOR_SIZE: usize = 64;

/// A decoded picture reduced to 64 x 64 RGB (`image.convert('RGB').resize((64, 64), LANCZOS)`) together with the
/// full size of the picture it came from (the descriptor keeps its aspect ratio).
#[derive(Clone, Debug)]
pub struct SmallRgb {
    /// 64 x 64 x 3 samples.
    pub pixels: Array3<u8>,
    /// Width of the original picture.
    pub width: u32,
    /// Height of the original picture.
    pub height: u32,
}

/// Decoding and resampling supplied by the application (Pillow in the oracle).
pub trait Thumbnailer {
    /// Decodes saved preview JPEG bytes; `None` when they cannot be read or the picture is larger than 2048
    /// pixels on a side (the oracle's `read_evidence` refuses those).
    fn evidence_image(&self, jpeg: &[u8]) -> Option<SmallRgb>;
    /// The oracle's `candidate_descriptor` image of `path`: with `rendered` the photo developed with `settings`
    /// and converted to `color_space` (320 pixel long side), otherwise its embedded or plain thumbnail.
    fn candidate_image(&self, path: &Path, settings: &Settings, rendered: bool, color_space: &str) -> Result<SmallRgb>;
}

/// `folder_recovery.descriptor`: the 64 x 64 image scaled to 0..1 and the aspect ratio.
#[derive(Clone, Debug)]
pub struct Descriptor {
    /// 64 x 64 x 3 float32 samples in 0..1.
    pub value: Array3<f32>,
    /// `width / height` of the original.
    pub ratio: f64,
}

/// Port of `folder_recovery.descriptor` (the resize is done by the [`Thumbnailer`]).
pub fn descriptor(image: &SmallRgb) -> Descriptor {
    Descriptor {
        value: image.pixels.mapv(|v| f32::from(v) / 255.0),
        ratio: f64::from(image.width) / f64::from(image.height),
    }
}

fn as_slice(a: &Array3<f32>) -> Vec<f32> {
    a.iter().copied().collect()
}

fn diff_abs(a: &Array3<f32>, axis: usize) -> Vec<f32> {
    let (h, w, c) = a.dim();
    let mut out = Vec::new();
    if axis == 0 {
        for y in 0..h - 1 {
            for x in 0..w {
                for ch in 0..c {
                    out.push((a[[y + 1, x, ch]] - a[[y, x, ch]]).abs());
                }
            }
        }
    } else {
        for y in 0..h {
            for x in 0..w - 1 {
                for ch in 0..c {
                    out.push((a[[y, x + 1, ch]] - a[[y, x, ch]]).abs());
                }
            }
        }
    }
    out
}

/// Port of `folder_recovery.compare`: do two descriptors show the same picture? Flat frames and
/// repetitive-looking scenes cannot identify a folder; a small low-pass removes JPEG/downsampling ringing, and
/// spatial layout and colour still have to agree closely.
pub fn compare(a: &Descriptor, b: &Descriptor) -> bool {
    let (x, ratio) = (&a.value, a.ratio);
    let (y, other) = (&b.value, b.ratio);
    if (ratio / other - 1.0).abs() > 0.015 {
        return false;
    }
    if std_f32(&as_slice(x)) < 0.055_f32 {
        return false;
    }
    if mean_f32(&diff_abs(x, 0)) + mean_f32(&diff_abs(x, 1)) < 0.008_f32 {
        return false;
    }
    let (bx, by) = (gaussian_blur5(x, 0.8), gaussian_blur5(y, 0.8));
    let error: Vec<f32> = bx.iter().zip(by.iter()).map(|(p, q)| (p - q).abs()).collect();
    mean_f32(&error) < 0.016_f32 && quantile_f32(&error, 0.99) < 0.09_f32
}

/// The four numbers `compare` thresholds: the standard deviation of `a`, the sum of its mean absolute gradients,
/// and the mean and 99th percentile of the absolute difference of the blurred pictures.
pub fn compare_stats(a: &Descriptor, b: &Descriptor) -> [f32; 4] {
    let x = &a.value;
    let (bx, by) = (gaussian_blur5(x, 0.8), gaussian_blur5(&b.value, 0.8));
    let error: Vec<f32> = bx.iter().zip(by.iter()).map(|(p, q)| (p - q).abs()).collect();
    [
        std_f32(&as_slice(x)),
        mean_f32(&diff_abs(x, 0)) + mean_f32(&diff_abs(x, 1)),
        mean_f32(&error),
        quantile_f32(&error, 0.99),
    ]
}

/// `np.abs(a - b).mean()` of two descriptors' pictures.
pub fn mean_difference(a: &Array3<f32>, b: &Array3<f32>) -> f32 {
    let d: Vec<f32> = a.iter().zip(b.iter()).map(|(p, q)| (p - q).abs()).collect();
    mean_f32(&d)
}

/// Port of `folder_recovery.missing_directory`: true when the path is not a directory because it is gone;
/// permission and transient connection errors are not renames.
pub fn missing_directory(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => !meta.is_dir(),
        Err(error) => matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory),
    }
}

/// One catalogue row as recovery reads it.
#[derive(Clone, Debug)]
pub struct Row {
    /// Photo id.
    pub id: i64,
    /// Stored path.
    pub path: String,
    /// Raw metadata JSON.
    pub metadata: String,
    /// Import time stamp (ISO 8601).
    pub imported: String,
    /// Source photo of a virtual copy.
    pub virtual_source: Option<i64>,
    /// Preview token of the saved preview, if any.
    pub token: Option<String>,
    /// Normalized settings.
    pub settings: Settings,
    fingerprint: Option<serde_json::Map<String, Value>>,
}

/// What `read_evidence` found in a saved preview.
#[derive(Clone, Debug)]
pub struct Evidence {
    /// The descriptor of the saved preview.
    pub value: Descriptor,
    /// A rendered preview (with a stamp), not a legacy unedited one.
    pub rendered: bool,
    /// The stamp JSON of a rendered preview.
    pub stamp: Option<Value>,
}

fn same_token(stamp: Option<&Value>, token: Option<&str>) -> bool {
    match (stamp, token) {
        (None | Some(Value::Null), None) => true,
        (Some(Value::String(s)), Some(t)) => s == t,
        _ => false,
    }
}

/// Port of `folder_recovery.read_evidence`: the descriptor of a photo's saved preview, `None` when there is none
/// or it is stale.
pub fn read_evidence(directory: &Path, row: &Row, thumbs: &dyn Thumbnailer) -> Option<Evidence> {
    let (jpeg, stamp) = grainy_catalog::preview::files(directory, row.id);
    let meta_file = std::fs::metadata(&jpeg).ok()?;
    if !meta_file.is_file() || meta_file.len() > 4 * 1024 * 1024 {
        return None;
    }
    let payload = std::fs::read(&jpeg).ok()?;
    let mut meta: Option<Value> = None;
    if stamp.exists() {
        if std::fs::metadata(&stamp).ok()?.len() > 8192 {
            return None;
        }
        let text = std::fs::read_to_string(&stamp).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        let version_ok = value.get("version").and_then(Value::as_f64) == Some(grainy_catalog::preview::VERSION as f64);
        if !version_ok
            || value.get("path").and_then(Value::as_str) != Some(row.path.as_str())
            || !same_token(value.get("token"), row.token.as_deref())
        {
            return None;
        }
        let digest = format!("{:x}", Sha256::digest(&payload));
        if value.get("sha256").and_then(Value::as_str) != Some(digest.as_str()) {
            return None;
        }
        meta = Some(value);
    } else {
        // Legacy imports stored an unedited JPEG. Avoid stale IDs after a removed record was replaced with a more
        // recently imported photo.
        let modified = meta_file.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
        let imported = parse_iso(&row.imported)?;
        if modified + 2.0 < imported {
            return None;
        }
    }
    let image = thumbs.evidence_image(&payload)?;
    Some(Evidence { value: descriptor(&image), rendered: meta.is_some(), stamp: meta })
}

/// `datetime.fromisoformat(text).timestamp()` for the forms the catalogue writes.
fn parse_iso(text: &str) -> Option<f64> {
    use chrono::TimeZone;
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(t.timestamp() as f64 + f64::from(t.timestamp_subsec_nanos()) * 1e-9);
    }
    let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f"))
        .ok()?;
    let local = chrono::Local.from_local_datetime(&naive).single()?;
    Some(local.timestamp() as f64 + f64::from(local.timestamp_subsec_nanos()) * 1e-9)
}

/// Size and modification time (ns) of a file, as `os.stat` reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    /// Size in bytes.
    pub size: u64,
    /// Modification time in nanoseconds since the epoch.
    pub mtime_ns: u128,
}

fn mtime_ns(meta: &std::fs::Metadata) -> Option<u128> {
    Some(meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos())
}

/// A recovered folder, with the evidence that allows it and the guards [`validate_recovery`] checks again.
#[derive(Clone, Debug)]
pub struct RecoveryPlan {
    /// The paths to rewrite (`source`, `destination`, `updates`, `found`, `missing`).
    pub plan: Plan,
    /// Distinct physical photos in the folder.
    pub physical: usize,
    /// `file_stats` or `saved_previews`.
    pub proof: String,
    /// Photos whose previews matched.
    pub anchors: Vec<i64>,
    /// A target path is already catalogued elsewhere.
    pub collision: bool,
    /// Photo id (as text) to the path it had.
    pub expected_paths: BTreeMap<String, String>,
    /// Every target file with the stat seen.
    pub guards: Vec<(String, FileStat)>,
    /// Identity of the destination folder.
    pub identity: Option<Identity>,
    /// Modification time of the destination's parent folder (ns).
    pub parent_mtime_ns: u128,
}

/// A folder that could not be recovered, with the reason (`ambiguous`, `already_cataloged`,
/// `insufficient_evidence`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unresolved {
    /// The missing folder.
    pub source: String,
    /// Why.
    pub reason: String,
}

/// What [`recover_folders`] found.
#[derive(Clone, Debug, Default)]
pub struct Recovery {
    /// Folders that can be relinked.
    pub plans: Vec<RecoveryPlan>,
    /// Folders left alone.
    pub unresolved: Vec<Unresolved>,
    /// Number of missing top folders looked at.
    pub total: usize,
    /// The search stopped early (time, entry limit, cancel, a changing folder).
    pub incomplete: bool,
    /// The search was cancelled.
    pub cancelled: bool,
}

struct Incomplete;

fn monotonic_seconds() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// Port of `folder_recovery.recover_folders` (default 120 second budget, real clock).
pub fn recover_folders(
    directory: &Path,
    cancel: Option<&Cancel>,
    sources: Option<&[String]>,
    thumbs: &dyn Thumbnailer,
) -> Result<Recovery> {
    recover_folders_with(directory, cancel, sources, 120.0, thumbs, &monotonic_seconds)
}

/// Port of `folder_recovery.recover_folders(directory, cancel, sources=, timeout=)` with the clock given.
pub fn recover_folders_with(
    directory: &Path,
    cancel: Option<&Cancel>,
    sources: Option<&[String]>,
    timeout: f64,
    thumbs: &dyn Thumbnailer,
    clock: &dyn Fn() -> f64,
) -> Result<Recovery> {
    let cancel = cancel.cloned().unwrap_or_default();
    let deadline = clock() + timeout;
    let catalog = Catalog::open_reader(directory)?;
    let platform = catalog.platform();
    let mut rows: Vec<Row> = Vec::new();
    {
        let mut stmt = catalog.connection().prepare(
            "SELECT p.id,p.path,p.metadata,p.edits,p.imported,p.virtual_source,s.token FROM photos p \
             LEFT JOIN preview_state s ON s.photo_id=p.id",
        )?;
        let mut cursor = stmt.query([])?;
        while let Some(r) = cursor.next()? {
            let edits: String = r.get(3)?;
            let settings = grainy_catalog::settings(&profiles::loads(catalog.connection(), &edits)?)?;
            rows.push(Row {
                id: r.get(0)?,
                path: r.get(1)?,
                metadata: r.get(2)?,
                imported: r.get(4)?,
                virtual_source: r.get(5)?,
                token: r.get(6)?,
                settings,
                fingerprint: None,
            });
        }
    }
    drop(catalog);
    let mut search = Search {
        platform,
        directory,
        cancel: &cancel,
        clock,
        deadline,
        thumbs,
        parent_state: HashMap::new(),
        evidence_cache: HashMap::new(),
        stat_cache: HashMap::new(),
        list_cache: HashMap::new(),
        parent_guards: HashMap::new(),
    };
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut up = parent(platform, &row.path);
        if !search.missing(&up) {
            continue;
        }
        loop {
            let above = parent(platform, &up);
            if above != up && search.missing(&above) {
                up = above;
            } else {
                break;
            }
        }
        if parent(platform, &up) == up {
            continue;
        }
        if let Some(list) = sources.filter(|s| !s.is_empty()) {
            if !list.iter().any(|p| platform.contains_folder(p, &up) || platform.contains_folder(&up, p)) {
                continue;
            }
        }
        match groups.iter_mut().find(|(k, _)| *k == up) {
            Some((_, members)) => members.push(index),
            None => groups.push((up, vec![index])),
        }
    }
    let mut plans: Vec<RecoveryPlan> = Vec::new();
    let mut unresolved: Vec<Unresolved> = Vec::new();
    let mut incomplete = false;
    let total = groups.len();
    let outcome = search.run_groups(&groups, &mut rows, &mut plans, &mut unresolved);
    if outcome.is_err() {
        incomplete = true;
    }
    // One destination must not silently collapse two old source folders.
    let mut by_destination: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, plan) in plans.iter().enumerate() {
        let key = platform.path_key(&plan.plan.destination);
        match by_destination.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(i),
            None => by_destination.push((key, vec![i])),
        }
    }
    let mut drop_indexes: Vec<usize> = Vec::new();
    for (_, group) in &by_destination {
        if group.len() > 1 {
            for i in group {
                drop_indexes.push(*i);
                unresolved.push(Unresolved { source: plans[*i].plan.source.clone(), reason: "ambiguous".into() });
            }
        }
    }
    let plans: Vec<RecoveryPlan> =
        plans.into_iter().enumerate().filter(|(i, _)| !drop_indexes.contains(i)).map(|(_, p)| p).collect();
    Ok(Recovery {
        plans: if cancel.is_set() { Vec::new() } else { plans },
        unresolved,
        total,
        incomplete,
        cancelled: cancel.is_set(),
    })
}

struct Search<'a> {
    platform: Platform,
    directory: &'a Path,
    cancel: &'a Cancel,
    clock: &'a dyn Fn() -> f64,
    deadline: f64,
    thumbs: &'a dyn Thumbnailer,
    parent_state: HashMap<String, bool>,
    evidence_cache: HashMap<i64, Option<Evidence>>,
    stat_cache: HashMap<String, Option<FileStat>>,
    list_cache: HashMap<String, Vec<String>>,
    parent_guards: HashMap<String, u128>,
}

impl Search<'_> {
    fn check(&self) -> std::result::Result<(), Incomplete> {
        if self.cancel.is_set() || (self.clock)() > self.deadline {
            Err(Incomplete)
        } else {
            Ok(())
        }
    }

    fn missing(&mut self, path: &str) -> bool {
        let key = self.platform.path_key(path);
        *self.parent_state.entry(key).or_insert_with(|| missing_directory(Path::new(path)))
    }

    fn siblings(&mut self, parent_path: &str) -> std::result::Result<Vec<String>, Incomplete> {
        let key = self.platform.path_key(parent_path);
        if let Some(found) = self.list_cache.get(&key) {
            return Ok(found.clone());
        }
        let mut result = Vec::new();
        let before = std::fs::metadata(parent_path).ok().and_then(|m| mtime_ns(&m)).ok_or(Incomplete)?;
        let entries = std::fs::read_dir(parent_path).map_err(|_| Incomplete)?;
        for (index, entry) in entries.enumerate() {
            self.check()?;
            if index >= 10000 {
                return Err(Incomplete);
            }
            let entry = entry.map_err(|_| Incomplete)?;
            let kind = entry.file_type().map_err(|_| Incomplete)?;
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            result.push(crate::pathx::norm_str(self.platform, &crate::pathx::text(&entry.path())));
        }
        let after = std::fs::metadata(parent_path).ok().and_then(|m| mtime_ns(&m)).ok_or(Incomplete)?;
        if after != before {
            return Err(Incomplete);
        }
        self.parent_guards.insert(key.clone(), before);
        self.list_cache.insert(key, result.clone());
        Ok(result)
    }

    fn file_stat(&mut self, path: &str) -> std::result::Result<Option<FileStat>, Incomplete> {
        self.check()?;
        let key = self.platform.path_key(path);
        if let Some(found) = self.stat_cache.get(&key) {
            return Ok(found.clone());
        }
        let value = match std::fs::metadata(path) {
            Ok(meta) => {
                if meta.is_file() {
                    Some(FileStat { size: meta.len(), mtime_ns: mtime_ns(&meta).ok_or(Incomplete)? })
                } else {
                    None
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => None,
            Err(_) => return Err(Incomplete),
        };
        self.stat_cache.insert(key, value.clone());
        Ok(value)
    }

    fn run_groups(
        &mut self,
        groups: &[(String, Vec<usize>)],
        rows: &mut [Row],
        plans: &mut Vec<RecoveryPlan>,
        unresolved: &mut Vec<Unresolved>,
    ) -> std::result::Result<(), Incomplete> {
        let p = self.platform;
        for (source, members) in groups {
            self.check()?;
            // Virtual copies provide no additional independent evidence.
            let mut order: Vec<usize> = members.clone();
            order.sort_by_key(|i| (rows[*i].virtual_source.is_some(), rows[*i].id));
            let mut unique: Vec<(String, usize)> = Vec::new();
            for i in order {
                let key = p.path_key(&rows[i].path);
                if !unique.iter().any(|(k, _)| *k == key) {
                    unique.push((key, i));
                }
            }
            let physical: Vec<usize> = unique.iter().map(|(_, i)| *i).collect();
            let mut anchors: Vec<usize> = Vec::new();
            for &i in &physical {
                let info: Option<serde_json::Map<String, Value>> =
                    match serde_json::from_str::<Value>(if rows[i].metadata.is_empty() {
                        "{}"
                    } else {
                        &rows[i].metadata
                    }) {
                        Ok(Value::Object(m)) => Some(m),
                        _ => None,
                    };
                let mut fingerprint = info.as_ref().and_then(|m| m.get("file_fingerprint")).cloned();
                if !fingerprint.as_ref().is_some_and(grainy_core::settings::truthy) {
                    fingerprint = None;
                    let stamp_path = self.directory.join("thumbnails").join(format!("{}.json", rows[i].id));
                    if let Ok(text) = std::fs::read_to_string(&stamp_path) {
                        if let Ok(stamp) = serde_json::from_str::<Value>(&text) {
                            if stamp.get("path").and_then(Value::as_str) == Some(rows[i].path.as_str())
                                && same_token(stamp.get("token"), rows[i].token.as_deref())
                            {
                                fingerprint = stamp.get("source").cloned();
                            }
                        }
                    }
                }
                rows[i].fingerprint = match fingerprint {
                    Some(Value::Object(m)) if !m.is_empty() => Some(m),
                    _ => None,
                };
                if rows[i].fingerprint.is_some() {
                    anchors.push(i);
                }
            }
            // Check discriminating recorded files first; names order candidates only for responsiveness, never
            // as an acceptance criterion.
            let mut ordered: Vec<usize> = anchors.clone();
            ordered.extend(physical.iter().copied().filter(|i| !anchors.contains(i)));
            let source_parent = parent(p, source);
            let mut options = self.siblings(&source_parent)?;
            let source_name = casefold(&name(p, source));
            options.sort_by(|a, b| {
                let ka = (casefold(&name(p, a)).ends_with(&source_name), name(p, a));
                let kb = (casefold(&name(p, b)).ends_with(&source_name), name(p, b));
                kb.cmp(&ka)
            });
            let mut matches: Vec<RecoveryPlan> = Vec::new();
            for destination in &options {
                self.check()?;
                if p.path_key(destination) == p.path_key(source) {
                    continue;
                }
                let mut stats: Vec<(String, FileStat)> = Vec::new();
                let mut exact: HashSet<(u64, u128)> = HashSet::new();
                let mut valid = true;
                for &i in &ordered {
                    let Ok(target) = join_below(p, destination, &rows[i].path, source) else {
                        valid = false;
                        break;
                    };
                    let Some(info) = self.file_stat(&target)? else {
                        valid = false;
                        break;
                    };
                    if let Some(fp) = &rows[i].fingerprint {
                        let size_ok = fp.get("size").and_then(Value::as_u64) == Some(info.size);
                        let time_ok = fp.get("mtime_ns").and_then(Value::as_u64).map(u128::from) == Some(info.mtime_ns);
                        if !size_ok || !time_ok {
                            valid = false;
                            break;
                        }
                        exact.insert((info.size, info.mtime_ns));
                    }
                    stats.push((target, info));
                }
                if !valid {
                    continue;
                }
                let mut proof = "file_stats";
                let mut visual: Vec<(i64, Descriptor)> = Vec::new();
                // Require three independent exact file signatures, with every other known signature matching
                // and every catalogue file present. Older catalogues need three distinct matching saved previews.
                if exact.len() < 3 {
                    proof = "saved_previews";
                    let mut unmatched = 0;
                    for &i in &ordered {
                        self.check()?;
                        if !self.evidence_cache.contains_key(&rows[i].id) {
                            let evidence = read_evidence(self.directory, &rows[i], self.thumbs);
                            self.evidence_cache.insert(rows[i].id, evidence);
                        }
                        let Some(evidence) = self.evidence_cache.get(&rows[i].id).cloned().flatten() else { continue };
                        if !compare(&evidence.value, &evidence.value) {
                            continue;
                        }
                        if visual.iter().any(|(_, v)| mean_difference(&evidence.value.value, &v.value) < 0.035_f32) {
                            continue;
                        }
                        let Ok(target) = join_below(p, destination, &rows[i].path, source) else {
                            valid = false;
                            break;
                        };
                        let color_space = evidence
                            .stamp
                            .as_ref()
                            .and_then(|s| s.get("color_space"))
                            .and_then(Value::as_str)
                            .unwrap_or("sRGB")
                            .to_owned();
                        let current = match self.thumbs.candidate_image(
                            Path::new(&target),
                            &rows[i].settings,
                            evidence.rendered,
                            &color_space,
                        ) {
                            Ok(image) => descriptor(&image),
                            Err(_) => {
                                valid = false;
                                break;
                            }
                        };
                        if !compare(&evidence.value, &current) {
                            // A saved preview may predate an edit/decoder change. It supplies no positive
                            // evidence; require three other distinct matches and stop after two misses.
                            unmatched += 1;
                            if unmatched > 2 {
                                valid = false;
                                break;
                            }
                            continue;
                        }
                        visual.push((rows[i].id, evidence.value.clone()));
                        if visual.len() >= 3 {
                            break;
                        }
                    }
                    if !valid || visual.len() < 3 {
                        continue;
                    }
                }
                let mut updates = Vec::new();
                for &i in members {
                    if let Ok(target) = join_below(p, destination, &rows[i].path, source) {
                        updates.push((target, rows[i].id));
                    }
                }
                let outside: HashSet<String> =
                    rows.iter().filter(|r| !p.in_folder(&r.path, source, true)).map(|r| p.path_key(&r.path)).collect();
                let collision = updates.iter().any(|(target, _)| outside.contains(&p.path_key(target)));
                let expected_paths: BTreeMap<String, String> =
                    members.iter().map(|&i| (rows[i].id.to_string(), rows[i].path.clone())).collect();
                matches.push(RecoveryPlan {
                    plan: Plan {
                        source: source.clone(),
                        destination: destination.clone(),
                        found: updates.len(),
                        updates,
                        missing: 0,
                    },
                    physical: physical.len(),
                    proof: proof.into(),
                    anchors: visual.iter().map(|(id, _)| *id).collect(),
                    collision,
                    expected_paths,
                    guards: stats,
                    identity: directory_identity(Path::new(destination)),
                    parent_mtime_ns: self.parent_guards.get(&p.path_key(&source_parent)).copied().unwrap_or(0),
                });
            }
            if matches.len() == 1 && !matches[0].collision {
                plans.push(matches.remove(0));
            } else {
                let reason = if matches.len() > 1 {
                    "ambiguous"
                } else if !matches.is_empty() {
                    "already_cataloged"
                } else {
                    "insufficient_evidence"
                };
                unresolved.push(Unresolved { source: source.clone(), reason: reason.into() });
            }
        }
        Ok(())
    }
}

/// Port of `folder_recovery.validate_recovery`: rejects stale worker results before any catalogue mutation.
pub fn validate_recovery(catalog: &Catalog, plans: &[RecoveryPlan]) -> Result<bool> {
    let p = catalog.platform();
    let rows = catalog.rows("SELECT id,path FROM photos", [])?;
    let path_of = |r: &Value| r["path"].as_str().unwrap_or("").to_owned();
    for plan in plans {
        let base = &plan.plan;
        if !missing_directory(Path::new(&base.source))
            || directory_identity(Path::new(&base.destination)) != plan.identity
        {
            return Ok(false);
        }
        let up = parent(p, &base.destination);
        match std::fs::metadata(&up).ok().and_then(|m| mtime_ns(&m)) {
            Some(t) if t == plan.parent_mtime_ns => {}
            _ => return Ok(false),
        }
        let current: BTreeMap<String, String> = rows
            .iter()
            .filter(|r| p.in_folder(&path_of(r), &base.source, true))
            .map(|r| (r["id"].as_i64().unwrap_or(0).to_string(), path_of(r)))
            .collect();
        if current != plan.expected_paths {
            return Ok(false);
        }
        let outside: HashSet<String> = rows
            .iter()
            .filter(|r| !p.in_folder(&path_of(r), &base.source, true))
            .map(|r| p.path_key(&path_of(r)))
            .collect();
        if base.updates.iter().any(|(path, _)| outside.contains(&p.path_key(path))) {
            return Ok(false);
        }
        for (path, before) in &plan.guards {
            let Ok(after) = std::fs::metadata(path) else { return Ok(false) };
            if after.len() != before.size || mtime_ns(&after) != Some(before.mtime_ns) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
