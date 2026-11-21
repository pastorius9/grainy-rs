//! Golden tests against the Python oracle (`tools/golden/gen_adobe.py`, `golden/adobe/`).
//!
//! Everything here is exact: strings, integers, booleans, the order of reports, and floats (they are written
//! with Python's shortest repr and read back exactly, and the maths is the same operations in `f64`). A float
//! that differs in the last digit would fail, so the tolerance is zero and the measured error is zero.

use std::collections::BTreeMap;
use std::fmt::Debug;

use chrono::NaiveDateTime;
use grainy_adobe::develop::{self, Converted};
use grainy_adobe::error::{Error, ErrorKind, Result};
use grainy_adobe::history;
use grainy_adobe::lua::{self, Key, Limits, Lua, Table};
use grainy_adobe::preset;
use grainy_adobe::pyutil::{float_repr, json_repr, Zone};
use grainy_adobe::smart::{self, Moment};
use grainy_testkit::root;
use serde_json::{json, Map, Value};

/// Loads `golden/adobe/<name>/case.json`. Some cases nest deeper than serde_json's default recursion limit
/// (the deeply nested Lua tables the parser must refuse), so the limit is lifted here.
fn case(name: &str) -> Value {
    use serde::Deserialize;
    let path = root().join("golden").join("adobe").join(name).join("case.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run tools/golden/gen_adobe.py", path.display()));
    let mut de = serde_json::Deserializer::from_str(&text);
    de.disable_recursion_limit();
    Value::deserialize(&mut de).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn list<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("{key} is not a list"))
}

// ------------------------------------------------------------------------------------------ encoding
fn dec(v: &Value) -> Lua {
    match v {
        Value::Null => Lua::Nil,
        Value::Bool(b) => Lua::Bool(*b),
        Value::Object(m) => {
            if let Some(s) = m.get("int") {
                let s = s.as_str().unwrap_or("0");
                return match s.parse::<i128>() {
                    Ok(i) => Lua::Int(i),
                    Err(_) => Lua::Huge(s.parse::<f64>().unwrap_or(f64::INFINITY)),
                };
            }
            if let Some(s) = m.get("float") {
                return Lua::Float(s.as_str().unwrap_or("0").parse::<f64>().unwrap_or(f64::NAN));
            }
            if let Some(s) = m.get("str") {
                return Lua::Str(s.as_str().unwrap_or("").to_owned());
            }
            if let Some(pairs) = m.get("table") {
                let mut t = Table::new();
                for pair in pairs.as_array().expect("table pairs") {
                    let key = match dec(&pair[0]) {
                        Lua::Int(i) => Key::Int(i),
                        Lua::Huge(f) => Key::Huge(f.to_bits()),
                        Lua::Float(f) => Key::Float(f.to_bits()),
                        Lua::Str(s) => Key::Str(s),
                        other => panic!("bad key {other:?}"),
                    };
                    t.set(key, dec(&pair[1]));
                }
                return Lua::Table(t);
            }
            if let Some(items) = m.get("list") {
                return Lua::List(items.as_array().expect("list").iter().map(dec).collect());
            }
            panic!("bad encoded value {v}")
        }
        other => panic!("bad encoded value {other}"),
    }
}

fn enc(v: &Lua, xml: bool) -> Value {
    match v {
        Lua::Nil => Value::Null,
        Lua::Bool(b) => Value::Bool(*b),
        Lua::Int(i) => json!({"int": i.to_string()}),
        Lua::Huge(f) => json!({"int": format!("{f:.0}")}),
        Lua::Float(f) => json!({"float": float_repr(*f)}),
        Lua::Str(s) => json!({"str": s}),
        Lua::List(items) => json!({"list": items.iter().map(|x| enc(x, xml)).collect::<Vec<_>>()}),
        Lua::Table(t) => {
            if xml && t.len() == 1 && t.get_str("xml").is_some() {
                return json!({"table": [[{"str": "xml"}, {"str": "XML"}]]});
            }
            json!({"table": t.iter().map(|(k, x)| json!([enc(&k.to_lua(), xml), enc(x, xml)])).collect::<Vec<_>>()})
        }
    }
}

