//! Port of `luma/import_copy.py` (everything but the Qt `dialog`): import by copying. Photos are copied from a card
//! or another folder into a destination (optionally in dated subfolders, renamed, with a verified second backup
//! copy); the application then hands the copies to the normal import. Sources are never changed or removed, and
//! existing files are never overwritten.

use crate::fsops::{copy_stat, digest_bytes};
use crate::pathx::{self, name, norm_str, stem_suffix, with_name, with_suffix};
use crate::pyformat::{self, strptime_exif, Arg};
use crate::{Cancel, Error, Result};
use chrono::{Datelike, NaiveDateTime, Timelike};
use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use grainy_i18n::{tr, tr_with};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// `import_copy.SUBFOLDERS`.
pub const SUBFOLDERS: [&str; 3] = ["none", "year/date", "date"];
/// `import_copy.FIELDS`: the names a file-name template may use.
pub const FIELDS: [&str; 5] = ["stem", "date", "time", "n", "camera"];

/// Port of `import_copy.capture_time`: the EXIF capture time (`metadata['date']`), else the file's modification
/// time in local time.
pub fn capture_time(path: &str, metadata: Option<&Map<String, Value>>) -> Result<NaiveDateTime> {
    if let Some(value) = metadata.and_then(|m| m.get("date")).filter(|v| grainy_core::settings::truthy(v)) {
        let text: String = pyformat::py_str(value).chars().take(19).collect();
        if let Some(parsed) = strptime_exif(&text) {
            return Ok(parsed);
        }
    }
    let meta = fs::metadata(path).map_err(|e| Error::os(&e, Path::new(path), false))?;
    let modified = meta.modified().map_err(|e| Error::os(&e, Path::new(path), false))?;
    Ok(grainy_core::local_time::wall_time_of(modified))
}

/// Port of `import_copy.check_template`: `ValueError` unless the name template uses only known fields and gives
/// a valid file name.
pub fn check_template(template: &str) -> Result<()> {
    let fields = pyformat::field_names(template)?;
    let unknown = fields.iter().any(|f| !FIELDS.contains(&f.as_str()));
    if template.trim().is_empty() || unknown {
        return Err(Error::value(tr("이름 규칙에는 {stem}, {date}, {time}, {n}, {camera}만 쓸 수 있습니다.")));
    }
    let name = pyformat::format(
        template,
        &[
            ("stem", Arg::Str("photo".into())),
            ("date", Arg::Str("20260925".into())),
            ("time", Arg::Str("120000".into())),
            ("n", Arg::Int(1)),
            ("camera", Arg::Str("Camera".into())),
        ],
    )?;
    let bad = name.chars().any(|c| "\\/:*?\"<>|".contains(c));
    if bad || name.trim_matches([' ', '.']) != name || name.is_empty() {
        return Err(Error::value(tr("이름 규칙이 올바른 파일 이름을 만들지 않습니다.")));
    }
    Ok(())
}

/// Port of the tuple `import_copy.duplicate_key` returns: (lower-case file name, capture time text) or
/// (file name, size in MB rounded to 2 places).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DuplicateKey {
    /// File name and `str(metadata['date'])`.
    Dated(String, String),
    /// File name and the size in MB as `round(float(size_mb), 2)` (bit pattern, zero normalised).
    Sized(String, u64),
}

impl DuplicateKey {
    /// The rounded size of a `Sized` key.
    pub fn size_mb(&self) -> Option<f64> {
        match self {
            DuplicateKey::Sized(_, bits) => Some(f64::from_bits(*bits)),
            DuplicateKey::Dated(..) => None,
        }
    }
}

fn round2(value: f64) -> f64 {
    format!("{value:.2}").parse::<f64>().unwrap_or(value)
}

/// Port of `import_copy.duplicate_key`: a suspected duplicate has the same original file name and capture time
/// (else file size). Errors as Python raises them for a non-numeric `size_mb`.
pub fn duplicate_key(platform: Platform, path: &str, metadata: &Map<String, Value>) -> Result<DuplicateKey> {
    let source = match metadata.get("filename") {
        Some(v) if grainy_core::settings::truthy(v) => pyformat::py_str(v),
        _ => path.to_owned(),
    };
    let file_name = name(platform, &source).to_lowercase();
    match metadata.get("date") {
        Some(date) if grainy_core::settings::truthy(date) => Ok(DuplicateKey::Dated(file_name, pyformat::py_str(date))),
        _ => {
            let size = match metadata.get("size_mb") {
                Some(v) => pyformat::py_float(v)?,
                None => 0.0,
            };
            let rounded = round2(size);
            Ok(DuplicateKey::Sized(file_name, if rounded == 0.0 { 0.0f64.to_bits() } else { rounded.to_bits() }))
        }
    }
}

