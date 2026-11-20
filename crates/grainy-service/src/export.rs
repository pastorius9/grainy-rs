//! Export naming and planning (`photo_service` "export"): naming templates, safe file names, folders the AI
//! program names, plans with collision numbers, planned sizes and the export loop. Rendering and writing a file
//! are injected ([`Exporter`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use grainy_catalog::folders::Platform;
use serde_json::{json, Map, Value};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

use crate::pyutil::{get, int_or_zero, path_str, path_text, repr, resolved, round_int, strip, text, truthy};
use crate::{service, Error, Failure, Result, IMAGE_EXTENSIONS, PRESET_KEY};

/// Port of `photo_service.TOKENS`: naming tokens that can be empty for a photo.
pub const TOKENS: [&str; 4] = ["film", "preset", "camera", "date"];
/// Port of `photo_service.OWN_TEXT`.
pub const OWN_TEXT: &str = "a folder Grainy manages itself (its library, settings or program); choose another folder";
const EMPTY: char = '\0';
const NAMING_MESSAGE: &str =
    "naming may contain only {stem}, {n} or {n:02d}..{n:06d}, {film}, {preset}, {camera}, {date} and no path characters";
/// Port of `quick_export.SUFFIXES`.
pub const SUFFIXES: [(&str, &str); 8] = [
    ("JPEG", ".jpg"),
    ("AVIF", ".avif"),
    ("JPEG XL", ".jxl"),
    ("PNG", ".png"),
    ("PNG 16-bit", ".png"),
    ("TIFF 16-bit", ".tif"),
    ("TIFF 8-bit", ".tif"),
    ("TIFF HDR 32-bit", ".tif"),
];

/// Port of `photo_service.DEVICES`: Windows device names.
fn is_device(name: &str) -> bool {
    matches!(name, "CON" | "PRN" | "AUX" | "NUL")
        || (name.len() == 4
            && (name.starts_with("COM") || name.starts_with("LPT"))
            && name.as_bytes()[3].is_ascii_digit()
            && name.as_bytes()[3] != b'0')
}

/// `name.split('.')[0].strip().upper()`.
fn device_part(name: &str) -> String {
    strip(name.split('.').next().unwrap_or("")).to_uppercase()
}

fn unsafe_char(c: char) -> bool {
    "<>:\"/\\|?*".contains(c) || (c as u32) < 32
}

/// `str.translate(UNSAFE)`: path and control characters become '_'.
fn translate(s: &str) -> String {
    s.chars().map(|c| if unsafe_char(c) { '_' } else { c }).collect()
}

fn rstrip_dot_space(s: &str) -> &str {
    s.trim_end_matches(['.', ' '])
}

/// The naming grammar `photo_service.NAMING` matches in full.
fn naming_matches(naming: &str) -> bool {
    let mut rest = naming;
    if rest.is_empty() {
        return false;
    }
    while let Some(c) = rest.chars().next() {
        if c == '{' {
            let Some(end) = rest.find('}') else { return false };
            let field = &rest[1..end];
            let ok = matches!(field, "stem" | "n" | "film" | "preset" | "camera" | "date")
                || (field.len() == 5
                    && field.starts_with("n:0")
                    && field.ends_with('d')
                    && (b'2'..=b'6').contains(&field.as_bytes()[3]));
            if !ok {
                return false;
            }
            rest = &rest[end + 1..];
        } else if c == '}' || unsafe_char(c) {
            return false;
        } else {
            rest = &rest[c.len_utf8()..];
        }
    }
    true
}

