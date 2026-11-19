//! The pure parts of luma/display_profiles.py: profile validation (`_checked`), `read_profile` and the
//! `resolve` logic. The operating-system lookups (Windows `GetICMProfileW` on the display DC, macOS
//! `CGDisplayCopyColorSpace` + `CGColorSpaceCopyICCData`) are injected through [`SystemProfiles`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::profiles::{profile, Profile};
use crate::transform::convert;
use crate::{CmsError, Result};

/// `display_profiles.MAX_PROFILE_BYTES`: 16 MB.
pub const MAX_PROFILE_BYTES: usize = 16 * 1024 * 1024;

/// Port of `display_profiles.DisplayProfile` (the frozen dataclass and its defaults).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayProfile {
    /// `'auto'`, `'manual'` or `'off'`.
    pub mode: String,
    /// The screen name.
    pub screen: String,
    /// The profile file, if one was used.
    pub path: String,
    /// The profile's description (or the display's name on macOS).
    pub description: String,
    /// The status text shown in the settings.
    pub message: String,
    /// Whether resolving failed (the display then uses sRGB).
    pub error: bool,
    /// The profile bytes, `None` for sRGB.
    pub data: Option<Profile>,
    /// SHA-256 hex digest of the bytes, `'srgb'` for sRGB.
    pub digest: String,
}

impl Default for DisplayProfile {
    fn default() -> Self {
        Self {
            mode: "auto".into(),
            screen: String::new(),
            path: String::new(),
            description: "sRGB".into(),
            message: String::new(),
            error: false,
            data: None,
            digest: "srgb".into(),
        }
    }
}

/// A usable display profile: what `_checked` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedProfile {
    /// The profile.
    pub data: Profile,
    /// The description, or the given name when the profile has none.
    pub description: String,
    /// SHA-256 hex digest of the bytes.
    pub digest: String,
}

/// Port of `display_profiles._checked`: LittleCMS opens it, it is an RGB display or colour-space profile,
/// and an sRGB → profile conversion of three sample pixels is finite.
pub fn checked(data: &[u8], name: &str) -> Result<CheckedProfile> {
    let candidate = Profile::from_bytes(data);
    let info = candidate.info()?;
    if info.colorspace != Some("rgb") || !matches!(info.deviceclass, Some("display" | "colorspace")) {
        return Err(CmsError::ChooseDisplayProfile);
    }
    let sample = grainy_core::Image::from_shape_vec((1, 3, 3), vec![0., 0., 0., 0.2, 0.5, 0.8, 1., 1., 1.])
        .expect("1x3x3 sample");
    let converted = convert(&sample, &profile("sRGB")?, &candidate)?;
    if !converted.iter().all(|v| v.is_finite()) {
        return Err(CmsError::NonFiniteResult);
    }
    let digest = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    Ok(CheckedProfile { data: candidate, description: info.description.unwrap_or_else(|| name.to_string()), digest })
}

type ReadKey = (PathBuf, u64, Option<SystemTime>);
static READ_CACHE: Mutex<VecDeque<(ReadKey, CheckedProfile)>> = Mutex::new(VecDeque::new());

