//! The Crop panel's actions (luma/app.py): 크롭 편집 (`toggle_crop`), 적용 (`accept_crop`), 취소 (`cancel_crop`),
//! the aspect box (`set_aspect`), 크롭 초기화 (`reset_crop`), 스캔 테두리 자동 자르기 (`auto_border_crop`, a
//! job: `scan_border.crop_for`), 시계 방향으로 90° 회전 (`rotate_photo`), 좌우 반전, 자동 수평 맞추기
//! (`auto_straighten`, a job: `upright.estimate(..., 'level')`). The frame and the window's crop functions are in
//! [`crate::tools::crop`].

use grainy_core::Settings;
use grainy_engine::files::load_photo;
use grainy_engine::{auto, Externals};

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::state::settings_eq;
use crate::text::tr;
use crate::tools::crop;

pub const COMMANDS: &[Command] = &[
    Command { id: "crop.toggle", label: "크롭 편집", enabled: photo_open, run: toggle },
    Command { id: "crop.apply", label: "적용", enabled: open, run: apply },
    Command { id: "crop.cancel", label: "취소", enabled: open, run: cancel },
    Command { id: "crop.aspect", label: "자유 비율", enabled: photo_open, run: aspect },
    Command { id: "crop.reset", label: "크롭 초기화", enabled: photo_open, run: reset },
    Command { id: "crop.border", label: "스캔 테두리 자동 자르기", enabled: border_idle, run: border },
    Command { id: "crop.rotate", label: "시계 방향으로 90° 회전", enabled: photo_open, run: rotate },
    Command { id: "crop.flip", label: "좌우 반전", enabled: photo_open, run: flip },
    Command { id: "crop.auto_level", label: "자동 수평 맞추기", enabled: level_idle, run: auto_level },
];

/// The develop source of the oracle's window (`load_image(path, 1800, ...)`).
const SOURCE_SIZE: usize = 1800;
/// `crop_for`'s default `longest`.
const BORDER_SIZE: usize = 1200;

fn open(cx: &AppContext) -> bool {
    crop::is_open(cx)
}

fn border_idle(cx: &AppContext) -> bool {
    photo_open(cx) && !cx.jobs.running("crop_border")
}

fn level_idle(cx: &AppContext) -> bool {
    photo_open(cx) && crate::panels::transform::buttons_enabled(cx)
}

fn toggle(cx: &mut AppContext, _: &Invocation) {
    crop::toggle(cx);
}

fn apply(cx: &mut AppContext, _: &Invocation) {
    crop::accept(cx);
}

fn cancel(cx: &mut AppContext, _: &Invocation) {
    crop::cancel(cx);
}

/// The aspect box: `value` is the index in [`crop::ASPECTS`].
fn aspect(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(index) = invocation.value.as_ref().and_then(serde_json::Value::as_u64) {
        crop::choose_aspect(cx, index as usize);
    }
}

fn reset(cx: &mut AppContext, _: &Invocation) {
    crop::reset(cx);
}

fn rotate(cx: &mut AppContext, _: &Invocation) {
    crop::rotate(cx);
}

fn flip(cx: &mut AppContext, _: &Invocation) {
    crop::flip(cx);
}

/// The open photo's id, path and settings (the job's input).
fn target(cx: &AppContext) -> Option<(u64, std::path::PathBuf, Settings, u64)> {
    let photo = cx.doc.current()?;
    Some((photo.id, photo.path.clone(), cx.settings().clone(), cx.document_generation))
}

/// Whether the job's photo is still open with the settings it was measured with.
fn still(cx: &AppContext, id: u64, saved: &Settings, generation: u64) -> bool {
    cx.document_generation == generation && cx.doc.current_id() == id && settings_eq(cx.settings(), saved)
}

/// The last line of an error without its exception name (`str(error).strip().splitlines()[-1].split(': ',1)[-1]`).
fn short(message: &str) -> String {
    let last = message.trim().lines().last().unwrap_or("");
    last.split_once(": ").map_or(last, |(_, rest)| rest).to_owned()
}

/// Port of `auto_border_crop`.
fn border(cx: &mut AppContext, _: &Invocation) {
    crop::border_begin(cx);
    let Some((id, path, saved, generation)) = target(cx) else { return };
    let settings = saved.clone();
    cx.jobs.spawn(
        "crop_border",
        tr("스캔 테두리 자동 자르기"),
        move |_| {
            let loaded = load_photo(&path, Some(SOURCE_SIZE), &settings).map_err(|e| e.to_string())?;
            auto::scan_border_crop(loaded.photo.source(), &settings, BORDER_SIZE, &Externals::default())
                .map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            if !still(cx, id, &saved, generation) {
                cx.set_status(tr("분석 중 사진이나 설정이 바뀌어 결과를 적용하지 않았습니다."));
                return;
            }
            match result {
                Ok(Some(found)) => {
                    crop::border_result(cx, found);
                    cx.set_status(tr("스캔 테두리를 잘랐습니다. 크롭 편집으로 조정할 수 있습니다."));
                }
                Ok(None) => cx.set_status(tr("자를 스캔 테두리를 찾지 못했습니다.")),
                Err(_) => cx.set_status(tr("스캔 테두리를 분석하지 못했습니다.")),
            }
        },
    );
}

/// Port of `auto_straighten`.
fn auto_level(cx: &mut AppContext, _: &Invocation) {
    let Some((id, path, saved, generation)) = target(cx) else { return };
    let settings = saved.clone();
    crop::set_level_note(cx, Some(tr("사진의 기준선을 분석하고 있습니다…")));
    cx.jobs.spawn(
        "auto_level",
        tr("자동 수평 맞추기"),
        move |_| {
            let loaded = load_photo(&path, Some(SOURCE_SIZE), &settings).map_err(|e| e.to_string())?;
            auto::upright_for(loaded.photo.source(), &settings, "level", &Externals::default())
                .map(|estimate| estimate.rotation)
                .map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            let same = still(cx, id, &saved, generation);
            match result {
                _ if !same => {
                    crop::set_level_note(cx, None);
                    cx.set_status(tr("분석 중 사진이나 설정이 바뀌어 결과를 적용하지 않았습니다."));
                }
                // The estimate turns the photo counter-clockwise by `rotation`; the slider's positive side is
                // clockwise.
                Ok(rotation) => crop::auto_level_result(cx, -rotation),
                Err(message) => crop::set_level_note(cx, Some(short(&message))),
            }
        },
    );
}
