use grainy_core::Settings;
use grainy_geometry::{
    lens::{lens_geometry, lens_shading, modifier_request},
    LensModel,
};
use grainy_lens::{Database, ProfileModel};
use grainy_testkit::{max_abs_diff, Case};
use ndarray::{Array3, Array4, Array5, Axis};
use serde_json::Value;

fn indices(v: &Value) -> Vec<usize> {
    v.as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as usize).collect()
}
fn select(a: &Array3<f32>, ys: &[usize], xs: &[usize]) -> Array3<f32> {
    a.select(Axis(0), ys).select(Axis(1), xs)
}
fn input(w: usize, h: usize) -> Array3<f32> {
    Array3::from_shape_fn((h, w, 3), |(y, x, c)| match c {
        0 => x as f32 / (w - 1).max(1) as f32,
        1 => y as f32 / (h - 1).max(1) as f32,
        _ => 0.2 + 0.4 * x as f32 / (w - 1).max(1) as f32 + 0.2 * y as f32 / (h - 1).max(1) as f32,
    })
}

#[test]
fn maps_shading_remap_and_develop() {
    let index = Case::open("lens", "index");
    let mut failures = Vec::new();
    for name in index.json()["cases"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        let case = Case::open("lens", name);
        let j = case.json();
        let s = Settings::from_value(j["settings"].clone()).unwrap();
        let m = ProfileModel::from_settings(&s).unwrap();
        let w = j["width"].as_u64().unwrap() as usize;
        let h = j["height"].as_u64().unwrap() as usize;
        let xs = indices(&j["xs"]);
        let ys = indices(&j["ys"]);
        let req = modifier_request(&s, h, w, false, Some(&m)).unwrap();
        let mut ge = 0f32;
        let mut se = 0f32;
        if let Some(r) = req {
            let step = if w * h * 24 > grainy_geometry::lens::MAP_BUDGET { 128 } else { h };
            let mut g = Array3::zeros((ys.len(), xs.len(), 2));
            let mut sub = Array4::zeros((ys.len(), xs.len(), 3, 2));
            for start in (0..h).step_by(step) {
                let rows = step.min(h - start);
                let gg = m.geometry_coordinates(&r, start, rows).unwrap();
                let ss = m.subpixel_coordinates(&r, start, rows).unwrap();
                assert_eq!(gg.is_some(), case.has("geometry"), "{name} geometry availability");
                assert_eq!(ss.is_some(), case.has("subpixel"), "{name} subpixel availability");
                for (yi, &y) in ys.iter().enumerate().filter(|(_, y)| **y >= start && **y < start + rows) {
                    for (xi, &x) in xs.iter().enumerate() {
                        for k in 0..2 {
                            if let Some(a) = &gg {
                                g[[yi, xi, k]] = a[[y - start, x, k]];
                            }
                        }
                        for c in 0..3 {
                            for k in 0..2 {
                                if let Some(a) = &ss {
                                    sub[[yi, xi, c, k]] = a[[y - start, x, c, k]];
                                }
                            }
                        }
                    }
                }
            }
            if case.has("geometry") {
                let expected: Array3<f32> = case.array("geometry");
                let (error, index) = max_abs_diff(&g, &expected);
                ge = error;
                if error > 2e-4 {
                    println!(
                        "Target miss {name} geom at x={} y={} component={} actual={} expected={}",
                        xs[(index / 2) % xs.len()],
                        ys[index / (xs.len() * 2)],
                        index % 2,
                        g.iter().nth(index).unwrap(),
                        expected.iter().nth(index).unwrap()
                    );
                }
            }
            if case.has("subpixel") {
                se = max_abs_diff(&sub, &case.array("subpixel")).0;
            }
        } else {
            assert!(!case.has("geometry") && !case.has("subpixel"));
        }
        let mut color = Array3::ones((h, w, 3));
        let cr = modifier_request(&s, h, w, true, Some(&m)).unwrap();
        let changed = cr.as_ref().is_some_and(|r| m.apply_color(r, &mut color).unwrap());
        assert_eq!(changed, j["color_changed"].as_bool().unwrap(), "{name} color availability");
        let ce = max_abs_diff(&select(&color, &ys, &xs), &case.array("gain")).0;
        let mut pe = 0f32;
        let mut de = 0f32;
        let mut ve = 0f32;
        if j["pixels"] == true {
            let a = j["input_name"].as_str().map_or_else(|| input(w, h), grainy_testkit::input);
            let o = j["orientation"].as_i64().unwrap();
            let inverse = match o {
                6 => 8,
                8 => 6,
                _ => o,
            };
            let a =
                grainy_geometry::lens::sensor_view(a.view(), &serde_json::json!({"orientation":inverse})).unwrap().0;
            let shade = lens_shading(&a, &s, Some(&m)).unwrap();
            let geo = lens_geometry(&a, &s, Some(&m)).unwrap();
            let shade = if w * h * 24 > grainy_geometry::lens::MAP_BUDGET {
                select(&shade, &ys, &xs)
            } else {
                shade.into_owned()
            };
            let geo =
                if w * h * 24 > grainy_geometry::lens::MAP_BUDGET { select(&geo, &ys, &xs) } else { geo.into_owned() };
            ve = max_abs_diff(&shade, &case.array("shading")).0;
            pe = max_abs_diff(&geo, &case.array("remap")).0;
            if case.has("develop") {
                let options = grainy_engine::Options {
                    externals: grainy_engine::Externals { lens: Some(&m), ..Default::default() },
                    ..Default::default()
                };
                let out = grainy_engine::develop(&a, &s.0, &options).unwrap();
                let out = if w * h * 24 > grainy_geometry::lens::MAP_BUDGET { select(&out, &ys, &xs) } else { out };
                de = max_abs_diff(&out, &case.array("develop")).0;
            }
        }
        println!("{name}: geometry={ge:.7e} subpixel={se:.7e} gain={ce:.7e} shading={ve:.7e} remap={pe:.7e} develop={de:.7e}");
        // No named-case exceptions remain. The remaining float32 normalization
        // differences also occur with an explicit scale (see scale_audit).
        let coordinate_limit = 2e-4;
        let develop_limit = 1e-5;
        println!("{name}: tolerances coordinates={coordinate_limit:.7e} gain/shading=2e-4 remap=1e-4 develop={develop_limit:.7e}");
        if ge > coordinate_limit || se > coordinate_limit || ce > 2e-4 || ve > 2e-4 || pe > 1e-4 || de > develop_limit {
            failures.push(name.to_owned());
        }
    }
    assert!(failures.is_empty(), "out-of-tolerance cases: {failures:?}");
}

