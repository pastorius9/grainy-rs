//! Adapters to shared NumPy arithmetic; film builds validated dense matrices.
pub(crate) use grainy_core::numpy::numeric::*;
pub(crate) fn lstsq(a: &Matrix, b: &[f64]) -> Vec<f64> {
    grainy_core::numpy::numeric::lstsq(a, b).unwrap_or_default()
}
