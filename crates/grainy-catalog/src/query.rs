//! Port of `luma/library_query.py` and the predicates it calls in `library.matches`.
use crate::{folders::Platform, integer, invalid, json, object, row_value, string, workflow, Result};
use chrono::{Datelike, Duration as Delta, NaiveDate, NaiveDateTime};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use unicode_casefold::UnicodeCaseFold;

pub(crate) fn fold(s: &str) -> String {
    s.case_fold().collect()
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(crate::Error::Cancelled)
    } else {
        Ok(())
    }
}
fn connect(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(10))?;
    Ok(db)
}
fn rows(db: &Connection, sql: &str) -> Result<Vec<Value>> {
    Ok(db.prepare(sql)?.query_map([], row_value)?.collect::<rusqlite::Result<_>>()?)
}
fn info(p: &Value) -> Result<Value> {
    let mut out = serde_json::Map::new();
    for key in ["metadata", "extras"] {
        let v: Value = serde_json::from_str(p[key].as_str().filter(|s| !s.is_empty()).unwrap_or("{}"))?;
        out.extend(object(&v)?.clone());
    }
    Ok(out.into())
}

/// Port of `library_query.sync_index`; each 512-row batch is one IMMEDIATE transaction.
pub fn sync_index(db: &Connection, cancel: &AtomicBool) -> Result<()> {
    sync_index_on(Platform::native(), db, cancel)
}

