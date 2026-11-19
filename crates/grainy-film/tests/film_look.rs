//! Golden tests of the film looks against luma/film_look.py (tools/golden/gen_film.py).
mod common;

use common::{a32, a64, close32, close64, every, num};
use grainy_film::{self as film, Found, WorkingSpace};
use grainy_testkit::{input, Case};
use serde_json::Value;

const SPACES: [(&str, WorkingSpace); 2] = [("sRGB", WorkingSpace::Srgb), ("ProPhoto", WorkingSpace::ProPhoto)];

fn flat3(m: &[[f64; 3]]) -> Vec<f64> {
    m.iter().flatten().copied().collect()
}

#[test]
fn numpy_float32_exp_and_log() {
    let case = Case::open("film", "npmath");
    for (input, output, f, what) in [
        ("x", "log", film::testing::np_log as fn(f32) -> f32, "np.log float32"),
        ("y", "exp", film::testing::np_exp, "np.exp float32"),
    ] {
        let x = a32(&case, input);
        let expected = a32(&case, output);
        let mut differ = 0;
        for (a, e) in x.iter().zip(expected.iter()) {
            let ours = f(*a);
            if ours.to_bits() != e.to_bits() && !(ours.is_nan() && e.is_nan()) {
                if differ < 5 {
                    println!("{what}({a:e}) = {ours:e}, numpy {e:e}");
                }
                differ += 1;
            }
        }
        println!("{what}: {differ} of {} values differ from numpy's bits", x.len());
        assert_eq!(differ, 0, "{what}");
    }
}

#[test]
fn spectral_model() {
    let case = Case::open("film", "spectral");
    let c = film::colorimetry();
    close64(&c.wavelengths, &a64(&case, "wavelengths"), 0.0, "wavelengths");
    close64(&flat3(&c.cmf), &a64(&case, "cmf"), 0.0, "cmf");
    close64(&flat3(&c.daylight), &a64(&case, "daylight"), 0.0, "daylight components");
    close64(&flat3(&c.basis), &a64(&case, "basis"), 0.0, "basis");
    for kelvin in [4000, 5003, 5503, 6504, 7000, 7001, 12000] {
        close64(&film::daylight(f64::from(kelvin)), &a64(&case, &format!("daylight_{kelvin}")), 1e-10, "daylight");
    }
    for kelvin in [2000, 3200, 3999] {
        close64(&film::blackbody(f64::from(kelvin)), &a64(&case, &format!("blackbody_{kelvin}")), 1e-12, "blackbody");
    }
    for name in case.json()["lights"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        close64(&film::light(name).unwrap(), &a64(&case, &format!("light_{name}")), 1e-10, &format!("light {name}"));
    }
    assert!(film::light("D").is_err() && film::light("sunset").is_err());
    close64(&flat3(&film::adaptation([0.9642, 1.0, 0.8251])), &a64(&case, "adaptation_d50"), 1e-14, "adaptation D50");
    close64(
        &flat3(&film::adaptation([1.0985 * 0.7, 0.7, 0.3558 * 0.7])),
        &a64(&case, "adaptation_a"),
        1e-14,
        "adaptation A",
    );
    let pts: Value = serde_json::json!([[600, 1.0], [420, 0.5], [700, 2.0]]);
    close64(&film::sampled(&pts, Some(0.0)), &a64(&case, "sampled_fill"), 1e-14, "sampled fill");
    close64(&film::sampled(&pts, None), &a64(&case, "sampled_ends"), 1e-14, "sampled ends");
    let pts: Value = serde_json::json!([[500, 1.0], [450, 0.5]]);
    close64(&film::sampled(&pts, Some(f64::NAN)), &a64(&case, "sampled_nan"), 1e-14, "sampled nan");
    close64(&flat3(&film::prophoto()), &a64(&case, "prophoto"), 1e-14, "_prophoto");
    for (name, space) in SPACES {
        let (into, out) = film::matrices(space);
        close64(&flat3(&into), &a64(&case, &format!("into_{name}")), 1e-14, "into");
        close64(&flat3(&out), &a64(&case, &format!("out_{name}")), 1e-13, "out");
    }
}

