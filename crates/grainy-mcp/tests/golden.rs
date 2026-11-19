//! Goldens from the Python oracle (tools/golden/gen_mcp.py). Everything the oracle produces as text is compared
//! as bytes (`dumps`); the rest as parsed values. There is no numeric tolerance: every comparison is exact.

use std::time::{Duration, Instant};

use grainy_mcp::protocol::{self, Flavour, Protocol};
use grainy_mcp::stub::StubTools;
use grainy_mcp::tools::{self, call_checked, english_error, ErrorKind, ToolError};
use grainy_mcp::{b64, check, dumps, pyjson, ServiceError};
use grainy_testkit::Case;
use serde_json::{json, Value};

fn load(name: &str) -> Value {
    Case::open("mcp", name).json().clone()
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("bad JSON text {text:?}: {e}"))
}

fn text(value: &Value) -> &str {
    value.as_str().expect("a string")
}

fn dumped(value: &Value) -> String {
    String::from_utf8(dumps(value)).expect("ASCII")
}

fn strings(value: &Value) -> Vec<&str> {
    value.as_array().expect("an array").iter().map(text).collect()
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(45)
}

#[test]
fn constants_match_the_oracle() {
    let c = load("constants");
    assert_eq!(strings(&c["modern"]), protocol::MODERN);
    assert_eq!(strings(&c["legacy"]), protocol::LEGACY);
    assert_eq!(strings(&c["supported"]), protocol::SUPPORTED);
    assert_eq!(text(&c["offline_version"]), protocol::OFFLINE_VERSION);
    assert_eq!(c["cache"], json!({"ttlMs": protocol::CACHE_TTL_MS, "cacheScope": "private"}));
    assert_eq!(text(&c["meta_version"]), protocol::META_VERSION);
    assert_eq!(text(&c["meta_capabilities"]), protocol::META_CAPABILITIES);
    assert_eq!(text(&c["meta_server"]), protocol::META_SERVER);
    assert_eq!(text(&c["instructions"]), protocol::INSTRUCTIONS);
    assert_eq!(text(&c["offline_text"]), protocol::OFFLINE_TEXT);
    assert_eq!(text(&c["offline_note"]), protocol::OFFLINE_NOTE);
    assert_eq!(text(&c["offline_error"]), protocol::OFFLINE_ERROR);
    assert_eq!(c["modern_enabled"], json!(protocol::MODERN_ENABLED));
    assert_eq!(c["deadline_seconds"], json!(protocol::DEADLINE_SECONDS));
    assert_eq!(text(&c["unfinished"]), tools::UNFINISHED);
    assert_eq!(text(&c["target_text"]), tools::TARGET_TEXT);
    assert_eq!(c["max_id"], json!(tools::MAX_ID));
    assert_eq!(c["max_targets"], json!(tools::MAX_TARGETS));
    assert_eq!(c["curve_points"], json!(tools::CURVE_POINTS));
    assert_eq!(c["sheet_cells"], json!(tools::SHEET_CELLS));
    assert_eq!(strings(&c["export_formats"]), tools::EXPORT_FORMATS);
    assert_eq!(strings(&c["color_spaces"]), tools::COLOR_SPACES);
    assert_eq!(strings(&c["film_types"]), tools::FILM_TYPES);
    assert_eq!(strings(&c["sorts"]), tools::SORTS);
    assert_eq!(strings(&c["copy_groups"]), tools::COPY_GROUPS);
    assert_eq!(strings(&c["flags"]), tools::FLAGS);
    assert_eq!(strings(&c["labels"]), tools::LABELS);
    let fields: Vec<Value> = tools::FIELDS.iter().map(|&(k, lo, hi)| json!([k, lo, hi])).collect();
    assert_eq!(parse(text(&c["fields_json"])), Value::Array(fields));
    // The server info carries the version the app passes in.
    let info = Protocol::new(text(&c["version"])).server_info();
    assert_eq!(info, parse(text(&c["server_info_json"])));
    assert_eq!(dumped(&info), text(&c["server_info_json"]).replace(", ", ",").replace(": ", ":"));
}

