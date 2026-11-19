//! Port of the tone stage of luma/engine.py: `highlight_rolloff`, `rolloff_white`, `_tone_encoded`, `tone_base`,
//! `tone_local`, `_develop_tone_pixels` and `_develop_tone` (CPU path).

use grainy_core::{tone_white, white_balance, Image, Settings};
use ndarray::{concatenate, Array3, ArrayView3, Axis};
use serde_json::Value;

use crate::cv::resize_area;
use crate::hdr::{curve_extended_value, extension_slope, Points};
use crate::interp::interp;
use crate::mapping;
use crate::num::{clip, maximum, owned, pixels_mut, round_half_even, rows_mut, Float};
use crate::response::{self, luma, ToneBase, ToneLocal, MAP_SIZE};
use crate::srgb::to_srgb_value;
use crate::{Error, FilmRender, Result};

/// `SHOULDER_KNEE`: linear values below are never changed by the highlight roll-off.
pub const SHOULDER_KNEE: f64 = 0.5;
/// `ROLLOFF_WHITE`: at most two stops of highlights are rolled off.
pub const ROLLOFF_WHITE: f64 = 4.0;

/// Port of luma/engine.py `rolloff_white`: the linear value mapped to white, the tone gain capped at 4.
pub fn rolloff_white(s: &Settings) -> f64 {
    tone_white(s).min(ROLLOFF_WHITE)
}

/// The roll-off of one value (float32, the oracle's order of operations).
#[inline]
fn rolloff_value(a: f32, white: f64, knee: f64) -> f32 {
    let slope = ((white - knee) / (1.0 - knee)) as f32;
    let k = knee as f32;
    let u = (a - k) / ((white - knee) as f32);
    let rolled = k + ((1.0 - knee) as f32) * (slope * u / (1.0 + (slope - 1.0) * u));
    if a > k {
        rolled
    } else {
        a
    }
}

/// Port of luma/engine.py `highlight_rolloff`: compresses linear values above `knee` so that `white` maps to 1
/// (rational shoulder with slope 1 at the knee). A no-op for `white <= 1`.
pub fn highlight_rolloff(a: &Image, white: f64, knee: f64) -> Image {
    let mut out = a.as_standard_layout().into_owned();
    highlight_rolloff_in_place(&mut out, white, knee);
    out
}

fn highlight_rolloff_in_place(a: &mut Image, white: f64, knee: f64) {
    if white <= 1.0 {
        return;
    }
    pixels_mut(a, |p| p.iter_mut().for_each(|v| *v = rolloff_value(*v, white, knee)));
}

/// `s.get('tone_version', 1)`.
fn tone_version(s: &Settings) -> f64 {
    match s.0.get("tone_version") {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(1.0),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => 1.0,
    }
}

fn check(a: &ArrayView3<'_, f32>) -> Result<()> {
    match a.dim().2 {
        3 => Ok(()),
        n => Err(Error::Channels(n)),
    }
}

/// Port of luma/engine.py `_tone_encoded`: exposure, white balance, lens vignette lift and the highlight roll-off
/// (or the DRT) on linear pixels, then sRGB encoding. `film` is `film_look.render` for these settings when
/// `film_look.active(s)`; its print replaces the rendering, blended by `film_strength`.
pub fn tone_encoded(a: ArrayView3<'_, f32>, s: &Settings, film: Option<FilmRender<'_>>) -> Result<Image> {
    check(&a)?;
    let mut a = owned(a);
    let (height, width, channels) = a.dim();
    let exposure = 2f64.powf(s.f64("exposure")) as f32;
    let gain = white_balance(s);
    pixels_mut(&mut a, |p| {
        for (v, g) in p.iter_mut().zip(gain) {
            *v *= exposure;
            *v *= g;
        }
    });
    if s.bool("lens_vignette") {
        let lens = s.f64("lens_vignette");
        // np.ogrid[-1:1:complex(n)]: arange(n) * (2 / (n - 1)) - 1 in float64.
        let axis = |n: usize| -> Vec<f64> {
            let step = if n > 1 { 2.0 / (n - 1) as f64 } else { 1.0 };
            (0..n).map(|i| i as f64 * step - 1.0).collect()
        };
        let (ys, xs) = (axis(height), axis(width));
        if let Some(data) = a.as_slice_mut() {
            rows_mut(data, width * channels, |y, row| {
                let yy = ys[y] * ys[y];
                for (x, pixel) in row.chunks_exact_mut(channels).enumerate() {
                    let lift = (1.0 + (xs[x] * xs[x] + yy) * lens / 100.0).max(0.1);
                    pixel.iter_mut().for_each(|v| *v = (f64::from(*v) * lift) as f32);
                }
            });
        }
    }
    let film = film.map(|render| {
        let mut print = render(a.view());
        pixels_mut(&mut print, |p| p.iter_mut().for_each(|v| *v = to_srgb_value(*v)));
        print
    });
    let strength = s.f64("film_strength").clamp(0.0, 100.0) / 100.0;
    if let Some(print) = &film {
        if strength >= 1.0 {
            return Ok(print.clone());
        }
    }
    if mapping::active(s) {
        a = mapping::render(&a, s);
    } else if tone_version(s) >= 2.0 && !s.bool("hdr") {
        highlight_rolloff_in_place(&mut a, rolloff_white(s), SHOULDER_KNEE);
    }
    pixels_mut(&mut a, |p| p.iter_mut().for_each(|v| *v = to_srgb_value(*v)));
    if let Some(print) = film {
        let w = strength as f32;
        ndarray::Zip::from(&mut a).and(&print).par_for_each(|plain, f| *plain += (f - *plain) * w);
    }
    Ok(a)
}