#[test]
fn stock_intermediates() {
    for id in [
        "kodak-portra-400",
        "fuji-provia-100f",
        "kodak-kodachrome-64",
        "ilford-hp5-plus",
        "cinestill-800t",
        "kodak-vision3-500t",
    ] {
        let case = Case::open("film", &format!("stock_{id}"));
        let meta = case.json();
        let s = film::stock(id).unwrap();
        println!("-- {id}");
        assert_eq!(s.kind, meta["kind"].as_str().unwrap());
        assert_eq!(s.channels as u64, meta["channels"].as_u64().unwrap());
        close64(&[s.reference], &ndarray::arr1(&[num(&meta["ref"])]).into_dyn(), 1e-9, "ref");
        close64(&[s.dx], &ndarray::arr1(&[num(&meta["dx"])]).into_dyn(), 1e-15, "dx");
        close64(&flat3(&s.matrix), &a64(&case, "matrix"), 1e-13, "exposure matrix (layer sensitivities)");
        close32(&s.grid, &a32(&case, "grid"), 0.0, "grid");
        let tables: Vec<f32> = s.tables.iter().flatten().copied().collect();
        close32(&tables, &a32(&case, "tables"), 2e-5, "characteristic curves (tables)");
        let slopes: Vec<f32> = s.slopes.iter().flatten().copied().collect();
        close32(&slopes, &a32(&case, "slopes"), 2e-5, "slopes");
        if case.has("lights") {
            close64(s.lights.as_ref().unwrap(), &a64(&case, "lights"), 1e-7, "printer lights (neutral calibration)");
        }
        if s.channels == 1 {
            let white = s.paper_white.unwrap();
            close64(&[white], &ndarray::arr1(&[num(&meta["paper_white"])]).into_dyn(), 0.0, "paper white");
        } else {
            let d = s.dyes.as_ref().unwrap();
            close64(&flat3(&d.spectra), &a64(&case, "dyes_spectra"), 1e-13, "dye spectra");
            close64(&d.base, &a64(&case, "dyes_base"), 1e-14, "dye base");
            close64(&d.base_bands, &a64(&case, "dyes_base_bands"), 1e-14, "dye base bands");
            close64(&flat3(&d.unmix), &a64(&case, "dyes_unmix"), 1e-13, "dye unmix");
            close64(&d.view, &a64(&case, "dyes_view"), 1e-10, "view light");
            close64(&flat3(&d.weights), &a64(&case, "dyes_weights"), 1e-10, "weights");
            close64(&flat3(&d.adapt), &a64(&case, "dyes_adapt"), 1e-14, "adapt");
            close64(&d.levels, &a64(&case, "dyes_levels"), 0.0, "levels");
            close64(&flat3(&d.strengths), &a64(&case, "dyes_strengths"), 1e-6, "dye strengths (neutral calibration)");
            close32(&s.lo, &a32(&case, "lo"), 2e-5, "cube lo");
            close32(&s.hi, &a32(&case, "hi"), 2e-5, "cube hi");
            close32(&s.step, &a32(&case, "step"), 1e-7, "cube step");
            close32(&s.inverse_step, &a32(&case, "inverse_step"), 1e-2, "cube inverse step");
            let n = film::NODES;
            let mut sub = Vec::new();
            for a in (0..n).step_by(4) {
                for b in (0..n).step_by(4) {
                    for c in (0..n).step_by(4) {
                        sub.extend(s.cube[(a * n + b) * n + c]);
                    }
                }
            }
            close32(&sub, &a32(&case, "cube_sub"), 2e-5, "density cube");
            let probe = a64(&case, "colour_probe");
            let mut colours = Vec::new();
            for row in probe.outer_iter() {
                colours.extend(d.colour([row[0], row[1], row[2]]));
            }
            close64(&colours, &a64(&case, "colour_of_probe"), 1e-9, "Dyes.colour");
        }
        let p = input("patches");
        let mut dens = Vec::new();
        let mut xyz = Vec::new();
        for y in (8..64).step_by(16) {
            for x in (8..96).step_by(16) {
                let rgb = [p[[y, x, 0]], p[[y, x, 1]], p[[y, x, 2]]];
                dens.extend(&s.densities(rgb)[..s.channels]);
                xyz.extend(s.xyz(rgb));
            }
        }
        close32(&dens, &a32(&case, "densities_patches"), 2e-5, "densities");
        if s.channels == 1 {
            close32(&xyz, &a32(&case, "xyz_patches"), 2e-5, "xyz");
        } else {
            // colour stocks interpolate the cube in float64; Stock::xyz returns the float32 the render casts to
            let xyz: Vec<f64> = xyz.iter().map(|&v| f64::from(v)).collect();
            close64(&xyz, &a64(&case, "xyz_patches"), 1e-6, "xyz");
        }
    }
}

