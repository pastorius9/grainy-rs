//! Golden cases of the pure functions (no catalogue): validation, composition, summaries, naming, sizes.
mod common;

use std::collections::BTreeMap;

use common::{case, check, full, outcome, Scrub};
use grainy_service::auto::{auto_records, AutoKind};
use grainy_service::edits::{self, Step};
use grainy_service::export::{self, Longest};
use grainy_service::photo_edits::{self, EditEntry, PresetChoice};
use grainy_service::{summary, targets, Error, Failure, Plain, Result};
use serde_json::{json, Map, Value};

fn opt(v: &Value) -> Option<&Value> {
    if v.is_null() {
        None
    } else {
        Some(v)
    }
}

fn changes_of(a: &Value) -> Result<edits::Changes> {
    edits::parse_changes(a.get("set").and_then(opt), a.get("add").and_then(opt))
}

fn groups(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(|g| g.as_str().unwrap().to_owned()).collect())
}

fn automatic(record: &Value, settings: &Value, straighten: bool, crop: bool) -> Result<Value> {
    if record["name"].as_str().unwrap().contains("bad") {
        return Err(Error::Failed(Failure::new("ValueError", "cannot read")));
    }
    let mut result = settings.as_object().unwrap().clone();
    if straighten {
        result.insert("straighten".into(), 0.into());
        result.insert("upright".into(), json!({"mode": "level", "rotation": 1.5}));
    }
    if crop {
        result.insert("crop".into(), json!([0.05, 0.05, 0.95, 0.95]));
    }
    grainy_service::normalized(&Value::Object(result))
}

