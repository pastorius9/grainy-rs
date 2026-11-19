//! Work package 28: XMP sidecars and the GPS map, against the oracle.
//!
//! * `golden/app/xmp` (`tools/golden/gen_app_xmp.py`): the Python Grainy's own `write_xmp` / `read_xmp` ran over a
//!   small library (virtual copies, a RAW photo, a name without an extension, a name with several dots, a foreign
//!   sidecar, a broken one). After every scripted step the sidecar files' bytes (line ends as `\n`), the status
//!   lines, the error and the catalogue (rows, history, undo preferences) are the oracle's.
//! * `golden/app/xmp_map` (the same generator): the oracle's own `MapView` over photos with every kind of
//!   position; the markers (photo, place, tooltip) and the land polygons of the Rust map are the oracle's.
//! * The window: the commands end to end, the map in Korean and English (snapshots `xmp-map-*`), double-click,
//!   wheel and drag on the map.
//!
//! Every test works on temporary folders and generated pictures; no library of the owner is opened and no photo
//! file is written.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::library_column::LAUNCH_DISABLED;
use grainy_app::commands::xmp::{apply_sidecars, read_sidecars, rows_of, sidecar_path, write_sidecars, READ_LABEL};
use grainy_app::commands::Invocation;
use grainy_app::dialogs::map::{
    gps_points, python_float, split_at_cusps, triangulate, MapDialog, MapPoint, MapView, World,
};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::Document;
use grainy_app::GrainyApp;
use grainy_catalog::rusqlite::Connection;
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case(name: &str) -> Value {
    Case::open("app", name).json().clone()
}

// ---- the oracle's library ---------------------------------------------------------------------------------------

fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// Builds the library of the generator's `BUILD` list with the Rust catalogue. Returns the library folder, the photo
/// folder and the document over it.
fn golden_library(name: &str) -> (PathBuf, PathBuf, CatalogDocument) {
    let meta = case("xmp");
    let dir = common::temp_dir(name);
    let photos = dir.join("Photos");
    std::fs::create_dir_all(&photos).unwrap();
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog =
        Catalog::with_sources(dir.join("Library"), Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    for op in meta["build"].as_array().unwrap() {
        let op = op.as_array().unwrap();
        match op[0].as_str().unwrap() {
            "add" => {
                let path = photos.join(op[1].as_str().unwrap());
                std::fs::write(&path, b"").unwrap();
                catalog.add(path.to_str().unwrap(), None, Some(&op[2]), true).unwrap();
            }
            "update" => catalog.update(op[1].as_i64().unwrap(), &object(&op[2])).unwrap(),
            "meta" => {
                // Pairs, in the oracle's insertion order.
                let values: Map<String, Value> = op[2]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pair| (pair[0].as_str().unwrap().to_owned(), pair[1].clone()))
                    .collect();
                catalog.set_user_metadata(op[1].as_i64().unwrap(), &values).unwrap()
            }
            "edit" => {
                let id = op[1].as_i64().unwrap();
                let mut settings = object(&catalog.photo(id).unwrap().unwrap()["settings"]);
                settings.extend(object(&op[3]));
                let settings = grainy_catalog::settings(&Value::Object(settings)).unwrap();
                catalog.edit(id, &settings, op[2].as_str().unwrap()).unwrap();
            }
            "copy" => {
                catalog.virtual_copy(op[1].as_i64().unwrap()).unwrap();
            }
            other => panic!("{other}"),
        }
    }
    let doc = CatalogDocument::with_catalog(catalog).unwrap();
    (dir, photos, doc)
}

