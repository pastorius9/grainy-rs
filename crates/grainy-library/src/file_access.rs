//! Port of `luma/file_access.py`: tell an original the system will not let Grainy read apart from one that is
//! gone. macOS keeps Files and Folders permission per app; an ad hoc signed update looks like a new app and every
//! read of the photos, even a stat, then fails with a permission error. Such photos need access, not relinking.
//!
//! The Qt dialog of the oracle (`guide`) and the timer thread (`check_folders`) belong to the application; this
//! module has what they decide with: [`access`], [`unreadable_folders`], the dialog texts ([`guide_text`]) and the
//! once-per-session state machine ([`Guide`]).

use grainy_i18n::{tr, tr_with};
use std::io::ErrorKind;
use std::path::Path;

/// `PRESENT`, `MISSING`, `DENIED` of the oracle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The file (or folder) is there.
    Present,
    /// Not there, or an error other than a refusal (a network volume that does not answer counts as missing).
    Missing,
    /// The system refuses access (macOS privacy settings): it is there, but Grainy may not read it.
    Denied,
}

impl Access {
    /// The oracle's constant (`'present'`, `'missing'`, `'denied'`).
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Present => "present",
            Access::Missing => "missing",
            Access::Denied => "denied",
        }
    }
}

/// `file_access.SETTINGS_URL`.
pub const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";

/// Port of `file_access.access`: `Present`, `Missing` or `Denied` for a photo file (a folder with `folder`).
pub fn access(path: &Path, folder: bool) -> Access {
    match std::fs::metadata(path) {
        Ok(meta) => {
            if (folder && meta.is_dir()) || (!folder && meta.is_file()) {
                Access::Present
            } else {
                Access::Missing
            }
        }
        Err(error) if error.kind() == ErrorKind::PermissionDenied => Access::Denied,
        Err(_) => Access::Missing,
    }
}

/// Port of `file_access.present`: `Path.is_file()` that never fails.
pub fn present(path: &Path) -> bool {
    access(path, false) == Access::Present
}

/// Port of `file_access.denied`.
pub fn denied(path: &Path) -> bool {
    access(path, false) == Access::Denied
}

/// Port of `file_access.unreadable_folders`: registered folders the system will not let Grainy list. Only each
/// folder itself is read (on macOS this is also what makes the system ask the person); missing or disconnected
/// folders are left to the folder panel.
pub fn unreadable_folders<S: AsRef<str>>(roots: &[S]) -> Vec<String> {
    let mut result = Vec::new();
    for root in roots {
        match std::fs::read_dir(root.as_ref()) {
            Ok(mut entries) => {
                let _ = entries.next();
            }
            Err(error) if error.kind() == ErrorKind::PermissionDenied => result.push(root.as_ref().to_owned()),
            Err(_) => {}
        }
    }
    result
}

/// What the access dialog says (`file_access.guide`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideText {
    /// Window title.
    pub title: String,
    /// Main text.
    pub text: String,
    /// Detail text (with the place appended).
    pub informative: String,
    /// The button that opens the system settings (macOS only).
    pub settings_button: Option<String>,
    /// The close button.
    pub close_button: String,
}

/// The dialog texts of `file_access.guide` for macOS (`mac`) or the other systems.
pub fn guide_text(place: &str, mac: bool) -> GuideText {
    let where_ = if place.is_empty() { String::new() } else { format!("\n\n{}", tr_with("위치: {0}", &[place])) };
    if mac {
        GuideText {
            title: tr("사진 폴더 접근 권한"),
            text: tr("macOS가 Grainy의 사진 폴더 접근을 막고 있습니다."),
            informative: tr("Grainy를 업데이트한 뒤 macOS가 폴더·디스크 접근 허용을 다시 묻거나 막는 일이 있습니다. 사진 파일과 보정 기록은 그대로 있고, 허용하면 다시 가져오지 않아도 그대로 돌아옵니다.\n\n허용하는 방법: 시스템 설정 › 개인정보 보호 및 보안 › 파일 및 폴더에서 Grainy 아래의 해당 폴더를 켭니다. 외장 디스크는 \"이동식 볼륨\", 네트워크 디스크는 \"네트워크 볼륨\"으로 나옵니다. 목록에 없으면 같은 화면의 전체 디스크 접근 권한에 Grainy를 추가합니다. 그다음 Grainy를 종료했다가 다시 여세요.") + &where_,
            settings_button: Some(tr("시스템 설정 열기")),
            close_button: tr("닫기"),
        }
    } else {
        GuideText {
            title: tr("사진 폴더 접근 권한"),
            text: tr("Grainy가 사진 폴더를 읽을 권한이 없습니다."),
            informative: tr("사진 파일과 보정 기록은 그대로 있습니다. 탐색기에서 폴더를 오른쪽 클릭 › 속성 › 보안에서 내 계정에 읽기 권한이 있는지 확인한 뒤 Grainy를 다시 여세요.") + &where_,
            settings_button: None,
            close_button: tr("닫기"),
        }
    }
}

/// What the application does after asking [`Guide::request`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideStep {
    /// Already shown this session (or already waiting): do nothing.
    Skip,
    /// Another modal question is open: ask again after about a second (call [`Guide::retry_fired`] then).
    RetryLater,
    /// Show the dialog now.
    Show,
}

/// The once-per-session state of `file_access.guide` (the `shown` and `pending` globals).
#[derive(Debug, Default)]
pub struct Guide {
    /// The guidance was shown in this run.
    pub shown: bool,
    /// A retry is waiting for the other dialog to close.
    pub pending: bool,
}

impl Guide {
    /// Port of the decision part of `file_access.guide`.
    pub fn request(&mut self, modal_active: bool) -> GuideStep {
        if self.shown {
            return GuideStep::Skip;
        }
        if modal_active {
            if self.pending {
                return GuideStep::Skip;
            }
            self.pending = true;
            return GuideStep::RetryLater;
        }
        self.shown = true;
        GuideStep::Show
    }
    /// The delayed retry fired (`pending=False` in the oracle's `later`).
    pub fn retry_fired(&mut self) {
        self.pending = false;
    }
}
