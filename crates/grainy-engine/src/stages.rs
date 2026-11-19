//! The stages around the tone–colour–detail chain: retouch, lens shading and geometry before it, local
//! adjustments after it, with the pieces they take from other crates.
//!
//! Everything is ported: dust repair comes from grainy-dust and sky detection from grainy-sky by default, and the
//! lens model of a stored lens profile is built from the settings by `develop` (grainy-lens). [`Externals`] lets
//! a caller replace any of them. A develop whose settings need a piece that is missing (a custom `Externals`
//! without it) fails with [`Error::Pending`](crate::Error::Pending) before any work: an edit is never silently
//! left out.

use std::borrow::Cow;

use grainy_core::settings::truthy;
use grainy_core::{Image, Plane, Settings};
use grainy_geometry::{LensModel, UseCrop};
use grainy_local::{DistanceTransform, DustRepair, Inpaint, LocalContext, LocalHooks, RetouchTools, SkyMask};
use ndarray::Array2;
use serde_json::Value;

use crate::{Crop, Result};

/// Implementations a develop takes from outside the pipeline crates. The default has dust repair and sky
/// detection; with no lens model given, `develop` builds the one of the settings' lens profile itself.
#[derive(Clone, Copy)]
pub struct Externals<'a> {
    /// The lens model of a lens profile (`optics._modifier`): distortion, lateral CA and vignetting.
    pub lens: Option<&'a dyn LensModel>,
    /// Dust repair (`dust.repair`) for retouch operations of type `dust`.
    pub dust: Option<&'a dyn DustRepair>,
    /// Sky detection (`sky_mask.sky_mask`) for mask components of type `sky`.
    pub sky: Option<&'a dyn SkyMask>,
}

/// `dust.repair(image, spots, copy=False, method=method)` from grainy-dust.
struct Dust;

impl DustRepair for Dust {
    fn repair(&self, image: &mut Image, spots: &[Value], method: &str) -> grainy_local::Result<()> {
        grainy_dust::repair_in_place(image, spots, None, method)
            .map_err(|error| grainy_local::Error::Invalid(error.to_string()))
    }
}

static DUST: Dust = Dust;

/// `sky_mask.sky_mask(source, component, transform)` from grainy-sky.
struct Sky;

impl SkyMask for Sky {
    fn sky_mask(
        &self,
        source: &Image,
        component: &serde_json::Map<String, Value>,
        transform: Option<grainy_local::Transform<'_>>,
    ) -> grainy_local::Result<Plane> {
        let component = Value::Object(component.clone());
        let moved = transform.map(|transform| {
            move |mask: &Image| transform(mask).map_err(|error| grainy_sky::Error::Input(error.to_string()))
        });
        let moved = moved.as_ref().map(|transform| transform as grainy_sky::Transform<'_>);
        grainy_sky::sky_mask(source, &component, moved).map_err(|error| grainy_local::Error::Invalid(error.to_string()))
    }
}

static SKY: Sky = Sky;

impl Default for Externals<'_> {
    fn default() -> Self {
        Self { lens: None, dust: Some(&DUST), sky: Some(&SKY) }
    }
}

impl std::fmt::Debug for Externals<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Externals")
            .field("lens", &self.lens.is_some())
            .field("dust", &self.dust.is_some())
            .field("sky", &self.sky.is_some())
            .finish()
    }
}

fn enabled(item: &Value) -> bool {
    item.get("enabled").is_none_or(truthy)
}

fn list<'a>(s: &'a Settings, key: &str) -> &'a [Value] {
    s.value(key).as_array().map_or(&[], Vec::as_slice)
}

