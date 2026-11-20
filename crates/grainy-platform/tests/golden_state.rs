//! Goldens for folders, settings file, crash marker, what's-new notes and feedback records: the oracle's results
//! and the bytes of the files it wrote (exact comparisons).

mod common;

use std::path::Path;

use common::*;
use grainy_i18n::Language;
use grainy_platform::feedback::{self, Gpu, Request, SendFailure, SystemFacts};
use grainy_platform::json::Value;
use grainy_platform::safe_start;
use grainy_platform::whats_new::{self, Entry};
use grainy_platform::{local_data, Environment, Preferences, System};
use sha2::{Digest, Sha256};

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// What Python's `read_text` gives: CRLF and a lone CR read as LF.
fn text_of(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n").replace('\r', "\n")
}

fn lang(code: &str) -> Language {
    Language::parse(code).unwrap()
}

fn as_serde(object: &grainy_platform::json::Object) -> serde_json::Value {
    serde_json::from_str(&Value::Object(object.clone()).dump(None, false)).unwrap()
}

fn summary(entries: &[Entry]) -> Vec<(String, String, usize, String)> {
    entries.iter().map(|e| (e.version.clone(), e.title.clone(), e.body.chars().count(), sha(&e.body))).collect()
}

fn expected_summary(value: &serde_json::Value) -> Vec<(String, String, usize, String)> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["version"].as_str().unwrap().into(),
                e["title"].as_str().unwrap().into(),
                e["chars"].as_u64().unwrap() as usize,
                e["sha256"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[test]
fn local_data_matches_the_oracle_on_both_systems() {
    for case in golden("user_paths")["cases"].as_array().unwrap() {
        let mac = case["mac"].as_bool().unwrap();
        let system = if mac { System::MacOs } else { System::Windows };
        let env = Environment {
            system,
            local_app_data: case["local_app_data"].as_str().map(str::to_string),
            home: case["home"].as_str().unwrap().into(),
        };
        let fallback = case["fallback"].as_str().map(Path::new);
        let got = local_data(&env, case["folder"].as_str().unwrap(), fallback).to_string_lossy().replace('\\', "/");
        assert_eq!(got, case["expected"].as_str().unwrap(), "{case}");
    }
    assert_eq!(System::Windows.native_suffix(), Some(".dll"));
    assert_eq!(System::MacOs.native_suffix(), Some(".dylib"));
    assert_eq!(System::Other.native_suffix(), None);
}

#[test]
fn the_settings_file_is_written_and_merged_like_the_oracle() {
    let mut count = 0;
    for case in golden("preferences")["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let root = Temp::new();
        let path = root.0.join("settings").join("settings.json");
        if let Some(initial) = case["initial"].as_str() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, initial.as_bytes()).unwrap();
        }
        let mut prefs = Preferences::new(&path);
        for step in case["steps"].as_array().unwrap() {
            let [kind, key, arg] = [&step["op"][0], &step["op"][1], &step["op"][2]];
            let mut outcome = "ok";
            match kind.as_str().unwrap() {
                "save" => prefs.save(key.as_str().unwrap(), Value::parse(arg.as_str().unwrap()).unwrap()).unwrap(),
                "language" => {
                    if prefs.save_language(key.as_str().unwrap()).is_err() {
                        outcome = "error";
                    }
                }
                "gpu" => prefs.save_use_gpu(key.as_bool().unwrap()).unwrap(),
                "external" => {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, arg.as_str().unwrap().as_bytes()).unwrap();
                }
                _ => {}
            }
            assert_eq!(outcome, step["outcome"].as_str().unwrap(), "{name}: {}", step["op"]);
            let file = std::fs::read(&path).ok().map(|b| String::from_utf8(b).unwrap());
            // The oracle wrote the goldens on Windows, where Python's text mode ends its lines with CRLF; on
            // macOS it writes LF (`preferences::LINE_END`). Contents a step writes itself have no CRLF.
            let expected = step["file"].as_str().map(|f| f.replace("\r\n", grainy_platform::preferences::LINE_END));
            assert_eq!(file.as_deref(), expected.as_deref(), "{name}: file after {}", step["op"]);
            assert_eq!(
                prefs.language().map(Language::code),
                step["language"].as_str(),
                "{name}: language after {}",
                step["op"]
            );
            assert_eq!(prefs.use_gpu(), step["use_gpu"].as_bool().unwrap(), "{name}: use_gpu after {}", step["op"]);
            let mut folder: Vec<String> = std::fs::read_dir(path.parent().unwrap())
                .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            folder.sort();
            assert_eq!(folder, strings(&step["folder"]), "{name}: no temporary file left");
            count += 1;
        }
        let again = Preferences::new(&path);
        assert_eq!(again.language().map(Language::code), case["reread_language"].as_str(), "{name}");
        assert_eq!(again.use_gpu(), case["reread_use_gpu"].as_bool().unwrap(), "{name}");
    }
    println!("preferences: {count} steps byte-identical");
}

