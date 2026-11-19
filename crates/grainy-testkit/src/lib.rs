//! Loads the golden cases the Python oracle wrote (`tools/golden/`, `golden/`) and compares arrays.
//!
//! ```ignore
//! let case = grainy_testkit::Case::open("tone", "rolloff_white_2");
//! let input: ndarray::Array3<f32> = case.array("input");
//! let expected: ndarray::Array3<f32> = case.array("output");
//! let white = case.json()["white"].as_f64().unwrap();
//! grainy_testkit::assert_close(&my_rolloff(&input, white), &expected, 1e-5, "rolloff");
//! ```

use std::fmt::Debug;
use std::path::{Path, PathBuf};

use ndarray::{Array, Array3, Dimension};
use ndarray_npy::ReadableElement;
use serde_json::Value;

/// The workspace root (the folder with `golden/`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// A standard input image by name (`golden/_inputs/<name>.npy`): `ramp`, `wide`, `colours`, `patches`, `noise`,
/// `edges` (64×96) and `garden` (160×240), all scene-linear `f32`.
pub fn input(name: &str) -> Array3<f32> {
    read(&root().join("golden").join("_inputs").join(format!("{name}.npy")))
}

fn read<T: ReadableElement, D: Dimension>(path: &Path) -> Array<T, D> {
    ndarray_npy::read_npy(path).unwrap_or_else(|error| panic!("cannot read golden array {}: {error}", path.display()))
}

/// One golden case: the folder `golden/<module>/<name>/` with `.npy` arrays and `case.json`.
pub struct Case {
    folder: PathBuf,
    meta: Value,
}

impl Case {
    /// Opens a case; panics with the generator to run when it is missing.
    pub fn open(module: &str, name: &str) -> Self {
        let folder = root().join("golden").join(module).join(name);
        let path = folder.join("case.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("golden case {} is missing ({error}); run tools/golden/gen_{module}.py", path.display())
        });
        let meta =
            serde_json::from_str(&text).unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
        Self { folder, meta }
    }

    /// The scalars and settings of the case (`case.json`).
    pub fn json(&self) -> &Value {
        &self.meta
    }

    /// An array of the case, with the dtype and rank the oracle stored (a mismatch panics).
    pub fn array<T: ReadableElement, D: Dimension>(&self, name: &str) -> Array<T, D> {
        read(&self.folder.join(format!("{name}.npy")))
    }

    /// Whether the case has an array of this name.
    pub fn has(&self, name: &str) -> bool {
        self.folder.join(format!("{name}.npy")).exists()
    }
}

/// The largest absolute difference of two same-shaped arrays and the index where it is. Two NaNs at the same
/// place are equal; a NaN against a number is an infinite difference.
pub fn max_abs_diff<D: Dimension>(actual: &Array<f32, D>, expected: &Array<f32, D>) -> (f32, usize) {
    assert_eq!(actual.shape(), expected.shape(), "shapes differ");
    let mut worst = (0f32, 0usize);
    for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        let difference = if a.is_nan() && e.is_nan() {
            0.0
        } else if a.is_nan() || e.is_nan() {
            f32::INFINITY
        } else {
            (a - e).abs()
        };
        if difference > worst.0 {
            worst = (difference, index);
        }
    }
    worst
}

/// Asserts that two arrays agree within `tolerance` everywhere, and prints the largest error (visible with
/// `cargo test -- --nocapture`), so reports can quote it.
pub fn assert_close<D: Dimension>(actual: &Array<f32, D>, expected: &Array<f32, D>, tolerance: f32, what: &str)
where
    D::Pattern: Debug,
{
    let (difference, index) = max_abs_diff(actual, expected);
    println!("{what}: max abs error {difference:.3e} (tolerance {tolerance:.1e})");
    if difference > tolerance {
        let a = actual.iter().nth(index).copied().unwrap_or(f32::NAN);
        let e = expected.iter().nth(index).copied().unwrap_or(f32::NAN);
        let above = actual
            .iter()
            .zip(expected.iter())
            .filter(|(x, y)| {
                let d = (**x - **y).abs();
                d > tolerance || d.is_nan()
            })
            .count();
        panic!(
            "{what}: max abs error {difference:.6e} > {tolerance:.1e} at flat index {index} (actual {a}, expected {e}); \
             {above} of {} values differ by more than the tolerance; shape {:?}",
            actual.len(),
            actual.shape()
        );
    }
}