/// The first thing the settings need that `externals` does not supply.
pub fn pending(s: &Settings, externals: &Externals<'_>) -> Option<&'static str> {
    if externals.lens.is_none() && s.bool("lensfun_enabled") && s.bool("lensfun") {
        return Some("lens profile corrections");
    }
    if externals.dust.is_none()
        && list(s, "retouch").iter().any(|op| enabled(op) && op.get("type").and_then(Value::as_str) == Some("dust"))
    {
        return Some("dust removal");
    }
    let sky = |layer: &Value| {
        layer.get("components").and_then(Value::as_array).is_some_and(|components| {
            components.iter().any(|c| enabled(c) && c.get("type").and_then(Value::as_str) == Some("sky"))
        })
    };
    if externals.sky.is_none() && list(s, "masks").iter().any(sky) {
        return Some("sky masks");
    }
    None
}

/// OpenCV's inpainting and distance transform for retouching, from grainy-imgproc.
struct Cv;

impl Inpaint for Cv {
    fn inpaint_ns(&self, plane: &Plane, mask: &Array2<u8>, radius: f64) -> grainy_local::Result<Plane> {
        Ok(grainy_imgproc::inpaint(plane, mask, radius)?)
    }
}

impl DistanceTransform for Cv {
    fn distance_l2(&self, mask: &Array2<u8>) -> grainy_local::Result<Plane> {
        Ok(grainy_imgproc::distance_transform(mask, 3)?)
    }
}

fn use_crop(crop: Crop) -> UseCrop {
    match crop {
        Crop::Cropped => UseCrop::Yes,
        Crop::Uncropped => UseCrop::No,
        Crop::All => UseCrop::All,
    }
}

/// `geometry(lens_shading(apply_retouch(linear, s['retouch']), s), s, use_crop, copy=False)`: `None` when nothing
/// applies and the frame is `linear` itself.
pub(crate) fn geometry(linear: &Image, s: &Settings, crop: Crop, externals: &Externals<'_>) -> Result<Option<Image>> {
    let operations = list(s, "retouch");
    let retouched = if operations.is_empty() {
        None
    } else {
        let tools = RetouchTools { inpaint: Some(&Cv), distance: Some(&Cv), dust: externals.dust };
        Some(grainy_local::apply_retouch(linear, operations, &tools)?)
    };
    let base = retouched.as_ref().unwrap_or(linear);
    let shaded = match grainy_geometry::lens::lens_shading(base, s, externals.lens)? {
        Cow::Owned(image) => Some(image),
        Cow::Borrowed(_) => None,
    };
    let base = shaded.as_ref().unwrap_or(base);
    let moved = match grainy_geometry::geometry(base, s, use_crop(crop), externals.lens)? {
        Cow::Owned(image) => Some(image),
        Cow::Borrowed(_) => None,
    };
    Ok(moved.or(shaded).or(retouched))
}

/// Whether the mask layers read the developed colour source (`to_srgb(linear)`): auto-masked brushes and sky
/// components.
pub(crate) fn needs_colour_source(s: &Settings) -> bool {
    list(s, "masks").iter().any(|layer| {
        layer.get("components").and_then(Value::as_array).is_some_and(|components| {
            components
                .iter()
                .any(|c| c.get("auto_mask").is_some_and(truthy) || c.get("type").and_then(Value::as_str) == Some("sky"))
        })
    })
}

/// `apply_local(a, source.shape, s['masks'], lambda mask: geometry(mask, s, use_crop), …)`.
pub(crate) fn local(
    a: Image,
    source_shape: (usize, usize),
    colour_source: Option<&Image>,
    s: &Settings,
    crop: Crop,
    pixel_scale: f64,
    externals: &Externals<'_>,
) -> Result<Image> {
    let stored = list(s, "masks");
    if stored.is_empty() {
        return Ok(a);
    }
    // HDR editing: every layer works on extended values.
    let extended: Vec<Value>;
    let layers = if s.bool("hdr") {
        extended = stored
            .iter()
            .map(|layer| {
                let mut layer = layer.clone();
                if let Some(map) = layer.as_object_mut() {
                    let adjustments = map.entry("adjustments").or_insert_with(|| Value::Object(Default::default()));
                    if let Some(adjustments) = adjustments.as_object_mut() {
                        adjustments.insert("hdr".into(), Value::Bool(true));
                    }
                }
                layer
            })
            .collect();
        extended.as_slice()
    } else {
        stored
    };
    let transform = |mask: &Image| -> grainy_local::Result<Image> {
        grainy_geometry::geometry(mask, s, use_crop(crop), externals.lens)
            .map(Cow::into_owned)
            .map_err(|error| grainy_local::Error::Invalid(error.to_string()))
    };
    let context = LocalContext {
        source_shape,
        transform: &transform,
        source_image: colour_source,
        pixel_scale,
        detail_version: s.i64("detail_version"),
        sky: externals.sky,
    };
    Ok(grainy_local::apply_local(&a, layers, &context, &mut LocalHooks::default())?)
}

#[cfg(test)]
mod output_goldens {
    use grainy_testkit::{assert_close, input, Case};
    #[test]
    fn retired_color_output_fixtures_use_exact_cms() {
        for (space, prefix, names, tolerance) in [
            ("sRGB", "srgb", &["wide", "hand", "colours"][..], 0.0),
            ("ProPhoto", "prophoto", &["colours", "patches", "noise", "wide", "hand"][..], 0.0),
        ] {
            for name in names {
                let case = Case::open("color", &format!("output_rgb_{prefix}_{name}"));
                let a = if *name == "hand" { case.array("input") } else { input(name) };
                let out = grainy_cms::output_rgb(&a, space, grainy_cms::Destination::Named("sRGB")).unwrap();
                assert_close(&out, &case.array("output"), tolerance, &format!("CMS output {space} {name}"));
            }
        }
    }
}