/// [`sync_index`] with the path rules of `platform`.
pub fn sync_index_on(platform: Platform, db: &Connection, cancel: &AtomicBool) -> Result<()> {
    loop {
        cancelled(cancel)?;
        db.execute_batch("BEGIN IMMEDIATE")?;
        let work = (|| {
            let rows=rows(db,"SELECT p.id,p.path,p.name,p.keywords,p.metadata,p.extras FROM browser_dirty d JOIN photos p ON p.id=d.id LIMIT 512")?;
            for p in &rows {
                let folder = platform.parent(string(&p["path"])?);
                let i = info(p)?;
                let date = json::text(i.get("DateTimeOriginal").or_else(|| i.get("date")).unwrap_or(&json!("")))
                    .replacen(':', "-", 2);
                db.execute(
                    "INSERT OR REPLACE INTO browser_index VALUES(?,?,?,?,?,?)",
                    params![
                        integer(&p["id"])?,
                        folder,
                        platform.path_key(&folder),
                        fold(string(&p["name"])?),
                        fold(string(&p["keywords"])?),
                        date
                    ],
                )?;
                db.execute("DELETE FROM browser_dirty WHERE id=?", [integer(&p["id"])?])?;
            }
            Ok::<_, crate::Error>(rows.is_empty())
        })();
        match work {
            Ok(empty) => {
                db.execute_batch("COMMIT")?;
                if empty {
                    return Ok(());
                }
            }
            Err(e) => {
                let _ = db.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
    }
}

/// Port of `library_query.query`; the caller owns cancellation and the query's local clock.
pub fn query(path: impl AsRef<Path>, options: &Value, cancel: &Arc<AtomicBool>, now: NaiveDateTime) -> Result<Value> {
    query_on(Platform::native(), path, options, cancel, now)
}

/// [`query`] with the path rules of `platform` (parity tests of a session the oracle ran on Windows).
pub fn query_on(
    platform: Platform,
    path: impl AsRef<Path>,
    options: &Value,
    cancel: &Arc<AtomicBool>,
    now: NaiveDateTime,
) -> Result<Value> {
    let result = query_inner(platform, path.as_ref(), options, cancel, now);
    if cancel.load(Ordering::Relaxed) {
        Err(crate::Error::Cancelled)
    } else {
        result
    }
}
fn query_inner(
    platform: Platform,
    path: &Path,
    options: &Value,
    cancel: &Arc<AtomicBool>,
    now: NaiveDateTime,
) -> Result<Value> {
    object(options)?;
    let db = connect(path)?;
    sync_index_on(platform, &db, cancel)?;
    let progress_cancel = Arc::clone(cancel);
    db.progress_handler(1000, Some(move || progress_cancel.load(Ordering::Relaxed)))?;
    db.execute_batch("BEGIN")?;
    let search = fold(options["search"].as_str().unwrap_or(""));
    let folder = options["folder"].as_str().filter(|s| !s.is_empty());
    let mut rules = vec![options.get("rules").filter(|v| !v.is_null()).cloned().unwrap_or(json!({}))];
    let mut membership = None;
    if let Some(id) = options["collection"].as_i64() {
        let r: Option<Option<String>> =
            db.query_row("SELECT rules FROM collections WHERE id=?", [id], |r| r.get(0)).optional()?;
        match r {
            None => membership = Some(std::collections::BTreeSet::new()),
            Some(Some(text)) => rules.push(serde_json::from_str(&text)?),
            Some(None) => {
                let ids = db
                    .prepare("SELECT photo_id FROM collection_members WHERE collection_id=?")?
                    .query_map([id], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
                membership = Some(ids);
            }
        }
    }
    let sort = match options["sort"].as_str() {
        Some("name") => "i.name_key",
        Some("capture") => "i.capture_key",
        Some("rating") => "p.rating",
        _ => "p.id",
    };
    let direction = if grainy_core::settings::truthy(&options["descending"]) { "DESC" } else { "ASC" };
    let mut statement=db.prepare(&format!("SELECT p.id,p.path,p.name,p.keywords,p.rating,p.flag,p.label,p.virtual_source,p.metadata,p.extras,p.imported,p.stack_id,i.folder_key,i.name_key,i.keywords_key FROM photos p JOIN browser_index i ON i.id=p.id ORDER BY {sort} {direction},p.id {direction}"))?;
    let mut cursor = statement.query([])?;
    let mut selected = Vec::new();
    while let Some(row) = cursor.next()? {
        cancelled(cancel)?;
        let p = row_value(row)?;
        let id = integer(&p["id"])?;
        if membership.as_ref().is_some_and(|s| !s.contains(&id)) {
            continue;
        }
        if !search.is_empty()
            && !string(&p["name_key"])?.contains(&search)
            && !string(&p["keywords_key"])?.contains(&search)
        {
            continue;
        }
        if let Some(f) = folder {
            let key = platform.path_key(f);
            let actual = string(&p["folder_key"])?;
            let recursive = options["recursive"].as_bool().unwrap_or(true);
            // SQLite LIKE is ASCII case-insensitive, even on POSIX; preserve that quirk.
            let prefix = format!("{}{}", key.trim_end_matches(['\\', '/']), platform.sep());
            if actual != key && (!recursive || !actual.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase())) {
                continue;
            }
        }
        match options["mode"].as_str() {
            Some("picked") if p["flag"] != 1 => continue,
            Some("rejected") if p["flag"] != -1 => continue,
            Some("rated") if p["rating"].as_i64().unwrap_or(0) < 4 => continue,
            _ => {}
        }
        let mut valid = true;
        for r in &rules {
            if !matches(&p, r, now, platform)? {
                valid = false;
                break;
            }
        }
        if valid {
            selected.push(json!({"id":id,"stack_id":p["stack_id"]}));
        }
    }
    let ids = workflow::arrange_stacks(
        &selected,
        options.get("stack_orders").unwrap_or(&json!({})),
        grainy_core::settings::truthy(&options["collapsed"]),
    )?;
    let folders = rows(&db, "SELECT MIN(folder) folder,COUNT(*) count FROM browser_index GROUP BY folder_key")?
        .into_iter()
        .map(|r| Ok(json!({"path":platform.join(string(&r["folder"])?,"__count__"),"count":r["count"]})))
        .collect::<Result<Vec<_>>>()?;
    let total: i64 = folders.iter().filter_map(|r| r["count"].as_i64()).sum();
    let roots = rows(&db, "SELECT path FROM folder_roots ORDER BY path COLLATE NOCASE")?
        .into_iter()
        .map(|r| r["path"].clone())
        .collect::<Vec<_>>();
    cancelled(cancel)?;
    Ok(json!({"ids":ids,"folders":folders,"roots":roots,"total":total}))
}
/// Port of `library_query.read_page`; keys are decimal photo ids in JSON.
pub fn read_page(path: impl AsRef<Path>, ids: &[i64]) -> Result<Value> {
    if ids.is_empty() {
        return Ok(json!({}));
    }
    let db = connect(path.as_ref())?;
    let mut result = serde_json::Map::new();
    for part in ids.chunks(500) {
        let sql = format!(
            "SELECT id,path,name,rating,flag,label FROM photos WHERE id IN ({})",
            vec!["?"; part.len()].join(",")
        );
        for r in db.prepare(&sql)?.query_map(rusqlite::params_from_iter(part), row_value)? {
            let r = r?;
            result.insert(integer(&r["id"])?.to_string(), r);
        }
    }
    Ok(result.into())
}
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}
fn compare<T: PartialOrd>(a: T, op: &str, b: T, c: Option<T>) -> bool {
    match op {
        "==" => a == b,
        "!=" => a != b,
        ">" => a > b,
        "<" => a < b,
        ">=" => a >= b,
        "<=" => a <= b,
        "in" => a >= b && c.is_some_and(|c| a <= c),
        _ => false,
    }
}
fn extension(platform: Platform, path: &str) -> String {
    let name = platform.name(path);
    name.rsplit_once('.')
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .map(|(_, b)| b.to_lowercase())
        .unwrap_or_default()
}

/// Port of `library.matches`, including nested rules and saved Adobe smart descriptors.
pub fn matches(photo: &Value, rules: &Value, now: NaiveDateTime, platform: Platform) -> Result<bool> {
    let i = info(photo)?;
    let mut values = object(photo)?.clone();
    values.extend(object(&i)?.clone());
    let path = string(&photo["path"])?;
    values.insert("extension".into(), extension(platform, path).into());
    values.insert("folder".into(), platform.parent(path).into());
    values.insert("date".into(), i.get("DateTimeOriginal").or_else(|| i.get("date")).cloned().unwrap_or(json!("")));
    for (key, expected) in object(rules)? {
        if expected.is_null() || expected == "" || expected == &json!([]) || expected == &json!({}) {
            continue;
        }
        let valid = match key.as_str() {
            "_adobe_smart" => smart(expected, photo, &i, now, platform, 0)?,
            "any" | "all" => {
                let a = expected.as_array().ok_or_else(|| invalid("compound rules must be an array"))?;
                let mut hits = Vec::new();
                for r in a {
                    hits.push(matches(photo, r, now, platform)?);
                }
                if key == "any" {
                    hits.iter().any(|v| *v)
                } else {
                    hits.iter().all(|v| *v)
                }
            }
            "missing" => {
                // `file_access.access(path) == MISSING`: a stat that fails for any reason but a denied permission
                // (macOS privacy settings), or anything but a regular file. A name too long for the file system
                // is missing on macOS (ENAMETOOLONG) as on Windows.
                let missing = match std::fs::metadata(path) {
                    Err(e) => e.kind() != std::io::ErrorKind::PermissionDenied,
                    Ok(meta) => !meta.is_file(),
                };
                missing == grainy_core::settings::truthy(expected)
            }
            "virtual" => (!photo["virtual_source"].is_null()) == grainy_core::settings::truthy(expected),
            "date_from" | "date_to" => {
                let actual = json::text(&values["date"]).replacen(':', "-", 2).chars().take(10).collect::<String>();
                !actual.is_empty()
                    && if key == "date_from" {
                        actual.as_str() >= string(expected)?
                    } else {
                        actual.as_str() <= string(expected)?
                    }
            }
            "orientation" => {
                let w = number(&i["width"]).unwrap_or(0.0);
                let h = number(&i["height"]).unwrap_or(0.0);
                w != 0.0
                    && h != 0.0
                    && match expected.as_str() {
                        Some("portrait") => w < h,
                        Some("landscape") => w > h,
                        Some("square") => w == h,
                        _ => true,
                    }
            }
            "rating" | "flag" | "label" => values.get(key).is_some_and(|v| json::equivalent(v, expected)),
            k if k.ends_with("_min") || k.ends_with("_max") => {
                let a = number(values.get(&k[..k.len() - 4]).unwrap_or(&json!(0)));
                let b = number(expected);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        if k.ends_with("_min") {
                            a >= b
                        } else {
                            a <= b
                        }
                    }
                    _ => false,
                }
            }
            _ => fold(&json::text(values.get(key).unwrap_or(&json!("")))).contains(&fold(&json::text(expected))),
        };
        if !valid {
            return Ok(false);
        }
    }
    Ok(true)
}
fn parse_date(v: &Value) -> Option<NaiveDateTime> {
    let text = json::text(v);
    let text = if text.as_bytes().get(4) == Some(&b':') { text.replacen(':', "-", 2) } else { text };
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(&text) {
        return Some(d.with_timezone(&chrono::Local).naive_local());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(&text, fmt) {
            return Some(d);
        }
    }
    NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0)
}
fn date_match(node: &Value, actual: NaiveDateTime, now: NaiveDateTime) -> Result<bool> {
    let op = string(&node["operation"])?;
    if ["==", "!=", ">", "<", "in"].contains(&op) {
        let a = actual.date().to_string();
        return Ok(compare(a.as_str(), op, string(&node["value"])?, node["value2"].as_str()));
    }
    let today = now.date();
    let n = node["value"].as_i64().unwrap_or(0);
    if !(0..=10000).contains(&n) {
        return Err(invalid("invalid relative date window"));
    }
    let unit = node["value_units"].as_str().unwrap_or("");
    if unit == "hours" {
        let start = now.checked_sub_signed(Delta::hours(n)).ok_or_else(|| invalid("date out of range"))?;
        let found = start <= actual && actual <= now;
        return Ok(if op == "notInLast" { !found } else { found });
    }
    let start = match op {
        "today" => today,
        "yesterday" => today - Delta::days(1),
        "thisWeek" => today - Delta::days(i64::from(today.weekday().num_days_from_monday())),
        "thisMonth" => today.with_day(1).ok_or_else(|| invalid("invalid date"))?,
        "thisYear" => today.with_ordinal(1).ok_or_else(|| invalid("invalid date"))?,
        "inLast" | "notInLast" => match unit {
            "days" => today - Delta::days(n),
            "weeks" => today - Delta::weeks(n),
            "months" | "years" => {
                let months = n * if unit == "years" { 12 } else { 1 };
                today
                    .checked_sub_months(chrono::Months::new(months as u32))
                    .filter(|d| d.year() >= 1)
                    .unwrap_or(NaiveDate::from_ymd_opt(1, 1, 1).ok_or_else(|| invalid("invalid date"))?)
            }
            _ => return Err(invalid("invalid date units")),
        },
        _ => return Err(invalid("invalid date operation")),
    };
    let end = if op == "yesterday" { start } else { today };
    let found = start <= actual.date() && actual.date() <= end;
    Ok(if op == "notInLast" { !found } else { found })
}
fn smart(node: &Value, p: &Value, i: &Value, now: NaiveDateTime, platform: Platform, depth: usize) -> Result<bool> {
    if depth > 24 {
        return Err(invalid("smart rule nesting exceeds 24"));
    }
    if let Some(mode) = node["combine"].as_str() {
        let children = node["children"].as_array().ok_or_else(|| invalid("missing smart children"))?;
        let mut hits = Vec::new();
        for n in children {
            hits.push(smart(n, p, i, now, platform, depth + 1)?);
        }
        return Ok(match mode {
            "union" => hits.iter().any(|v| *v),
            "exclude" => !hits.iter().any(|v| *v),
            _ => hits.iter().all(|v| *v),
        });
    }
    let field = string(&node["criteria"])?;
    let op = string(&node["operation"])?;
    let expected = &node["value"];
    let path = string(&p["path"])?;
    let mut values = object(i)?.clone();
    values.extend(object(p)?.clone());
    values.insert("filename".into(), platform.name(path).into());
    values.insert("folder".into(), platform.parent(path).into());
    values.insert("copyname".into(), i.get("_copy_name").cloned().unwrap_or(json!("")));
    let numeric = match field {
        "rating" => Some("rating"),
        "pick" => Some("flag"),
        "isoSpeedRating" => Some("iso"),
        _ => None,
    };
    if let Some(key) = numeric {
        return Ok(match (values.get(key).and_then(number), number(expected)) {
            (Some(a), Some(b)) => a.is_finite() && compare(a, op, b, number(&node["value2"])),
            _ => false,
        });
    }
    let text_key = match field {
        "filename" | "copyname" | "folder" | "title" | "caption" | "keywords" | "camera" | "lens" | "country"
        | "state" | "city" | "location" | "creator" => Some(field),
        "labelText" => Some("label"),
        "cameraSN" => Some("serial"),
        "jobIdentifier" => Some("job"),
        _ => None,
    };
    if let Some(key) = text_key {
        let actual =
            fold(&values.get(key).filter(|v| grainy_core::settings::truthy(v)).map(json::text).unwrap_or_default());
        if op == "empty" {
            return Ok(actual.trim().is_empty());
        }
        if op == "notEmpty" {
            return Ok(!actual.trim().is_empty());
        }
        let mut expected = fold(string(expected)?);
        if field == "labelText" {
            expected = match expected.as_str() {
                "red" => "빨강",
                "yellow" => "노랑",
                "green" => "초록",
                "blue" => "파랑",
                "purple" => "보라",
                _ => &expected,
            }
            .into();
        }
        if op == "==" || op == "!=" {
            return Ok(compare(actual.as_str(), op, expected.as_str(), None));
        }
        if op == "beginsWith" {
            return Ok(actual.starts_with(&expected));
        }
        if op == "endsWith" {
            return Ok(actual.ends_with(&expected));
        }
        let words =
            expected.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).collect::<Vec<_>>();
        if words.is_empty() {
            return Ok(false);
        }
        let hits = words
            .iter()
            .map(|word| {
                if op == "words" {
                    actual.match_indices(word).any(|(idx, matched)| {
                        let before = actual[..idx].chars().next_back();
                        let after = actual[idx + matched.len()..].chars().next();
                        !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                            && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
                    })
                } else {
                    actual.contains(word)
                }
            })
            .collect::<Vec<_>>();
        return Ok(match op {
            "words" | "all" => hits.iter().all(|v| *v),
            "noneOf" => !hits.iter().any(|v| *v),
            _ => hits.iter().any(|v| *v),
        });
    }
    if field == "captureTime" || field == "touchTime" {
        let date = if field == "captureTime" {
            i.get("DateTimeOriginal").or_else(|| i.get("date"))
        } else {
            i.get("_last_edit_time")
        };
        return date.and_then(parse_date).map(|d| date_match(node, d, now)).unwrap_or(Ok(false));
    }
    if field == "labelColor" {
        let labels = ["", "빨강", "노랑", "초록", "파랑", "보라"];
        let label = p["label"].as_str().unwrap_or("");
        let found = if expected == "custom" {
            !labels.contains(&label)
        } else {
            let index = expected.as_u64().unwrap_or(0) as usize;
            label == *labels.get(index).ok_or_else(|| invalid("invalid label color"))?
        };
        return Ok(if op == "==" { found } else { !found });
    }
    if field == "hasGPSData" {
        let found = number(&i["latitude"]).is_some_and(|v| (-90.0..=90.0).contains(&v))
            && number(&i["longitude"]).is_some_and(|v| (-180.0..=180.0).contains(&v));
        return Ok(if op == "isTrue" { found } else { !found });
    }
    if field == "fileFormat" {
        let ext = extension(platform, path);
        let actual = if ext == "dng" {
            "DNG".into()
        } else if ["arw", "cr2", "cr3", "nef", "nrw", "raf", "rw2", "orf", "pef", "srw", "3fr", "iiq", "kdc", "mos"]
            .contains(&ext.as_str())
        {
            "RAW".into()
        } else if ["mp4", "mov", "avi", "mkv", "m4v"].contains(&ext.as_str()) {
            "VIDEO".into()
        } else {
            match ext.as_str() {
                "jpg" | "jpeg" => "JPG".into(),
                "tif" | "tiff" => "TIFF".into(),
                _ => ext.to_uppercase(),
            }
        };
        return Ok(compare(actual.as_str(), op, string(expected)?, None));
    }
    Ok(false)
}
