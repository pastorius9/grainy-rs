//! The mask layers as the settings key `masks` stores them: a list of layer objects
//! (`name`, `enabled`, `opacity`, `invert`, `components`, `adjustments`), each component an object with its
//! `type` (`brush`, `linear`, `radial`, `luma`, `color`, `sky`), `operation` (`add`, `subtract`, `intersect`) and
//! the kind's own fields. Values are kept as JSON so fields this crate does not know survive every edit.
//!
//! Port of the list edits of luma/studio.py (`new_layer`, `brush_range_place`, `range_component`, `ranged`,
//! `load_layers` names).

use grainy_core::Settings;
use serde_json::{json, Map, Value};

use super::{kind_name, BRUSH_RANGES, BRUSH_RANGE_SOFTNESS};

/// A JSON object.
pub type Obj = Map<String, Value>;

/// The settings' layers (`settings['masks']`), a copy.
pub fn list(settings: &Settings) -> Vec<Value> {
    settings.value("masks").as_array().cloned().unwrap_or_default()
}

/// Layer `index` of `masks` when it exists (`0 <= index < len`).
pub fn at(masks: &[Value], index: isize) -> Option<&Obj> {
    usize::try_from(index).ok().and_then(|i| masks.get(i)).and_then(Value::as_object)
}

/// Mutable layer `index`.
pub fn at_mut(masks: &mut [Value], index: isize) -> Option<&mut Obj> {
    usize::try_from(index).ok().and_then(|i| masks.get_mut(i)).and_then(Value::as_object_mut)
}

/// The components of a layer (empty when it has none).
pub fn components(layer: &Obj) -> &[Value] {
    layer.get("components").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The components of a layer, to change (created when missing).
pub fn components_mut(layer: &mut Obj) -> &mut Vec<Value> {
    if !layer.get("components").is_some_and(Value::is_array) {
        layer.insert("components".into(), Value::Array(Vec::new()));
    }
    match layer.get_mut("components") {
        Some(Value::Array(list)) => list,
        _ => unreachable!("inserted above"),
    }
}

/// Component `index` of layer `layer` when both exist.
pub fn component_at(masks: &[Value], layer: isize, index: isize) -> Option<&Obj> {
    let components = components(at(masks, layer)?);
    usize::try_from(index).ok().and_then(|i| components.get(i)).and_then(Value::as_object)
}

/// A new, empty layer (`new_layer`): `name` is already translated, as the oracle stores `tr('마스크 {0}')`.
pub fn new_layer(name: &str) -> Value {
    json!({"name": name, "enabled": true, "opacity": 1, "components": [], "adjustments": {}})
}

/// The layer `preview_settings` adds for a stroke on a mask that does not exist yet.
pub fn preview_layer() -> Value {
    json!({"name": "마스크", "enabled": true, "opacity": 1, "components": [], "adjustments": {}})
}

/// `dict.get(key, True)` for `enabled`: Python truthiness of a stored value.
pub fn truthy(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(Value::Bool(b)) => *b,
        Some(Value::Null) => false,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn text<'a>(object: &'a Obj, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

/// Port of `brush_range_place`: the position of the mask's brush range (its last component), if it has one.
pub fn brush_range_place(components: &[Value]) -> Option<usize> {
    let last = components.last()?.as_object()?;
    let ranged = truthy(last.get("brush_range"), false)
        && text(last, "type") == Some("luma")
        && text(last, "operation") == Some("intersect");
    ranged.then(|| components.len() - 1)
}

/// Port of `range_component`: the luminance component that limits a brushed mask to the tones of `key`.
pub fn range_component(key: &str) -> Value {
    let (low, high) = BRUSH_RANGES.iter().find(|(k, _, _)| *k == key).and_then(|(_, _, r)| *r).unwrap_or((0.0, 1.0));
    json!({"type": "luma", "range": [low, high], "softness": BRUSH_RANGE_SOFTNESS, "operation": "intersect", "brush_range": key})
}

/// Port of `ranged`: (components with a brush stroke added, the stroke's position). The stroke goes inside the
/// mask's brush range; the first stroke starts the range chosen in the tool options (`brush_range`).
pub fn ranged(components: &[Value], stroke: Value, brush_range: &str) -> (Vec<Value>, usize) {
    let mut components = components.to_vec();
    if let Some(place) = brush_range_place(&components) {
        components.insert(place, stroke);
        return (components, place);
    }
    let adds = stroke.as_object().and_then(|s| s.get("operation")).and_then(Value::as_str).unwrap_or("add") == "add";
    components.push(stroke);
    if brush_range != "all" && adds {
        components.push(range_component(brush_range));
    }
    let index = if brush_range == "all" || !adds { components.len() - 1 } else { components.len() - 2 };
    (components, index)
}

/// The rows of a layer in the mask list (`load_layers`): each component's name, numbered per kind, with its
/// sign; the brush range shows as 밝기 제한. Already translated.
pub fn component_names(layer: &Obj) -> Vec<String> {
    let tr = crate::text::tr;
    let list = components(layer);
    let place = brush_range_place(list);
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut out = Vec::new();
    for (j, component) in list.iter().enumerate() {
        let empty = Map::new();
        let component = component.as_object().unwrap_or(&empty);
        let kind = text(component, "type").unwrap_or("");
        let count = match counts.iter_mut().find(|(k, _)| k == kind) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                counts.push((kind.to_owned(), 1));
                1
            }
        };
        let sign = match component.get("operation").and_then(Value::as_str).unwrap_or("add") {
            "subtract" => "-",
            "intersect" => "∩",
            _ => "+",
        };
        let name = if place == Some(j) {
            let key = text(component, "brush_range").unwrap_or("");
            let title = BRUSH_RANGES.iter().find(|(k, _, _)| *k == key).map_or("전체", |(_, t, _)| *t);
            format!("{sign}  {}: {}", tr("밝기 제한"), tr(title))
        } else {
            let mut name = format!("{sign}  {} {count}", tr(kind_name(kind)));
            if !truthy(component.get("enabled"), true) {
                name.push_str(&tr(" · 꺼짐"));
            }
            name
        };
        out.push(name);
    }
    out
}

/// A layer's name as the list shows it (`layer.get('name', '마스크')`).
pub fn layer_name(layer: &Obj) -> String {
    match layer.get("name") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "마스크".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_ranges() {
        let stroke = json!({"type": "brush", "operation": "add"});
        let (list, at) = ranged(&[], stroke.clone(), "shadows");
        assert_eq!(at, 0);
        assert_eq!(list.len(), 2);
        assert_eq!(brush_range_place(&list), Some(1));
        let (list, at) = ranged(&list, stroke.clone(), "highlights");
        assert_eq!((at, list.len()), (1, 3));
        let (list, at) = ranged(&[], json!({"type": "brush", "operation": "subtract"}), "shadows");
        assert_eq!((at, list.len()), (0, 1));
        let (_, at) = ranged(&[], stroke, "all");
        assert_eq!(at, 0);
    }

    #[test]
    fn names_count_per_kind() {
        let layer = json!({"components": [
            {"type": "linear"}, {"type": "radial", "operation": "subtract"}, {"type": "linear", "operation": "intersect", "enabled": false},
            {"type": "brush"}, range_component("midtones")
        ]});
        let names = component_names(layer.as_object().unwrap());
        assert_eq!(names[0], "+  선형 그레이디언트 1");
        assert_eq!(names[1], "-  방사형 그레이디언트 1");
        assert_eq!(names[2], "∩  선형 그레이디언트 2 · 꺼짐");
        assert_eq!(names[4], "∩  밝기 제한: 중간 밝기");
    }
}
