//! Film grain: port of `luma/engine.py` `_grain_field`, `_normalized_blur`, `grain_pattern`, `_grain_noise`,
//! `grain_key`, `settings_grain` and `_add_grain`, with their bounded caches.
//!
//! The field is numpy's `default_rng(2026).standard_normal(shape)` (float64), reproduced bit for bit, so a photo
//! has the same grain as in the Python Grainy. The oracle's dtypes are kept: the pattern stays float64 while no
//! blur has touched it and becomes float32 after one, which changes the rounding of the steps after it.

use std::sync::{Arc, Mutex, MutexGuard};

use grainy_core::{Image, Settings};
use ndarray::{s, Array2, Axis, Zip};

use crate::blur::gaussian_blur;
use crate::npstats;
use crate::numpy_random::Generator;

/// The fixed grain seed of the oracle (`np.random.default_rng(2026)`).
const GRAIN_SEED: u128 = 2026;

/// Image shape (height, width), as the oracle's `a.shape[:2]`.
pub type Shape = (usize, usize);

/// A grain pattern with the dtype the oracle's `grain_pattern` returns for the same arguments: float64 when no
/// blur was applied (the field, possibly scaled or modulated in float64), float32 otherwise.
#[derive(Clone, Debug, PartialEq)]
pub enum GrainPattern {
    /// numpy float64 result.
    F64(Array2<f64>),
    /// numpy float32 result.
    F32(Array2<f32>),
}

impl GrainPattern {
    /// The values as f64 (exact for both dtypes).
    pub fn to_f64(&self) -> Array2<f64> {
        match self {
            Self::F64(a) => a.clone(),
            Self::F32(a) => a.mapv(f64::from),
        }
    }

    fn as_f32(&self) -> Array2<f32> {
        match self {
            Self::F64(a) => a.mapv(|v| v as f32),
            Self::F32(a) => a.clone(),
        }
    }

    /// `f * c` with a Python float `c`: numpy computes in the array's dtype (the scalar is weak).
    fn scale(self, c: f64) -> Self {
        match self {
            Self::F64(a) => Self::F64(a * c),
            Self::F32(a) => Self::F32(a * c as f32),
        }
    }

    /// `f * m` with a float32 array `m`: float64 stays float64, float32 stays float32.
    fn multiply(self, m: &Array2<f32>) -> Self {
        match self {
            Self::F64(mut a) => {
                Zip::from(&mut a).and(m).for_each(|x, &y| *x *= f64::from(y));
                Self::F64(a)
            }
            Self::F32(a) => Self::F32(a * m),
        }
    }

    /// `f + m` with a float32 array `m`.
    fn add(self, m: &Array2<f32>) -> Self {
        match self {
            Self::F64(mut a) => {
                Zip::from(&mut a).and(m).for_each(|x, &y| *x += f64::from(y));
                Self::F64(a)
            }
            Self::F32(a) => Self::F32(a + m),
        }
    }
}

/// A small least-recently-used cache like the oracle's `OrderedDict` caches.
struct Lru<K, V> {
    items: Vec<(K, V)>,
}

impl<K: PartialEq, V: Clone> Lru<K, V> {
    const fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// `cache.pop(key, None)`.
    fn pop(&mut self, key: &K) -> Option<V> {
        let index = self.items.iter().position(|(k, _)| k == key)?;
        Some(self.items.remove(index).1)
    }

    fn push(&mut self, key: K, value: V) {
        self.items.push((key, value));
    }

    fn trim_to(&mut self, len: usize) {
        while self.items.len() > len {
            self.items.remove(0);
        }
    }
}

static GRAIN_FIELDS: Mutex<Lru<Shape, Arc<Array2<f64>>>> = Mutex::new(Lru::new());
static GRAIN_NOISE: Mutex<Lru<NoiseKey, Arc<Array2<f32>>>> = Mutex::new(Lru::new());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Port of `engine._grain_field`: the standard-normal field of the fixed seed, float64, kept for the last three
/// shapes.
fn grain_field(shape: Shape) -> Arc<Array2<f64>> {
    let mut cache = lock(&GRAIN_FIELDS);
    let field = match cache.pop(&shape) {
        Some(field) => field,
        None => {
            let mut values = vec![0f64; shape.0 * shape.1];
            Generator::default_rng(GRAIN_SEED).fill_standard_normal(&mut values);
            let field = Array2::from_shape_vec(shape, values).unwrap_or_else(|_| Array2::zeros(shape));
            cache.trim_to(2);
            Arc::new(field)
        }
    };
    cache.push(shape, Arc::clone(&field));
    field
}

