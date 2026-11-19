//! The AI connection's tools as data (port of the data half of `luma/mcp_tools.py`), the content helpers and the
//! [`Tools`] trait the protocol layer calls.
//!
//! Tool names, descriptions and every error text are English protocol text for the AI program. Key order of every
//! object follows the oracle's Python dict insertion order: `tools/list` is compared byte for byte.

use std::sync::OnceLock;
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::check::{check, ServiceError};
use crate::pyjson::format_g;

// --- tables from luma/photo_service.py, luma/edit_fields.py and luma/mcp_tools.py ------------------

/// `photo_service.MAX_ID`: the largest integer JSON readers keep exactly.
pub const MAX_ID: i64 = (1 << 53) - 1;
/// `photo_service.MAX_TARGETS`.
pub const MAX_TARGETS: i64 = 500;
/// `photo_service.CURVE_POINTS`.
pub const CURVE_POINTS: i64 = 16;
/// `mcp_tools.SHEET_CELLS`: most contact-sheet cells, and the default page.
pub const SHEET_CELLS: i64 = 36;
/// `mcp_tools.TARGET_TEXT`.
pub const TARGET_TEXT: &str = "Target: one of ids, where (list_photos filters) or scope ('selection' or 'current').";
/// `mcp_tools.UNFINISHED`.
pub const UNFINISHED: &str = "Grainy started this call but did not finish it within the time limit; it may still take effect. Check the photos (get_photo) before repeating it, or undo it with undo_last_batch.";
/// `mcp_tools.EXPORT_FORMATS`.
pub const EXPORT_FORMATS: [&str; 7] = ["JPEG", "TIFF 16-bit", "TIFF 8-bit", "PNG", "PNG 16-bit", "AVIF", "JPEG XL"];
/// `mcp_tools.COLOR_SPACES`.
pub const COLOR_SPACES: [&str; 4] = ["sRGB", "Adobe RGB", "Display P3", "ProPhoto RGB"];
/// `mcp_tools.FILM_TYPES`.
pub const FILM_TYPES: [&str; 6] = ["all", "color", "bw", "slide", "cine", "presets"];
/// `photo_service.SORTS`.
pub const SORTS: [&str; 4] = ["imported", "name", "capture", "rating"];
/// `photo_service.COPY_GROUPS` keys (the Korean values are GUI text and not needed here).
pub const COPY_GROUPS: [&str; 15] = [
    "working_space",
    "light",
    "white_balance",
    "raw_profile",
    "color",
    "film",
    "color_filter",
    "curves",
    "detail",
    "effects",
    "negative",
    "lens",
    "crop",
    "masks",
    "retouch",
];
/// `photo_service.FLAGS` keys.
pub const FLAGS: [&str; 3] = ["picked", "rejected", "unflagged"];
/// `photo_service.LABELS` keys (stored Korean names are not needed here).
pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// `photo_service.FIELDS` (`edit_fields.EDIT_FIELDS` plus the negative, film, rotation and flip entries): key,
/// low, high, in the oracle's order. The Korean slider labels are GUI text and not needed here.
pub const FIELDS: [(&str, i64, i64); 28] = [
    ("exposure", -5, 5),
    ("contrast", -100, 100),
    ("highlights", -100, 100),
    ("shadows", -100, 100),
    ("whites", -200, 200),
    ("blacks", -200, 200),
    ("temperature", -100, 100),
    ("tint", -100, 100),
    ("saturation", -100, 100),
    ("vibrance", -100, 100),
    ("clarity", -100, 100),
    ("sharpen", 0, 150),
    ("vignette", 0, 100),
    ("grain", 0, 100),
    ("vignette_midpoint", 0, 100),
    ("vignette_roundness", -100, 100),
    ("vignette_feather", 0, 100),
    ("vignette_highlights", 0, 100),
    ("grain_size", 0, 100),
    ("grain_roughness", 0, 100),
    ("straighten", -45, 45),
    ("monochrome", 0, 1),
    ("negative_density", -3, 3),
    ("negative_range", 3, 10),
    ("film_strength", 0, 100),
    ("film_look", 0, 0),
    ("rotation", 0, 270),
    ("flip", 0, 1),
];

