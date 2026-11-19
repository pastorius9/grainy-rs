//! Shared ports of NumPy operations used by the Python oracle.
//! Separate variants preserve dtype promotion and accumulation order.

pub mod float32;
mod interp;
mod linspace;
pub mod numeric;
mod percentile;
pub mod random;
mod scalar;
mod stats;
pub use linspace::{linspace_int, linspace_vector};
pub use percentile::{median_f32, percentile, percentile_i64, percentiles, percentiles_f64};

pub use interp::{interp, interp_lr};
pub use scalar::*;
pub use stats::*;
