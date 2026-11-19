//! Port of luma/shortcut_list.py: the window's keyboard shortcuts as one table (data only; the oracle's `run`
//! callbacks become a stable command id per row).

use crate::{keys as shown, Platform};

/// One row of the shortcut table. Port of the `Shortcut` namedtuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    /// Group heading (Korean source text; pass through `tr`).
    pub group: &'static str,
    /// The key sequences bound to the row (Qt notation, `Ctrl` is Command on macOS).
    pub keys: &'static [&'static str],
    /// Description (Korean source text; pass through `tr`).
    pub text: &'static str,
    /// Stable command id replacing the oracle's `run` callback, e.g. `view.clipping`.
    pub command: &'static str,
    /// How the list writes the keys when that differs from the keys themselves; empty when it does not.
    pub label: &'static str,
    /// Whether holding the key repeats it.
    pub repeat: bool,
    /// The Tab rows: in a text field they move the focus instead of running the row.
    pub focus: bool,
}

/// The groups in display order (`GROUPS`).
pub const GROUPS: [&str; 5] = ["화면 보기", "현상", "마스크", "분류", "파일"];

/// New Mask kinds: (kind, key, text). Port of `MASKS`.
pub const MASKS: [(&str, &str, &str); 3] = [
    ("brush", "K", "새 브러시 마스크"),
    ("linear", "M", "새 선형 그레이디언트 마스크"),
    ("radial", "Shift+M", "새 방사형 그레이디언트 마스크"),
];

/// Colour label keys by the catalog's name for them. Port of `LABELS` (in the oracle's order).
pub const LABELS: [(&str, &str); 4] = [("6", "빨강"), ("7", "노랑"), ("8", "초록"), ("9", "파랑")];

const fn row(
    group: &'static str,
    keys: &'static [&'static str],
    text: &'static str,
    command: &'static str,
    label: &'static str,
    repeat: bool,
    focus: bool,
) -> Shortcut {
    Shortcut { group, keys, text, command, label, repeat, focus }
}

