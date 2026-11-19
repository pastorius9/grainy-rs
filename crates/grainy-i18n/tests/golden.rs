//! Goldens from the Python oracle (tools/golden/gen_i18n.py). All comparisons are exact (text and tables).

use grainy_i18n::fields::EDIT_FIELDS;
use grainy_i18n::shortcuts::{self, GROUPS, LABELS, MASKS, TABLE};
use grainy_i18n::{keys, try_tr_in, Language, Platform};
use grainy_testkit::Case;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

fn language(code: &str) -> Language {
    Language::parse(code).unwrap()
}

fn platform(name: &str) -> Platform {
    match name {
        "windows" => Platform::Windows,
        "macos" => Platform::MacOs,
        "linux" => Platform::Linux,
        other => panic!("platform {other}"),
    }
}

/// Compare a Rust result with the oracle's `{"ok": text}` or `{"error": class}`.
fn check(result: Result<String, grainy_i18n::FormatError>, expected: &Value, what: &str) {
    match (result, expected.get("ok"), expected.get("error")) {
        (Ok(a), Some(Value::String(b)), _) => assert_eq!(&a, b, "{what}"),
        (Err(e), _, Some(Value::String(class))) => assert_eq!(e.python_class(), class, "{what}: {e}"),
        (r, ok, err) => panic!("{what}: rust {r:?} vs oracle ok={ok:?} error={err:?}"),
    }
}

#[test]
fn tr_every_key_of_the_table_in_both_languages() {
    let case = Case::open("i18n", "tr_table");
    let rows = case.json()["rows"].as_array().unwrap();
    assert_eq!(rows.len() as u64, case.json()["count"].as_u64().unwrap());
    assert_eq!(rows.len(), grainy_i18n::english_table().len());
    let many = ["v0", "v1", "v2", "v3", "v4", "v5", "v6"];
    for row in rows {
        let source = row["source"].as_str().unwrap();
        for code in ["ko", "en"] {
            let lang = language(code);
            check(try_tr_in(lang, source, &[]), &row[code], &format!("{code} {source}"));
            check(try_tr_in(lang, source, &many[..1]), &row[format!("{code}_one")], &format!("{code} one {source}"));
            check(try_tr_in(lang, source, &many), &row[format!("{code}_many")], &format!("{code} many {source}"));
        }
        // The English result is the table's value and the Korean result the source.
        assert_eq!(try_tr_in(Language::Korean, source, &[]).unwrap(), source);
        assert_eq!(&try_tr_in(Language::English, source, &[]).unwrap(), &grainy_i18n::english_table()[source]);
    }
}

