//! Golden tests of grainy-local against the Python oracle (`tools/golden/gen_local.py`, `golden/local/`).
//!
//! Tolerance 0 (bit-exact: full-frame bit sums plus subsamples) everywhere except local luminance noise
//! reduction of detail_version 1 and 2, which goes through the oracle's Intel IPP bilateral filter (`IPP`, as in
//! grainy-detail). The functions taken as traits (inpainting, the distance transform, dust repair, sky detection)
//! are replayed from the oracle's recorded calls; the stand-ins check the inputs the port computes.

use std::cell::{Cell, RefCell};
use std::ops::ControlFlow;
use std::path::Path;

use grainy_core::{Image, Plane, Settings};
use grainy_local::{self as local, LocalContext, LocalHooks, PointInput, RetouchTools};
use grainy_testkit::{assert_close, input, max_abs_diff, Case};
use ndarray::{s, Array, Array1, Array2, ArrayD, Dimension, Slice};
use serde_json::{Map, Value};

const MODULE: &str = "local";
/// Local luminance noise reduction of detail_version 1-2 (the oracle's IPP bilateral filter; grainy-detail's
/// tolerance for the same filter).
const IPP: f32 = 2e-5;

type Obj = Map<String, Value>;

fn bits<'a>(values: impl IntoIterator<Item = &'a f32>) -> u64 {
    values.into_iter().fold(0u64, |acc, &v| acc.wrapping_add(f64::from(v).to_bits()))
}

fn parse_bits(v: &Value) -> u64 {
    v.as_str().and_then(|s| s.parse().ok()).expect("a bit sum")
}

fn cases(prefix: &str) -> Vec<String> {
    let folder = grainy_testkit::root().join("golden").join(MODULE);
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .expect("golden/local")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix) && Path::new(&folder.join(n).join("case.json")).exists())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no golden cases {prefix}*");
    names
}

/// Tolerance of the "exact" outputs where the C runtime is not the goldens' (macOS): the display frames are
/// `to_srgb` of the inputs, whose `powf` rounds some samples differently in the last place, and everything after
/// works on those frames (run 1: 6.0e-8 in `local_blacks_0`, 1.2e-7 in the colour component `color_tol2`).
const OTHER_MATHS: f32 = 2e-5;

/// The largest error [`assert_exact`] measured where the C runtime is not the goldens' (f32 bits; errors are not
/// negative, so their bits order as the values do).
static OTHER_WORST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn report_other_worst(what: &str) {
    let worst = f32::from_bits(OTHER_WORST.load(std::sync::atomic::Ordering::Relaxed));
    grainy_testkit::report_maths(what, f64::from(worst), f64::from(OTHER_MATHS));
}

/// Checks an output stored as `{name}_bits`, `{name}_shape` and `{name}_sub` (every `{name}_step`-th row and
/// column): bit-exact with the goldens' C runtime; elsewhere the subsampled values within [`OTHER_MATHS`].
fn assert_exact<D: Dimension>(case: &Case, name: &str, got: &Array<f32, D>, what: &str) {
    let meta = case.json();
    let shape: Vec<usize> = meta[format!("{name}_shape")]
        .as_array()
        .expect("shape")
        .iter()
        .map(|v| v.as_u64().unwrap_or(0) as usize)
        .collect();
    assert_eq!(got.shape(), &shape[..], "{what}: shape");
    let step = meta[format!("{name}_step")].as_i64().unwrap_or(1) as isize;
    let got = got.view().into_dyn();
    let sub = got
        .slice_each_axis(|ax| if ax.axis.index() < 2 { Slice::new(0, None, step) } else { Slice::from(..) })
        .to_owned();
    let expected: ArrayD<f32> = case.array(&format!("{name}_sub"));
    let error = max_abs_diff(&sub, &expected).0;
    OTHER_WORST.fetch_max(error.to_bits(), std::sync::atomic::Ordering::Relaxed);
    assert_close(&sub, &expected, grainy_testkit::maths_tolerance(0.0, OTHER_MATHS), what);
    if grainy_testkit::GOLDEN_MATHS {
        assert_eq!(bits(got.iter()), parse_bits(&meta[format!("{name}_bits")]), "{what}: full bit sum");
    }
}

fn error_text(error: &local::Error) -> String {
    format!("ValueError: {error}")
}

fn obj(v: &Value) -> &Obj {
    v.as_object().expect("a JSON object")
}

