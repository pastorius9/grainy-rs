//! Port of luma/updater.py, the logic that needs no window: updates from the project's GitHub releases.
//!
//! Checking: one HTTPS request for the newest release's description (the request itself is a function the caller
//! gives, so nothing here touches the network). Installing: the release zip is downloaded, verified against the
//! SHA-256 GitHub publishes for it, and unpacked next to the app. The new version then replaces the old one after
//! the app has closed: the previous version is moved aside (never deleted here) and restored if anything fails.
//! Photo libraries are not touched.
//!
//! Windows replaces the files of the release folder and works in its `.update` folder. macOS replaces the whole
//! `Grainy.app` bundle and works in `Application Support/Grainy/Update`. `UpdateController` (the dialogs and menu)
//! is UI and is not ported.
//!
//! Choices: the zip reading uses the `zip` crate and the checksum the `sha2` crate; the recycle bin is the `trash`
//! crate (the oracle uses Send2Trash); free disk space comes from `fs4`; the macOS steps shell out to `/usr/bin/ditto`
//! exactly as the oracle does (they cannot run on Windows, but their name checks and the swap are tested).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use grainy_i18n::tr;
use sha2::{Digest, Sha256};

use crate::json::Value;
use crate::user_paths::{local_data, Environment, System};
use crate::util::{unique_hex, wait_for_exit};
use crate::version::{self, VersionKey};

/// The oracle's (the Python Grainy's) GitHub repository. **The Rust app never reads it**: the oracle-port functions
/// ([`feed`], [`latest`]) keep it for their goldens only; the app uses the `RS_` constants.
pub const ORACLE_REPOSITORY: &str = "pastorius9/grainy";
/// The oracle's feed (port of `FEED`; goldens only).
pub const ORACLE_FEED: &str = "https://api.github.com/repos/pastorius9/grainy/releases/latest";
/// The oracle's download page (port of `PAGE`; goldens only).
pub const ORACLE_PAGE: &str = "https://github.com/pastorius9/grainy/releases/latest";
/// The oracle's download address prefix (port of `DOWNLOADS`; goldens only).
pub const ORACLE_DOWNLOADS: &str = "https://github.com/pastorius9/grainy/releases/download/";

/// The Rust build's own GitHub repository (separate from the Python Grainy's, whose updater would otherwise offer
/// a Rust release to Python users).
pub const RS_REPOSITORY: &str = "pastorius9/grainy-rs";
/// The Rust build's feed: the newest release's description ([`latest_rust`]).
pub const RS_FEED: &str = "https://api.github.com/repos/pastorius9/grainy-rs/releases/latest";
/// The Rust build's page to download from by hand.
pub const RS_PAGE: &str = "https://github.com/pastorius9/grainy-rs/releases/latest";
/// The only address the Rust build's downloads may come from.
pub const RS_DOWNLOADS: &str = "https://github.com/pastorius9/grainy-rs/releases/download/";
/// Everything a release folder holds.
pub const ITEMS: [&str; 4] = ["Grainy.exe", "_internal", "THIRD_PARTY_LICENSES", "LICENSE.txt"];

/// What a Windows release zip holds and what the swap replaces: the one top folder of the zip, the program in it,
/// the items of the program folder that belong to the release, and a folder that must not be empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package {
    /// The zip's only top folder (`Grainy`, `GrainyRS`).
    pub folder: &'static str,
    /// The program file inside it, started by [`launch_for`] and checked by the swap.
    pub program: &'static str,
    /// The files and folders of the program folder that a release replaces; nothing else there is touched.
    pub items: &'static [&'static str],
    /// A folder inside the top folder that must hold at least one entry (the oracle's `_internal`).
    pub required_folder: Option<&'static str>,
}

/// The Python Grainy's release (port of `ITEMS`, `stage`, `launch`, `finish`; the oracle's goldens).
pub const ORACLE_PACKAGE: Package =
    Package { folder: "Grainy", program: "Grainy.exe", items: &ITEMS, required_folder: Some("_internal") };

/// The files of the Rust build's release folder (`GrainyRS/` in its zip). The lens database is built into the
/// program, so nothing else is needed beside it.
pub const RS_ITEMS: [&str; 5] = ["GrainyRS.exe", "grainy-cli.exe", "LICENSE", "THIRD_PARTY_NOTICES.txt", "README.txt"];

/// The Rust build's release: `GrainyRS-<version>-windows-x64.zip` holding one folder `GrainyRS/` with
/// [`RS_ITEMS`]; only `GrainyRS.exe` is required.
pub const RS_PACKAGE: Package =
    Package { folder: "GrainyRS", program: "GrainyRS.exe", items: &RS_ITEMS, required_folder: None };
