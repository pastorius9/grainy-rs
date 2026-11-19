//! Goldens that touch real files: automatic values (with a stand-in loader) and export folders, plans and the
//! export loop (with a stand-in renderer that records its calls).
mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use common::{case, check, outcome, unplace, Scrub};
use grainy_service::auto::{auto_values, reason, AutoSource};
use grainy_service::export::{self, ExportJob, ExportOptions, Exporter, ImageFiles, Longest};
use grainy_service::{Error, Failure, Result};
use serde_json::{json, Value};

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

struct Loader {
    calls: Vec<Value>,
}
impl AutoSource for Loader {
    type Image = f64;
    type Value = Value;
    fn load(&mut self, record: &Value, path: &Path) -> std::result::Result<f64, Failure> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        self.calls.push(json!(["load", name, 720, record["settings"]["working_space"]]));
        if name == "fail.png" {
            return Err(Failure::new("ValueError", "cannot decode"));
        }
        if name == "changes.png" {
            std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(b"+").unwrap();
        }
        Ok(record["settings"]["exposure"].as_f64().unwrap())
    }
    fn offline(&mut self, record: &Value) -> std::result::Result<f64, Failure> {
        self.calls.push(json!(["offline", record["id"]]));
        Err(Failure::new("FileNotFoundError", "no packet"))
    }
    fn compute(&mut self, image: f64, settings: &Value) -> std::result::Result<Value, Failure> {
        let mean = f64::from(image as f32);
        self.calls.push(json!(["compute", mean]));
        Ok(json!({"mean": mean, "ws": settings["working_space"]}))
    }
}

#[test]
fn automatic_values_match_the_oracle() {
    let golden = case("auto");
    let defaults = common::defaults();
    let root = common::fresh("auto");
    let root_text = common::text_path(&root);
    for name in golden["names"].as_array().unwrap() {
        if s(name) != "gone.png" {
            std::fs::write(root.join(s(name)), format!("pixels of {}", s(name))).unwrap();
        }
    }
    let scrub = Scrub::new(Some(&root_text), &defaults);
    let records: Vec<Value> = unplace(&golden["records"], &root_text).as_array().unwrap().clone();
    for (n, run) in golden["runs"].as_array().unwrap().iter().enumerate() {
        let chosen: Vec<Value> = match run["subset"].as_array() {
            Some(subset) => subset.iter().map(|i| records[i.as_u64().unwrap() as usize].clone()).collect(),
            None => records.clone(),
        };
        let cancel_at = run["cancel_at"].as_u64().map(|v| v as usize);
        let cancel = AtomicBool::new(cancel_at == Some(0));
        let mut seen = Vec::new();
        let mut progress = |i: usize| {
            seen.push(i);
            if cancel_at.is_some_and(|c| i >= c) {
                cancel.store(true, Ordering::SeqCst);
            }
        };
        let mut loader = Loader { calls: vec![] };
        let result =
            auto_values(&chosen, &mut loader, Some(&cancel), Some(&mut progress), run["strict"].as_bool().unwrap())
                .map(|(values, errors)| json!([values, errors]));
        let mut actual = scrub.value(&outcome(result)).as_object().unwrap().clone();
        actual.insert("progress".into(), json!(seen));
        actual.insert("calls".into(), scrub.value(&Value::Array(loader.calls)));
        let mut expected = serde_json::Map::new();
        for key in ["result", "error", "progress", "calls"] {
            if let Some(v) = run.get(key) {
                expected.insert(key.into(), v.clone());
            }
        }
        let order = |m: serde_json::Map<String, Value>| {
            let mut sorted: Vec<(String, Value)> = m.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(sorted.into_iter().collect())
        };
        check(&order(actual), &order(expected), &format!("auto run {n}"));
    }
    for r in golden["reasons"].as_array().unwrap() {
        let error = if s(&r["error"]) == "OriginalChanged" {
            Error::OriginalChanged("x".into())
        } else {
            Error::Failed(Failure::new(s(&r["error"]), "x"))
        };
        assert_eq!(reason(&error, &format!("{root_text}\\{}", s(&r["path"]))), s(&r["reason"]));
    }
}

