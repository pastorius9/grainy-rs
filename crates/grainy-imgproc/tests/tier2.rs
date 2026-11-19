use grainy_imgproc::*;
use grainy_testkit::{assert_close, Case};
use ndarray::Array2;

fn variants(op: &str, mut run: impl FnMut(&Case, usize, &serde_json::Value)) {
    let case = Case::open("imgproc", &format!("tier2_{op}"));
    for (i, j) in case.json()["variants"].as_array().unwrap().iter().enumerate() {
        run(&case, i, j);
    }
}

#[test]
fn inpaint_goldens() {
    variants("inpaint", |c, i, j| {
        let a = c.array(&format!("input_{i}"));
        let mask = c.array(&format!("mask_{i}"));
        let out = inpaint(&a, &mask, j["radius"].as_f64().unwrap()).unwrap();
        assert_close(
            &out,
            &c.array(&format!("output_{i}")),
            j["tolerance"].as_f64().unwrap() as f32,
            &format!("tier2/inpaint/{i}/{}", j["label"]),
        );
        for ((y, x), &v) in mask.indexed_iter() {
            if v == 0 {
                assert_eq!(out[[y, x]].to_bits(), a[[y, x]].to_bits());
            }
        }
    });
}

#[test]
fn distance_goldens() {
    variants("distance", |c, i, j| {
        let out = distance_transform(&c.array(&format!("input_{i}")), j["size"].as_u64().unwrap() as usize).unwrap();
        assert_close(
            &out,
            &c.array(&format!("output_{i}")),
            j["tolerance"].as_f64().unwrap() as f32,
            &format!("tier2/distance/{i}/{}", j["label"]),
        );
    });
}

#[test]
fn components_goldens() {
    variants("components", |c, i, j| {
        let a = c.array(&format!("input_{i}"));
        let connectivity = j["connectivity"].as_u64().unwrap() as usize;
        let out = connected_components_with_stats(&a, connectivity).unwrap();
        assert_eq!(out.count, j["count"].as_u64().unwrap() as usize);
        assert_eq!(out.labels, c.array::<i32, _>(&format!("labels_{i}")), "labels {i}");
        assert_eq!(out.stats, c.array::<i32, _>(&format!("stats_{i}")), "stats {i}");
        let expected: Array2<f64> = c.array(&format!("centroids_{i}"));
        for (a, b) in out.centroids.iter().zip(expected.iter()) {
            assert!(a == b || a.is_nan() && b.is_nan(), "centroids {i}: {a} {b}");
        }
        let (count, labels) = connected_components(&a, connectivity).unwrap();
        assert_eq!(count, out.count);
        assert_eq!(labels, out.labels);
        println!("tier2/components/{i}: equal (tolerance 0)");
    });
}

#[test]
fn flood_goldens() {
    variants("flood", |c, i, j| {
        let seed = (j["seed"][0].as_u64().unwrap() as usize, j["seed"][1].as_u64().unwrap() as usize);
        let out = flood_fill(&c.array(&format!("input_{i}")), seed, j["value"].as_u64().unwrap() as u8).unwrap();
        assert_eq!(out.area, j["count"].as_u64().unwrap() as usize);
        assert_eq!(out.image, c.array::<u8, _>(&format!("output_{i}")), "image {i}");
        assert_eq!(out.mask, c.array::<u8, _>(&format!("mask_{i}")), "mask {i}");
        assert_eq!(out.rect, std::array::from_fn(|k| j["rect"][k].as_i64().unwrap() as i32));
        println!("tier2/flood/{i}: equal (tolerance 0)");
    });
}

