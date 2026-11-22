//! The detail stage of `engine.develop`, ported from luma/engine.py `_develop_detail` (CPU path).

use grainy_core::{Image, Settings};
use ndarray::Zip;

use crate::np::{dot3, LUMA};
use crate::tools::{blur, detail_tools, detail_version, sharpen};
use crate::Result;

/// Port of luma/engine.py `_develop_detail` on the CPU path: detail tools (through `hdr.color_operation` when
/// `hdr` is on), monochrome, clarity, sharpening, post-crop vignette and grain, in the oracle's order. `s` are
/// normalized settings; `pixel_scale` is the preview's size relative to the original (1 for full size).
pub fn develop_detail(image: &Image, s: &Settings, pixel_scale: f64) -> Result<Image> {
    if image.ndim() != 3 || image.dim().2 != 3 {
        return Err(crate::Error::Shape(image.shape().to_vec()));
    }
    let mut a = if s.bool("hdr") {
        grainy_tone::hdr::color_operation(image, |bounded| detail_tools(bounded, s, pixel_scale))
    } else {
        detail_tools(image, s, pixel_scale)
    };
    if s.bool("monochrome") {
        Zip::from(a.rows_mut()).par_for_each(|mut p| {
            let y = dot3(&[p[0], p[1], p[2]], &LUMA);
            p.fill(y);
        });
    }
    if s.bool("clarity") {
        if detail_version(s) >= 4 {
            a = crate::laplacian::local_laplacian_clarity(&a, s.f64("clarity"), pixel_scale);
        } else {
            let (h, w, _) = a.dim();
            let sigma = pixel_scale.max(h.min(w) as f64 / 80.0);
            let blurred = blur(&a, sigma);
            let amount = s.f32("clarity");
            Zip::from(&mut a).and(&blurred).par_for_each(|v, &b| *v += (*v - b) * amount / 140.0);
        }
    }
    if s.bool("sharpen") {
        let _timing = grainy_core::timing::Span::new("detail.sharpen");
        a = sharpen(
            &a,
            s.f64("sharpen"),
            s.f64("sharpen_radius"),
            s.f64("sharpen_detail"),
            s.f64("sharpen_mask"),
            pixel_scale,
            detail_version(s) >= 2,
        );
    }
    if s.bool("vignette") {
        a = grainy_effects::post_crop_vignette(a, s);
    }
    Ok(grainy_effects::add_grain(a, s, pixel_scale))
}
