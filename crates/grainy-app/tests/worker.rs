//! The render worker, the histogram, the session and export naming, without a window.

mod common;

use std::time::{Duration, Instant};

use grainy_app::export::{self, ExportEvent, ExportOptions};
use grainy_app::histogram;
use grainy_app::render::{PreviewJob, Renderer, Reply, Resolution, ThumbnailJob};
use grainy_app::session::{self, Session};
use grainy_app::worker::Worker;
use grainy_core::Settings;
use serde_json::json;

fn wait(worker: &mut Worker) -> Vec<Reply> {
    let started = Instant::now();
    let mut replies = Vec::new();
    loop {
        replies.extend(worker.poll());
        if worker.idle() {
            replies.extend(worker.poll());
            return replies;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn job(id: u64, path: &std::path::Path, settings: Settings) -> PreviewJob {
    PreviewJob {
        id,
        photo: 1,
        path: path.to_path_buf(),
        settings,
        resolution: Resolution::Fit,
        crop: grainy_engine::Crop::Cropped,
    }
}

#[test]
fn fit_delivers_screen_pixels_after_crop_and_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::new();
    for (width, height, rotation, crop, shown, expected) in [
        (2048, 1536, 0, [0.25, 0.25, 0.75, 0.75], 1024, (1024, 768)),
        (3072, 1536, 0, [0.0, 0.0, 0.25, 0.25], 640, (640, 320)),
        (1536, 3072, 1, [0.0, 0.0, 0.25, 0.5], 768, (768, 768)),
        (2048, 1536, 0, [0.0, 0.0, 1.0, 1.0], 1024, (1024, 768)),
    ] {
        let path = dir.path().join(format!("{width}-{height}.png"));
        if !path.exists() {
            image::RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, (x ^ y) as u8]))
                .save(&path)
                .unwrap();
        }
        worker.set_view_size(Some(shown));
        let settings = common::settings(json!({"rotation": rotation, "crop": crop}));
        worker.preview(job(1, &path, settings.clone()));
        let frame = wait(&mut worker)
            .into_iter()
            .find_map(|reply| match reply {
                Reply::Frame(f) => Some(f),
                Reply::Failed { message, .. } => panic!("{message}"),
                Reply::Thumbnail(_) => None,
            })
            .expect("cropped frame");
        assert_eq!(frame.original, (width, height));
        assert_eq!((frame.preview.width, frame.preview.height), expected);
        let reference = common::engine_render_for_frame(&path, &frame, &settings);
        assert!(frame.preview.rgba == reference.0, "cropped pixels equal the CPU develop at screen resolution");
    }
}

#[test]
fn histogram_matches_the_oracle() {
    for name in ["histogram_garden", "histogram_small"] {
        let case = grainy_testkit::Case::open("app", name);
        let input: ndarray::Array3<f32> = case.array("input");
        let expected: ndarray::Array2<i64> = case.array("bins");
        let converted = histogram::convert(&input);
        let bins = histogram::bins(&converted.rgba, converted.width, converted.height);
        for c in 0..3 {
            for b in 0..128 {
                assert_eq!(i64::from(bins[c][b]), expected[[c, b]], "{name}: channel {c} bin {b}");
            }
        }
        let meta = case.json();
        let flags = |key: &str| -> [bool; 3] {
            let list = meta[key].as_array().expect("flags");
            [0, 1, 2].map(|i| list[i].as_bool().unwrap_or(false))
        };
        assert_eq!(converted.channels, (flags("shadows"), flags("highlights")), "{name}: clipping channels");
        // The overlay marks the clipped pixels.
        let overlay = histogram::overlay(&converted.mask, true, true);
        let marked = overlay.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
        assert_eq!(marked > 0, converted.channels.0.iter().chain(&converted.channels.1).any(|v| *v));
    }
}

#[test]
fn newest_request_wins() {
    let dir = common::temp_dir("newest");
    let path = dir.join("garden.png");
    common::save_png(&common::garden(4, [1.0; 3], false), &path);
    let mut worker = Worker::new();
    for id in 1..=12u64 {
        let s = common::settings(json!({"exposure": id as f64 * 0.1, "clarity": 20}));
        worker.preview(job(id, &path, s));
    }
    let replies = wait(&mut worker);
    let ids: Vec<u64> = replies
        .iter()
        .map(|r| match r {
            Reply::Frame(f) => f.id,
            Reply::Failed { message, .. } => panic!("render failed: {message}"),
            Reply::Thumbnail(_) => 0,
        })
        .collect();
    assert_eq!(ids.last(), Some(&12), "the newest request is the last answer");
    assert!(worker.preview_renders() < 12, "replaced requests are not rendered ({} renders)", worker.preview_renders());
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "answers come in order: {ids:?}");
}