fn points(v: &Value) -> Vec<[f64; 2]> {
    v.as_array()
        .map(|list| {
            list.iter().map(|p| [p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN)]).collect()
        })
        .unwrap_or_default()
}

fn shape2(v: &Value) -> (usize, usize) {
    (v[0].as_u64().unwrap_or(0) as usize, v[1].as_u64().unwrap_or(0) as usize)
}

// ---------------------------------------------------------------- frames and transforms

/// The display-encoded frames `apply_local` works on: `to_srgb` of the standard inputs.
fn frame(name: &str) -> Image {
    match name {
        "odd" => frame("garden").slice(s![3..40, 5..58, ..]).to_owned(),
        _ => grainy_color::to_srgb(&input(name)),
    }
}

/// The scene-linear frames retouching works on (the standard inputs; `red` has two red discs).
fn linear(name: &str) -> Image {
    if name != "red" {
        return input(name);
    }
    let mut red = input("patches");
    for ((y, x, c), v) in red.indexed_iter_mut() {
        let (y, x) = (y as i64, x as i64);
        if (y - 30).pow(2) + (x - 40).pow(2) < 49 {
            *v = [0.8f64 as f32, 0.1f64 as f32, 0.12f64 as f32][c];
        }
        if (y - 20).pow(2) + (x - 70).pow(2) < 25 {
            *v = [0.5f64 as f32, 0.3f64 as f32, 0.32f64 as f32][c];
        }
    }
    red
}

fn identity(m: &Image) -> local::Result<Image> {
    Ok(m.clone())
}

/// The generator's `cropflip`: crop 3 rows top, 2 bottom, 1 column left, 4 right, then mirror horizontally.
fn cropflip(m: &Image) -> local::Result<Image> {
    let (h, w, _) = m.dim();
    Ok(m.slice(s![3..h - 2, 1..w - 4;-1, ..]).to_owned())
}

fn transform(name: &str) -> fn(&Image) -> local::Result<Image> {
    match name {
        "identity" => identity,
        "cropflip" => cropflip,
        other => panic!("unknown transform {other}"),
    }
}

#[test]
fn frames_match_the_oracle() {
    let case = Case::open(MODULE, "inputs");
    // The display frames are `to_srgb` (`powf`): bit-identical with the goldens' C runtime only (elsewhere
    // grainy-color's own goldens check `to_srgb` within their tolerance).
    for (name, expected) in case.json()["frames"].as_object().expect("frames") {
        if grainy_testkit::GOLDEN_MATHS {
            assert_eq!(bits(frame(name).iter()), parse_bits(expected), "frame {name}");
        }
    }
    for (name, expected) in case.json()["linear"].as_object().expect("linear") {
        assert_eq!(bits(linear(name).iter()), parse_bits(expected), "linear frame {name}");
    }
}

// ---------------------------------------------------------------- replayed trait implementations

/// Replays the oracle's recorded calls of a case, in order, checking the inputs.
struct Replay {
    case: Case,
    inpaint: Cell<usize>,
    distance: Cell<usize>,
    dust: Cell<usize>,
    sky: Cell<usize>,
}

impl Replay {
    fn new(name: &str) -> Self {
        Self {
            case: Case::open(MODULE, name),
            inpaint: Cell::new(0),
            distance: Cell::new(0),
            dust: Cell::new(0),
            sky: Cell::new(0),
        }
    }

    fn next(counter: &Cell<usize>) -> usize {
        let k = counter.get();
        counter.set(k + 1);
        k
    }

    fn recorded(&self, key: &str) -> usize {
        let v = &self.case.json()["recorded"][key];
        v.as_u64().map(|n| n as usize).or_else(|| v.as_array().map(Vec::len)).unwrap_or(0)
    }

    /// Asserts that every recorded call was made.
    fn finish(&self, what: &str) {
        assert_eq!(self.inpaint.get(), self.recorded("inpaint"), "{what}: inpaint calls");
        assert_eq!(self.distance.get(), self.recorded("distance"), "{what}: distance transform calls");
        assert_eq!(self.dust.get(), self.recorded("dust"), "{what}: dust repair calls");
        assert_eq!(self.sky.get(), self.recorded("sky"), "{what}: sky detection calls");
    }
}

