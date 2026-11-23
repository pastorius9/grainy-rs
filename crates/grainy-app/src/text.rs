//! The app's own strings: texts the oracle does not have (render times, export progress…). Korean is the source
//! language, as in the oracle; English comes from [`ENGLISH`]. Oracle strings go through `grainy_i18n::tr` as
//! they are; [`tr`] falls back to it, so it can be used for both.

use grainy_i18n::Language;

/// Korean source → English, for the strings this crate adds.
pub const ENGLISH: [(&str, &str); 114] = [
    ("Grainy RS {0}의 달라진 점", "What's new in Grainy RS {0}"),
    ("Grainy RS가 {0} 버전으로 업데이트되었습니다.", "Grainy RS has been updated to version {0}."),
    ("AI에게 시키기 (로그인·모델 선택)…", "Ask AI (Sign In · Model)…"),
    ("프로그램 기본 모델", "Program default"),
    ("직접 입력", "Custom"),
    ("Claude Sonnet (권장)", "Claude Sonnet (Recommended)"),
    ("Claude Opus (최고 품질)", "Claude Opus (Highest quality)"),
    ("Claude Haiku (빠름)", "Claude Haiku (Fast)"),
    ("GPT-5.4 (권장)", "GPT-5.4 (Recommended)"),
    ("GPT-5.4 Mini (빠름)", "GPT-5.4 Mini (Fast)"),
    ("o3 (추론)", "o3 (Reasoning)"),
    ("Gemini 2.5 Pro (권장)", "Gemini 2.5 Pro (Recommended)"),
    ("Gemini 2.5 Flash (빠름)", "Gemini 2.5 Flash (Fast)"),
    (
        "자동 색 세기 슬라이더로 적용 정도(0–100%)를 실시간으로 조절할 수 있습니다.",
        "Adjust the Auto Color strength (0–100%) live after applying.",
    ),
    ("AI 노이즈 제거", "AI Denoise"),
    ("GPU 협업 패치 AI 디노이즈 · 로컬 오프라인 처리", "GPU Collaborative Patch AI Denoise · Local Offline"),
    ("소프트 프루핑", "Soft Proofing"),
    ("소프트 프루핑 끄기", "Off"),
    ("색역 경고 표시", "Show Gamut Warning"),
    ("블랙 포인트 보정 (BPC)", "Black Point Compensation (BPC)"),
    ("소프트 프루핑 · {0}", "Soft Proof · {0}"),
    ("소프트 프루핑 · {0} (색역 경고)", "Soft Proof · {0} (Gamut Warning)"),
    ("그림자", "Shadows"),
    ("중간톤", "Midtones"),
    (
        "숫자를 입력하거나 ↑/↓(Shift: 10배) 키로 미세 조정한 뒤 Enter 또는 다른 곳을 클릭해 적용합니다.",
        "Type a number or press Up/Down (Shift: 10×) to fine-adjust, then press Enter or click away to apply.",
    ),
    ("16비트 무손실", "16-bit lossless"),
    ("품질", "Quality"),
    ("HDR 모니터 창 열기", "Open HDR monitor window"),
    ("HDR 모니터 창 닫기", "Close HDR monitor window"),
    ("HDR 모니터 · +{0} EV", "HDR monitor · +{0} EV"),
    ("Windows에서 HDR을 켜고 HDR 모니터의 밝기 정보를 확인해 주세요.", "Enable HDR in Windows and check the HDR monitor's reported luminance."),
    ("macOS 디스플레이 설정에서 HDR(EDR) 지원 모니터를 확인해 주세요.", "Enable HDR/EDR on an HDR-capable display in macOS Display settings."),
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
    ("인물 피부 선택", "Select Portrait Skin"),
    ("사진 안에서 개체를 둘러싸 주세요.", "Drag a rectangle around the object."),
    ("선택 영역을 계산하는 중…", "Computing selection…"),
    ("피사체의 경계를 찾지 못했습니다. 개체를 더 가깝게 둘러싸 주세요.", "No distinct foreground was found. Draw a tighter rectangle around the object."),
    ("피부 영역을 찾지 못했습니다. 인물 사진인지 확인해 주세요.", "No distinct portrait skin region was found."),
    ("경계 검출", "Edge detection"),
    ("경계 검출 + 경계 다듬기", "Edge detection + guided matte"),
    ("피부 검출 + 경계 다듬기", "Skin detection + guided matte"),
    ("프로세스 버전", "Process Version"),
    ("프로세스 3 · 클래식 (호환)", "Process 3 · Classic (Compatible)"),
    ("프로세스 4 · Grainy RS 네이티브", "Process 4 · Grainy RS Native"),
    (
        "하이라이트 복원, 무헤일로 부분 대비(Local Laplacian), 엣지 인식 텍스처·안개 제거를 사용합니다.",
        "Uses highlight reconstruction, halo-free Local Laplacian clarity, and edge-aware texture, dehaze & denoise.",
    ),
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
    ("파이썬 Grainy 라이브러리 가져오기…", "Import the Python Grainy Library…"),
    ("Grainy RS 라이브러리를 연 상태에서 가져오세요.", "Open a Grainy RS library first, then import."),
    ("가져오기·내보내기 완료 후 다시 시도하세요.", "Try again once imports and exports have finished."),
    ("파이썬 Grainy 라이브러리를 찾지 못했습니다.", "The Python Grainy library was not found."),
    ("파이썬 Grainy 라이브러리를 읽는 중…", "Reading the Python Grainy library…"),
    ("파이썬 Grainy 라이브러리에 가져올 사진이 없습니다.", "The Python Grainy library has no photos to import."),
    ("지금 라이브러리(사진 {1}장)를 백업해 두고 파이썬 Grainy 라이브러리(사진 {0}장)로 바꿀까요?\n\n백업은 파일 관리의 '카탈로그 복원'으로 되돌릴 수 있습니다. 파이썬 Grainy의 라이브러리와 원본 사진은 바뀌지 않습니다.", "Back up the current library ({1} photos) and replace it with the Python Grainy library ({0} photos)?\n\nFile management › Restore catalog brings the backup back. The Python Grainy's library and your original photos are not changed."),
    ("파이썬 Grainy 라이브러리의 사진 {0}장을 보정·기록·컬렉션과 함께 가져올까요?\n\n파이썬 Grainy의 라이브러리와 원본 사진은 바뀌지 않습니다.", "Import the {0} photos of the Python Grainy library with their edits, history and collections?\n\nThe Python Grainy's library and your original photos are not changed."),
    ("파이썬 Grainy가 라이브러리를 쓰는 중입니다. 파이썬 Grainy를 닫고 다시 시도하세요.", "The Python Grainy is writing to its library. Close it and try again."),
    ("파이썬 Grainy 라이브러리를 읽지 못했습니다.", "The Python Grainy library could not be read."),
    ("파이썬 Grainy 라이브러리의 사진 {0}장을 가져왔습니다.", "Imported {0} photos from the Python Grainy library."),
    ("이제부터 파이썬 Grainy에서 고친 내용은 옮겨지지 않습니다. 파이썬 Grainy의 라이브러리와 원본 사진은 그대로입니다.", "Edits made in the Python Grainy from now on are not carried over. The Python Grainy's library and your original photos stay as they are."),
    ("파이썬 Grainy에서 쓰던 라이브러리를 가져올까요?", "Import the library you used in the Python Grainy?"),
    ("사진·보정·기록·컬렉션을 그대로 옮겨 옵니다. 파이썬 Grainy의 라이브러리와 원본 사진은 바뀌지 않습니다. 나중에 파일 관리 메뉴에서도 가져올 수 있습니다.", "Photos, edits, history and collections come over as they are. The Python Grainy's library and your original photos are not changed. You can also import it later from the File management menu."),
    ("AI 프로그램(Claude, Codex 등)을 파이썬 Grainy에 연결해 두셨다면, AI 연결 창의 설치 명령을 한 번 다시 실행해야 Grainy RS에 연결됩니다.", "If you connected AI programs (Claude, Codex…) to the Python Grainy, run the install command of the AI Connection window once more to connect them to Grainy RS."),
    ("AI 연결 열기", "Open AI Connection"),
    ("파이썬 Grainy 라이브러리 가져오기", "Import the Python Grainy Library"),
    ("{0}에서 아래 한 줄을 한 번 실행한 뒤 Claude Code를 다시 시작하세요. 확인은 claude mcp list. 예전 grainy 연결(파이썬 Grainy 등)이 있으면 이 연결로 바뀝니다.", "Run the line below once in {0}, then restart Claude Code. Check with claude mcp list. An earlier grainy connection (such as the Python Grainy's) is replaced by this one."),
    ("{0}에서 아래 한 줄을 한 번 실행한 뒤 Codex를 다시 시작하세요. 확인은 codex mcp list. 예전 grainy 연결이 있으면 이 연결로 바뀝니다.", "Run the line below once in {0}, then restart Codex. Check with codex mcp list. An earlier grainy connection is replaced by this one."),
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