fn call(d: &Map<String, Value>, f: &str, a: &Value) -> Result<Value> {
    let full = |v: &Value| full(d, v);
    Ok(match f {
        "parse_changes" => {
            let c = edits::parse_changes(opt(&a["set"]), opt(&a["add"]))?;
            json!([c.values, c.operations])
        }
        "curve_points" => edits::curve_points(&a["value"], a["key"].as_str().unwrap())?,
        "canvas" => json!(edits::canvas(&full(&a["settings"]), &a["info"]).map(|(w, h)| vec![w, h])),
        "edited" => edits::edited(&full(&a["settings"]), &changes_of(a)?, &a["info"], a["ident"].as_i64().unwrap())?,
        "compose" => {
            let parsed = a["groups"].as_array().unwrap().iter().map(changes_of).collect::<Result<Vec<_>>>()?;
            let steps: Vec<Step> = parsed.iter().map(Step::Edit).collect();
            edits::compose(&full(&a["settings"]), &steps, &a["info"], a["ident"].as_i64().unwrap())?
        }
        "preset_settings" => edits::preset_settings(&a["preset"], &full(&a["settings"]), &a["record"], &Plain)?,
        "copy_keys" => json!(edits::copy_keys(groups(&a["groups"]).as_deref())?),
        "copied_settings" => {
            let keys = edits::copy_keys(groups(&a["groups"]).as_deref())?;
            edits::copied_settings(&full(&a["source"]), &full(&a["settings"]), &a["info"], &keys, &Plain)?
        }
        "where_options" => targets::where_options(
            &a["where"],
            a["sort"].as_str(),
            a["descending"].as_bool().unwrap(),
            a["allow_empty"].as_bool().unwrap(),
        )?,
        "fmt" => summary::fmt(&a["value"])?,
        "summary" => {
            let mut record = a["record"].clone();
            record["settings"] = full(&a["record"]["settings"]);
            summary::summary(&record)?
        }
        "summaries" => {
            let records: Vec<Value> = a["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r["settings"] = full(&r["settings"]);
                    r
                })
                .collect();
            let (dirs, entries) = summary::summaries(&records)?;
            json!([dirs, entries])
        }
        "check_naming" => {
            export::check_naming(&a["naming"])?;
            Value::Null
        }
        "file_stem" => json!(export::file_stem(
            a["naming"].as_str().unwrap(),
            a["stem"].as_str().unwrap(),
            a["n"].as_i64().unwrap(),
            a["values"].as_object().unwrap()
        )?),
        "name_values" => {
            let mut record = a["record"].clone();
            record["settings"] = full(a["record"].get("settings").unwrap_or(&json!({})));
            Value::Object(export::name_values(&record))
        }
        "safe_filename" => json!(export::safe_filename(
            a["stem"].as_str().unwrap(),
            a["suffix"].as_str().unwrap(),
            a["index"].as_u64().unwrap() as usize
        )),
        "english_error" => json!(export::english_error(&Failure::new(a["kind"].as_str().unwrap(), "x"))),
        "export_longest" => json!(export::export_longest(
            a["h"].as_i64().unwrap(),
            a["w"].as_i64().unwrap(),
            &Longest::from_value(&a["longest"]).unwrap()
        )),
        "planned_size" => {
            let mut record = a["record"].clone();
            record["settings"] = full(&a["record"]["settings"]);
            let probe = (a["probe"][0].as_i64().unwrap(), a["probe"][1].as_i64().unwrap());
            let (w, h) =
                export::planned_size(&record, &Longest::from_value(&a["longest"]).unwrap(), &mut |_| Ok(probe))?;
            json!([w, h])
        }
        "crop_value" => photo_edits::crop_value(&a["value"])?,
        "auto_job" => {
            json!(auto_records(AutoKind::parse(a["kind"].as_str().unwrap())?, a["records"].as_array().unwrap())?)
        }
        "photo_edits.compose" => {
            let with_full = |r: &Value| {
                let mut r = r.clone();
                r["settings"] = full(&r["settings"]);
                r
            };
            let records: Vec<Value> = a["records"].as_array().unwrap().iter().map(with_full).collect();
            let sources: BTreeMap<i64, Value> =
                a["sources"].as_object().unwrap().iter().map(|(k, r)| (k.parse().unwrap(), with_full(r))).collect();
            let mut entries = Vec::new();
            for e in a["entries"].as_array().unwrap() {
                let has = |k: &str| e.get(k).is_some();
                let mut entry = EditEntry {
                    ids: e["ids"].as_array().unwrap().iter().map(|i| i.as_i64().unwrap()).collect(),
                    copy_from: e.get("copy_from").and_then(Value::as_i64),
                    changes: if has("set") || has("add") { Some(changes_of(e)?) } else { None },
                    auto_straighten: e.get("auto_straighten").and_then(Value::as_bool).unwrap_or(false),
                    auto_crop: e.get("auto_crop").and_then(Value::as_bool).unwrap_or(false),
                    ..Default::default()
                };
                if let Some(name) = e.get("preset").and_then(Value::as_str) {
                    entry.preset = Some(PresetChoice { name: name.into(), base: e["base"].clone() });
                }
                if entry.copy_from.is_some() {
                    entry.keys = edits::copy_keys(groups(e.get("groups").unwrap_or(&Value::Null)).as_deref())?;
                }
                if let Some(crop) = e.get("crop") {
                    entry.crop = Some(photo_edits::crop_value(crop)?);
                }
                entries.push(entry);
            }
            let limit = a["deadline"].as_i64();
            let mut calls = 0;
            let mut deadline = || {
                calls += 1;
                match limit {
                    Some(l) if calls > l => {
                        Err(Error::Service("the time limit was reached; ask for fewer photos or try again".into()))
                    }
                    _ => Ok(()),
                }
            };
            let (used, values, failures, presets) =
                photo_edits::compose(&entries, &records, &sources, &Plain, &mut deadline, &mut automatic)?;
            let ids: Vec<Value> = used.iter().map(|r| r["id"].clone()).collect();
            let presets: Map<String, Value> = presets.into_iter().map(|(k, v)| (k.to_string(), v.into())).collect();
            json!([ids, values, failures, presets])
        }
        other => panic!("unknown pure case {other}"),
    })
}

#[test]
fn pure_functions_match_the_oracle() {
    let golden = case("pure");
    let d = golden["defaults"].as_object().unwrap().clone();
    let scrub = Scrub::new(None, &d);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (n, c) in golden["cases"].as_array().unwrap().iter().enumerate() {
        let f = c["fn"].as_str().unwrap();
        let actual = scrub.value(&outcome(call(&d, f, &c["args"])));
        let mut expected = c.as_object().unwrap().clone();
        expected.shift_remove("fn");
        expected.shift_remove("args");
        check(&actual, &Value::Object(expected), &format!("case {n} {f} {}", c["args"]));
        *counts.entry(f.into()).or_default() += 1;
    }
    eprintln!("pure cases (tolerance 0, all exact): {counts:?}");
}

#[test]
fn fields_follow_edit_fields() {
    for field in grainy_i18n::fields::EDIT_FIELDS.iter() {
        let (_, low, high) = grainy_service::FIELDS.iter().find(|(k, _, _)| *k == field.key).copied().unwrap();
        assert_eq!((low as f64, high as f64), (field.min, field.max), "{}", field.key);
    }
}