/// Port of `display_profiles.read_profile` (with `_read_profile`'s cache of 8 by path, size and modification
/// time; errors are not cached).
pub fn read_profile(path: impl AsRef<Path>) -> Result<CheckedProfile> {
    let io = |error: std::io::Error| CmsError::Io(error.to_string());
    let path = std::fs::canonicalize(path.as_ref()).map_err(io)?;
    let meta = std::fs::metadata(&path).map_err(io)?;
    if meta.len() > MAX_PROFILE_BYTES as u64 {
        return Err(CmsError::DisplayProfileTooLarge);
    }
    let key = (path.clone(), meta.len(), meta.modified().ok());
    {
        let mut cache = READ_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(index) = cache.iter().position(|(k, _)| *k == key) {
            let entry = cache.remove(index).expect("index from position");
            let value = entry.1.clone();
            cache.push_back(entry);
            return Ok(value);
        }
    }
    let mut data = Vec::new();
    {
        use std::io::Read;
        let file = std::fs::File::open(&path).map_err(io)?;
        file.take(MAX_PROFILE_BYTES as u64 + 1).read_to_end(&mut data).map_err(io)?;
    }
    if data.len() > MAX_PROFILE_BYTES {
        return Err(CmsError::DisplayProfileTooLarge);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let result = checked(&data, &name)?;
    let mut cache = READ_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() >= 8 {
        cache.pop_front();
    }
    cache.push_back((key, result.clone()));
    Ok(result)
}

/// The operating-system lookups `resolve` needs; the UI crate implements them.
///
/// * Windows (`windows_profile_path`): the screen is the GDI device name (`\\.\DISPLAYn`, else the error
///   "Windows 모니터 이름을 확인할 수 없습니다."); `CreateDCW(screen, screen)`, then `GetICMProfileW` twice
///   (size, then a buffer of at most 32768 UTF-16 units; error 122 retried once with the new size) and
///   `DeleteDC`. A zero size with success or error 0/2/1168/2015 means "no profile" (`Ok(None)`, sRGB, also in
///   Advanced Color mode); other errors fail. Read-only: never enumerate installed profiles.
/// * macOS (`mac_profile_data`): the screen is named `"Name (id)"`; `CGDisplayCopyColorSpace(id)`,
///   `CGColorSpaceCopyICCData`, its bytes (at most [`MAX_PROFILE_BYTES`], else an error), `CFRelease` both.
///   `Ok(None)` without an id, a colour space or data.
pub trait SystemProfiles {
    /// Whether this is macOS (the oracle's `MAC`); it also names the system in messages.
    fn is_mac(&self) -> bool;
    /// The Windows profile path of a screen, `None` for the system default (sRGB).
    fn windows_profile_path(&self, screen: &str) -> std::result::Result<Option<String>, String>;
    /// The macOS ICC data of a screen, `None` for the system default.
    fn mac_profile_data(&self, screen: &str) -> std::result::Result<Option<Vec<u8>>, String>;
}

/// Port of `display_profiles.resolve(mode, screen, path)`, with the system lookups injected.
pub fn resolve(mode: &str, screen: &str, path: &str, system: &dyn SystemProfiles) -> DisplayProfile {
    let base = |mode: &str| DisplayProfile { mode: mode.into(), screen: screen.into(), ..DisplayProfile::default() };
    if mode == "off" {
        return DisplayProfile { message: "앱의 화면 색상 보정이 꺼져 있습니다.".into(), ..base(mode) };
    }
    let name = if system.is_mac() { "macOS" } else { "Windows" };
    let provided = format!("{name}가 현재 모니터에 제공한 프로파일입니다.");
    let mut selected = path.to_string();
    let attempt = (|| -> Result<DisplayProfile> {
        if mode == "auto" && system.is_mac() {
            selected.clear();
            // `if system:` — empty bytes are no profile either.
            let system_data = system.mac_profile_data(screen).map_err(CmsError::System)?;
            if let Some(data) = system_data.filter(|data| !data.is_empty()) {
                let found = checked(&data, "")?;
                let display = screen.rsplit_once(" (").map_or(screen, |(head, _)| head);
                return Ok(DisplayProfile {
                    description: if display.is_empty() { found.description } else { display.to_string() },
                    data: Some(found.data),
                    digest: found.digest,
                    message: provided.clone(),
                    ..base(mode)
                });
            }
        } else if mode == "auto" {
            selected = system.windows_profile_path(screen).map_err(CmsError::System)?.unwrap_or_default();
        } else if mode != "manual" {
            return Err(CmsError::UnknownDisplayMode);
        }
        if selected.is_empty() {
            if mode == "manual" {
                return Err(CmsError::ManualProfileMissing);
            }
            return Ok(DisplayProfile {
                screen: screen.into(),
                message: format!("{name} 기본 색상(sRGB)을 사용합니다."),
                ..DisplayProfile::default()
            });
        }
        let found = read_profile(&selected)?;
        Ok(DisplayProfile {
            path: selected.clone(),
            description: found.description,
            data: Some(found.data),
            digest: found.digest,
            message: if mode == "auto" {
                provided.clone()
            } else {
                "직접 선택한 화면 프로파일입니다.".into()
            },
            ..base(mode)
        })
    })();
    attempt.unwrap_or_else(|error| DisplayProfile {
        path: selected.clone(),
        error: true,
        message: format!("{error}\n현재 화면은 sRGB로 표시합니다. 파일을 다시 선택하거나 자동 모드를 사용하세요."),
        ..base(mode)
    })
}