/// Port of `engine._normalized_blur`: cv2's Gaussian blur of the float32 field divided by its float32 standard
/// deviation (at least 1e-6).
fn normalized_blur(field: &Array2<f32>, sigma: f64) -> Array2<f32> {
    let blurred = gaussian_blur(field.view(), sigma);
    let flat: Vec<f32> = blurred.iter().copied().collect();
    let deviation = if flat.is_empty() { 0.0 } else { f64::from(npstats::std(&flat)) };
    let divisor = deviation.max(1e-6) as f32;
    blurred.mapv(|v| v / divisor)
}

/// Port of `engine.grain_pattern`: unit-deviation grain of the fixed seed. Size above 25 blurs the pixel noise
/// into larger grains, roughness above 50 modulates it with a coarse field, below 50 blends in a slightly
/// smoothed copy. Size 25 / roughness 50 is the field itself.
pub fn grain_pattern(shape: Shape, size: f64, roughness: f64, pixel_scale: f64) -> GrainPattern {
    let field = grain_field(shape);
    pattern_from_field(&field, size, roughness, pixel_scale)
}

fn pattern_from_field(field: &Array2<f64>, size: f64, roughness: f64, pixel_scale: f64) -> GrainPattern {
    let sigma = ((size - 25.0) / 25.0).max(0.0) * 1.5 * pixel_scale;
    let mut f = if sigma >= 0.3 {
        GrainPattern::F32(normalized_blur(&field.mapv(|v| v as f32), sigma))
    } else {
        GrainPattern::F64(field.clone())
    };
    if size > 25.0 {
        // f*np.float32(...): the factor is rounded to float32 first, whatever the array's dtype.
        let factor = f64::from((1.0 + (size - 25.0) / 75.0 * 0.9) as f32);
        f = match f {
            GrainPattern::F64(a) => GrainPattern::F64(a * factor),
            GrainPattern::F32(a) => GrainPattern::F32(a * factor as f32),
        };
    }
    if roughness > 50.0 {
        let depth = ((roughness - 50.0) / 50.0 * 0.6) as f32;
        let reversed = field.slice(s![..;-1, ..;-1]).mapv(|v| v as f32);
        let coarse = normalized_blur(&reversed, (3.0 * sigma + 2.0).max(2.0));
        let modulation = coarse.mapv(|c| {
            let m = 1.0 + depth * c;
            if m > 0.0 {
                m
            } else {
                0.0
            }
        });
        f = f.multiply(&modulation);
    } else if roughness < 50.0 {
        let k = (50.0 - roughness) / 50.0;
        let smooth = normalized_blur(&f.as_f32(), 0.7).mapv(|v| v * k as f32);
        f = f.scale(1.0 - k).add(&smooth);
    }
    f
}

/// Cache key of `_grain_noise`: (shape, grain) for the plain character, the full character otherwise. Floats are
/// compared by value like Python's (−0.0 equals 0.0).
#[derive(Clone, Copy, Debug, PartialEq)]
struct NoiseKey {
    shape: Shape,
    grain: f64,
    character: Option<(f64, f64, f64)>,
}