struct StandIn {
    calls: Mutex<Vec<Value>>,
}
impl Exporter for StandIn {
    fn export_image(&self, job: &ExportJob<'_>) -> std::result::Result<Value, Failure> {
        let o = job.options;
        let longest = match job.longest {
            None => Value::Null,
            Some(Longest::Side(n)) => json!(n),
            Some(Longest::Box(w, h, fit)) => json!([w, h, fit]),
        };
        let name = Path::new(s(&job.record["path"])).file_name().unwrap().to_string_lossy().into_owned();
        self.calls.lock().unwrap().push(json!([
            job.destination.file_name().unwrap().to_string_lossy(),
            o.format,
            o.quality,
            longest,
            o.color_space,
            o.keep_metadata,
            job.record["user_metadata"],
            job.record["keywords"],
            o.grayscale,
            o.dither,
            job.record["settings"].get("exposure")
        ]));
        if name == "bad.png" {
            return Err(Failure::new("ValueError", "bad photo"));
        }
        if name == "locked.png" {
            return Err(Failure::new("PermissionError", "[Errno 13] locked"));
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(job.destination)
            .map_err(|e| Failure::io(&e))?;
        f.write_all(&vec![b'x'; 10 + name.chars().count()]).unwrap();
        Ok(json!([100 + name.chars().count(), 50]))
    }
    fn copy_original(&self, record: &Value, destination: &Path) -> std::result::Result<Value, Failure> {
        self.calls.lock().unwrap().push(json!(["original", destination.file_name().unwrap().to_string_lossy()]));
        let create = |p: &Path, data: &[u8]| -> std::result::Result<(), Failure> {
            let mut f =
                std::fs::OpenOptions::new().write(true).create_new(true).open(p).map_err(|e| Failure::io(&e))?;
            f.write_all(data).map_err(|e| Failure::io(&e))
        };
        create(destination, b"ooooooo")?;
        create(&destination.with_extension("xmp"), b"xmp")?;
        Ok(json!([record["info"]["width"], record["info"]["height"]]))
    }
}

fn paths(v: &Value) -> Vec<PathBuf> {
    v.as_array().unwrap().iter().map(|p| PathBuf::from(s(p))).collect()
}

fn options(o: &Value) -> ExportOptions {
    let mut options = ExportOptions::default();
    if let Some(v) = o.get("format") {
        options.format = s(v).into();
    }
    if let Some(v) = o.get("quality") {
        options.quality = v.as_i64().unwrap();
    }
    if let Some(v) = o.get("longest") {
        options.longest = Longest::from_value(v).unwrap();
    }
    if let Some(v) = o.get("color_space") {
        options.color_space = s(v).into();
    }
    if let Some(v) = o.get("keep_metadata") {
        options.keep_metadata = v.as_bool().unwrap();
    }
    if let Some(v) = o.get("naming") {
        options.naming = s(v).into();
    }
    if let Some(v) = o.get("grayscale") {
        options.grayscale = v.as_bool().unwrap();
    }
    if let Some(v) = o.get("dither") {
        options.dither = v.as_bool().unwrap();
    }
    options
}

fn call(f: &str, a: &Value) -> Result<Value> {
    let path = |v: &Value| PathBuf::from(s(v));
    let text = |p: PathBuf| Value::String(common::text_path(&p));
    Ok(match f {
        "export_folder" => {
            let sub = if a["subfolder"].is_null() { None } else { Some(&a["subfolder"]) };
            text(export::export_folder(&path(&a["root"]), sub, &paths(&a["own"]), a["create"].as_bool().unwrap())?)
        }
        "named_folder" => text(export::named_folder(&a["text"], &paths(&a["own"]), s(&a["what"]))?),
        "image_files" => {
            let cancel = AtomicBool::new(a["cancel"].as_bool().unwrap());
            match export::image_files(
                &path(&a["folder"]),
                a["recursive"].as_bool().unwrap(),
                &cancel,
                a["limit"].as_u64().unwrap() as usize,
                &paths(&a["own"]),
            )? {
                ImageFiles::Cancelled => Value::Null,
                ImageFiles::TooMany(message) => json!({"error": message}),
                ImageFiles::Files(files) => json!(files),
            }
        }
        "export_plans" => {
            let plans = export::export_plans(
                a["records"].as_array().unwrap(),
                &path(&a["folder"]),
                s(&a["format"]),
                s(&a["naming"]),
            )?;
            json!(plans.into_iter().map(|(r, p)| json!([r["id"], p])).collect::<Vec<_>>())
        }
        "export_records" => {
            let cancel_at = a["cancel_at"].as_u64().map(|v| v as usize);
            let cancel = AtomicBool::new(cancel_at == Some(0));
            let seen = Mutex::new(Vec::new());
            let progress = |i: usize| {
                seen.lock().unwrap().push(i);
                if cancel_at.is_some_and(|c| i >= c) {
                    cancel.store(true, Ordering::SeqCst);
                }
            };
            let exporter = StandIn { calls: Mutex::new(vec![]) };
            let folder = path(&a["folder"]);
            let result = export::export_records(
                a["records"].as_array().unwrap(),
                &folder,
                &options(&a["options"]),
                Some(&cancel),
                Some(&progress),
                &exporter,
                a["english"].as_bool().unwrap(),
                1,
            )?;
            let mut listing: Vec<String> = std::fs::read_dir(&folder)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            listing.sort();
            json!({"result": result, "progress": seen.into_inner().unwrap(), "calls": exporter.calls.into_inner().unwrap(),
                   "listing": listing})
        }
        "output_files" => export::output_files(a["outputs"].as_array().unwrap(), a["limit"].as_u64().unwrap() as usize),
        other => panic!("unknown export case {other}"),
    })
}

#[test]
fn export_folders_plans_and_loop_match_the_oracle() {
    let golden = case("export");
    let defaults = common::defaults();
    let root = common::fresh("export");
    let root_text = common::text_path(&root);
    for name in golden["files"].as_array().unwrap() {
        let p = root.join(s(name));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "taken").unwrap();
    }
    let scrub = Scrub::new(Some(&root_text), &defaults);
    let steps = golden["steps"].as_array().unwrap();
    for (n, step) in steps.iter().enumerate() {
        let f = s(&step["fn"]);
        let args = unplace(&step["args"], &root_text);
        let actual = scrub.value(&outcome(call(f, &args)));
        let mut expected = step.as_object().unwrap().clone();
        expected.shift_remove("fn");
        expected.shift_remove("args");
        check(&actual, &Value::Object(expected), &format!("export step {n} {f} {}", step["args"]));
    }
    eprintln!("export: {} steps exact", steps.len());
}

#[test]
fn parallel_export_keeps_the_sequential_results() {
    let golden = case("export");
    let step = golden["steps"].as_array().unwrap().iter().find(|s| s["fn"] == "export_records").unwrap();
    let mut results = Vec::new();
    for workers in [1, 3] {
        let root = common::fresh(&format!("export-workers-{workers}"));
        let root_text = common::text_path(&root);
        for name in golden["files"].as_array().unwrap() {
            let p = root.join(s(name));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "taken").unwrap();
        }
        let a = unplace(&step["args"], &root_text);
        let exporter = StandIn { calls: Mutex::new(vec![]) };
        let done = Mutex::new(Vec::new());
        let progress = |i: usize| done.lock().unwrap().push(i);
        let result = export::export_records(
            a["records"].as_array().unwrap(),
            &PathBuf::from(s(&a["folder"])),
            &options(&a["options"]),
            None,
            Some(&progress),
            &exporter,
            true,
            workers,
        )
        .unwrap();
        let mut done = done.into_inner().unwrap();
        done.sort_unstable();
        let scrub = Scrub::new(Some(&root_text), &common::defaults());
        results.push((scrub.value(&result), done));
    }
    assert_eq!(results[0], results[1]);
}
