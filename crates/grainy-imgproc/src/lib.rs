//! CPU ports of the `cv2` primitives called by Python Grainy.
//!
//! Arrays have shape H×W or H×W×C; sizes and anchors use OpenCV's (width, height)
//! and (x, y) order. Non-contiguous owned arrays are supported. See README.md for
//! supported variants, numerical limits, and the oracle call-site inventory.

mod array;
mod border;
mod color;
mod contours;
mod drawing;
mod exact;
mod filters;
mod geometry;
mod linear_columns;
pub use linear_columns::LinearColumns;
pub mod hls;
mod inpaint;
mod lines;
mod misc;
mod morphology;
mod segmentation;

pub use lines::{hough_lines_p, line_segment_detector};

pub use drawing::draw_contours;
pub use misc::{absdiff, bilateral_filter, laplacian, match_template, moments, Moments, TemplateMatch};

pub use contours::{
    arc_length, bounding_rect, contour_area, convex_hull, find_contours, is_contour_convex, Contours, Coordinate,
    Retrieval,
};

pub use inpaint::inpaint;
pub use segmentation::{
    connected_components, connected_components_with_stats, distance_transform, flood_fill, Components, FloodFill,
};

pub use array::Pixel;
pub use border::Border;
pub use color::{cvt_color, ColorConversion};
pub use filters::{
    blur, box_filter, gaussian_blur, gaussian_blur_unfused, get_gaussian_kernel, get_gaussian_kernel_f64, integral,
    integral_u8, magnitude, magnitude_values, median_blur, sep_filter_2d, sobel,
};
pub use geometry::{
    get_perspective_transform, get_rotation_matrix_2d, remap, remap_interleaved, resize, warp_affine, warp_perspective,
    Interpolation,
};
pub use morphology::{dilate, erode, get_structuring_element, morphology_ex, MorphShape, Morphology};

/// Validation failures for ports of OpenCV's imgproc functions (instead of cv::Exception).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// An image, map, kernel, or output shape is unsupported by the corresponding cv2 function.
    #[error("invalid shape: {0}")]
    Shape(&'static str),
    /// A scalar argument is outside the corresponding cv2 function's supported domain.
    #[error("invalid argument: {0}")]
    Argument(&'static str),
}

/// Fallible return type used by the ports of cv2 functions.
pub type Result<T> = std::result::Result<T, Error>;
