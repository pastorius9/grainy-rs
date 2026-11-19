//! Port of luma/edit_fields.py: the numeric edit fields of the photo tools and their ranges. The UI and the AI
//! connection both show this table.

/// One editable field: settings key, label (Korean source text; pass through `tr`) and range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditField {
    /// The settings key.
    pub key: &'static str,
    /// The label (Korean source text).
    pub label: &'static str,
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
}

const fn f(key: &'static str, label: &'static str, min: f64, max: f64) -> EditField {
    EditField { key, label, min, max }
}

/// Port of `EDIT_FIELDS`, in the oracle's order.
pub static EDIT_FIELDS: [EditField; 22] = [
    f("exposure", "노출", -5.0, 5.0),
    f("contrast", "대비", -100.0, 100.0),
    f("highlights", "하이라이트", -100.0, 100.0),
    f("shadows", "그림자", -100.0, 100.0),
    f("whites", "흰색 계열", -200.0, 200.0),
    f("blacks", "검정 계열", -200.0, 200.0),
    f("temperature", "색온도", -100.0, 100.0),
    f("tint", "색조", -100.0, 100.0),
    f("saturation", "채도", -100.0, 100.0),
    f("vibrance", "생동감", -100.0, 100.0),
    f("clarity", "부분 대비", -100.0, 100.0),
    f("sharpen", "선명하게", 0.0, 150.0),
    f("vignette", "비네팅", 0.0, 100.0),
    f("grain", "그레인", 0.0, 100.0),
    f("vignette_midpoint", "비네팅 중간점", 0.0, 100.0),
    f("vignette_roundness", "비네팅 둥글기", -100.0, 100.0),
    f("vignette_feather", "비네팅 페더", 0.0, 100.0),
    f("vignette_highlights", "비네팅 하이라이트 보호", 0.0, 100.0),
    f("grain_size", "그레인 크기", 0.0, 100.0),
    f("grain_roughness", "그레인 거칠기", 0.0, 100.0),
    f("straighten", "수평 맞추기", -45.0, 45.0),
    f("monochrome", "흑백", 0.0, 1.0),
];

/// The field for a settings key.
pub fn field(key: &str) -> Option<&'static EditField> {
    EDIT_FIELDS.iter().find(|e| e.key == key)
}
