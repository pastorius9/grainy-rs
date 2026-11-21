//! Generated photos only: the real GPU worker, CPU toggle and unchanged thumbnail lane.
mod common;

use grainy_app::{
    render::{Frame, PreviewJob, Renderer, Reply, Resolution, ThumbnailJob},
    worker::Worker,
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn wait(worker: &mut Worker) -> Vec<Reply> {
    let started = Instant::now();
    let mut replies = Vec::new();
    loop {
        replies.extend(worker.poll());
        if worker.idle() {
            return replies;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "worker timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn frame(replies: Vec<Reply>) -> Arc<Frame> {
    replies
        .into_iter()
        .find_map(|r| match r {
            Reply::Frame(f) => Some(f),
            Reply::Failed { message, .. } => panic!("{message}"),
            _ => None,
        })
        .expect("preview")
}

#[test]
fn preview_toggle_and_cpu_thumbnails() {
    let _turn = common::language_lock();
    check_toggle(None);
    check_toggle(Some(grainy_cms::profile("Display P3").expect("monitor profile")));
}

fn check_toggle(profile: Option<grainy_cms::Profile>) {
    let dir = common::temp_dir("gpu-worker");
    let path = dir.join("generated.png");
    common::save_png(&common::garden(1, [1.0; 3], false), &path);
    let mut cpu = Worker::new();
    let mut gpu = Worker::with_gpu(true);
    gpu.set_monitor(profile.clone());
    cpu.set_monitor(profile);
    let settings = common::settings(json!({"exposure":0.3,"saturation":15,"grain":20}));
    let job = PreviewJob {
        id: 1,
        photo: 1,
        path: path.clone(),
        settings: settings.clone(),
        resolution: Resolution::Fit,
        crop: grainy_engine::Crop::Cropped,
    };
    cpu.preview(job.clone());
    gpu.preview(job);
    let expected = frame(wait(&mut cpu));
    let actual = frame(wait(&mut gpu));
    let status = gpu.gpu_status().unwrap();
    println!("GPU worker status: {}", status.text);
    assert_eq!(actual.original, expected.original);
    assert_eq!(actual.preview.width, expected.preview.width);
    let error = actual.preview.rgba.iter().zip(&expected.preview.rgba).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    // A float32 ULP at a quantization boundary can change one displayed byte.
    assert!(error <= 1, "RGBA8 max difference {error}");
    gpu.set_gpu_enabled(false);
    assert_eq!(gpu.gpu_status().unwrap().text, "off");
    let disabled = frame(wait(&mut gpu));
    assert_eq!(disabled.preview, expected.preview);
    assert_eq!(disabled.bins, expected.bins);
    assert_eq!(disabled.clipping, expected.clipping);
    gpu.set_gpu_enabled(true);
    assert_eq!(frame(wait(&mut gpu)).preview, actual.preview);
    let thumb = ThumbnailJob { photo: 1, path, settings, key: 2, cache: None };
    cpu.thumbnail(thumb.clone());
    gpu.thumbnail(thumb);
    let thumbnail = |replies: Vec<Reply>| {
        replies
            .into_iter()
            .find_map(|r| match r {
                Reply::Thumbnail(t) => Some(t.result.unwrap()),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(thumbnail(wait(&mut gpu)), thumbnail(wait(&mut cpu)));
}

#[test]
fn disabled_gpu_does_not_initialize_or_change_pixels() {
    let _turn = common::language_lock();
    let mut worker = Worker::with_gpu(false);
    assert_eq!(worker.gpu_status().unwrap().text, "off");
    let dir = common::temp_dir("gpu-disabled");
    let path = dir.join("generated.png");
    common::save_png(&common::garden(1, [1.0; 3], false), &path);
    let settings = common::settings(json!({"exposure":0.7}));
    let expected = common::engine_render(&path, Some(grainy_app::worker::PREVIEW_SIZE), &settings);
    worker.preview(PreviewJob {
        id: 1,
        photo: 1,
        path,
        settings,
        resolution: Resolution::Fit,
        crop: grainy_engine::Crop::Cropped,
    });
    let actual = frame(wait(&mut worker));
    assert_eq!(actual.preview.rgba, expected.0);
}