// --- schema building ---------------------------------------------------------------------------

/// Python's `{**a, **b, ...}`: later keys replace earlier ones in place, new keys append.
fn merge(parts: &[&Map<String, Value>]) -> Map<String, Value> {
    let mut out = Map::new();
    for part in parts {
        for (key, value) in part.iter() {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// `mcp_tools.FILTER`: the list_photos filters, with descriptions.
fn filter() -> Map<String, Value> {
    object(json!({
        "folder": {"type": "string", "minLength": 1, "maxLength": 1024, "description": "Folder path as list_photos shows it."},
        "recursive": {"type": "boolean", "description": "Include subfolders (default false); needs folder."},
        "text": {"type": "string", "minLength": 1, "maxLength": 200, "description": "Part of the file name or a keyword."},
        "rating_min": {"type": "integer", "minimum": 1, "maximum": 5},
        "rating_max": {"type": "integer", "minimum": 0, "maximum": 5, "description": "At most N stars; 0 finds unrated photos."},
        "flag": {"type": "string", "enum": ["picked", "rejected", "unflagged"]},
        "label": {"type": "string", "enum": ["red", "yellow", "green", "blue", "purple"]},
        "collection": {"type": "integer", "minimum": 1, "maximum": MAX_ID, "description": "Collection id."}
    }))
}

/// `mcp_tools.SCOPE`.
fn scope() -> Value {
    json!({"type": "string", "enum": ["selection", "current"],
           "description": "selection: photos selected in Grainy; current: the photo open in the editor."})
}

/// `mcp_tools.BARE_FILTER`: the filters without descriptions (inside targets).
fn bare_filter() -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in filter() {
        let bare: Map<String, Value> = object(value).into_iter().filter(|(k, _)| k != "description").collect();
        out.insert(key, Value::Object(bare));
    }
    out
}

/// `mcp_tools.TARGET(max_ids)`: the ids / where / scope target properties.
pub fn target(max_ids: i64) -> Map<String, Value> {
    object(json!({
        "ids": {"type": "array", "items": {"type": "integer", "minimum": 1, "maximum": MAX_ID},
                "minItems": 1, "maxItems": max_ids, "uniqueItems": true},
        "where": {"type": "object", "properties": bare_filter(), "additionalProperties": false, "minProperties": 1},
        "scope": {"type": "string", "enum": ["selection", "current"]}
    }))
}

/// `mcp_tools._range`: `f'{low:g}..{high:g}'`.
pub fn range_text(low: i64, high: i64) -> String {
    format!("{}..{}", format_g(low as f64), format_g(high as f64))
}

/// Port of `mcp_tools._edits_description`: the apply_edits description with the ranges grouped.
pub fn edits_description() -> String {
    let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
    for &(key, low, high) in &FIELDS {
        if [
            "monochrome",
            "straighten",
            "exposure",
            "negative_density",
            "negative_range",
            "film_look",
            "rotation",
            "flip",
        ]
        .contains(&key)
        {
            continue;
        }
        let span = range_text(low, high);
        match groups.iter_mut().find(|(s, _)| *s == span) {
            Some((_, keys)) => keys.push(key),
            None => groups.push((span, vec![key])),
        }
    }
    let ranges: Vec<String> = groups.iter().map(|(span, keys)| format!("{} {}", keys.join(", "), span)).collect();
    let ranges = ranges.join("; ");
    format!(
        "One undoable batch; entries apply in order. copy_from reads the source's current state, including earlier \
entries; groups limits the copy (default all but crop). preset names: list_films type presets. set rejects \
out-of-range values; add results clamp to: exposure -5..5 stops; {ranges}; straighten -45..45 degrees; rotation \
0/90/180/270 (crop resets); negative_density -3..3, negative_range 3..10 after inversion. Set only: \
monochrome/flip booleans; film_look name ('' none); curve/rgb_curves [r,g,b], 2-16 [x,y] points 0..1 with x rising \
0 to 1. After set/add: auto_straighten uses Grainy's auto level, auto_crop detects scan borders, then crop \
[left,top,right,bottom] 0..1 in the displayed uncropped frame (null resets crop)."
    )
}

fn annotations(read_only: bool, idempotent: Option<bool>) -> Map<String, Value> {
    let mut a = object(if read_only {
        json!({"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false})
    } else {
        json!({"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false})
    });
    if let Some(i) = idempotent {
        a.insert("idempotentHint".into(), Value::Bool(i));
    }
    a
}

/// Port of `mcp_tools._tool`.
fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Map<String, Value>,
    required: &[&str],
    read_only: bool,
    idempotent: Option<bool>,
) -> Value {
    let mut schema = object(json!({"type": "object", "properties": properties, "additionalProperties": false}));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    let mut notes = object(json!({"title": title}));
    for (k, v) in annotations(read_only, idempotent) {
        notes.insert(k, v);
    }
    json!({"name": name, "title": title, "description": description, "inputSchema": schema, "annotations": notes})
}

fn curve() -> Value {
    json!({"type": "array", "minItems": 2, "maxItems": CURVE_POINTS,
           "items": {"type": "array", "minItems": 2, "maxItems": 2,
                     "items": {"type": "number", "minimum": 0, "maximum": 1}}})
}

/// `mcp_tools.EDIT_ENTRY`: one apply_edits entry.
fn edit_entry() -> Value {
    let mut set_properties = Map::new();
    for &(key, _, _) in &FIELDS {
        let schema = match key {
            "monochrome" | "flip" => json!({"type": "boolean"}),
            "film_look" => json!({"type": "string"}),
            "rotation" => json!({"type": "integer", "enum": [0, 90, 180, 270]}),
            _ => json!({"type": "number"}),
        };
        set_properties.insert(key.into(), schema);
    }
    set_properties.insert("curve".into(), curve());
    set_properties
        .insert("rgb_curves".into(), json!({"type": "array", "minItems": 3, "maxItems": 3, "items": curve()}));
    let mut add_properties = Map::new();
    for &(key, _, _) in &FIELDS {
        if !["monochrome", "film_look", "flip"].contains(&key) {
            add_properties.insert(key.into(), json!({"type": "number"}));
        }
    }
    let extra = object(json!({
        "crop": {"type": ["array", "null"], "minItems": 4, "maxItems": 4,
                 "items": {"type": "number", "minimum": 0, "maximum": 1}},
        "auto_straighten": {"type": "boolean"},
        "auto_crop": {"type": "boolean"},
        "preset": {"type": "string", "minLength": 1, "maxLength": 200},
        "copy_from": {"type": "integer", "minimum": 1, "maximum": MAX_ID},
        "groups": {"type": "array", "minItems": 1, "uniqueItems": true,
                   "items": {"type": "string", "enum": COPY_GROUPS}},
        "set": {"type": "object", "additionalProperties": false, "minProperties": 1, "properties": set_properties},
        "add": {"type": "object", "additionalProperties": false, "minProperties": 1, "properties": add_properties}
    }));
    let properties = merge(&[&target(MAX_TARGETS), &extra]);
    json!({"type": "object", "additionalProperties": false, "properties": properties})
}

fn destination() -> Map<String, Value> {
    object(json!({
        "destination": {"type": "string", "enum": ["default", "last", "path"]},
        "path": {"type": "string", "minLength": 1, "maxLength": 1024},
        "subfolder": {"type": "string", "minLength": 1, "maxLength": 80}
    }))
}

fn label_fields() -> Map<String, Value> {
    let mut labels: Vec<&str> = LABELS.to_vec();
    labels.push("none");
    object(json!({
        "rating": {"type": "integer", "minimum": 0, "maximum": 5},
        "flag": {"type": "string", "enum": FLAGS},
        "label": {"type": "string", "enum": labels}
    }))
}

/// The tool definitions before `_published` trims them: what `mcp_tools.DEFINITIONS` holds while the module
/// builds `SCHEMAS`. The argument checker uses the schemas of these.
fn raw_definitions() -> Vec<Value> {
    let filter = filter();
    let list_photos_properties = merge(&[
        &filter,
        &object(json!({"scope": scope(),
            "group": {"type": "string", "enum": ["photos", "folders"]},
            "sort": {"type": "string", "enum": SORTS},
            "descending": {"type": "boolean"},
            "offset": {"type": "integer", "minimum": 0},
            "limit": {"type": "integer", "minimum": 1, "maximum": 200}})),
    ]);
    let contact_sheet_properties = merge(&[
        &target(SHEET_CELLS),
        &object(json!({
            "offset": {"type": "integer", "minimum": 0},
            "count": {"type": "integer", "minimum": 1, "maximum": SHEET_CELLS},
            "cell": {"type": "integer", "minimum": 96, "maximum": 256},
            "variants": {"type": "array", "minItems": 1, "maxItems": 12,
                "items": {"type": "object", "additionalProperties": false,
                    "properties": {"film_look": {"type": "string", "maxLength": 200},
                                   "preset": {"type": "string", "minLength": 1, "maxLength": 200},
                                   "label": {"type": "string", "maxLength": 80}}}},
            "save": {"type": "boolean"},
            "size": {"type": "integer", "minimum": 512, "maximum": 4000}})),
        &destination(),
    ]);
    let export_properties = merge(&[
        &target(MAX_TARGETS),
        &object(json!({
            "format": {"type": "string", "enum": EXPORT_FORMATS},
            "quality": {"type": "integer", "minimum": 40, "maximum": 100},
            "longest": {"type": "integer", "minimum": 0, "maximum": 30000, "description": "Long side px; 0 = full size."},
            "width": {"type": "integer", "minimum": 1, "maximum": 30000},
            "height": {"type": "integer", "minimum": 1, "maximum": 30000},
            "fit": {"type": "string", "enum": ["inside", "cover"]},
            "color_space": {"type": "string", "enum": COLOR_SPACES},
            "keep_metadata": {"type": "boolean"},
            "naming": {"type": "string", "minLength": 1, "maxLength": 80},
            "dry_run": {"type": "boolean"}})),
        &destination(),
    ]);
    let set_labels_properties = merge(&[
        &target(MAX_TARGETS),
        &label_fields(),
        &object(json!({"entries": {"type": "array", "minItems": 1, "maxItems": 100,
            "items": {"type": "object", "additionalProperties": false,
                      "properties": merge(&[&target(MAX_TARGETS), &label_fields()])}}})),
    ]);
    let target_text = TARGET_TEXT;
    vec![
        tool(
            "list_photos",
            "List photos",
            "List photos in the Grainy library, filtered and paged (default 50 rows). Rows: id, name, r=rating, f=flag \
(1 picked, -1 rejected), l=colour label, v=1 virtual copy, x=1 original missing, d=index into dirs (omitted \
when 0). Zero or empty fields are omitted. Give filters or scope ('selection' or 'current'), not both; no filter \
lists the whole library. group='folders' lists folders with photo counts instead and takes only folder, text \
(part of the folder path), offset and limit.",
            list_photos_properties,
            &[],
            true,
            None,
        ),
        tool(
            "get_photo",
            "Photo details",
            "Details and edit values of up to 50 photos. Folders are in dirs with d as in list_photos; file only \
when it is not name. edits: non-default values (keys apply_edits accepts); other: other changed tool groups; \
off: disabled panels. crop: normalized displayed frame or null; straighten: manual degrees; auto_level: automatic degrees or null.",
            object(json!({"ids": target(50)["ids"]})),
            &["ids"],
            true,
            None,
        ),
        tool(
            "get_preview",
            "Photo previews",
            "Developed JPEG (current edits) of 1-4 photos, size is an upper limit on the long side (default 512); byte limits may shrink it. For many photos use contact_sheet.",
            object(json!({"ids": target(4)["ids"],
                "size": {"type": "integer", "minimum": 128, "maximum": 1568},
                "quality": {"type": "integer", "minimum": 40, "maximum": 90}})),
            &["ids"],
            true,
            None,
        ),
        tool(
            "contact_sheet",
            "Contact sheet",
            &format!(
                "One JPEG, up to 36 photos, labelled id/rating/P/X. {target_text} where/scope: offset/count page \
(default 36); cell upper limit 160 px. variants: one target photo, up to 12 film_look or preset alternatives, \
optional labels; no catalog changes. save=true also writes a new JPEG with file/look names, size upper limit \
4000 px (default 2400), using export_photos destinations; returns path."
            ),
            contact_sheet_properties,
            &[],
            false,
            None,
        ),
        tool(
            "list_films",
            "Films and presets",
            "Film looks for apply_edits film_look: name, iso, type (color, bw or slide) and maker; cine=1 marks \
motion-picture stocks. type filters them (color, bw, slide, cine; default all); type 'presets' lists Grainy's \
develop presets instead (built in, and saved by the person) for apply_edits preset.",
            object(json!({"type": {"type": "string", "enum": FILM_TYPES}})),
            &[],
            true,
            None,
        ),
        tool(
            "apply_edits",
            "Apply edits",
            &edits_description(),
            object(json!({"edits": {"type": "array", "minItems": 1, "maxItems": 100, "items": edit_entry()}})),
            &["edits"],
            false,
            None,
        ),
        tool(
            "auto_tone",
            "Auto tone",
            &format!(
                "Grainy's automatic tone (exposure, contrast, highlights, shadows, whites, blacks), computed separately for each \
photo and applied as one undoable batch. {target_text} Returns the result, or a job id for get_job if it takes \
longer than about 40 s."
            ),
            target(MAX_TARGETS),
            &[],
            false,
            None,
        ),
        tool(
            "auto_color",
            "Auto color",
            &format!(
                "Grainy's automatic colour correction (channel curves) per photo, as one undoable batch. strength 0-100 \
(default: the strength last used in Grainy). {target_text}"
            ),
            merge(&[&target(MAX_TARGETS), &object(json!({"strength": {"type": "integer", "minimum": 0, "maximum": 100}}))]),
            &[],
            false,
            None,
        ),
        tool(
            "invert_negatives",
            "Invert negatives",
            &format!(
                "Turn scanned film negatives into positives, as one undoable batch. roll=true measures the film base and \
densest part over all targeted photos together (frames of one roll scanned with the same settings; at least 2 \
photos); otherwise each photo is measured alone. Refine afterwards with apply_edits (negative_density, \
negative_range), auto_tone and auto_color. {target_text}"
            ),
            merge(&[&target(MAX_TARGETS), &object(json!({"roll": {"type": "boolean"}}))]),
            &[],
            false,
            None,
        ),
        tool(
            "undo_last_batch",
            "Undo last AI batch",
            "Undo the most recent AI batch in this Grainy library (made by any connected AI program), or with redo=true redo \
the batch undone last. Up to 5 batches can be undone in order. A batch can no longer be undone once any of its \
photos changed again. Includes edits and set_labels.",
            object(json!({"batch": {"type": "string", "pattern": "^b[0-9a-f]{6}$"}, "redo": {"type": "boolean"}})),
            &[],
            false,
            None,
        ),
        tool(
            "export_photos",
            "Export photos",
            &format!(
                "Export new files; never overwrite (collision numbers added). Returns files with name/path, w/h and bytes \
(first 100, then more_files), or a job id for get_job. dry_run=true plans names and pixel sizes without writing; \
bytes is null until exported. Defaults: JPEG, sRGB, full size. longest or width/height with fit inside/cover \
(preserves ratio, never enlarges). destination default: AI export folder (Pictures/Grainy); last: person's last \
export folder; path: absolute folder, created if missing. subfolder adds one level. naming: {{stem}}, {{n:02d}}, \
{{film}}, {{preset}}, {{camera}}, {{date}}; missing values drop one separator. JPEG XL: sRGB only. {target_text}"
            ),
            export_properties,
            &[],
            false,
            None,
        ),
        tool(
            "import_folder",
            "Import folder",
            "Import a folder's photos into the library as Grainy's own import does: files already there are skipped; \
originals are only read. path: an absolute folder; recursive: include subfolders (default false). Returns the result, or a job id \
for get_job; it gives progress and, when done, the new photo ids (list_photos with folder and recursive finds them too).",
            object(json!({"path": {"type": "string", "minLength": 1, "maxLength": 1024}, "recursive": {"type": "boolean"}})),
            &["path"],
            false,
            None,
        ),
        tool(
            "create_virtual_copies",
            "Virtual copies",
            "Make count virtual copies of a photo (Grainy's Make Virtual Copy): new library entries of the same original, \
starting from its current edits, e.g. to try different looks. Returns their ids; undo_last_batch does not \
remove them.",
            object(json!({"id": {"type": "integer", "minimum": 1, "maximum": MAX_ID},
                          "count": {"type": "integer", "minimum": 1, "maximum": 20}})),
            &["id", "count"],
            false,
            None,
        ),
        tool(
            "set_labels",
            "Rating, flag, label",
            &format!(
                "One undoable batch of ratings (0 none), flags and/or color labels (none clears). {target_text} Or entries with separate targets/values, applied in order; do not mix both forms. undo_last_batch reverts it."
            ),
            set_labels_properties,
            &[],
            false,
            None,
        ),
        tool(
            "snapshot",
            "Snapshots",
            &format!(
                "Named snapshots of photo edits, as in Grainy's Snapshot menu. save keeps the current edits under name; list \
gives each photo's snapshots, newest first; restore sets the edits back to the newest snapshot of that name as \
one undoable batch. Grainy itself saves a 'Before AI edits' snapshot before a photo's first AI edit. {target_text}"
            ),
            merge(&[
                &target(MAX_TARGETS),
                &object(json!({"action": {"type": "string", "enum": ["save", "restore", "list"]},
                                "name": {"type": "string", "minLength": 1, "maxLength": 80}})),
            ]),
            &["action"],
            false,
            None,
        ),
        tool(
            "open_photo",
            "Open photo",
            "Show a photo in the Grainy window (editor or library view). Does not change the photo.",
            object(json!({"id": {"type": "integer", "minimum": 1, "maximum": MAX_ID},
                          "view": {"type": "string", "enum": ["edit", "library"]}})),
            &["id"],
            false,
            Some(true),
        ),
        tool(
            "get_job",
            "Job progress",
            "Progress or result of a job started by auto_tone, auto_color, invert_negatives, export_photos or import_folder. \
Waits up to wait seconds (default 20) for it to finish.",
            object(json!({"job": {"type": "string", "pattern": "^j[0-9]+$"},
                          "wait": {"type": "integer", "minimum": 0, "maximum": 40}})),
            &["job"],
            true,
            Some(false),
        ),
    ]
}

/// Port of `mcp_tools._published`: the schema as `tools/list` shows it, without `uniqueItems` and `minProperties`,
/// without the `add` limits and without the id maximum (`2**53-1`), so tools/list stays under 24,000 characters.
fn published(schema: &Value, inside_add: bool) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.iter().map(|item| published(item, false)).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                if key == "uniqueItems" || key == "minProperties" {
                    continue;
                }
                if inside_add && (key == "minimum" || key == "maximum") {
                    continue;
                }
                if key == "maximum" && value.as_i64() == Some(MAX_ID) {
                    continue;
                }
                if key == "properties" {
                    if let Value::Object(props) = value {
                        let mut shown = Map::new();
                        for (name, item) in props {
                            shown.insert(name.clone(), published(item, name == "add" || inside_add));
                        }
                        out.insert(key.clone(), Value::Object(shown));
                        continue;
                    }
                }
                out.insert(key.clone(), published(value, inside_add));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn build_definitions() -> Vec<Value> {
    let mut definitions = raw_definitions();
    for definition in &mut definitions {
        let shown = published(&definition["inputSchema"], false);
        definition["inputSchema"] = shown;
    }
    // The curves as tools/list shows them: their point counts and the 0..1 range are in the description.
    if let Some(apply) = definitions.iter_mut().find(|d| d["name"] == "apply_edits") {
        let set = &mut apply["inputSchema"]["properties"]["edits"]["items"]["properties"]["set"]["properties"];
        let simple = json!({"type": "array", "items": {"type": "array", "items": {"type": "number"}}});
        set["curve"] = simple.clone();
        set["rgb_curves"] = json!({"type": "array", "minItems": 3, "maxItems": 3, "items": simple});
    }
    definitions
}

/// `mcp_tools.DEFINITIONS`: the 17 tool definitions as `tools/list` returns them.
pub fn definitions() -> &'static [Value] {
    static DEFINITIONS: OnceLock<Vec<Value>> = OnceLock::new();
    DEFINITIONS.get_or_init(build_definitions)
}

/// `mcp_tools.SCHEMAS`: the input schema of each tool as the argument checker applies it (the full schemas,
/// before `_published` trimmed the ones in `tools/list`), in tool order.
pub fn schemas() -> &'static [(String, Value)] {
    static SCHEMAS: OnceLock<Vec<(String, Value)>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        raw_definitions()
            .into_iter()
            .map(|d| (d["name"].as_str().unwrap_or_default().to_string(), d["inputSchema"].clone()))
            .collect()
    })
}

/// The checker schema of one tool (`SCHEMAS[name]`).
pub fn schema(name: &str) -> Option<&'static Value> {
    schemas().iter().find(|(n, _)| n == name).map(|(_, s)| s)
}