/// The macOS bundle name.
pub const BUNDLE: &str = "Grainy.app";
/// The program inside the bundle.
pub const PROGRAM: &str = "Contents/MacOS/Grainy";
/// Seconds between automatic checks.
pub const INTERVAL: u64 = 24 * 3600;
/// Seconds before a request gives up.
pub const TIMEOUT: u64 = 20;
/// Free space required at least.
pub const FREE_BYTES: u64 = 1024 * 1024 * 1024;
/// Seconds to wait for the old app to close.
pub const EXIT_WAIT: u64 = 120;

/// What can go wrong; the oracle's `OSError` is [`UpdateError::Io`], its `ValueError` is [`UpdateError::Value`].
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// A file system or process failure (`OSError`).
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Something that must be refused: a wrong download, an unsafe archive, an unreadable feed (`ValueError`).
    #[error("{0}")]
    Value(String),
    /// The archive could not be read at all (`BadZipFile`).
    #[error("{0}")]
    Zip(String),
}

impl From<zip::result::ZipError> for UpdateError {
    fn from(error: zip::result::ZipError) -> Self {
        match error {
            zip::result::ZipError::Io(io) => UpdateError::Io(io),
            other => UpdateError::Zip(other.to_string()),
        }
    }
}

/// The machine facts the updater depends on (the oracle reads them from `sys` and `platform`).
#[derive(Debug, Clone)]
pub struct Layout {
    /// The operating system.
    pub system: System,
    /// `platform.machine()`: `arm64` or `x86_64` on macOS.
    pub machine: String,
    /// Folders of the user.
    pub env: Environment,
}

impl Layout {
    /// The machine this runs on.
    pub fn current() -> Layout {
        let machine = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            other => other,
        };
        Layout { system: System::current(), machine: machine.to_string(), env: Environment::current() }
    }

    /// Port of `SUPPORTED`: only Windows and macOS have an installer.
    pub fn supported(&self) -> bool {
        matches!(self.system, System::Windows | System::MacOs)
    }
}

/// The release file name pattern of one system: `<prefix><version><tail>` (prefix `Grainy-` for the Python
/// Grainy, [`RUST_MARKER`] for this Rust build).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPattern {
    prefix: String,
    tail: String,
}

impl AssetPattern {
    /// The version in `name` when it is this system's release file (`^Grainy-(\d+(?:\.\d+)+)-...\.zip$`; as in
    /// Python, one newline may follow).
    pub fn captures(&self, name: &str) -> Option<String> {
        let name = name.strip_suffix('\n').unwrap_or(name);
        let version = name.strip_prefix(self.prefix.as_str())?.strip_suffix(self.tail.as_str())?;
        let mut count = 0;
        for piece in version.split('.') {
            if piece.is_empty() || !piece.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            count += 1;
        }
        (count >= 2).then(|| version.to_string())
    }

    /// The part of the name after the version.
    pub fn tail(&self) -> &str {
        &self.tail
    }

