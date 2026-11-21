use grainy_catalog::{folders::Platform, preview, query, workflow, Catalog, Sources};
use grainy_core::Settings;
use grainy_testkit::Case;
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

const STAMP: &str = "2026-11-13T01:02:03.456789+00:00";
const TOKEN: &str = "0123456789abcdef0123456789abcdef";
/// The scripted sessions and migrations are the oracle's run on Windows (paths such as `Z:\Grainy Generated\...`):
/// they are replayed with Windows path rules on every system. The rules of each system are checked by
/// `folders_match_all_platforms`.
const GOLDEN_PLATFORM: Platform = Platform::Windows;
/// Deterministic sources with this system's path rules (tests on real files).
fn sources() -> Sources {
    Sources::new(|| STAMP.into(), || TOKEN.into())
}
/// Deterministic sources with the path rules of the goldens' system (the oracle's Windows sessions).
fn golden_sources() -> Sources {
    sources().on(GOLDEN_PLATFORM)
}
fn input(name: &str) -> PathBuf {
    grainy_testkit::root().join("golden/catalog").join(name)
}
fn generated(name: &str) -> PathBuf {
    let root = grainy_testkit::root().join("target/catalog-tests");
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let directory = root.join(name);
    if directory.exists() {
        assert_eq!(directory.parent(), Some(root.as_path()));
        fs::remove_dir_all(&directory).unwrap();
    }
    fs::create_dir_all(&directory).unwrap();
    directory
}
fn copy_case(case: &str, name: &str) -> PathBuf {
    let p = generated(name);
    fs::copy(input(case).join("start.sqlite"), p.join("catalog.sqlite")).unwrap();
    p
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}
fn id(v: &Value) -> i64 {
    v.as_i64().unwrap()
}
fn ids(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(id).collect()
}
fn settings(v: &Value) -> Settings {
    Settings::from_value(v.clone()).unwrap()
}
fn now() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-11-13 01:02:03.456789", "%Y-%m-%d %H:%M:%S%.f").unwrap()
}
fn dump(path: &Path) -> Value {
    let db = Connection::open(path).unwrap();
    let mut result = serde_json::Map::new();
    let tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for table in tables {
        let columns = db
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut statement = db
            .prepare(&format!(
                "SELECT * FROM {table} ORDER BY {}",
                columns.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(",")
            ))
            .unwrap();
        let rows = statement
            .query_map([], |r| {
                let mut row = serde_json::Map::new();
                for (index, col) in columns.iter().enumerate() {
                    let value = if table == "preview_state" && col == "token" {
                        json!(TOKEN)
                    } else {
                        match r.get_ref(index)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(i) => json!(i),
                            ValueRef::Real(f) => json!(f),
                            ValueRef::Text(t) => json!(std::str::from_utf8(t).unwrap()),
                            ValueRef::Blob(b) => {
                                json!({"blob":b.iter().map(|b|format!("{b:02x}")).collect::<String>()})
                            }
                        }
                    };
                    row.insert(col.clone(), value);
                }
                Ok(Value::Object(row))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        result.insert(table, json!(rows));
    }
    result.into()
}
fn readers(c: &Catalog, path: &Path) -> Value {
    let photos = c.photos().unwrap();
    let ids = photos.iter().map(|p| id(&p["id"])).collect::<Vec<_>>();
    let db = Connection::open(path).unwrap();
    let prefs = db
        .prepare("SELECT key FROM preferences ORDER BY key")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut photo = serde_json::Map::new();
    for i in ids.iter().chain([99999].iter()) {
        photo.insert(i.to_string(), c.photo(*i).unwrap().unwrap_or(Value::Null));
    }
    let mut history = serde_json::Map::new();
    let mut snapshots = serde_json::Map::new();
    for i in &ids {
        history.insert(i.to_string(), json!(c.histories(*i).unwrap()));
        snapshots.insert(i.to_string(), json!(c.snapshots(*i).unwrap()));
    }
    let collections = c.collections().unwrap();
    let mut collection_ids = serde_json::Map::new();
    for r in &collections {
        collection_ids.insert(id(&r["id"]).to_string(), json!(c.collection_ids(id(&r["id"])).unwrap()));
    }
    let mut preferences = serde_json::Map::new();
    for k in prefs {
        preferences.insert(k.clone(), c.preference(&k, Value::Null).unwrap());
    }
    json!({"photos":photos,"paths":c.paths().unwrap(),"photo":photo,"histories":history,"snapshots":snapshots,"presets":c.presets().unwrap(),"folder_roots":c.folder_roots().unwrap(),"removed_folders":c.removed_folders().unwrap(),"collections":collections,"collection_ids":collection_ids,"preferences":preferences,"missing_preference":c.preference("missing",json!({"fallback":true})).unwrap()})
}
fn operation(c: &Catalog, path: &Path, op: &Value) -> Value {
    let a = &op["args"];
    let method = s(&op["method"]);
    match method {
        "add" => return json!(c.add(s(&a[0]), Some(&settings(&a[1])), Some(&a[2]), true).unwrap()),
        "update" => c.update(id(&a[0]), a[1].as_object().unwrap()).unwrap(),
        "update_many" => c.update_many(&ids(&a[0]), s(&a[1]), &a[2]).unwrap(),
        "edit" => c.edit(id(&a[0]), &settings(&a[1]), s(&a[2])).unwrap(),
        "set_settings" => c.set_settings(id(&a[0]), &settings(&a[1])).unwrap(),
        "set_user_metadata" => c.set_user_metadata(id(&a[0]), a[1].as_object().unwrap()).unwrap(),
        "snapshot" => c.snapshot(id(&a[0]), s(&a[1])).unwrap(),
        "virtual_copy" => return json!(c.virtual_copy(id(&a[0])).unwrap()),
        "stack" => c.stack(&ids(&a[0])).unwrap(),
        "save_preset" => c.save_preset(s(&a[0]), &settings(&a[1])).unwrap(),
        "delete_preset" => c.delete_preset(s(&a[0])).unwrap(),
        "save_adobe_preset" => c.save_adobe_preset(s(&a[0]), &a[1], |_| Ok(op["translated"].clone())).unwrap(),
        "save_preference" => c.save_preference(s(&a[0]), &a[1]).unwrap(),
        "add_collection" => {
            return json!(c
                .add_collection(s(&a[0]), if a[1].is_null() { None } else { Some(&a[1]) }, a[2].as_i64())
                .unwrap())
        }
        "collection_add" => c.collection_add(id(&a[0]), &ids(&a[1])).unwrap(),
        "collection_ids" => return json!(c.collection_ids(id(&a[0])).unwrap()),
        "remove_collection" => c.remove_collection(id(&a[0])).unwrap(),
        "remove_photos" => c.remove_photos(&ids(&a[0])).unwrap(),
        "remove_folder" => return json!(c.remove_folder(s(&a[0])).unwrap()),
        "restore_folder" => c.restore_folder(s(&a[0])).unwrap(),
        "register_folder" => return json!(c.register_folder(s(&a[0]), true).unwrap()),
        "folder_photo_ids" => return json!(c.folder_photo_ids(s(&a[0])).unwrap()),
        "backup" => {
            return json!(c
                .backup(path.parent().unwrap().join(s(&a[0])))
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap())
        }
        "histories" => return json!(c.histories(id(&a[0])).unwrap()),
        "readers" => return readers(c, path),
        "query" => {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            return query::query_on(GOLDEN_PLATFORM, path, &a[0], &cancel, now()).unwrap();
        }
        "read_page" => return query::read_page(path, &ids(&a[0])).unwrap(),
        "repeat_edit" => {
            for i in 0..id(&a[1]) {
                c.edit(id(&a[0]), &settings(&json!({"exposure":i as f64/100.0})), "history-cap").unwrap();
            }
            return json!(c.histories(id(&a[0])).unwrap().len());
        }
        "workflow.stack_order" => return json!(workflow::stack_order(c, id(&a[0]), a[1].as_i64()).unwrap()),
        "workflow.stack_members" => return json!(workflow::stack_members(c, id(&a[0])).unwrap()),
        "workflow.toggle_quick" => return json!(workflow::toggle_quick(c, &ids(&a[0]), "빠른 컬렉션").unwrap()),
        "workflow.save_metadata_preset" => workflow::save_metadata_preset(c, s(&a[0]), &a[1]).unwrap(),
        "workflow.metadata_presets" => return workflow::metadata_presets(c).unwrap(),
        "workflow.apply_metadata_preset" => workflow::apply_metadata_preset(c, &ids(&a[0]), &a[1]).unwrap(),
        "workflow.add_keyword" => workflow::add_keyword(c, &ids(&a[0]), s(&a[1])).unwrap(),
        "workflow.edit_signature" => return json!(workflow::edit_signature(&settings(&a[0])).unwrap()),
        "workflow.record_exports" => workflow::record_exports(c, a[0].as_array().unwrap()).unwrap(),
        "workflow.export_history" => return json!(workflow::export_history(c, &ids(&a[0])).unwrap()),
        "text_hash" => return json!(workflow::text_hash(s(&a[0]))),
        _ => panic!("unhandled fixture operation {method}"),
    }
    Value::Null
}
fn equal(actual: &Value, expected: &Value, where_: &str) {
    if actual == expected {
        return;
    }
    match (actual, expected) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                equal(a.get(k).unwrap_or(&Value::Null), v, &format!("{where_}.{k}"));
            }
            assert_eq!(a.len(), b.len(), "{where_}: object size");
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{where_}: array length");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                equal(a, b, &format!("{where_}[{i}]"));
            }
        }
        _ => panic!("{where_}\nactual: {actual}\nexpected: {expected}"),
    }
}