impl local::Inpaint for Replay {
    fn inpaint_ns(&self, plane: &Plane, mask: &Array2<u8>, radius: f64) -> local::Result<Plane> {
        let k = Self::next(&self.inpaint);
        assert!(k < self.recorded("inpaint"), "an inpaint call the oracle did not make");
        let expected: Plane = self.case.array(&format!("inpaint_{k}_in"));
        assert_close(plane, &expected, 0.0, &format!("inpaint call {k}: input plane"));
        let expected_mask: Array2<u8> = self.case.array(&format!("inpaint_{k}_mask"));
        assert_eq!(mask, &expected_mask, "inpaint call {k}: mask");
        assert_eq!(radius, 3.0, "inpaint call {k}: radius");
        Ok(self.case.array(&format!("inpaint_{k}_out")))
    }
}

impl local::DistanceTransform for Replay {
    fn distance_l2(&self, mask: &Array2<u8>) -> local::Result<Plane> {
        let k = Self::next(&self.distance);
        let meta = &self.case.json()["recorded"]["distance"][k];
        let shape = shape2(&meta["shape"]);
        assert_eq!(mask.dim(), shape, "distance call {k}: shape");
        // np.packbits of the binary input (row-major, most significant bit first).
        let flat: Vec<bool> = mask.iter().map(|&v| v != 0).collect();
        let packed: Vec<u8> = flat
            .chunks(8)
            .map(|c| c.iter().enumerate().fold(0u8, |acc, (i, &b)| acc | (u8::from(b) << (7 - i))))
            .collect();
        let expected: Array1<u8> = self.case.array(&format!("distance_{k}_in"));
        assert_eq!(packed, expected.to_vec(), "distance call {k}: input");
        // automatic_offset reads only the largest distance.
        let mut out = Plane::zeros(shape);
        out[[0, 0]] = meta["max"].as_f64().unwrap_or(f64::NAN) as f32;
        Ok(out)
    }
}

impl local::DustRepair for Replay {
    fn repair(&self, image: &mut Image, spots: &[Value], method: &str) -> local::Result<()> {
        let k = Self::next(&self.dust);
        let meta = &self.case.json()["recorded"]["dust"][k];
        assert_eq!(bits(image.iter()), parse_bits(&meta["input_bits"]), "dust call {k}: input");
        assert_eq!(Value::Array(spots.to_vec()), meta["spots"], "dust call {k}: spots");
        assert_eq!(method, meta["method"].as_str().unwrap_or(""), "dust call {k}: method");
        let w: Vec<usize> =
            meta["window"].as_array().expect("window").iter().map(|v| v.as_u64().unwrap_or(0) as usize).collect();
        let patch: Image = self.case.array(&format!("dust_{k}_patch"));
        image.slice_mut(s![w[0]..w[1], w[2]..w[3], ..]).assign(&patch);
        assert_eq!(bits(image.iter()), parse_bits(&meta["output_bits"]), "dust call {k}: output");
        Ok(())
    }
}

impl local::SkyMask for Replay {
    fn sky_mask(
        &self,
        source: &Image,
        component: &Obj,
        transform: Option<local::Transform<'_>>,
    ) -> local::Result<Plane> {
        let k = Self::next(&self.sky);
        let meta = &self.case.json()["recorded"]["sky"][k];
        assert_eq!(bits(source.iter()), parse_bits(&meta["input_bits"]), "sky call {k}: source");
        assert_eq!(&Value::Object(component.clone()), &meta["component"], "sky call {k}: component");
        assert_eq!(transform.is_some(), meta["transformed"].as_bool().unwrap_or(false), "sky call {k}: transform");
        Ok(self.case.array(&format!("sky_{k}_out")))
    }
}

// ---------------------------------------------------------------- masks

#[test]
fn brush_masks() {
    let case = Case::open(MODULE, "brush");
    for row in case.json()["cases"].as_array().expect("cases") {
        let name = row["name"].as_str().unwrap_or("");
        let (mask, bounds) = local::brush_mask_bounds(
            shape2(&row["shape"]),
            &points(&row["points"]),
            row["radius"].as_f64().unwrap_or(0.0),
            row["feather"].as_f64().unwrap_or(0.0),
        )
        .expect("brush mask");
        assert_exact(&case, name, &mask, &format!("brush {name}"));
        let expected = row["bounds"].as_array().map(|b| {
            let v: Vec<usize> = b.iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect();
            (v[0], v[1], v[2], v[3])
        });
        assert_eq!(bounds, expected, "brush {name}: bounds");
    }
}

