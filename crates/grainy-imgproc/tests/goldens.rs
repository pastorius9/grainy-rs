use grainy_imgproc::*;
use grainy_testkit::{assert_close, Case};
use ndarray::{Array2, Array3, ArrayD, Ix3};
use serde_json::Value;

fn pair(v: &Value) -> (usize, usize) {
    (v[0].as_u64().unwrap() as usize, v[1].as_u64().unwrap() as usize)
}
fn anchor(v: &Value) -> Option<(usize, usize)> {
    (!v.is_null()).then(|| pair(v))
}
fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}
fn u(v: &Value) -> usize {
    v.as_u64().unwrap() as usize
}
fn border(j: &Value) -> Border {
    match j["border"].as_str().unwrap_or("constant") {
        "reflect101" => Border::Reflect101,
        "reflect" => Border::Reflect,
        "replicate" => Border::Replicate,
        "wrap" => Border::Wrap,
        "constant" => {
            if j["border_value"].is_array() {
                let mut values = [0.0; 4];
                for (i, v) in j["border_value"].as_array().unwrap().iter().enumerate() {
                    values[i] = f(v) as f32;
                }
                Border::ConstantChannels(values)
            } else {
                Border::Constant(0.0)
            }
        }
        other => panic!("unknown border {other}"),
    }
}
fn interpolation(j: &Value) -> Interpolation {
    match j["interpolation"].as_str().unwrap() {
        "linear" => Interpolation::Linear,
        "area" => Interpolation::Area,
        "nearest" => Interpolation::Nearest,
        _ => unreachable!(),
    }
}
fn morph(j: &Value) -> Morphology {
    match j["operation"].as_str().unwrap() {
        "erode" => Morphology::Erode,
        "dilate" => Morphology::Dilate,
        "open" => Morphology::Open,
        "close" => Morphology::Close,
        "gradient" => Morphology::Gradient,
        "tophat" => Morphology::TopHat,
        "blackhat" => Morphology::BlackHat,
        _ => unreachable!(),
    }
}
fn common_op<T: Pixel>(a: &ArrayD<T>, case: &Case) -> Result<ArrayD<T>> {
    let j = case.json();
    match j["op"].as_str().unwrap() {
        "median" => median_blur(a, u(&j["size"])),
        "resize" => resize(a, pair(&j["size"]), interpolation(j)),
        "remap" => remap(a, &case.array("mx"), &case.array("my"), interpolation(j), border(j)),
        "remap_interleaved" => remap_interleaved(a, &case.array("map"), interpolation(j), border(j)),
        "warp_affine" => {
            let m: Array2<f64> = case.array("matrix");
            let matrix = std::array::from_fn(|y| std::array::from_fn(|x| m[[y, x]]));
            warp_affine(a, matrix, pair(&j["size"]), interpolation(j), border(j), j["inverse"].as_bool().unwrap())
        }
        "warp_perspective" => {
            let m: Array2<f64> = case.array("matrix");
            let matrix = std::array::from_fn(|y| std::array::from_fn(|x| m[[y, x]]));
            warp_perspective(a, matrix, pair(&j["size"]), interpolation(j), border(j), j["inverse"].as_bool().unwrap())
        }
        "color" => {
            let code = match j["code"].as_str().unwrap() {
                "RGB2HLS" => ColorConversion::Rgb2Hls,
                "HLS2RGB" => ColorConversion::Hls2Rgb,
                "RGB2HSV" => ColorConversion::Rgb2Hsv,
                "HSV2RGB" => ColorConversion::Hsv2Rgb,
                "RGB2GRAY" => ColorConversion::Rgb2Gray,
                _ => unreachable!(),
            };
            cvt_color(&a.clone().into_dimensionality::<Ix3>().unwrap(), code).map(|v| v.into_dyn())
        }
        "morph" => {
            let kernel = case.array("kernel");
            let at = anchor(&j["anchor"]);
            let n = u(&j["iterations"]);
            let b = if j["border"].is_null() { None } else { Some(border(j)) };
            // Exercise the standalone erode/dilate public functions as well as morphology_ex.
            match morph(j) {
                Morphology::Erode => erode(a, &kernel, at, n, b),
                Morphology::Dilate => dilate(a, &kernel, at, n, b),
                op => morphology_ex(a, op, &kernel, at, n, b),
            }
        }
        other => panic!("unknown generic operation {other}"),
    }
}

fn double_close(actual: &ArrayD<f64>, expected: &ArrayD<f64>, tolerance: f64, name: &str) {
    assert_eq!(actual.shape(), expected.shape());
    let error = actual.iter().zip(expected).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
    println!("{name}: max abs error {error:.9e} (tolerance {tolerance:.1e})");
    assert!(error <= tolerance, "{name}: {error} > {tolerance}\nactual {actual}\nexpected {expected}");
}