    /// The part of the name before the version.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

/// Port of `asset_pattern`: the release file for this system.
pub fn asset_pattern(layout: &Layout) -> AssetPattern {
    let prefix = "Grainy-".to_string();
    if !layout.system.is_mac() {
        return AssetPattern { prefix, tail: "-windows-x64.zip".into() };
    }
    let machine = match layout.machine.as_str() {
        "arm64" => "arm64",
        "x86_64" => "x64",
        _ => "unknown",
    };
    AssetPattern { prefix, tail: format!("-macos-{machine}.zip") }
}

/// The marker of this Rust build's release files: `GrainyRS-<version>-windows-x64.zip` (macOS
/// `GrainyRS-<version>-macos-<arm64|x64>.zip`).
///
/// **The update rule of the Rust build.** It reads only its own repository's feed ([`RS_FEED`]), never the Python
/// Grainy's, and never downloads or starts a Python installer (`Grainy-<version>-...zip`). A release qualifies only
/// when it carries a file whose name starts with this marker, with the oracle's other conditions unchanged: not a
/// draft or prerelease, a positive integer size, GitHub's `sha256:` digest of the file (checked after the download,
/// as the oracle does) and an address under [`RS_DOWNLOADS`], even with a feed override; and only when its version
/// is newer than [`crate::RS_VERSION`]. The two patterns exclude each other (`Grainy-` never matches `GrainyRS-` and
/// the reverse), so neither program picks up the other's installer.
pub const RUST_MARKER: &str = "GrainyRS-";

/// The release file of this Rust build for this system ([`RUST_MARKER`]).
pub fn rust_asset_pattern(layout: &Layout) -> AssetPattern {
    AssetPattern { prefix: RUST_MARKER.to_string(), ..asset_pattern(layout) }
}

/// Whether `release` is this Rust build's file for this system: its name carries [`RUST_MARKER`] and the
/// version it names, its size and digest are valid, and its download address belongs to this repository.
pub fn is_rust_release(release: &Release, layout: &Layout) -> bool {
    rust_asset_pattern(layout).captures(&release.name).is_some_and(|version| version == release.version)
        && !release.name.ends_with('\n')
        && release.size > 0
        && release.sha256.len() == 64
        && release.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && rust_download_address(&release.url, &release.name)
}

// Check components as well as the prefix: URL clients normalize dot segments,
// including percent-encoded ones, before sending a request.
fn rust_download_address(url: &str, name: &str) -> bool {
    let Some(rest) = url.strip_prefix(RS_DOWNLOADS) else { return false };
    let Some((tag, file)) = rest.split_once('/') else { return false };
    !tag.is_empty()
        && tag != "."
        && tag != ".."
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        && file == name
}

/// Port of `version_key`: every run of digits, as numbers.
pub fn version_key(text: &str) -> VersionKey {
    version::key(text, None)
}

/// Port of `newer`.
pub fn newer(candidate: &str, current: &str) -> bool {
    version_key(candidate) > version_key(current)
}

/// Port of `feed`: `GRAINY_UPDATE_FEED` (a URL or a local JSON file) replaces the feed for testing; pass its value.
pub fn feed(override_value: Option<&str>) -> String {
    match override_value {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => ORACLE_FEED.to_string(),
    }
}

/// [`feed`] for this Rust build: the override, else [`RS_FEED`].
pub fn rust_feed(override_value: Option<&str>) -> String {
    match override_value {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => RS_FEED.to_string(),
    }
}

/// How the caller opens an address for reading (the HTTP request, with the oracle's headers and [`TIMEOUT`]).
pub type Opener<'a> = &'a dyn Fn(&str) -> std::io::Result<Box<dyn Read>>;

/// Port of `fetch`: a local file when `url` has no `://`, else everything the opener gives.
pub fn fetch(url: &str, open: Opener<'_>) -> std::io::Result<Vec<u8>> {
    if !url.contains("://") {
        return std::fs::read(url);
    }
    let mut bytes = Vec::new();
    open(url)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A published release's zip for this system (the oracle's `{'version','name','url','size','sha256','notes'}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version in the file name.
    pub version: String,
    /// The zip's file name.
    pub name: String,
    /// Where it downloads from.
    pub url: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its SHA-256 digest, lower-case hex.
    pub sha256: String,
    /// The release description.
    pub notes: String,
}

fn text_of(value: Option<&Value>) -> String {
    value.filter(|v| v.truthy()).map(Value::py_str).unwrap_or_default()
}

/// Port of `parse_release`: the release's zip for this system, or none. The zip must come from this project's
/// release downloads (`trusted` is the required address prefix; none means any) and carry a SHA-256 digest.
/// Drafts, prereleases, other systems' files, and non-positive or non-integer sizes are refused.
pub fn parse_release(payload: &Value, trusted: Option<&str>, layout: &Layout) -> Option<Release> {
    parse_release_with(payload, trusted, &asset_pattern(layout))
}

/// [`parse_release`] for this Rust build: only a file carrying [`RUST_MARKER`] qualifies; the Python Grainy's
/// files are never returned.
pub fn parse_rust_release(payload: &Value, trusted: Option<&str>, layout: &Layout) -> Option<Release> {
    parse_release_with(payload, trusted, &rust_asset_pattern(layout)).filter(|release| is_rust_release(release, layout))
}

/// [`parse_release`] with the file name pattern given.
pub fn parse_release_with(payload: &Value, trusted: Option<&str>, pattern: &AssetPattern) -> Option<Release> {
    let Value::Object(payload) = payload else { return None };
    if payload.get("draft").is_some_and(Value::truthy) || payload.get("prerelease").is_some_and(Value::truthy) {
        return None;
    }
    let assets = match payload.get("assets") {
        Some(Value::List(items)) => items.as_slice(),
        _ => &[],
    };
    for asset in assets {
        let Value::Object(asset) = asset else { continue };
        let name = text_of(asset.get("name"));
        let digest_text = text_of(asset.get("digest"));
        let url = text_of(asset.get("browser_download_url"));
        let digest = digest_text
            .strip_prefix("sha256:")
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let (Some(version), Some(digest)) = (pattern.captures(&name), digest) else { continue };
        let size = match asset.get("size") {
            Some(Value::Int(size)) if *size > 0 => u64::try_from(*size).ok(),
            _ => None,
        };
        let Some(size) = size else { continue };
        if trusted.is_none_or(|prefix| url.starts_with(prefix)) {
            return Some(Release {
                version,
                name,
                url,
                size,
                sha256: digest.to_string(),
                notes: text_of(payload.get("body")),
            });
        }
    }
    None
}

/// Port of `latest`: the newest release (see [`parse_release`]) or none; fails when the feed cannot be read.
/// `override_feed` is the value of `GRAINY_UPDATE_FEED`.
pub fn latest(open: Opener<'_>, override_feed: Option<&str>, layout: &Layout) -> Result<Option<Release>, UpdateError> {
    latest_with(open, &feed(override_feed), (ORACLE_FEED, ORACLE_DOWNLOADS), &asset_pattern(layout))
}

/// [`latest`] for this Rust build: reads [`RS_FEED`] (or the override) and returns the newest release's file
/// carrying [`RUST_MARKER`], or none (the Python Grainy's files never qualify; see [`RUST_MARKER`]).
pub fn latest_rust(
    open: Opener<'_>,
    override_feed: Option<&str>,
    layout: &Layout,
) -> Result<Option<Release>, UpdateError> {
    let bytes = fetch(&rust_feed(override_feed), open)?;
    let text = String::from_utf8(bytes).map_err(|e| UpdateError::Value(e.to_string()))?;
    let payload = Value::parse(&text).map_err(|e| UpdateError::Value(e.to_string()))?;
    Ok(parse_rust_release(&payload, Some(RS_DOWNLOADS), layout))
}

/// `project` is the project's feed and its download prefix: only a release read from that feed must come from
/// that prefix (the oracle's `None if source != FEED else DOWNLOADS`).
fn latest_with(
    open: Opener<'_>,
    source: &str,
    project: (&str, &str),
    pattern: &AssetPattern,
) -> Result<Option<Release>, UpdateError> {
    let bytes = fetch(source, open)?;
    let text = String::from_utf8(bytes).map_err(|e| UpdateError::Value(e.to_string()))?;
    let payload = Value::parse(&text).map_err(|e| UpdateError::Value(e.to_string()))?;
    Ok(parse_release_with(&payload, if source != project.0 { None } else { Some(project.1) }, pattern))
}

/// Port of `app_folder` for a packaged app whose executable is `executable` (already resolved): the folder of the
/// program, on macOS the `.app` bundle; none when the program is not inside a bundle. Running from source (the
/// oracle's `sys.frozen` unset) is the caller's case: it has no app folder.
pub fn app_folder(executable: &Path, layout: &Layout) -> Option<PathBuf> {
    if !layout.system.is_mac() {
        return executable.parent().map(Path::to_path_buf);
    }
    let bundle = executable.ancestors().nth(3)?;
    (bundle.extension().is_some_and(|e| e == "app")).then(|| bundle.to_path_buf())
}

/// Port of `work_folder`: where downloads, the staged version and previous versions are kept.
pub fn work_folder(app: &Path, layout: &Layout) -> PathBuf {
    if !layout.system.is_mac() {
        return app.join(".update");
    }
    local_data(&layout.env, "Grainy", None).join("Update")
}

/// Free bytes on the disk that holds `path` (what `shutil.disk_usage(path).free` reports).
pub fn free_space(path: &Path) -> std::io::Result<u64> {
    fs4::available_space(path)
}

/// Port of `blocked`: why this copy cannot update itself (empty when it can). `free` reports the free bytes of a
/// folder ([`free_space`]); a failure to read it is reported as the reason.
pub fn blocked(app: Option<&Path>, size: u64, layout: &Layout, free: &dyn Fn(&Path) -> std::io::Result<u64>) -> String {
    let Some(app) = app else { return tr("소스에서 실행 중이라 직접 업데이트할 수 없습니다.") };
    // macOS: the bundle is replaced as a whole, so the folder that holds it must be writable.
    let folder = if layout.system.is_mac() { app.parent().unwrap_or(app) } else { app };
    let probe = folder.join(format!(".update-probe-{}", unique_hex()));
    let wrote = std::fs::write(&probe, b"").and_then(|()| std::fs::remove_file(&probe));
    if wrote.is_err() {
        let _ = std::fs::remove_file(&probe);
        return tr("프로그램 폴더에 쓸 수 없습니다.");
    }
    match free(app) {
        Ok(bytes) if bytes < FREE_BYTES.max(size.saturating_mul(6)) => tr("디스크 공간이 부족합니다."),
        Ok(_) => String::new(),
        Err(error) => error.to_string(),
    }
}

fn read_block(stream: &mut dyn Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    // `read(1 << 20)` of a Python stream returns a full block unless the stream ends.
    let mut filled = 0;
    while filled < buffer.len() {
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Port of `download`: downloads the release zip into `folder` and verifies size and SHA-256. Returns the path,
/// or none if cancelled. `progress(done, total)` is called after each block; nothing unverified is left behind.
pub fn download(
    release: &Release,
    folder: &Path,
    mut progress: Option<&mut dyn FnMut(u64, u64)>,
    cancel: Option<&AtomicBool>,
    open: Opener<'_>,
) -> Result<Option<PathBuf>, UpdateError> {
    std::fs::create_dir_all(folder)?;
    let target = folder.join(&release.name);
    let part = folder.join(format!("{}.part", release.name));
    let mut stream: Box<dyn Read> =
        if release.url.contains("://") { open(&release.url)? } else { Box::new(std::fs::File::open(&release.url)?) };
    let result = (|| -> Result<Option<PathBuf>, UpdateError> {
        let mut output = std::fs::File::create(&part)?;
        let mut digest = Sha256::new();
        let mut done: u64 = 0;
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                return Ok(None);
            }
            let n = read_block(stream.as_mut(), &mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            digest.update(&buffer[..n]);
            done += n as u64;
            if done > release.size {
                break;
            }
            if let Some(report) = progress.as_deref_mut() {
                report(done, release.size);
            }
        }
        drop(output);
        if done != release.size || hex(&digest.finalize()) != release.sha256 {
            return Err(UpdateError::Value(tr("내려받은 파일이 깃헙에 올라간 파일과 다릅니다.")));
        }
        std::fs::rename(&part, &target)?;
        Ok(Some(target.clone()))
    })();
    let _ = std::fs::remove_file(&part);
    result
}

fn bad_structure() -> UpdateError {
    UpdateError::Value(tr("업데이트 파일의 구조가 올바르지 않습니다."))
}

fn unfinished() -> UpdateError {
    UpdateError::Io(std::io::Error::other(tr(
        "이전 업데이트가 정리되지 않았습니다. Grainy를 다시 시작한 뒤 시도해 주세요.",
    )))
}

/// The parts of a path as Windows reads them: split on both slashes, empty and `.` parts dropped.
fn windows_parts(name: &str) -> Vec<&str> {
    name.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect()
}

fn posix_parts(name: &str) -> Vec<&str> {
    name.split('/').filter(|p| !p.is_empty() && *p != ".").collect()
}

/// The name checks of the Windows release zip: every entry under `Grainy/`, none that escapes, and the program and
/// its `_internal` folder present.
pub fn check_release_names(names: &[String]) -> Result<(), UpdateError> {
    check_package_names(names, &ORACLE_PACKAGE)
}

/// [`check_release_names`] for `package`: every entry under its top folder, none that escapes, the program present
/// (and its required folder not empty).
pub fn check_package_names(names: &[String], package: &Package) -> Result<(), UpdateError> {
    // On Windows zipfile reads a backslash in an entry name as a slash.
    let names: Vec<String> = names.iter().map(|n| n.replace('\\', "/")).collect();
    for name in &names {
        let parts = windows_parts(name);
        if name.starts_with(['/', '\\'])
            || name.contains(':')
            || parts.contains(&"..")
            || parts.first() != Some(&package.folder)
        {
            return Err(bad_structure());
        }
    }
    let program = format!("{}/{}", package.folder, package.program);
    let required = package.required_folder.map(|folder| format!("{}/{folder}/", package.folder));
    if !names.contains(&program) || required.is_some_and(|prefix| !names.iter().any(|n| n.starts_with(&prefix))) {
        return Err(bad_structure());
    }
    Ok(())
}

/// The name checks of the macOS release zip: every entry under `Grainy.app/`, none that escapes, the program and
/// `Info.plist` present.
pub fn check_bundle_names(names: &[String]) -> Result<(), UpdateError> {
    for name in names {
        let parts = posix_parts(name);
        if name.starts_with('/') || parts.contains(&"..") || parts.first() != Some(&BUNDLE) {
            return Err(bad_structure());
        }
    }
    let has = |wanted: String| names.contains(&wanted);
    if !has(format!("{BUNDLE}/{PROGRAM}")) || !has(format!("{BUNDLE}/Contents/Info.plist")) {
        return Err(bad_structure());
    }
    Ok(())
}

fn entry_names(archive: &Path) -> Result<Vec<String>, UpdateError> {
    let zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    Ok(zip.file_names_raw().map(|raw| String::from_utf8_lossy(raw).into_owned()).collect())
}

/// Where `zipfile.extractall` puts an entry on Windows: drive and `..` parts dropped, `:<>|"?*` become `_`.
fn extraction_path(base: &Path, name: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in name.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..") {
        path.push(part.replace([':', '<', '>', '|', '"', '?', '*'], "_"));
    }
    path
}

fn extract_all(archive: &Path, scratch: &Path) -> Result<(), UpdateError> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    std::fs::create_dir_all(scratch)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = String::from_utf8_lossy(entry.name_raw()).replace('\\', "/");
        let target = extraction_path(scratch, &name);
        if name.ends_with('/') {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A link entry is written as a plain file holding the link text, as zipfile does: nothing is followed.
        let mut output = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

fn stage_bundle(archive: &Path, work: &Path) -> Result<PathBuf, UpdateError> {
    let staged = work.join("new");
    if staged.exists() {
        return Err(unfinished());
    }
    check_bundle_names(&entry_names(archive)?)?;
    let scratch = work.join(format!("extract-{}", unique_hex()));
    std::fs::create_dir_all(&scratch)?;
    // ditto restores the bundle's symbolic links and executable bits, which plain unzipping does not.
    let status =
        std::process::Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(archive).arg(&scratch).output()?;
    if !status.status.success() {
        return Err(UpdateError::Io(std::io::Error::other(String::from_utf8_lossy(&status.stderr).into_owned())));
    }
    let unpacked = scratch.join(BUNDLE);
    let is_link = std::fs::symlink_metadata(&unpacked).is_ok_and(|m| m.file_type().is_symlink());
    if is_link || !unpacked.join(PROGRAM).is_file() {
        return Err(bad_structure());
    }
    std::fs::rename(&scratch, &staged)?;
    Ok(staged.join(BUNDLE))
}

/// Port of `stage`: unpacks a verified release zip to `work/new` and returns that folder (which then holds
/// `Grainy.exe`); on macOS the unpacked bundle, `work/new/Grainy.app`. Refuses a zip whose entries leave the
/// `Grainy` folder (`..`, absolute or drive paths, other top folders) or that lacks the program.
pub fn stage(archive: &Path, work: &Path, layout: &Layout) -> Result<PathBuf, UpdateError> {
    if layout.system.is_mac() {
        return stage_bundle(archive, work);
    }
    stage_package(archive, work, &ORACLE_PACKAGE)
}

/// [`stage`] of a Windows release zip of `package`: checked ([`check_package_names`]), unpacked, its top folder
/// moved to `work/new`, which is returned.
pub fn stage_package(archive: &Path, work: &Path, package: &Package) -> Result<PathBuf, UpdateError> {
    let staged = work.join("new");
    if staged.exists() {
        return Err(unfinished());
    }
    let scratch = work.join(format!("extract-{}", unique_hex()));
    check_package_names(&entry_names(archive)?, package)?;
    extract_all(archive, &scratch)?;
    std::fs::rename(scratch.join(package.folder), &staged)?;
    let _ = std::fs::remove_dir(&scratch);
    Ok(staged)
}

/// Port of `restart_arguments`: the arguments the app was started with, without update commands.
pub fn restart_arguments(argv: &[String]) -> Vec<String> {
    argv.iter().filter(|a| a.as_str() != "--update-now").cloned().collect()
}

/// The command [`launch`] starts: the program, its arguments and its working folder.
pub fn launch_command(
    staged: &Path,
    app: &Path,
    arguments: &[String],
    own_pid: u32,
    layout: &Layout,
) -> (PathBuf, Vec<String>, PathBuf) {
    launch_command_for(staged, app, arguments, own_pid, layout, &ORACLE_PACKAGE)
}

/// [`launch_command`] for `package` (on Windows its program in the staged folder).
pub fn launch_command_for(
    staged: &Path,
    app: &Path,
    arguments: &[String],
    own_pid: u32,
    layout: &Layout,
    package: &Package,
) -> (PathBuf, Vec<String>, PathBuf) {
    let mut command = vec![
        "--finish-update".to_string(),
        app.display().to_string(),
        "--wait-pid".into(),
        own_pid.to_string(),
        "--".into(),
    ];
    command.extend(arguments.iter().cloned());
    if layout.system.is_mac() {
        (staged.join(PROGRAM), command, staged.parent().unwrap_or(staged).to_path_buf())
    } else {
        (staged.join(package.program), command, staged.to_path_buf())
    }
}

/// Port of `launch`: starts the new version's own installer step; it waits for this process to exit.
pub fn launch(staged: &Path, app: &Path, arguments: &[String], layout: &Layout) -> std::io::Result<()> {
    launch_for(staged, app, arguments, layout, &ORACLE_PACKAGE)
}

/// [`launch`] for `package`.
pub fn launch_for(
    staged: &Path,
    app: &Path,
    arguments: &[String],
    layout: &Layout,
    package: &Package,
) -> std::io::Result<()> {
    let (program, command, cwd) = launch_command_for(staged, app, arguments, std::process::id(), layout, package);
    let mut process = std::process::Command::new(program);
    process.args(command).current_dir(cwd).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    process.spawn().map(|_| ())
}

fn is_busy(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(), Some(32 | 33))
}

/// Port of `_move`: a rename retried while a scanner or the exiting app still holds the file.
fn move_item(source: &Path, target: &Path, attempts: u32) -> std::io::Result<()> {
    for attempt in 0..attempts {
        match std::fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(error) if is_busy(&error) => {
                if attempt == attempts - 1 {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Port of `_place`: moves a bundle; across volumes it copies it with ditto.
pub fn place(source: &Path, target: &Path) -> std::io::Result<()> {
    match std::fs::rename(source, target) {
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            let out = std::process::Command::new("/usr/bin/ditto").arg(source).arg(target).output()?;
            if out.status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(String::from_utf8_lossy(&out.stderr).into_owned()))
            }
        }
        other => other,
    }
}

fn copy_file(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::copy(source, target)?;
    // shutil.copy2 keeps the modification time.
    if let Ok(modified) = std::fs::metadata(source).and_then(|m| m.modified()) {
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(target) {
            let _ = file.set_modified(modified);
        }
    }
    Ok(())
}

/// Port of `shutil.copytree(source, target)`: the target must not exist; links are followed.
pub fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    if target.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists", target.display())));
    }
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let path = entry?.path();
        let destination = target.join(path.file_name().unwrap_or_default());
        if std::fs::metadata(&path)?.is_dir() {
            copy_tree(&path, &destination)?;
        } else {
            copy_file(&path, &destination)?;
        }
    }
    Ok(())
}