#[test]
fn tools_list_is_byte_identical() {
    let c = load("tools_list");
    let defs = tools::definitions();
    assert_eq!(defs.len(), 17);
    assert_eq!(dumped(&Value::Array(defs.to_vec())), text(&c["definitions_bytes"]));
    let per_tool = strings(&c["per_tool_bytes"]);
    for (definition, expected) in defs.iter().zip(per_tool) {
        assert_eq!(dumped(definition), expected, "tool {}", definition["name"]);
    }
    let names: Vec<&str> = defs.iter().map(|d| text(&d["name"])).collect();
    assert_eq!(names, strings(&c["names"]));
    assert_eq!(tools::edits_description(), text(&c["edits_description"]));
    assert_eq!(dumped(&Value::Object(tools::target(7))), text(&c["target_bytes"]));
    let schemas: serde_json::Map<String, Value> = tools::schemas().iter().cloned().collect();
    assert_eq!(dumped(&Value::Object(schemas.clone())), text(&c["schemas_bytes"]));
    assert_eq!(Value::Object(schemas), parse(text(&c["schemas_json"])));
    for (key, expected) in c["range_samples"].as_object().unwrap() {
        let (low, high) = key.split_once(',').unwrap();
        assert_eq!(tools::range_text(low.parse().unwrap(), high.parse().unwrap()), text(expected));
    }
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    value.as_array().unwrap().iter().map(|p| (text(&p[0]).to_string(), text(&p[1]).to_string())).collect()
}

#[test]
fn handle_matches_every_oracle_case() {
    let c = load("handle");
    let protocol_for = |modern: bool| Protocol::new(text(&c["version"])).with_modern(modern);
    let cases = c["cases"].as_array().unwrap();
    assert!(cases.len() > 300);
    for case in cases {
        let name = text(&case["name"]);
        let result = parse(text(&case["tool_result_json"]));
        let tools =
            if case["tool_fail"] == json!(true) { StubTools::failing() } else { StubTools::with_result(result) };
        let flavour = if text(&case["flavour"]) == "stdio" { Flavour::Stdio } else { Flavour::Http };
        let message = parse(text(&case["message_json"]));
        let modern = case["modern"] == json!(true);
        let reply = protocol_for(modern).handle(&message, &pairs(&case["headers"]), flavour, &tools, deadline());
        assert_eq!(u64::from(reply.status), case["status"].as_u64().unwrap(), "{name}: status");
        let expected = match &case["body"] {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            blob => Some(text(&c["blobs"][text(&blob["blob"])]).to_string()),
        };
        let actual = reply.body_bytes().map(|b| String::from_utf8(b).unwrap());
        assert_eq!(actual, expected, "{name}: body bytes");
        assert!(reply.headers.is_empty(), "{name}");
        let calls: Vec<Value> = tools.calls.borrow().iter().map(|(n, a, m)| json!([n, a, m])).collect::<Vec<_>>();
        assert_eq!(Value::Array(calls), parse(text(&case["calls_json"])), "{name}: calls");
    }
}

#[test]
fn the_shipped_switch_keeps_the_modern_era_off() {
    const { assert!(!protocol::MODERN_ENABLED) };
    let shipped = Protocol::new("9.9.9");
    let meta = json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": {"_meta": {protocol::META_VERSION: "2026-07-28", protocol::META_CAPABILITIES: {}}}});
    let reply = shipped.handle(&meta, &[], Flavour::Http, &StubTools::new(), deadline());
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.unwrap()["error"]["code"], json!(-32601));
}

#[test]
fn offline_templates_and_builders() {
    let c = load("offline");
    let templates = Protocol::new(text(&c["version"])).offline_templates();
    let order = strings(&c["order"]);
    assert_eq!(templates.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), order);
    for (name, line) in &templates {
        assert_eq!(line, text(&c["templates"][name]), "{name}");
        assert!(line.is_ascii() && !line.contains('\n') && line.matches("__ID__").count() == 1);
    }
    let p = Protocol::new(text(&c["version"]));
    let offline = protocol::result(json!("__ID__"), p.initialize_result(Some("2025-03-26"), true));
    assert_eq!(dumped(&offline), text(&c["initialize_offline_bytes"]));
    let online = protocol::result(json!(1), p.initialize_result(Some("2025-03-26"), false));
    assert_eq!(dumped(&online), text(&c["initialize_online_bytes"]));
    for sample in c["error_samples"].as_array().unwrap() {
        let ident = parse(text(&sample[0]));
        let data = parse(text(&sample[3]));
        let data = if data.is_null() { None } else { Some(data) };
        let body = protocol::error(ident, sample[1].as_i64().unwrap(), text(&sample[2]), data);
        assert_eq!(dumped(&body), text(&sample[4]));
    }
    for sample in c["result_samples"].as_array().unwrap() {
        let body = protocol::result(parse(text(&sample[0])), parse(text(&sample[1])));
        assert_eq!(dumped(&body), text(&sample[2]));
    }
}

#[test]
fn header_values_decode_like_python() {
    for row in load("header_value")["rows"].as_array().unwrap() {
        let input = row[0].as_str();
        let expected = row[1].as_str().map(str::to_string);
        assert_eq!(protocol::header_value(input), expected, "header {input:?}");
    }
}