fn check(name: &str) {
    let case = Case::open("imgproc", name);
    let j = case.json();
    let tolerance = f(&j["tolerance"]);
    let op = j["op"].as_str().unwrap();
    if op == "kernel" {
        if j["precision"] == "f64" {
            double_close(
                &get_gaussian_kernel_f64(u(&j["size"]), f(&j["sigma"])).unwrap().into_dyn(),
                &case.array("output"),
                tolerance,
                name,
            );
        } else {
            assert_close(
                &get_gaussian_kernel(u(&j["size"]), f(&j["sigma"])).unwrap().into_dyn(),
                &case.array("output"),
                tolerance as f32,
                name,
            );
        }
        return;
    }
    if op == "rotation" || op == "perspective_matrix" {
        let a = if op == "rotation" {
            let m = get_rotation_matrix_2d((f(&j["center"][0]), f(&j["center"][1])), f(&j["angle"]), f(&j["scale"]))
                .unwrap();
            Array2::from_shape_fn((2, 3), |(y, x)| m[y][x]).into_dyn()
        } else {
            let s: Array2<f32> = case.array("src_points");
            let t: Array2<f32> = case.array("dst_points");
            let s = std::array::from_fn(|y| std::array::from_fn(|x| s[[y, x]]));
            let t = std::array::from_fn(|y| std::array::from_fn(|x| t[[y, x]]));
            let m = get_perspective_transform(s, t);
            if j["expected_error"] == true {
                assert!(m.is_err(), "degenerate oracle SVD system must be rejected");
                println!("{name}: rejected degenerate system (oracle output retained for inspection)");
                return;
            }
            let m = m.unwrap();
            Array2::from_shape_fn((3, 3), |(y, x)| m[y][x]).into_dyn()
        };
        double_close(&a, &case.array("output"), tolerance, name);
        return;
    }
    if op == "structuring" {
        let shape = match j["kind"].as_str().unwrap() {
            "rect" => MorphShape::Rect,
            "cross" => MorphShape::Cross,
            _ => MorphShape::Ellipse,
        };
        let a =
            get_structuring_element(shape, pair(&j["size"]), anchor(&j["anchor"])).unwrap().mapv(f32::from).into_dyn();
        let b: ArrayD<u8> = case.array("output");
        assert_close(&a, &b.mapv(f32::from), 0.0, name);
        return;
    }
    if j["dtype"] == "uint8" {
        let a: ArrayD<u8> = case.array("input");
        if op == "integral_u8" {
            let actual = integral_u8(&a).unwrap();
            let expected: ArrayD<i32> = case.array("output");
            assert_eq!(actual, expected);
            println!("{name}: max abs error 0 (tolerance 0)");
        } else {
            let actual = common_op(&a, &case).unwrap();
            let expected: ArrayD<u8> = case.array("output");
            assert_close(&actual.mapv(f32::from), &expected.mapv(f32::from), tolerance as f32, name);
        }
        return;
    }
    let a: ArrayD<f32> = case.array("input");
    if op == "integral" {
        double_close(&integral(&a).unwrap(), &case.array("output"), tolerance, name);
        return;
    }
    let actual = match op {
        "gaussian" => gaussian_blur(&a, pair(&j["size"]), f(&j["sigma_x"]), f(&j["sigma_y"]), border(j)),
        "box" => box_filter(&a, pair(&j["size"]), anchor(&j["anchor"]), j["normalize"].as_bool().unwrap(), border(j)),
        "blur" => blur(&a, pair(&j["size"]), anchor(&j["anchor"]), border(j)),
        "sep" => {
            let kx: ndarray::Array1<f32> = case.array("kx");
            let ky: ndarray::Array1<f32> = case.array("ky");
            sep_filter_2d(&a, &kx.to_vec(), &ky.to_vec(), anchor(&j["anchor"]), f(&j["delta"]), border(j))
        }
        "sobel" => sobel(&a, u(&j["dx"]), u(&j["dy"]), u(&j["size"]), f(&j["scale"]), f(&j["delta"]), border(j)),
        "magnitude" => magnitude(&a, &case.array("other")),
        _ => common_op(&a, &case),
    }
    .unwrap();
    assert_close(&actual, &case.array("output"), tolerance as f32, name);
}