#[test]
fn scripted_writers_and_all_readers_match_oracle() {
    let case = Case::open("catalog", "operations");
    let directory = copy_case("operations", "roundtrip");
    let path = directory.join("catalog.sqlite");
    let c = Catalog::with_sources(&directory, golden_sources()).unwrap();
    for (index, op) in case.json()["steps"].as_array().unwrap().iter().enumerate() {
        let label = format!("operation {index} {}", op["method"]);
        let actual = operation(&c, &path, op);
        equal(&actual, &op["expected"], &label);
        let tables = dump(&path);
        for (table, expected) in op["delta"].as_object().unwrap() {
            equal(&tables[table], expected, &format!("{label} table {table}"));
        }
        if let Some(expected) = op.get("tables") {
            equal(&tables, expected, &format!("{label} tables"));
        }
        println!("{label}: exact match, max error 0");
    }
    equal(&dump(&path), &case.json()["final_tables"], "final tables");
    equal(&readers(&c, &path), &case.json()["final_readers"], "final readers");
    c.close().unwrap();
    for name in ["half", "raw"] {
        let packet = preview::read_offline(input("preview").join(format!("{name}.npz"))).unwrap();
        preview::write_offline(directory.join(format!("{name}.npz")), &packet).unwrap();
    }
}
#[test]
fn python_written_database_all_public_readers() {
    let case = Case::open("catalog", "operations");
    let directory = generated("python-read");
    fs::copy(input("operations").join("oracle.sqlite"), directory.join("catalog.sqlite")).unwrap();
    let c = Catalog::open_reader(&directory).unwrap();
    equal(&readers(&c, &directory.join("catalog.sqlite")), &case.json()["final_readers"], "oracle readers");
    c.close().unwrap();
    println!("Python database readers: max error 0");
}
#[test]
fn every_historical_migration_matches() {
    for stage in ["v1", "v2", "v3", "preview", "profiles", "current"] {
        let name = format!("migration_{stage}");
        let case = Case::open("catalog", &name);
        let directory = copy_case(&name, &name);
        let path = directory.join("catalog.sqlite");
        let c = Catalog::with_sources(&directory, golden_sources()).unwrap();
        equal(&dump(&path), &case.json()["tables"], &name);
        equal(&readers(&c, &path), &case.json()["readers"], &format!("{name} readers"));
        let mut backups = if directory.join("backups").exists() {
            fs::read_dir(directory.join("backups"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        backups.sort();
        assert_eq!(json!(backups), case.json()["backups"]);
        for backup in backups {
            let db = Connection::open(directory.join("backups").join(backup)).unwrap();
            assert_eq!(db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
        }
        c.close().unwrap();
        let c = Catalog::with_sources(&directory, Sources::default().on(GOLDEN_PLATFORM)).unwrap();
        equal(&dump(&path), &case.json()["tables"], "reopening migration is idempotent");
        c.close().unwrap();
        println!("migration {stage}: max error 0");
    }
}
#[test]
fn folders_match_all_platforms() {
    let case = Case::open("catalog", "folders");
    for scenario in case.json()["scenarios"].as_array().unwrap() {
        let platform = match s(&scenario["platform"]) {
            "windows" => Platform::Windows,
            "mac" => Platform::Mac,
            _ => Platform::Posix,
        };
        for item in scenario["items"].as_array().unwrap() {
            assert_eq!(platform.path_key(s(&item["path"])), s(&item["key"]), "path_key {item}");
            assert_eq!(platform.stored_path(s(&item["path"])), s(&item["stored"]));
        }
        for r in scenario["relations"].as_array().unwrap() {
            let (a, b) = (s(&r["a"]), s(&r["b"]));
            assert_eq!(json!(platform.in_folder(a, b, true)), r["in"], "in_folder {r}");
            assert_eq!(json!(platform.in_folder(a, b, false)), r["direct"], "direct {r}");
            assert_eq!(json!(platform.contains_folder(a, b)), r["contains"], "contains {r}");
            if r["below"].is_null() {
                assert!(platform.below(a, b).is_err());
            } else {
                assert_eq!(platform.below(a, b).unwrap(), s(&r["below"]));
            }
        }
        let roots = scenario["roots"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect::<Vec<_>>();
        equal(
            &json!(platform.folder_nodes(scenario["photos"].as_array().unwrap(), &roots).unwrap()),
            &scenario["nodes"],
            "folder nodes",
        );
        for r in scenario["left_out"].as_array().unwrap() {
            let list = |k: &str| r[k].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect::<Vec<_>>();
            assert_eq!(json!(platform.left_out(s(&r["path"]), &list("registered"), &list("removed"))), r["expected"]);
        }
        println!("folders {platform:?}: max error 0");
    }
}
#[test]
fn readonly_snapshots_never_migrate_or_leave_journals() {
    let directory = copy_case("operations", "readonly");
    let path = directory.join("catalog.sqlite");
    let before = fs::read(&path).unwrap();
    let c = Catalog::open_reader(&directory).unwrap();
    assert!(c.save_preference("bad", &json!(1)).is_err());
    c.photos().unwrap();
    c.close().unwrap();
    assert_eq!(before, fs::read(&path).unwrap());
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    let writer = Catalog::with_sources(&directory, sources()).unwrap();
    let reader = Catalog::open_reader(&directory).unwrap();
    assert!(reader.photos().unwrap().is_empty());
    writer.add(r"Z:\Generated\snapshot.jpg", None, None, true).unwrap();
    assert!(reader.photos().unwrap().is_empty());
    reader.close().unwrap();
    assert_eq!(Catalog::open_reader(&directory).unwrap().photos().unwrap().len(), 1);
    writer.close().unwrap();
}
#[test]
fn offline_packets_match_float16_and_float32_goldens() {
    let case = Case::open("catalog", "preview");
    for name in ["half", "raw"] {
        let packet = preview::read_offline(input("preview").join(format!("{name}.npz"))).unwrap();
        let expected: grainy_core::Image = case.array(&format!("{name}_pixels"));
        assert_eq!(packet.pixels.values, expected.as_slice().unwrap());
        assert_eq!(packet.pixels.shape, [4, 6, 3]);
        assert_eq!(packet.pixels.half, name == "half");
        if name == "raw" {
            let expected: grainy_core::Image = case.array("raw_legacy");
            assert_eq!(packet.legacy_pixels.as_ref().unwrap().values, expected.as_slice().unwrap());
        }
        let directory = generated(&format!("npz-{name}"));
        let path = directory.join("packet.npz");
        preview::write_offline(&path, &packet).unwrap();
        assert_eq!(preview::read_offline(path).unwrap(), packet);
        println!("offline {name}: max absolute error 0, tolerance 0");
    }
}

#[test]
fn all_smart_rule_branches_match_oracle() {
    let case = Case::open("catalog", "predicates");
    for (index, r) in case.json()["rules"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            json!(query::matches(&case.json()["photo"], &r["rule"], now(), Platform::Windows).unwrap()),
            r["expected"],
            "predicate {index}: {}",
            r["rule"]
        );
    }
    println!("{} predicates: max error 0", case.json()["rules"].as_array().unwrap().len());
}

#[test]
fn preview_validity_rules_match_oracle() {
    let case = Case::open("catalog", "preview");
    let directory = generated("preview-status");
    fs::create_dir_all(directory.join("thumbnails")).unwrap();
    let payload = fs::read(input("preview").join("packet.jpg")).unwrap();
    for scenario in case.json()["scenarios"].as_array().unwrap() {
        let (jpeg, stamp) = preview::files(&directory, 1);
        if jpeg.exists() {
            fs::remove_file(&jpeg).unwrap();
        }
        match s(&scenario["kind"]) {
            "jpeg" => fs::write(&jpeg, &payload).unwrap(),
            "corrupt" => fs::write(&jpeg, b"invalid jpeg").unwrap(),
            _ => {}
        }
        fs::write(stamp, serde_json::to_vec(&scenario["stamp"]).unwrap()).unwrap();
        let mut inspection = preview::inspect_snapshot(
            &directory,
            1,
            if scenario["current"].is_null() { None } else { Some(scenario["current"].clone()) },
        )
        .unwrap();
        inspection.state["image"] = inspection.image.is_some().into();
        equal(&inspection.state, &scenario["expected"], s(&scenario["name"]));
    }
    println!("23 preview validity scenarios: max error 0");
}

#[test]
fn preview_atomic_publish_and_stale_worker_rejection() {
    let directory = generated("preview-publish");
    let source = directory.join("generated-source.bin");
    fs::write(&source, b"generated bytes").unwrap();
    let c = Catalog::with_sources(&directory, sources()).unwrap();
    let id = c.add(source.to_str().unwrap(), None, Some(&json!({"test":"metadata"})), false).unwrap();
    let state = preview::snapshot(&directory, id, true).unwrap().unwrap();
    assert_eq!(state["info"], json!({"test":"metadata"}));
    assert_eq!(state["source"], preview::signature(&source).unwrap());
    assert!(!state["source"].is_null());
    assert!(preview::signature(&directory).is_none());
    let jpeg = fs::read(input("preview").join("packet.jpg")).unwrap();
    assert!(!preview::publish(&directory, &state, &jpeg, Some(&AtomicBool::new(true)), "sRGB").unwrap());
    assert!(preview::publish(&directory, &state, &jpeg, None, "sRGB").unwrap());
    let inspection = preview::inspect(&directory, id).unwrap();
    assert_eq!(inspection.state["rebuild"], false);
    assert_eq!(inspection.state["status"], "");
    assert_eq!(inspection.image.as_ref().unwrap(), &jpeg);
    let (file, stamp) = preview::files(&directory, id);
    let original = fs::read(&file).unwrap();
    let original_stamp = fs::read(&stamp).unwrap();
    let mut stale = state.clone();
    stale["token"] = "stale".into();
    assert!(!preview::publish(&directory, &stale, &jpeg, None, "sRGB").unwrap());
    stale = state.clone();
    stale["path"] = "wrong".into();
    assert!(!preview::publish(&directory, &stale, &jpeg, None, "sRGB").unwrap());
    assert_eq!(fs::read(&file).unwrap(), original);
    assert_eq!(fs::read(&stamp).unwrap(), original_stamp);
    assert!(preview::store_render(
        &directory,
        id,
        &settings(&state["settings"]),
        &jpeg,
        &state["source"],
        &state["offline"],
        "ProPhoto RGB"
    )
    .unwrap());
    assert!(!preview::store_render(
        &directory,
        id,
        &settings(&json!({"exposure":99})),
        &jpeg,
        &state["source"],
        &state["offline"],
        "sRGB"
    )
    .unwrap());
    fs::write(&source, b"changed generated bytes").unwrap();
    assert!(!preview::publish(&directory, &state, &jpeg, None, "sRGB").unwrap());
    assert_eq!(preview::inspect(&directory, id).unwrap().state["rebuild"], true);
    fs::remove_file(&source).unwrap();
    assert_eq!(preview::inspect(&directory, id).unwrap().state["status"], "오프라인");
    fs::create_dir_all(directory.join("previews")).unwrap();
    let offline = directory.join(format!("previews/{id}.npz"));
    fs::copy(input("preview").join("half.npz"), &offline).unwrap();
    let state = preview::snapshot(&directory, id, true).unwrap().unwrap();
    assert!(preview::publish(&directory, &state, &jpeg, None, "sRGB").unwrap());
    fs::write(&offline, b"changed").unwrap();
    assert!(!preview::publish(&directory, &state, &jpeg, None, "sRGB").unwrap());
    c.remove_photos(&[id]).unwrap();
    assert!(!preview::publish(&directory, &state, &jpeg, None, "sRGB").unwrap());
    assert_eq!(preview::inspect(&directory, id).unwrap().state["status"], "카탈로그에서 제거됨");
    assert_eq!(fs::read_dir(directory.join("thumbnails")).unwrap().count(), 2);
    c.close().unwrap();
}

#[test]
fn original_batch_v1_v2_storage_and_hashes_survive() {
    for name in ["batch_v1", "batch_v2"] {
        let case = Case::open("catalog", name);
        let directory = copy_case(name, name);
        let path = directory.join("catalog.sqlite");
        let c = Catalog::with_sources(&directory, sources()).unwrap();
        equal(&readers(&c, &path), &case.json()["readers"], name);
        let before = dump(&path);
        for key in ["undo:1", "undo:command_batch"] {
            let value = c.preference(key, Value::Null).unwrap();
            c.save_preference(key, &value).unwrap();
        }
        equal(&dump(&path), &before, &format!("{name} exact stored text"));
        c.close().unwrap();
        println!("{name}: original 81-entry undo state and batch hashes preserved, max error 0");
    }
}

#[test]
fn invalid_inputs_rollback_and_noop_tokens() {
    let directory = generated("failures");
    let path = directory.join("catalog.sqlite");
    let c = Catalog::new(&directory).unwrap();
    let first = c.add(r"Z:\Generated\one.jpg", None, None, true).unwrap();
    let second = c.add(r"Z:\Generated\two.jpg", None, None, true).unwrap();
    let original = dump(&path);
    let token = preview::snapshot(&directory, first, false).unwrap().unwrap()["token"].clone();
    c.edit(first, &settings(&json!({})), "no-op").unwrap();
    c.set_settings(first, &settings(&json!({}))).unwrap();
    assert_eq!(preview::snapshot(&directory, first, false).unwrap().unwrap()["token"], token);
    assert_eq!(dump(&path), original);
    assert!(c.update_many(&[first], "name; DROP TABLE photos", &json!("bad")).is_err());
    assert!(c.snapshot(999, "missing").is_err());
    assert!(c.virtual_copy(999).is_err());
    assert!(c.set_user_metadata(999, json!({"title":"x"}).as_object().unwrap()).is_err());
    let unrelated = directory.join("original.tif");
    fs::write(&unrelated, b"original").unwrap();
    assert!(c.backup(&unrelated).is_err());
    assert_eq!(fs::read(&unrelated).unwrap(), b"original");
    let saved = c.backup(directory.join("safety.sqlite")).unwrap();
    let archive = c.portable_backup(directory.join("cloud.sqlite")).unwrap();
    let archived = Connection::open(archive).unwrap();
    let mode: String = archived.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "delete");
    let old = fs::read(&saved).unwrap();
    fs::write(directory.join("safety.sqlite-wal"), b"busy").unwrap();
    assert!(c.backup(&saved).is_err());
    assert_eq!(fs::read(&saved).unwrap(), old);
    assert!(c.backup(&path).is_err());
    assert!(c.backup(directory.join("thumbnails/../catalog.sqlite")).is_err());
    for profile in [
        json!({"sha256":"bad","data":"AA=="}),
        json!({"sha256":"0".repeat(64),"data":"!"}),
        json!({"sha256":"0".repeat(64),"data_ref":"0".repeat(64)}),
    ] {
        assert!(c.edit(first, &settings(&json!({"dcp_profile":profile})), "invalid").is_err());
        assert_eq!(dump(&path), original);
    }
    assert!(workflow::apply_metadata_preset(&c, &[first, 999], &json!({"title":"rolled back"})).is_err());
    assert!(workflow::add_keyword(&c, &[first, 999], "rolled back").is_err());
    assert_eq!(dump(&path), original);
    for values in
        [json!({"latitude":91}), json!({"longitude":181}), json!({"DateTimeOriginal":"bad"}), json!({"unknown":"bad"})]
    {
        assert!(workflow::save_metadata_preset(&c, "test", &values).is_err());
    }
    assert!(workflow::save_metadata_preset(&c, "  ", &json!({})).is_err());
    assert!(workflow::add_keyword(&c, &[first], "one,two").is_err());
    assert!(c.save_adobe_preset("bad", &json!({"_adobe_preset":{}}), |_| Ok(json!({"mapped":[]}))).is_err());
    assert_eq!(dump(&path), original);
    assert!(matches!(
        query::query(&path, &json!({}), &std::sync::Arc::new(AtomicBool::new(true)), now()),
        Err(grainy_catalog::Error::Cancelled)
    ));
    c.edit(first, &settings(&json!({"exposure":1})), "changed").unwrap();
    assert_ne!(preview::snapshot(&directory, first, false).unwrap().unwrap()["token"], token);
    // Trigger errors after a successful first row must roll back the entire bulk update.
    let db = Connection::open(&path).unwrap();
    db.execute_batch(&format!("CREATE TRIGGER reject_second BEFORE UPDATE OF rating ON photos WHEN NEW.id={second} BEGIN SELECT RAISE(ABORT,'test'); END;")).unwrap();
    assert!(c.update_many(&[first, second], "rating", &json!(5)).is_err());
    assert_eq!(c.photo(first).unwrap().unwrap()["rating"], 0);
    drop(db);
    c.close().unwrap();
}

#[test]
fn offline_loading_selection_metadata_and_converter_boundary() {
    let case = Case::open("catalog", "preview");
    let directory = generated("offline-load");
    fs::create_dir_all(directory.join("previews")).unwrap();
    for scenario in case.json()["loads"].as_array().unwrap() {
        fs::copy(input("preview").join(format!("{}.npz", s(&scenario["name"]))), directory.join("previews/1.npz"))
            .unwrap();
        let result = preview::load_offline(&directory, 1, &settings(&scenario["settings"]), |mut p, from, to| {
            assert_eq!(from, "Linear sRGB");
            assert_eq!(to, "Linear ProPhoto");
            for v in &mut p.values {
                *v *= 2.0;
            }
            Ok(p)
        });
        if scenario["error"] == true {
            assert!(result.is_err());
            continue;
        }
        let (pixels, mut info) = result.unwrap();
        assert!(!pixels.half);
        assert!(info["offline_fingerprint"].is_object());
        info["offline_fingerprint"] = "<fingerprint>".into();
        equal(&info, &scenario["info"], "offline load info");
        let expected = scenario["pixels"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap())
            .flat_map(|pixel| pixel.as_array().unwrap())
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<_>>();
        assert_eq!(pixels.values, expected);
    }
}

#[test]
fn migrations_preserve_schema_indexes_and_triggers() {
    for stage in ["v1", "v2", "v3", "preview", "profiles", "current"] {
        let name = format!("migration_{stage}");
        let case = Case::open("catalog", &name);
        let directory = copy_case(&name, &format!("schema-{stage}"));
        let c = Catalog::new(&directory).unwrap();
        let db = Connection::open(directory.join("catalog.sqlite")).unwrap();
        let schema = db
            .prepare(
                "SELECT type,name,tbl_name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
            )
            .unwrap()
            .query_map([], |r| Ok(vec![r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let normalized = |v: &Value| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    row.as_array()
                        .unwrap()
                        .iter()
                        .map(|v| s(v).split_whitespace().collect::<Vec<_>>().join(" "))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(normalized(&json!(schema)), normalized(&case.json()["schema"]), "schema {stage}");
        drop(db);
        c.close().unwrap();
    }
}

#[test]
fn invalid_legacy_profile_can_be_repaired_without_losing_history() {
    let case = Case::open("catalog", "legacy_profile");
    let directory = copy_case("legacy_profile", "legacy-profile");
    let path = directory.join("catalog.sqlite");
    let c = Catalog::with_sources(&directory, sources()).unwrap();
    c.edit(1, &settings(&json!({"exposure":3})), "repair legacy profile").unwrap();
    equal(&dump(&path), &case.json()["tables"], "legacy profile tables");
    equal(&readers(&c, &path), &case.json()["readers"], "legacy profile readers");
    c.close().unwrap();
}

#[test]
fn missing_corrupt_and_external_profile_references_fail_closed() {
    for mode in ["missing", "corrupt", "oversize", "wrong-type", "wrong-size", "no-table"] {
        let directory = generated(&format!("profile-{mode}"));
        fs::copy(input("operations").join("oracle.sqlite"), directory.join("catalog.sqlite")).unwrap();
        let db = Connection::open(directory.join("catalog.sqlite")).unwrap();
        match mode {
            "missing" => {
                db.execute("DELETE FROM profile_assets", []).unwrap();
            }
            "corrupt" => {
                db.execute("UPDATE profile_assets SET data=zeroblob(size)", []).unwrap();
            }
            "oversize" => {
                db.execute("UPDATE profile_assets SET size=4194305", []).unwrap();
            }
            "wrong-type" => {
                db.execute("UPDATE profile_assets SET data='text'", []).unwrap();
            }
            "wrong-size" => {
                db.execute("UPDATE profile_assets SET size=size+1", []).unwrap();
            }
            _ => {
                db.execute("DROP TABLE profile_assets", []).unwrap();
            }
        }
        drop(db);
        let c = Catalog::open_reader(&directory).unwrap();
        assert!(c.presets().is_err(), "{mode}");
        c.close().unwrap();
    }
}

#[test]
fn busy_restore_and_backup_preserve_both_catalogues() {
    let tmp = tempfile::tempdir().unwrap();
    let live = Catalog::new(tmp.path()).unwrap();
    let backup = tmp.path().join("backup.sqlite");
    live.add("generated-first.jpg", None, None, true).unwrap();
    live.backup(&backup).unwrap();
    let initial = fs::read(&backup).unwrap();
    live.add("generated-second.jpg", None, None, true).unwrap();
    let archive = Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    live.connection().execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(live.restore_from(&archive).is_err());
    drop(archive);
    let another = tmp.path().join("other.sqlite");
    assert!(live.backup(&another).is_err());
    live.connection().execute_batch("ROLLBACK").unwrap();
    assert!(!another.exists());
    assert_eq!(fs::read(backup).unwrap(), initial);
    assert_eq!(live.connection().query_row("SELECT COUNT(*) FROM photos", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
}

#[test]
fn batching_over_500_deletes_all_related_rows_and_keeps_sources() {
    let directory = generated("large-batch");
    let c = Catalog::new(&directory).unwrap();
    let db = Connection::open(directory.join("catalog.sqlite")).unwrap();
    db.execute_batch("BEGIN; WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<1005) INSERT INTO photos(id,path,name,edits,imported) SELECT i,'Z:\\Generated\\'||i||'.jpg','generated','{}','generated' FROM n; INSERT INTO history(photo_id,label,edits,created) SELECT id,'generated','{}','generated' FROM photos; INSERT INTO snapshots(photo_id,name,edits,created) SELECT id,'generated','{}','generated' FROM photos; INSERT INTO collection_members SELECT 1,id FROM photos; INSERT INTO preferences SELECT 'undo:'||id,'{}' FROM photos; COMMIT;").unwrap();
    let ids = (1..=1005).collect::<Vec<i64>>();
    assert_eq!(query::read_page(directory.join("catalog.sqlite"), &ids).unwrap().as_object().unwrap().len(), 1005);
    c.remove_photos(&ids).unwrap();
    for table in [
        "photos",
        "history",
        "snapshots",
        "collection_members",
        "preferences",
        "preview_state",
        "browser_dirty",
        "browser_index",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap(),
            0,
            "{table}"
        );
    }
    drop(db);
    c.close().unwrap();
}

#[test]
fn indexed_substrings_keep_literal_unicode_and_track_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let c = Catalog::new(tmp.path()).unwrap();
    let path = tmp.path().join("catalog.sqlite");
    c.connection().execute_batch("WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM seq WHERE i<600)
        INSERT INTO photos(path,name,edits,imported) SELECT printf('/roll/%d.png',i),printf('%d.png',i),'{}','' FROM seq;
        UPDATE photos SET name='Straße_%\"FÄRBEN.png' WHERE id=2;
        UPDATE photos SET keywords='pre' || char(0) || 'NUL-marker' WHERE id=3").unwrap();
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let search = |term| query::query(&path, &json!({"search":term}), &cancel, now()).unwrap()["ids"].clone();
    assert_eq!(search("SSE_%"), json!([2]));
    assert_eq!(search("%\"Fä"), json!([2]));
    assert_eq!(search("nul-marker"), json!([3]));
    assert_eq!(search("\0NUL"), json!([3]));
    c.connection().execute("UPDATE photos SET name='new name',keywords='new value' WHERE id=2", []).unwrap();
    assert_eq!(search("SSE_%"), json!([]));
    assert_eq!(search("W VAL"), json!([2]));
    c.remove_photos(&[2]).unwrap();
    assert_eq!(search("W VAL"), json!([]));
    let all = || query::query(&path, &json!({}), &cancel, now()).unwrap();
    let before = all();
    assert_eq!(before["total"], 599);
    assert_eq!(before["folders"].as_array().unwrap().len(), 1);
    c.connection().execute("UPDATE photos SET path='/another/3.png' WHERE id=3", []).unwrap();
    let moved = all();
    assert_eq!(moved["total"], 599);
    assert_eq!(moved["folders"].as_array().unwrap().len(), 2);
    assert!(moved["folders"].as_array().unwrap().iter().any(|folder| folder["count"] == 1));
    c.remove_photos(&[3]).unwrap();
    let after = all();
    assert_eq!(after["total"], 598);
    assert_eq!(after["folders"].as_array().unwrap().len(), 1);
}

#[test]
#[ignore = "measures warm filename search at three library sizes"]
fn large_library_filename_search() {
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    for size in [1000, 10_000, 100_000] {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = Catalog::new(tmp.path()).unwrap();
        catalog
            .connection()
            .execute_batch(&format!(
                "WITH RECURSIVE nums(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM nums WHERE i<{size})
             INSERT INTO photos(path,name,edits,imported)
             SELECT printf('C:/photos/img%07d.jpg',i),printf('img%07d.jpg',i),'{{}}','2026-11-21' FROM nums"
            ))
            .unwrap();
        let path = tmp.path().join("catalog.sqlite");
        let search = || query::query(&path, &json!({"search":"img0000001.jpg"}), &cancel, now()).unwrap();
        assert_eq!(search()["ids"], json!([1]));
        let mut elapsed = Vec::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            assert_eq!(search()["ids"], json!([1]));
            elapsed.push(start.elapsed().as_micros());
        }
        println!("{size} photos, warm filename search in microseconds: {elapsed:?}");
    }
}

#[test]
fn interrupted_trash_keeps_recoverable_edits_and_reconciles_on_open() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("library");
    let source = root.path().join("frame.dng");
    let bin = root.path().join("recycled-frame.dng");
    fs::write(&source, b"test original").unwrap();
    let c = Catalog::new(&folder).unwrap();
    let id = c.add(source.to_str().unwrap(), None, None, false).unwrap();
    let copy = c.virtual_copy(id).unwrap();
    let record = c.photo(id).unwrap().unwrap();
    let catalog_path = record["path"].as_str().unwrap();
    c.connection()
        .execute_batch(
            "CREATE TRIGGER refuse_delete BEFORE DELETE ON photos BEGIN
        SELECT RAISE(ABORT,'catalog cannot accept delete'); END",
        )
        .unwrap();
    let result = c.trash_original(catalog_path, |_| panic!("bin called despite a failed catalog write"));
    assert!(result.is_err());
    assert!(source.exists());
    assert!(c.photo(copy).unwrap().is_some());

    // A deferred constraint fails at COMMIT after the filesystem move, simulating a
    // rollback that leaves the durable intent and the pre-move catalog records intact.
    c.connection()
        .execute_batch(
            "DROP TRIGGER refuse_delete; PRAGMA foreign_keys=ON;
        CREATE TABLE fail_parent(id INTEGER PRIMARY KEY);
        CREATE TABLE fail_child(parent INTEGER REFERENCES fail_parent(id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER fail_commit AFTER DELETE ON photos BEGIN INSERT INTO fail_child VALUES(1); END",
        )
        .unwrap();
    assert!(c.trash_original(catalog_path, |path| Ok(fs::rename(path, &bin)?)).is_err());
    assert!(!source.exists());
    assert!(c.photo(id).unwrap().is_some());
    assert!(c.photo(copy).unwrap().is_some());
    c.connection().execute_batch("DROP TRIGGER fail_commit").unwrap();
    drop(c);
    let reopened = Catalog::new(&folder).unwrap();
    assert!(reopened.photo(id).unwrap().is_none());
    assert!(reopened.photo(copy).unwrap().is_none());
    assert_eq!(fs::read(bin).unwrap(), b"test original");
}
