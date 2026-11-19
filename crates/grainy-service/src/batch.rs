//! Port of `luma/command_batch.py`: frozen command targets and atomic catalogue-only edits with guarded batch
//! undo, in the full (v1) and compact (v2) record formats.

use grainy_catalog::{rusqlite, workflow::text_hash, Catalog};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::panels::{changed_keys, switch_on};
use crate::pyutil::{dumps, eq, get, parent, suffix};
use crate::{id_of, normalized, object, profiles, text_of, Conflict, Error, Result, VIDEO_EXTENSIONS};

/// Port of `command_batch.BATCH_KEY`.
pub const BATCH_KEY: &str = "undo:command_batch";
/// Port of `command_batch.SCOPE_LABELS`.
pub const SCOPE_LABELS: [(&str, &str); 3] =
    [("current", "현재 사진"), ("selection", "선택한 사진"), ("folder", "현재 폴더 전체")];
/// Port of `command_batch.UNDO_DEPTH`: per-photo undo entries kept, as the editor does.
pub const UNDO_DEPTH: usize = 80;
/// Port of `command_batch.CHANGED`.
pub const CHANGED: &str = "대상 사진 또는 보정값이 바뀌었습니다. 다시 요청해 주세요. 변경한 사진은 없습니다.";
/// Port of `command_batch.HISTORY`.
pub const HISTORY: &str = "대상 사진의 편집 이력이 바뀌었습니다. 개별 사진의 실행 취소를 사용해 주세요.";

/// A callback run inside the batch transaction with the batch's photo ids.
pub type Hook<'a> = &'a mut dyn FnMut(&[i64]) -> Result<()>;

fn conflict(message: &str, kind: Conflict) -> Error {
    Error::Conflict { message: message.into(), kind }
}

/// `catalog.db.in_transaction`.
pub fn in_transaction(catalog: &Catalog) -> bool {
    !catalog.connection().is_autocommit()
}

/// Port of command_batch's BEGIN IMMEDIATE transaction.
pub(crate) fn immediate<T>(catalog: &Catalog, f: impl FnOnce() -> Result<T>) -> Result<T> {
    grainy_catalog::transaction::run(catalog.connection(), true, f)
}

/// Port of `command_batch.capture`: the current photo, the selection and the current folder, frozen.
pub fn capture(
    catalog: &Catalog,
    current_id: Option<i64>,
    selected_ids: &[i64],
    folder: Option<&str>,
) -> Result<Value> {
    let current = match current_id.filter(|i| *i != 0) {
        Some(id) => catalog.photo(id)?,
        None => None,
    };
    let folder = match folder.filter(|f| !f.is_empty()) {
        Some(f) => Some(f.to_owned()),
        None => match &current {
            Some(c) => Some(parent(text_of(c, "path")?)),
            None => None,
        },
    };
    let platform = catalog.platform();
    let mut folders = Vec::new();
    if let Some(f) = &folder {
        for row in catalog.rows("SELECT id,path FROM photos ORDER BY id", [])? {
            let path = text_of(&row, "path")?;
            if platform.in_folder(path, f, false) && !VIDEO_EXTENSIONS.contains(&suffix(path).to_lowercase().as_str()) {
                folders.push(id_of(&row)?);
            }
        }
    }
    let mut selection: Vec<i64> = Vec::new();
    for i in selected_ids {
        if !selection.contains(i) {
            selection.push(*i);
        }
    }
    let current_scope: Vec<i64> = match (&current, current_id) {
        (Some(_), Some(id)) => vec![id],
        _ => vec![],
    };
    let scopes = [("current", current_scope), ("selection", selection), ("folder", folders)];
    let mut ids: Vec<i64> = scopes.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut records = Map::new();
    for ident in ids {
        if let Some(row) = catalog.photo(ident)? {
            records.insert(
                ident.to_string(),
                json!({"id": ident, "path": get(&row, "path"), "settings": get(&row, "settings")}),
            );
        }
    }
    let mut scope_map = Map::new();
    for (key, values) in scopes {
        let kept: Vec<i64> = values.into_iter().filter(|i| records.contains_key(&i.to_string())).collect();
        scope_map.insert(key.into(), json!(kept));
    }
    Ok(json!({
        "photo_id": current_id,
        "settings": current.as_ref().map_or_else(|| json!({}), |c| get(c, "settings").clone()),
        "folder": folder,
        "scopes": scope_map,
        "records": records,
    }))
}