#[test]
fn render_every_stock_on_patches() {
    let p = input("patches");
    let mut worst = (0f32, String::new());
    for e in film::catalog() {
        let case = Case::open("film", &format!("render_patches_{}", e.id));
        for (name, space) in SPACES {
            let out = film::render(&p, &e.id, space).unwrap();
            let expected = a32(&case, name);
            let err = close32(&every(&out, 8, 16), &expected, 1e-5, &format!("render {} {name} patches", e.id));
            if err > worst.0 {
                worst = (err, format!("{} {name}", e.id));
            }
        }
    }
    println!("render patches, all {} stocks: largest error {:.3e} ({})", film::catalog().len(), worst.0, worst.1);
}

#[test]
fn render_dozen_on_all_inputs() {
    for dir in std::fs::read_dir(grainy_testkit::root().join("golden").join("film")).unwrap() {
        let name = dir.unwrap().file_name().into_string().unwrap();
        let Some(id) = name.strip_prefix("render_").filter(|n| !n.starts_with("patches_")) else { continue };
        let case = Case::open("film", &name);
        let step = case.json()["garden_step"].as_u64().unwrap() as usize;
        for (space_name, space) in SPACES {
            let wide = film::render(&input("wide"), id, space).unwrap();
            let row: Vec<f32> = wide.slice(ndarray::s![0..1, .., ..]).iter().copied().collect();
            close32(&row, &a32(&case, &format!("wide_{space_name}")), 1e-5, &format!("{id} wide {space_name}"));
            let colours = film::render(&input("colours"), id, space).unwrap();
            close32(
                &every(&colours, 0, 2),
                &a32(&case, &format!("colours_{space_name}")),
                1e-5,
                &format!("{id} colours {space_name}"),
            );
            let garden = film::render(&input("garden"), id, space).unwrap();
            close32(
                &every(&garden, 0, step),
                &a32(&case, &format!("garden_{space_name}")),
                1e-5,
                &format!("{id} garden {space_name}"),
            );
            if case.has(&format!("garden4_{space_name}")) {
                let bright = input("garden").mapv(|v| v * 4.0);
                let out = film::render(&bright, id, space).unwrap();
                close32(
                    &every(&out, 0, 2),
                    &a32(&case, &format!("garden4_{space_name}")),
                    1e-5,
                    &format!("{id} garden x4 (halation) {space_name}"),
                );
            }
        }
    }
}

#[test]
fn halation_and_blur() {
    let case = Case::open("film", "halation");
    let meta = case.json();
    for (name, space) in SPACES {
        let into = film::matrices(space).0.map(|r| r.map(|v| v as f32));
        let bright = input("garden").mapv(|v| v * 4.0);
        let glow = film::halation(&bright, Some(&meta["spec"]), &into).unwrap();
        close32(&every(&glow, 0, 2), &a32(&case, &format!("garden_{name}")), 1e-5, &format!("halation garden {name}"));
        let edges = input("edges").mapv(|v| v * 3.0);
        let glow = film::halation(&edges, Some(&meta["wide"]), &into).unwrap();
        close32(
            glow.as_slice().unwrap(),
            &a32(&case, &format!("edges_{name}")),
            1e-5,
            &format!("halation edges {name}"),
        );
        let none = &meta["none"][name];
        assert_eq!(
            film::halation(&input("garden"), Some(&meta["spec"]), &into).is_none(),
            none["garden_none"].as_bool().unwrap()
        );
        assert_eq!(
            film::halation(&input("edges"), Some(&meta["spec"]), &into).is_none(),
            none["small_none"].as_bool().unwrap()
        );
        let radius = serde_json::json!({"radius": 0.05});
        assert_eq!(
            film::halation(&input("patches"), Some(&radius), &into).is_none(),
            none["dark_none"].as_bool().unwrap()
        );
        assert_eq!(
            film::halation(&input("edges"), Some(&serde_json::json!({})), &into).is_none(),
            none["empty_none"].as_bool().unwrap()
        );
        assert!(film::halation(&input("edges"), None, &into).is_none());
    }
    let case = Case::open("film", "gaussian");
    for (i, sigma) in case.json()["sigmas"].as_array().unwrap().iter().enumerate() {
        let sigma = num(sigma);
        let noise = input("noise").slice(ndarray::s![.., .., 0]).to_owned();
        let out = film::testing::gaussian_blur(&noise, sigma);
        close32(
            out.as_slice().unwrap(),
            &a32(&case, &format!("noise_{i}")),
            1e-5,
            &format!("GaussianBlur noise sigma {sigma}"),
        );
        if case.has(&format!("garden_{i}")) {
            let garden = input("garden").slice(ndarray::s![.., .., 1]).to_owned();
            let out = film::testing::gaussian_blur(&garden, sigma);
            close32(
                out.as_slice().unwrap(),
                &a32(&case, &format!("garden_{i}")),
                1e-5,
                &format!("GaussianBlur garden sigma {sigma}"),
            );
        }
    }
}