fn sparse(settings: &Value) -> Value {
    let defaults = grainy_core::defaults();
    let map = settings.as_object().expect("settings are an object");
    let changed: Map<String, Value> = map
        .iter()
        .filter(|(k, v)| defaults.0.get(*k).is_none_or(|d| !grainy_core::settings::equivalent(v, d)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    json!({"keys": map.len(), "changed": changed})
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// The catalogue as the generator's `project` stores it.
fn project(library: &Path) -> Value {
    let db = Connection::open(library.join("catalog.sqlite")).unwrap();
    let mut photos = Map::new();
    let mut statement =
        db.prepare("SELECT id,edits,extras,rating,flag,label,keywords FROM photos ORDER BY id").unwrap();
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (id, edits, extras, rating, flag, label, keywords) = row.unwrap();
        let extras = extras.filter(|e| !e.is_empty()).map_or(json!({}), |e| parse(&e));
        photos.insert(
            id.to_string(),
            json!({"settings": sparse(&parse(&edits)), "extras": extras, "rating": rating, "flag": flag,
                   "label": label, "keywords": keywords}),
        );
    }
    let mut statement = db.prepare("SELECT id,photo_id,label,edits,created FROM history ORDER BY id").unwrap();
    let history: Vec<Value> = statement
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "photo": r.get::<_, i64>(1)?, "label": r.get::<_, String>(2)?,
                      "settings": sparse(&parse(&r.get::<_, String>(3)?)), "created": r.get::<_, String>(4)?}))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut preferences = Map::new();
    let mut statement = db.prepare("SELECT key,value FROM preferences ORDER BY key").unwrap();
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap();
    for row in rows {
        let (key, text) = row.unwrap();
        let mut value = parse(&text);
        if key.starts_with("undo:") && value.get("undo").is_some() {
            let list = |name: &str| -> Value {
                Value::Array(value[name].as_array().map(|a| a.iter().map(sparse).collect()).unwrap_or_default())
            };
            value = json!({"undo": list("undo"), "redo": list("redo")});
        }
        preferences.insert(key, value);
    }
    json!({"photos": photos, "history": history, "preferences": preferences})
}

fn equal(actual: &Value, expected: &Value, at: &str) {
    if actual == expected {
        return;
    }
    match (actual, expected) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                equal(a.get(k).unwrap_or(&Value::Null), v, &format!("{at}.{k}"));
            }
            assert_eq!(
                a.len(),
                b.len(),
                "{at}: keys {:?} vs {:?}",
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>()
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                equal(x, y, &format!("{at}[{i}]"));
            }
            assert_eq!(a.len(), b.len(), "{at}: length");
        }
        _ => panic!("{at}: {actual} != {expected}"),
    }
}

/// Every `.xmp` file of the folder, `\r\n` as `\n`, by name.
fn sidecars(folder: &Path) -> Value {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "xmp") {
            let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap().replace("\r\n", "\n");
            files.insert(path.file_name().unwrap().to_string_lossy().into_owned(), Value::String(text));
        }
    }
    Value::Object(files.into_iter().collect())
}

#[test]
fn the_library_starts_as_the_oracles() {
    let meta = case("xmp");
    let (dir, _, doc) = golden_library("xmp-start");
    equal(&project(&dir.join("Library")), &meta["start"], "start");
    let rows: Vec<Value> = doc
        .metadata_catalog()
        .unwrap()
        .photos()
        .unwrap()
        .iter()
        .map(|p| {
            let mut row = Map::new();
            for key in ["id", "name", "rating", "flag", "label", "keywords", "virtual_source"] {
                row.insert(key.into(), p[key].clone());
            }
            let extras = p["extras"].as_str().filter(|e| !e.is_empty()).map_or(json!({}), parse);
            row.insert("extras".into(), extras);
            Value::Object(row)
        })
        .collect();
    equal(&Value::Array(rows), &meta["start_rows"], "rows");
}