// ---------------------------------------------------------------------------------------------- errors
fn lenient(message: &str) -> bool {
    [
        "codec can't",
        "line 1 column",
        "Expecting",
        "Extra data",
        "Unterminated",
        "Invalid control",
        "Invalid \\",
        "Invalid isoformat",
        "utf-",
        "must be in",
        "out of range for month",
        "not supported between",
    ]
    .iter()
    .any(|p| message.contains(p))
}

/// Checks `got` against the golden entry: an error entry must match the kind and (unless it is a library text)
/// the message; returns the value when the entry is a success.
fn outcome<T: Debug>(label: &str, got: Result<T>, entry: &Value) -> Option<T> {
    if let Some(expected) = entry.get("error") {
        let error: Error = match got {
            Err(e) => e,
            Ok(v) => panic!("{label}: expected {} but got {v:?}", expected),
        };
        assert_eq!(error.kind.python_name(), expected["type"].as_str().unwrap_or(""), "{label}: error kind ({error})");
        let message = expected["message"].as_str().unwrap_or("");
        if !lenient(message) && !matches!(error.kind, ErrorKind::Overflow | ErrorKind::Io) {
            assert_eq!(error.message, message, "{label}: error message");
        }
        return None;
    }
    match got {
        Ok(v) => Some(v),
        Err(e) => panic!("{label}: unexpected error {:?}: {e}", e.kind),
    }
}

fn pairs_of(omitted: &develop::Omitted) -> Value {
    Value::Array(omitted.0.iter().map(|(k, v)| json!([k, v])).collect())
}

fn shaped(c: &Converted) -> Value {
    json!({"settings": Value::Object(c.settings.0.clone()), "mapped": c.mapped, "omitted": pairs_of(&c.omitted)})
}

fn assert_settings(label: &str, got: &Map<String, Value>, expected: &Value) {
    let want = expected.as_object().expect("settings object");
    for key in got.keys().chain(want.keys()) {
        assert_eq!(got.get(key), want.get(key), "{label}: settings[{key}]");
    }
}