#[test]
fn the_development_cache_is_reused() {
    let dir = common::temp_dir("cache");
    let path = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &path);
    let mut worker = Worker::new();
    let frame_of = |replies: Vec<Reply>| {
        replies
            .into_iter()
            .find_map(|r| match r {
                Reply::Frame(f) => Some(f),
                _ => None,
            })
            .expect("a frame")
    };
    worker.preview(job(1, &path, common::settings(json!({"exposure": 0.5}))));
    let first = frame_of(wait(&mut worker));
    // Only a late stage changes: the earlier stages come from the cache.
    worker.preview(job(2, &path, common::settings(json!({"exposure": 0.5, "vignette": -30}))));
    let second = frame_of(wait(&mut worker));
    assert!(second.hits > first.hits, "hits {} → {}", first.hits, second.hits);
    // The same settings again: no stage is computed.
    worker.preview(job(3, &path, common::settings(json!({"exposure": 0.5, "vignette": -30}))));
    let third = frame_of(wait(&mut worker));
    assert_eq!(third.misses, second.misses, "nothing recomputed");
    assert_eq!(third.preview, second.preview, "the cache gives the same pixels");
    // The frame is the engine's develop.
    let (rgba, w, h) =
        common::engine_render(&path, Some(2048), &common::settings(json!({"exposure": 0.5, "vignette": -30})));
    assert_eq!((w, h), (third.preview.width, third.preview.height));
    assert!(rgba == third.preview.rgba, "the worker's frame is the engine's develop");
}