/// Runs one step of the generator's script as the commands do (without the job): returns the error text and the
/// status lines.
fn step(doc: &mut CatalogDocument, photos: &Path, step: &Value) -> (Option<String>, Vec<String>) {
    let args = step.as_array().unwrap();
    let ids = |v: &Value| -> Vec<i64> { v.as_array().unwrap().iter().map(|i| i.as_i64().unwrap()).collect() };
    let mut status = Vec::new();
    let mut error = None;
    match args[0].as_str().unwrap() {
        "write" => {
            let rows = rows_of(doc.metadata_catalog().unwrap(), &ids(&args[1])).unwrap();
            match write_sidecars(&rows) {
                Ok(_) => status.push(grainy_app::text::tr("XMP에 별점·키워드·Grainy 보정값을 저장했습니다.")),
                Err(message) => error = Some(message),
            }
        }
        "read" => {
            let rows = rows_of(doc.metadata_catalog().unwrap(), &ids(&args[1])).unwrap();
            error = apply_sidecars(doc, read_sidecars(&rows)).err();
        }
        "update" => doc.metadata_catalog().unwrap().update(args[1].as_i64().unwrap(), &object(&args[2])).unwrap(),
        "file" => std::fs::write(photos.join(args[1].as_str().unwrap()), args[2].as_str().unwrap()).unwrap(),
        "delete" => {
            let _ = std::fs::remove_file(photos.join(args[1].as_str().unwrap()));
        }
        other => panic!("{other}"),
    }
    (error, status)
}

#[test]
fn a_scripted_session_writes_and_reads_what_the_oracle_does() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case("xmp");
    let (dir, photos, mut doc) = golden_library("xmp-script");
    let mut errors = 0;
    for expected in meta["steps"].as_array().unwrap() {
        let at = expected["step"].to_string();
        let at = at.chars().take(60).collect::<String>();
        let (error, status) = step(&mut doc, &photos, &expected["step"]);
        match expected["error"].as_str() {
            Some(oracle) => {
                errors += 1;
                let error = error.unwrap_or_else(|| panic!("the oracle refuses {at}: {oracle}"));
                let (_, text) = oracle.split_once(": ").unwrap();
                assert_eq!(error, text, "{at}");
            }
            None => assert_eq!(error, None, "{at}"),
        }
        let oracle_status: Vec<String> =
            expected["messages"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect();
        assert_eq!(status, oracle_status, "{at}");
        equal(&sidecars(&photos), &expected["files"], &format!("files after {at}"));
        equal(&project(&dir.join("Library")), &expected["catalog"], &format!("catalogue after {at}"));
    }
    assert_eq!(errors, 2, "the broken sidecar is refused twice");
}

#[test]
fn only_sidecars_are_written_and_the_photos_are_left_alone() {
    let (_, photos, mut doc) = golden_library("xmp-only-sidecars");
    let before: Vec<(String, u64)> = std::fs::read_dir(&photos)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), e.metadata().unwrap().len())
        })
        .collect();
    let rows = rows_of(doc.metadata_catalog().unwrap(), &[1, 2, 3, 4, 5, 6, 7]).unwrap();
    assert_eq!(write_sidecars(&rows).unwrap().len(), 7);
    let mut after: Vec<String> =
        std::fs::read_dir(&photos).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    after.sort();
    let mut expected: Vec<String> = before.iter().map(|(n, _)| n.clone()).collect();
    expected.extend(
        ["a.xmp", "a.luma-copy-2.xmp", "b.xmp", "c.name.xmp", "noext.xmp", "d.xmp", "d.luma-copy-7.xmp"]
            .map(str::to_owned),
    );
    expected.sort();
    assert_eq!(after, expected, "no temporary file stays and nothing else appears");
    for (name, size) in before {
        assert_eq!(std::fs::metadata(photos.join(&name)).unwrap().len(), size, "{name} is not written");
    }
    // A read of a photo that has no sidecar changes nothing.
    std::fs::remove_file(photos.join("b.xmp")).unwrap();
    let rows = rows_of(doc.metadata_catalog().unwrap(), &[3]).unwrap();
    let reads = read_sidecars(&rows);
    assert!(reads.iter().all(|(_, read)| read.is_none()));
    assert_eq!(apply_sidecars(&mut doc, reads), Ok(0));
}

