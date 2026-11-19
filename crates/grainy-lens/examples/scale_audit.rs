//! Strict, independently generated map audit, including strong synthetic lenses.
//! Run with `cargo run -p grainy-lens --example scale_audit -- <report.json>`.
//! A nonzero exit explicitly means the automatic-scale accuracy target is unmet.
use grainy_geometry::lens::{LensFlags, LensModel, LensRequest};
use grainy_lens::ProfileModel;
use grainy_testkit::Case;
use ndarray::{Array4, Array5};
use serde_json::{json, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let case = Case::open("lens", "scales");
    let j = case.json();
    let geometry: Array4<f32> = case.array("geometry");
    let subpixel: Array5<f32> = case.array("subpixel");
    let mut measurements = Vec::new();
    let mut failures = 0;
    let mut maximum = 0f32;
    for (i, row) in j["cases"].as_array().ok_or("missing cases")?.iter().enumerate() {
        let number = |key: &str| row[key].as_f64().ok_or("missing number");
        let w = number("width")? as usize;
        let h = number("height")? as usize;
        let bits = number("flags")? as u32;
        let profile = json!({"xml": j["xmls"][number("xml_index")? as usize]});
        let model = ProfileModel::from_profile(&profile)?;
        let mut request = LensRequest {
            profile,
            width: w,
            height: h,
            crop: number("crop")?,
            focal: number("focal")?,
            aperture: number("aperture")?,
            distance: number("distance")?,
            scale: 0.,
            flags: LensFlags { distortion: bits & 8 != 0, tca: bits & 1 != 0, vignetting: bits & 2 != 0, scale: true },
        };
        let mut errors = [0f32; 4];
        // The explicit-scale control separates errors in the solver from those
        // in normalization, the distortion polynomial, and SIMD radius lookup.
        for pass in 0..2 {
            if pass == 1 {
                request.scale = row["scales"][0].as_f64().ok_or("missing measured scale")?;
            }
            for (yi, y) in [0, h / 2, h - 1].into_iter().enumerate() {
                if row["geometry"] == true {
                    let actual = model.geometry_coordinates(&request, y, 1)?.ok_or("missing geometry")?;
                    for (xi, x) in [0, w / 2, w - 1].into_iter().enumerate() {
                        for k in 0..2 {
                            errors[pass * 2] =
                                errors[pass * 2].max((actual[[0, x, k]] - geometry[[i, yi, xi, k]]).abs());
                        }
                    }
                }
                if row["subpixel"] == true {
                    let actual = model.subpixel_coordinates(&request, y, 1)?.ok_or("missing subpixel")?;
                    for (xi, x) in [0, w / 2, w - 1].into_iter().enumerate() {
                        for c in 0..3 {
                            for k in 0..2 {
                                errors[pass * 2 + 1] = errors[pass * 2 + 1]
                                    .max((actual[[0, x, c, k]] - subpixel[[i, yi, xi, c, k]]).abs());
                            }
                        }
                    }
                }
            }
        }
        let error = errors[0].max(errors[1]);
        // Two float32 ULPs at the largest retained coordinates (1450 pixels).
        let tolerance = if w.max(h) > 1024 { 2e-4 } else { 1e-4 };
        let passed = error <= tolerance;
        failures += usize::from(!passed);
        maximum = maximum.max(error);
        measurements.push(json!({"name": row["name"], "suite": row["suite"], "geometry": errors[0],
            "subpixel": errors[1], "explicit_geometry": errors[2], "explicit_subpixel": errors[3],
            "tolerance": tolerance, "passed": passed}));
    }
    let report =
        json!({"cases": measurements.len(), "failures": failures, "maximum": maximum, "measurements": measurements});
    if let Some(path) = std::env::args_os().nth(1) {
        std::fs::write(path, serde_json::to_string_pretty(&report)? + "\n")?;
    }
    for suite in ["profiles", "stress"] {
        let rows: Vec<&Value> = report["measurements"]
            .as_array()
            .ok_or("missing measurements")?
            .iter()
            .filter(|r| r["suite"] == suite)
            .collect();
        let failed = rows.iter().filter(|r| r["passed"] == false).count();
        let worst = rows
            .iter()
            .map(|r| r["geometry"].as_f64().unwrap_or(0.).max(r["subpixel"].as_f64().unwrap_or(0.)))
            .fold(0f64, f64::max);
        println!("{suite}: {} cases, {failed} target misses, maximum {worst:.7e} px", rows.len());
    }
    if failures != 0 {
        return Err(format!("{failures} automatic-scale map cases miss the strict target").into());
    }
    Ok(())
}
