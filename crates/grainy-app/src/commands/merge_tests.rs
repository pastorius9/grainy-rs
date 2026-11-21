use super::*;
use crate::commands;
use crate::render::TestCard;
use crate::session::Session;
use grainy_codecs::{ExportFormat, ExportOptions, LoadOptions};
use grainy_core::Image;
use grainy_engine::files::Cms;
use std::time::{Duration, Instant};

fn scene(x: usize, y: usize, c: usize) -> f32 {
    // Irregular edges across several scales; the vertical stripe saturates only the brighter exposure.
    if (120..133).contains(&x) {
        return [1.6, 1.3, 1.8][c];
    }
    let hash = (x / 7 * 713 + y / 5 * 917 + (x / 11 * y / 9 * 67)) % 223;
    0.045 + hash as f32 / 530. + (c as f32) * 0.025
}
fn source(path: &Path, dx: usize, dy: usize, scale: f32) {
    let data = Image::from_shape_fn((112, 180, 3), |(y, x, c)| (scene(x + dx, y + dy, c) * scale).min(1.));
    let opt = ExportOptions { format: ExportFormat::TiffHdr32, hdr_enabled: true, ..Default::default() };
    grainy_codecs::export_image(&data, path, path, &opt, &Cms::for_export_of_developed_pixels()).unwrap();
}
fn decode(path: &Path) -> Image {
    grainy_codecs::load_image(path, &LoadOptions::default(), &Cms::for_loading()).unwrap().pixels
}

#[test]
fn selected_hdr_bracket_aligns_restores_highlights_and_opens_in_session() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, out) = (dir.path().join("a.tif"), dir.path().join("b.tif"), dir.path().join("hdr.tif"));
    source(&a, 0, 0, 0.25);
    source(&b, 4, 2, 1.);
    let originals = [std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()];
    let mut session = Session::default();
    session.add([a.clone(), b.clone()]);
    session.select(0);
    session.toggle_selected(1);
    let mut cx = AppContext::new(&Default::default(), Box::<TestCard>::default(), Box::new(session));
    assert!(commands::run(&mut cx, "photo.merge_hdr", &Invocation::value(out.to_str().unwrap())));
    let started = Instant::now();
    loop {
        cx.poll_background();
        if !cx.jobs.running("photo_merge") {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(90));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(out.exists(), "{}", cx.status());
    assert_eq!(cx.doc.len(), 3);
    assert_eq!(cx.doc.current().unwrap().path, out);
    assert!(cx.doc.edit().settings().bool("hdr"));
    let pixels = decode(&out);
    assert_eq!(pixels.dim(), (110, 176, 3));
    assert!((pixels[[35, 122, 2]] - 1.8).abs() < 0.015, "highlight {}", pixels[[35, 122, 2]]);
    assert_eq!(std::fs::read(&a).unwrap(), originals[0]);
    assert_eq!(std::fs::read(&b).unwrap(), originals[1]);
}

#[test]
fn panorama_matches_overlapping_exposures_and_exclusive_output() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c, out) =
        (dir.path().join("a.tif"), dir.path().join("b.tif"), dir.path().join("c.tif"), dir.path().join("panorama.tif"));
    source(&a, 0, 0, 0.45);
    source(&b, 82, 3, 0.6);
    source(&c, 154, 1, 0.5);
    let paths = [a, b, c];
    let flag = AtomicBool::new(false);
    merge_to_tiff(&paths, &out, Mode::Panorama, &flag, &|_| {}).unwrap();
    let rgb = decode(&out);
    assert_eq!(rgb.dim(), (109, 334, 3));
    for (x, y) in [(24, 31), (101, 61), (197, 87), (300, 14)] {
        assert!((rgb[[y, x, 1]] - 0.45 * scene(x, y + 3, 1)).abs() < 0.004);
    }
    assert!(merge_to_tiff(&paths, &out, Mode::Panorama, &flag, &|_| {}).is_err());
    assert!(
        merge_to_tiff(&paths, &dir.path().join("cancel.tif"), Mode::Panorama, &AtomicBool::new(true), &|_| {}).is_err()
    );
    assert!(!dir.path().join("cancel.tif").exists());
}

#[test]
fn identical_non_bracket_cannot_create_an_hdr_result() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, out) = (dir.path().join("a.tif"), dir.path().join("b.tif"), dir.path().join("bad.tif"));
    source(&a, 0, 0, 1.);
    std::fs::copy(&a, &b).unwrap();
    assert!(merge_to_tiff(&[a, b], &out, Mode::Hdr, &AtomicBool::new(false), &|_| {}).is_err());
    assert!(!out.exists());
}