#[test]
fn automatic_profile_maps() {
    use grainy_geometry::lens::{LensFlags, LensRequest};
    let case = Case::open("lens", "scales");
    let j = case.json();
    let geometry: Array4<f32> = case.array("geometry");
    let subpixel: Array5<f32> = case.array("subpixel");
    let mut maximum = 0f32;
    let mut count = 0;
    for (i, row) in j["cases"].as_array().unwrap().iter().enumerate().filter(|(_, r)| r["suite"] == "profiles") {
        let number = |key: &str| row[key].as_f64().unwrap();
        let w = number("width") as usize;
        let h = number("height") as usize;
        let flags = number("flags") as u32;
        let profile = serde_json::json!({"xml": j["xmls"][number("xml_index") as usize]});
        let model = ProfileModel::from_profile(&profile).unwrap();
        let request = LensRequest {
            profile,
            width: w,
            height: h,
            crop: number("crop"),
            focal: number("focal"),
            aperture: number("aperture"),
            distance: number("distance"),
            scale: 0.,
            flags: LensFlags {
                distortion: flags & 8 != 0,
                tca: flags & 1 != 0,
                vignetting: flags & 2 != 0,
                scale: true,
            },
        };
        let mut error = 0f32;
        for (yi, y) in [0, h / 2, h - 1].into_iter().enumerate() {
            let g = model.geometry_coordinates(&request, y, 1).unwrap();
            let s = model.subpixel_coordinates(&request, y, 1).unwrap();
            assert_eq!(g.is_some(), row["geometry"].as_bool().unwrap());
            assert_eq!(s.is_some(), row["subpixel"].as_bool().unwrap());
            for (xi, x) in [0, w / 2, w - 1].into_iter().enumerate() {
                for k in 0..2 {
                    if let Some(g) = &g {
                        error = error.max((g[[0, x, k]] - geometry[[i, yi, xi, k]]).abs());
                    }
                    if let Some(s) = &s {
                        for c in 0..3 {
                            error = error.max((s[[0, x, c, k]] - subpixel[[i, yi, xi, c, k]]).abs());
                        }
                    }
                }
            }
        }
        let tolerance = if w.max(h) > 1024 { 2e-4 } else { 1e-4 };
        println!("automatic profile map {}: maximum {error:.7e}, tolerance {tolerance:.7e}", row["name"]);
        assert!(error <= tolerance, "{}: {error} > {tolerance}", row["name"]);
        maximum = maximum.max(error);
        count += 1;
    }
    println!("automatic profile maps: {count} cases, maximum {maximum:.7e}");
}