#[test]
fn tr_synthetic_strings() {
    let case = Case::open("i18n", "tr_synthetic");
    let cases = case.json()["cases"].as_array().unwrap();
    assert!(cases.len() > 500);
    for c in cases {
        let source = c["source"].as_str().unwrap();
        let values: Vec<&str> = c["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let lang = language(c["language"].as_str().unwrap());
        check(try_tr_in(lang, source, &values), c, &format!("{source:?} {values:?}"));
    }
}

#[test]
fn keys_on_every_platform() {
    let case = Case::open("i18n", "keys");
    for row in case.json()["rows"].as_array().unwrap() {
        let text = row["text"].as_str().unwrap();
        let p = platform(row["platform"].as_str().unwrap());
        assert_eq!(keys(text, p), row["result"].as_str().unwrap(), "{text:?} {p:?}");
    }
}

#[test]
fn shortcut_table_matches_the_oracle() {
    let case = Case::open("i18n", "shortcuts");
    let json = case.json();
    let groups: Vec<&str> = json["groups"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(groups, GROUPS);
    let table = json["table"].as_array().unwrap();
    assert_eq!(table.len(), TABLE.len());
    for (row, entry) in table.iter().zip(TABLE.iter()) {
        let keys: Vec<&str> = row["keys"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(row["group"].as_str().unwrap(), entry.group);
        assert_eq!(keys, entry.keys);
        assert_eq!(row["text"].as_str().unwrap(), entry.text);
        assert_eq!(row["label"].as_str().unwrap(), entry.label);
        assert_eq!(row["repeat"].as_bool().unwrap(), entry.repeat);
        assert_eq!(row["focus"].as_bool().unwrap(), entry.focus);
    }
    // Command ids: present, unique, "group.name" shaped.
    let ids: HashSet<&str> = TABLE.iter().map(|e| e.command).collect();
    assert_eq!(ids.len(), TABLE.len());
    assert!(TABLE.iter().all(|e| e.command.contains('.') && !e.command.contains(' ')));

    let masks: Vec<(String, String, String)> = json["masks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m[0].as_str().unwrap().into(), m[1].as_str().unwrap().into(), m[2].as_str().unwrap().into()))
        .collect();
    let ours: Vec<(String, String, String)> = MASKS.iter().map(|m| (m.0.into(), m.1.into(), m.2.into())).collect();
    assert_eq!(masks, ours);
    let labels: Vec<(String, String)> = json["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m[0].as_str().unwrap().into(), m[1].as_str().unwrap().into()))
        .collect();
    assert_eq!(labels, LABELS.iter().map(|m| (m.0.to_string(), m.1.to_string())).collect::<Vec<_>>());
    // The mask rows of the table are generated from MASKS in the oracle.
    for (kind, key, text) in MASKS {
        let row = TABLE.iter().find(|e| e.keys == [key] && e.group == "마스크").unwrap();
        assert_eq!(row.text, text);
        assert!(row.command.ends_with(kind), "{} / {kind}", row.command);
    }

    for w in json["written"].as_array().unwrap() {
        let entry = &TABLE[w["row"].as_u64().unwrap() as usize];
        let p = platform(w["platform"].as_str().unwrap());
        assert_eq!(shortcuts::written(entry, p), w["result"].as_str().unwrap(), "{:?} {p:?}", entry.keys);
    }
    for h in json["hints"].as_array().unwrap() {
        let key = h["key"].as_str().unwrap();
        let p = platform(h["platform"].as_str().unwrap());
        assert_eq!(shortcuts::key_hint(key, p).as_deref(), h["result"].as_str(), "{key} {p:?}");
    }
    assert_eq!(json["missing_hint"], "StopIteration");
    assert_eq!(shortcuts::key_hint("F13", Platform::Windows), None);
}

#[test]
fn edit_fields_match_the_oracle() {
    let case = Case::open("i18n", "edit_fields");
    let fields = case.json()["fields"].as_array().unwrap();
    assert_eq!(fields.len(), EDIT_FIELDS.len());
    for (row, field) in fields.iter().zip(EDIT_FIELDS.iter()) {
        assert_eq!(row[0].as_str().unwrap(), field.key);
        assert_eq!(row[1].as_str().unwrap(), field.label);
        assert_eq!(row[2].as_f64().unwrap(), field.min);
        assert_eq!(row[3].as_f64().unwrap(), field.max);
    }
}

#[test]
fn embedded_table_equals_what_the_compiler_makes_from_the_oracle_sources() {
    // The oracle (the Python Grainy repository) is named by GRAINY_ORACLE (CONTRIBUTING.md).
    let Some(oracle) = std::env::var_os("GRAINY_ORACLE").filter(|v| !v.is_empty()) else {
        eprintln!("skipped: GRAINY_ORACLE is not set");
        return;
    };
    let directory = PathBuf::from(oracle).join("assets").join("i18n");
    if !directory.is_dir() {
        eprintln!("skipped: the oracle's assets/i18n folder is not present");
        return;
    }
    let compiled = grainy_i18n::compile::compile_directory(&directory).unwrap();
    assert_eq!(&compiled, grainy_i18n::english_table());
}
