//! The menu bar with the oracle's menus and items, in the oracle's order (luma/manager.py `build_menus`, the Edit
//! menu of luma/app.py, luma/extras.py, luma/workflow_ui.py and the Settings menu of luma/app.py). An item with a
//! command id runs that [`crate::commands`] command (greyed out when it cannot run, its key from the shortcut
//! table); items without one are later work packages and log their name.

use eframe::egui::{self, Ui};
use grainy_i18n::shortcuts::key_hint;
use grainy_i18n::Platform;

use crate::commands::{self, Invocation};
use crate::context::AppContext;
use crate::text::tr;

/// One menu entry: (Korean title, command id or "" for a later work package).
type Entry = (&'static str, &'static str);

const FILES: &[Entry] = &[
    ("선택 사진 이름 변경", "file.rename"),
    ("선택 사진 이동", "file.move"),
    ("폴더 이름 변경·이동", "file.move_folder"),
    ("카탈로그에서만 제거", "file.remove"),
    ("원본을 휴지통으로", "file.trash"),
    ("누락 원본 다시 연결", "file.relink"),
    ("중복 파일 검사", "file.duplicates"),
    ("오프라인 편집용 미리보기 만들기", "file.smart_previews"),
    ("카탈로그 백업", "file.backup"),
    ("구글 드라이브 백업…", "file.cloud_backup"),
    ("카탈로그 복원", "file.restore"),
    ("카탈로그 최적화", "file.optimize"),
    ("카탈로그 검사", "file.integrity"),
    ("선택 사진 미리보기 다시 만들기", "file.rebuild_previews"),
    ("누락 폴더 다시 연결", "file.relink_folder"),
];

const LIBRARY: &[Entry] = &[
    ("가상 사본 만들기", "cull.virtual_copy"),
    ("현재 보정 스냅샷 저장", "snapshots.save"),
    ("스냅샷 불러오기", "snapshots.restore"),
    ("선택 사진 스택으로 묶기", "stacks.stack"),
    ("선택 사진 스택 해제", "stacks.unstack"),
    ("HDR 노출 병합…", "photo.merge_hdr"),
    ("평면 파노라마 병합…", "photo.merge_panorama"),
    ("메타데이터 편집", "metadata.edit"),
    ("촬영 시각 일괄 이동", "metadata.shift_date"),
    ("XMP 저장", "metadata.write_xmp"),
    ("XMP 불러오기", "metadata.read_xmp"),
    ("프리셋 파일 가져오기", "presets.import"),
    ("프리셋 파일 내보내기", "presets.export"),
    ("사용자 프리셋 삭제", "presets.delete"),
];

const LIBRARY_AFTER_LABELS: &[Entry] = &[
    ("전체 EXIF 메타데이터 새로 읽기", "metadata.refresh"),
    ("RAW 가져오기 기본 보정", "raw.defaults"),
    ("계층형 키워드 트리", "metadata.keyword_tree"),
];

/// Colour labels: (name, key). Port of the 색 라벨 submenu.
const LABELS: [(&str, &str); 6] =
    [("해제", ""), ("빨강", "6"), ("노랑", "7"), ("초록", "8"), ("파랑", "9"), ("보라", "")];

const VIEW: &[Entry] = &[
    ("상세 필터", "collections.filter"),
    ("필터·컬렉션 해제", "collections.clear_filter"),
    ("선택 사진 비교", "view.compare_photos"),
    ("선택 사진 모아보기", "view.survey"),
    ("현재 사진 전·후 비교", "view.compare_before"),
    ("스택 접어서 보기", "stacks.toggle"),
    ("화면 색상 관리…", "display.dialog"),
    ("모니터 색상 자동 감지", "display.auto"),
    ("모니터 ICC 프로파일 선택", "display.choose"),
    ("모니터 ICC 보정 해제", "display.off"),
];

const COLLECTIONS: &[Entry] =
    &[("선택 컬렉션 이름·규칙 변경", "collections.edit"), ("선택 사진을 컬렉션에서 제외", "collections.remove_photos")];

const SYNC: &[Entry] = &[
    ("복사할 항목 선택", "sync.choose_items"),
    ("현재 보정 → 선택 사진", "sync.selected"),
    ("Auto Sync · 선택한 사진 함께 보정", "sync.auto"),
];