#[test]
fn the_crash_marker_is_read_and_written_like_the_oracle() {
    for case in golden("safe_start")["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let root = Temp::new();
        if let Some(text) = case["marker"].as_str() {
            std::fs::write(root.0.join(safe_start::NAME), text).unwrap();
        }
        let alive = |pid: Option<&Value>| matches!(pid, Some(Value::Int(4242)));
        let previous = safe_start::begin_with(&root.0, "0.6.8", 777, 1.5, &alive);
        match (&previous, case["previous"].is_null()) {
            (None, true) => {}
            (Some(object), false) => assert_eq!(as_serde(object), case["previous"], "{name}"),
            _ => panic!("{name}: got {previous:?}, oracle {}", case["previous"]),
        }
        let raw = std::fs::read_to_string(root.0.join(safe_start::NAME)).unwrap();
        let written = Value::parse(&raw).unwrap();
        let keys: Vec<&str> = written.as_object().unwrap().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, strings(&case["written_keys"]).iter().map(String::as_str).collect::<Vec<_>>(), "{name}");
        let template = {
            let at = raw.find("\"started\": ").unwrap();
            format!("{}\"started\": <t>}}", raw[..at].replacen("777", "<pid>", 1))
        };
        assert_eq!(template, case["written_template"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn change_log_sections_match_the_oracle() {
    let case = golden("sections");
    for (name, text) in case["texts"].as_object().unwrap() {
        let got: Vec<[String; 3]> =
            whats_new::sections(text.as_str().unwrap()).into_iter().map(|e| [e.version, e.title, e.body]).collect();
        let expected: Vec<[String; 3]> = case["expected"][name]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| [e[0].as_str().unwrap().into(), e[1].as_str().unwrap().into(), e[2].as_str().unwrap().into()])
            .collect();
        assert_eq!(got, expected, "{name}");
    }
}

#[test]
fn notes_over_the_real_files_match_the_oracle() {
    let case = golden("notes_real");
    let root = notes_root();
    let read = |p: &str| text_of(&root.join(p));
    assert_eq!(summary(&whats_new::sections(&read("CHANGELOG.md"))), expected_summary(&case["real"]["changelog"]));
    assert_eq!(
        summary(&whats_new::sections(&read("assets/whats-new-en.md"))),
        expected_summary(&case["real"]["english"])
    );
    for entry in case["cases"].as_array().unwrap() {
        let previous = entry["previous"].as_str();
        let (current, language) = (entry["current"].as_str().unwrap(), lang(entry["lang"].as_str().unwrap()));
        let got = whats_new::notes(previous, current, Some(language), &root);
        let label = format!("notes({previous:?}, {current:?}, {language:?})");
        assert_eq!(summary(&got), expected_summary(&entry["entries"]), "{label}");
        let markdown = whats_new::markdown(&got);
        assert_eq!(sha(&markdown), entry["markdown_sha256"].as_str().unwrap(), "{label}: markdown");
        assert_eq!(markdown.chars().count(), entry["markdown_chars"].as_u64().unwrap() as usize);
    }
}

#[test]
fn notes_over_synthetic_files_match_the_oracle() {
    let case = golden("notes_synthetic");
    let mut count = 0;
    for (setup, files) in case["setups"].as_object().unwrap() {
        let root = Temp::new();
        if let Some(korean) = files["changelog"].as_str() {
            std::fs::create_dir_all(root.0.join("assets")).unwrap();
            std::fs::write(root.0.join("CHANGELOG.md"), korean.as_bytes()).unwrap();
            if let Some(english) = files["english"].as_str() {
                std::fs::write(root.0.join("assets").join("whats-new-en.md"), english.as_bytes()).unwrap();
            }
        }
        for entry in case["cases"].as_array().unwrap().iter().filter(|e| e["setup"] == *setup) {
            let got = whats_new::notes(
                entry["previous"].as_str(),
                entry["current"].as_str().unwrap(),
                Some(lang(entry["lang"].as_str().unwrap())),
                &root.0,
            );
            let expected: Vec<[String; 3]> = entry["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| [e[0].as_str().unwrap().into(), e[1].as_str().unwrap().into(), e[2].as_str().unwrap().into()])
                .collect();
            let got_list: Vec<[String; 3]> =
                got.iter().map(|e| [e.version.clone(), e.title.clone(), e.body.clone()]).collect();
            assert_eq!(got_list, expected, "{setup} {entry}");
            assert_eq!(whats_new::markdown(&got), entry["markdown"].as_str().unwrap());
            count += 1;
        }
    }
    println!("notes_synthetic: {count} cases equal");
}

fn prefs_with(root: &Path, seen: Option<&serde_json::Value>) -> Preferences {
    let mut prefs = Preferences::new(root.join("s.json"));
    if let Some(seen) = seen {
        prefs.save(whats_new::KEY, Value::from(seen)).unwrap();
    }
    prefs
}

#[test]
fn pending_and_remember_match_the_oracle() {
    let case = golden("pending");
    let root = notes_root();
    for entry in case["pending"].as_array().unwrap() {
        let temp = Temp::new();
        let seen = entry["seen_present"].as_bool().unwrap().then(|| &entry["seen"]);
        let prefs = prefs_with(&temp.0, seen);
        let got = whats_new::pending(
            &prefs,
            entry["first_run"].as_bool().unwrap(),
            grainy_platform::APP_VERSION,
            Some(lang(entry["lang"].as_str().unwrap())),
            &root,
        );
        assert_eq!(summary(&got), expected_summary(&entry["entries"]), "pending with seen {:?}", entry["seen"]);
    }
    for entry in case["remember"].as_array().unwrap() {
        let temp = Temp::new();
        let seen = entry["seen_present"].as_bool().unwrap().then(|| &entry["seen"]);
        let mut prefs = prefs_with(&temp.0, seen);
        whats_new::remember(&mut prefs, grainy_platform::APP_VERSION);
        assert_eq!(
            text_of(&temp.0.join("s.json")),
            entry["file"].as_str().unwrap(),
            "remember after {:?}",
            entry["seen"]
        );
        let value: serde_json::Value =
            serde_json::from_str(&prefs.values.get(whats_new::KEY).unwrap().dump(None, false)).unwrap();
        assert_eq!(value, entry["value"]);
    }
}

#[test]
fn shipped_feedback_and_thanks_match_the_oracle() {
    let case = golden("shipped");
    let mut count = 0;
    for entry in case["cases"].as_array().unwrap() {
        let label = format!("{} thanked {}", entry["label"], entry["thanked_text"]);
        let temp = Temp::new();
        let root = match entry["root_kind"].as_str().unwrap() {
            "real" => notes_root(),
            "synthetic" => {
                let log = golden("sections")["texts"]["log"].as_str().unwrap().to_string();
                std::fs::create_dir_all(temp.0.join("assets")).unwrap();
                std::fs::write(temp.0.join("CHANGELOG.md"), log).unwrap();
                if let Some(done) = entry["done_text"].as_str() {
                    std::fs::write(temp.0.join("assets").join("feedback-done.json"), done).unwrap();
                }
                temp.0.clone()
            }
            _ => temp.0.join("missing"),
        };
        let mut prefs = Preferences::new(temp.0.join("prefs").join("s.json"));
        prefs.save(feedback::SENT, Value::parse(entry["sent_text"].as_str().unwrap()).unwrap()).unwrap();
        if entry["thanked_present"].as_bool().unwrap() {
            prefs.save(whats_new::THANKED, Value::parse(entry["thanked_text"].as_str().unwrap()).unwrap()).unwrap();
        }
        for result in entry["results"].as_array().unwrap() {
            let got =
                whats_new::shipped(&prefs, result["previous"].as_str(), result["current"].as_str().unwrap(), &root);
            assert_eq!(
                got,
                strings(&result["shipped"]),
                "{label}: shipped({}, {})",
                result["previous"],
                result["current"]
            );
            count += 1;
        }
        for step in entry["thank_steps"].as_array().unwrap() {
            whats_new::thank(&mut prefs, &strings(&step["refs"]));
            assert_eq!(
                text_of(&temp.0.join("prefs").join("s.json")),
                step["file"].as_str().unwrap(),
                "{label}: file after thank({})",
                step["refs"]
            );
            assert_eq!(
                whats_new::shipped(&prefs, Some("1.2.8"), "1.2.10", &root),
                strings(&step["shipped_after"]),
                "{label}"
            );
        }
        assert_eq!(as_serde(&whats_new::done(&root)), entry["done"], "{label}: done()");
    }
    for language in ["ko", "en"] {
        for item in case["thanks"][language].as_array().unwrap() {
            if item["refs"].as_array().unwrap().is_empty() {
                continue; // thanks([]) is never shown; the oracle formats an empty list too, but the dialog skips it
            }
            assert_eq!(whats_new::thanks(&strings(&item["refs"]), lang(language)), item["text"].as_str().unwrap());
        }
    }
    println!("shipped: {count} range checks equal");
}

#[test]
fn feedback_messages_and_records_match_the_oracle() {
    let case = golden("feedback");
    assert_eq!(case["form_id"].as_str().unwrap(), feedback::FORM_ID);
    assert_eq!(case["limit"].as_u64().unwrap() as usize, feedback::LIMIT);
    assert_eq!(case["alphabet"].as_str().unwrap(), feedback::ALPHABET);
    assert_eq!(case["keep"].as_u64().unwrap() as usize, feedback::KEEP);
    assert_eq!(case["version"].as_str().unwrap(), grainy_platform::APP_VERSION);
    for (name, entry) in case["fields"].as_object().unwrap() {
        assert!(feedback::FIELDS.contains(&(name.as_str(), entry.as_str().unwrap())));
    }
    assert_eq!(feedback::tagged("sys", "K7Q2M"), case["tagged"].as_str().unwrap());
    for post in case["posts"].as_array().unwrap() {
        let seen = std::cell::RefCell::new(None::<(Request, u64)>);
        let result = feedback::send(
            post["message"].as_str().unwrap(),
            post["system"].as_str().unwrap(),
            grainy_platform::APP_VERSION,
            &|r: &Request, t: u64| {
                *seen.borrow_mut() = Some((r.clone(), t));
                Ok(200)
            },
        );
        let (request, timeout) = seen.into_inner().unwrap();
        assert_eq!(result, post["result"].as_str().unwrap());
        assert_eq!(request.url, post["url"].as_str().unwrap());
        assert_eq!(
            String::from_utf8(request.body).unwrap(),
            post["data"].as_str().unwrap(),
            "body of {}",
            post["message"]
        );
        assert_eq!(timeout, post["timeout"].as_u64().unwrap());
        let headers: std::collections::BTreeMap<String, String> =
            request.headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
        let expected: std::collections::BTreeMap<String, String> = post["headers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.as_str().unwrap().to_string()))
            .collect();
        assert_eq!(headers, expected);
    }
    for outcome in case["outcomes"].as_array().unwrap() {
        let name = outcome["name"].as_str().unwrap();
        let reply: Result<u16, SendFailure> = match name {
            "ok" => Ok(200),
            "status_204" => Ok(204),
            "status_500" => Ok(500),
            "http_404" => Err(SendFailure::Http(404)),
            "os_error" => Err(SendFailure::Other("network down".into())),
            "url_error" => Err(SendFailure::Other("no route".into())),
            "empty_os_error" => Err(SendFailure::Other(String::new())),
            other => panic!("unknown outcome {other}"),
        };
        assert_eq!(
            feedback::send("m", "s", "0.6.8", &|_: &Request, _: u64| reply.clone()),
            outcome["result"].as_str().unwrap(),
            "{name}"
        );
    }
    for info in case["system_info"].as_array().unwrap() {
        let mac = info["mac"].as_bool().unwrap();
        let adapter = info["adapter"].as_str().map(str::to_string);
        let facts = SystemFacts {
            system: if mac { System::MacOs } else { System::Windows },
            mac_version: info["mac_ver"].as_str().unwrap().into(),
            windows_release: info["release"].as_str().unwrap().into(),
            windows_version: info["version"].as_str().unwrap().into(),
            machine: info["machine"].as_str().unwrap().into(),
            gpu: adapter.map(|adapter| Gpu { adapter, enabled: info["enabled"].as_bool().unwrap() }),
        };
        assert_eq!(feedback::system_info(&facts), info["expected"].as_str().unwrap(), "{info}");
    }
    let dates = ["2026-11-01", "2026-11-02", "2026-11-03"];
    for record in case["records"].as_array().unwrap() {
        let temp = Temp::new();
        let mut prefs = Preferences::new(temp.0.join("s.json"));
        if record["initial_present"].as_bool().unwrap() {
            prefs.save(feedback::SENT, Value::parse(record["initial_text"].as_str().unwrap()).unwrap()).unwrap();
        }
        for (index, expected) in strings(&record["files"]).iter().enumerate() {
            feedback::record(&mut prefs, &format!("AB{index}CD"), "0.6.8", dates[index]).unwrap();
            assert_eq!(&text_of(&temp.0.join("s.json")), expected, "{} record {index}", record["name"]);
        }
    }
    let temp = Temp::new();
    let mut prefs = Preferences::new(temp.0.join("s.json"));
    for index in 0..55 {
        feedback::record(&mut prefs, &format!("M{index:04}")[..5], "0.6.8", "2026-11-01").unwrap();
    }
    assert_eq!(text_of(&temp.0.join("s.json")), case["many"].as_str().unwrap());
}