/// `naming.format(stem=..., n=..., film=..., preset=..., camera=..., date=...)` for the naming grammar.
fn format_naming(naming: &str, stem: &str, n: i64, values: &[(&str, String)]) -> Result<String> {
    let mut out = String::new();
    let mut rest = naming;
    while let Some(start) = rest.find(['{', '}']) {
        out.push_str(&rest[..start]);
        if rest[start..].starts_with('}') {
            return Err(Error::Value("Single '}' encountered in format string".into()));
        }
        let Some(end) = rest[start..].find('}').map(|e| e + start) else {
            return Err(Error::Value("Single '{' encountered in format string".into()));
        };
        let field = &rest[start + 1..end];
        let (name, spec) = field.split_once(':').unwrap_or((field, ""));
        let value = match name {
            "stem" => stem.to_owned(),
            "n" => n.to_string(),
            other => values
                .iter()
                .find(|(k, _)| *k == other)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| Error::Key(repr(other)))?,
        };
        if spec.is_empty() {
            out.push_str(&value);
        } else if name == "n" {
            let width = spec.trim_start_matches('0').trim_end_matches('d').parse::<usize>().unwrap_or(0);
            if spec.starts_with('0') {
                out.push_str(&format!("{n:0width$}"));
            } else {
                out.push_str(&format!("{n:>width$}"));
            }
        } else {
            return Err(Error::Value(format!("Invalid format specifier '{spec}'")));
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Port of `photo_service.check_naming`: only the naming tokens, with file-name characters.
pub fn check_naming(naming: &Value) -> Result<()> {
    let Some(naming) = naming.as_str() else { return Err(service(NAMING_MESSAGE)) };
    let length = naming.chars().count();
    if !(1..=80).contains(&length) || naming.ends_with(['.', ' ']) || !naming_matches(naming) {
        return Err(service(NAMING_MESSAGE));
    }
    let values = [
        ("film", "Film".to_owned()),
        ("preset", "Preset".to_owned()),
        ("camera", "Camera".to_owned()),
        ("date", "2026-01-01".to_owned()),
    ];
    let name = format_naming(naming, "photo", 1, &values).map_err(|_| service(NAMING_MESSAGE))?;
    if strip(&name).is_empty() || crate::pyutil::name(&name) != name || is_device(&device_part(&name)) {
        return Err(service(NAMING_MESSAGE));
    }
    Ok(())
}

/// `photo_service._safe`: a metadata value as part of a file name.
fn safe(value: &str, limit: usize) -> String {
    let translated = translate(value);
    let cut: String = strip(&translated).chars().take(limit).collect();
    rstrip_dot_space(&cut).to_owned()
}

/// Starting code points of the Unicode decimal-digit (Nd) runs of ten, for Python's `\d`.
const ND_ZEROS: [u32; 64] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6, 0xE50, 0xED0,
    0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620,
    0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10, 0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0,
    0x112F0, 0x11450, 0x114D0, 0x11650, 0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50,
    0x16A60, 0x16AC0, 0x16B50, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950, 0x1FBF0, 0x1D7CE,
];

fn decimal(c: char) -> bool {
    let c = c as u32;
    if (0x1D7CE..=0x1D7FF).contains(&c) {
        return true;
    }
    ND_ZEROS.iter().any(|z| (*z..z + 10).contains(&c))
}

/// `re.match(r'(\d{4})[:\-](\d{2})[:\-](\d{2})', stamp)` as `YYYY-MM-DD`.
fn date_of(stamp: &str) -> String {
    let chars: Vec<char> = stamp.chars().collect();
    let digits =
        |range: std::ops::Range<usize>| chars.get(range.clone()).is_some_and(|c| c.iter().all(|c| decimal(*c)));
    let sep = |i: usize| chars.get(i).is_some_and(|c| *c == ':' || *c == '-');
    if digits(0..4) && sep(4) && digits(5..7) && sep(7) && digits(8..10) {
        let part = |r: std::ops::Range<usize>| chars[r].iter().collect::<String>();
        format!("{}-{}-{}", part(0..4), part(5..7), part(8..10))
    } else {
        String::new()
    }
}

fn or_empty(value: &Value) -> Value {
    if truthy(value) {
        value.clone()
    } else {
        Value::Object(Map::new())
    }
}

/// Port of `photo_service.name_values`: `{film, preset, camera, date}` of a catalogue record; '' where none.
pub fn name_values(record: &Value) -> Map<String, Value> {
    let settings = or_empty(get(record, "settings"));
    let info = or_empty(get(record, "info"));
    let extras = or_empty(get(record, "user_metadata"));
    let film = get(&settings, "film_look").as_str().and_then(grainy_film::entry);
    let first = |values: [&Value; 2]| values.into_iter().find(|v| truthy(v)).map(text).unwrap_or_default();
    let stamp = first([get(&extras, "DateTimeOriginal"), get(&info, "date")]);
    let mut out = Map::new();
    out.insert("film".into(), film.map(|f| safe(&f.name, 60)).unwrap_or_default().into());
    out.insert("preset".into(), safe(&first([get(&extras, PRESET_KEY), &Value::Null]), 60).into());
    out.insert("camera".into(), safe(&first([get(&info, "camera"), &Value::Null]), 60).into());
    out.insert("date".into(), date_of(&stamp).into());
    out
}

/// Port of `photo_service.file_stem`: naming filled in; an empty token goes with one separator before it (after
/// it when it starts the name); an empty result is the source's stem.
pub fn file_stem(naming: &str, stem: &str, n: i64, values: &Map<String, Value>) -> Result<String> {
    let filled: Vec<(&str, String)> = TOKENS
        .iter()
        .map(|k| {
            let v = values.get(*k).filter(|v| truthy(v)).map(text).unwrap_or_else(|| EMPTY.to_string());
            (*k, v)
        })
        .collect();
    let mut name = format_naming(naming, stem, n, &filled)?;
    if !name.contains(EMPTY) {
        return Ok(name);
    }
    let separator = |c: char| matches!(c, ' ' | '_' | '.' | '-');
    while let Some(at) = name.find(EMPTY) {
        if at == 0 {
            let mut rest = &name[1..];
            if rest.starts_with(separator) {
                rest = &rest[1..];
            }
            name = rest.to_owned();
        } else {
            let before = name[..at].chars().last().filter(|c| separator(*c));
            let start = before.map_or(at, |c| at - c.len_utf8());
            name = format!("{}{}", &name[..start], &name[at + 1..]);
        }
    }
    let result = rstrip_dot_space(strip(&name)).to_owned();
    Ok(if result.is_empty() { stem.to_owned() } else { result })
}

fn platform() -> Platform {
    Platform::native()
}

fn owned_as_written(own: &[PathBuf], target: &Path) -> bool {
    own.iter().any(|root| platform().contains_folder(&path_text(root), &path_text(target)))
}

fn own_contains(own: &[PathBuf], target: &Path) -> Result<bool> {
    if owned_as_written(own, target) {
        return Ok(true);
    }
    let env = grainy_platform::Environment::current();
    let protected = ["Grainy", "Luma"].map(|name| grainy_platform::local_data(&env, name, None));
    for folder in own {
        let is_protected =
            protected.iter().any(|p| platform().path_key(&path_text(p)) == platform().path_key(&path_text(folder)));
        let folder = if is_protected {
            match (folder.parent(), folder.file_name()) {
                (Some(parent), Some(name)) => resolved(parent).map(|p| p.join(name)),
                _ => Ok(folder.clone()),
            }
        } else {
            resolved(folder)
        }
        .map_err(|e| Error::Failed(Failure::io(&e)))?;
        if platform().contains_folder(&path_text(&folder), &path_text(target)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Port of `photo_service.export_folder`: `root`, or one folder level below it, created when missing; never
/// inside one of Grainy's own folders.
pub fn export_folder(root: &Path, subfolder: Option<&Value>, own: &[PathBuf], create: bool) -> Result<PathBuf> {
    let io = |e: std::io::Error| Error::Failed(Failure::io(&e));
    if owned_as_written(own, root) {
        return Err(service(format!("the export folder is inside {OWN_TEXT}")));
    }
    let target = match subfolder {
        Some(sub) => {
            let message = "subfolder must be one folder name without path characters";
            let Some(s) = sub.as_str() else { return Err(service(message)) };
            let length = s.chars().count();
            let bad = !(1..=80).contains(&length)
                || s == "."
                || s == ".."
                || s.chars().any(unsafe_char)
                || s.ends_with(['.', ' '])
                || is_device(&device_part(s));
            if bad {
                return Err(service(message));
            }
            let child = root.join(s);
            if owned_as_written(own, &child) {
                return Err(service(format!("the export folder is inside {OWN_TEXT}")));
            }
            let target = resolved(&child).map_err(io)?;
            let base = resolved(root).map_err(io)?;
            let parent = target.parent().map(path_text).unwrap_or_default();
            if platform().path_key(&parent) != platform().path_key(&path_text(&base)) {
                return Err(service(message));
            }
            target
        }
        None => resolved(root).map_err(io)?,
    };
    if own_contains(own, &target)? {
        return Err(service(format!("the export folder is inside {OWN_TEXT}")));
    }
    if create {
        if let Err(error) = std::fs::create_dir_all(&target) {
            return Err(service(format!(
                "the export folder cannot be created: {}",
                english_error(&Failure::io(&error))
            )));
        }
    }
    Ok(target)
}

/// Port of `photo_service.named_folder`: a folder the AI program named, absolute, resolved and outside Grainy's own
/// folders. It need not exist.
pub fn named_folder(value: &Value, own: &[PathBuf], what: &str) -> Result<PathBuf> {
    let text = value.as_str().filter(|t| !strip(t).is_empty() && !t.contains('\0'));
    let Some(text) = text else { return Err(service(format!("{what} must be a folder path"))) };
    let path = PathBuf::from(path_str(strip(text)));
    if !path.is_absolute() {
        return Err(service(format!("{what} must be an absolute folder path (with its drive or starting at /)")));
    }
    if owned_as_written(own, &path) {
        return Err(service(format!("{what} is inside {OWN_TEXT}")));
    }
    let path = resolved(&path).map_err(|_| service(format!("{what} cannot be used")))?;
    if own_contains(own, &path)? {
        return Err(service(format!("{what} is inside {OWN_TEXT}")));
    }
    Ok(path)
}

/// Result of [`image_files`].
#[derive(Clone, Debug, PartialEq)]
pub enum ImageFiles {
    /// Cancelled (`None`).
    Cancelled,
    /// More than the limit (`{'error': ...}`).
    TooMany(String),
    /// The files, sorted.
    Files(Vec<String>),
}

/// Port of `photo_service.image_files`: the image files Grainy imports in `folder` (and its subfolders).
pub fn image_files(
    folder: &Path,
    recursive: bool,
    cancel: &AtomicBool,
    limit: usize,
    own: &[PathBuf],
) -> Result<ImageFiles> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    let io = |e: std::io::Error| Error::Failed(Failure::io(&e));
    while let Some(dir) = pending.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir).map_err(io)?.collect::<std::io::Result<_>>().map_err(io)?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if cancel.load(Ordering::SeqCst) {
                return Ok(ImageFiles::Cancelled);
            }
            let path = entry.path();
            let file_type = entry.file_type().map_err(io)?;
            if recursive && file_type.is_dir() {
                pending.push(path.clone());
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let suffix = crate::pyutil::suffix(&name).to_lowercase();
            if IMAGE_EXTENSIONS.contains(&suffix.as_str()) && path.is_file() {
                let resolved = named_folder(&Value::String(path_text(&path)), own, "import file")?;
                found.push(path_text(&resolved));
                if found.len() > limit {
                    return Ok(ImageFiles::TooMany(format!(
                        "more than {limit} image files; import fewer at a time (subfolders, or recursive false)"
                    )));
                }
            }
        }
    }
    found.sort();
    Ok(ImageFiles::Files(found))
}

/// Port of `photo_service.english_error`: stable English at the protocol boundary.
pub fn english_error(error: &Failure) -> String {
    match error.kind.as_str() {
        "PermissionError" => "permission denied".into(),
        "FileNotFoundError" => "file or folder not found".into(),
        "FileExistsError" => "output already exists; try again for a new name".into(),
        _ if error.is_os_error() => "file system operation failed".into(),
        "" => "operation failed (internal error)".into(),
        kind => format!("operation failed ({kind})"),
    }
}

/// Port of `photo_service.safe_filename`: a portable file name with room for its suffix and collision number.
pub fn safe_filename(stem: &str, suffix: &str, index: usize) -> String {
    let normal: String = stem.nfc().collect();
    let translated = translate(&normal);
    let mut stem = rstrip_dot_space(strip(&translated)).to_owned();
    if stem.is_empty() {
        stem = "photo".into();
    }
    let device: String = device_part(&stem)
        .chars()
        .map(|c| match c {
            '¹' => '1',
            '²' => '2',
            '³' => '3',
            c => c,
        })
        .collect();
    if is_device(&device) {
        stem = format!("_{stem}");
    }
    let tail = if index == 1 { suffix.to_owned() } else { format!("_{index}{suffix}") };
    loop {
        let whole = format!("{stem}{tail}");
        if whole.len() <= 255 && whole.encode_utf16().count() <= 255 {
            break;
        }
        stem.pop();
    }
    let stem = rstrip_dot_space(&stem);
    format!("{}{tail}", if stem.is_empty() { "photo" } else { stem })
}

fn key(name: &str) -> String {
    name.nfc().collect::<String>().case_fold().collect()
}

/// The suffix of an export format (`quick_export.SUFFIXES`), or the original's for `Original`.
fn format_suffix(format: &str, path: &str) -> Result<String> {
    if format == "Original" {
        return Ok(crate::pyutil::suffix(path));
    }
    SUFFIXES.iter().find(|(f, _)| *f == format).map(|(_, s)| (*s).to_owned()).ok_or_else(|| Error::Key(repr(format)))
}

/// `Path(name).with_suffix('.xmp').name`.
fn sidecar(name: &str) -> String {
    let suffix = crate::pyutil::suffix(name);
    format!("{}.xmp", &name[..name.len() - suffix.len()])
}

/// Port of `photo_service.export_plans`: `(record, destination)` with names reserved in memory only.
pub fn export_plans(records: &[Value], folder: &Path, format: &str, naming: &str) -> Result<Vec<(Value, String)>> {
    let mut reserved: Vec<String> = Vec::new();
    if folder.is_dir() {
        for entry in std::fs::read_dir(folder).map_err(|e| Error::Failed(Failure::io(&e)))? {
            let entry = entry.map_err(|e| Error::Failed(Failure::io(&e)))?;
            reserved.push(key(&entry.file_name().to_string_lossy()));
        }
    }
    let folder_text = path_text(folder);
    let sep = if cfg!(windows) { "\\" } else { "/" };
    let mut plans = Vec::new();
    for (n, record) in records.iter().enumerate() {
        let path = crate::text_of(record, "path")?;
        let suffix = format_suffix(format, path)?;
        let stem = file_stem(naming, &crate::pyutil::stem(path), n as i64 + 1, &name_values(record))?;
        let mut index = 1;
        let (name, side) = loop {
            let name = safe_filename(&stem, &suffix, index);
            let side = sidecar(&name);
            if !reserved.contains(&key(&name)) && (format != "Original" || !reserved.contains(&key(&side))) {
                break (name, side);
            }
            index += 1;
        };
        reserved.push(key(&name));
        if format == "Original" {
            reserved.push(key(&side));
        }
        plans.push((record.clone(), path_str(&format!("{folder_text}{sep}{name}"))));
    }
    Ok(plans)
}

/// Port of `photo_service.output_files`: compact file results (at most `limit`, then `more_files`).
pub fn output_files(outputs: &[Value], limit: usize) -> Value {
    let files: Vec<Value> = outputs
        .iter()
        .take(limit)
        .map(|o| {
            let path = text(get(o, "path"));
            json!({"id": get(o, "photo_id"), "name": crate::pyutil::name(&path), "path": get(o, "path"),
                   "w": o["size"][0], "h": o["size"][1], "bytes": get(o, "bytes")})
        })
        .collect();
    let mut result = json!({"files": files});
    if outputs.len() > limit {
        result["more_files"] = (outputs.len() - limit).into();
    }
    result
}

/// The export size limit: a long side, or a `(width, height, fit)` box (0 = no limit on that side).
#[derive(Clone, Debug, PartialEq)]
pub enum Longest {
    /// The long side (0 = the photo's own size).
    Side(i64),
    /// A box and its fit, `inside` or `cover`.
    Box(i64, i64, String),
}
impl Longest {
    fn truthy(&self) -> bool {
        !matches!(self, Self::Side(0))
    }
    /// The oracle's JSON form: an integer or `[width, height, fit]`.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Array(a) if a.len() == 3 => {
                Some(Self::Box(a[0].as_i64()?, a[1].as_i64()?, a[2].as_str()?.to_owned()))
            }
            Value::Number(n) => Some(Self::Side(n.as_i64()?)),
            Value::Null => Some(Self::Side(0)),
            _ => None,
        }
    }
}

