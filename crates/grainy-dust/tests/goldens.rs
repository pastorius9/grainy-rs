use grainy_core::{Image, Plane};
use grainy_dust as dust;
use grainy_testkit::{assert_close, Case};
use ndarray::{Array1, Array2};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

fn integer(v: &Value) -> usize {
    v.as_u64().unwrap() as usize
}
fn decoded<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap()
}
fn equal(a: Value, b: &Value, label: &str) {
    assert!(grainy_catalog::json::equivalent(&a, b), "{label}:\nactual {}\nexpected {}", a, b);
    println!("{label}: equal (tolerance 0)");
}
fn checksum(a: &Image) -> String {
    let mut hash = Sha256::new();
    for &v in a {
        hash.update(v.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[test]
fn detect_goldens() {
    let scenes = Case::open("dust", "scenes");
    let case = Case::open("dust", "detect");
    for (i, c) in case.json()["cases"].as_array().unwrap().iter().enumerate() {
        let input: Plane = scenes.array(c["name"].as_str().unwrap());
        let options = decoded(&c["options"]);
        let mut events = Vec::new();
        let mut progress = |a, b| events.push([a, b]);
        let out =
            dust::detect(&input, &options, None, Some(&mut progress), integer(&c["tile_size"]), integer(&c["limit"]))
                .unwrap();
        let mut value = serde_json::to_value(out).unwrap();
        value.as_object_mut().unwrap().remove("seconds");
        equal(value, &c["result"], &format!("detect/{i}/{}", c["name"]));
        equal(json!(events), &c["progress"], &format!("progress/{i}"));
    }
}
#[test]
fn candidates_goldens() {
    let scenes = Case::open("dust", "scenes");
    let case = Case::open("dust", "candidates");
    for (i, c) in case.json()["cases"].as_array().unwrap().iter().enumerate() {
        let input: Plane = scenes.array(c["name"].as_str().unwrap());
        let options = decoded(&c["options"]);
        let mut events = Vec::new();
        let mut progress = |a, b| events.push([a, b]);
        let f = if c["function"] == "soft" { dust::soft::candidates } else { dust::scratches::candidates };
        let out = f(&input, &options, None, Some(&mut progress), integer(&c["tile_size"])).unwrap();
        equal(json!(out), &c["output"], &format!("candidates/{i}"));
        equal(json!(events), &c["progress"], &format!("candidate_progress/{i}"));
    }
}
#[test]
fn statistics_goldens() {
    let c = Case::open("dust", "helpers");
    let fractions: Vec<f64> = decoded(&c.json()["fractions"]);
    assert_eq!(c.json()["percentile_dtype"], "float32");
    for (i, v) in c.json()["vectors"].as_array().unwrap().iter().enumerate() {
        let a: Array1<f32> = c.array(&format!("vector_{i}"));
        let a = a.as_slice().unwrap();
        equal(json!(dust::median(a).unwrap()), &v["median"], &format!("median/{i}"));
        equal(json!(dust::quantiles(a, &fractions).unwrap()), &v["quantiles"], &format!("quantiles/{i}"));
        let p = dust::quantile_plan(a.len(), &fractions).unwrap();
        equal(
            json!([p.kth, p.lower, p.upper, p.backward, p.inverse, p.weight]),
            &v["plan"],
            &format!("quantile_plan/{i}"),
        );
    }
    let stats: Array2<i32> = c.array("stats");
    for (i, r) in c.json()["components"].as_array().unwrap().iter().enumerate() {
        let args: Vec<usize> = decoded(&r["args"]);
        let out = dust::component_indices(
            &stats,
            args[0] as f64,
            args[1] as f64,
            (args[2], args[3]),
            [args[4], args[5], args[6], args[7]],
        )
        .unwrap();
        equal(json!(out), &r["output"], &format!("component_indices/{i}"));
    }
    let scenes = Case::open("dust", "scenes");
    for (i, r) in c.json()["repeated"].as_array().unwrap().iter().enumerate() {
        let input = scenes.array(r["name"].as_str().unwrap());
        let out = dust::repeated_detail(&input, decoded(&r["rect"]), r["reduce"].as_bool().unwrap()).unwrap();
        equal(json!(out), &r["output"], &format!("repeated_detail/{i}"));
    }
}
#[test]
fn shapes_goldens() {
    let c = Case::open("dust", "shapes");
    let shape = decoded(&c.json()["shape"]);
    for (i, r) in c.json()["records"].as_array().unwrap().iter().enumerate() {
        let spot = &r["spot"];
        equal(json!(dust::spot_bounds(spot, shape).unwrap()), &r["spot_bounds"], &format!("spot_bounds/{i}"));
        let parts = dust::shapes::ellipses(spot, shape).unwrap();
        equal(json!(parts), &r["parts"], &format!("ellipses/{i}"));
        equal(json!(dust::shapes::bounds(&parts, shape).unwrap()), &r["bounds"], &format!("bounds/{i}"));
        let mut mask = Array2::zeros(shape);
        dust::shapes::paint(&mut mask, &parts, 0, 0).unwrap();
        assert_eq!(mask, c.array::<u8, _>(&format!("mask_{i}")));
        let mut crop = Array2::from_elem((21, 27), 17);
        dust::shapes::paint(&mut crop, &parts, 13, 9).unwrap();
        assert_eq!(crop, c.array::<u8, _>(&format!("crop_{i}")));
        println!("paint/{i}: equal (tolerance 0)");
    }
    for (i, r) in c.json()["manual"].as_array().unwrap().iter().enumerate() {
        let out = dust::shapes::manual(&decoded::<Vec<[f64; 2]>>(&r["points"]), shape, r["diameter"].as_f64().unwrap())
            .unwrap();
        equal(out, &r["spot"], &format!("manual/{i}"));
    }
    equal(
        dust::shapes::make_spot(
            &[[5., 10., 2., 3.], [14.5, 18.5, 4., 2.]],
            shape,
            json!({"polarity":"manual","score":0}).as_object().unwrap().clone(),
        )
        .unwrap(),
        &c.json()["made"],
        "make_spot",
    );
}
#[test]
fn cover_merge_goldens() {
    let c = Case::open("dust", "cover");
    for (i, r) in c.json()["cases"].as_array().unwrap().iter().enumerate() {
        let mask = c.array(&format!("mask_{i}"));
        let distance = c.array(&format!("distance_{i}"));
        let origin: Vec<usize> = decoded(&r["origin"]);
        equal(
            json!(dust::scratches::cover(&mask, &distance, origin[0], origin[1], None).unwrap()),
            &r["parts"],
            &format!("cover/{i}"),
        );
    }
    let c = Case::open("dust", "merge");
    let r = c.json();
    equal(
        json!(dust::soft::merge(
            r["existing"].as_array().unwrap(),
            r["additional"].as_array().unwrap(),
            integer(&r["width"]),
            integer(&r["height"]),
            integer(&r["maximum"]),
            None
        )
        .unwrap()),
        &r["output"],
        "merge",
    );
}
#[test]
fn visualize_goldens() {
    let scenes = Case::open("dust", "scenes");
    let c = Case::open("dust", "visualize");
    for (i, r) in c.json()["cases"].as_array().unwrap().iter().enumerate() {
        let a = scenes.array(r["name"].as_str().unwrap());
        let out = dust::visualize(&a, r["strength"].as_f64().unwrap()).unwrap();
        let expected = c.array(r["array"].as_str().unwrap());
        assert_close(&out, &expected, 0., &format!("visualize/{i}"));
    }
}
#[test]
fn repair_goldens() {
    let c = Case::open("dust", "repair");
    for (i, r) in c.json()["cases"].as_array().unwrap().iter().enumerate() {
        let input: Image = c.array(r.get("input").and_then(Value::as_str).unwrap_or("input"));
        let method = r["method"].as_str().unwrap();
        let spots = r["spots"].as_array().unwrap();
        let expected = c.array(r["array"].as_str().unwrap());
        let out = dust::repair(&input, spots, None, method).unwrap();
        let tolerance = if method == "smooth" { 0. } else { 2e-4 };
        assert_close(&out, &expected, tolerance, &format!("repair/{i}/{method}"));
        if method == "smooth" {
            assert_eq!(checksum(&out), r["checksum"]);
        }
        let mut in_place = input.clone();
        dust::repair_in_place(&mut in_place, spots, None, method).unwrap();
        assert_eq!(out, in_place, "copy=False {i}");
    }
}

#[test]
fn detected_repair_goldens() {
    let scenes = Case::open("dust", "scenes");
    let c = Case::open("dust", "repair_detected");
    for (i, r) in c.json()["cases"].as_array().unwrap().iter().enumerate() {
        let gray: Plane = scenes.array(r["name"].as_str().unwrap());
        let (h, w) = gray.dim();
        let input = Image::from_shape_fn((h, w, 3), |(y, x, _)| gray[(y, x)]);
        let spots = r["spots"].as_array().unwrap();
        let method = r["method"].as_str().unwrap();
        let out = dust::repair(&input, spots, None, method).unwrap();
        let mut mask = Array2::zeros((h, w));
        for spot in spots {
            dust::shapes::paint(&mut mask, &dust::shapes::ellipses(spot, (h, w)).unwrap(), 0, 0).unwrap();
        }
        let mut pixels = Vec::new();
        for ((y, x), &v) in mask.indexed_iter() {
            for channel in 0..3 {
                if v > 0 {
                    pixels.push(out[(y, x, channel)]);
                } else {
                    assert_eq!(out[(y, x, channel)].to_bits(), input[(y, x, channel)].to_bits());
                }
            }
        }
        let actual = Array2::from_shape_vec((pixels.len() / 3, 3), pixels).unwrap();
        let expected = c.array(r["array"].as_str().unwrap());
        assert_close(
            &actual,
            &expected,
            if method == "smooth" { 0. } else { 2e-4 },
            &format!("detected_repair/{i}/{method}"),
        );
        if method == "smooth" || spots.is_empty() {
            assert_eq!(checksum(&out), r["checksum"]);
        }
        let mut in_place = input.clone();
        dust::repair_in_place(&mut in_place, spots, None, method).unwrap();
        assert_eq!(in_place, out);
    }
}
#[test]
fn texture_goldens() {
    let c = Case::open("dust", "texture");
    for (i, r) in c.json()["cases"].as_array().unwrap().iter().enumerate() {
        let original = c.array(&format!("original_{i}"));
        let smooth: Image = c.array(&format!("smooth_{i}"));
        let mask = c.array(&format!("mask_{i}"));
        let expected = c.array(&format!("output_{i}"));
        let mut out = smooth.clone();
        let stats = dust::texture::restore(&original, &mut out, &mask, None).unwrap();
        equal(json!(stats), &r["stats"], &format!("texture_stats/{i}"));
        assert_close(&out, &expected, 2e-4, &format!("texture/{i}"));
        let prepared =
            dust::texture::prepare(&original, &smooth, &mask, &decoded::<Vec<(usize, usize)>>(&r["blocks"]), None)
                .unwrap();
        assert_eq!(prepared.stats().transferred_pixels, integer(&r["transferred"]));
        assert_eq!(prepared.stats().fallback_pixels, integer(&r["fallback"]));
        let mut other = smooth.clone();
        prepared.apply(&mut other).unwrap();
        assert_eq!(other, out);
    }
}
#[test]
fn cancellation_and_threads() {
    let c = Case::open("dust", "cancel");
    let input = Case::open("dust", "scenes").array("grain");
    let options = dust::Options { detailed: true, soft: true, scratches: true, ..Default::default() };
    for (i, r) in c.json()["cases"].as_array().unwrap().iter().enumerate() {
        let calls = AtomicUsize::new(0);
        let cancel = || calls.fetch_add(1, Ordering::Relaxed) + 1 >= integer(&r["after"]);
        let mut events = Vec::new();
        let mut progress = |a, b| events.push([a, b]);
        let result = dust::detect(&input, &options, Some(&cancel), Some(&mut progress), 48, 500);
        assert!(matches!(result, Err(dust::Error::Cancelled)));
        assert_eq!(calls.load(Ordering::Relaxed), integer(&r["calls"]));
        equal(json!(events), &r["progress"], &format!("cancel/{i}"));
    }
    assert!(dust::checkpoint(Some(&|| true)).is_err());
    assert!(dust::checkpoint(None).is_ok());
    let c = Case::open("dust", "repair");
    let input: Image = c.array("input");
    let spots = c.json()["cases"][4]["spots"].as_array().unwrap();
    for method in ["smooth", "texture"] {
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| dust::repair(&input, spots, None, method).unwrap())
        };
        assert_eq!(run(1), run(4));
        println!("threads/{method}: equal (tolerance 0)");
    }
}
#[test]
fn store_goldens() {
    let c = Case::open("dust", "store");
    let folder = grainy_testkit::root().join("target/dust-roundtrip");
    std::fs::create_dir_all(&folder).unwrap();
    for (i, v) in c.json()["values"].as_array().unwrap().iter().enumerate() {
        let payload: Array1<u8> = c.array(&format!("payload_{i}"));
        let out: Value = serde_json::from_str(&dust::store::unpack(payload.as_slice().unwrap()).unwrap()).unwrap();
        equal(out, v, &format!("unpack/{i}"));
        let rust = dust::store::pack(v).unwrap();
        equal(serde_json::from_str(&dust::store::unpack(&rust).unwrap()).unwrap(), v, &format!("pack/{i}"));
        std::fs::write(folder.join(format!("payload_{i}.zlib")), rust).unwrap();
        std::fs::write(folder.join(format!("payload_{i}.json")), serde_json::to_vec(v).unwrap()).unwrap();
    }
    for (i, v) in c.json()["targets"].as_array().unwrap().iter().enumerate() {
        equal(
            json!(dust::store::target_key(&v["snapshot"], v["scope"].as_str().unwrap()).unwrap()),
            &v["key"],
            &format!("target_key/{i}"),
        );
    }
    let payload = dust::store::pack(&json!({"state":"scanning"})).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&dust::store::unpack(&payload).unwrap()).unwrap(),
        json!({"state":"pending"})
    );
    for p in [
        vec![],
        vec![1, 2, 3],
        payload[..payload.len() - 1].to_vec(),
        [payload.clone(), vec![0]].concat(),
        [payload.clone(), payload.clone()].concat(),
    ] {
        assert!(dust::store::unpack(&p).is_err());
    }
}
#[test]
fn catalog_goldens_and_roundtrip() {
    let c = Case::open("dust", "batch");
    let root = grainy_testkit::root();
    let catalog = grainy_catalog::Catalog::open_reader(root.join("golden/dust/batch")).unwrap();
    for (i, v) in c.json()["scopes"].as_array().unwrap().iter().enumerate() {
        equal(
            dust::batch::capture_scope_ids(
                &catalog,
                v["current"].as_i64(),
                &decoded::<Vec<i64>>(&v["selection"]),
                v["folder"].as_str(),
            )
            .unwrap(),
            &v["output"],
            &format!("capture_scope_ids/{i}"),
        );
    }
    for (i, v) in c.json()["records"].as_array().unwrap().iter().enumerate() {
        equal(
            dust::batch::capture_record(&catalog, v["id"].as_i64().unwrap()).unwrap(),
            v,
            &format!("capture_record/{i}"),
        );
        assert!(dust::batch::verify_source(&catalog, v).is_err());
    }
    assert!(dust::batch::capture_record(&catalog, 4).is_err());
    assert!(dust::batch::capture_record(&catalog, 999).is_err());
    let folder = root.join("target/dust-roundtrip");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("source.jpg");
    std::fs::write(&source, b"synthetic source bytes, no image decoding").unwrap();
    let live = grainy_catalog::Catalog::new(folder.join("catalog")).unwrap();
    live.connection().execute("DELETE FROM photos", []).unwrap();
    let path = source.to_string_lossy().to_string();
    live.add(&path, None, Some(&json!({})), true).unwrap();
    let record = dust::batch::capture_record(&live, 1).unwrap();
    dust::batch::verify_source(&live, &record).unwrap();
    let scope = dust::batch::capture_scope_ids(&live, Some(1), &[2, 1, 2, 999], None).unwrap();
    std::fs::write(folder.join("batch.json"), serde_json::to_vec(&json!({"scope":scope,"record":record})).unwrap())
        .unwrap();
    let mut wrong = record.clone();
    wrong["settings"]["exposure"] = json!(2);
    assert!(dust::batch::verify_source(&live, &wrong).is_err());
    wrong = record.clone();
    wrong["path"] = json!("missing");
    assert!(dust::batch::verify_source(&live, &wrong).is_err());
    wrong = record.clone();
    wrong["fingerprint"]["size"] = json!(0);
    assert!(dust::batch::verify_source(&live, &wrong).is_err());
}
#[test]
fn invalid_inputs() {
    let golden = Case::open("dust", "validation");
    for (i, c) in golden.json()["cases"].as_array().unwrap().iter().enumerate() {
        let args = &c["args"];
        let rejected = match c["function"].as_str().unwrap() {
            "options" => serde_json::from_value::<dust::Options>(args.clone()).map_or(true, |o| o.validate().is_err()),
            "spot" => dust::shapes::ellipses(args, (10, 10)).is_err(),
            "manual" => dust::shapes::manual(
                &decoded::<Vec<[f64; 2]>>(&args["points"]),
                (10, 10),
                args["diameter"].as_f64().unwrap(),
            )
            .is_err(),
            "visualize" => dust::visualize(&Array2::zeros((2, 2)), args.as_f64().unwrap()).is_err(),
            "detect_tile" => {
                dust::detect(&Array2::zeros((2, 2)), &Default::default(), None, None, integer(args), 500).is_err()
            }
            "detect_limit" => {
                dust::detect(&Array2::zeros((2, 2)), &Default::default(), None, None, 640, integer(args)).is_err()
            }
            "repair" => dust::repair(&Image::zeros((2, 2, 3)), &[], None, args.as_str().unwrap()).is_err(),
            _ => panic!("unknown golden operation"),
        };
        equal(json!(rejected), &c["rejected"], &format!("validation/{i}"));
    }
    for i in 0..integer(&golden.json()["bad_payloads"]) {
        let data: Array1<u8> = golden.array(&format!("bad_payload_{i}"));
        assert!(dust::store::unpack(data.as_slice().unwrap()).is_err());
        println!("bad_payload/{i}: equal (tolerance 0)");
    }
    for options in [
        dust::Options { sensitivity: 0, ..Default::default() },
        dust::Options { minimum: 1, ..Default::default() },
        dust::Options { maximum: 161, ..Default::default() },
        dust::Options { polarity: "none".into(), ..Default::default() },
        dust::Options { scratch_width: 41, ..Default::default() },
    ] {
        assert!(options.validate().is_err());
    }
    assert!(dust::median(&[]).is_err());
    assert!(dust::quantiles(&[f32::NAN], &[0.5]).is_err());
    assert!(dust::quantile_plan(0, &[0.5]).is_err());
    assert!(dust::quantile_plan(1, &[2.]).is_err());
    assert!(dust::detect(&Array2::zeros((0, 1)), &Default::default(), None, None, 640, 500).is_err());
    assert!(dust::visualize(&Array2::zeros((2, 2)), 0.).is_err());
    for spot in [
        json!({}),
        json!({"x":2,"y":0,"rx":0.1,"ry":0.1}),
        json!({"x":0,"y":0,"rx":0,"ry":0.1}),
        json!({"x":0,"y":0,"rx":0.1,"ry":0.1,"parts":[]}),
    ] {
        assert!(dust::shapes::ellipses(&spot, (10, 10)).is_err());
    }
    assert!(dust::shapes::manual(&[], (10, 10), 3.).is_err());
    assert!(dust::shapes::manual(&[[0., 0.], [1., 1.]], (usize::MAX, usize::MAX), 1.).is_err());
    assert!(dust::soft::merge(&[], &[], usize::MAX, 1, 1, None).is_err());
    assert!(dust::shapes::manual(&[[1.1, 0.]], (10, 10), 3.).is_err());
    assert!(dust::shapes::bounds(&[], (10, 10)).is_err());
    let a = Image::zeros((8, 8, 3));
    assert!(dust::repair(&a, &[], None, "bad").is_err());
    assert!(dust::repair(&a, &[], Some(&|| true), "smooth").is_err());
    assert!(dust::texture::restore(&a, &mut a.clone(), &Array2::zeros((9, 8)), None).is_err());
    let c = Case::open("dust", "repair");
    let source: Image = c.array("input");
    let spots = c.json()["cases"][4]["spots"].as_array().unwrap();
    for method in ["smooth", "texture"] {
        let calls = AtomicUsize::new(0);
        let cancel = || calls.fetch_add(1, Ordering::Relaxed) >= spots.len() * 2 + 2;
        assert!(matches!(dust::repair(&source, spots, Some(&cancel), method), Err(dust::Error::Cancelled)));
    }
}