/// `plan()` result: `(source, relative target path)` in capture order and the suspected duplicates skipped.
pub type Planned = (Vec<(String, String)>, Vec<String>);

/// Port of `import_copy.plan`: `[(source, relative target path)]` in capture order; suspected duplicates of
/// `known` keys are skipped and returned separately. `read` is the metadata reader (`engine.read_metadata`: the
/// application passes the real one). `n` counts every file, skipped ones included, as the oracle does.
pub fn plan(
    platform: Platform,
    sources: &[String],
    subfolders: &str,
    template: &str,
    known: &HashSet<DuplicateKey>,
    read: &mut dyn FnMut(&str) -> Result<Map<String, Value>>,
) -> Result<Planned> {
    check_template(template)?;
    if !SUBFOLDERS.contains(&subfolders) {
        return Err(Error::value(subfolders));
    }
    let mut items: Vec<(NaiveDateTime, String, Map<String, Value>)> = Vec::new();
    for source in sources {
        let mut metadata = read(source)?;
        let size = fs::metadata(source).map_err(|e| Error::os(&e, Path::new(source), false))?.len();
        metadata.insert("filename".into(), Value::String(name(platform, source)));
        metadata.insert("size_mb".into(), serde_json::json!(round2(size as f64 / (1024.0 * 1024.0))));
        items.push((capture_time(source, Some(&metadata))?, source.clone(), metadata));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut result = Vec::new();
    let mut skipped = Vec::new();
    for (n, (when, source, metadata)) in items.iter().enumerate() {
        if known.contains(&duplicate_key(platform, source, metadata)?) {
            skipped.push(source.clone());
            continue;
        }
        let camera_text = match metadata.get("camera") {
            Some(v) if grainy_core::settings::truthy(v) => pyformat::py_str(v),
            _ => "camera".to_owned(),
        };
        let camera: String = camera_text.chars().filter(|c| !"\\/:*?\"<>|".contains(*c)).collect();
        let camera = camera.trim().to_owned();
        let camera = if camera.is_empty() { "camera".to_owned() } else { camera };
        let stem = stem_suffix(&name(platform, source)).0;
        let file_name = pyformat::format(
            template,
            &[
                ("stem", Arg::Str(stem)),
                ("date", Arg::Str(format!("{:04}{:02}{:02}", when.year(), when.month(), when.day()))),
                ("time", Arg::Str(format!("{:02}{:02}{:02}", when.hour(), when.minute(), when.second()))),
                ("n", Arg::Int((n + 1) as i64)),
                ("camera", Arg::Str(camera)),
            ],
        )?;
        let day = format!("{:04}-{:02}-{:02}", when.year(), when.month(), when.day());
        let folder = match subfolders {
            "none" => String::new(),
            "year/date" => format!("{:04}/{day}", when.year()),
            _ => day,
        };
        let suffix = stem_suffix(&name(platform, source)).1.to_lowercase();
        let relative = norm_str(
            platform,
            &if folder.is_empty() { format!("{file_name}{suffix}") } else { format!("{folder}/{file_name}{suffix}") },
        );
        result.push((source.clone(), relative));
    }
    Ok((result, skipped))
}

/// Port of `import_copy._digest`: SHA-256 of a file.
pub fn digest(path: &Path) -> io::Result<[u8; 32]> {
    digest_bytes(path)
}

/// Port of `import_copy._free`: `path`, or `path` with `-1`, `-2`, ... when it exists (never overwrite).
pub fn free_name(platform: Platform, path: &str) -> String {
    let mut candidate = norm_str(platform, path);
    let mut n = 1;
    let (stem, suffix) = stem_suffix(&name(platform, path));
    while Path::new(&candidate).exists() {
        candidate = with_name(platform, path, &format!("{stem}-{n}{suffix}"));
        n += 1;
    }
    candidate
}

/// Port of `import_copy._copy`: copy to a new file (exclusive create) and verify the bytes; returns the written
/// path. A stale or foreign `.grainy-part` file is an error, never overwritten.
pub fn copy_verified(platform: Platform, source: &str, target: &str) -> Result<String> {
    let target = free_name(platform, target);
    let parent = pathx::parent(platform, &target);
    fs::create_dir_all(&parent).map_err(|e| Error::os(&e, Path::new(&parent), false))?;
    let partial = with_name(platform, &target, &format!("{}.grainy-part", name(platform, &target)));
    {
        let mut input = File::open(source).map_err(|e| Error::os(&e, Path::new(source), true))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|e| Error::os(&e, Path::new(&partial), true))?;
        io::copy(&mut input, &mut output).map_err(|e| Error::os(&e, Path::new(&partial), true))?;
    }
    copy_stat(Path::new(source), Path::new(&partial)).map_err(|e| Error::os(&e, Path::new(&partial), false))?;
    let same = digest_bytes(Path::new(&partial)).map_err(|e| Error::os(&e, Path::new(&partial), true))?
        == digest_bytes(Path::new(source)).map_err(|e| Error::os(&e, Path::new(source), true))?;
    if !same {
        let _ = fs::remove_file(&partial);
        return Err(Error::Os {
            message: tr_with("복사한 파일이 원본과 다릅니다: {0}", &[&name(platform, source)]),
            kind: io::ErrorKind::InvalidData,
        });
    }
    let target = if Path::new(&target).exists() { free_name(platform, &target) } else { target }; // appeared meanwhile
    fs::rename(&partial, &target).map_err(|e| Error::os(&e, Path::new(&target), false))?;
    Ok(target)
}

