use grainy_imgproc::*;
use grainy_testkit::{assert_close, Case};
use ndarray::{Array2, Array3, ShapeBuilder};

#[test]
fn invalid_inputs_return_errors() {
    let empty = Array2::<u8>::zeros((0, 3));
    let mask = Array2::<u8>::zeros((3, 4));
    let a = Array2::<f32>::zeros((3, 4));
    assert!(inpaint(&a, &empty, 3.0).is_err());
    assert!(inpaint(&a, &mask, f64::NAN).is_err());
    assert!(inpaint(&Array2::zeros((1, 2)), &ndarray::arr2(&[[0, 1]]), 3.0).is_err());
    assert!(distance_transform(&empty, 3).is_err());
    assert!(distance_transform(&mask, 0).is_err());
    assert!(connected_components(&mask, 6).is_err());
    assert!(connected_components_with_stats(&empty, 8).is_err());
    assert!(flood_fill(&mask, (4, 0), 2).is_err());
    assert!(flood_fill(&empty, (0, 0), 2).is_err());
    assert!(find_contours(&empty, Retrieval::External).is_err());
    assert!(contour_area(&[[f32::NAN, 0.0]], false).is_err());
    assert!(arc_length(&[[f32::INFINITY, 0.0]], true).is_err());
    assert!(bounding_rect(&[[f32::MAX, 0.0]]).is_err());
    assert!(is_contour_convex(&[[0.0, f32::NAN]]).is_err());
    assert!(draw_contours(&empty, &[], 1, -1).is_err());
    assert!(draw_contours(&mask, &[], 1, 9).is_err());
    assert!(moments(&empty, true).is_err());
    assert!(absdiff(&mask, &empty).is_err());
    assert!(laplacian(&a, Border::Wrap).is_err());
    assert!(bilateral_filter(&a, 3, f64::NAN, 1.0, Border::Reflect101).is_err());
    assert!(bilateral_filter(&a, 0, 1.0, 1e10, Border::Reflect101).is_ok());
    assert!(match_template(&a, &Array2::zeros((2, 5)), TemplateMatch::SqDiff).is_err());
    assert!(match_template(&Array3::zeros((3, 4, 3)), &Array3::zeros((1, 1, 1)), TemplateMatch::SqDiff).is_err());
    assert!(line_segment_detector(&empty).is_err());
    for (rho, theta) in [(0.0, 0.1), (1.0, 0.0), (1.0, 1e-300), (1e300, 0.1), (f64::NAN, 0.1)] {
        assert!(hough_lines_p(&mask, rho, theta, 2, 1.0, 1.0).is_err());
    }
    let extreme = vec![vec![[i32::MIN, 0], [i32::MAX, 1], [i32::MAX, i32::MAX], [0, i32::MIN]]];
    assert!(draw_contours(&mask, &extreme, 1, -1).is_ok());
}

#[test]
fn identities_and_degenerate_cases() {
    let a = Array2::from_shape_fn((5, 7), |(y, x)| (y * 7 + x) as f32 / 35.0);
    for value in [0, 255] {
        assert_eq!(inpaint(&a, &Array2::from_elem(a.dim(), value), 3.0).unwrap(), a);
    }
    assert_eq!(absdiff(&a, &a).unwrap(), Array2::<f32>::zeros(a.dim()));
    assert_eq!(bilateral_filter(&a, 7, 0.0, 3.0, Border::Reflect101).unwrap(), a);
    assert!(match_template(&a, &Array2::from_elem((2, 2), 0.3), TemplateMatch::CCoeffNormed)
        .unwrap()
        .iter()
        .all(|&v| v == 1.0));
    assert!(convex_hull(&[], false).unwrap().is_empty());
    assert_eq!(bounding_rect::<i32>(&[]).unwrap(), [0; 4]);
    assert_eq!(draw_contours(&Array2::zeros((3, 4)), &[], 7, -1).unwrap(), Array2::<u8>::zeros((3, 4)));
}

#[test]
fn strided_inputs_match_goldens() {
    let case = Case::open("imgproc", "tier2_inpaint");
    let (i, j) = case.json()["variants"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .find(|(_, j)| j["label"] == "hole_signed" && j["radius"] == 3.0)
        .unwrap();
    let a: Array2<f32> = case.array(&format!("input_{i}"));
    let m: Array2<u8> = case.array(&format!("mask_{i}"));
    let a = Array2::from_shape_fn(a.dim().f(), |p| a[p]);
    let m = Array2::from_shape_fn(m.dim().f(), |p| m[p]);
    assert!(!a.is_standard_layout());
    assert_close(
        &inpaint(&a, &m, 3.0).unwrap(),
        &case.array(&format!("output_{i}")),
        j["tolerance"].as_f64().unwrap() as f32,
        "strided NS",
    );
    let c = Case::open("imgproc", "tier2_components");
    let src: Array2<u8> = c.array("input_16");
    let src = Array2::from_shape_fn(src.dim().f(), |p| src[p]);
    assert_eq!(connected_components(&src, 4).unwrap().1, c.array::<i32, _>("labels_16"));
    let c = Case::open("imgproc", "tier2_lines");
    let src: Array2<u8> = c.array("input_6");
    let src = Array2::from_shape_fn(src.dim().f(), |p| src[p]);
    let expected: Vec<[f32; 4]> = c.json()["variants"][6]["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| std::array::from_fn(|k| p[k].as_f64().unwrap() as f32))
        .collect();
    assert_eq!(line_segment_detector(&src).unwrap(), expected);
}

#[test]
fn row_parallel_work_is_deterministic() {
    let a = Array2::from_shape_fn((193, 197), |(y, x)| ((x * 61 + y * 23) % 257) as f32 / 256.0);
    let t = Array2::from_shape_fn((2, 3), |(y, x)| a[[y, x]]);
    let run = |threads| {
        rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap().install(|| {
            (
                laplacian(&a, Border::Reflect101).unwrap(),
                bilateral_filter(&a, 5, 0.3, 1.2, Border::Reflect101).unwrap(),
                match_template(&a, &t, TemplateMatch::CCoeffNormed).unwrap(),
            )
        })
    };
    assert_eq!(run(1), run(4));
}