#[test]
fn a_read_is_one_history_row_and_one_undo_step_of_its_own() {
    let _turn = common::language_lock();
    let (dir, photos, mut doc) = golden_library("xmp-history");
    let before = project(&dir.join("Library"));
    std::fs::write(
        photos.join("b.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:luma="https://luma.local/ns/1.0/"><rdf:RDF><rdf:Description luma:Settings='{"exposure": 0.75}'/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let rows = rows_of(doc.metadata_catalog().unwrap(), &[3]).unwrap();
    assert_eq!(apply_sidecars(&mut doc, read_sidecars(&rows)), Ok(1));
    let after = project(&dir.join("Library"));
    let rows_before = before["history"].as_array().unwrap().len();
    let history = after["history"].as_array().unwrap();
    assert_eq!(history.len(), rows_before + 1);
    assert_eq!(history.last().unwrap()["label"], READ_LABEL);
    assert_eq!(after["preferences"]["undo:3"]["undo"].as_array().unwrap().len(), 1);
    assert_eq!(after["photos"]["3"]["settings"]["changed"]["exposure"], 0.75);
    // The same file again changes nothing more.
    let rows = rows_of(doc.metadata_catalog().unwrap(), &[3]).unwrap();
    assert_eq!(apply_sidecars(&mut doc, read_sidecars(&rows)), Ok(1));
    assert_eq!(project(&dir.join("Library"))["history"].as_array().unwrap().len(), rows_before + 1);
}

#[test]
fn the_sidecar_is_named_as_the_oracle_names_it() {
    let row = |path: &str, source: Value| json!({"id": 9, "path": path, "virtual_source": source});
    let name = |v: Value| sidecar_path(&v).file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name(row("D:/a/b.jpg", json!(null))), "b.xmp");
    assert_eq!(name(row("D:/a/b.jpg", json!(3))), "b.luma-copy-9.xmp");
    assert_eq!(name(row("D:/a/b.c.NEF", json!(null))), "b.c.xmp");
    assert_eq!(name(row("D:/a/noext", json!(null))), "noext.xmp");
    assert_eq!(name(row("D:/a/noext", json!(1))), "noext.luma-copy-9.xmp");
    assert_eq!(name(row("D:/a/.hidden", json!(null))), ".hidden.xmp");
    assert_eq!(name(row("D:/a/한글 사진.png", json!(null))), "한글 사진.xmp");
}

// ---- the map: photos and land -----------------------------------------------------------------------------------

#[test]
fn the_markers_are_the_oracles() {
    let meta = case("xmp_map");
    let dir = common::temp_dir("xmp-map-markers");
    let catalog = Catalog::new(dir.join("Library")).unwrap();
    let mut list = Vec::new();
    for photo in meta["photos"].as_array().unwrap() {
        let name = photo["name"].as_str().unwrap();
        let id = catalog.add(dir.join(name).to_str().unwrap(), None, Some(&photo["info"]), true).unwrap();
        let mut fields = Map::new();
        fields.insert("extras".into(), Value::String(photo["extras"].to_string()));
        catalog.update(id, &fields).unwrap();
        list.push((id, name.to_owned()));
    }
    let points = gps_points(&catalog, &list).unwrap();
    let oracle = meta["markers"].as_array().unwrap();
    assert_eq!(points.len(), oracle.len());
    for (point, expected) in points.iter().zip(oracle) {
        assert_eq!(point.id as i64, expected["id"].as_i64().unwrap());
        assert_eq!(point.tip(), expected["tip"].as_str().unwrap(), "tooltip of {}", point.id);
        // The marker is placed at (longitude, -latitude): the oracle's scene position.
        assert!((point.lon - expected["x"].as_f64().unwrap()).abs() < 1e-12, "x of {}", point.id);
        assert!((-point.lat - expected["y"].as_f64().unwrap()).abs() < 1e-12, "y of {}", point.id);
    }
    // A photo that is not in the catalogue is left out.
    assert!(gps_points(&catalog, &[(999, "x".into())]).unwrap().is_empty());
}

#[test]
fn python_float_reads_what_float_reads() {
    for (value, expected) in [
        (json!("1_0"), Some(10.0)),
        (json!(" 5 "), Some(5.0)),
        (json!("1e1"), Some(10.0)),
        (json!(true), Some(1.0)),
        (json!("1__0"), None),
        (json!("_1"), None),
        (json!("1_"), None),
        (json!("0x10"), None),
        (json!(""), None),
        (json!(null), None),
        (json!([1]), None),
    ] {
        assert_eq!(python_float(&value), expected, "{value}");
    }
}

fn ring_area(ring: &[[f32; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1])
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

fn triangles_area(triangles: &[[[f32; 2]; 3]]) -> f64 {
    triangles.iter().map(|t| ring_area(t)).sum()
}

#[test]
fn the_triangulation_covers_exactly_the_ring() {
    // A square, a concave "L" and a comb, in both orientations.
    let square = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
    let ell = [[0.0, 0.0], [6.0, 0.0], [6.0, 2.0], [2.0, 2.0], [2.0, 6.0], [0.0, 6.0]];
    let comb = [
        [0.0, 0.0],
        [9.0, 0.0],
        [9.0, 5.0],
        [7.0, 5.0],
        [7.0, 1.0],
        [5.0, 1.0],
        [5.0, 5.0],
        [3.0, 5.0],
        [3.0, 1.0],
        [1.0, 1.0],
        [1.0, 5.0],
        [0.0, 5.0],
    ];
    for ring in [&square[..], &ell[..], &comb[..]] {
        let mut reversed = ring.to_vec();
        reversed.reverse();
        for r in [ring.to_vec(), reversed] {
            let triangles = triangulate(&r);
            assert_eq!(triangles.len(), r.len() - 2);
            assert!((triangles_area(&triangles) - ring_area(&r)).abs() < 1e-9);
        }
    }
    // A closed ring (first point repeated) is the same ring.
    let mut closed = square.to_vec();
    closed.push(square[0]);
    assert_eq!(triangulate(&closed).len(), 2);
    assert!(triangulate(&square[..2]).is_empty());
}

#[test]
fn a_ring_that_turns_back_is_stroked_in_runs() {
    // A square: one closed path. A square with a spike that goes out and comes back along the same line: the
    // stroke is cut at the tip, so that no miter spike is drawn across the map.
    let square = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
    assert_eq!(split_at_cusps(&square), vec![(square.to_vec(), true)]);
    let spiked = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [100.0, 4.0], [4.0, 4.0 + 1e-3 * 0.0], [0.0, 4.0]];
    let runs = split_at_cusps(&spiked);
    assert!(runs.iter().all(|(_, closed)| !closed));
    assert!(runs
        .iter()
        .any(|(points, _)| points.first() == Some(&[100.0, 4.0]) || points.last() == Some(&[100.0, 4.0])));
}

#[test]
fn the_world_has_the_oracles_polygons_and_every_ring_is_filled() {
    let meta = case("xmp_map");
    let world = World::load();
    // The oracle makes one path for each polygon; the outer ring of each is the first of its rings.
    assert_eq!(world.polygons as i64, meta["polygons"].as_i64().unwrap());
    assert!(world.rings.len() >= world.polygons);
    let mut filled = 0.0;
    let mut wanted = 0.0;
    for ring in &world.rings {
        wanted += ring_area(ring);
    }
    for triangle in world.land.iter().chain(&world.holes) {
        filled += ring_area(triangle);
    }
    assert!((filled - wanted).abs() / wanted < 1e-4, "the triangles cover the rings: {filled} vs {wanted}");
    // The same file parsed twice gives the same map.
    assert_eq!(
        World::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/world.geojson")).unwrap())
            .land
            .len(),
        world.land.len()
    );
    assert!(World::parse("not json").rings.is_empty());
}

// ---- the map: the view ------------------------------------------------------------------------------------------

#[test]
fn the_view_fits_zooms_and_pans_like_the_graphics_view() {
    let size = egui::vec2(1000.0, 600.0);
    let fit = MapView::fit(size);
    // 360 x 180 degrees in 996 x 596 pixels (Qt keeps 2 pixels around): the width decides.
    assert!((fit.scale - 996.0 / 360.0).abs() < 1e-5);
    assert_eq!(fit.center, egui::Vec2::ZERO);

    // Wheel: a factor of 1.3 under the pointer.
    let mut view = fit;
    let offset = egui::vec2(120.0, -40.0);
    let under = view.center + offset / view.scale;
    view.zoom(size, offset, true);
    assert!((view.scale - fit.scale * 1.3).abs() < 1e-5);
    let after = view.center + offset / view.scale;
    assert!((after - under).length() < 1e-3, "the point under the pointer stays");
    view.zoom(size, offset, false);
    assert!((view.scale - fit.scale).abs() < 1e-4);

    // The scale stays between 0.5 and 2000.
    let mut view = MapView { scale: 1900.0, center: egui::Vec2::ZERO };
    view.zoom(size, egui::Vec2::ZERO, true);
    assert_eq!(view.scale, 1900.0, "a step past 2000 is refused");
    let mut view = MapView { scale: 0.6, center: egui::Vec2::ZERO };
    view.zoom(size, egui::Vec2::ZERO, false);
    assert_eq!(view.scale, 0.6, "a step below 0.5 is refused");

    // Pan: a smaller scene than the view stays centred; a larger one cannot leave the scene rectangle.
    let mut view = fit;
    view.pan(size, egui::vec2(500.0, 500.0));
    assert_eq!(view.center.x, 0.0, "the map is narrower than the view: centred");
    let mut view = MapView { scale: 20.0, center: egui::Vec2::ZERO };
    view.pan(size, egui::vec2(100000.0, -100000.0));
    assert!(view.center.x.abs() <= 180.0 - 1000.0 / 40.0 + 1e-3);
    assert!(view.center.y.abs() <= 90.0 - 600.0 / 40.0 + 1e-3);
}

fn point(id: u64, lat: f64, lon: f64) -> MapPoint {
    MapPoint { id, name: format!("photo {id}"), lat, lon }
}

#[test]
fn double_click_wheel_and_drag_on_the_map() {
    let points = vec![point(7, 37.5, 127.0), point(8, 35.7, 139.7), point(9, 48.86, 2.35)];
    let dialog = MapDialog::new(points, World::load());
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(0.02)
        .build_ui_state(
            |ui, dialog: &mut MapDialog| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
                dialog.interact(ui, rect);
            },
            dialog,
        );
    harness.run_steps(2);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
    let at = |harness: &Harness<'_, MapDialog>, n: usize| {
        let p = &harness.state().points[n];
        harness.state().view.to_screen(rect, [p.lon as f32, -p.lat as f32])
    };
    // A double-click beside a marker opens nothing.
    let seoul = at(&harness, 0);
    let click = |harness: &Harness<'_, MapDialog>, pos: egui::Pos2, times: usize| {
        harness.event(egui::Event::PointerMoved(pos));
        for _ in 0..times {
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
    };
    click(&harness, seoul + egui::vec2(20.0, 0.0), 2);
    harness.run_steps(30);
    assert_eq!(harness.state().opened, None);
    // On the marker: a single click does nothing, a double-click opens the photo.
    click(&harness, seoul, 1);
    harness.run_steps(30);
    assert_eq!(harness.state().opened, None);
    // Two clicks in two frames (frames are 20 ms long here: inside the double-click time).
    click(&harness, seoul, 1);
    harness.run_steps(2);
    click(&harness, seoul, 1);
    harness.run_steps(2);
    assert_eq!(harness.state_mut().opened.take(), Some(7));

    // The wheel zooms by 1.3 for each notch, under the pointer.
    let before = harness.state().view;
    let pos = seoul;
    harness.event(egui::Event::PointerMoved(pos));
    harness.run_steps(1);
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, 1.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(1);
    let view = harness.state().view;
    assert!((view.scale - before.scale * 1.3).abs() < 1e-4, "{} vs {}", view.scale, before.scale);
    let moved = at(&harness, 0);
    // (The scroll range of the scene rectangle holds the map back a few pixels near its top edge, as in Qt.)
    assert!((moved - pos).length() < 10.0, "Seoul stays under the pointer: {moved:?} vs {pos:?}");

    // Dragging with the primary button moves the map (zoomed in, so that it can).
    for _ in 0..6 {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(1);
    }
    let before = harness.state().view;
    let start = egui::pos2(500.0, 300.0);
    harness.event(egui::Event::PointerMoved(start));
    harness.run_steps(1);
    harness.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(1);
    harness.event(egui::Event::PointerMoved(start + egui::vec2(60.0, 0.0)));
    harness.run_steps(2);
    harness.event(egui::Event::PointerButton {
        pos: start + egui::vec2(60.0, 0.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(1);
    let view = harness.state().view;
    assert!((before.center.x - view.center.x - 60.0 / before.scale).abs() < 1e-3, "{before:?} {view:?}");
}

// ---- the window --------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library in a temporary folder with three generated photos, the first open.
fn library_harness<'a>(name: &str) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = common::temp_dir(name);
    let photos = [dir.join("garden.png"), dir.join("garden two.png"), dir.join("garden three.png")];
    common::save_png(&common::garden(2, [1.0; 3], false), &photos[0]);
    common::save_png(&common::garden(2, [0.8, 1.0, 1.1], true), &photos[1]);
    common::save_png(&common::garden(2, [1.2, 1.0, 0.8], false), &photos[2]);
    let library = dir.join("Library");
    let data = dir.join("data");
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app.open_paths(photos.to_vec());
        app
    });
    common::snapshot::settle(&mut harness);
    (harness, dir)
}