/// Port of `engine.export_longest` (copied): the long side the export resize gets for a photo of `(h, w)`.
pub fn export_longest(h: i64, w: i64, longest: &Longest) -> i64 {
    match longest {
        Longest::Side(n) => *n,
        Longest::Box(width, height, fit) => {
            let factors: Vec<f64> = [(*width, w), (*height, h)]
                .iter()
                .filter(|(limit, _)| *limit != 0)
                .map(|(limit, side)| *limit as f64 / *side as f64)
                .collect();
            if factors.is_empty() {
                return w.max(h);
            }
            let cover = fit == "cover";
            let mut factor = factors[0];
            for f in &factors[1..] {
                if (cover && *f > factor) || (!cover && *f < factor) {
                    factor = *f;
                }
            }
            if factor >= 1.0 {
                return w.max(h);
            }
            let side = w.max(h) as f64 * factor;
            let value = if cover { (side - 1e-6).ceil() } else { (side + 1e-6).floor() };
            (value as i64).max(1)
        }
    }
}

/// `(width, height)` of a photo whose size the catalogue does not know.
pub type Probe<'a> = &'a mut dyn FnMut(&Value) -> std::result::Result<(i64, i64), Failure>;

type Outcome = (Option<Value>, Option<String>);

/// Port of `photo_service.planned_size`: dimensions after the same orientation, crop rounding and resize as
/// `engine.export_image`. `probe` gives `(width, height)` of a photo whose size the catalogue does not know
/// (the oracle loads it at 32 px).
pub fn planned_size(record: &Value, longest: &Longest, probe: Probe<'_>) -> Result<(i64, i64)> {
    let settings = get(record, "settings");
    let info = get(record, "info");
    let (width, height) = (get(info, "width"), get(info, "height"));
    let (mut w, mut h) = if !truthy(width) || !truthy(height) {
        probe(record).map_err(Error::Failed)?
    } else {
        (int_or_zero(width), int_or_zero(height))
    };
    if int_or_zero(get(settings, "rotation")).rem_euclid(2) == 1 {
        std::mem::swap(&mut w, &mut h);
    }
    if let Some(crop) = get(settings, "crop").as_array().filter(|c| c.len() == 4) {
        let c: Vec<f64> = crop.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
        let (wf, hf) = (w as f64, h as f64);
        let l = (w - 1).min(0.max(round_int(c[0] * wf)));
        let t = (h - 1).min(0.max(round_int(c[1] * hf)));
        let r = (l + 1).max(w.min(round_int(c[2] * wf)));
        let b = (t + 1).max(h.min(round_int(c[3] * hf)));
        w = r - l;
        h = b - t;
    }
    let side = if longest.truthy() { export_longest(h, w, longest) } else { w.max(h) };
    let factor = (side as f64 / w.max(h) as f64).min(1.0);
    Ok((1.max(round_int(w as f64 * factor)), 1.max(round_int(h as f64 * factor))))
}