// --- results ----------------------------------------------------------------------------------

/// Port of `mcp_tools.text_block`: `{'type':'text','text': compact JSON}` (non-ASCII kept; the HTTP body escapes
/// it again in `dumps`). Compact means separators `,` and `:`.
pub fn text_block(value: &Value) -> Value {
    let mut text = String::new();
    write_unescaped(&mut text, value);
    json!({"type": "text", "text": text})
}

/// `json.dumps(value, separators=(',', ':'), ensure_ascii=False)`: like `dumps`, but non-ASCII stays as it is.
fn write_unescaped(out: &mut String, value: &Value) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_unescaped(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string_unescaped(out, key);
                out.push(':');
                write_unescaped(out, item);
            }
            out.push('}');
        }
        Value::String(s) => write_string_unescaped(out, s),
        other => out.push_str(&String::from_utf8(crate::pyjson::dumps(other)).unwrap_or_default()),
    }
}

fn write_string_unescaped(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Port of `mcp_tools.image_block`: a base64 JPEG content block.
pub fn image_block(data: &[u8]) -> Value {
    json!({"type": "image", "data": crate::b64::encode(data), "mimeType": "image/jpeg"})
}

/// Port of `mcp_tools.failure`: a tool result that is an error with one text block.
pub fn failure(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

// --- the Tools trait ----------------------------------------------------------------------------

/// What `tools.call` raising an exception means to the protocol: the request is answered with JSON-RPC
/// error -32603 "Internal error". (Tool failures a caller can correct are normal results with `isError`.)
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("internal error: {0}")]
pub struct ToolFault(pub String);

/// The result of a tool call: the `{content, isError}` object, or a fault.
pub type ToolResult = Result<Value, ToolFault>;

/// What the protocol layer needs from the tool implementations (the oracle's `tools` object): the definitions
/// `tools/list` returns and the call. The real implementation (catalog and engine) comes later; tests use
/// [`crate::stub::StubTools`].
pub trait Tools {
    /// `tools.definitions()`: the published tool definitions.
    fn definitions(&self) -> &[Value];
    /// `tools.call(name, arguments, modern=, deadline=)`: `name` is a known tool and `arguments` an object.
    fn call(&self, name: &str, arguments: &Value, modern: bool, deadline: Instant) -> ToolResult;
}

/// A failure inside a tool implementation, with the mapping `mcp_tools.Tools.call` gives each kind.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolError {
    /// `ServiceError`: a request the caller can correct (text for the AI program).
    Service(String),
    /// `Busy`: Grainy did not start the request; the text completes "Grainy is busy: ...".
    Busy(String),
    /// `Unfinished`: GUI work started but did not finish before the deadline.
    Unfinished,
    /// `JobFailure`: a failed or cancelled job; `message` is its text and `view` the job's `view()` object.
    Job { message: String, view: Value },
    /// Any other exception: `last` is `photo_service.english_error` of it.
    Other { last: String },
}

