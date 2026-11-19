//! The scripted catalogue golden: every operation of `golden/service/script` replayed on a fresh catalogue, its
//! return value (or exception) and every changed table compared with the oracle's, tolerance 0.
mod common;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use common::{case, check, dump, full, outcome, unplace, Scrub};
use grainy_catalog::{rusqlite, Catalog, Sources};
use grainy_core::Settings;
use grainy_service::batch::{self, ApplyOptions};
use grainy_service::labels::{self, LabelEntry};
use grainy_service::stack::{self, BatchOptions};
use grainy_service::{edits, targets, Error, Result};
use serde_json::{json, Map, Value};

struct Runner {
    c: Catalog,
    defaults: Map<String, Value>,
    snapshotted: HashSet<i64>,
    captures: HashMap<String, Value>,
    tokens: RefCell<(Vec<String>, usize)>,
}

fn ids(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(|i| i.as_i64().unwrap()).collect()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn opt(v: &Value) -> Option<&Value> {
    if v.is_null() {
        None
    } else {
        Some(v)
    }
}

impl Runner {
    fn token(&self) -> Result<String> {
        let mut t = self.tokens.borrow_mut();
        let value = t.0[t.1].clone();
        t.1 += 1;
        Ok(value)
    }

    fn presets(v: &Value) -> Option<BTreeMap<i64, String>> {
        v.as_object().map(|m| m.iter().map(|(k, v)| (k.parse().unwrap(), s(v).to_owned())).collect())
    }

    fn apply_batch(
        &mut self,
        records: Vec<Value>,
        values: Vec<Value>,
        tool: &str,
        switch: bool,
        presets: &Value,
    ) -> Result<Value> {
        let presets = Self::presets(presets);
        let options =
            BatchOptions { label: "AI 보정", tool, snapshot_name: "AI 이전", switch, presets: presets.as_ref() };
        let mut snapshotted = std::mem::take(&mut self.snapshotted);
        let mut token = || self.token();
        let result = stack::apply_batch(&self.c, &records, &values, &options, &mut snapshotted, &mut token);
        self.snapshotted = snapshotted;
        let (bid, changed) = result?;
        Ok(json!([bid, changed]))
    }

    fn call(&mut self, op: &str, a: &Value) -> Result<Value> {
        let c = &self.c;
        Ok(match op {
            "add" => json!(c.add(
                s(&a[0]),
                Some(&Settings::from_value(full(&self.defaults, &a[1])).unwrap()),
                Some(&a[2]),
                true
            )?),
            "virtual_copy" => json!(c.virtual_copy(a[0].as_i64().unwrap())?),
            "update" => {
                c.update(a[0].as_i64().unwrap(), a[1].as_object().unwrap())?;
                Value::Null
            }
            "edit" => {
                let id = a[0].as_i64().unwrap();
                let mut settings = c.photo(id)?.unwrap()["settings"].as_object().unwrap().clone();
                for (k, v) in a[1].as_object().unwrap() {
                    settings.insert(k.clone(), v.clone());
                }
                c.edit(id, &Settings(settings), s(&a[2]))?;
                Value::Null
            }
            "save_preference" => {
                c.save_preference(s(&a[0]), &a[1])?;
                Value::Null
            }
            "sql" => {
                let params = a[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(grainy_catalog::sql_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                c.connection().execute(s(&a[0]), rusqlite::params_from_iter(params))?;
                Value::Null
            }
            "remove_photos" => {
                c.remove_photos(&ids(&a[0]))?;
                Value::Null
            }
            "stacked" => {
                let depth = a[1].as_i64().unwrap();
                let redo = a[2].as_i64().unwrap();
                let (order, int_first) = (a[3].as_bool().unwrap(), a[4].as_bool().unwrap());
                for ident in ids(&a[0]) {
                    let base = c.photo(ident)?.unwrap()["settings"].as_object().unwrap().clone();
                    let mut undo: Vec<Map<String, Value>> = (0..depth)
                        .map(|i| {
                            let mut m = base.clone();
                            m.insert("exposure".into(), json!((-i) as f64 / 100.0));
                            m
                        })
                        .collect();
                    if order && !undo.is_empty() {
                        undo[0] = undo[0].iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                    if int_first && !undo.is_empty() {
                        undo[0].insert("contrast".into(), json!(0));
                    }
                    let redo: Vec<Value> = (0..redo)
                        .map(|i| {
                            let mut m = base.clone();
                            m.insert("contrast".into(), json!(i as f64));
                            Value::Object(m)
                        })
                        .collect();
                    c.save_preference(&format!("undo:{ident}"), &json!({"undo": undo, "redo": redo}))?;
                }
                Value::Null
            }
            "undo_state" => {
                let value = c.preference(&format!("undo:{}", a[0]), Value::Null)?;
                match value {
                    Value::Object(m) => Value::Object(
                        m.into_iter().map(|(k, v)| (k, v.as_array().map_or(v.clone(), |l| json!(l.len())))).collect(),
                    ),
                    other => other,
                }
            }
            "query_ids" => {
                let cancel = Arc::new(AtomicBool::new(a[4].as_bool().unwrap()));
                json!(targets::query_ids(
                    c.directory(),
                    &a[0],
                    a[1].as_str(),
                    a[2].as_bool().unwrap(),
                    Some(&cancel),
                    a[3].as_bool().unwrap()
                )?)
            }
            "page_rows" => {
                let (dirs, rows) = targets::page_rows(c.directory(), &ids(&a[0]))?;
                json!([dirs, rows])
            }
            "folder_rows" => {
                let cancel = AtomicBool::new(a[1].as_bool().unwrap());
                json!(targets::folder_rows(c.directory(), a[0].as_str(), Some(&cancel))?)
            }
            "freeze" => {
                let (records, errors) = targets::freeze(c, &ids(&a[0]))?;
                json!([records, errors])
            }
            "capture" => {
                let value = batch::capture(c, a[0].as_i64(), &ids(&a[1]), a[2].as_str())?;
                self.captures.insert(s(&a[3]).into(), value.clone());
                value
            }
            "describe" => json!(batch::describe(&self.captures[s(&a[0])], s(&a[1]))?),
            "targets" => json!(batch::targets(self.captures.get(s(&a[0])), s(&a[1]))?),
            "cb_apply" => {
                let records = batch::targets(self.captures.get(s(&a[0])), s(&a[1]))?;
                let mut values: Vec<Value> = records
                    .iter()
                    .map(|r| {
                        let mut m = r["settings"].as_object().unwrap().clone();
                        for (k, v) in a[2].as_object().unwrap() {
                            m.insert(k.clone(), v.clone());
                        }
                        Value::Object(m)
                    })
                    .collect();
                if a[4].as_bool().unwrap() {
                    values.push(json!({}));
                }
                let o = &a[3];
                let mut options = ApplyOptions::default();
                if let Some(v) = o.get("label") {
                    options.label = s(v).into();
                }
                if let Some(v) = o.get("remember") {
                    options.remember = v.as_bool().unwrap();
                }
                if let Some(v) = o.get("batch_key") {
                    options.batch_key = s(v).into();
                }
                if let Some(v) = o.get("meta") {
                    options.meta = v.as_object().cloned();
                }
                if let Some(v) = o.get("compact") {
                    options.compact = v.as_bool().unwrap();
                }
                if let Some(v) = o.get("switch") {
                    options.switch = v.as_bool().unwrap();
                }
                json!(batch::apply(c, &records, &values, &options, None)?)
            }
            "cb_duplicate" => {
                let mut records = batch::targets(self.captures.get(s(&a[0])), s(&a[1]))?;
                records.push(records[0].clone());
                let values: Vec<Value> = records.iter().map(|r| r["settings"].clone()).collect();
                json!(batch::apply(c, &records, &values, &ApplyOptions::default(), None)?)
            }
            "cb_restore" => json!(batch::restore(c, a[0].as_bool().unwrap(), s(&a[1]), None, opt(&a[2]))?),
            "batch" => {
                let (records, errors) = targets::freeze(c, &ids(&a[0]))?;
                let changes = edits::parse_changes(opt(&a[1]), opt(&a[2]))?;
                let mut values = Vec::new();
                for r in &records {
                    values.push(edits::compose(
                        &r["settings"],
                        &[edits::Step::Edit(&changes)],
                        &r["info"],
                        r["id"].as_i64().unwrap(),
                    )?);
                }
                let result = self.apply_batch(records, values, s(&a[3]), a[4].as_bool().unwrap(), &a[5])?;
                json!([result[0], result[1], errors])
            }
            "batch_values" => {
                let (records, errors) = targets::freeze(c, &ids(&a[0]))?;
                let values: Vec<Value> = records
                    .iter()
                    .map(|r| {
                        let mut m = r["settings"].as_object().unwrap().clone();
                        for (k, v) in a[1].as_object().unwrap() {
                            m.insert(k.clone(), v.clone());
                        }
                        Value::Object(m)
                    })
                    .collect();
                let result = self.apply_batch(records, values, "apply_edits", a[2].as_bool().unwrap(), &a[3])?;
                json!([result[0], result[1], errors])
            }
            "undo_batch" => {
                let (id, tool, ids) = stack::undo_batch(c, a[0].as_str(), a[1].as_bool().unwrap())?;
                json!([id, tool, ids])
            }
            "batch_index" => json!(stack::batch_index(c)?),
            "last_batch" => json!(stack::last_batch(c)?),
            "snapshotted" => {
                let mut v: Vec<i64> = self.snapshotted.iter().copied().collect();
                v.sort_unstable();
                json!(v)
            }
            "labels" => {
                let entries: Vec<LabelEntry> = a[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| LabelEntry { ids: ids(&e["ids"]), fields: e["fields"].as_object().unwrap().clone() })
                    .collect();
                let mut token = || self.token();
                let (bid, changed, unchanged, errors) = labels::apply(c, &entries, &mut token)?;
                json!([bid, changed, unchanged, errors])
            }
            "busy" => {
                self.c.connection().execute_batch("BEGIN")?;
                let result = self.call(s(&a[0]), &a[1]);
                self.c.connection().execute_batch("ROLLBACK")?;
                result?
            }
            other => panic!("unknown operation {other}"),
        })
    }
}

#[test]
fn scripted_catalogue_matches_the_oracle() {
    let golden = case("script");
    let defaults = common::defaults();
    let root = common::fresh("script");
    let root_text = common::text_path(&root);
    for name in golden["files"].as_array().unwrap() {
        let path = root.join(s(name));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("generated {}", s(name))).unwrap();
    }
    let stamp = s(&golden["stamp"]).to_owned();
    let sources = Sources::new(move || stamp.clone(), || "0123456789abcdef0123456789abcdef".into());
    let catalog = Catalog::with_sources(root.join("data"), sources).unwrap();
    let scrub = Scrub::new(Some(&root_text), &defaults);
    let tokens: Vec<String> = golden["tokens"].as_array().unwrap().iter().map(|t| s(t).to_owned()).collect();
    let mut runner = Runner {
        c: catalog,
        defaults: defaults.clone(),
        snapshotted: HashSet::new(),
        captures: HashMap::new(),
        tokens: RefCell::new((tokens, 0)),
    };
    let mut previous = dump(runner.c.connection(), &scrub);
    check(&Value::Object(previous.clone()), &golden["initial"], "initial tables");
    let steps = golden["steps"].as_array().unwrap();
    for (n, step) in steps.iter().enumerate() {
        let op = s(&step["op"]);
        let args = unplace(&step["args"], &root_text);
        let actual = scrub.value(&outcome(runner.call(op, &args)));
        let mut expected = step.as_object().unwrap().clone();
        for key in ["op", "args", "delta"] {
            expected.shift_remove(key);
        }
        check(&actual, &Value::Object(expected), &format!("step {n} {op} {}", step["args"]));
        let current = dump(runner.c.connection(), &scrub);
        let delta: Map<String, Value> =
            current.iter().filter(|(k, v)| previous.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
        check(&Value::Object(delta), &step["delta"], &format!("step {n} {op}: changed tables"));
        previous = current;
    }
    check(&Value::Object(previous), &golden["final"], "final tables");
    assert_eq!(runner.tokens.borrow().1 as u64, golden["tokens_used"].as_u64().unwrap(), "batch ids drawn");
    eprintln!("script: {} steps, all results and changed tables exact (tolerance 0)", steps.len());
}

#[test]
fn busy_and_error_names_are_python_classes() {
    let e = Error::Busy("x".into());
    assert_eq!(e.python_name(), "Busy");
}
