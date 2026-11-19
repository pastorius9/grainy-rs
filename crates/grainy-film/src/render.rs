//! Rendering through a stock (port of `film_look._prophoto`, `_matrices`, `render`, `halation`) and the
//! OpenCV Gaussian blur halation uses.

use grainy_core::{parallel, Image, Plane};
use ndarray::{s, Array2, Axis};
use serde_json::Value;

use crate::numeric::{inv3, matmul3, mul3, mulv3, to_f32};
use crate::spectral::{d65, xyz_srgb, BRADFORD};
use crate::stock::stock;
use crate::{Error, Result};

/// The working space a render takes and returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkingSpace {
    /// Linear sRGB.
    Srgb,
    /// Linear ProPhoto (D50; colours adapted with Bradford).
    ProPhoto,
}

impl WorkingSpace {
    /// The setting value ('ProPhoto'; anything else is sRGB, as `_matrices` reads it).
    pub fn from_setting(name: &str) -> Self {
        if name == "ProPhoto" {
            Self::ProPhoto
        } else {
            Self::Srgb
        }
    }
}

pub use grainy_color::prophoto_xyz;

/// Port of `film_look._prophoto`: linear ProPhoto to XYZ adapted to D65.
pub fn prophoto() -> [[f64; 3]; 3] {
    let p = prophoto_xyz();
    let d50 = p.map(|r| r[0] + r[1] + r[2]);
    let d65 = d65();
    let target = mulv3(&BRADFORD, d65);
    let source = mulv3(&BRADFORD, d50);
    let diag = [0, 1, 2].map(|i| {
        let mut row = [0.0; 3];
        row[i] = target[i] / source[i];
        row
    });
    mul3(&mul3(&mul3(&inv3(&BRADFORD), &diag), &BRADFORD), &p)
}

/// Port of `film_look._matrices`: (working space to linear sRGB, XYZ to the working space).
pub fn matrices(space: WorkingSpace) -> ([[f64; 3]; 3], [[f64; 3]; 3]) {
    match space {
        WorkingSpace::ProPhoto => {
            let to_xyz = prophoto();
            (mul3(&xyz_srgb(), &to_xyz), inv3(&to_xyz))
        }
        WorkingSpace::Srgb => ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], xyz_srgb()),
    }
}

/// Port of `film_look.render`: display-linear working-space pixels (0..1) of scene-linear working-space
/// pixels `a` through stock `name`. Extra channels beyond three are dropped, as the oracle returns three.
pub fn render(a: &Image, name: &str, space: WorkingSpace) -> Result<Image> {
    if a.dim().2 < 3 {
        return Err(Error::Shape("render needs three channels".into()));
    }
    let film = stock(name)?;
    let (into, out) = matrices(space);
    let into = to_f32(&into);
    let out = to_f32(&out);
    let spec = film.data.get("halation");
    let glow = halation(a, spec, &into);
    let back = glow.as_ref().map(|_| {
        let m = into.map(|r| r.map(f64::from));
        to_f32(&inv3(&m))
    });
    let (height, width, _) = a.dim();
    let mut result = Image::zeros((height, width, 3));
    let pixel = |p: [f32; 3]| matmul3(film.xyz(matmul3(p, &into)), &out);
    let rows = parallel::bands(height, width, |first, stop| {
        let mut band = Array2::<f32>::zeros(((stop - first) * width, 3));
        for y in first..stop {
            for x in 0..width {
                let mut p = [a[[y, x, 0]], a[[y, x, 1]], a[[y, x, 2]]];
                if let (Some(glow), Some(back)) = (&glow, &back) {
                    let g = matmul3([glow[[y, x, 0]], glow[[y, x, 1]], glow[[y, x, 2]]], back);
                    p = [p[0] + g[0], p[1] + g[1], p[2] + g[2]];
                }
                let v = pixel(p);
                let row = (y - first) * width + x;
                band[[row, 0]] = v[0];
                band[[row, 1]] = v[1];
                band[[row, 2]] = v[2];
            }
        }
        (first, band)
    });
    for (first, band) in rows {
        let stop = first + band.nrows() / width.max(1);
        let block = band.into_shape_with_order((stop - first, width, 3)).map_err(|e| Error::Shape(e.to_string()))?;
        result.slice_mut(s![first..stop, .., ..]).assign(&block);
    }
    Ok(result)
}

/// Port of `film_look.halation`: light scattered back from the film base around bright areas (linear sRGB,
/// sized to the frame), or `None`. `spec`: {"radius": fraction of the diagonal, "threshold", "strength":
/// [r, g, b]}; `into`: working space to linear sRGB (float32).
pub fn halation(a: &Image, spec: Option<&Value>, into: &[[f32; 3]; 3]) -> Option<Image> {
    let spec = spec?;
    if !grainy_core::settings::truthy(spec) {
        return None;
    }
    let number = |key: &str, default: f64| spec.get(key).and_then(Value::as_f64).unwrap_or(default);
    let (height, width, _) = a.dim();
    let threshold = number("threshold", 1.0) as f32;
    let w = [0.2126f32, 0.7152, 0.0722];
    let mut bright = Plane::zeros((height, width));
    for ((y, x), v) in bright.indexed_iter_mut() {
        let rgb = matmul3([a[[y, x, 0]], a[[y, x, 1]], a[[y, x, 2]]], into);
        let luminance = rgb[0] * w[0] + rgb[1] * w[1] + rgb[2] * w[2];
        *v = (luminance - threshold).max(0.0);
    }
    let sigma = number("radius", 0.004) * (height as f64).hypot(width as f64);
    if sigma < 0.5 || !bright.iter().any(|&v| v != 0.0) {
        return None;
    }
    let spread = gaussian_blur(&bright, sigma);
    let strength: [f32; 3] = match spec.get("strength").and_then(Value::as_array) {
        Some(list) => [0, 1, 2].map(|i| list.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32),
        None => [0.35, 0.06, 0.0],
    };
    let mut glow = Image::zeros((height, width, 3));
    for (mut pixel, v) in glow.lanes_mut(Axis(2)).into_iter().zip(spread.iter()) {
        for c in 0..3 {
            pixel[c] = v * strength[c];
        }
    }
    Some(glow)
}

/// Port of cv2.GaussianBlur for halation, using the shared AVX2 arithmetic.
pub(crate) fn gaussian_blur(plane: &Plane, sigma: f64) -> Plane {
    grainy_imgproc::gaussian_blur(plane, (0, 0), sigma, 0., grainy_imgproc::Border::Reflect101)
        .unwrap_or_else(|_| plane.clone())
}
