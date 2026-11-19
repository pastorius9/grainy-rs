//! Port of luma/panel_keys.py editing helpers over the shared panel table.
use crate::settings::equivalent as py_eq;
use serde_json::{Map, Value};
/// `panel_keys.switch_on(settings, keys)`: switches on, in place, the panels that own the changed keys.
pub fn switch_on(settings: &mut Map<String, Value>, keys: &[&str]) {
    let panels = crate::panels();
    let switch = |panel: &str| panels.switches.iter().find(|(p, _)| p == panel).map(|(_, key)| key.clone());
    let mut owners: Vec<&str> = Vec::new();
    for key in keys {
        if panels.switches.iter().any(|(_, k)| k == key) {
            continue;
        }
        if let Some((panel, _)) = panels.keys.iter().find(|(_, owned)| owned.iter().any(|k| k == key)) {
            if !owners.contains(&panel.as_str()) {
                owners.push(panel);
            }
        }
    }
    for panel in owners {
        let off: Vec<Value> = match settings.get("panels_off") {
            Some(Value::Array(list)) => list.clone(),
            _ => Vec::new(),
        };
        match switch(panel) {
            Some(key) => {
                if !settings.get(&key).is_some_and(crate::settings::truthy) {
                    settings.insert(key, Value::Bool(true));
                }
            }
            None => {
                if off.iter().any(|p| p.as_str() == Some(panel)) {
                    let kept = off.into_iter().filter(|p| p.as_str() != Some(panel)).collect();
                    settings.insert("panels_off".into(), Value::Array(kept));
                }
            }
        }
    }
}

/// `panel_keys.changed_keys(before, after)`: the panel-owned keys whose values differ (Python `!=`).
pub fn changed_keys(before: &Map<String, Value>, after: &Map<String, Value>) -> Vec<&'static str> {
    let null = Value::Null;
    crate::panels()
        .keys
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .filter(|key| !py_eq(before.get(key.as_str()).unwrap_or(&null), after.get(key.as_str()).unwrap_or(&null)))
        .map(String::as_str)
        .collect()
}

fn off_list(settings: &Map<String, Value>) -> Vec<Value> {
    match settings.get("panels_off") {
        Some(Value::Array(list)) => list.clone(),
        _ => Vec::new(),
    }
}

/// `panel_keys.carried`: target's `panels_off` after pasting `panels` from source.
pub fn carried(target: &Map<String, Value>, source: &Map<String, Value>, panels: &[&str]) -> Vec<Value> {
    let names = |list: Vec<Value>| list.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>();
    let mut off = names(off_list(target));
    let source_off = names(off_list(source));
    for panel in panels {
        if source_off.iter().any(|p| p == panel) {
            if !off.iter().any(|p| p == panel) {
                off.push((*panel).into());
            }
        } else {
            off.retain(|p| p != panel);
        }
    }
    off_panels().filter(|p| off.iter().any(|o| o == p)).map(|p| Value::String(p.into())).collect()
}

fn off_panels() -> impl Iterator<Item = &'static str> {
    crate::panels().off_panels.iter().map(String::as_str)
}
/// Port of panel_keys.panels_of, in panel-table order.
pub fn panels_of<'a>(keys: impl IntoIterator<Item = &'a str> + Clone) -> Vec<&'static str> {
    crate::panels()
        .keys
        .iter()
        .filter(|(_, owned)| keys.clone().into_iter().any(|k| owned.iter().any(|v| v == k)))
        .map(|(p, _)| p.as_str())
        .collect()
}