#[test]
fn unknown_stock_is_an_error() {
    assert!(matches!(film::render(&input("patches"), "nope", WorkingSpace::Srgb), Err(film::Error::UnknownStock(_))));
}

#[test]
fn catalogue() {
    let case = Case::open("film", "catalog");
    let meta = case.json();
    let stored = meta["catalog"].as_array().unwrap();
    assert_eq!(film::catalog().len(), stored.len());
    for (e, s) in film::catalog().iter().zip(stored) {
        assert_eq!(&e.raw, s);
        assert_eq!(e.id, s["id"].as_str().unwrap());
    }
    let shorts: Vec<String> = film::catalog().iter().map(|e| film::short_name(&e.maker, &e.name)).collect();
    assert_eq!(serde_json::to_value(&shorts).unwrap(), meta["short"]);
    assert_eq!(
        serde_json::json!([
            film::short_name("Kodak", "Kodak"),
            film::short_name("", "Plain"),
            film::short_name("Kodak", "Kodakx 1")
        ]),
        meta["short_extra"]
    );
    let by_maker: Vec<Value> = film::by_maker()
        .into_iter()
        .map(|(maker, folders)| {
            let folders: Vec<Value> = folders
                .into_iter()
                .map(|(title, entries)| {
                    serde_json::json!([title, entries.iter().map(|e| e.id.clone()).collect::<Vec<_>>()])
                })
                .collect();
            serde_json::json!([maker, folders])
        })
        .collect();
    assert_eq!(Value::Array(by_maker), meta["by_maker"]);
    for (name, known) in meta["known"].as_object().unwrap() {
        let name = if name == "None" { "" } else { name.as_str() };
        assert_eq!(film::known(name), known.as_bool().unwrap(), "known({name})");
    }
    for (name, grain) in meta["grain"].as_object().unwrap() {
        let ours = film::grain_settings(name).map(Value::Object).unwrap_or(Value::Null);
        assert_eq!(&ours, grain, "grain_settings({name})");
    }
    let kinds: Vec<Value> = film::KINDS.iter().map(|(k, t)| serde_json::json!([k, t])).collect();
    assert_eq!(Value::Array(kinds), meta["kinds"]);
    assert_eq!(serde_json::to_value(film::MAKERS).unwrap(), meta["makers"]);
    let mut checked = 0;
    for f in meta["find"].as_array().unwrap() {
        let query = f["query"].as_str();
        let found = film::find(query);
        let expected = match (&f["id"], &f["close"]) {
            (Value::String(id), Value::Null) if id.is_empty() => Found::Off,
            (Value::String(id), Value::Null) => Found::Stock(id.clone()),
            (Value::Null, Value::Array(close)) => {
                Found::Suggestions(close.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
            }
            other => panic!("unexpected golden {other:?}"),
        };
        assert_eq!(found, expected, "find({query:?})");
        checked += 1;
    }
    println!("find: {checked} queries match exactly");
}

#[test]
fn with_film_and_active() {
    let case = Case::open("film", "with_film");
    let meta = case.json();
    for c in meta["cases"].as_array().unwrap() {
        let start = grainy_core::Settings::from_value(c["start"].clone()).unwrap();
        let explicit: Vec<&str> = c["explicit"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let result = film::with_film(&start, c["name"].as_str().unwrap(), &explicit);
        assert_eq!(Value::Object(result.0), c["result"], "with_film to {}", c["name"]);
    }
    for c in meta["active"].as_array().unwrap() {
        let s = grainy_core::Settings::from_value(c["settings"].clone()).unwrap();
        assert_eq!(film::active(&s), c["active"].as_bool().unwrap(), "active {}", c["settings"]);
    }
}
