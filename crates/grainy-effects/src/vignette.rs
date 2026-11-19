//! Post-crop vignette: port of `luma/engine.py` `vignette_shape` and `post_crop_vignette`.

use grainy_core::{Image, Settings};
use ndarray::{Axis, Zip};

/// Port of `engine.vignette_shape`: the vignette's shape parameters, shared with the GPU path. The defaults give
/// (0, 2, 1, 1, 0, …): the elliptical x²+y² vignette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VignetteShape {
    /// Roundness blend towards a circle (0..1, roundness above 0).
    pub round: f64,
    /// p-norm exponent (2 = ellipse, up to 8 = rounded rectangle for roundness below 0).
    pub p: f64,
    /// Midpoint scale applied to the squared distance.
    pub scale: f64,
    /// Feather exponent.
    pub exponent: f64,
    /// Highlight protection (0..1).
    pub protect: f64,
    /// Width over the longest side.
    pub aspect_x: f64,
    /// Height over the longest side.
    pub aspect_y: f64,
}

/// Port of `engine.vignette_shape(s, width, height)`.
pub fn vignette_shape(s: &Settings, width: usize, height: usize) -> VignetteShape {
    let roundness = s.f64("vignette_roundness");
    let longest = width.max(height).max(1) as f64;
    VignetteShape {
        round: roundness.max(0.0) / 100.0,
        p: 2.0 + (-roundness).max(0.0) / 100.0 * 6.0,
        scale: 4f64.powf((50.0 - s.f64("vignette_midpoint")) / 50.0),
        exponent: 4f64.powf((50.0 - s.f64("vignette_feather")) / 50.0),
        protect: s.f64("vignette_highlights") / 100.0,
        aspect_x: width as f64 / longest,
        aspect_y: height as f64 / longest,
    }
}

/// numpy's `array ** scalar` for float64, with numpy's fast paths for the exponents it special-cases.
fn np_pow(x: f64, e: f64) -> f64 {
    if e == 2.0 {
        x * x
    } else if e == 0.5 {
        x.sqrt()
    } else if e == 1.0 {
        x
    } else {
        x.powf(e)
    }
}

/// The sample positions of `np.ogrid[-1:1:complex(n)]`: `i*step - 1` with `step = 2/(n-1)` (n = 1 gives -1).
fn ogrid(n: usize) -> Vec<f64> {
    let step = if n == 1 { 1.0 } else { 2.0 / (n as f64 - 1.0) };
    (0..n).map(|i| i as f64 * step + -1.0).collect()
}

/// Port of `engine.post_crop_vignette`: darkens (amount below 0) or lightens towards the frame edges, in
/// float64, then multiplies the float32 image. Roundness above 0 turns the frame-shaped ellipse into a circle,
/// below 0 into a rounded rectangle; midpoint below 50 starts nearer the centre; feather below 50 makes the edge
/// harder; highlights keeps bright areas. Amount 0 is an exact no-op. Highlight protection uses the Rec. 709
/// luminance of the first three channels.
pub fn post_crop_vignette(mut image: Image, s: &Settings) -> Image {
    let (height, width, _) = image.dim();
    let shape = vignette_shape(s, width, height);
    let amount = s.f64("vignette");
    let mut xs = ogrid(width);
    let mut ys = ogrid(height);
    if shape.round != 0.0 {
        let fx = f64::from((1.0 + shape.round * (shape.aspect_x - 1.0)) as f32);
        let fy = f64::from((1.0 + shape.round * (shape.aspect_y - 1.0)) as f32);
        xs.iter_mut().for_each(|x| *x *= fx);
        ys.iter_mut().for_each(|y| *y *= fy);
    }
    let p = shape.p;
    let (xp, yp): (Vec<f64>, Vec<f64>) = if p == 2.0 {
        (xs.iter().map(|x| x * x).collect(), ys.iter().map(|y| y * y).collect())
    } else {
        (xs.iter().map(|x| np_pow(x.abs(), p)).collect(), ys.iter().map(|y| np_pow(y.abs(), p)).collect())
    };
    let weights = [0.2126f32, 0.7152, 0.0722];
    let protect32 = shape.protect as f32;
    Zip::from(image.axis_iter_mut(Axis(0))).and(&yp).par_for_each(|mut row, &y| {
        for (mut pixel, &x) in row.axis_iter_mut(Axis(0)).zip(&xp) {
            let mut d2 = if p == 2.0 { x + y } else { np_pow(x + y, 2.0 / p) };
            if shape.scale != 1.0 {
                d2 *= shape.scale;
            }
            d2 = d2.clamp(0.0, 2.0);
            if shape.exponent != 1.0 {
                d2 = 2.0 * np_pow(d2 / 2.0, shape.exponent);
            }
            let mut darken = d2 * amount / 220.0;
            if shape.protect != 0.0 {
                let luminance = pixel.iter().zip(weights).fold(0f32, |acc, (&v, w)| acc + v * w);
                let lum = ((luminance - 0.5) / 0.5).clamp(0.0, 1.0);
                darken *= f64::from(1.0 - protect32 * lum * lum);
            }
            let factor = 1.0 - darken;
            pixel.mapv_inplace(|v| (f64::from(v) * factor) as f32);
        }
    });
    image
}
