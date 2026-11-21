//! The app's own strings: texts the oracle does not have (render times, export progress…). Korean is the source
//! language, as in the oracle; English comes from [`ENGLISH`]. Oracle strings go through `grainy_i18n::tr` as
//! they are; [`tr`] falls back to it, so it can be used for both.

use grainy_i18n::Language;

/// Korean source → English, for the strings this crate adds.
pub const ENGLISH: [(&str, &str); 61] = [
    ("16비트 무손실", "16-bit lossless"),
    ("품질", "Quality"),
    ("HDR 모니터 창 열기", "Open HDR monitor window"),
    ("HDR 모니터 창 닫기", "Close HDR monitor window"),
    ("HDR 모니터 · +{0} EV", "HDR monitor · +{0} EV"),
    ("Windows에서 HDR을 켜고 HDR 모니터의 밝기 정보를 확인해 주세요.", "Enable HDR in Windows and check the HDR monitor's reported luminance."),
    ("HDR 미리보기를 열 수 없습니다 ({0}).", "The HDR preview could not be displayed ({0})."),
    ("Adobe 처리 버전 {0} → Grainy 현상 · {1}장", "Adobe process version {0} → Grainy rendering · {1} photo(s)"),
    ("테더 촬영 폴더 선택…", "Choose Tethered Capture Folder…"),
    ("카메라 셔터 릴리스", "Release Camera Shutter"),
    ("테더 촬영 저장 폴더", "Tethered Capture Folder"),
    ("테더 촬영 저장 폴더: {0}", "Tethered capture folder: {0}"),
    ("카메라에서 촬영하고 파일을 받는 중…", "Capturing and downloading from the camera…"),
    ("Windows 시스템 폴더를 찾지 못했습니다.", "Windows system directory could not be found."),
    ("테더 촬영에는 gphoto2와 원격 촬영을 지원하는 카메라가 필요합니다.", "Tethered capture requires gphoto2 and a camera with remote shutter support."),
    ("촬영을 취소했습니다.", "Capture cancelled."),
    ("카메라 촬영에 실패했습니다.", "Camera capture failed."),
    ("카메라가 제한 시간 안에 응답하지 않았습니다.", "Camera capture timed out."),
    ("카메라에서 받은 사진이 없습니다.", "The camera returned no photos."),
    ("가져올 수 있는 촬영 파일이 없습니다.", "No supported capture files were received."),
    ("피사체 선택", "Select Subject"),
    ("배경 선택", "Select Background"),
    ("개체 선택", "Select Object"),
    ("사진 안에서 개체를 둘러싸 주세요.", "Drag a rectangle around the object."),
    ("선택 영역을 계산하는 중…", "Computing selection…"),
    ("피사체의 경계를 찾지 못했습니다. 개체를 더 가깝게 둘러싸 주세요.", "No distinct foreground was found. Draw a tighter rectangle around the object."),
    ("경계 검출", "Edge detection"),
    ("{0} 선택 완료 · {1}", "{0} complete · {1}"),
    ("HDR 노출 병합…", "Merge HDR Exposures…"),
    ("평면 파노라마 병합…", "Merge Planar Panorama…"),
    ("사진 병합", "Merge Photos"),
    ("새 사진을 저장했습니다: {0}", "Saved new photo: {0}"),
    ("TIFF 저장 완료 ({0}). 가져오기 실패: {1}", "Saved TIFF ({0}). Import failed: {1}"),
    ("네거티브", "Negative"),
    ("스포이드", "Eyedropper"),
    ("미리보기 {0} × {1} · {2} ms", "Preview {0} × {1} · {2} ms"),
    ("원본 해상도 {0} × {1} · {2} ms", "Full resolution {0} × {1} · {2} ms"),
    ("사진 {0}장을 가져왔습니다.", "Added {0} photos."),
    ("가져올 사진을 찾지 못했습니다.", "No photos found to add."),
    ("내보내는 중… {0}/{1}", "Exporting… {0}/{1}"),
    ("처리하지 못했습니다: {0}", "Could not process: {0}"),
    ("사진 {0}장", "{0} photos"),
    ("사진", "Photos"),
    ("화면 {0}%", "{0}%"),
    ("내보낼 사진이 없습니다.", "No photos to export."),
    ("읽기 전용", "Read-only"),
    ("라이브러리 폴더 열기…", "Open Library Folder…"),
    ("Grainy 라이브러리 읽기 전용으로 열기", "Open the Grainy Library Read-Only"),
    ("디버그 정보 표시", "Show Debug Information"),
    (
        "읽기 전용 · 보정은 이 창에서만 유지되고 저장되지 않습니다.",
        "Read-only · edits stay in this window and are not saved.",
    ),
    ("Grainy 라이브러리를 찾지 못했습니다.", "The Grainy library was not found."),
    ("읽기 전용 라이브러리에는 사진을 가져올 수 없습니다.", "Photos cannot be imported into a read-only library."),
    (
        "Python Grainy의 라이브러리를 읽기 전용으로 열었습니다. 보정은 저장되지 않습니다.",
        "The Python Grainy's library is open read-only. Edits are not saved.",
    ),
    ("선택한 {0}장을 카탈로그에서 제거합니다.", "{0} selected photos will be removed from the catalog."),
    ("예", "Yes"),
    ("아니요", "No"),
    ("원본 파일은 그대로 남습니다.", "The original files stay where they are."),
    ("준비됨", "Ready"),
    ("저장", "Save"),
    (
        "밝기 차이가 거의 없어 자동톤을 적용하지 않았습니다.",
        "Auto Tone was not applied: the photo has almost no tonal range.",
    ),
    (
        "렌즈 보정 데이터베이스는 Lensfun(https://lensfun.github.io/)의 것으로, CC BY-SA 3.0 라이선스(https://creativecommons.org/licenses/by-sa/3.0/)에 따라 수정 없이 들어 있습니다. 저작권은 Lensfun 기여자들에게 있습니다.",
        "The lens correction database is Lensfun's (https://lensfun.github.io/), included unmodified under the CC BY-SA 3.0 licence (https://creativecommons.org/licenses/by-sa/3.0/). Copyright: the Lensfun contributors.",
    ),
];

/// The translation of `source` for the current language: the app's table first, then the oracle's.
pub fn tr(source: &str) -> String {
    if grainy_i18n::language() == Language::English {
        if let Some((_, english)) = ENGLISH.iter().find(|(k, _)| *k == source) {
            return (*english).to_owned();
        }
    }
    grainy_i18n::tr(source)
}

/// [`tr`], then `{0}`, `{1}`… filled with `values` (Python's `str.format`).
pub fn tr_with(source: &str, values: &[&str]) -> String {
    let text = tr(source);
    grainy_i18n::format(&text, values).unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_formats_in_both_languages() {
        for (ko, en) in ENGLISH {
            let n = ko.matches('{').count();
            assert_eq!(n, en.matches('{').count(), "{ko}");
            let values: Vec<String> = (0..n).map(|i| i.to_string()).collect();
            let refs: Vec<&str> = values.iter().map(String::as_str).collect();
            assert!(grainy_i18n::format(ko, &refs).is_ok() && grainy_i18n::format(en, &refs).is_ok(), "{ko}");
        }
    }
}