#[test]
fn database_and_profiles() {
    let case = Case::open("lens", "database");
    let j = case.json();
    let db = Database::from_folder(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data")).unwrap();
    assert_eq!(serde_json::to_value(db.cameras()).unwrap(), j["cameras"]);
    let records: Vec<Value> = j["lenses"].as_array().unwrap().iter().map(|v| v["record"].clone()).collect();
    assert_eq!(serde_json::to_value(db.lens_records()).unwrap(), Value::Array(records));
    assert_eq!(
        serde_json::to_value(db.compatible_lenses(None).iter().map(|r| &r.id).collect::<Vec<_>>()).unwrap(),
        j["compatible_none"]
    );
    for entry in j["descriptions"].as_array().unwrap() {
        assert_eq!(grainy_lens::description(&entry["profile"]).unwrap(), entry["description"].as_str().unwrap());
    }
    for k in j["keys"].as_array().unwrap() {
        assert_eq!(grainy_lens::key(k["text"].as_str().unwrap()), k["key"].as_str().unwrap());
        assert_eq!(grainy_lens::model_key(k["text"].as_str().unwrap(), "Canon"), k["model_key"].as_str().unwrap());
    }
    for entry in j["lenses"].as_array().unwrap() {
        let record: grainy_lens::LensRecord = serde_json::from_value(entry["record"].clone()).unwrap();
        let single = Database::from_xml(&record.xml, &record.file).unwrap();
        assert_eq!(single.lens_records(), std::slice::from_ref(&record));
        let lens = grainy_lens::profile_lens(&record).unwrap();
        let expected = &entry["lens"];
        let got = serde_json::to_value(&lens).unwrap();
        for k in ["maker", "model", "mounts", "crop_factor", "min_focal", "max_focal", "min_aperture", "max_aperture"] {
            let e = if expected[k].is_null() { serde_json::json!(0.) } else { expected[k].clone() };
            assert_eq!(got[k], e, "{} {k}", lens.model);
        }
        for kind in ["distortion", "tca", "vignetting"] {
            let actual = got[kind].as_array().unwrap();
            let expected = expected[kind].as_array().unwrap();
            assert_eq!(actual.len(), expected.len(), "{} {kind} count", lens.model);
            for (a, b) in actual.iter().zip(expected) {
                for (key, b) in b.as_object().unwrap() {
                    assert_eq!(&a[key], b, "{} {kind} {key}", lens.model);
                }
            }
        }
    }
    for c in j["compatible"].as_array().unwrap() {
        let camera = db.cameras().iter().find(|cam| cam.model == c["camera"]).unwrap();
        let actual: Vec<_> = db.compatible_lenses(Some(camera)).into_iter().map(|r| r.id.clone()).collect();
        assert_eq!(serde_json::to_value(actual).unwrap(), c["ids"], "compatibility {}", camera.model);
    }
    for entry in j["matches"].as_array().unwrap() {
        let m = db.match_metadata(&entry["metadata"]);
        assert_eq!(serde_json::to_value(m.camera.map(|c| &c.model)).unwrap(), entry["camera"]);
        assert_eq!(
            serde_json::to_value(m.candidates.iter().map(|r| &r.id).collect::<Vec<_>>()).unwrap(),
            entry["candidates"]
        );
        assert_eq!(m.reason, entry["reason"].as_str().unwrap());
        let (p, why) = db.automatic_profile(&entry["metadata"]);
        assert_eq!(p.clone().unwrap_or(Value::Null), entry["profile"], "profile {}", entry["metadata"]);
        assert_eq!(why, entry["profile_reason"].as_str().unwrap());
        assert_eq!(
            grainy_lens::description(&p.unwrap_or(Value::Null)).unwrap(),
            entry["description"].as_str().unwrap()
        );
    }
    for entry in j["retarget"].as_array().unwrap() {
        let actual = db.retarget(&entry["settings"], &entry["metadata"]).unwrap();
        assert_eq!(actual, entry["result"]);
        assert_eq!(grainy_lens::description(&actual["lensfun"]).unwrap(), entry["description"].as_str().unwrap());
    }
}

#[test]
fn matching_ambiguities_and_mount_boundaries() {
    let case = Case::open("lens", "matching");
    let j = case.json();
    let db = Database::from_xml(j["xml"].as_str().unwrap(), "synthetic.xml").unwrap();
    assert_eq!(serde_json::to_value(db.lens_records()).unwrap(), j["records"]);
    for row in j["compatible"].as_array().unwrap() {
        let camera = db.cameras().iter().find(|c| c.model == row["camera"] && c.variant == row["variant"]).unwrap();
        let ids: Vec<_> = db.compatible_lenses(Some(camera)).iter().map(|r| r.id.clone()).collect();
        assert_eq!(serde_json::to_value(ids).unwrap(), row["ids"], "{}", camera.model);
    }
    for row in j["matches"].as_array().unwrap() {
        let m = db.match_metadata(&row["metadata"]);
        assert_eq!(serde_json::to_value(m.camera.map(|c| &c.model)).unwrap(), row["camera"]);
        assert_eq!(
            serde_json::to_value(m.candidates.iter().map(|r| &r.id).collect::<Vec<_>>()).unwrap(),
            row["candidates"]
        );
        assert_eq!(m.reason, row["reason"].as_str().unwrap());
        let (p, why) = db.automatic_profile(&row["metadata"]);
        assert_eq!(p.unwrap_or(Value::Null), row["profile"]);
        assert_eq!(why, row["profile_reason"].as_str().unwrap());
    }
}

#[test]
fn coefficient_interpolation() {
    let case = Case::open("lens", "interpolation");
    let j = case.json();
    let mut failures = Vec::new();
    let mut maximum = 0f32;
    for (i, row) in j["cases"].as_array().unwrap().iter().enumerate() {
        let lens =
            grainy_lens::saved_lens(j["xmls"][row["xml_index"].as_u64().unwrap() as usize].as_str().unwrap()).unwrap();
        let focal = row["focal"].as_f64().unwrap() as f32;
        let aperture = row["aperture"].as_f64().unwrap() as f32;
        let distance = row["distance"].as_f64().unwrap() as f32;
        for (name, actual) in [
            ("distortion", lens.interpolate_distortion(focal)),
            ("tca", lens.interpolate_tca(focal)),
            ("vignetting", lens.interpolate_vignetting(focal, aperture, distance)),
        ] {
            let expected = &row[name];
            if actual.is_none() != expected.is_null() {
                failures.push(format!("{i} {name} availability"));
                continue;
            }
            if let Some(actual) = actual {
                if actual.model != expected["model"].as_str().unwrap() {
                    failures.push(format!("{i} {name} model {} != {}", actual.model, expected["model"]));
                    continue;
                }
                let terms = expected["terms"].as_array().unwrap();
                let mut error = 0f32;
                for (a, b) in actual.terms.iter().zip(terms) {
                    error = error.max((a - b.as_f64().unwrap() as f32).abs());
                }
                maximum = maximum.max(error);
                if error > 1e-5 {
                    failures.push(format!("{i} {name} error {error} ({focal}, {aperture}, {distance})"));
                }
            }
        }
    }
    println!("coefficient interpolation: maximum {maximum:.7e}, tolerance 1e-5");
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn validation_matches_oracle() {
    let case = Case::open("lens", "validation");
    let j = case.json();
    let record = serde_json::from_value(j["record"].clone()).unwrap();
    for entry in j["profiles"].as_array().unwrap() {
        let c = &entry["conditions"];
        let conditions = grainy_lens::Conditions {
            crop: c["crop"].as_f64().unwrap(),
            focal: c["focal"].as_f64().unwrap(),
            aperture: c["aperture"].as_f64().unwrap(),
            distance: c["distance"].as_f64().unwrap(),
            orientation: c["orientation"].as_i64().unwrap(),
            camera: c["camera"].as_str().unwrap().into(),
        };
        match grainy_lens::make_profile(&record, &conditions) {
            Ok(p) => assert_eq!(p, entry["profile"]),
            Err(e) => assert_eq!(e.to_string(), entry["error"].as_str().unwrap()),
        };
    }
    for entry in j["xml"].as_array().unwrap() {
        let xml = if entry["oversize"] == true { "x".repeat(256001) } else { entry["xml"].as_str().unwrap().into() };
        assert_eq!(grainy_lens::saved_lens(&xml).unwrap_err().to_string(), entry["error"].as_str().unwrap());
    }
}

#[test]
fn defaults_are_exact_noops() {
    let c = Case::open("lens", "defaults");
    let input = grainy_testkit::input("colours");
    for row in c.json()["cases"].as_array().unwrap() {
        let s = Settings::from_value(row["settings"].clone()).unwrap();
        let model = ProfileModel::from_settings(&s).ok();
        let lens = model.as_ref().map(|m| m as &dyn LensModel);
        assert_eq!(
            lens_geometry(&input, &s, lens).unwrap().as_ref() == input,
            row["geometry_identity"].as_bool().unwrap()
        );
        assert_eq!(
            lens_shading(&input, &s, lens).unwrap().as_ref() == input,
            row["shading_identity"].as_bool().unwrap()
        );
    }
}

#[test]
fn parallelism_and_bad_requests() {
    let case = Case::open("lens", "m0_large");
    let s = Settings::from_value(case.json()["settings"].clone()).unwrap();
    let model = ProfileModel::from_settings(&s).unwrap();
    let request = modifier_request(&s, 1000, 1450, false, Some(&model)).unwrap().unwrap();
    let run = |threads| {
        rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap().install(|| {
            let coordinates = model.subpixel_coordinates(&request, 128, 128).unwrap().unwrap();
            let mut color = Array3::ones((1000, 1450, 3));
            let cr = modifier_request(&s, 1000, 1450, true, Some(&model)).unwrap().unwrap();
            model.apply_color(&cr, &mut color).unwrap();
            (coordinates, color)
        })
    };
    assert_eq!(run(1), run(4));
    assert!(model.geometry_coordinates(&request, 999, 2).is_err());
    assert!(model.subpixel_coordinates(&request, usize::MAX, 1).is_err());
    for value in [f64::NAN, f64::INFINITY, 0., -1., 1e-100, 1e100] {
        let mut bad = request.clone();
        bad.crop = value;
        assert!(model.geometry_coordinates(&bad, 0, 1).is_err());
    }
    let mut bad = request.clone();
    bad.width = usize::MAX;
    assert!(model.geometry_coordinates(&bad, 0, 1).is_err());
    let mut bad = request.clone();
    bad.profile = Value::Null;
    assert!(model.geometry_coordinates(&bad, 0, 1).is_err());
    assert!(model.apply_color(&request, &mut Array3::ones((1, 1, 3))).is_err());
    assert!(ProfileModel::from_profile(&Value::Null).is_err());
    let invalid_xml="<lensdatabase><lens><maker>Test</maker><model>Test</model><mount>Test</mount><aspect-ratio>1:0</aspect-ratio></lens></lensdatabase>";
    assert!(Database::from_xml(invalid_xml, "test.xml").is_err());
    let unicode_xml="<lensdatabase><lens><maker>Test</maker><model>試35mm</model><mount>Test</mount><cropfactor>1</cropfactor></lens></lensdatabase>";
    assert!(Database::from_xml(unicode_xml, "test.xml").is_ok());
}