const EXTRAS: &[Entry] = &[
    ("사진 복사해서 가져오기 (카드·다른 폴더)…", "import.copy"),
    ("테더 촬영 폴더 선택…", "tether.folder"),
    ("카메라 셔터 릴리스", "tether.capture"),
    ("GPS 지도", "extras.map"),
    ("현재 폴더 자동 가져오기", "import.watch"),
    ("자동 가져오기 해제", "import.unwatch"),
    ("Lightroom 사진·컬렉션 가져오기", "extras.lightroom_import"),
    ("최근 Lightroom 가져오기 결과", "extras.lightroom_report"),
];

const WORKFLOW: &[Entry] = &[
    ("자동 톤 (선택한 사진)", "workflow.auto_tone"),
    ("자동 색 (선택한 사진)", "workflow.auto_color"),
    ("자동 화이트 밸런스", "workflow.auto_white"),
    ("네거티브 반전 (선택한 사진)", "workflow.invert_negatives"),
    ("네거티브 반전 · 한 롤로 맞춤 (선택한 사진)", "workflow.invert_roll"),
    ("스캔 테두리 자동 자르기 (선택한 사진)", "workflow.border_crop"),
    ("최신 처리 방식으로 업데이트 (선택한 사진)", "workflow.update_process"),
    ("현재 사진에 총노출 맞추기", "workflow.match_exposure"),
    ("이전 사진 보정 붙이기", "develop.paste_previous"),
    ("카메라 방향으로 복원", "photo.camera_orientation"),
    ("빠른 컬렉션에 추가·제외", "cull.quick_collection"),
    ("빠른 컬렉션 보기", "collections.quick_show"),
    ("소속 컬렉션으로 이동…", "collections.containing"),
    ("현재 사진을 스택 대표로", "stacks.cover"),
    ("스택 안에서 앞으로", "stacks.forward"),
    ("스택 안에서 뒤로", "stacks.backward"),
    ("메타데이터 프리셋 저장…", "workflow.metadata_save"),
    ("메타데이터 프리셋 적용…", "workflow.metadata_apply"),
    ("메타데이터 프리셋 삭제…", "workflow.metadata_delete"),
    ("바로가기 키워드 설정…", "workflow.keyword_set"),
    ("바로가기 키워드 적용", "workflow.keyword_apply"),
    ("내보내기 기록…", "export.history"),
    ("내보내기 상태 초기화", "export.reset_history"),
    ("현재 사진을 참조 사진으로 고정", "workflow.reference"),
    ("두 번째 화면 열기", "workflow.second_window"),
];

/// The menu titles in the bar, in order.
pub const TITLES: [&str; 9] =
    ["파일 관리", "편집", "사진 관리", "보기·필터", "컬렉션", "보정 동기화", "추가 도구", "사진 작업", "설정"];

fn hint(key: &str) -> String {
    if key.is_empty() {
        String::new()
    } else {
        key_hint(key, Platform::current()).unwrap_or_else(|| grainy_i18n::keys_here(key))
    }
}

/// A menu entry; returns whether it was chosen (and closes the menu).
fn entry(ui: &mut Ui, title: &str, key: &str, enabled: bool, checked: Option<bool>) -> bool {
    let shortcut = hint(key);
    let mut button = egui::Button::new(tr(title));
    if let Some(checked) = checked {
        button = button.selected(checked);
    }
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut);
    }
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// The item of a command: its key, its enabled state; runs it when chosen.
fn command(ui: &mut Ui, cx: &mut AppContext, title: &str, id: &str, invocation: Invocation) {
    let key = commands::keys(id).first().copied().unwrap_or("");
    let checked = commands::sync::checked(cx, id).or_else(|| commands::stacks::checked(cx, id));
    if entry(ui, title, key, commands::enabled(cx, id), checked) {
        cx.run_with(id, &invocation);
    }
}

fn entries(ui: &mut Ui, cx: &mut AppContext, list: &[Entry]) {
    for (title, id) in list {
        if id.is_empty() {
            if entry(ui, title, "", true, None) {
                cx.stub(title);
            }
        } else {
            command(ui, cx, title, id, Invocation::default());
        }
    }
}