/// Port of luma/engine.py `tone_base`: the whole-frame analysis of linear frame `a` for the process 3
/// Highlights/Shadows (reduced to `MAP_SIZE` on the long edge with `INTER_AREA`).
pub fn tone_base(a: &Image, s: &Settings, film: Option<FilmRender<'_>>) -> Result<ToneBase> {
    check(&a.view())?;
    let (height, width, _) = a.dim();
    let scale = MAP_SIZE as f64 / height.max(width) as f64;
    let small = if scale < 1.0 {
        let w = (round_half_even(width as f64 * scale) as usize).max(1);
        let h = (round_half_even(height as f64 * scale) as usize).max(1);
        resize_area(a, w, h).ok_or(Error::Resize)?
    } else {
        a.clone()
    };
    let encoded = tone_encoded(small.view(), s, film)?;
    Ok(response::analyse(&encoded, (height, width)))
}

fn uses_local(s: &Settings) -> bool {
    tone_version(s) >= 3.0 && !s.bool("hdr") && (s.bool("shadows") || s.bool("highlights"))
}

/// Port of luma/engine.py `tone_local`: what `tone_response.apply` needs for linear frame `a`; `None` when
/// process 3 Highlights/Shadows are not used. `base` is a previous `tone_base` of the same frame.
pub fn tone_local(
    a: &Image,
    s: &Settings,
    base: Option<&ToneBase>,
    film: Option<FilmRender<'_>>,
) -> Result<Option<ToneLocal>> {
    if !uses_local(s) {
        return Ok(None);
    }
    let base = match base {
        Some(base) => base.clone(),
        None => tone_base(a, s, film)?,
    };
    Ok(Some(response::with_curve(base, s.f64("highlights"), s.f64("shadows"))))
}

/// The point curve as float32 pairs, and whether it changes anything (`not np.allclose(x, y)`).
fn point_curve(s: &Settings) -> Result<Option<Points>> {
    let bad = || Error::Curve(s.value("curve").to_string());
    let list = s.value("curve").as_array().ok_or_else(bad)?;
    let mut points: Points = Vec::with_capacity(list.len());
    for item in list {
        let pair = item.as_array().filter(|p| p.len() == 2).ok_or_else(bad)?;
        let x = mapping::python_float(&pair[0]).ok_or_else(bad)?;
        let y = mapping::python_float(&pair[1]).ok_or_else(bad)?;
        points.push([x as f32, y as f32]);
    }
    if points.is_empty() {
        return Err(bad());
    }
    let (atol, rtol) = (1e-8_f64 as f32, 1e-5_f64 as f32);
    let close = points.iter().all(|&[x, y]| x == y || ((x - y).abs() <= atol + rtol * y.abs() && y.is_finite()));
    Ok((!close).then_some(points))
}

/// The settings `_develop_tone_pixels` reads after the encoding, read once.
struct Finish {
    black: f64,
    whites: Option<f64>,
    contrast: Option<f64>,
    curve: Option<(Points, Vec<f64>, Vec<f64>, f64)>,
    hdr: bool,
}

impl Finish {
    fn new(s: &Settings) -> Result<Self> {
        let curve = point_curve(s)?.map(|points| {
            let xs = points.iter().map(|p| f64::from(p[0])).collect();
            let ys = points.iter().map(|p| f64::from(p[1])).collect();
            let slope = extension_slope(&points);
            (points, xs, ys, slope)
        });
        Ok(Self {
            black: s.f64("blacks").clamp(-200.0, 200.0) / 250.0,
            whites: s.bool("whites").then(|| 2f64.powf(s.f64("whites").clamp(-200.0, 200.0) / 100.0)),
            contrast: s.bool("contrast").then(|| 1.0 + s.f64("contrast") / 125.0),
            curve,
            hdr: s.bool("hdr"),
        })
    }