#[test]
fn failures_are_answered_and_the_worker_goes_on() {
    let dir = common::temp_dir("failures");
    let path = dir.join("garden.png");
    common::save_png(&common::garden(1, [1.0; 3], false), &path);
    let mut worker = Worker::new();
    worker.preview(job(1, &dir.join("missing.png"), common::settings(json!({}))));
    let replies = wait(&mut worker);
    assert!(matches!(&replies[..], [Reply::Failed { id: 1, .. }]), "{replies:?}");
    // A develop that fails (an unusable lens profile): the engine's message comes back.
    let lens = json!({"lensfun_enabled": true, "lensfun": {"crop": 1.0, "focal": 50.0, "xml": "<lens/>"}});
    worker.preview(job(2, &path, common::settings(lens)));
    match &wait(&mut worker)[..] {
        [Reply::Failed { id: 2, message, .. }] => assert!(message.contains("렌즈"), "{message}"),
        other => panic!("{other:?}"),
    }
    worker.preview(job(3, &path, common::settings(json!({}))));
    assert!(matches!(&wait(&mut worker)[..], [Reply::Frame(f)] if f.id == 3));
    // A broken RAW file fails with the loader's text.
    let raw = dir.join("photo.nef");
    std::fs::write(&raw, b"not a raw file").expect("write");
    worker.preview(job(4, &raw, common::settings(json!({}))));
    match &wait(&mut worker)[..] {
        [Reply::Failed { id: 4, message, .. }] => assert!(!message.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn thumbnails_use_each_photos_settings() {
    let dir = common::temp_dir("thumbs");
    let paths = common::eight_photos(&dir);
    let mut worker = Worker::new();
    let plain = common::settings(json!({}));
    let dark = common::settings(json!({"exposure": -2.0}));
    for (i, path) in paths.iter().take(2).enumerate() {
        let settings = if i == 0 { plain.clone() } else { dark.clone() };
        let key = session::thumb_key(path, &settings);
        worker.thumbnail(ThumbnailJob { photo: i as u64 + 1, path: path.clone(), settings, key, cache: None });
    }
    let replies = wait(&mut worker);
    assert_eq!(replies.len(), 2);
    let mut received = Vec::new();
    for reply in replies {
        let Reply::Thumbnail(thumb) = reply else { panic!("{reply:?}") };
        let preview = thumb.result.expect("thumbnail");
        assert_eq!(preview.width.max(preview.height), 320, "thumbnails are 320 pixels");
        assert_eq!(thumb.original, Some((480, 320)));
        let (path, settings) = match thumb.photo {
            1 => (&paths[0], &plain),
            2 => (&paths[1], &dark),
            other => panic!("unexpected thumbnail for photo {other}"),
        };
        let (expected, width, height) = common::engine_render(path, Some(320), settings);
        assert_eq!((preview.width, preview.height), (width, height));
        assert!(preview.rgba == expected, "thumbnail {} differs from its photo's own develop", thumb.photo);
        received.push(thumb.photo);
    }
    received.sort_unstable();
    assert_eq!(received, [1, 2]);
}

#[test]
fn thumbnail_keys_follow_path_and_settings() {
    let a = std::path::Path::new("a.png");
    let b = std::path::Path::new("b.png");
    let s = common::settings(json!({}));
    let t = common::settings(json!({"exposure": 0.3}));
    assert_eq!(session::thumb_key(a, &s), session::thumb_key(a, &s.clone()));
    assert_ne!(session::thumb_key(a, &s), session::thumb_key(b, &s));
    assert_ne!(session::thumb_key(a, &s), session::thumb_key(a, &t));
}

#[test]
fn each_photo_keeps_its_edits_and_undo() {
    let mut session = Session::default();
    let added = session.add(["a.png", "b.png", "c.png"].map(std::path::PathBuf::from));
    assert_eq!(added, 3);
    assert_eq!(session.add([std::path::PathBuf::from("a.png")]), 0, "a photo is added once");
    assert_eq!(session.current_index(), Some(0));
    session.edit.set("exposure", 0.5);
    session.edit.commit("보정 변경");
    session.edit.set("contrast", 10.0); // not committed: switching commits it
    assert!(session.select(1));
    session.edit.set("exposure", -1.0);
    session.edit.commit("보정 변경");
    assert!(session.step(-1));
    assert_eq!(session.edit.settings().f64("exposure"), 0.5);
    assert_eq!(session.edit.settings().f64("contrast"), 10.0);
    assert_eq!(session.edit.depth(), (2, 0));
    assert!(session.edit.undo());
    assert_eq!(session.edit.settings().f64("contrast"), 0.0);
    assert_eq!(session.settings_of(1).f64("exposure"), -1.0);
    assert_eq!(session.settings_of(2).f64("exposure"), 0.0);
    assert!(!session.step(-1), "already at the first photo");
    assert!(session.step(5) && session.current_index() == Some(2), "stops at the last photo");
}

#[test]
fn copy_and_paste_between_photos() {
    let mut session = Session::default();
    session.add(["a.png", "b.png", "c.png"].map(std::path::PathBuf::from));
    session.edit.set("exposure", 0.8);
    session.edit.set("film_look", "kodak-portra-400");
    session.edit.set("crop", json!([0.1, 0.1, 0.9, 0.9]));
    session.edit.commit("보정 변경");
    assert!(session.copy());
    session.select(1);
    session.toggle_selected(2);
    assert_eq!(session.selected_indices(), vec![1, 2]);
    assert_eq!(session.paste(&session.selected_indices()), Ok(2));
    for i in [1, 2] {
        let s = session.settings_of(i);
        assert_eq!(s.f64("exposure"), 0.8, "photo {i}");
        assert_eq!(s.value("crop"), &serde_json::Value::Null, "crop is not copied by default");
    }
    assert_eq!(session.edit.history()[0], "보정 붙여넣기");
    assert!(session.edit.undo());
    assert_eq!(session.edit.settings().f64("exposure"), 0.0);
}

#[test]
fn folders_are_read_with_their_subfolders() {
    let dir = common::temp_dir("folders");
    std::fs::create_dir_all(dir.join("sub")).expect("folder");
    let image = common::garden(1, [1.0; 3], false);
    for name in ["b.png", "a.PNG", "sub/c.tif"] {
        if name.ends_with(".tif") {
            common::save_tiff(&image, &dir.join(name));
        } else {
            common::save_png(&image, &dir.join(name));
        }
    }
    std::fs::write(dir.join("notes.txt"), "x").expect("write");
    std::fs::write(dir.join(".hidden.png"), "x").expect("write");
    let files = session::photo_files(std::slice::from_ref(&dir));
    let names: Vec<String> = files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["a.PNG", "b.png", "c.tif"]);
}

#[test]
fn export_names_never_overwrite() {
    let dir = common::temp_dir("export_names");
    let source = dir.join("garden.png");
    common::save_png(&common::garden(1, [1.0; 3], false), &source);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).expect("folder");
    let items = vec![(source.clone(), common::settings(json!({})))];
    let first = export::plans(&items, &out, "JPEG", export::NAMING).expect("plans");
    assert_eq!(first, vec![out.join("garden_grainy.jpg")]);
    std::fs::write(out.join("garden_grainy.jpg"), b"taken").expect("write");
    let second = export::plans(&items, &out, "JPEG", export::NAMING).expect("plans");
    assert_eq!(second, vec![out.join("garden_grainy_2.jpg")]);
    // Two photos with the same stem in one export.
    let twice = vec![items[0].clone(), items[0].clone()];
    let both = export::plans(&twice, &out, "TIFF 16-bit", "{stem}_{n:03d}").expect("plans");
    assert_eq!(both, vec![out.join("garden_001.tif"), out.join("garden_002.tif")]);
    // The dialog's checks.
    let mut options = ExportOptions { folder: out.to_string_lossy().into_owned(), ..ExportOptions::default() };
    assert_eq!(options.validate(), Ok(()));
    options.naming = "a/b".into();
    assert!(options.validate().is_err());
    options.folder = dir.join("nowhere").to_string_lossy().into_owned();
    assert_eq!(options.validate(), Err("저장할 폴더를 선택해 주세요."));
}

#[test]
fn export_writes_new_files_on_a_thread() {
    let dir = common::temp_dir("export_run");
    let source = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &source);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).expect("folder");
    std::fs::write(out.join("garden_grainy.jpg"), b"someone else's file").expect("write");
    let settings = common::settings(json!({"exposure": 0.4}));
    let options =
        ExportOptions { folder: out.to_string_lossy().into_owned(), longest: 200, ..ExportOptions::default() };
    let rx =
        export::spawn(vec![(source.clone(), settings.clone()), (dir.join("missing.png"), settings)], options, None);
    let mut events = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_secs(120)) {
        let last = matches!(event, ExportEvent::Finished { .. });
        events.push(event);
        if last {
            break;
        }
    }
    assert!(events.contains(&ExportEvent::Progress { done: 2, total: 2 }), "{events:?}");
    let Some(ExportEvent::Finished { written, failures }) = events.last() else { panic!("{events:?}") };
    assert_eq!(written, &vec![out.join("garden_grainy_2.jpg")]);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(std::fs::read(out.join("garden_grainy.jpg")).expect("read"), b"someone else's file");
    let written = image::open(&written[0]).expect("a JPEG");
    assert_eq!(written.width().max(written.height()), 200);
}