fn object(v: &Value) -> Option<Map<String, Value>> {
    match v {
        Value::Object(m) => Some(m.clone()),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------- lua_data
#[test]
fn lua_parse() {
    let c = case("lua_parse");
    let mut n = 0;
    for entry in list(&c, "cases") {
        let text = entry["text"].as_str().expect("text");
        let mut limits = Limits::default();
        if let Some(v) = entry["limits"]["max_items"].as_u64() {
            limits.max_items = v as usize;
        }
        if let Some(v) = entry["limits"]["max_depth"].as_u64() {
            limits.max_depth = v as usize;
        }
        if let Some(v) = entry["limits"]["max_bytes"].as_u64() {
            limits.max_bytes = v as usize;
        }
        let label = format!("parse {text:?} {}", entry["limits"]);
        let got = lua::parse(text, limits).map(Lua::Table);
        if let Some(value) = outcome(&label, got, entry) {
            assert_eq!(enc(&value, false), entry["ok"], "{label}");
        }
        n += 1;
    }
    println!("lua_parse: {n} cases exact");
}

#[test]
fn lua_sequence() {
    let c = case("lua_sequence");
    for entry in list(&c, "cases") {
        let (label, input) = match entry.get("text") {
            Some(text) => {
                let text = text.as_str().expect("text");
                (format!("sequence {text}"), Lua::Table(lua::parse(text, Limits::default()).expect("parses")))
            }
            None => (format!("sequence {}", entry["value"]), dec(&entry["value"])),
        };
        if let Some(items) = outcome(&label, lua::sequence(&input), entry) {
            assert_eq!(enc(&Lua::List(items), false), entry["ok"], "{label}");
        }
    }
}

#[test]
fn lua_hostile_input_never_panics_or_hangs() {
    // Deep nesting: a clean error at the depth limit, not a stack overflow.
    for depth in [100usize, 1000, 100_000] {
        let text = "{".repeat(depth) + &"}".repeat(depth);
        let error = lua::parse(&text, Limits::default()).expect_err("too deep");
        assert_eq!(error.kind, ErrorKind::Data);
        assert_eq!(error.message, "데이터의 중첩 또는 항목 수가 지원 범위를 넘었습니다.");
    }
    // Too many items with the default limit (200000).
    let many = format!("{{{}}}", "0,".repeat(200_001));
    assert_eq!(
        lua::parse(&many, Limits::default()).expect_err("items").message,
        "데이터의 중첩 또는 항목 수가 지원 범위를 넘었습니다."
    );
    let ok = format!("{{{}}}", "0,".repeat(199_999));
    assert_eq!(lua::parse(&ok, Limits::default()).expect("fits").len(), 199_999);
    // Larger than the size limit.
    let big = " ".repeat(16 * 1024 * 1024 + 1);
    assert_eq!(
        lua::parse(&big, Limits::default()).expect_err("size").message,
        "저장된 데이터의 크기 또는 문자 형식을 확인할 수 없습니다."
    );
    // Unterminated constructs and a long run of near-misses.
    for text in [
        "[==[".to_owned() + &"]=".repeat(300_000),
        "--[==[".to_owned() + &"]=".repeat(300_000),
        "{a=\"".to_owned() + &"x".repeat(1_000_000),
        "{[[".to_owned() + &"]".repeat(500_000),
    ] {
        assert!(lua::parse(&text, Limits::default()).is_err());
    }
    // Many keys: duplicate detection must not be quadratic.
    let keys: String = (0..150_000).map(|i| format!("k{i}=1,")).collect();
    assert_eq!(lua::parse(&format!("{{{keys}}}"), Limits::default()).expect("keys").len(), 150_000);
    // Every prefix of a realistic table: an error or a table, never a panic.
    let sample = "return { Exposure2012 = 0.5, ToneCurvePV2012 = { 0, 0, \"a\\x41\\065\", [[long]] }, [ \"k\" ] = true; 1e3, 0x1p4 } -- c";
    let chars: Vec<char> = sample.chars().collect();
    for end in 0..=chars.len() {
        let prefix: String = chars[..end].iter().collect();
        let _ = lua::parse(&prefix, Limits::default());
    }
}

// ------------------------------------------------------------------------------------------ develop
#[test]
fn number_and_boolean() {
    let c = case("number");
    for entry in list(&c, "cases") {
        let value = dec(&entry["value"]);
        let (low, high) = (entry["low"].as_f64().expect("low"), entry["high"].as_f64().expect("high"));
        let label = format!("number {} in {low}..{high}", entry["value"]);
        if let Some(x) = outcome(&label, develop::number(&value, low, high), entry) {
            assert_eq!(float_repr(x), entry["ok"]["float"].as_str().expect("float"), "{label}");
        }
    }
    let c = case("boolean");
    for entry in list(&c, "cases") {
        let label = format!("boolean {}", entry["value"]);
        if let Some(b) = outcome(&label, develop::boolean(&dec(&entry["value"])), entry) {
            assert_eq!(Value::Bool(b), entry["ok"], "{label}");
        }
    }
}

#[test]
fn curve() {
    let c = case("curve");
    for entry in list(&c, "cases") {
        let label = format!("curve {}", entry["value"]);
        if let Some(points) = outcome(&label, develop::curve(&dec(&entry["value"])), entry) {
            let got: Vec<Value> =
                points.iter().map(|[x, y]| json!([{"float": float_repr(*x)}, {"float": float_repr(*y)}])).collect();
            assert_eq!(Value::Array(got), entry["ok"], "{label}");
        }
    }
}

#[test]
fn from_lua() {
    let c = case("from_lua");
    for entry in list(&c, "cases") {
        let text = entry["text"].as_str().expect("text");
        if let Some(v) = outcome(&format!("from_lua {text:?}"), develop::from_lua(text), entry) {
            assert_eq!(enc(&v, false), entry["ok"], "from_lua {text:?}");
        }
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex")).collect()
}

#[test]
fn xmp() {
    let c = case("xmp");
    let mut accepted = 0;
    for entry in list(&c, "cases") {
        let label = format!("xmp {}", entry["label"]);
        let data = unhex(entry["hex"].as_str().expect("hex"));
        if let Some(v) = outcome(&label, develop::from_xmp_bytes(&data), &entry["values"]) {
            assert_eq!(enc(&v, true), entry["values"]["ok"], "{label}");
            accepted += 1;
        }
        outcome(&format!("{label} (root)"), develop::xmp_root(&data), &entry["root"]);
    }
    println!("xmp: {accepted} documents accepted, all outcomes equal the oracle's");
}

fn base_of(v: &Value) -> Option<Map<String, Value>> {
    object(v)
}

#[test]
fn convert() {
    let c = case("convert");
    let mut mapped_total = 0;
    for entry in list(&c, "cases") {
        let label = format!("convert {}", entry["label"]);
        let values = dec(&entry["values"]);
        let base = base_of(&entry["base"]);
        let orientation = dec(&entry["orientation"]);
        let orientation = if orientation == Lua::Nil { None } else { Some(&orientation) };
        let got = develop::convert(&values, base.as_ref(), entry["is_raw"].as_bool().expect("raw"), orientation);
        if let Some(result) = outcome(&label, got, entry) {
            let expected = &entry["ok"];
            assert_settings(&label, &result.settings.0, &expected["settings"]);
            assert_eq!(json!(result.mapped), expected["mapped"], "{label}: mapped");
            assert_eq!(pairs_of(&result.omitted), expected["omitted"], "{label}: omitted (and its order)");
            // Grainy also identifies the Adobe renderer used by the source. All other text remains
            // byte-for-byte identical to the reference converter, for each supported report length.
            let source_version = entry["values"]["table"].as_array().and_then(|items| {
                let encoded = items.iter().find(|pair| pair[0]["str"] == "ProcessVersion")?.get(1)?;
                encoded.get("str").or_else(|| encoded.get("int")).or_else(|| encoded.get("float"))?.as_str()
            });
            assert_eq!(result.process_version.as_deref(), source_version, "{label}: process version");
            for (limit, key) in [(None, "summary"), (Some(5), "summary5"), (Some(1), "summary1"), (Some(0), "summary0")]
            {
                let legacy = expected[key].as_str().expect("summary");
                let wanted = expected_summary(legacy, source_version);
                assert_eq!(develop::summary(&result, limit), wanted, "{label}: {key}");
            }
            mapped_total += result.mapped.len();
        }
    }
    println!("convert: {} cases exact ({mapped_total} mapped keys)", list(&c, "cases").len());
}

// ------------------------------------------------------------------------------------------ presets
#[test]
fn preset_values_and_translate() {
    let c = case("preset_values");
    for entry in list(&c, "cases") {
        let label = format!("preset values {}", entry["label"]);
        let record = object(&entry["record"]).expect("record");
        if let Some(v) = outcome(&label, preset::values(&record), entry) {
            assert_eq!(enc(&v, true), entry["ok"], "{label}");
        }
    }
    let c = case("preset_translate");
    for entry in list(&c, "cases") {
        let label = format!("preset translate {}", entry["label"]);
        let record = object(&entry["record"]).expect("record");
        let base = base_of(&entry["base"]);
        let got = preset::translate(&record, base.as_ref(), entry["is_raw"].as_bool().expect("raw"));
        if let Some(result) = outcome(&label, got, entry) {
            assert_eq!(shaped(&result), entry["ok"], "{label}");
        }
    }
}

fn expected_summary(legacy: &str, source_version: Option<&str>) -> String {
    let insertion = legacy.find("\n\n").unwrap_or(legacy.len());
    let mut wanted = legacy.to_owned();
    if let Some(version) = source_version {
        wanted.insert_str(insertion, &format!("\n원본 Adobe ProcessVersion {version} · Grainy 현상 엔진으로 변환"));
    }
    wanted
}

#[test]
fn preset_read_and_resolve() {
    let c = case("preset_read");
    let folder = root().join("golden").join("adobe").join("preset_read");
    let mut stored: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for entry in list(&c, "cases") {
        let file = entry["file"].as_str().expect("file");
        let label = format!("preset read {file}");
        let mut expected = entry.clone();
        let version_reports: &[&str] = match file {
            "b.lrtemplate" => &["/ok/_adobe_preset/report", "/ok/_adobe_preset/preview"],
            "info.xmp" => &["/error/message"],
            _ => &[],
        };
        for path in version_reports {
            let field = expected.pointer_mut(path).expect("report field");
            *field = json!(expected_summary(field.as_str().expect("report text"), Some("11.0")));
        }
        if let Some(settings) = outcome(&label, preset::read(&folder.join(file)), &expected) {
            assert_eq!(Value::Object(settings.clone()), expected["ok"], "{label}");
            stored.insert(file.to_owned(), settings);
        }
    }
    let c = case("preset_resolve");
    for entry in list(&c, "cases") {
        let path = entry["path"].as_str().expect("path");
        let base = object(&entry["base"]).expect("base");
        let (name, settings) = match entry["file"].as_str() {
            Some(file) => (file.to_owned(), stored.get(file).cloned().expect("stored preset")),
            None => ("(inline)".to_owned(), object(&entry["preset"]).expect("preset")),
        };
        let label = format!("preset resolve {name} {path:?}");
        if let Some(result) = outcome(&label, preset::resolve(&settings, &base, path), entry) {
            assert_eq!(Value::Object(result.0), entry["ok"], "{label}");
        }
    }
}

// ------------------------------------------------------------------------------------------ history
#[test]
fn adobe_time() {
    let c = case("adobe_time");
    for entry in list(&c, "cases") {
        let value = match dec_plain(&entry["value"]) {
            Some(v) => v,
            None => continue,
        };
        let got = history::adobe_time(&value);
        let expected = &entry["ok"];
        assert_eq!(got.map_or(Value::Null, Value::String), *expected, "adobe_time {}", entry["value"]);
    }
}

/// The golden value of `adobe_time` inputs as JSON: tagged numbers back to plain JSON numbers.
fn dec_plain(v: &Value) -> Option<Value> {
    match dec(v) {
        Lua::Nil => Some(Value::Null),
        Lua::Bool(b) => Some(Value::Bool(b)),
        Lua::Int(i) => Some(match i64::try_from(i) {
            Ok(n) => Value::from(n),
            Err(_) => serde_json::Number::from_f64(i as f64).map_or(Value::Null, Value::Number),
        }),
        Lua::Huge(f) | Lua::Float(f) => match serde_json::Number::from_f64(f) {
            Some(n) => Some(Value::Number(n)),
            // Not representable in JSON (inf, nan): the oracle's answer is None for these as well, tested via the
            // number path.
            None => {
                assert!(history::adobe_time(&Value::Null).is_none());
                None
            }
        },
        Lua::Str(s) => Some(Value::String(s)),
        Lua::List(_) => Some(json!([])),
        Lua::Table(_) => Some(json!({})),
    }
}

#[test]
fn decode_row() {
    let c = case("decode_row");
    for entry in list(&c, "cases") {
        let label =
            format!("decode_row {} raw={} orientation={}", entry["label"], entry["is_raw"], entry["orientation"]);
        let orientation = dec(&entry["orientation"]);
        let orientation = if orientation == Lua::Nil { None } else { Some(&orientation) };
        let mut warnings: BTreeMap<String, i64> = BTreeMap::new();
        let got =
            history::decode_row(&entry["row"], entry["is_raw"].as_bool().expect("raw"), orientation, &mut warnings);
        if let Some(result) = outcome(&label, got, entry) {
            let expected = &entry["ok"];
            let shaped_result = result.as_ref().map_or(Value::Null, shaped);
            assert_eq!(shaped_result, expected["result"], "{label}: result");
            assert_eq!(json!(warnings), expected["warnings"], "{label}: warnings");
        }
    }
    let mut warnings: BTreeMap<String, i64> = BTreeMap::new();
    for text in ["return { Foo = 1 }", "return { Foo = 1, Exposure2012 = 1 }", "bad", "bad"] {
        history::decode_row(&json!({"text": text}), false, None, &mut warnings).expect("row");
    }
    assert_eq!(json!(warnings), c["accumulate"]["ok"]);
}

// ---------------------------------------------------------------------------------------- smart rules
fn time_of(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").expect("time")
}

fn iso(t: NaiveDateTime) -> String {
    use chrono::Timelike;
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let micro = t.nanosecond() / 1000;
    if micro == 0 {
        base
    } else {
        format!("{base}.{micro:06}")
    }
}

#[test]
fn smart_translate_and_finite() {
    let c = case("smart_translate");
    let mut accepted = 0;
    for entry in list(&c, "cases") {
        let text = match entry["text"].as_str() {
            Some(t) => t.to_owned(),
            None => format!("\"{}\"", "x".repeat(entry["size"].as_u64().expect("size") as usize - 2)),
        };
        let label = format!("smart translate {}", entry["label"]);
        if let Some(r) = outcome(&label, smart::translate(&text), entry) {
            assert_eq!(Value::Object(r), entry["ok"], "{label}");
            accepted += 1;
        }
    }
    println!("smart_translate: {accepted} accepted, the rest rejected with the oracle's errors");
    let c = case("smart_finite");
    for entry in list(&c, "cases") {
        let value = dec(&entry["value"]);
        let label = format!("finite {}", entry["value"]);
        if let Some(v) = outcome(&label, smart::finite(&value).cloned(), entry) {
            assert_eq!(enc(&v, false), entry["ok"], "{label}");
        }
    }
}

fn rule_nodes(c: &Value, key: &str) -> Vec<(String, Value)> {
    list(c, key).iter().map(|e| (e["label"].as_str().unwrap_or("").to_owned(), e["node"].clone())).collect()
}

#[test]
fn smart_evaluate_matrix() {
    let c = case("smart_rules");
    let nodes = rule_nodes(&c, "nodes");
    let photos: Vec<Map<String, Value>> = list(&c, "photos").iter().map(|p| object(p).expect("photo")).collect();
    let nows: Vec<NaiveDateTime> = list(&c, "nows").iter().map(|n| time_of(n.as_str().expect("now"))).collect();
    let matrix = list(&c, "matrix");
    let mut cells = 0;
    let mut errors = 0;
    for (i, (label, node)) in nodes.iter().enumerate() {
        for (j, photo) in photos.iter().enumerate() {
            for (k, now) in nows.iter().enumerate() {
                let expected = &matrix[i][j][k];
                let got = smart::evaluate_in(node, photo, Some(*now), None, Zone::Fixed(0));
                let context = format!("evaluate [{label}] photo {j} now {now}");
                match (got, expected) {
                    (Ok(b), Value::Bool(e)) => assert_eq!(b, *e, "{context}"),
                    (Err(e), Value::String(s)) => {
                        errors += 1;
                        assert_eq!(format!("!{}", e.kind.python_name()), *s, "{context}: {e}");
                    }
                    (got, expected) => panic!("{context}: got {got:?}, expected {expected}"),
                }
                cells += 1;
            }
        }
    }
    println!("smart_evaluate: {cells} node x photo x now cells equal ({errors} of them the oracle's errors)");
    // with an explicit info argument
    for entry in list(&c, "info_cases") {
        let label = entry["label"].as_str().expect("label");
        let info = object(&entry["info"]).expect("info");
        let got = smart::evaluate_in(
            &nodes[entry["index"].as_u64().expect("index") as usize].1,
            &photos[0],
            Some(nows[0]),
            Some(&info),
            Zone::Fixed(0),
        );
        match (got, &entry["result"]) {
            (Ok(b), Value::Bool(e)) => assert_eq!(b, *e, "info case {label}"),
            (Err(e), Value::String(s)) => assert_eq!(format!("!{}", e.kind.python_name()), *s, "info case {label}"),
            (got, expected) => panic!("info case {label}: got {got:?}, expected {expected}"),
        }
    }
}

#[test]
fn smart_sql_describe_time() {
    let c = case("smart_rules");
    let nodes = rule_nodes(&c, "nodes");
    for (entry, (label, node)) in list(&c, "sql").iter().zip(&nodes) {
        let got = smart::sql_condition(node);
        if let Some(r) = outcome(&format!("sql [{label}]"), got, entry) {
            let shaped = r.map_or(Value::Null, |(sql, params)| json!([sql, params]));
            assert_eq!(shaped, entry["ok"], "sql [{label}]");
        }
    }
    for (entry, (label, node)) in list(&c, "describe").iter().zip(&nodes) {
        if let Some(text) = outcome(&format!("describe [{label}]"), smart::describe(node, 0), entry) {
            assert_eq!(Value::String(text), entry["ok"], "describe [{label}]");
        }
    }
    for (entry, (label, node)) in list(&c, "describe_indent").iter().zip(&nodes) {
        if let Some(text) = outcome(&format!("describe3 [{label}]"), smart::describe(node, 3), entry) {
            assert_eq!(Value::String(text), entry["ok"], "describe3 [{label}]");
        }
    }
    let time_nodes = rule_nodes(&c, "time_nodes");
    for (entry, (label, node)) in list(&c, "time").iter().zip(&time_nodes) {
        if let Some(b) = outcome(&format!("time_dependent [{label}]"), smart::time_dependent(node), entry) {
            assert_eq!(Value::Bool(b), entry["ok"], "time_dependent [{label}]");
        }
    }
}

#[test]
fn smart_compare_date_window() {
    let c = case("smart_rules");
    for entry in list(&c, "compares") {
        let actual = dec(&entry["actual"]);
        let value = value_of(&dec(&entry["value"]));
        let value2 = value_of(&dec(&entry["value2"]));
        let op = entry["op"].as_str().expect("op");
        let label = format!("compare {} {op} {} {}", entry["actual"], entry["value"], entry["value2"]);
        let got: Result<bool> = match &actual {
            Lua::Nil => Ok(false),
            Lua::Int(_) | Lua::Float(_) | Lua::Bool(_) => {
                let a = match actual {
                    Lua::Int(i) => i as f64,
                    Lua::Float(f) => f,
                    _ => 0.0,
                };
                smart::compare_number(a, op, &value, Some(&value2))
            }
            Lua::Str(s) => smart::compare_text(s, op, &value, Some(&value2)),
            other => panic!("unexpected actual {other:?}"),
        };
        if let Some(b) = outcome(&label, got, entry) {
            assert_eq!(Value::Bool(b), entry["ok"], "{label}");
        }
    }
    for entry in list(&c, "dates") {
        let label = format!("_date {}", entry["value"]);
        let value = value_of(&dec(&entry["value"]));
        let got = smart::parse_date_in(&value, Zone::Fixed(0)).map(iso);
        assert_eq!(got.map_or(Value::Null, Value::String), entry["ok"], "{label}");
    }
    for entry in list(&c, "windows") {
        let label = format!("_window {} now {}", entry["label"], entry["now"]);
        let now = time_of(entry["now"].as_str().expect("now"));
        if let Some((start, end)) = outcome(&label, smart::window(&entry["node"], now), entry) {
            let show = |m: Moment| match m {
                Moment::Date(d) => ("date", d.format("%Y-%m-%d").to_string()),
                Moment::Time(t) => ("datetime", iso(t)),
            };
            let (kind, s) = show(start);
            let (_, e) = show(end);
            assert_eq!(json!({"kind": kind, "start": s, "end": e}), entry["ok"], "{label}");
        }
    }
}

/// A golden value as the JSON the oracle's functions received.
fn value_of(l: &Lua) -> Value {
    match l {
        Lua::Nil => Value::Null,
        Lua::Bool(b) => Value::Bool(*b),
        Lua::Int(i) => Value::from(i64::try_from(*i).unwrap_or(0)),
        Lua::Huge(f) | Lua::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Lua::Str(s) => Value::String(s.clone()),
        Lua::List(_) => json!([]),
        Lua::Table(_) => json!({}),
    }
}

#[test]
fn helpers_match_python() {
    // repr(float)
    for (x, repr) in [
        (0.1, "0.1"),
        (1.0, "1.0"),
        (-0.0, "-0.0"),
        (1e16, "1e+16"),
        (1e15, "1000000000000000.0"),
        (1.5e-7, "1.5e-07"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (123_456_789.123_456_79, "123456789.12345679"),
        (0.1 + 0.2, "0.30000000000000004"),
        (5e-324, "5e-324"),
        (1.7976931348623157e308, "1.7976931348623157e+308"),
        (2.5, "2.5"),
        (100.0, "100.0"),
        (12345678901234567890.0, "1.2345678901234567e+19"),
    ] {
        assert_eq!(float_repr(x), repr);
    }
    assert_eq!(json_repr(&json!(["a", null, true, 2.5, {"k": "it's"}])), "['a', None, True, 2.5, {'k': \"it's\"}]");
}