    /// Blacks, Whites, Contrast, the point curve and the final clip of one value, in the dtype the oracle
    /// has at that point (float64 after process 3 Highlights/Shadows), as float32.
    #[inline]
    fn value<T: Float>(&self, mut v: T) -> f32 {
        let black = self.black;
        if black < 0.0 {
            v = maximum(v + T::of(black), T::ZERO) / T::of(1.0 + black);
        } else if black > 0.0 {
            v = v * T::of(1.0 - black) + T::of(black);
        }
        if let Some(gain) = self.whites {
            v = v * T::of(gain);
        }
        if let Some(factor) = self.contrast {
            v = (v - T::of(0.5)) * T::of(factor) + T::of(0.5);
        }
        match &self.curve {
            Some((points, xs, ys, slope)) => {
                let v = if self.hdr {
                    curve_extended_value(v, points, xs, ys, *slope)
                } else {
                    T::of(f64::from(interp(clip(v, T::ZERO, T::ONE).to_f64(), xs, ys) as f32))
                };
                self.clip(v)
            }
            None => self.clip(v),
        }
    }

    #[inline]
    fn clip<T: Float>(&self, v: T) -> f32 {
        if self.hdr {
            maximum(v, T::ZERO).to_f32()
        } else {
            clip(v, T::ZERO, T::ONE).to_f32()
        }
    }
}

/// Port of luma/engine.py `_develop_tone_pixels`: the tone stage on linear rows `first..` of a frame. `local` is
/// `tone_local()` of the whole frame; without it `a` is taken to be the whole frame.
pub fn develop_tone_pixels(
    a: ArrayView3<'_, f32>,
    s: &Settings,
    local: Option<&ToneLocal>,
    first: usize,
    film: Option<FilmRender<'_>>,
) -> Result<Image> {
    check(&a)?;
    let computed;
    let local = match local {
        Some(local) => Some(local),
        None => {
            computed = tone_local(&owned(a.view()), s, None, film)?;
            computed.as_ref()
        }
    };
    pixels_after_local(a, s, local, first, film)
}

fn pixels_after_local(
    a: ArrayView3<'_, f32>,
    s: &Settings,
    local: Option<&ToneLocal>,
    first: usize,
    film: Option<FilmRender<'_>>,
) -> Result<Image> {
    let finish = Finish::new(s)?;
    let mut encoded = tone_encoded(a, s, film)?;
    if let Some(local) = local {
        let applied = response::apply(encoded.view(), local, first);
        let mut out = Image::zeros(applied.raw_dim());
        ndarray::Zip::from(&mut out).and(&applied).par_for_each(|o, &v| *o = finish.value(v));
        return Ok(out);
    }
    let (shadows, highlights) = (s.f64("shadows"), s.f64("highlights"));
    let old = s.bool("shadows") || s.bool("highlights");
    let (sh, hl) = ((shadows / 180.0) as f32, (highlights / 180.0) as f32);
    pixels_mut(&mut encoded, |p| {
        if old {
            let l = luma(p);
            let low = clip(1.0 - l, 0.0, 1.0).powf(3.0);
            let high = clip(l, 0.0, 1.0).powf(3.0);
            let add = sh * low * clip(l * 4.0, 0.0, 1.0) + hl * high * clip((1.0 - l) * 4.0, 0.0, 1.0);
            p.iter_mut().for_each(|v| *v += add);
        }
        p.iter_mut().for_each(|v| *v = finish.value(*v));
    });
    Ok(encoded)
}

/// Port of luma/engine.py `_develop_tone` (CPU path): the tone stage of a linear frame, from the geometry result
/// to sRGB-encoded pixels for the colour stage. Without the lens vignette lift the frame is processed in the
/// oracle's row bands (`pixel_jobs.rows`), which is what a film render (`film`) sees.
pub fn develop_tone(a: &Image, s: &Settings, film: Option<FilmRender<'_>>) -> Result<Image> {
    check(&a.view())?;
    let local = tone_local(a, s, None, film)?;
    let (height, width, _) = a.dim();
    if s.bool("lens_vignette") {
        return pixels_after_local(a.view(), s, local.as_ref(), 0, film);
    }
    let bands = grainy_core::parallel::bands(height, width, |first, stop| {
        pixels_after_local(a.slice(ndarray::s![first..stop, .., ..]), s, local.as_ref(), first, film)
    });
    let mut bands: Vec<Image> = bands.into_iter().collect::<Result<_>>()?;
    if bands.len() <= 1 {
        return Ok(bands.pop().unwrap_or_else(|| Array3::zeros((0, width, 3))));
    }
    let views: Vec<_> = bands.iter().map(|b| b.view()).collect();
    concatenate(Axis(0), &views).map_err(|_| Error::Resize)
}