fn run(op: &str) {
    let text = std::fs::read_to_string(grainy_testkit::root().join("golden/imgproc/manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let mut failures = Vec::new();
    let mut count = 0;
    for name in manifest["cases"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        if !name.starts_with(&format!("{op}_"))
            || (op == "integral" && name.starts_with("integral_u8_"))
            || (op == "remap" && name.starts_with("remap_interleaved_"))
        {
            continue;
        }
        count += 1;
        if std::panic::catch_unwind(|| check(name)).is_err() {
            failures.push(name);
        }
    }
    assert!(count > 0, "no golden cases for {op}");
    assert!(failures.is_empty(), "{op}: {}/{} cases failed: {failures:?}", failures.len(), count);
}

macro_rules! golden_test { ($($name:ident),*) => {$ (
    mod $name { #[test] fn oracle() {super::run(stringify!($name));} }
)*}; }
golden_test!(
    kernel,
    gaussian,
    sep,
    blur,
    median,
    sobel,
    integral,
    integral_u8,
    magnitude,
    resize,
    remap,
    remap_interleaved,
    rotation,
    perspective_matrix,
    warp_affine,
    warp_perspective,
    color,
    structuring,
    morph
);

#[test]
fn box_filter_goldens() {
    run("box");
}

#[test]
fn invalid_inputs_return_errors() {
    let a = Array2::<f32>::zeros((2, 3));
    assert!(gaussian_blur(&a, (0, 0), 0.0, 0.0, Border::default()).is_err());
    assert!(gaussian_blur(&a, (2, 3), 1.0, 0.0, Border::default()).is_err());
    assert!(gaussian_blur(&a, (3, 3), f64::NAN, 0.0, Border::default()).is_err());
    assert!(get_gaussian_kernel(0, 1.0).is_err());
    assert!(sep_filter_2d(&a, &[], &[1.0], None, 0.0, Border::default()).is_err());
    assert!(box_filter(&a, (3, 3), Some((3, 0)), true, Border::default()).is_err());
    assert!(box_filter(&a, (3, 3), None, true, Border::Wrap).is_err());
    assert!(median_blur(&a, 7).is_err());
    assert!(sobel(&a, 0, 0, 3, 1.0, 0.0, Border::default()).is_err());
    assert!(sobel(&a, usize::MAX, usize::MAX, 3, 1.0, 0.0, Border::default()).is_err());
    assert!(resize(&a, (0, 1), Interpolation::Linear).is_err());
    assert!(resize(&a, (usize::MAX, 2), Interpolation::Linear).is_err());
    assert!(remap(&a, &a, &Array2::zeros((3, 3)), Interpolation::Linear, Border::default()).is_err());
    assert!(remap(&a, &a, &a, Interpolation::Area, Border::default()).is_err());
    assert!(dilate(&a, &Array2::zeros((3, 3)), None, 1, None).is_err());
    assert!(erode(&a, &Array2::zeros((0, 0)), None, 1, None).is_err());
    assert!(magnitude(&a, &Array2::zeros((3, 3))).is_err());
    assert!(integral(&Array2::zeros((0, 3))).is_err());
    assert!(cvt_color(&Array3::<f32>::zeros((1, 1, 2)), ColorConversion::Rgb2Gray).is_err());
    assert!(remap_interleaved(&a, &Array3::zeros((2, 3, 1)), Interpolation::Linear, Border::default()).is_err());
    assert!(remap(&a, &Array2::from_elem((2, 3), f32::NAN), &a, Interpolation::Linear, Border::default()).is_err());
    assert!(get_rotation_matrix_2d((f64::MAX, 0.0), 1.0, 1.0).is_err());
    assert!(warp_affine(&a, [[0.0; 3]; 2], (3, 2), Interpolation::Linear, Border::Reflect101, false).is_err());
    assert!(warp_affine(
        &a,
        [[1.0, 0.0, 1e30], [0.0, 1.0, 0.0]],
        (3, 2),
        Interpolation::Linear,
        Border::Reflect101,
        false
    )
    .is_err());
    assert!(warp_perspective(&a, [[0.0; 3]; 3], (3, 2), Interpolation::Linear, Border::Reflect101, false).is_err());
    assert!(warp_perspective(
        &a,
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, -1.0]],
        (3, 2),
        Interpolation::Linear,
        Border::Reflect101,
        true
    )
    .is_err());
}

#[test]
fn identity_layout_and_thread_count() {
    let a = grainy_testkit::input("garden");
    assert_eq!(gaussian_blur(&a, (1, 1), 0.0, 0.0, Border::default()).unwrap(), a);
    assert_eq!(median_blur(&a, 1).unwrap(), a);
    assert_eq!(resize(&a, (240, 160), Interpolation::Linear).unwrap(), a);
    let large = Array3::from_shape_fn((361, 379, 3), |(y, x, c)| a[[y % 160, x % 240, c]]);
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let four = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let process = || {
        let b = gaussian_blur(&large, (0, 0), 2.5, 0.0, Border::default()).unwrap();
        let b = box_filter(&b, (17, 19), None, true, Border::Reflect101).unwrap();
        resize(&b, (137, 171), Interpolation::Area).unwrap()
    };
    assert_eq!(one.install(process), four.install(process));
    let nonstandard = a.clone().reversed_axes().as_standard_layout().to_owned().reversed_axes();
    assert!(!nonstandard.is_standard_layout());
    assert_eq!(
        gaussian_blur(&a, (5, 5), 0.8, 0.0, Border::default()).unwrap(),
        gaussian_blur(&nonstandard, (5, 5), 0.8, 0.0, Border::default()).unwrap()
    );
}
