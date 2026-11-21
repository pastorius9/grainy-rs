//! Generate a matte once on a worker, in oriented source space. Subsequent previews, export and
//! reopening a catalogue only read the saved matte.
use grainy_core::{Image, Settings};
use grainy_local::selection::{self, Box2d, FULL, SIDE};
use serde_json::{json, Map, Value};

use super::{actions, Mode, Operation};
use crate::context::AppContext;
use crate::panels::Tool;
use crate::text::{tr, tr_with};
use crate::tools::range_pick::{self, RangeState};

#[cfg(windows)]
mod winml;

fn pixels(image: &Image, bounds: Box2d) -> Vec<[u8; 3]> {
    let (h, w, _) = image.dim();
    let sample = |i: usize, n: usize, lo: f64, hi: f64| {
        let p = ((lo + (hi - lo) * (i as f64 + 0.5) / SIDE as f64) * n as f64 - 0.5).clamp(0., (n - 1) as f64);
        (p as usize, (p as usize + 1).min(n - 1), p.fract() as f32)
    };
    (0..SIDE * SIDE)
        .map(|i| {
            let (x0, x1, dx) = sample(i % SIDE, w, bounds[0], bounds[2]);
            let (y0, y1, dy) = sample(i / SIDE, h, bounds[1], bounds[3]);
            std::array::from_fn(|c| {
                let lerp = |y| image[[y, x0, c]] * (1. - dx) + image[[y, x1, c]] * dx;
                ((lerp(y0) * (1. - dy) + lerp(y1) * dy).clamp(0., 1.) * 255.).round() as u8
            })
        })
        .collect()
}

fn generate(
    path: &std::path::Path,
    settings: &Settings,
    kind: &str,
    bounds: Box2d,
) -> Result<(Map<String, Value>, bool), String> {
    let mut s = settings.clone();
    let defaults = grainy_core::defaults();
    for key in grainy_engine::GEOMETRY_KEYS {
        s.0.insert(key.to_owned(), defaults.value(key).clone());
    }
    s.set("masks", json!([]));
    s.set("hdr", false);
    let loaded = range_pick::load(path, 1200, &s)?;
    let image = grainy_engine::develop(loaded.photo.source(), &s.0, &Default::default()).map_err(|e| e.to_string())?;
    let rgb = pixels(&image, bounds);
    #[cfg(windows)]
    let learned = winml::foreground(&rgb)?;
    #[cfg(not(windows))]
    let learned: Option<Vec<u8>> = None;
    let is_learned = learned.is_some();
    let matte = match learned {
        Some(matte) => matte,
        None => selection::foreground(&rgb)
            .map_err(|_| tr("피사체의 경계를 찾지 못했습니다. 개체를 더 가깝게 둘러싸 주세요."))?,
    };
    let component = selection::component(kind, bounds, &matte).map_err(|e| e.to_string())?;
    Ok((component, is_learned))
}

pub(super) fn choose(cx: &mut AppContext, kind: &str, new_layer: bool, operation: Operation) -> bool {
    match kind {
        "object" => {
            actions::mode(cx, Mode::Object);
            let state = actions::state(cx);
            state.new_object_layer = new_layer;
            state.operation = operation;
        }
        "subject" | "background" => {
            actions::mode(cx, Mode::None);
            let kind = if kind == "subject" { "subject" } else { "background" };
            start(cx, kind, FULL, new_layer, operation);
        }
        _ => return false,
    }
    true
}

pub(crate) fn rectangle(cx: &mut AppContext, a: [f64; 2], b: [f64; 2]) {
    if (a[0] - b[0]).abs() < 0.008 || (a[1] - b[1]).abs() < 0.008 {
        return;
    }
    let Some(map) = actions::coordinate_map(cx) else {
        return;
    };
    let corners = [[a[0], a[1]], [a[0], b[1]], [b[0], a[1]], [b[0], b[1]]];
    let points = map.source_points(&corners);
    if points.len() != 4 {
        cx.set_status(tr("사진 안에서 개체를 둘러싸 주세요."));
        return;
    }
    let mut bounds: Box2d = [1., 1., 0., 0.];
    for [x, y] in points {
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(y);
        bounds[2] = bounds[2].max(x);
        bounds[3] = bounds[3].max(y);
    }
    if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
        return;
    }
    let state = actions::state(cx);
    let (new_layer, op) = (state.new_object_layer, state.operation);
    start(cx, "object", bounds, new_layer, op);
}

fn start(cx: &mut AppContext, kind: &'static str, bounds: Box2d, new_layer: bool, operation: Operation) {
    let Some(photo) = cx.doc.current() else {
        return;
    };
    let (id, path) = (photo.id, photo.path.clone());
    let document = cx.document_generation;
    let state = actions::state(cx);
    let (layer, index, mode) = (state.layer, state.component, state.mode);
    let settings = cx.settings().clone();
    let snapshot = settings.clone();
    let generation = &mut cx.state::<RangeState>().generation;
    *generation += 1;
    let token = *generation;
    let name = tr(super::kind_name(kind));
    cx.set_status(tr("선택 영역을 계산하는 중…"));
    cx.jobs.spawn(
        "selection_matte",
        name.clone(),
        move |handle| {
            if handle.cancelled() {
                return Err(tr("취소했습니다."));
            }
            let result = generate(&path, &settings, kind, bounds);
            if handle.cancelled() {
                return Err(tr("취소했습니다."));
            }
            result
        },
        move |cx: &mut AppContext, result| {
            if cx.document_generation != document
                || cx.doc.current_id() != id
                || cx.tool() != Tool::Masks
                || cx.settings() != &snapshot
                || cx.state::<RangeState>().generation != token
            {
                return;
            }
            let state = actions::state(cx);
            if (state.layer, state.component, state.mode) != (layer, index, mode) {
                return;
            }
            let (mut component, learned) = match result {
                Ok(x) => x,
                Err(e) => {
                    cx.set_status(e);
                    return;
                }
            };
            cx.doc.edit_mut().commit("보정 변경");
            if new_layer {
                actions::new_layer(cx);
            }
            actions::state(cx).fresh_layer = None;
            component.insert("operation".into(), json!(operation.as_str()));
            actions::add_component(cx, component);
            actions::mode(cx, Mode::None);
            cx.doc.edit_mut().commit(super::kind_name(kind));
            let method = if learned { "U²-Net".to_owned() } else { tr("경계 검출") };
            cx.set_status(tr_with("{0} 선택 완료 · {1}", &[&name, &method]));
        },
    );
}
