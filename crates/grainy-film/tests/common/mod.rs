//! Comparison helpers for the golden tests.
#![allow(dead_code)]

use grainy_core::Image;
use grainy_testkit::Case;
use ndarray::{s, ArrayD, IxDyn};

/// Asserts float64 values agree within `tolerance`, prints the largest error, returns it.
pub fn close64(actual: &[f64], expected: &ArrayD<f64>, tolerance: f64, what: &str) -> f64 {
    assert_eq!(actual.len(), expected.len(), "{what}: lengths differ");
    let mut worst = 0f64;
    for (a, e) in actual.iter().zip(expected.iter()) {
        let d = if a.is_nan() && e.is_nan() { 0.0 } else { (a - e).abs() };
        let d = if d.is_nan() { f64::INFINITY } else { d };
        worst = worst.max(d);
    }
    println!("{what}: max abs error {worst:.3e} (tolerance {tolerance:.1e})");
    assert!(worst <= tolerance, "{what}: max abs error {worst:.6e} > {tolerance:.1e}");
    worst
}

/// Asserts float32 values agree within `tolerance`, prints the largest error, returns it.
pub fn close32(actual: &[f32], expected: &ArrayD<f32>, tolerance: f32, what: &str) -> f32 {
    assert_eq!(actual.len(), expected.len(), "{what}: lengths differ");
    let mut worst = 0f32;
    for (a, e) in actual.iter().zip(expected.iter()) {
        let d = if a.is_nan() && e.is_nan() { 0.0 } else { (a - e).abs() };
        let d = if d.is_nan() { f32::INFINITY } else { d };
        worst = worst.max(d);
    }
    println!("{what}: max abs error {worst:.3e} (tolerance {tolerance:.1e})");
    assert!(worst <= tolerance, "{what}: max abs error {worst:.6e} > {tolerance:.1e}");
    worst
}

/// An array of a case as float64 of any rank.
pub fn a64(case: &Case, name: &str) -> ArrayD<f64> {
    case.array::<f64, IxDyn>(name)
}

/// An array of a case as float32 of any rank.
pub fn a32(case: &Case, name: &str) -> ArrayD<f32> {
    case.array::<f32, IxDyn>(name)
}

/// Every `step`-th pixel from `start`, flattened.
pub fn every(image: &Image, start: usize, step: usize) -> Vec<f32> {
    image.slice(s![start..;step, start..;step, ..]).iter().copied().collect()
}

/// A JSON number as f64.
pub fn num(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}
