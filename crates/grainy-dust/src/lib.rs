//! Ports of luma/dust*.py. Images are finite float32 arrays; spot dictionaries retain oracle metadata.
pub mod batch;
mod detect;
mod exact;
mod helpers;
mod repair;
pub mod scratches;
pub mod shapes;
pub mod soft;
pub mod store;
pub mod texture;

pub use detect::{detect, Detection, Options};
use grainy_core::Plane;
pub use helpers::{component_indices, median, quantile_plan, quantiles, repeated_detail, QuantilePlan};
pub use repair::{repair, repair_in_place};
use serde_json::Value;

/// Port of luma/dust.py Cancelled and Python validation failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Port of luma/dust.py Cancelled.
    #[error("dust operation cancelled")]
    Cancelled,
    /// Port of luma/dust*.py ValueError.
    #[error("invalid dust input: {0}")]
    Invalid(String),
    /// Errors from the OpenCV primitive ports used by luma/dust*.py.
    #[error(transparent)]
    Imgproc(#[from] grainy_imgproc::Error),
    /// Errors from luma/dust_batch.py catalog access.
    #[error(transparent)]
    Catalog(#[from] grainy_catalog::Error),
    /// Errors from luma/dust_store.py JSON data.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Errors from luma/dust_store.py compressed data.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
/// Fallible return type for the luma/dust*.py ports.
pub type Result<T> = std::result::Result<T, Error>;
/// Port of the luma/dust.py cancel event; the callback may read an AtomicBool or count checkpoints.
pub type Cancel<'a> = Option<&'a (dyn Fn() -> bool + Sync)>;
/// Port of luma/dust.py progress(done,total).
pub type Progress<'a> = Option<&'a mut dyn FnMut(usize, usize)>;
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// Port of luma/dust.py checkpoint.
pub fn checkpoint(cancel: Cancel<'_>) -> Result<()> {
    if cancel.is_some_and(|f| f()) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
pub(crate) fn plane(a: &Plane) -> Result<()> {
    shape(a.dim())?;
    if a.is_empty() || a.iter().any(|v| !v.is_finite()) {
        return Err(invalid("expected nonempty finite gray image"));
    }
    Ok(())
}
pub(crate) fn shape((h, w): (usize, usize)) -> Result<()> {
    if h == 0 || w == 0 || h > i32::MAX as usize || w > i32::MAX as usize {
        return Err(invalid("shape must have nonempty signed-32-bit pixel extents"));
    }
    Ok(())
}
pub(crate) fn number(s: &Value, key: &str) -> Result<f64> {
    s.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| invalid(format!("missing or invalid {key}")))
}
pub(crate) fn num(s: &Value, key: &str) -> f64 {
    s.get(key).and_then(Value::as_f64).unwrap_or(0.)
}
pub(crate) fn round(v: f64, digits: usize) -> f64 {
    // Python rounds the exact binary float to decimal (not a rounded v*10**digits).
    format!("{v:.digits$}").parse().unwrap_or(v)
}
pub(crate) fn rank(a: &Value, b: &Value) -> std::cmp::Ordering {
    num(b, "score")
        .total_cmp(&num(a, "score"))
        .then(num(a, "y").total_cmp(&num(b, "y")))
        .then(num(a, "x").total_cmp(&num(b, "x")))
}
pub(crate) fn eigen(a: f64, b: f64, d: f64) -> [f64; 2] {
    let mid = (a + d) * 0.5;
    let r = ((a - d) * 0.5).hypot(b);
    [mid - r, mid + r]
}
pub(crate) fn blur(a: &Plane, sigma: f64) -> Result<Plane> {
    exact::blur(a, sigma)
}

/// Port of luma/dust.py spot_bounds; returns pixel x,y,rx,ry.
pub fn spot_bounds(spot: &Value, shape: (usize, usize)) -> Result<[f64; 4]> {
    crate::shape(shape)?;
    let (h, w) = shape;
    if h == 0 || w == 0 {
        return Err(invalid("empty shape"));
    }
    let [x, y, rx, ry] = [number(spot, "x")?, number(spot, "y")?, number(spot, "rx")?, number(spot, "ry")?];
    let scale = spot.get("scale").map_or(Ok(100.), |_| number(spot, "scale"))? / 100.;
    if !(0. ..=1.).contains(&x)
        || !(0. ..=1.).contains(&y)
        || rx <= 0.
        || rx > 0.5
        || ry <= 0.
        || ry > 0.5
        || !(0.25..=2.5).contains(&scale)
    {
        return Err(invalid("spot coordinates, radii or scale"));
    }
    Ok([
        x * w.saturating_sub(1).max(1) as f64,
        y * h.saturating_sub(1).max(1) as f64,
        (rx * w as f64 * scale).max(0.75),
        (ry * h as f64 * scale).max(0.75),
    ])
}
/// Port of luma/dust.py visualize.
pub fn visualize(gray: &Plane, strength: f64) -> Result<Plane> {
    plane(gray)?;
    if !strength.is_finite() || !(1. ..=100.).contains(&strength) {
        return Err(invalid("visualize strength"));
    }
    let low = blur(gray, 1.5)?;
    Ok(ndarray::Zip::from(gray)
        .and(&low)
        .map_collect(|&a, &b| (((a - b).abs() - 0.003) * (4. + strength * 0.6) as f32).clamp(0., 1.)))
}
