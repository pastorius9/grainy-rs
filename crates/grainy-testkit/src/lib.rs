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

/// Whether this build computes `powf`, `expf`, `logf`, `log2f`, `cbrtf`, `sin`, ... with the C runtime the goldens
/// were written with: Microsoft's, on Windows x86-64 (numpy calls it, and so does Rust's `f32::powf` and the
/// like). Other maths libraries (macOS) round some of these results differently in the last place. A pure-Rust
/// replacement that keeps every result of the goldens' machine is not possible: the Microsoft C runtime's float
/// functions are not correctly rounded and their errors follow no published algorithm (of the 2^32 inputs,
/// `expf` differs from the correctly rounded result for 23,280, `log2f` for 313,550, `powf(x, 2.4)` for 4.0
/// million and `cbrtf` for 1.29 billion), and its own versions differ too. Tests keep their tolerance here and
/// state a separate, measured one for other systems ([`maths_tolerance`]).
pub const GOLDEN_MATHS: bool = cfg!(all(windows, target_arch = "x86_64"));

/// `golden` where the C runtime is the goldens' ([`GOLDEN_MATHS`]), `other` elsewhere.
pub fn maths_tolerance<T>(golden: T, other: T) -> T {
    if GOLDEN_MATHS {
        golden
    } else {
        other
    }
}

/// The bytes of a profile LittleCMS created, as LittleCMS creates it on this system. The goldens were written on
/// Windows, where LittleCMS puts the platform signature `MSFT` into the header (bytes 40..44); on every other
/// system it writes `APPL` (`cmsio0.c`, `CMS_IS_WINDOWS_`), as the oracle's LittleCMS does on macOS. Profiles
/// LittleCMS read from a file keep their signature: use this only for profiles it made.
pub fn lcms_created(golden: &[u8]) -> Vec<u8> {
    let mut bytes = golden.to_vec();
    if !cfg!(windows) && bytes.len() >= 128 && &bytes[40..44] == b"MSFT" {
        bytes[40..44].copy_from_slice(b"APPL");
    }
    bytes
}

/// Runs the checks of many cases and reports every failing case at the end instead of stopping at the first, so
/// that one run on another system (CI) shows all differences.
#[derive(Default)]
pub struct Checks {
    failures: Vec<String>,
}

impl Checks {
    /// Runs the checks of one case; a failing assertion inside is recorded under `what`.
    pub fn case(&mut self, what: &str, checks: impl FnOnce()) {
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(checks)) {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("panic");
            self.failures.push(format!("{what}: {message}"));
        }
    }

    /// Fails with every recorded failure.
    pub fn finish(self) {
        assert!(self.failures.is_empty(), "{} failing cases:\n{}", self.failures.len(), self.failures.join("\n"));
    }
}

/// The paths below a placeholder of golden data as the oracle writes them on this system. The goldens were
/// written on Windows (`<ROOT>\photos\Aone.jpg`, `<ROOT>\\photos\\A` inside JSON text or a Python repr);
/// on macOS the oracle writes `<ROOT>/photos/A/one.jpg`. Outside Windows every text (and key) of `value` gets
/// [`slashed`]; on Windows `value` stays as it is.
pub fn native_paths(value: &Value, placeholders: &[&str]) -> Value {
    if cfg!(windows) {
        return value.clone();
    }
    match value {
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (slashed(k, placeholders), native_paths(v, placeholders))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| native_paths(v, placeholders)).collect()),
        Value::String(s) => Value::String(slashed(s, placeholders)),
        other => other.clone(),
    }
}

/// The Windows path after each placeholder in `text` with slashes: a backslash, or a doubled one (escaped JSON
/// text, a Python repr), is one separator. The path ends at a quote (also an escaped one) or a line end; the
/// `\uXXXX`, `\xXX` and `\UXXXXXXXX` escapes of ASCII-only JSON text or a repr stay as they are.
pub fn slashed(text: &str, placeholders: &[&str]) -> String {
    let next =
        |rest: &str| placeholders.iter().filter_map(|p| rest.find(p).map(|at| (at, p.len()))).min_by_key(|(at, _)| *at);
    let mut out = String::new();
    let mut rest = text;
    while let Some((at, len)) = next(rest) {
        out.push_str(&rest[..at + len]);
        rest = &rest[at + len..];
        let mut chars = rest.char_indices().peekable();
        let mut end = rest.len();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' | '\'' | '\n' => {
                    end = i;
                    break;
                }
                '\\' => match chars.peek().map(|(_, n)| *n) {
                    Some('"' | '\'') => {
                        end = i;
                        break;
                    }
                    Some('\\') => {
                        chars.next();
                        out.push('/');
                    }
                    Some(kind @ ('u' | 'x' | 'U')) if hex_escape(&rest[i + 2..], kind) => out.push('\\'),
                    _ => out.push('/'),
                },
                c => out.push(c),
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Whether `text` starts with the hex digits of a `\u`, `\x` or `\U` escape.
fn hex_escape(text: &str, kind: char) -> bool {
    let digits = match kind {
        'x' => 2,
        'u' => 4,
        _ => 8,
    };
    text.chars().take(digits).filter(char::is_ascii_hexdigit).count() == digits
}

#[cfg(test)]
mod tests {
    #[test]
    fn golden_paths_are_rewritten_for_other_systems() {
        let b = '\\';
        let places = ["<ROOT>", "$ROOT"];
        let cases = [
            // A column or a reply text.
            (format!("<ROOT>{b}photos{b}A{b}one.jpg"), "<ROOT>/photos/A/one.jpg".to_owned()),
            // JSON text in a column: separators doubled, ASCII-only escapes kept.
            (
                format!("{{\"path\": \"<ROOT>{b}{b}photos{b}{b}Roll 12{b}{b}caf{b}u00e9.jpg\", \"n\": 1}}"),
                format!("{{\"path\": \"<ROOT>/photos/Roll 12/caf{b}u00e9.jpg\", \"n\": 1}}"),
            ),
            // A Python repr in an error message ends at its quote; other backslashes stay.
            (format!("No such file: '$ROOT{b}{b}gone.png' (x{b}y)"), format!("No such file: '$ROOT/gone.png' (x{b}y)")),
            ("<ROOT>".to_owned(), "<ROOT>".to_owned()),
            (format!("a{b}b <ROOT>{b}x $ROOT{b}y{b}z"), format!("a{b}b <ROOT>/x $ROOT/y/z")),
            // JSON text inside JSON text: an escaped quote ends the path.
            (
                format!("{{{b}\"p{b}\": {b}\"<ROOT>{b}{b}a{b}{b}b{b}\"}}"),
                format!("{{{b}\"p{b}\": {b}\"<ROOT>/a/b{b}\"}}"),
            ),
            ("plain text".to_owned(), "plain text".to_owned()),
        ];
        for (windows, other) in cases {
            assert_eq!(super::slashed(&windows, &places), other, "{windows}");
        }
    }
}