#[test]
fn component_masks() {
    let case = Case::open(MODULE, "component");
    for row in case.json()["cases"].as_array().expect("cases") {
        let name = row["name"].as_str().unwrap_or("");
        let mask = local::component_mask(shape2(&row["shape"]), obj(&row["component"])).expect("component mask");
        assert_exact(&case, name, &mask, &format!("component {name}"));
    }
}

#[test]
fn layer_components() {
    let mut checks = grainy_testkit::Checks::default();
    for tname in ["identity", "cropflip"] {
        let case = Case::open(MODULE, &format!("layer_component_{tname}"));
        let t = transform(tname);
        for row in case.json()["cases"].as_array().expect("cases") {
            let name = row["name"].as_str().unwrap_or("");
            let source = frame(row["frame"].as_str().unwrap_or(""));
            let image = t(&source).expect("transform");
            let source_image = row["source_image"].as_str().map(frame);
            let (h, w, _) = source.dim();
            let mask = local::layer_component(&image, (h, w), obj(&row["component"]), &t, source_image.as_ref(), None)
                .expect("component");
            let what = format!("{tname} component {name}");
            checks.case(&what, || assert_exact(&case, name, &mask, &what));
        }
    }
    report_other_worst("local layer components and earlier exact checks");
    checks.finish();
}

#[test]
fn sky_components_through_the_trait() {
    for tname in ["identity", "cropflip"] {
        let name = format!("layer_sky_{tname}");
        let replay = Replay::new(&name);
        let t = transform(tname);
        let source = frame("garden");
        let image = t(&source).expect("transform");
        let component = obj(&replay.case.json()["component"]).clone();
        let (h, w, _) = source.dim();
        let without = local::layer_component(&image, (h, w), &component, &t, None, Some(&replay)).expect("sky");
        assert_exact(&replay.case, "frame", &without, &format!("{name} (frame)"));
        let with = local::layer_component(&image, (h, w), &component, &t, Some(&source), Some(&replay)).expect("sky");
        assert_exact(&replay.case, "source", &with, &format!("{name} (colour source)"));
        replay.finish(&name);
        // Without a sky implementation the component is an error, never an empty mask.
        let missing = local::layer_component(&image, (h, w), &component, &t, None, None);
        assert!(matches!(missing, Err(local::Error::Missing(_))), "{name}: missing SkyMask");
    }
}

#[test]
fn layer_errors() {
    let case = Case::open(MODULE, "layer_errors");
    let errors = &case.json()["errors"];
    let patches = frame("patches");
    let shape = (patches.dim().0, patches.dim().1);
    let run = |component: Value, t: &dyn Fn(&Image) -> local::Result<Image>| {
        let image = t(&patches).expect("transform");
        local::layer_component(&image, shape, obj(&component), &t, None, None).map(|_| ())
    };
    let cases: Vec<(&str, local::Result<()>)> = vec![
        ("auto_shape", run(serde_json::json!({"type": "brush", "points": [[0.5, 0.5]], "auto_mask": true}), &cropflip)),
        ("samples6", run(serde_json::json!({"type": "color", "samples": vec![vec![0, 0, 0]; 6]}), &identity)),
        ("samples0", run(serde_json::json!({"type": "color", "samples": []}), &identity)),
        ("samples_inf", run(serde_json::json!({"type": "color", "samples": [[1e39, 0, 0]]}), &identity)),
        ("samples_shape", run(serde_json::json!({"type": "color", "samples": [[1, 2]]}), &identity)),
        ("tolerance0", run(serde_json::json!({"type": "color", "samples": [[1, 0, 0]], "tolerance": 0}), &identity)),
        (
            "tolerance_neg",
            run(serde_json::json!({"type": "color", "samples": [[1, 0, 0]], "tolerance": -1}), &identity),
        ),
    ];
    for (name, result) in cases {
        let error = result.expect_err(name);
        assert_eq!(error_text(&error), errors[name].as_str().unwrap_or(""), "error {name}");
    }
}

#[test]
fn layer_masks() {
    for name in cases("layer_mask_") {
        let case = Case::open(MODULE, &name);
        let t = transform(if name.ends_with("cropflip") { "cropflip" } else { "identity" });
        let source = frame(case.json()["frame"].as_str().unwrap_or(""));
        let image = t(&source).expect("transform");
        let (h, w, _) = source.dim();
        for row in case.json()["cases"].as_array().expect("cases") {
            let layer_name = row["name"].as_str().unwrap_or("");
            let mask = local::layer_mask(&image, (h, w), obj(&row["layer"]), &t, None, None).expect("layer mask");
            assert_exact(&case, layer_name, &mask, &format!("{name} {layer_name}"));
        }
        for row in case.json()["cases"].as_array().expect("cases") {
            let expected = row["image_dependent"].as_bool().expect("image_dependent");
            assert_eq!(local::image_dependent(obj(&row["layer"])), expected, "image_dependent {}", row["name"]);
        }
    }
}

