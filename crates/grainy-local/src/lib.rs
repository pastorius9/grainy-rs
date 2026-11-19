//! Local adjustments and retouching of Grainy's develop pipeline, ported from the Python Grainy:
//! `luma/processing.py` (mask layers, `apply_local`, `apply_retouch`), `luma/healing.py` and `luma/range_mask.py`.
//!
//! Layers, components and retouch operations are the JSON objects stored in the settings keys `masks` and
//! `retouch`, read as they are (`serde_json::Value`).
//!
//! Functions that need OpenCV primitives not ported yet, or other crates' work, take them as traits:
//! [`Inpaint`] (`cv2.inpaint` with `INPAINT_NS`), [`DistanceTransform`] (`cv2.distanceTransform` L2, mask 3),
//! [`DustRepair`] (`luma/dust.py repair`) and [`SkyMask`] (`luma/sky_mask.py sky_mask`). An operation that needs
//! one that was not supplied returns [`Error::Missing`]; nothing is skipped silently.
//!
//! The geometry of masks (`transform` in the oracle: `lambda mask: geometry(mask, s, use_crop)`) is a closure on
//! H×W×1 arrays ([`Transform`]).
//!
//! The oracle memoises masks, components, linear regions, local results and noise profiles in a render cache
//! (`cache=` arguments). That cache is left out here; the places where a cache plugs in are named in the
//! documentation of [`apply_local`], [`layer_mask`] and [`local::local_region`].

pub mod healing;
pub mod local;
pub mod mask;
mod np;
pub mod range;
pub mod retouch;
mod value;

use grainy_core::{Image, Plane};
use ndarray::Array2;
use serde_json::{Map, Value};

pub use healing::{automatic_offset, blend_boundary, directions, inpaint};
pub use local::{
    apply_local, local_region, LayerInputCallback, LocalContext, LocalHooks, MaskCallback, PointCallback, PointInput,
};
pub use mask::{
    brush_mask, brush_mask_bounds, component_mask, image_dependent, layer_component, layer_mask, limit_brush_color,
    Bounds, IMAGE_COMPONENTS,
};
pub use range::{color_coverage, sample_selection, select_colors, RangeSample};
pub use retouch::{apply_retouch, RetouchTools};

/// The geometry applied to masks in source coordinates (`lambda mask: geometry(mask, s, use_crop)` in the oracle):
/// an H×W×1 array in, the transformed H'×W'×1 array out.
pub type Transform<'a> = &'a dyn Fn(&Image) -> Result<Image>;

/// OpenCV's `cv2.inpaint(plane, mask, radius, cv2.INPAINT_NS)` on one float32 plane (Navier–Stokes inpainting of
/// the pixels where `mask` is nonzero). Used by the retouch kind `inpaint`, [`blend_boundary`] and [`inpaint`].
pub trait Inpaint {
    /// Inpaints `plane` where `mask` (0 or 255) is nonzero, with OpenCV's neighbourhood `radius`.
    fn inpaint_ns(&self, plane: &Plane, mask: &Array2<u8>, radius: f64) -> Result<Plane>;
}

/// OpenCV's `cv2.distanceTransform(mask, cv2.DIST_L2, 3)`: for every nonzero pixel of `mask`, the distance to the
/// nearest zero pixel (float32). Used by [`automatic_offset`].
pub trait DistanceTransform {
    /// The distance map of `mask`.
    fn distance_l2(&self, mask: &Array2<u8>) -> Result<Plane>;
}

impl<F> DistanceTransform for F
where
    F: Fn(&Array2<u8>) -> Result<Plane>,
{
    fn distance_l2(&self, mask: &Array2<u8>) -> Result<Plane> {
        self(mask)
    }
}

/// `luma/dust.py repair(image, spots, copy=False, method=method)`: repairs dust spots in place. Used by retouch
/// operations of `type == 'dust'`.
pub trait DustRepair {
    /// Repairs `spots` (the operation's `spots` list) in `image`, with `method` (`'smooth'` or `'texture'`).
    fn repair(&self, image: &mut Image, spots: &[Value], method: &str) -> Result<()>;
}

/// `luma/sky_mask.py sky_mask(source, component, transform)`: the sky coverage (0..1) in the pixels of `source`.
pub trait SkyMask {
    /// The coverage of a `sky` component in `source`'s pixels; `transform` is the mask geometry when the source is
    /// the developed colour source (the oracle passes it so detection sees the visible frame).
    fn sky_mask(
        &self,
        source: &Image,
        component: &Map<String, Value>,
        transform: Option<Transform<'_>>,
    ) -> Result<Plane>;
}

/// Errors of local adjustments and retouching.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A layer, component or operation value of the wrong kind.
    #[error("`{key}` has an unexpected value: {found}")]
    BadValue {
        /// The key (or a description of the value).
        key: String,
        /// The value found.
        found: String,
    },
    /// Arrays whose shapes do not fit together.
    #[error("{0}")]
    Shape(String),
    /// An operation needs an implementation that was not supplied (inpainting, distance transform, dust repair,
    /// sky detection).
    #[error("this operation needs {0}, which was not supplied")]
    Missing(&'static str),
    /// A value the oracle rejects with a `ValueError` (its message).
    #[error("{0}")]
    Invalid(String),
    /// A callback asked to stop (the oracle raises an exception from its callback to end `develop`).
    #[error("interrupted by a callback")]
    Interrupted,
    /// An error of the colour tools (point colours).
    #[error(transparent)]
    Color(#[from] grainy_colorgrade::Error),
    /// An error of an OpenCV primitive.
    #[error(transparent)]
    Imgproc(#[from] grainy_imgproc::Error),
}

/// Result of local adjustments and retouching.
pub type Result<T> = std::result::Result<T, Error>;
