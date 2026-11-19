//! The canvas tools' coordinate maps against the oracle's own `StudioTools` methods (`golden/app/coordinates_*`):
//! `source_point`, `source_points` (exact: the same geometry stage maps the coordinates), `frame_affine`,
//! `gradient_point` and `screen_point` (the fitted affine maps: normal equations here, numpy's SVD `lstsq` in the
//! oracle; tolerance 1e-9).

use grainy_app::imagespace::{apply, CoordinateMap};
use grainy_core::normalized;
use grainy_testkit::Case;
use serde_json::Value;

const CASES: [&str; 7] =
    ["plain", "rotation_1", "rotation_3_flip", "crop", "straighten", "straighten_crop_rotation", "transform"];

fn points(v: &Value) -> Vec<[f64; 2]> {
    v.as_array().unwrap().iter().map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()]).collect()
}

fn close(a: [f64; 2], b: [f64; 2], tolerance: f64) -> f64 {
    let e = (a[0] - b[0]).abs().max((a[1] - b[1]).abs());
    assert!(e <= tolerance, "{a:?} vs {b:?}");
    e
}

#[test]
fn coordinate_maps_match_the_oracle() {
    for name in CASES {
        let case = Case::open("app", &format!("coordinates_{name}"));
        let meta = case.json();
        let mut settings = grainy_core::defaults().clone();
        for (k, v) in meta["changes"].as_object().unwrap() {
            settings.set(k, v.clone());
        }
        let settings = normalized(&settings.0);
        let w = meta["width"].as_u64().unwrap() as usize;
        let h = meta["height"].as_u64().unwrap() as usize;
        let map = CoordinateMap::new(w, h, &settings).unwrap();
        let frame = meta["frame_size"].as_array().unwrap();
        assert_eq!(
            map.frame_size(),
            (frame[0].as_u64().unwrap() as usize, frame[1].as_u64().unwrap() as usize),
            "{name}"
        );
        let frame_points = points(&meta["frame_points"]);
        let off_points = points(&meta["off_points"]);
        let mut largest = 0.0f64;
        for (p, expected) in frame_points.iter().zip(meta["source"].as_array().unwrap()) {
            match map.source_point(*p) {
                Some(q) => {
                    largest = largest.max(close(q, [expected[0].as_f64().unwrap(), expected[1].as_f64().unwrap()], 0.0))
                }
                None => assert!(expected.is_null(), "{name} {p:?}"),
            }
        }
        let stroke = map.source_points(&frame_points);
        assert_eq!(stroke.len(), meta["stroke"].as_array().unwrap().len(), "{name}");
        for (q, e) in stroke.iter().zip(points(&meta["stroke"])) {
            close(*q, e, 0.0);
        }
        let (forward, backward) = map.affine();
        let mut affine = 0.0f64;
        for (m, key) in [(forward, "forward"), (backward, "backward")] {
            let expected = meta[key].as_array().unwrap();
            for r in 0..2 {
                for c in 0..3 {
                    let e = expected[r][c].as_f64().unwrap();
                    affine = affine.max((m[r][c] - e).abs());
                }
            }
        }
        assert!(affine < 1e-9, "{name}: affine maps differ by {affine}");
        for (p, e) in frame_points.iter().chain(&off_points).zip(points(&meta["gradient"])) {
            affine = affine.max(close(map.gradient_point(*p), e, 1e-9));
        }
        let sources: Vec<[f64; 2]> = points(&meta["source_points_in"]).into_iter().chain(off_points.clone()).collect();
        for (p, e) in sources.iter().zip(points(&meta["screen"])) {
            affine = affine.max(close(map.screen_point(*p), e, 1e-9));
        }
        let _ = apply(&forward, [0.0, 0.0]);
        println!("{name}: source points exact (largest {largest}); fitted maps within {affine:.2e}");
    }
}
