//! Port of luma/straighten.py refit through grainy-geometry.
pub(crate) fn refit(
    crop: Option<[f64; 4]>,
    previous_angle: f64,
    angle: f64,
    width: f64,
    height: f64,
) -> Option<[f64; 4]> {
    grainy_geometry::straighten::refit(crop, previous_angle, angle, width, height, None)
}
