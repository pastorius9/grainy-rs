//! The automatic buttons of the Basic panel, as background jobs: 자동 톤 (`auto_tone`), 자동 색 (`auto_color`),
//! 네거티브 반전 (`toggle_negative` with `measure_negative`) and 다시 분석 (`measure_negative_again`) of
//! luma/app.py, measuring through `grainy_engine::auto` on the shared 1800-pixel source.
//! The result is applied only if the same photo is still open with the same settings.

use grainy_core::Settings;
use grainy_engine::auto;
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
fn scale_curves_and_infer_amount_round_trip() {
    let full = [
        vec![[0.0, 0.0], [0.22, 0.18], [0.54, 0.48], [0.84, 0.80], [1.0, 1.0]],
        vec![[0.0, 0.0], [0.16, 0.18], [0.46, 0.48], [0.78, 0.80], [1.0, 1.0]],
        vec![[0.0, 0.0], [0.18, 0.18], [0.48, 0.48], [0.80, 0.80], [1.0, 1.0]],
    ];
    // 100% reproduces full curves verbatim.
    let at_100 = scale_curves(&full, 100.0);
    assert_eq!(at_100, serde_json::json!(full));
    assert_eq!(infer_amount(&at_100, &full), Some(100.0));

    // 50% halves every correction shift and recovers cleanly for Undo/Redo.
    let at_50 = scale_curves(&full, 50.0);
    assert_eq!(at_50[0][2], serde_json::json!([0.54, 0.51]));
    assert_eq!(at_50[1][2], serde_json::json!([0.46, 0.47]));
    assert_eq!(infer_amount(&at_50, &full), Some(50.0));

    // 0% produces exact identity along every anchor.
    let at_0 = scale_curves(&full, 0.0);
    assert_eq!(at_0[0][2], serde_json::json!([0.54, 0.54]));
    assert_eq!(infer_amount(&at_0, &full), Some(0.0));

    // Manual curve edits are rejected so the Auto Color strength slider hides cleanly.
    let manual =
        serde_json::json!([[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0]],]);
    assert_eq!(infer_amount(&manual, &full), None);
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