impl AppContext {
    pub(crate) fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().button_padding = egui::vec2(9.0, 4.0);
            ui.menu_button(tr(TITLES[0]), |ui| self.file_menu(ui));
            ui.menu_button(tr(TITLES[1]), |ui| self.edit_menu(ui));
            ui.menu_button(tr(TITLES[2]), |ui| {
                entries(ui, self, LIBRARY);
                ui.menu_button(tr("색 라벨"), |ui| {
                    for (name, key) in LABELS {
                        let value = if name == "해제" { "" } else { name };
                        if entry(ui, name, key, commands::enabled(self, "cull.color_label"), None) {
                            self.run_with("cull.color_label", &Invocation::value(value));
                        }
                    }
                });
                entries(ui, self, LIBRARY_AFTER_LABELS);
            });
            ui.menu_button(tr(TITLES[3]), |ui| self.view_menu(ui));
            ui.menu_button(tr(TITLES[4]), |ui| entries(ui, self, COLLECTIONS));
            ui.menu_button(tr(TITLES[5]), |ui| entries(ui, self, SYNC));
            ui.menu_button(tr(TITLES[6]), |ui| entries(ui, self, EXTRAS));
            ui.menu_button(tr(TITLES[7]), |ui| entries(ui, self, WORKFLOW));
            ui.menu_button(tr(TITLES[8]), |ui| self.settings_menu(ui));
        });
    }

    /// The import and export entries (the header's buttons and their keys), the libraries, then the catalogue's
    /// entries.
    fn file_menu(&mut self, ui: &mut Ui) {
        command(ui, self, "사진 가져오기", "file.import", Invocation::default());
        command(ui, self, "사진 폴더 가져오기 (하위 폴더 포함)", "file.import_folder", Invocation::default());
        command(ui, self, "내보내기…", "file.export", Invocation::default());
        ui.separator();
        command(ui, self, "라이브러리 폴더 열기…", "file.open_library", Invocation::default());
        command(ui, self, "Grainy 라이브러리 읽기 전용으로 열기", "file.open_grainy_library", Invocation::default());
        ui.separator();
        entries(ui, self, FILES);
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        command(ui, self, "실행 취소", "develop.undo", Invocation::default());
        command(ui, self, "다시 실행", "develop.redo", Invocation::default());
        ui.separator();
        command(ui, self, "보정 복사", "develop.copy_edits", Invocation::default());
        command(ui, self, "보정 붙여넣기", "develop.paste_edits", Invocation::default());
        ui.separator();
        command(ui, self, "보정 초기화", "develop.reset", Invocation::default());
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        entries(ui, self, VIEW);
        ui.menu_button(tr("사진 배경"), |ui| {
            let current = crate::display::background_key(self);
            for (name, key, _) in crate::display::BACKGROUNDS {
                if entry(ui, name, "", true, Some(key == current)) {
                    let value = Invocation { key: None, value: Some(serde_json::json!(key)) };
                    self.run_with("view.background", &value);
                }
            }
        });
        ui.separator();
        command(ui, self, "전체 화면", "view.full_screen", Invocation::default());
        command(ui, self, "양옆 패널 숨기기·보이기", "view.sides", Invocation::default());
        command(ui, self, "모든 패널 숨기기·보이기", "view.bars", Invocation::default());
        command(ui, self, "조명 끄기", "view.lights", Invocation::default());
    }

    fn settings_menu(&mut self, ui: &mut Ui) {
        command(ui, self, "언어 / Language…", "settings.language", Invocation::default());
        crate::gpu_worker::menu(ui, self);
        let on = crate::folder_watch::sync_enabled(self);
        if entry(ui, "등록한 폴더의 새 사진 자동으로 가져오기", "", true, Some(on)) {
            self.run("import.folder_sync");
        }
        command(ui, self, "단축키 목록…", "view.shortcuts", Invocation::default());
        command(ui, self, "Grainy 배우기 (웹 튜토리얼)…", "settings.learn", Invocation::default());
        // The update items exist where the program can update itself (`SUPPORTED`).
        if grainy_platform::updater::Layout::current().supported() {
            let on = commands::settings::update_check_enabled(self);
            if entry(ui, "새 버전 자동 확인", "", true, Some(on)) {
                self.run("settings.update_auto");
            }
            command(ui, self, "업데이트 확인…", "settings.update_check", Invocation::default());
        }
        command(ui, self, "AI 연결 (MCP)…", "ai.connection", Invocation::default());
        ui.separator();
        command(ui, self, "이번 버전의 달라진 점…", "settings.whats_new", Invocation::default());
        command(ui, self, "Grainy 정보…", "settings.about", Invocation::default());
        ui.separator();
        if entry(ui, "디버그 정보 표시", "", true, Some(self.debug_overlay)) {
            self.run("view.debug");
        }
    }
}