/// The steps of the swap that tests may replace (the oracle's tests patch `shutil.copytree` and `_place`).
#[derive(Clone)]
pub struct Hooks {
    /// Copies a folder to a new place (default [`copy_tree`]).
    pub copy_tree: fn(&Path, &Path) -> std::io::Result<()>,
    /// Moves a bundle (default [`place`]).
    pub place: fn(&Path, &Path) -> std::io::Result<()>,
    /// The time stamp of the `previous-*` and `failed-*` folders (default: now, local time, `%Y%m%d-%H%M%S`).
    pub stamp: Option<String>,
    /// How long to wait for the old app to close.
    pub exit_wait: Duration,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks { copy_tree, place, stamp: None, exit_wait: Duration::from_secs(EXIT_WAIT) }
    }
}

fn python_list(items: &[&str]) -> String {
    format!("[{}]", items.iter().map(|i| crate::json::str_repr(i)).collect::<Vec<_>>().join(", "))
}

fn reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    if text.is_empty() {
        "OSError".into()
    } else {
        text
    }
}

fn finish_bundle(
    app: &Path,
    staged: &Path,
    pid: i64,
    note: &mut dyn FnMut(&str),
    layout: &Layout,
    hooks: &Hooks,
    stamp: &str,
) -> String {
    let work = work_folder(app, layout);
    if !staged.join(PROGRAM).is_file() {
        return "staged version is incomplete".into();
    }
    if !wait_for_exit(pid, hooks.exit_wait) {
        note("the app did not close");
        return tr("Grainy가 닫히지 않아 업데이트하지 않았습니다.");
    }
    let backup = work.join(format!("previous-{stamp}"));
    let _ = std::fs::create_dir_all(&backup);
    let name = app.file_name().unwrap_or_default().to_os_string();
    let mut moved = false;
    let outcome = (|| -> std::io::Result<()> {
        (hooks.place)(app, &backup.join(&name))?;
        moved = true;
        if app.exists() {
            std::fs::remove_dir_all(app)?;
        }
        note(&format!("previous version moved to {}", backup.file_name().unwrap_or_default().to_string_lossy()));
        (hooks.place)(staged, app)?;
        if !app.join(PROGRAM).is_file() {
            return Err(std::io::Error::other("Grainy.app is incomplete after the move"));
        }
        note("new version in place");
        Ok(())
    })();
    match outcome {
        Ok(()) => String::new(),
        Err(error) => {
            note(&format!("failed: OSError({}); restoring", crate::json::str_repr(&reason(&error))));
            let _ = (|| -> std::io::Result<()> {
                if app.exists() {
                    let failed = work.join(format!("failed-{stamp}"));
                    std::fs::create_dir_all(&failed)?;
                    (hooks.place)(app, &failed.join(&name))?;
                }
                if moved && !app.exists() {
                    (hooks.place)(&backup.join(&name), app)?;
                }
                Ok(())
            })();
            reason(&error)
        }
    }
}