fn int_points(v: &serde_json::Value) -> Vec<[i32; 2]> {
    v.as_array().unwrap().iter().map(|p| [p[0].as_i64().unwrap() as i32, p[1].as_i64().unwrap() as i32]).collect()
}
fn measure<T: Coordinate>(p: &[[T; 2]], j: &serde_json::Value) {
    assert_eq!(contour_area(p, false).unwrap(), j["area"].as_f64().unwrap());
    assert_eq!(contour_area(p, true).unwrap(), j["signed"].as_f64().unwrap());
    assert_eq!(arc_length(p, true).unwrap(), j["closed"].as_f64().unwrap());
    assert_eq!(arc_length(p, false).unwrap(), j["opened"].as_f64().unwrap());
    assert_eq!(bounding_rect(p).unwrap(), std::array::from_fn(|k| j["rect"][k].as_i64().unwrap() as i32));
    assert_eq!(is_contour_convex(p).unwrap(), j["convex"].as_bool().unwrap());
}
#[test]
fn contours_goldens() {
    variants("contours", |c, i, j| {
        let mode = if j["mode"] == "external" { Retrieval::External } else { Retrieval::CComp };
        let out = find_contours(&c.array(&format!("input_{i}")), mode).unwrap();
        let expected: Vec<_> = j["contours"].as_array().unwrap().iter().map(int_points).collect();
        assert_eq!(out.contours, expected, "contours {i}: {}", j["label"]);
        let hierarchy: Vec<[i32; 4]> = j["hierarchy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| std::array::from_fn(|k| p[k].as_i64().unwrap() as i32))
            .collect();
        assert_eq!(out.hierarchy, hierarchy, "hierarchy {i}");
        for (p, m) in out.contours.iter().zip(j["measurements"].as_array().unwrap()) {
            measure(p, m);
            assert_eq!(convex_hull(p, false).unwrap(), int_points(&m["hull"]), "hull {i}");
            assert_eq!(convex_hull(p, true).unwrap(), int_points(&m["hull_cw"]), "hull clockwise {i}");
        }
        println!("tier2/contours/{i}: equal (tolerance 0)");
    });
}
#[test]
fn polygon_goldens() {
    variants("polygon", |_, i, j| {
        let p: Vec<[f32; 2]> = j["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| [p[0].as_f64().unwrap() as f32, p[1].as_f64().unwrap() as f32])
            .collect();
        measure(&p, j);
        println!("tier2/polygon/{i}: equal (tolerance 0)");
    });
}

#[test]
fn hull_goldens() {
    variants("hull", |_, i, j| {
        let p = int_points(&j["points"]);
        assert_eq!(convex_hull(&p, false).unwrap(), int_points(&j["hull"]), "hull {i}");
        assert_eq!(convex_hull(&p, true).unwrap(), int_points(&j["hull_cw"]), "clockwise hull {i}");
        println!("tier2/hull/{i}: equal (tolerance 0)");
    });
}

#[test]
fn drawing_goldens() {
    variants("drawing", |c, i, j| {
        let contours: Vec<_> = j["contours"].as_array().unwrap().iter().map(int_points).collect();
        let out = draw_contours(&c.array(&format!("input_{i}")), &contours, 7, j["thickness"].as_i64().unwrap() as i32)
            .unwrap();
        assert_eq!(out, c.array::<u8, _>(&format!("output_{i}")), "drawing {i}");
        println!("tier2/drawing/{i}: equal (tolerance 0)");
    });
}

#[test]
fn moments_goldens() {
    variants("moments", |c, i, j| {
        let m = moments(&c.array(&format!("input_{i}")), j["binary"].as_bool().unwrap()).unwrap();
        let values = [
            m.m00, m.m10, m.m01, m.m20, m.m11, m.m02, m.m30, m.m21, m.m12, m.m03, m.mu20, m.mu11, m.mu02, m.mu30,
            m.mu21, m.mu12, m.mu03, m.nu20, m.nu11, m.nu02, m.nu30, m.nu21, m.nu12, m.nu03,
        ];
        let keys = [
            "m00", "m10", "m01", "m20", "m11", "m02", "m30", "m21", "m12", "m03", "mu20", "mu11", "mu02", "mu30",
            "mu21", "mu12", "mu03", "nu20", "nu11", "nu02", "nu30", "nu21", "nu12", "nu03",
        ];
        let mut max = 0_f64;
        for (key, actual) in keys.into_iter().zip(values) {
            let expected = j["moments"][key].as_f64().unwrap();
            let error = (actual - expected).abs();
            max = max.max(error);
            assert!(error <= 1e-5, "moments {i} {key}: {actual} != {expected}");
        }
        println!("tier2/moments/{i}: max abs error {max:.9e} (tolerance 1e-5)");
    });
}

#[test]
fn misc_goldens() {
    for op in ["absdiff", "laplacian", "bilateral", "template"] {
        variants(op, |c, i, j| {
            if op == "absdiff" && j["dtype"] == "uint8" {
                let a: ndarray::ArrayD<u8> = c.array(&format!("input_{i}"));
                assert_eq!(
                    absdiff(&a, &c.array(&format!("other_{i}"))).unwrap(),
                    c.array::<u8, _>(&format!("output_{i}"))
                );
                println!("tier2/absdiff/{i}: equal (tolerance 0)");
                return;
            }
            let out = match op {
                "absdiff" => {
                    let a: ndarray::ArrayD<f32> = c.array(&format!("input_{i}"));
                    absdiff(&a, &c.array(&format!("other_{i}"))).unwrap()
                }
                "laplacian" => {
                    let a: ndarray::ArrayD<f32> = c.array(&format!("input_{i}"));
                    let b = match j["border"].as_str().unwrap() {
                        "reflect101" => Border::Reflect101,
                        "reflect" => Border::Reflect,
                        "replicate" => Border::Replicate,
                        _ => Border::Constant(0.0),
                    };
                    laplacian(&a, b).unwrap()
                }
                "bilateral" => bilateral_filter(
                    &c.array(&format!("input_{i}")),
                    j["d"].as_i64().unwrap() as i32,
                    j["sc"].as_f64().unwrap(),
                    j["ss"].as_f64().unwrap(),
                    Border::Reflect101,
                )
                .unwrap()
                .into_dyn(),
                "template" => {
                    let a: ndarray::ArrayD<f32> = c.array(&format!("input_{i}"));
                    match_template(
                        &a,
                        &c.array(&format!("template_{i}")),
                        if j["method"] == "sqdiff" { TemplateMatch::SqDiff } else { TemplateMatch::CCoeffNormed },
                    )
                    .unwrap()
                    .into_dyn()
                }
                _ => unreachable!(),
            };
            assert_close(
                &out,
                &c.array(&format!("output_{i}")),
                j["tolerance"].as_f64().unwrap() as f32,
                &format!("tier2/{op}/{i}/{}", j["label"]),
            );
        });
    }
}

#[test]
fn lsd_goldens() {
    variants("lines", |c, i, j| {
        let out = line_segment_detector(&c.array(&format!("input_{i}"))).unwrap();
        let expected: Vec<[f32; 4]> = j["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| std::array::from_fn(|k| p[k].as_f64().unwrap() as f32))
            .collect();
        assert_eq!(out.len(), expected.len(), "LSD count {i}: {}", j["label"]);
        let actual = Array2::from_shape_vec((out.len(), 4), out.into_iter().flatten().collect()).unwrap();
        let expected = Array2::from_shape_vec((expected.len(), 4), expected.into_iter().flatten().collect()).unwrap();
        assert_close(
            &actual,
            &expected,
            j["tolerance"].as_f64().unwrap() as f32,
            &format!("tier2/lsd/{i}/{}", j["label"]),
        );
    });
}

#[test]
fn hough_goldens() {
    variants("lines", |c, i, j| {
        let a = c.array(&format!("input_{i}"));
        for (k, p) in j["hough"].as_array().unwrap().iter().enumerate() {
            let out = hough_lines_p(
                &a,
                p["rho"].as_f64().unwrap(),
                p["theta"].as_f64().unwrap(),
                p["threshold"].as_i64().unwrap() as i32,
                p["length"].as_f64().unwrap(),
                p["gap"].as_f64().unwrap(),
            )
            .unwrap();
            let expected: Vec<[i32; 4]> = p["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| std::array::from_fn(|k| p[k].as_i64().unwrap() as i32))
                .collect();
            assert_eq!(out, expected, "Hough {i}/{k}: {}", j["label"]);
            println!("tier2/hough/{i}/{k}: equal (tolerance 0)");
        }
    });
}