#[test]
fn argument_checker_texts_match() {
    let c = load("check");
    let cases = c["cases"].as_array().unwrap();
    assert!(cases.len() > 1000);
    let mut errors = 0;
    for case in cases {
        let schema = match &case["tool"] {
            Value::String(tool) => tools::schema(tool).unwrap_or_else(|| panic!("no schema for {tool}")).clone(),
            _ => parse(text(&case["schema_json"])),
        };
        let value = parse(text(&case["value_json"]));
        let path = text(&case["path"]);
        let actual = check(&schema, &value, path).err().map(|ServiceError(t)| t);
        let expected = case["error"].as_str().map(str::to_string);
        assert_eq!(actual, expected, "schema {} value {} path {path}", case["tool"], text(&case["value_json"]));
        errors += usize::from(expected.is_some());
    }
    assert!(errors > 500, "the goldens should cover many error texts, got {errors}");
}

#[test]
fn content_helpers_match() {
    let c = load("helpers");
    for row in c["shown"].as_array().unwrap() {
        let value = parse(text(&row[0]));
        assert_eq!(grainy_mcp::check::show(&value), text(&row[1]), "show {}", text(&row[0]));
        if let Some(f) = value.as_f64().filter(|_| !(value.is_i64() || value.is_u64())) {
            assert_eq!(pyjson::format_g(f), text(&row[2]), "format_g {f}");
        }
    }
    for (row, raw) in c["text_blocks"].as_array().unwrap().iter().zip(c["text_blocks_raw"].as_array().unwrap()) {
        let value = parse(text(&row[0]));
        let block = tools::text_block(&value);
        assert_eq!(dumped(&block), text(&row[1]));
        assert_eq!(text(&block["text"]), text(&raw[1]));
    }
    for row in c["image_blocks"].as_array().unwrap() {
        let data: Vec<u8> = row[0].as_array().unwrap().iter().map(|b| b.as_u64().unwrap() as u8).collect();
        assert_eq!(dumped(&tools::image_block(&data)), text(&row[1]));
        assert_eq!(b64::decode(&b64::encode(&data)), Some(data));
    }
    for row in c["failures"].as_array().unwrap() {
        assert_eq!(dumped(&tools::failure(text(&row[0]))), text(&row[1]));
    }
}

#[test]
fn python_text_forms_match() {
    for row in load("pyjson")["rows"].as_array().unwrap() {
        let value = parse(text(&row[0]));
        assert_eq!(pyjson::py_repr(&value), text(&row[1]), "repr of {}", text(&row[0]));
        assert_eq!(pyjson::py_str(&value), text(&row[2]), "str of {}", text(&row[0]));
        assert_eq!(dumped(&value), text(&row[3]), "dumps of {}", text(&row[0]));
        assert_eq!(pyjson::type_name(&value), text(&row[4]));
    }
}

#[test]
fn tool_call_wrapper_maps_outcomes_like_the_oracle() {
    let c = load("call");
    for case in c["cases"].as_array().unwrap() {
        let tool = text(&case["tool"]);
        let arguments = parse(text(&case["arguments_json"]));
        let extra = parse(text(&case["extra_json"]));
        let mut ran = false;
        let result = call_checked(tool, &arguments, || {
            ran = true;
            match text(&case["outcome"]) {
                "ok" => Ok(extra["content"].as_array().unwrap().clone()),
                "service" => Err(ToolError::Service(text(&extra["text"]).into())),
                "busy" => Err(ToolError::Busy(text(&extra["text"]).into())),
                "unfinished" => Err(ToolError::Unfinished),
                "job" => Err(ToolError::Job { message: text(&extra["message"]).into(), view: extra["view"].clone() }),
                "other" => Err(ToolError::Other { last: text(&extra["last"]).into() }),
                other => panic!("outcome {other}"),
            }
        })
        .unwrap();
        assert_eq!(ran, case["ran"] == json!(true), "{tool} {}: whether the tool ran", text(&case["outcome"]));
        assert_eq!(dumped(&result), text(&case["result_bytes"]), "{tool} {}", text(&case["outcome"]));
    }
    for row in c["english"].as_array().unwrap() {
        let kind = text(&row[0]);
        let mapped = match kind {
            "PermissionDenied" => ErrorKind::PermissionDenied,
            "NotFound" => ErrorKind::NotFound,
            "AlreadyExists" => ErrorKind::AlreadyExists,
            "Io" => ErrorKind::Io,
            "Internal" => ErrorKind::Internal,
            named => ErrorKind::Named(named.strip_prefix("Named:").unwrap()),
        };
        assert_eq!(english_error(mapped), text(&row[1]), "{kind}");
    }
}

#[test]
fn unknown_tool_in_the_wrapper_is_a_fault() {
    assert!(call_checked("nope", &json!({}), || Ok(vec![])).is_err());
}