/// Port of `auto_tone`.
fn auto_tone(cx: &mut AppContext, _: &Invocation) {
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    let input = settings.clone();
    let source = super::auto_source::for_click(cx, &path, &input);
    cx.jobs.spawn(
        "auto",
        tr("자동 톤"),
        move |_| {
            let loaded = source.load(&input)?;
            auto::auto_tone_settings(loaded.source(), &input, &Externals::default()).map_err(|e| e.to_string())
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

/// Live state for the "자동 색 세기" (Auto color strength) slider shown after Auto Color is applied.
pub(crate) struct AutoColorState {
    amount: f64,
    loaded_generation: Option<u64>,
    active: Option<ActiveAutoColor>,
}

impl Default for AutoColorState {
    fn default() -> Self {
        Self { amount: 100.0, loaded_generation: None, active: None }
    }
}

struct ActiveAutoColor {
    generation: u64,
    photo_id: u64,
    full_curves: [Vec<[f64; 2]>; 3],
    last_applied: Value,
}

fn round4(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.4}").parse().unwrap_or(x)
}

fn parse_curves(value: &Value) -> Option<[Vec<[f64; 2]>; 3]> {
    let channels = value.as_array()?;
    if channels.len() != 3 {
        return None;
    }
    let mut out = [Vec::new(), Vec::new(), Vec::new()];
    for (c, ch) in channels.iter().enumerate() {
        let pts = ch.as_array()?;
        let mut parsed = Vec::with_capacity(pts.len());
        for pt in pts {
            let pair = pt.as_array()?;
            if pair.len() != 2 {
                return None;
            }
            parsed.push([pair[0].as_f64()?, pair[1].as_f64()?]);
        }
        out[c] = parsed;
    }
    Some(out)
}

/// Scales the 100%-strength auto-color channel curves linearly to `amount_pct` (0..=100%),
/// preserving exact monotonicity and endpoint identity (`[0, 0]` and `[1, 1]`).
pub(crate) fn scale_curves(full: &[Vec<[f64; 2]>; 3], amount_pct: f64) -> Value {
    let scale = (amount_pct / 100.0).clamp(0.0, 1.0);
    let scaled: [Vec<[f64; 2]>; 3] = std::array::from_fn(|c| {
        let mut out: Vec<[f64; 2]> = Vec::with_capacity(full[c].len());
        let mut prev_y = 0.0f64;
        for &[x, y_full] in &full[c] {
            let y = if x <= 0.0 {
                0.0
            } else if x >= 1.0 {
                1.0
            } else {
                let raw = x + (y_full - x) * scale;
                round4(raw.max(prev_y).clamp(0.0, 1.0))
            };
            prev_y = y;
            out.push([x, y]);
        }
        out
    });
    serde_json::json!(scaled)
}

/// If `current` still matches a scaled version of `full` (e.g. after Undo/Redo between strengths),
/// recovers the percentage (`0..=100`). Returns `None` if the user manually edited or cleared the curves.
fn infer_amount(current: &Value, full: &[Vec<[f64; 2]>; 3]) -> Option<f64> {
    let cur = parse_curves(current)?;
    let mut ratio_sum = 0.0;
    let mut ratio_count = 0usize;
    for c in 0..3 {
        if cur[c].len() != full[c].len() {
            return None;
        }
        for (&[cx, cy], &[fx, fy]) in cur[c].iter().zip(&full[c]) {
            if (cx - fx).abs() > 1e-4 {
                return None;
            }
            let df = fy - fx;
            if df.abs() >= 1e-3 {
                let r = (cy - cx) / df;
                if !(-0.02..=1.02).contains(&r) {
                    return None;
                }
                ratio_sum += r.clamp(0.0, 1.0);
                ratio_count += 1;
            } else if (cy - cx).abs() > 2e-3 {
                return None;
            }
        }
    }
    if ratio_count == 0 {
        return None;
    }
    let avg = (ratio_sum / ratio_count as f64).clamp(0.0, 1.0);
    // Verify every interior anchor agrees closely with this single global scale factor.
    for c in 0..3 {
        for (&[_cx, cy], &[fx, fy]) in cur[c].iter().zip(&full[c]) {
            let expected = (fx + (fy - fx) * avg).clamp(0.0, 1.0);
            if (cy - expected).abs() > 2e-3 {
                return None;
            }
        }
    }
    Some((avg * 100.0).round().clamp(0.0, 100.0))
}

fn ensure_loaded_amount(cx: &mut AppContext) -> f64 {
    let generation = cx.document_generation;
    let loaded = cx.state_ref::<AutoColorState>().and_then(|s| s.loaded_generation);
    if loaded != Some(generation) {
        let saved = cx.doc.preference("auto_color_amount").and_then(|v| v.as_f64()).unwrap_or(100.0).clamp(0.0, 100.0);
        let state = cx.state::<AutoColorState>();
        state.amount = saved.round();
        state.loaded_generation = Some(generation);
        state.active = None;
    }
    cx.state::<AutoColorState>().amount
}

/// Returns the current Auto Color strength (`0..=100%`) while Auto Color is active on the open photo,
/// or `None` when the strength slider should remain hidden.
pub(crate) fn active_auto_color_amount(cx: &mut AppContext) -> Option<f64> {
    let generation = cx.document_generation;
    let photo_id = cx.doc.current_id();
    let current_curves = cx.settings().value("rgb_curves").clone();
    let state = cx.state::<AutoColorState>();
    let active = state.active.as_mut()?;
    if active.generation != generation || active.photo_id != photo_id {
        state.active = None;
        return None;
    }
    if current_curves == active.last_applied {
        return Some(state.amount);
    }
    if let Some(inferred) = infer_amount(&current_curves, &active.full_curves) {
        state.amount = inferred;
        active.last_applied = current_curves;
        return Some(inferred);
    }
    state.active = None;
    None
}

/// Applies live changes and commit events from the Basic panel's "자동 색 세기" slider.
pub(crate) fn apply_auto_color_amount(cx: &mut AppContext, event: crate::widgets::RowEvent) {
    if let Some(new_pct) = event.set {
        let clamped = new_pct.round().clamp(0.0, 100.0);
        let scaled = {
            let state = cx.state::<AutoColorState>();
            let Some(active) = state.active.as_mut() else { return };
            state.amount = clamped;
            let scaled = scale_curves(&active.full_curves, clamped);
            active.last_applied = scaled.clone();
            scaled
        };
        cx.doc.edit_mut().set("rgb_curves", scaled);
        cx.before = false;
    }
    if event.commit {
        let amount = cx.state::<AutoColorState>().amount;
        cx.doc.edit_mut().commit("자동 색 세기");
        let _ = cx.doc.save_preference("auto_color_amount", &serde_json::json!(amount));
        cx.status = tr_with("자동 색 적용 · 세기 {0}% · 채널 커브에서 다듬을 수 있습니다.", &[&format!("{amount:.0}")]);
    }
}

/// Port of `auto_color`: computes the full 100% cast-correction curves once, applies them at the saved
/// `auto_color_amount` (0–100%), and enables the live "자동 색 세기" slider in the Basic panel.
fn auto_color(cx: &mut AppContext, _: &Invocation) {
    let Some((id, path, settings, version, generation)) = target(cx) else { return };
    let amount = ensure_loaded_amount(cx);
    let source = super::auto_source::for_click(cx, &path, &settings);
    cx.jobs.spawn(
        "auto",
        tr("자동 색"),
        move |_| {
            let loaded = source.load(&settings)?;
            auto::auto_color_settings(loaded.source(), &settings, 1.0, &Externals::default()).map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, version, generation) {
                return;
            }
            match result {
                Ok((adjusted, report)) if report["status"] == "applied" => {
                    let full_value = adjusted.value("rgb_curves").clone();
                    let (applied_value, active) = match parse_curves(&full_value) {
                        Some(full_curves) => {
                            let scaled = scale_curves(&full_curves, amount);
                            (
                                scaled.clone(),
                                Some(ActiveAutoColor { generation, photo_id: id, full_curves, last_applied: scaled }),
                            )
                        }
                        None => (full_value, None),
                    };
                    let edit = cx.doc.edit_mut();
                    edit.set("rgb_curves", applied_value);
                    edit.commit("자동 색");
                    cx.before = false;
                    let state = cx.state::<AutoColorState>();
                    state.amount = amount;
                    state.loaded_generation = Some(generation);
                    state.active = active;
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
    let source = super::auto_source::for_click(cx, &path, &input);
    cx.jobs.spawn(
        "auto",
        tr("네거티브 반전"),
        move |_| {
            let loaded = source.load(&input)?;
            auto::measure_negative(loaded.source(), &settings, &Externals::default()).map_err(|e| e.to_string())
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