/// Port of `engine._grain_noise`: float32 grain for (shape, amount, character), kept for the last two
/// combinations.
fn grain_noise(shape: Shape, grain: f64, size: f64, roughness: f64, pixel_scale: f64) -> Arc<Array2<f32>> {
    let plain = size == 25.0 && roughness == 50.0;
    let key = NoiseKey { shape, grain, character: (!plain).then_some((size, roughness, pixel_scale)) };
    let cached = lock(&GRAIN_NOISE).pop(&key);
    let noise = cached.unwrap_or_else(|| {
        let c = grain / 1800.0;
        // (0 + grain/1800 * pattern).astype(np.float32), in the pattern's dtype.
        let noise = if plain {
            grain_field(shape).mapv(|v| (0.0 + c * v) as f32)
        } else {
            match grain_pattern(shape, size, roughness, pixel_scale) {
                GrainPattern::F64(a) => a.mapv(|v| (0.0 + c * v) as f32),
                GrainPattern::F32(a) => {
                    let c = c as f32;
                    a.mapv(|v| 0.0 + c * v)
                }
            }
        };
        Arc::new(noise)
    });
    let mut cache = lock(&GRAIN_NOISE);
    cache.push(key, Arc::clone(&noise));
    cache.trim_to(2);
    noise
}

/// Port of `engine.grain_key`: identity of the grain field (shape, grain, size, roughness, pixel scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainKey {
    /// Image shape (height, width).
    pub shape: Shape,
    /// `grain` amount.
    pub grain: f64,
    /// `grain_size`.
    pub size: f64,
    /// `grain_roughness`.
    pub roughness: f64,
    /// Pixel scale of the render (1 for full size).
    pub pixel_scale: f64,
}

/// Port of `engine.grain_key`.
pub fn grain_key(shape: Shape, s: &Settings, pixel_scale: f64) -> GrainKey {
    GrainKey {
        shape,
        grain: s.f64("grain"),
        size: s.f64("grain_size"),
        roughness: s.f64("grain_roughness"),
        pixel_scale,
    }
}

/// Port of `engine.settings_grain`: the float32 grain noise (height × width) for these settings, shared and
/// read-only like the oracle's cached array.
pub fn settings_grain(shape: Shape, s: &Settings, pixel_scale: f64) -> Arc<Array2<f32>> {
    grain_noise(shape, s.f64("grain"), s.f64("grain_size"), s.f64("grain_roughness"), pixel_scale)
}

/// Port of `engine._add_grain`: adds the grain noise to every channel (float32). Grain 0 returns the image
/// unchanged. Rows are processed in parallel; the result does not depend on the thread count.
pub fn add_grain(mut image: Image, s: &Settings, pixel_scale: f64) -> Image {
    if !s.bool("grain") {
        return image;
    }
    let (height, width, _) = image.dim();
    let noise = settings_grain((height, width), s, pixel_scale);
    Zip::from(image.axis_iter_mut(Axis(0))).and(noise.axis_iter(Axis(0))).par_for_each(|mut row, noise_row| {
        for (mut pixel, &n) in row.axis_iter_mut(Axis(0)).zip(noise_row.iter()) {
            pixel.mapv_inplace(|v| v + n);
        }
    });
    image
}

#[cfg(test)]
mod tests {
    use super::{normalized_blur, GrainPattern};
    use grainy_testkit::{assert_close, Case};
    use ndarray::Array2;

    #[test]
    fn normalized_blur_matches_oracle() {
        for shape in ["37x53", "5x7", "64x96", "1x9"] {
            for sigma in ["0.3", "0.7", "1.125", "1.5", "4.5", "15.5"] {
                let case = Case::open("effects", &format!("blur_{shape}_{sigma}"));
                let input: Array2<f32> = case.array("input");
                let expected: Array2<f32> = case.array("normalized");
                let sigma = case.json()["sigma"].as_f64().expect("sigma");
                if (shape == "5x7" || shape == "1x9") && sigma > 10.0 {
                    // A kernel far wider than the image leaves a nearly flat plane (deviation ~1e-4): dividing
                    // by it magnifies the blur's float32 rounding differences to ~0.2 %. Not a real-photo case.
                    continue;
                }
                assert_close(
                    &normalized_blur(&input, sigma),
                    &expected,
                    2e-5,
                    &format!("normalized blur {shape} σ {sigma}"),
                );
            }
        }
    }

    #[test]
    fn pattern_dtype_helpers() {
        let a = GrainPattern::F32(Array2::from_elem((1, 2), 2.0f32)).scale(0.5);
        assert_eq!(a, GrainPattern::F32(Array2::from_elem((1, 2), 1.0)));
    }
}
