//! The Develop column's panels. Port of `PANELS`, `ROUTES` and `TOOLS` of luma/develop_panels.py and of
//! `is_on`, `edited` and `switch_on` of luma/panel_keys.py.

use std::sync::OnceLock;

use grainy_core::Settings;
use serde_json::Value;

/// The panel table (`PANEL_KEYS`, `OFF_PANELS`, `SWITCHES`), read from grainy-core's asset so both crates use
/// the same file. (grainy-core keeps its parsed copy private; see the report's requests.)
const PANELS_JSON: &str = include_str!("../../../grainy-core/assets/panels.json");

/// The panels in the column's order: (key, Korean title). Port of `PANELS`.
pub const PANELS: [(&str, &str); 15] = [
    ("crop", "크롭"),
    ("healing", "복구"),
    ("masks", "마스크"),
    ("basic", "기본"),
    ("film", "필름"),
    ("curve", "톤 곡선"),
    ("mixer", "색상 혼합"),
    ("grading", "컬러 그레이딩"),
    ("detail", "디테일"),
    ("lens", "렌즈 교정"),
    ("transform", "변형"),
    ("effects", "효과"),
    ("photo_filter", "컬러 필터"),
    ("calibration", "카메라 보정"),
    ("metadata", "사진 정보·관리"),
];

/// Panels that are tools: shown only while their tool is selected. Port of `TOOLS`.
pub const TOOLS: [&str; 3] = ["crop", "healing", "masks"];

/// The tool strip: (route, Korean title). The oracle's indices 0, 1, 6, 5 into `ROUTES`.
pub const TOOL_STRIP: [(Tool, &str); 4] =
    [(Tool::Edit, "보정"), (Tool::Crop, "크롭"), (Tool::Healing, "복구"), (Tool::Masks, "마스크")];

/// The tool selected in the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    /// The panels (보정).
    Edit,
    Crop,
    Healing,
    Masks,
}

impl Tool {
    /// The panel shown for a tool (`None` for the panels).
    pub fn panel(self) -> Option<&'static str> {
        match self {
            Tool::Edit => None,
            Tool::Crop => Some("crop"),
            Tool::Healing => Some("healing"),
            Tool::Masks => Some("masks"),
        }
    }
}

struct Table {
    keys: Vec<(String, Vec<String>)>,
    off_panels: Vec<String>,
    switches: Vec<(String, String)>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let value: Value = serde_json::from_str(PANELS_JSON).unwrap_or(Value::Null);
        let names = |v: &Value| -> Vec<String> {
            v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
        };
        let keys = value["panel_keys"]
            .as_object()
            .map(|m| m.iter().map(|(panel, list)| (panel.clone(), names(list))).collect())
            .unwrap_or_default();
        let switches = value["switches"]
            .as_object()
            .map(|m| m.iter().filter_map(|(p, k)| k.as_str().map(|k| (p.clone(), k.to_owned()))).collect())
            .unwrap_or_default();
        Table { keys, off_panels: names(&value["off_panels"]), switches }
    })
}

/// The settings keys a panel holds (`PANEL_KEYS[panel]`); empty for panels without edits (crop, metadata).
pub fn keys(panel: &str) -> &'static [String] {
    table().keys.iter().find(|(name, _)| name == panel).map(|(_, keys)| keys.as_slice()).unwrap_or(&[])
}

/// Whether the panel has edits, hence a switch and a dot (`key in PANEL_KEYS`).
pub fn has_switch(panel: &str) -> bool {
    table().keys.iter().any(|(name, _)| name == panel)
}

/// The panel's own on/off key (`SWITCHES`), if it has one.
pub fn switch_key(panel: &str) -> Option<&'static str> {
    table().switches.iter().find(|(p, _)| p == panel).map(|(_, k)| k.as_str())
}

/// The panel that owns a key (`OWNER`).
pub fn owner(key: &str) -> Option<&'static str> {
    table().keys.iter().find(|(_, keys)| keys.iter().any(|k| k == key)).map(|(p, _)| p.as_str())
}

/// The Korean title of a panel.
pub fn title(panel: &str) -> &'static str {
    PANELS.iter().find(|(key, _)| *key == panel).map(|(_, t)| *t).unwrap_or("")
}

fn off_list(settings: &Settings) -> Vec<String> {
    match settings.value("panels_off") {
        Value::Array(list) => list.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

/// Port of `panel_keys.is_on`.
pub fn is_on(settings: &Settings, panel: &str) -> bool {
    match switch_key(panel) {
        Some(key) => settings.bool(key),
        None => !off_list(settings).iter().any(|p| p == panel),
    }
}

/// Port of `panel_keys.edited`: any of the panel's values differs from its default.
pub fn edited(settings: &Settings, panel: &str) -> bool {
    let defaults = grainy_core::defaults();
    keys(panel).iter().any(|key| !crate::state::py_eq(settings.value(key), defaults.value(key)))
}

/// The `panels_off` list with `panel` added or removed, in `OFF_PANELS` order.
pub fn panels_off_with(settings: &Settings, panel: &str, on: bool) -> Value {
    let mut off = off_list(settings);
    off.retain(|p| p != panel);
    if !on {
        off.push(panel.to_owned());
    }
    Value::Array(table().off_panels.iter().filter(|p| off.contains(p)).map(|p| Value::String(p.clone())).collect())
}

/// Port of `panel_keys.switch_on`: switches on the panels that own the changed keys (an edit made in a panel
/// that is off shows). A panel's own switch value does not count as an edit.
pub fn switch_on(settings: &mut Settings, changed: &[&str]) {
    let switch_values: Vec<&str> = table().switches.iter().map(|(_, k)| k.as_str()).collect();
    let mut panels: Vec<&str> = Vec::new();
    for key in changed {
        if switch_values.contains(key) {
            continue;
        }
        if let Some(panel) = owner(key) {
            if !panels.contains(&panel) {
                panels.push(panel);
            }
        }
    }
    for panel in panels {
        if is_on(settings, panel) {
            continue;
        }
        match switch_key(panel) {
            Some(key) => settings.set(key, true),
            None => {
                let list = panels_off_with(settings, panel, true);
                settings.set("panels_off", list);
            }
        }
    }
}