/// Port of `photo_service.english_error` for the error kinds that exist in Rust: stable English text.
pub fn english_error(kind: ErrorKind<'_>) -> String {
    match kind {
        ErrorKind::PermissionDenied => "permission denied".into(),
        ErrorKind::NotFound => "file or folder not found".into(),
        ErrorKind::AlreadyExists => "output already exists; try again for a new name".into(),
        ErrorKind::Io => "file system operation failed".into(),
        ErrorKind::Named(name) => format!("operation failed ({name})"),
        ErrorKind::Internal => "operation failed (internal error)".into(),
    }
}

/// The kinds `english_error` tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind<'a> {
    /// `PermissionError`.
    PermissionDenied,
    /// `FileNotFoundError`.
    NotFound,
    /// `FileExistsError`.
    AlreadyExists,
    /// Any other `OSError`.
    Io,
    /// Another exception, by its class name.
    Named(&'a str),
    /// Not an exception at all.
    Internal,
}

/// Port of the wrapper in `mcp_tools.Tools.call`: validates `arguments` against the tool's schema, runs the
/// implementation and maps its outcome to the `{content, isError}` result exactly as the oracle does. `run`
/// returns the content blocks. An unknown `name` is a fault (the protocol layer has checked it already).
pub fn call_checked(name: &str, arguments: &Value, run: impl FnOnce() -> Result<Vec<Value>, ToolError>) -> ToolResult {
    let Some(schema) = schema(name) else { return Err(ToolFault(format!("no such tool: {name}"))) };
    let outcome = match check(schema, arguments, "arguments") {
        Err(ServiceError(text)) => Err(ToolError::Service(text)),
        Ok(()) => run(),
    };
    Ok(match outcome {
        Ok(content) => json!({"content": content, "isError": false}),
        Err(ToolError::Job { message, view }) => {
            json!({"content": [{"type": "text", "text": message}, text_block(&view)], "isError": true})
        }
        Err(ToolError::Service(text)) => failure(&text),
        Err(ToolError::Busy(text)) => {
            failure(&format!("Grainy is busy: {text}. Nothing was changed; try again in a moment."))
        }
        Err(ToolError::Unfinished) => failure(UNFINISHED),
        Err(ToolError::Other { last }) => failure(&format!("Grainy could not finish this call: {last}")),
    })
}