/// What [`execute`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Executed {
    /// Destination paths of the photos copied (and backed up when asked).
    pub copied: Vec<String>,
    /// `"<file name>: <error>"` for every photo that failed.
    pub errors: Vec<String>,
}

/// Port of `import_copy.execute`: copies every planned file (and its `.xmp` sidecar) into `destination`, and to
/// `backup` when given. Stops between photos when `cancel` is set. `progress` gets the number of photos handled.
pub fn execute(
    platform: Platform,
    planned: &[(String, String)],
    destination: &str,
    backup: Option<&str>,
    cancel: Option<&Cancel>,
    progress: Option<&mut dyn FnMut(usize)>,
) -> Result<Executed> {
    let mut result = Executed::default();
    let mut progress = progress;
    for (index, (source, relative)) in planned.iter().enumerate() {
        if cancel.is_some_and(Cancel::is_set) {
            break;
        }
        let outcome = (|| -> Result<String> {
            let target = copy_verified(platform, source, &pathx::join(platform, destination, relative))?;
            let sidecar = with_suffix(platform, source, ".xmp");
            if Path::new(&sidecar).is_file() {
                copy_verified(platform, &sidecar, &with_suffix(platform, &target, ".xmp"))?;
            }
            if let Some(backup) = backup.filter(|b| !b.is_empty()) {
                let folder = pathx::parent(platform, relative);
                let place = pathx::join(platform, &pathx::join(platform, backup, &folder), &name(platform, &target));
                copy_verified(platform, source, &place)?;
            }
            Ok(target)
        })();
        match outcome {
            Ok(target) => result.copied.push(target),
            Err(error @ Error::Os { .. }) => result.errors.push(format!("{}: {error}", name(platform, source))),
            Err(other) => return Err(other),
        }
        if let Some(callback) = progress.as_deref_mut() {
            callback(index + 1);
        }
    }
    Ok(result)
}

/// Port of `import_copy.known_keys`: the duplicate keys of every real (non-virtual) photo in the catalogue.
pub fn known_keys(catalog: &Catalog) -> Result<HashSet<DuplicateKey>> {
    let mut keys = HashSet::new();
    let rows = catalog.rows("SELECT path,metadata FROM photos WHERE virtual_source IS NULL", [])?;
    for row in rows {
        let path = row["path"].as_str().unwrap_or("");
        let text = match &row["metadata"] {
            Value::String(t) if !t.is_empty() => t.as_str(),
            _ => "{}",
        };
        let Ok(Value::Object(metadata)) = serde_json::from_str::<Value>(text) else { continue };
        if let Ok(key) = duplicate_key(catalog.platform(), path, &metadata) {
            keys.insert(key);
        }
    }
    Ok(keys)
}

/// The keys as a sorted list (handy for comparisons).
pub fn sorted_keys(keys: &HashSet<DuplicateKey>) -> Vec<DuplicateKey> {
    keys.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}
