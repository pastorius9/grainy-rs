//! Chromaticities from luma/dcp.py.
use grainy_core::numpy::numeric::{inv3, mulv3};
/// Port of dcp.PROPHOTO_XYZ's unscaled ProPhoto RGB primaries.
pub const PROPHOTO_PRIMARIES: [[f64; 3]; 3] = [
    [0.7347 / 0.2653, 0.1596 / 0.8404, 0.0366 / 0.0001],
    [1.0, 1.0, 1.0],
    [0.0, 0.0, (1.0 - 0.0366 - 0.0001) / 0.0001],
];
/// Port of dcp.D50, retained at the oracle's decimal precision.
pub const D50: [f64; 3] = [0.9642956764, 1.0, 0.8251046025];
/// ProPhoto RGB to XYZ (D50): port of `dcp.PROPHOTO_XYZ`, the primaries scaled so that white is D50.
pub fn prophoto_xyz() -> [[f64; 3]; 3] {
    let primaries = PROPHOTO_PRIMARIES;
    let d50 = D50;
    let scale = mulv3(&inv3(&primaries), d50);
    primaries.map(|row| [0, 1, 2].map(|c| row[c] * scale[c]))
}