/// Render times of a 24 MP JPEG (6000 × 4000): `cargo test -p grainy-app --release --test worker -- --ignored
/// --nocapture timing`.
#[test]
#[ignore = "timing on this machine; run in release"]
fn timing_24_megapixels() {
    let dir = common::temp_dir("timing");
    let path = dir.join("garden-24mp.jpg");
    let g = common::garden(25, [1.0; 3], false);
    let (h, w, _) = g.dim();
    let buffer = image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let p = |c: usize| {
            let v = g[[y as usize, x as usize, c]].clamp(0.0, 1.0);
            let e = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (e * 255.0 + 0.5) as u8
        };
        image::Rgb([p(0), p(1), p(2)])
    });
    buffer.save(&path).expect("write the JPEG");
    let mut worker = Worker::new();
    let steps: [(&str, Resolution, serde_json::Value); 6] = [
        ("fit, defaults (first load)", Resolution::Fit, json!({})),
        ("fit, exposure +0.5", Resolution::Fit, json!({"exposure": 0.5})),
        ("fit, exposure +0.5, vignette (cache)", Resolution::Fit, json!({"exposure": 0.5, "vignette": -20})),
        ("full, defaults (first load)", Resolution::Full, json!({})),
        ("full, exposure +0.5", Resolution::Full, json!({"exposure": 0.5})),
        ("full, exposure +0.5, vignette (cache)", Resolution::Full, json!({"exposure": 0.5, "vignette": -20})),
    ];
    for (id, (what, resolution, settings)) in steps.into_iter().enumerate() {
        let started = Instant::now();
        let job = PreviewJob {
            id: id as u64 + 1,
            photo: 1,
            path: path.clone(),
            settings: common::settings(settings),
            resolution,
            crop: grainy_engine::Crop::Cropped,
        };
        worker.preview(job);
        for reply in wait(&mut worker) {
            match reply {
                Reply::Frame(f) => println!(
                    "{what}: {} × {} · develop+convert {:.0} ms · with loading {:.0} ms · hits {} misses {}",
                    f.preview.width,
                    f.preview.height,
                    f.millis,
                    started.elapsed().as_secs_f64() * 1000.0,
                    f.hits,
                    f.misses
                ),
                other => panic!("{other:?}"),
            }
        }
    }
}
