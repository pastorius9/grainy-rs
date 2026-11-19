//! The automatic buttons of the Basic panel, as background jobs: 자동 톤 (`auto_tone`), 자동 색 (`auto_color`),
//! 네거티브 반전 (`toggle_negative` with `measure_negative`) and 다시 분석 (`measure_negative_again`) of
//! luma/app.py, measuring through `grainy_engine::auto`. The photo is loaded on the job (`load_image(path, 1800)`,
//! the oracle's source); the result is applied only if the same photo is still open with the same settings.

use grainy_core::Settings;
use grainy_engine::auto;
use grainy_engine::files::load_photo;
use grainy_engine::Externals;
use serde_json::Value;

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::panels;
use crate::state::changed_keys;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "develop.auto_tone", label: "자동 톤", enabled: idle_photo, run: auto_tone },
    Command { id: "develop.auto_color", label: "자동 색", enabled: idle_photo, run: auto_color },
    Command { id: "develop.negative", label: "네거티브 반전", enabled: idle_photo, run: negative },
    Command { id: "develop.negative_again", label: "다시 분석", enabled: negative_on, run: negative_again },
];

/// The source size of the oracle's develop view (`load_image(path, 1800, ...)`).
const SOURCE_SIZE: usize = 1800;

fn idle_photo(cx: &AppContext) -> bool {
    photo_open(cx) && !cx.jobs.running("auto")
}

fn negative_on(cx: &AppContext) -> bool {
    idle_photo(cx) && cx.settings().bool("negative")
}

/// The open photo: id, path, its committed settings and the edit version they belong to.
fn target(cx: &mut AppContext) -> Option<(u64, std::path::PathBuf, Settings, u64, u64)> {
    let photo = cx.doc.current()?;
    let (id, path) = (photo.id, photo.path.clone());
    let edit = cx.doc.edit_mut();
    edit.commit("보정 변경");
    Some((id, path, edit.settings().clone(), edit.version(), cx.document_generation))
}

/// Whether the job's photo is still open, unchanged.
fn still(cx: &AppContext, id: u64, version: u64, generation: u64) -> bool {
    cx.document_generation == generation && cx.doc.current_id() == id && cx.doc.edit().version() == version
}

#[cfg(test)]
#[test]
fn an_auto_result_belongs_to_one_document_even_if_ids_and_versions_match() {
    let mut cx = AppContext::new(
        &eframe::egui::Context::default(),
        Box::<crate::render::TestCard>::default(),
        crate::context::memory_document(),
    );
    let (id, version, generation) = (cx.doc.current_id(), cx.doc.edit().version(), cx.document_generation);
    assert!(still(&cx, id, version, generation));
    cx.set_document(crate::context::memory_document());
    assert_eq!((cx.doc.current_id(), cx.doc.edit().version()), (id, version));
    assert!(!still(&cx, id, version, generation));
}

fn source(path: &std::path::Path, settings: &Settings) -> Result<grainy_engine::files::Loaded, String> {
    load_photo(path, Some(SOURCE_SIZE), settings).map_err(|e| e.to_string())
}

/// Port of `auto_tone`.
fn auto_tone(cx: &mut AppContext, _: &Invocation) {
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    let input = settings.clone();
    cx.jobs.spawn(
        "auto",
        tr("자동 톤"),
        move |_| {
            let loaded = source(&path, &input)?;
            auto::auto_tone_settings(loaded.photo.source(), &input, &Externals::default()).map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, version, generation) {
                return;
            }
            match result {
                Ok((_, report)) if report["status"] == "flat" => {
                    cx.auto_note = report["message"].as_str().unwrap_or("").to_owned();
                    cx.status = tr(&cx.auto_note.clone());
                }
                Ok((mut adjusted, report)) => {
                    // The crop stays as it is (a draft crop is applied by the crop tool).
                    adjusted.set("crop", settings.value("crop").clone());
                    let keys = changed_keys(&settings, &adjusted);
                    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                    panels::switch_on(&mut adjusted, &refs);
                    let edit = cx.doc.edit_mut();
                    edit.set_all(adjusted);
                    edit.commit("자동톤");
                    cx.before = false;
                    let number = |k: &str| report[k].as_f64().unwrap_or(0.0);
                    cx.auto_note = tr_with(
                        "자동톤 적용 · 밝기 {0}–{1}\n→ 약 {2}–{3} / 255 (양끝 {4}% 여유)",
                        &[
                            &python_g(number("input_black")),
                            &python_g(number("input_white")),
                            &format!("{:.0}", number("output_black")),
                            &format!("{:.0}", number("output_white")),
                            &python_g(number("margin_percent")),
                        ],
                    );
                }
                Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
            }
        },
    );
}