/// The shortcut table, in the oracle's order. Port of `TABLE`.
pub static TABLE: [Shortcut; 37] = [
    row("화면 보기", &["\\"], "원본 비교", "view.compare_original", "", true, false),
    row("화면 보기", &["C"], "비교 보기", "view.compare", "", true, false),
    row("화면 보기", &["D"], "현상으로 이동", "view.develop", "", true, false),
    row("화면 보기", &["G"], "라이브러리로 이동", "view.library", "", true, false),
    row("화면 보기", &["J"], "클리핑 표시 켜기·끄기", "view.clipping", "", false, false),
    row("화면 보기", &["Z"], "1:1과 화면 맞춤 번갈아 보기", "view.zoom_toggle", "", false, false),
    row("화면 보기", &["F"], "전체 화면 켜기·끄기 (Esc로 나옴)", "view.full_screen", "", false, false),
    row("화면 보기", &["Tab"], "양옆 패널 숨기기·보이기", "view.sides", "", false, true),
    row("화면 보기", &["Shift+Tab"], "모든 패널 숨기기·보이기", "view.bars", "", false, true),
    row("화면 보기", &["L"], "조명 끄기: 사진만 보기 (Esc로 나옴)", "view.lights", "", false, false),
    row("화면 보기", &["Shift+/", "Ctrl+/"], "단축키 목록", "view.shortcuts", "? / Ctrl+/", false, false),
    row("현상", &["R"], "크롭", "develop.crop", "", true, false),
    row("현상", &["W"], "화이트 밸런스 스포이드 · 드래그하면 영역 평균", "develop.white_balance", "", false, false),
    row("현상", &["Q"], "힐링 브러시", "develop.heal", "", false, false),
    row("현상", &["Ctrl+Z"], "실행 취소", "develop.undo", "", true, false),
    row("현상", &["Ctrl+Shift+Z", "Ctrl+Y"], "다시 실행", "develop.redo", "", true, false),
    row("현상", &["Ctrl+U"], "자동 톤", "develop.auto_tone", "", false, false),
    row("현상", &["Ctrl+Shift+U"], "자동 색", "develop.auto_color", "", false, false),
    row("현상", &["Ctrl+[", "Ctrl+]"], "90° 회전 (왼쪽 · 오른쪽)", "develop.rotate", "Ctrl+[ / Ctrl+]", true, false),
    row("현상", &["Ctrl+Shift+C"], "보정 복사", "develop.copy_edits", "", true, false),
    row("현상", &["Ctrl+Shift+V"], "보정 붙여넣기", "develop.paste_edits", "", true, false),
    row("현상", &["Ctrl+Alt+V"], "이전 사진 보정 붙이기", "develop.paste_previous", "", true, false),
    row("마스크", &["K"], "새 브러시 마스크", "mask.new_brush", "", false, false),
    row("마스크", &["M"], "새 선형 그레이디언트 마스크", "mask.new_linear", "", false, false),
    row("마스크", &["Shift+M"], "새 방사형 그레이디언트 마스크", "mask.new_radial", "", false, false),
    row("마스크", &["O"], "마스크 오버레이 켜기·끄기", "mask.overlay", "", true, false),
    row("마스크", &["[", "]"], "브러시 크기 줄이기 · 키우기", "mask.brush_size", "[ / ]", true, false),
    row("분류", &["P"], "선택 표시", "cull.pick", "", true, false),
    row("분류", &["X"], "제외 표시", "cull.reject", "", true, false),
    row("분류", &["U"], "표시 해제", "cull.unflag", "", true, false),
    row("분류", &["0", "1", "2", "3", "4", "5"], "별점 0–5", "cull.rating", "0 – 5", true, false),
    row(
        "분류",
        &["6", "7", "8", "9"],
        "색 라벨: 빨강 · 노랑 · 초록 · 파랑 (같은 키를 한 번 더 누르면 해제)",
        "cull.color_label",
        "6 – 9",
        false,
        false,
    ),
    row("분류", &["B"], "빠른 컬렉션에 추가·제외", "cull.quick_collection", "", true, false),
    row("분류", &["Ctrl+'"], "가상 사본 만들기", "cull.virtual_copy", "", false, false),
    row("파일", &["Ctrl+O"], "사진 가져오기", "file.import", "", true, false),
    row("파일", &["Ctrl+Shift+E"], "내보내기…", "file.export", "", true, false),
    row("파일", &["Ctrl+E"], "이전 설정으로 내보내기", "file.quick_export", "", true, false),
];

/// The rows of one group, in order.
pub fn group_rows(group: &str) -> impl Iterator<Item = &'static Shortcut> + '_ {
    TABLE.iter().filter(move |e| e.group == group)
}

/// Port of `written(entry)`: the keys of a row as the list and the menus write them.
pub fn written(entry: &Shortcut, platform: Platform) -> String {
    if entry.label.is_empty() {
        shown(&entry.keys.join(" / "), platform)
    } else {
        shown(entry.label, platform)
    }
}

/// Port of `key_hint(key)`: the text for the row that binds `key`, for a tooltip or a menu. `None` where the
/// oracle's `next(...)` finds no row.
pub fn key_hint(key: &str, platform: Platform) -> Option<String> {
    let entry = TABLE.iter().find(|e| e.keys.contains(&key))?;
    Some(shown(if entry.label.is_empty() { key } else { entry.label }, platform))
}

/// Port of `columns(groups)`: the groups split in two runs, in order, with about the same number of rows each.
/// `sizes` are the row counts per group; returns the index where the second run starts.
pub fn column_split(sizes: &[usize]) -> usize {
    let total: usize = sizes.iter().sum();
    let mut best = 1;
    let mut best_diff = usize::MAX;
    let mut head = 0;
    for i in 1..sizes.len() {
        head += sizes[i - 1];
        let diff = head.abs_diff(total - head);
        if diff < best_diff {
            best_diff = diff;
            best = i;
        }
    }
    best
}
