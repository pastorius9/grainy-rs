//! Tone-grid adapters to grainy-imgproc: reflect-101 box filters, float32 INTER_AREA,
//! and float64 horizontal INTER_LINEAR rows.

use grainy_core::{Image, Plane};

/// Port of cv2.boxFilter with reflect-101 borders.
pub fn box_filter(plane: &Plane, r: usize) -> Plane {
    let k = 2 * r + 1;
    grainy_imgproc::box_filter(plane, (k, k), None, true, grainy_imgproc::Border::Reflect101)
        .unwrap_or_else(|_| plane.clone())
}

/// Port of cv2.resize INTER_AREA on a reduced float image.
pub fn resize_area(a: &Image, width: usize, height: usize) -> Option<Image> {
    if width > a.dim().1 || height > a.dim().0 {
        return None;
    }
    grainy_imgproc::resize(a, (width, height), grainy_imgproc::Interpolation::Area).ok()
}

/// Adapter for the validated tone-grid dimensions.
pub struct LinearColumns(Option<grainy_imgproc::LinearColumns>);
impl LinearColumns {
    pub fn new(source: usize, width: usize) -> Self {
        Self(grainy_imgproc::LinearColumns::new(source, width).ok())
    }
    pub fn row(&self, source: &[f64], out: &mut [f64]) {
        if let Some(table) = &self.0 {
            let _ = table.row(source, out);
        }
    }
}