/// Port of `command_batch.targets`: the frozen records of one scope.
pub fn targets(snapshot: Option<&Value>, scope: &str) -> Result<Vec<Value>> {
    let Some(snapshot) = snapshot.filter(|s| crate::pyutil::truthy(s)) else {
        return Err(Error::Value("적용할 사진을 먼저 선택해 주세요.".into()));
    };
    let ids = snapshot.get("scopes").and_then(|s| s.get(scope)).and_then(Value::as_array).cloned().unwrap_or_default();
    if ids.is_empty() {
        return Err(Error::Value("해당 범위에 적용할 사진이 없습니다.".into()));
    }
    ids.iter()
        .map(|i| {
            let key = crate::pyutil::text(i);
            snapshot["records"].get(&key).cloned().ok_or_else(|| Error::Key(key))
        })
        .collect()
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Port of `command_batch.describe`: the scope and its photo count for the confirmation text.
pub fn describe(snapshot: &Value, scope: &str) -> Result<String> {
    let count = snapshot.get("scopes").and_then(|s| s.get(scope)).and_then(Value::as_array).map_or(0, Vec::len);
    let label = SCOPE_LABELS.iter().find(|(k, _)| *k == scope).ok_or_else(|| Error::Key(crate::pyutil::repr(scope)))?.1;
    let mut text = format!("{label} · {}장", thousands(count));
    if scope == "folder" {
        let folder = match snapshot.get("folder") {
            Some(f) if crate::pyutil::truthy(f) => crate::pyutil::text(f),
            _ => "폴더 없음".into(),
        };
        text += &format!("\n{folder}\n필터에 가려진 사진 포함 · 하위 폴더·영상 제외");
    }
    Ok(text)
}

/// Port of `command_batch.verify`: every record's photo still has its path and settings.
pub fn verify(catalog: &Catalog, records: &[Value]) -> Result<()> {
    for item in records {
        let now = catalog.photo(id_of(item)?)?;
        let same = now.as_ref().is_some_and(|now| {
            get(now, "path") == get(item, "path") && eq(get(now, "settings"), get(item, "settings"))
        });
        if !same {
            return Err(conflict(CHANGED, Conflict::Changed));
        }
    }
    Ok(())
}

fn preference_write(catalog: &Catalog, key: &str, value: &Value) -> Result<()> {
    let text = profiles::dumps(catalog.connection(), value)?;
    catalog.connection().execute("INSERT OR REPLACE INTO preferences VALUES(?,?)", params![key, text])?;
    Ok(())
}

fn preference(catalog: &Catalog, key: &str, default: Value) -> Result<Value> {
    let text: Option<String> =
        catalog.connection().query_row("SELECT value FROM preferences WHERE key=?", [key], |r| r.get(0)).optional()?;
    match text {
        Some(t) => profiles::loads(catalog.connection(), &t),
        None => Ok(default),
    }
}

/// `command_batch._extras`: the photo's user metadata with a new edit time.
fn extras(catalog: &Catalog, ident: i64, stamp: &str) -> Result<String> {
    let row: Option<Option<String>> =
        catalog.connection().query_row("SELECT extras FROM photos WHERE id=?", [ident], |r| r.get(0)).optional()?;
    let mut value: Value = match row {
        Some(Some(text)) if !text.is_empty() => serde_json::from_str(&text)?,
        _ => json!({}),
    };
    object(&value)?;
    value["_last_edit_time"] = stamp.into();
    Ok(dumps(&value, false))
}

/// `command_batch._undo_text`: stored text of the photo's undo preference (the empty state when it has none).
pub(crate) fn undo_text(catalog: &Catalog, ident: i64) -> Result<String> {
    let row: Option<String> = catalog
        .connection()
        .query_row("SELECT value FROM preferences WHERE key=?", [format!("undo:{ident}")], |r| r.get(0))
        .optional()?;
    match row {
        Some(text) => Ok(text),
        None => profiles::dumps(catalog.connection(), &json!({"undo": [], "redo": []})),
    }
}

fn keys(value: &Value) -> Vec<String> {
    value.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// `command_batch._diff`: entry as a change of base; `order` only when entry's keys are not in base's order.
fn diff(base: &Value, entry: &Value) -> Result<Value> {
    let (b, e) = (object(base)?, object(entry)?);
    let set: Map<String, Value> = e
        .iter()
        .filter(|(k, v)| b.get(*k).is_none_or(|old| !eq(old, v)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let drop: Vec<Value> = b.keys().filter(|k| !e.contains_key(*k)).map(|k| Value::String(k.clone())).collect();
    let mut change = json!({"set": set, "drop": drop});
    if keys(&undiff(base, &change)?) != keys(entry) {
        change["order"] = json!(keys(entry));
    }
    Ok(change)
}

/// `command_batch._undiff`.
fn undiff(base: &Value, change: &Value) -> Result<Value> {
    let drop: Vec<&str> = change
        .get("drop")
        .and_then(Value::as_array)
        .map(|d| d.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut result: Map<String, Value> = object(base)?
        .iter()
        .filter(|(k, _)| !drop.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(set) = change.get("set").and_then(Value::as_object) {
        for (k, v) in set {
            result.insert(k.clone(), v.clone());
        }
    }
    if let Some(order) = change.get("order").and_then(Value::as_array) {
        let mut ordered = Map::new();
        for k in order {
            let k = crate::pyutil::text(k);
            let v = result.get(&k).cloned().ok_or_else(|| Error::Key(crate::pyutil::repr(&k)))?;
            ordered.insert(k, v);
        }
        result = ordered;
    }
    Ok(Value::Object(result))
}

/// `command_batch._state`: an undo preference in its original key order.
fn state(keys: &[Value], extra: &Value, undo: Value, redo: Value) -> Result<Value> {
    let mut out = Map::new();
    for key in keys {
        let key = crate::pyutil::text(key);
        let value = match key.as_str() {
            "undo" => undo.clone(),
            "redo" => redo.clone(),
            _ => extra.get(&key).cloned().ok_or_else(|| Error::Key(crate::pyutil::repr(&key)))?,
        };
        out.insert(key, value);
    }
    Ok(Value::Object(out))
}

fn list(value: &Value, key: &str) -> Vec<Value> {
    value.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn head_len(undo_len: usize) -> usize {
    (undo_len + 1).saturating_sub(UNDO_DEPTH)
}

/// `command_batch._compact`: one photo of a v2 group.
fn compact(item: &Value, value: &Value, before: &Value, before_text: &str, after_text: &str) -> Result<Value> {
    let old = get(item, "settings");
    let change = diff(old, value)?;
    if !eq(&undiff(old, &change)?, value) {
        return Err(Error::Runtime("compact batch record does not rebuild the new settings".into()));
    }
    let undo = list(before, "undo");
    let head = &undo[..head_len(undo.len()).min(undo.len())];
    let mut record = Map::new();
    record.insert("id".into(), get(item, "id").clone());
    record.insert("path".into(), get(item, "path").clone());
    record.insert("before".into(), old.clone());
    record.insert("after_set".into(), change["set"].clone());
    record.insert("after_drop".into(), change["drop"].clone());
    if let Some(order) = change.get("order") {
        record.insert("after_order".into(), order.clone());
    }
    record.insert("undo_head".into(), Value::Array(head.iter().map(|e| diff(old, e)).collect::<Result<_>>()?));
    record.insert("redo_before".into(), before.get("redo").cloned().unwrap_or_else(|| json!([])));
    let extra: Map<String, Value> = object(before)?
        .iter()
        .filter(|(k, _)| *k != "undo" && *k != "redo")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    record.insert("undo_extra".into(), Value::Object(extra));
    record.insert("undo_keys".into(), json!(keys(before)));
    record.insert("h_before".into(), text_hash(before_text).into());
    record.insert("h_after".into(), text_hash(after_text).into());
    Ok(Value::Object(record))
}

fn rebuilt_after(item: &Value) -> Result<Value> {
    let mut change = json!({"set": get(item, "after_set"), "drop": get(item, "after_drop")});
    if let Some(order) = item.get("after_order") {
        change["order"] = order.clone();
    }
    undiff(get(item, "before"), &change)
}

/// Python `text[a:len(text)-b]` (empty when the ends overlap).
fn middle(text: &str, a: usize, b: usize) -> &str {
    if a + b > text.len() {
        ""
    } else {
        &text[a..text.len() - b]
    }
}

/// `command_batch._after_by_text`: apply()'s new undo state as text, from the stored text.
fn after_by_text(catalog: &Catalog, before: &Value, before_text: &str, settings: &Value) -> Result<Option<String>> {
    let db = catalog.connection();
    if keys(before) != ["undo", "redo"] {
        return Ok(None);
    }
    let prefix = "{\"undo\": [";
    let suffix = format!("], \"redo\": {}}}", profiles::dumps(db, &before["redo"])?);
    if !(before_text.starts_with(prefix) && before_text.ends_with(&suffix)) {
        return Ok(None);
    }
    let mut inner = middle(before_text, prefix.len(), suffix.len()).to_owned();
    let undo = list(before, "undo");
    let cut = head_len(undo.len());
    for entry in &undo[..cut.min(undo.len())] {
        let first = profiles::dumps(db, entry)?;
        if inner == first {
            inner.clear();
        } else if let Some(rest) = inner.strip_prefix(&format!("{first}, ")) {
            inner = rest.to_owned();
        } else {
            return Ok(None);
        }
    }
    if !inner.is_empty() != (undo.len() > cut) {
        return Ok(None);
    }
    let joined = if inner.is_empty() { String::new() } else { format!("{inner}, ") };
    Ok(Some(format!("{prefix}{joined}{}], \"redo\": []}}", profiles::dumps(db, settings)?)))
}

/// `command_batch._undo_by_text`: the undo state before the batch, rebuilt from the state the batch wrote.
fn undo_by_text(catalog: &Catalog, item: &Value, text: &str) -> Result<Option<String>> {
    let db = catalog.connection();
    let (prefix, suffix) = ("{\"undo\": [", "], \"redo\": []}");
    if !(text.starts_with(prefix) && text.ends_with(suffix)) {
        return Ok(None);
    }
    let inner = middle(text, prefix.len(), suffix.len());
    let last = profiles::dumps(db, get(item, "before"))?;
    let rest = if inner == last {
        ""
    } else if let Some(rest) = inner.strip_suffix(&format!(", {last}")) {
        rest
    } else {
        return Ok(None);
    };
    let mut entries = Vec::new();
    for change in list(item, "undo_head") {
        entries.push(profiles::dumps(db, &undiff(get(item, "before"), &change)?)?);
    }
    if !rest.is_empty() {
        entries.push(rest.to_owned());
    }
    let mut parts = Vec::new();
    for key in list(item, "undo_keys") {
        let key = crate::pyutil::text(&key);
        let value = match key.as_str() {
            "undo" => format!("[{}]", entries.join(", ")),
            "redo" => profiles::dumps(db, get(item, "redo_before"))?,
            _ => profiles::dumps(
                db,
                item.get("undo_extra")
                    .and_then(|e| e.get(&key))
                    .ok_or_else(|| Error::Key(crate::pyutil::repr(&key)))?,
            )?,
        };
        parts.push(format!("{}: {value}", dumps(&Value::String(key), true)));
    }
    Ok(Some(format!("{{{}}}", parts.join(", "))))
}

/// `command_batch._redo_by_text`: the state the batch wrote, rebuilt from the state before it.
fn redo_by_text(catalog: &Catalog, item: &Value, text: &str) -> Result<Option<String>> {
    let db = catalog.connection();
    if list(item, "undo_keys") != [json!("undo"), json!("redo")] {
        return Ok(None);
    }
    let prefix = "{\"undo\": [";
    let suffix = format!("], \"redo\": {}}}", profiles::dumps(db, get(item, "redo_before"))?);
    if !(text.starts_with(prefix) && text.ends_with(&suffix)) {
        return Ok(None);
    }
    let mut inner = middle(text, prefix.len(), suffix.len()).to_owned();
    for change in list(item, "undo_head") {
        let first = profiles::dumps(db, &undiff(get(item, "before"), &change)?)?;
        if inner == first {
            inner.clear();
        } else if let Some(rest) = inner.strip_prefix(&format!("{first}, ")) {
            inner = rest.to_owned();
        } else {
            return Ok(None);
        }
    }
    let joined = if inner.is_empty() { String::new() } else { format!("{inner}, ") };
    Ok(Some(format!("{prefix}{joined}{}], \"redo\": []}}", profiles::dumps(db, get(item, "before"))?)))
}

/// Options of [`apply`], with the oracle's defaults.
#[derive(Clone, Debug)]
pub struct ApplyOptions {
    /// History label.
    pub label: String,
    /// Keep the batch for [`restore`].
    pub remember: bool,
    /// The preference key of the batch record.
    pub batch_key: String,
    /// Merged into the record (restore keeps it).
    pub meta: Option<Map<String, Value>>,
    /// Write a v2 (compact) record.
    pub compact: bool,
    /// Switch on panels the values change (`False` for settings that bring their own panel states).
    pub switch: bool,
}
impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            label: "Codex 일괄 명령".into(),
            remember: true,
            batch_key: BATCH_KEY.into(),
            meta: None,
            compact: false,
            switch: true,
        }
    }
}

/// Port of `command_batch.apply`: write every changed photo in one transaction; the changed ids.
/// `after_apply` runs inside the transaction with the changed ids.
pub fn apply(
    catalog: &Catalog,
    records: &[Value],
    values: &[Value],
    options: &ApplyOptions,
    after_apply: Option<Hook<'_>>,
) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for r in records {
        ids.push(id_of(r)?);
    }
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    if records.len() != values.len() || unique.len() != records.len() {
        return Err(Error::Value("일괄 보정 대상이 올바르지 않습니다.".into()));
    }
    if in_transaction(catalog) {
        return Err(Error::Runtime("다른 저장 작업이 진행 중입니다.".into()));
    }
    let now = catalog.now();
    let db = catalog.connection();
    immediate(catalog, || {
        verify(catalog, records)?;
        let mut changed: Vec<Value> = Vec::new();
        for (item, value) in records.iter().zip(values) {
            let mut value = normalized(value)?;
            let old = get(item, "settings");
            if options.switch {
                let keys = changed_keys(object(old)?, object(&value)?);
                if let Value::Object(map) = &mut value {
                    switch_on(map, &keys);
                }
            }
            if eq(&value, old) {
                continue;
            }
            let ident = id_of(item)?;
            let (before, before_text) = if options.compact {
                let text = undo_text(catalog, ident)?;
                (profiles::loads(db, &text)?, text)
            } else {
                (preference(catalog, &format!("undo:{ident}"), json!({"undo": [], "redo": []}))?, String::new())
            };
            let mut undo = list(&before, "undo");
            undo.push(old.clone());
            let start = undo.len().saturating_sub(UNDO_DEPTH);
            let after = json!({"undo": undo[start..].to_vec(), "redo": []});
            db.execute(
                "INSERT INTO history(photo_id,label,edits,created) VALUES(?,?,?,?)",
                params![ident, options.label, profiles::dumps(db, old)?, now],
            )?;
            db.execute(
                "UPDATE photos SET edits=?,extras=? WHERE id=?",
                params![profiles::dumps(db, &value)?, extras(catalog, ident, &now)?, ident],
            )?;
            let by_text = if options.compact { after_by_text(catalog, &before, &before_text, old)? } else { None };
            let after_text = match by_text {
                Some(t) if !t.is_empty() => t,
                _ => profiles::dumps(db, &after)?,
            };
            db.execute("INSERT OR REPLACE INTO preferences VALUES(?,?)", params![format!("undo:{ident}"), after_text])?;
            if options.compact {
                changed.push(compact(item, &value, &before, &before_text, &after_text)?);
            } else {
                changed.push(json!({"id": ident, "path": get(item, "path"), "before": old, "after": value,
                                    "undo_before": before, "undo_after": after}));
            }
        }
        if options.remember && !changed.is_empty() {
            let mut group = options.meta.clone().unwrap_or_default();
            if options.compact {
                group.insert("v".into(), 2.into());
            }
            group.insert("state".into(), "applied".into());
            group.insert("records".into(), Value::Array(changed.clone()));
            preference_write(catalog, &options.batch_key, &Value::Object(group))?;
        }
        let ids: Vec<i64> = changed.iter().map(id_of).collect::<Result<_>>()?;
        if let Some(f) = after_apply {
            f(&ids)?;
        }
        Ok(ids)
    })
}

fn restore_compact(catalog: &Catalog, records: &[Value], redo: bool) -> Result<Vec<(i64, Value, String)>> {
    let db = catalog.connection();
    let mut checks = Vec::new();
    for r in records {
        let settings = if redo { get(r, "before").clone() } else { rebuilt_after(r)? };
        checks.push(json!({"id": get(r, "id"), "path": get(r, "path"), "settings": settings}));
    }
    verify(catalog, &checks)?;
    let mut writes = Vec::new();
    for item in records {
        let ident = id_of(item)?;
        let current_text = undo_text(catalog, ident)?;
        let (h_before, h_after) = (get(item, "h_before"), get(item, "h_after"));
        let current_hash = Value::String(text_hash(&current_text));
        if current_hash != *(if redo { h_before } else { h_after }) {
            return Err(conflict(HISTORY, Conflict::History));
        }
        let expected = if redo { h_after } else { h_before };
        let rebuilt = if redo {
            redo_by_text(catalog, item, &current_text)?
        } else {
            undo_by_text(catalog, item, &current_text)?
        };
        let text = match rebuilt {
            Some(t) if Value::String(text_hash(&t)) == *expected => t,
            _ => {
                let current = profiles::loads(db, &current_text)?;
                let state = if redo {
                    let mut undo = list(&current, "undo");
                    undo.push(get(item, "before").clone());
                    let start = undo.len().saturating_sub(UNDO_DEPTH);
                    json!({"undo": undo[start..].to_vec(), "redo": []})
                } else {
                    let mut undo = Vec::new();
                    for change in list(item, "undo_head") {
                        undo.push(undiff(get(item, "before"), &change)?);
                    }
                    let mut tail = list(&current, "undo");
                    tail.pop();
                    undo.extend(tail);
                    state(
                        &list(item, "undo_keys"),
                        get(item, "undo_extra"),
                        Value::Array(undo),
                        get(item, "redo_before").clone(),
                    )?
                };
                profiles::dumps(db, &state)?
            }
        };
        if Value::String(text_hash(&text)) != *expected {
            return Err(conflict(HISTORY, Conflict::History));
        }
        let settings = if redo { rebuilt_after(item)? } else { get(item, "before").clone() };
        writes.push((ident, settings, text));
    }
    Ok(writes)
}

/// Port of `command_batch.restore`: undo (or redo) the batch under `batch_key` only if no member's edits or undo
/// history changed. `expect` is the batch's id. `after_restore` runs inside the transaction.
pub fn restore(
    catalog: &Catalog,
    redo: bool,
    batch_key: &str,
    after_restore: Option<Hook<'_>>,
    expect: Option<&Value>,
) -> Result<Vec<i64>> {
    if in_transaction(catalog) {
        return Err(Error::Runtime("다른 저장 작업이 진행 중입니다.".into()));
    }
    let db = catalog.connection();
    immediate(catalog, || {
        let mut group = preference(catalog, batch_key, json!({}))?;
        let wanted = if redo { "undone" } else { "applied" };
        if group.get("state").and_then(Value::as_str) != Some(wanted) {
            let message = if redo {
                "다시 실행할 일괄 작업이 없습니다."
            } else {
                "실행 취소할 일괄 작업이 없습니다."
            };
            return Err(conflict(message, Conflict::None));
        }
        if let Some(expect) = expect {
            if !eq(get(&group, "id"), expect) {
                return Err(conflict("실행 취소할 일괄 작업이 없습니다.", Conflict::None));
            }
        }
        let records =
            group.get("records").and_then(Value::as_array).cloned().ok_or_else(|| Error::Key("'records'".into()))?;
        let writes = if eq(get(&group, "v"), &json!(2)) {
            restore_compact(catalog, &records, redo)?
        } else {
            let (side, destination) = if redo { ("before", "after") } else { ("after", "before") };
            let checks: Vec<Value> = records
                .iter()
                .map(|r| json!({"id": get(r, "id"), "path": get(r, "path"), "settings": get(r, side)}))
                .collect();
            verify(catalog, &checks)?;
            for item in &records {
                let stored = preference(catalog, &format!("undo:{}", id_of(item)?), json!({"undo": [], "redo": []}))?;
                if !eq(&stored, get(item, &format!("undo_{side}"))) {
                    return Err(conflict(HISTORY, Conflict::History));
                }
            }
            let mut writes = Vec::new();
            for item in &records {
                writes.push((
                    id_of(item)?,
                    get(item, destination).clone(),
                    profiles::dumps(db, get(item, &format!("undo_{destination}")))?,
                ));
            }
            writes
        };
        let stamp = catalog.now();
        for (ident, settings, state) in &writes {
            db.execute(
                "UPDATE photos SET edits=?,extras=? WHERE id=?",
                params![profiles::dumps(db, settings)?, extras(catalog, *ident, &stamp)?, ident],
            )?;
            db.execute("INSERT OR REPLACE INTO preferences VALUES(?,?)", params![format!("undo:{ident}"), state])?;
        }
        group["state"] = (if redo { "applied" } else { "undone" }).into();
        preference_write(catalog, batch_key, &group)?;
        let ids: Vec<i64> = records.iter().map(id_of).collect::<Result<_>>()?;
        if let Some(f) = after_restore {
            f(&ids)?;
        }
        Ok(ids)
    })
}