/// Port of `finish`: replaces the app's files with the staged version, `pid` being the old app (waited for). The
/// previous version is kept in `.update/previous-*`. On any failure the previous version is put back. Returns an
/// empty string or the reason it failed. `log` receives one timestamped line per step.
pub fn finish(app: &Path, staged: &Path, pid: i64, log: Option<&mut dyn Write>, layout: &Layout) -> String {
    finish_with(app, staged, pid, log, layout, &Hooks::default())
}

/// [`finish`] with replaceable steps.
pub fn finish_with(
    app: &Path,
    staged: &Path,
    pid: i64,
    log: Option<&mut dyn Write>,
    layout: &Layout,
    hooks: &Hooks,
) -> String {
    finish_package(app, staged, pid, log, layout, hooks, &ORACLE_PACKAGE)
}

/// [`finish_with`] for `package`: on Windows only `package.items` of the program folder are moved aside and
/// replaced, and its program must be in place afterwards.
pub fn finish_package(
    app: &Path,
    staged: &Path,
    pid: i64,
    log: Option<&mut dyn Write>,
    layout: &Layout,
    hooks: &Hooks,
    package: &Package,
) -> String {
    let work = work_folder(app, layout);
    let stamp = hooks.stamp.clone().unwrap_or_else(|| chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    let mut log = log;
    let mut note = |text: &str| {
        if let Some(log) = log.as_deref_mut() {
            let _ = writeln!(log, "{} {}", chrono::Local::now().format("%H:%M:%S"), text);
            let _ = log.flush();
        }
    };
    if layout.system.is_mac() {
        return finish_bundle(app, staged, pid, &mut note, layout, hooks, &stamp);
    }
    if !staged.join(package.program).is_file() {
        return "staged version is incomplete".into();
    }
    if !wait_for_exit(pid, hooks.exit_wait) {
        note("the app did not close");
        return tr("Grainy가 닫히지 않아 업데이트하지 않았습니다.");
    }
    let backup = work.join(format!("previous-{stamp}"));
    let _ = std::fs::create_dir_all(&backup);
    let mut moved: Vec<&str> = Vec::new();
    let mut copied: Vec<&str> = Vec::new();
    let outcome = (|| -> std::io::Result<()> {
        for &name in package.items {
            if app.join(name).exists() {
                move_item(&app.join(name), &backup.join(name), 40)?;
                moved.push(name);
            }
        }
        note(&format!(
            "previous version moved to {}: {}",
            backup.file_name().unwrap_or_default().to_string_lossy(),
            python_list(&moved)
        ));
        for &name in package.items {
            let source = staged.join(name);
            if source.is_dir() {
                copied.push(name);
                (hooks.copy_tree)(&source, &app.join(name))?;
            } else if source.is_file() {
                copied.push(name);
                copy_file(&source, &app.join(name))?;
            }
        }
        if !app.join(package.program).is_file() {
            return Err(std::io::Error::other(format!("{} missing after the copy", package.program)));
        }
        note(&format!("new version copied: {}", python_list(&copied)));
        Ok(())
    })();
    match outcome {
        Ok(()) => String::new(),
        Err(error) => {
            note(&format!("failed: OSError({}); restoring", crate::json::str_repr(&reason(&error))));
            let failed = work.join(format!("failed-{stamp}"));
            let _ = std::fs::create_dir_all(&failed);
            for name in &copied {
                if app.join(name).exists() {
                    let _ = move_item(&app.join(name), &failed.join(name), 8);
                }
            }
            for name in &moved {
                if !app.join(name).exists() {
                    let _ = move_item(&backup.join(name), &app.join(name), 8);
                }
            }
            reason(&error)
        }
    }
}

/// The core of `run_finish` (without its progress window): runs [`finish`] with the log `update.log` in the work
/// folder, appended to. Returns the failure reason or an empty string.
pub fn finish_logged(app: &Path, staged: &Path, pid: i64, layout: &Layout) -> String {
    finish_logged_for(app, staged, pid, layout, &ORACLE_PACKAGE)
}

/// [`finish_logged`] for `package`.
pub fn finish_logged_for(app: &Path, staged: &Path, pid: i64, layout: &Layout, package: &Package) -> String {
    let work = work_folder(app, layout);
    let _ = std::fs::create_dir_all(&work);
    match std::fs::OpenOptions::new().create(true).append(true).open(work.join("update.log")) {
        Ok(mut log) => finish_package(app, staged, pid, Some(&mut log), layout, &Hooks::default(), package),
        Err(error) => reason(&error),
    }
}

/// Sends a path to the recycle bin; false when that fails.
pub fn recycle(path: &Path) -> bool {
    trash::delete(path).is_ok()
}

fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => {
            std::fs::canonicalize(parent).map_or_else(|_| path.to_path_buf(), |p| p.join(name))
        }
        _ => path.to_path_buf(),
    })
}

fn matching(work: &Path, prefix: &str, directories_only: bool) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(work)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)))
        .filter(|p| !directories_only || p.is_dir())
        .collect();
    found.sort();
    found
}

/// Port of `cleanup`: tidies what an update left behind through `discard` (the recycle bin, [`recycle`]): the
/// staged copy, downloads, failed attempts, extractions, and all but the newest previous version. Does nothing when
/// the running program is itself the staged copy (`executable`).
pub fn cleanup(app: &Path, layout: &Layout, executable: &Path, discard: &mut dyn FnMut(&Path) -> bool) {
    let work = work_folder(app, layout);
    if !work.is_dir() || resolved(executable).ancestors().skip(1).any(|p| p == resolved(&work.join("new"))) {
        return;
    }
    let previous = matching(&work, "previous-", true);
    let mut paths = vec![work.join("new"), work.join("download")];
    paths.extend(matching(&work, "failed-", false));
    paths.extend(matching(&work, "extract-", false));
    paths.extend(previous.iter().take(previous.len().saturating_sub(1)).cloned());
    for path in paths {
        if path.exists() {
            let _ = discard(&path);
        }
    }
}