/// Port of `photo_service.export_records`'s options (the keyword arguments of the oracle).
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// `format` (`JPEG`, `PNG`, …, or `Original`).
    pub format: String,
    /// `quality`.
    pub quality: i64,
    /// `longest`.
    pub longest: Longest,
    /// `color_space`.
    pub color_space: String,
    /// `keep_metadata`.
    pub keep_metadata: bool,
    /// `naming`.
    pub naming: String,
    /// `grayscale`.
    pub grayscale: bool,
    /// `dither`.
    pub dither: bool,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: "JPEG".into(),
            quality: 95,
            longest: Longest::Side(0),
            color_space: "sRGB".into(),
            keep_metadata: false,
            naming: "{stem}_grainy".into(),
            grayscale: true,
            dither: true,
        }
    }
}

/// One `engine.export_image` call.
#[derive(Debug)]
pub struct ExportJob<'a> {
    /// The catalogue record (`path`, `settings`, `user_metadata`, `keywords`).
    pub record: &'a Value,
    /// The new file.
    pub destination: &'a Path,
    /// The options; `longest` `None` stands for the oracle's `longest or None`.
    pub options: &'a ExportOptions,
    /// `longest or None`.
    pub longest: Option<&'a Longest>,
}

/// What [`export_records`] needs from the codecs: write one new file and give its `(width, height)`.
pub trait Exporter: Sync {
    /// `engine.export_image(path, destination, settings, format, quality, longest, color_space, keep_metadata,
    /// user_metadata, keywords, grayscale=, dither=)`.
    fn export_image(&self, job: &ExportJob<'_>) -> std::result::Result<Value, Failure>;
    /// `quick_export.copy_original(record, destination)` for the `Original` format.
    fn copy_original(&self, record: &Value, destination: &Path) -> std::result::Result<Value, Failure>;
}

/// Port of `photo_service.export_records`: export new files with their sizes and bytes. `workers` is
/// `quick_export.export_concurrency(records)` (photos exported at once; results keep the plans' order).
/// `english`: English failures for the AI connection, `str(error)` otherwise.
#[allow(clippy::too_many_arguments)]
pub fn export_records(
    records: &[Value],
    folder: &Path,
    options: &ExportOptions,
    cancel: Option<&AtomicBool>,
    progress: Option<&(dyn Fn(usize) + Sync)>,
    exporter: &dyn Exporter,
    english: bool,
    workers: usize,
) -> Result<Value> {
    let plans = export_plans(records, folder, &options.format, &options.naming)?;
    let state = Mutex::new((0usize, 0usize)); // done, skipped
    let one = |(record, destination): &(Value, String)| -> Outcome {
        let finish = || {
            if let Some(p) = progress {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                s.0 += 1;
                p(s.0);
            }
        };
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            state.lock().unwrap_or_else(|e| e.into_inner()).1 += 1;
            finish();
            return (None, None);
        }
        let path = Path::new(destination);
        let result = if options.format == "Original" {
            exporter.copy_original(record, path)
        } else {
            let job = ExportJob {
                record,
                destination: path,
                options,
                longest: options.longest.truthy().then_some(&options.longest),
            };
            exporter.export_image(&job)
        };
        let out = match result {
            Ok(size) => {
                let bytes = std::fs::metadata(path).map(|m| Value::from(m.len())).unwrap_or(Value::Null);
                let signature = grainy_catalog::settings(get(record, "settings"))
                    .and_then(|s| grainy_catalog::workflow::edit_signature(&s));
                match signature {
                    Ok(signature) => (
                        Some(json!({"path": destination, "size": size, "bytes": bytes, "photo_id": get(record, "id"),
                                    "edit_signature": signature})),
                        None,
                    ),
                    Err(error) => {
                        let failure = Failure::new("ValueError", error.to_string());
                        (None, Some(failure_text(record, &failure, english)))
                    }
                }
            }
            Err(failure) => (None, Some(failure_text(record, &failure, english))),
        };
        finish();
        out
    };
    let results: Vec<Outcome> = if workers <= 1 || plans.len() < 2 {
        plans.iter().map(one).collect()
    } else {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<Outcome>>> = plans.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            for _ in 0..workers.min(plans.len()) {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(plan) = plans.get(i) else { break };
                    let r = one(plan);
                    *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                });
            }
        });
        slots.into_iter().map(|s| s.into_inner().unwrap_or_else(|e| e.into_inner()).unwrap_or((None, None))).collect()
    };
    let skipped = state.lock().unwrap_or_else(|e| e.into_inner()).1;
    let outputs: Vec<Value> = results.iter().filter_map(|(o, _)| o.clone()).collect();
    let failures: Vec<Value> = results.iter().filter_map(|(_, f)| f.clone().map(Value::String)).collect();
    Ok(json!({"outputs": outputs, "failures": failures, "skipped": skipped}))
}

fn failure_text(record: &Value, failure: &Failure, english: bool) -> String {
    let detail =
        if english { format!("could not export: {}", english_error(failure)) } else { failure.message.clone() };
    format!("{}: {detail}", text(get(record, "name")))
}