// ---------------------------------------------------------------- apply_local

#[derive(Debug, PartialEq)]
struct PointCall {
    layer: usize,
    point: usize,
    image: u64,
    image_shape: Vec<usize>,
    mask: u64,
    mask_shape: Vec<usize>,
    origin: (usize, usize),
    frame: (usize, usize),
}

#[test]
fn apply_local_cases() {
    let mut worst_tolerant = 0f32;
    let mut checks = grainy_testkit::Checks::default();
    for name in cases("local_") {
        let case = Case::open(MODULE, &name);
        let meta = case.json();
        let source = frame(meta["frame"].as_str().unwrap_or(""));
        let t = transform(meta["transform"].as_str().unwrap_or("identity"));
        let image = t(&source).expect("transform");
        let source_image = meta["source_image"].as_str().map(frame);
        let layers = meta["layers"].as_array().cloned().unwrap_or_default();
        let ctx = LocalContext {
            source_shape: (source.dim().0, source.dim().1),
            transform: &t,
            source_image: source_image.as_ref(),
            pixel_scale: meta["pixel_scale"].as_f64().unwrap_or(1.0),
            detail_version: meta["detail_version"].as_i64().unwrap_or(1),
            sky: None,
        };
        let mode = &meta["callback_mode"];
        let masks: RefCell<Vec<(usize, Plane)>> = RefCell::new(Vec::new());
        let inputs: RefCell<Vec<(usize, u64)>> = RefCell::new(Vec::new());
        let calls: RefCell<Vec<PointCall>> = RefCell::new(Vec::new());
        let mut on_mask = |index: usize, mask: &Plane| masks.borrow_mut().push((index, mask.clone()));
        let mut on_input = |index: usize, value: &Image| {
            inputs.borrow_mut().push((index, bits(value.iter())));
            ControlFlow::Continue(())
        };
        let mut on_point = |p: &PointInput<'_>| {
            calls.borrow_mut().push(PointCall {
                layer: p.layer,
                point: p.point,
                image: bits(p.image.iter()),
                image_shape: p.image.shape().to_vec(),
                mask: bits(p.mask.iter()),
                mask_shape: p.mask.shape().to_vec(),
                origin: p.origin,
                frame: p.frame,
            });
            ControlFlow::Continue(())
        };
        let mut hooks = LocalHooks::default();
        if !mode.is_null() {
            hooks.on_mask = Some(&mut on_mask);
            hooks.on_layer_input = Some(&mut on_input);
        }
        if mode.as_str() == Some("points") {
            hooks.on_point = Some(&mut on_point);
        }
        let out = local::apply_local(&image, &layers, &ctx, &mut hooks).unwrap_or_else(|e| panic!("{name}: {e}"));
        checks.case(&name, || {
            if meta["tolerant"].as_bool() == Some(true) {
                let step = meta["output_step"].as_i64().unwrap_or(1) as isize;
                let sub = out.slice(s![..;step, ..;step, ..]).to_owned();
                let expected: Image = case.array("output_sub");
                assert_close(&sub, &expected, IPP, &format!("{name} (IPP bilateral)"));
                worst_tolerant = worst_tolerant.max(max_abs_diff(&sub, &expected).0);
            } else {
                assert_exact(&case, "output", &out, &name);
            }
            let seen = &meta["callbacks"];
            if !mode.is_null() {
                let expected_masks: Vec<usize> =
                    seen["masks"].as_array().expect("masks").iter().map(|v| v.as_u64().unwrap_or(0) as usize).collect();
                let got: Vec<usize> = masks.borrow().iter().map(|(i, _)| *i).collect();
                assert_eq!(got, expected_masks, "{name}: on_mask indices");
                for (k, (_, mask)) in masks.borrow().iter().enumerate() {
                    assert_exact(&case, &format!("cb_mask_{k}"), mask, &format!("{name}: on_mask {k}"));
                }
                let expected_inputs: Vec<(usize, u64)> = seen["inputs"]
                    .as_array()
                    .expect("inputs")
                    .iter()
                    .map(|v| (v[0].as_u64().unwrap_or(0) as usize, parse_bits(&v[1])))
                    .collect();
                // The layer inputs are the frames (`to_srgb`): their bit sums only with the goldens' C runtime.
                if grainy_testkit::GOLDEN_MATHS {
                    assert_eq!(*inputs.borrow(), expected_inputs, "{name}: on_layer_input");
                } else {
                    let indices = |list: &[(usize, u64)]| list.iter().map(|(i, _)| *i).collect::<Vec<_>>();
                    assert_eq!(indices(&inputs.borrow()), indices(&expected_inputs), "{name}: on_layer_input");
                }
            }
            let expected_points: Vec<PointCall> = seen["points"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .map(|p| {
                            let usizes = |v: &Value| {
                                v.as_array()
                                    .map(|a| a.iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect::<Vec<_>>())
                                    .unwrap_or_default()
                            };
                            PointCall {
                                layer: p["layer"].as_u64().unwrap_or(0) as usize,
                                point: p["point"].as_u64().unwrap_or(0) as usize,
                                image: parse_bits(&p["image"]),
                                image_shape: usizes(&p["image_shape"]),
                                mask: parse_bits(&p["mask"]),
                                mask_shape: usizes(&p["mask_shape"]),
                                origin: shape2(&p["origin"]),
                                frame: shape2(&p["frame"]),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            if grainy_testkit::GOLDEN_MATHS {
                assert_eq!(*calls.borrow(), expected_points, "{name}: on_point");
            } else {
                // Image and mask bit sums come from the frames (`to_srgb`): compare everything else.
                let plain = |list: &[PointCall]| {
                    list.iter()
                        .map(|c| (c.layer, c.point, c.image_shape.clone(), c.mask_shape.clone(), c.origin, c.frame))
                        .collect::<Vec<_>>()
                };
                assert_eq!(plain(&calls.borrow()), plain(&expected_points), "{name}: on_point");
            }
        });
    }
    report_other_worst("local apply_local cases and earlier exact checks");
    checks.finish();
    println!("local cases through the IPP bilateral filter: max abs error {worst_tolerant:.3e} (tolerance {IPP:.1e})");
}

#[test]
fn callbacks_can_stop_apply_local() {
    let case = Case::open(MODULE, "local_callbacks_points");
    let layers = case.json()["layers"].as_array().cloned().unwrap_or_default();
    let image = frame("colours");
    let ctx = LocalContext {
        source_shape: (image.dim().0, image.dim().1),
        transform: &identity,
        source_image: None,
        pixel_scale: 1.0,
        detail_version: 1,
        sky: None,
    };
    let mut seen = Vec::new();
    let mut stop_at_second = |index: usize, _: &Image| {
        seen.push(index);
        if index == 1 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let mut hooks = LocalHooks { on_layer_input: Some(&mut stop_at_second), ..LocalHooks::default() };
    assert!(matches!(local::apply_local(&image, &layers, &ctx, &mut hooks), Err(local::Error::Interrupted)));
    assert_eq!(seen, vec![0, 1]);
    let mut count = 0;
    let mut stop_point = |_: &PointInput<'_>| {
        count += 1;
        ControlFlow::Break(())
    };
    let mut hooks = LocalHooks { on_point: Some(&mut stop_point), ..LocalHooks::default() };
    assert!(matches!(local::apply_local(&image, &layers, &ctx, &mut hooks), Err(local::Error::Interrupted)));
    assert_eq!(count, 1);
}

// ---------------------------------------------------------------- retouching

#[test]
fn retouch_cases() {
    for name in cases("retouch_") {
        let replay = Replay::new(&name);
        let meta = replay.case.json().clone();
        let mut source = linear(meta["frame"].as_str().unwrap_or(""));
        let shift = meta["shift"].as_f64().unwrap_or(0.0) as f32;
        if shift != 0.0 {
            source.mapv_inplace(|v| v + shift);
        }
        let operations = meta["operations"].as_array().cloned().unwrap_or_default();
        let tools = RetouchTools { inpaint: Some(&replay), distance: Some(&replay), dust: Some(&replay) };
        let result = local::apply_retouch(&source, &operations, &tools);
        match meta["error"].as_str() {
            Some(error) => assert_eq!(error_text(&result.expect_err(&name)), error, "{name}: error"),
            None => assert_exact(&replay.case, "output", &result.unwrap_or_else(|e| panic!("{name}: {e}")), &name),
        }
        replay.finish(&name);
        // Without the implementations the trait-backed operations fail, never skip.
        let needs_tool = replay.recorded("inpaint") + replay.recorded("distance") + replay.recorded("dust") > 0;
        if needs_tool {
            let missing = local::apply_retouch(&source, &operations, &RetouchTools::default());
            assert!(matches!(missing, Err(local::Error::Missing(_))), "{name}: missing implementations");
        }
    }
}

#[test]
fn retouch_without_operations_returns_the_source() {
    let mut source = input("noise");
    source[[0, 0, 0]] = -1.0;
    assert_eq!(local::apply_retouch(&source, &[], &RetouchTools::default()).expect("no-op"), source);
}

// ---------------------------------------------------------------- healing

#[test]
fn healing_offsets() {
    let replay = Replay::new("healing_offsets");
    let directions: Array2<f64> = replay.case.array("directions");
    // `math.cos`/`math.sin`: the Microsoft C runtime's are not correctly rounded (sin(pi/4) is 0.7071067811865476
    // there, 0.7071067811865475 on macOS), so elsewhere a direction may differ in the last place.
    let ulps = |a: f64, b: f64| a.to_bits().abs_diff(b.to_bits());
    let worst = local::directions()
        .iter()
        .enumerate()
        .map(|(k, (c, sn))| ulps(*c, directions[[k, 0]]).max(ulps(*sn, directions[[k, 1]])))
        .max()
        .unwrap_or(0);
    grainy_testkit::report_maths("local healing directions (ulp)", worst as f64, 1.0);
    for (k, (c, sn)) in local::directions().iter().enumerate() {
        let (ec, es) = (directions[[k, 0]], directions[[k, 1]]);
        let off = ulps(*c, ec).max(ulps(*sn, es));
        assert!(off <= grainy_testkit::maths_tolerance(0, 1), "direction {k}: {:?} against {:?}", (c, sn), (ec, es));
    }
    let garden = input("garden");
    let (gh, gw, _) = garden.dim();
    let big = Image::from_shape_fn((gh * 4, gw * 3, 3), |(y, x, c)| garden[[y % gh, x % gw, c]]);
    for row in replay.case.json()["cases"].as_array().expect("cases") {
        let name = row["name"].as_str().unwrap_or("");
        let image = match row["frame"].as_str() {
            Some("big") => big.clone(),
            Some(other) => linear(other),
            None => unreachable!(),
        };
        let (h, w, _) = image.dim();
        let mask = local::brush_mask((h, w), &points(&row["points"]), row["radius"].as_f64().unwrap_or(0.0), 60.0)
            .expect("mask");
        let result = local::automatic_offset(&image, &mask, &replay);
        match row["error"].as_str() {
            Some(error) => assert_eq!(error_text(&result.expect_err(name)), error, "offset {name}: error"),
            None => {
                let expected =
                    row["offset"].as_array().map(|o| (o[0].as_f64().unwrap_or(0.0), o[1].as_f64().unwrap_or(0.0)));
                assert_eq!(result.expect(name), expected, "offset {name}");
            }
        }
    }
    replay.finish("healing_offsets");
}

#[test]
fn healing_blend_and_inpaint() {
    let replay = Replay::new("healing_blend");
    let garden = input("garden");
    let target = garden.slice(s![40..80, 60..120, ..]).to_owned();
    let donor = garden.slice(s![90..130, 150..210, ..]).to_owned();
    for name in replay.case.json()["weights"].as_array().expect("weights") {
        let name = name.as_str().unwrap_or("");
        let weight: Plane = replay.case.array(&format!("weight_{name}"));
        let blended = local::blend_boundary(&target, &donor, weight.view(), &replay).expect("blend");
        assert_exact(&replay.case, &format!("blend_{name}"), &blended, &format!("blend_boundary {name}"));
        let filled = local::inpaint(&target, weight.view(), &replay).expect("inpaint");
        assert_exact(&replay.case, &format!("inpaint_{name}"), &filled, &format!("inpaint {name}"));
    }
    replay.finish("healing_blend");
}

// ---------------------------------------------------------------- range masks

#[test]
fn range_coverage() {
    let case = Case::open(MODULE, "range_coverage");
    for row in case.json()["cases"].as_array().expect("cases") {
        let name = row["name"].as_str().unwrap_or("");
        let image = frame(row["frame"].as_str().unwrap_or(""));
        let got = local::color_coverage(&image, &row["samples"], row["tolerance"].as_f64().unwrap_or(0.0)).expect(name);
        assert_exact(&case, name, &got, &format!("color_coverage {name}"));
    }
    let image = frame("patches");
    let samples = serde_json::json!([[0.5, 0.5, 0.5]]);
    assert!(local::color_coverage(&image, &samples, f64::INFINITY).is_err(), "infinite tolerance");
    assert!(local::color_coverage(&image, &samples, f64::NAN).is_err(), "NaN tolerance");
}

#[test]
fn range_selections() {
    for name in cases("range_select_") {
        let case = Case::open(MODULE, &name);
        let meta = case.json();
        let settings = Settings::from_value(meta["settings"].clone()).expect("settings");
        let source_shape = shape2(&meta["source_shape"]);
        let frames = meta["inputs"].as_array().cloned().unwrap_or_default();
        let read = frames.iter().position(|f| f.get("geometry_shape").is_some());
        let window = |f: &Value| -> Vec<usize> {
            f["window"].as_array().expect("window").iter().map(|v| v.as_u64().unwrap_or(0) as usize).collect()
        };
        let develop_calls = Cell::new(0);
        let mut develop = |s: &Settings, hook: &mut dyn FnMut(usize, &Image) -> ControlFlow<()>| -> local::Result<()> {
            let expected = &meta["develop_settings"][develop_calls.get()];
            develop_calls.set(develop_calls.get() + 1);
            let masks = s.value("masks").as_array().map(Vec::len).unwrap_or(0);
            assert_eq!(masks as u64, expected["masks"].as_u64().unwrap_or(0), "{name}: layers rendered");
            assert_eq!(s.value("panels_off"), &expected["panels_off"], "{name}: panels_off");
            for (k, f) in frames.iter().enumerate() {
                let shape: Vec<usize> =
                    f["shape"].as_array().expect("shape").iter().map(|v| v.as_u64().unwrap_or(0) as usize).collect();
                let mut image = Image::zeros((shape[0], shape[1], shape[2]));
                if case.has(&format!("input_{k}")) {
                    let w = window(f);
                    let patch: Image = case.array(&format!("input_{k}"));
                    image.slice_mut(s![w[0]..w[1], w[2]..w[3], ..]).assign(&patch);
                }
                if hook(f["index"].as_u64().unwrap_or(0) as usize, &image).is_break() {
                    return Err(local::Error::Interrupted);
                }
            }
            Ok(())
        };
        let geometry_calls = Cell::new(0);
        let geometry = |ones: &Image, _: &Settings| -> local::Result<Image> {
            geometry_calls.set(geometry_calls.get() + 1);
            assert_eq!(ones.dim(), (source_shape.0, source_shape.1, 1), "{name}: geometry input");
            assert!(ones.iter().all(|&v| v == 1.0));
            let f = &frames[read.expect("the read layer")];
            let shape: Vec<usize> = f["geometry_shape"]
                .as_array()
                .expect("shape")
                .iter()
                .map(|v| v.as_u64().unwrap_or(0) as usize)
                .collect();
            let mut valid = Image::zeros((shape[0], shape[1], shape[2]));
            let w = window(f);
            let patch: Image = case.array("geometry_0");
            valid.slice_mut(s![w[0]..w[1], w[2]..w[3], ..]).assign(&patch);
            Ok(valid)
        };
        let result = local::sample_selection(
            source_shape,
            &settings,
            &points(&meta["points"]),
            meta["layer"].as_i64(),
            &mut develop,
            &geometry,
        );
        match meta["error"].as_str() {
            Some(error) => assert_eq!(error_text(&result.expect_err(&name)), error, "{name}: error"),
            None => {
                let got = result.unwrap_or_else(|e| panic!("{name}: {e}"));
                let expected = &meta["result"];
                let colors: Vec<[f32; 3]> = expected["colors"]
                    .as_array()
                    .expect("colors")
                    .iter()
                    .map(|c| [0, 1, 2].map(|i| c[i].as_f64().unwrap_or(f64::NAN) as f32))
                    .collect();
                assert_eq!(got.colors, colors, "{name}: colours");
                let range = [
                    expected["range"][0].as_f64().unwrap_or(f64::NAN),
                    expected["range"][1].as_f64().unwrap_or(f64::NAN),
                ];
                assert_eq!(got.range, range, "{name}: range");
            }
        }
        assert_eq!(geometry_calls.get() as u64, meta["geometry"].as_u64().unwrap_or(0), "{name}: geometry calls");
    }
}