/// Python's `f"{v:g}"` for the report's numbers.
fn python_g(v: f64) -> String {
    let text = format!("{v}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

/// Port of `auto_color` at full strength (the strength slider is the Basic panel's later work).
fn auto_color(cx: &mut AppContext, _: &Invocation) {
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    let amount = 100.0;
    cx.jobs.spawn(
        "auto",
        tr("자동 색"),
        move |_| {
            let loaded = source(&path, &settings)?;
            auto::auto_color_settings(loaded.photo.source(), &settings, amount / 100.0, &Externals::default())
                .map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, version, generation) {
                return;
            }
            match result {
                Ok((adjusted, report)) if report["status"] == "applied" => {
                    let edit = cx.doc.edit_mut();
                    edit.set("rgb_curves", adjusted.value("rgb_curves").clone());
                    edit.commit("자동 색");
                    cx.before = false;
                    cx.status = tr_with(
                        "자동 색 적용 · 세기 {0}% · 채널 커브에서 다듬을 수 있습니다.",
                        &[&format!("{amount:.0}")],
                    );
                }
                Ok((_, report)) => {
                    cx.status = tr(match report["status"].as_str() {
                        Some("monochrome") => "흑백 사진에는 자동 색을 쓰지 않습니다.",
                        Some("neutral") => "색 틀어짐을 찾지 못했습니다. 바꾼 것이 없습니다.",
                        _ => "색을 판단할 영역이 부족해 자동 색을 적용하지 않았습니다.",
                    });
                }
                Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
            }
        },
    );
}

/// The keys `measure_negative` copies from the measurement.
const MEASURED: [&str; 5] = ["negative_base", "negative_peak", "negative_density", "exposure", "blacks"];

/// Measures the scan on a job, then applies the measurement with `label` (and the conversion switched on).
fn measure(cx: &mut AppContext, label: &'static str) {
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    let mut input = settings.clone();
    // A RAW scan is measured in the camera's own colours, as the oracle loads it once the conversion is on.
    input.set("negative", true);
    cx.jobs.spawn(
        "auto",
        tr("네거티브 반전"),
        move |_| {
            let loaded = source(&path, &input)?;
            auto::measure_negative(loaded.photo.source(), &settings, &Externals::default()).map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, version, generation) {
                return;
            }
            match result {
                Ok(measured) => {
                    let edit = cx.doc.edit_mut();
                    let mut next = edit.settings().clone();
                    for key in MEASURED {
                        next.set(key, measured.value(key).clone());
                    }
                    panels::switch_on(&mut next, &["exposure", "blacks"]);
                    next.set("negative", true);
                    edit.set_all(next);
                    edit.commit(label);
                    cx.before = false;
                    if label == "네거티브 반전" {
                        cx.status = tr("네거티브 반전 적용 · 필름 농도와 필름 대비로 다듬고, 자동 톤·자동 색을 이어서 쓸 수 있습니다.");
                    }
                }
                Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
            }
        },
    );
}

/// Port of `toggle_negative`: the scan is measured the first time only.
fn negative(cx: &mut AppContext, _: &Invocation) {
    let on = !cx.settings().bool("negative");
    if on {
        let s = cx.settings();
        let measured =
            !matches!(s.value("negative_base"), Value::Null) && !matches!(s.value("negative_peak"), Value::Null);
        if !measured {
            measure(cx, "네거티브 반전");
            return;
        }
    }
    let edit = cx.doc.edit_mut();
    edit.commit("보정 변경");
    // The oracle writes the value directly (no panel is switched on).
    let mut next = edit.settings().clone();
    next.set("negative", on);
    edit.set_all(next);
    edit.commit(if on { "네거티브 반전" } else { "네거티브 반전 해제" });
    cx.before = false;
    if on {
        cx.status = tr("네거티브 반전 적용 · 필름 농도와 필름 대비로 다듬고, 자동 톤·자동 색을 이어서 쓸 수 있습니다.");
    }
}

/// Port of `measure_negative_again`.
fn negative_again(cx: &mut AppContext, _: &Invocation) {
    measure(cx, "네거티브 다시 분석");
}