fn run(harness: &mut Harness<'_, GrainyApp>, id: &str) -> bool {
    harness.state_mut().cx.run_with(id, &Invocation::default())
}

/// Runs frames until the job of `kind` has answered (its `done` has run).
fn wait_job(harness: &mut Harness<'_, GrainyApp>, kind: &str) {
    let started = Instant::now();
    harness.run_steps(1);
    while harness.state().cx.jobs.running(kind) {
        assert!(started.elapsed() < Duration::from_secs(120), "{kind} did not finish");
        std::thread::sleep(Duration::from_millis(5));
        harness.run_steps(1);
    }
}

fn reader(dir: &Path) -> Catalog {
    Catalog::open_reader(dir.join("Library")).unwrap()
}

const SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:luma="https://luma.local/ns/1.0/"><rdf:RDF><rdf:Description xmp:Rating="4" xmp:Label="빨강" luma:Settings='{"exposure": 1.25}' luma:Metadata='{"title": "From sidecar"}'><dc:subject><rdf:Bag><rdf:li>a</rdf:li><rdf:li>b</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    LAUNCH_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let (mut harness, dir) = library_harness(&format!("xmp-window-{tag}"));
    let text = |source: &str| grainy_app::text::tr(source);

    // The photo menu's two items and the main menu's run the commands (no empty command is left).
    for id in ["metadata.write_xmp", "metadata.read_xmp", "extras.map"] {
        assert!(grainy_app::commands::enabled(&harness.state().cx, id), "{id}");
    }

    // XMP 저장: the open photo's sidecar is written next to the file, the photo itself is not touched.
    let photo = harness.state().cx.doc.current().unwrap().path.clone();
    let bytes = std::fs::read(&photo).unwrap();
    assert!(run(&mut harness, "metadata.write_xmp"));
    wait_job(&mut harness, "xmp_write");
    assert_eq!(harness.state().status(), text("XMP에 별점·키워드·Grainy 보정값을 저장했습니다."));
    let sidecar = photo.with_extension("xmp");
    let written = std::fs::read_to_string(&sidecar).unwrap();
    assert!(written.contains("luma:Settings") && written.contains("xmp:Rating"), "{written}");
    assert_eq!(std::fs::read(&photo).unwrap(), bytes, "the photo file is not written");
    let count = std::fs::read_dir(&dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "xmp"))
        .count();
    assert_eq!(count, 1, "only the selection gets a sidecar");

    // XMP 불러오기: a sidecar made elsewhere brings its settings, rating, label, keywords and metadata in.
    std::fs::write(&sidecar, SIDECAR).unwrap();
    assert!(run(&mut harness, "metadata.read_xmp"));
    wait_job(&mut harness, "xmp_read");
    common::snapshot::settle(&mut harness);
    let id = harness.state().cx.doc.current_id() as i64;
    let row = reader(&dir).photo(id).unwrap().unwrap();
    assert_eq!(row["rating"], 4);
    assert_eq!(row["label"], "빨강");
    assert_eq!(row["keywords"], "a, b");
    assert_eq!(row["user_metadata"]["title"], "From sidecar");
    assert_eq!(row["settings"]["exposure"], 1.25);
    assert_eq!(harness.state().cx.settings().f32("exposure"), 1.25, "the open photo shows the new settings");
    // One undo step brings the old settings back.
    assert!(run(&mut harness, "develop.undo"));
    common::snapshot::settle(&mut harness);
    assert_eq!(harness.state().cx.settings().f32("exposure"), 0.0);

    // 추가 도구 → GPS 지도: three photos with positions, the map in a window.
    {
        let catalog = Catalog::new(dir.join("Library")).unwrap();
        for (id, lat, lon) in [(1, 37.5665, 126.978), (2, 35.6762, 139.6503), (3, -33.8688, 151.2093)] {
            let mut values = Map::new();
            values.insert("latitude".into(), json!(lat));
            values.insert("longitude".into(), json!(lon));
            catalog.set_user_metadata(id, &values).unwrap();
        }
        let mut values = Map::new();
        values.insert("latitude".into(), json!(48.8566));
        values.insert("longitude".into(), json!(2.3522));
        catalog.set_user_metadata(2, &values).unwrap();
    }
    assert!(run(&mut harness, "extras.map"));
    wait_job(&mut harness, "gps_map");
    harness.run_steps(3);
    assert!(harness.state().dialog_open("map"));
    common::snapshot::open_folders(harness.state_mut(), &[]);
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(3);
    common::snapshot::save(&mut harness, &format!("xmp-map-{tag}"));
    harness.state_mut().close_dialog("map");
    harness.run_steps(2);
    assert!(!harness.state().dialog_open("map"));

    LAUNCH_DISABLED.store(false, std::sync::atomic::Ordering::Relaxed);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_window_runs_the_xmp_and_map_commands_korean() {
    flow(Language::Korean, "ko");
}

#[test]
fn the_window_runs_the_xmp_and_map_commands_english() {
    flow(Language::English, "en");
}

#[test]
fn the_commands_need_a_library_open_for_writing() {
    let _turn = common::language_lock();
    let mut cx = grainy_app::AppContext::new(
        &egui::Context::default(),
        Box::new(grainy_app::render::TestCard::default()),
        Box::new(grainy_app::session::Session::default()),
    );
    for id in ["metadata.write_xmp", "metadata.read_xmp"] {
        assert!(!grainy_app::commands::enabled(&cx, id), "{id}");
        assert!(!cx.run(id), "{id}");
    }
    // The map opens on a session too (empty: a session has no catalogue rows).
    assert!(cx.run("extras.map"));
    assert!(cx.dialog_open("map"));
}
