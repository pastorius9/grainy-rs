use crate::{invalid, validate, Result};
use grainy_core::{parallel, Image, Plane};
use ndarray::{parallel::prelude::*, Array3, Axis};
use std::sync::OnceLock;

/// Port of `engine.export_longest`'s integer or (width, height, fit) argument.
#[derive(Clone, Copy, Debug)]
pub enum SizeLimit {
    /// Longest side, without upscaling.
    Longest(usize),
    /// Box; zero means no limit on that axis, cover selects the larger scale.
    Box { width: usize, height: usize, cover: bool },
}

/// Port of `luma/engine.py export_longest`.
pub fn export_longest(shape: (usize, usize), limit: SizeLimit) -> Result<usize> {
    let (h, w) = shape;
    if h == 0 || w == 0 {
        return Err(invalid("empty image"));
    }
    match limit {
        SizeLimit::Longest(n) if n > 0 => Ok(n),
        SizeLimit::Longest(_) => Err(invalid("zero resize limit")),
        SizeLimit::Box { width, height, cover } => {
            let factors: Vec<f64> = [(width, w), (height, h)]
                .into_iter()
                .filter(|(n, _)| *n != 0)
                .map(|(n, s)| n as f64 / s as f64)
                .collect();
            if factors.is_empty() {
                return Ok(w.max(h));
            }
            let f = factors.into_iter().reduce(|a, b| if cover { a.max(b) } else { a.min(b) }).unwrap_or(1.);
            if f >= 1. {
                return Ok(w.max(h));
            }
            let side = w.max(h) as f64 * f;
            Ok((if cover { (side - 1e-6).ceil() } else { (side + 1e-6).floor() }).max(1.) as usize)
        }
    }
}

fn coefficients(input: usize, output: usize, lanczos: bool) -> Vec<(usize, Vec<f64>)> {
    let scale = input as f64 / output as f64;
    let filterscale = scale.max(1.);
    let support = if lanczos { 3. } else { 1. } * filterscale;
    (0..output)
        .map(|i| {
            let center = (i as f64 + 0.5) * scale;
            let start = ((center - support + 0.5) as isize).max(0) as usize;
            let end = ((center + support + 0.5) as usize).min(input);
            let mut weights: Vec<f64> = (start..end)
                .map(|x| {
                    let t = (x as f64 - center + 0.5) / filterscale;
                    if lanczos {
                        let sinc = |v: f64| {
                            if v == 0. {
                                1.
                            } else {
                                let v = v * std::f64::consts::PI;
                                v.sin() / v
                            }
                        };
                        if (-3. ..3.).contains(&t) {
                            sinc(t) * sinc(t / 3.)
                        } else {
                            0.
                        }
                    } else {
                        (1. - t.abs()).max(0.)
                    }
                })
                .collect();
            let total: f64 = weights.iter().sum();
            if total != 0. {
                for w in &mut weights {
                    *w /= total;
                }
            }
            (start, weights)
        })
        .collect()
}

fn resize(a: &Image, longest: usize, lanczos: bool, hdr: bool) -> Result<Image> {
    let _timing = grainy_core::timing::Span::new(if lanczos { "export.resize" } else { "load.resize" });
    validate(a)?;
    if longest == 0 {
        return Err(invalid("zero resize limit"));
    }
    let (h, w, c) = a.dim();
    if h.max(w) <= longest {
        return Ok(a.clone());
    }
    let factor = longest as f64 / h.max(w) as f64;
    let ow = (w as f64 * factor).round_ties_even().max(1.) as usize;
    let oh = (h as f64 * factor).round_ties_even().max(1.) as usize;
    // Pillow's float resampler accumulates in double and rounds to float after each separable pass.
    let horizontal = coefficients(w, ow, lanczos);
    let vertical = coefficients(h, oh, lanczos);
    let mut temp = Image::zeros((h, ow, c));
    temp.axis_iter_mut(Axis(0)).into_par_iter().zip(a.axis_iter(Axis(0)).into_par_iter()).for_each(|(mut out, row)| {
        for (x, (start, ws)) in horizontal.iter().enumerate() {
            for k in 0..c {
                let mut v = 0.;
                for (i, weight) in ws.iter().enumerate() {
                    v += row[[start + i, k]] as f64 * weight;
                }
                out[[x, k]] = v as f32;
            }
        }
    });
    let bands = parallel::bands(oh, ow, |first, last| {
        let mut out = Vec::with_capacity((last - first) * ow * c);
        for (start, ws) in &vertical[first..last] {
            for x in 0..ow {
                for k in 0..c {
                    let mut v = 0.;
                    for (i, weight) in ws.iter().enumerate() {
                        v += temp[[start + i, x, k]] as f64 * weight;
                    }
                    let v = v as f32;
                    out.push(if lanczos {
                        if hdr {
                            v.max(0.)
                        } else {
                            v.clamp(0., 1.)
                        }
                    } else {
                        v
                    });
                }
            }
        }
        out
    });
    Array3::from_shape_vec((oh, ow, c), bands.into_iter().flatten().collect()).map_err(crate::codec)
}

/// Port of `luma/engine.py resize_float`: Pillow float bilinear, identity when no reduction is needed.
pub fn resize_float(a: &Image, longest: usize) -> Result<Image> {
    resize(a, longest, false, false)
}
/// Port of `luma/engine.py resize_export`: Pillow float Lanczos-3, clipped only when resized.
pub fn resize_export(a: &Image, longest: usize, hdr: bool) -> Result<Image> {
    resize(a, longest, true, hdr)
}

/// Port of `luma/engine.py _dither_tile`: NumPy PCG64 seed 1905, float32 TPDF, 256×256.
pub fn dither_tile() -> &'static Plane {
    static TILE: OnceLock<Plane> = OnceLock::new();
    TILE.get_or_init(|| {
        // SeedSequence(1905) expanded state, verified by gen_codecs.py against NumPy.
        let mut state: u128 = 148955313848161859251057375211882031232;
        let inc: u128 = 202992800449934853642521897101836953157;
        let mut pending = None;
        let mut random = || {
            let word = if let Some(v) = pending.take() {
                v
            } else {
                state = state.wrapping_mul(47026247687942121848144207491837523525).wrapping_add(inc);
                let hi = (state >> 64) as u64;
                let raw = (hi ^ state as u64).rotate_right((hi >> 58) as u32);
                pending = Some((raw >> 32) as u32);
                raw as u32
            };
            (word >> 8) as f32 * (1.0 / 16777216.0)
        };
        let mut tile = Plane::from_shape_fn((256, 256), |_| random());
        for v in &mut tile {
            *v -= random();
        }
        tile
    })
}

/// Port of `luma/engine.py _quantize8_dithered`, exact float32 operations and truncating cast.
pub fn quantize8_dithered(a: &Image) -> Result<Array3<u8>> {
    validate(a)?;
    let tile = dither_tile();
    quantize_rows(a, |y, x, v| {
        let noise = if v <= 0. || v >= 1. { 0. } else { tile[[y % 256, x % 256]] };
        let scaled = v * 255.;
        let scaled = scaled + 0.5;
        let scaled = scaled + noise;
        scaled.clamp(0., 255.5) as u8
    })
}
fn quantize_rows<T: Send>(a: &Image, f: impl Fn(usize, usize, f32) -> T + Sync) -> Result<Array3<T>> {
    let (h, w, c) = a.dim();
    let bands = parallel::bands(h, w, |first, last| {
        let mut out = Vec::with_capacity((last - first) * w * c);
        for y in first..last {
            for x in 0..w {
                for k in 0..c {
                    out.push(f(y, x, a[[y, x, k]]));
                }
            }
        }
        out
    });
    Array3::from_shape_vec((h, w, c), bands.into_iter().flatten().collect()).map_err(crate::codec)
}
// NumPy's integer conversion wraps instead of Rust's saturating float-to-unsigned cast.
fn wrapped(v: f32, modulus: f64) -> u32 {
    ((v as f64).trunc().rem_euclid(modulus)) as u32
}
/// Port of `luma/engine.py _quantize(result, 255, uint8)`. Keeps the caller's pixels immutable.
pub fn quantize8(a: &Image) -> Result<Array3<u8>> {
    validate(a)?;
    quantize_rows(a, |_, _, v| wrapped(v * 255. + 0.5, 256.) as u8)
}
/// Port of `luma/engine.py _quantize(result, 65535, uint16)`. Keeps the caller's pixels immutable.
pub fn quantize16(a: &Image) -> Result<Array3<u16>> {
    validate(a)?;
    quantize_rows(a, |_, _, v| wrapped(v * 65535. + 0.5, 65536.) as u16)
}

/// Port of `luma/engine.py _neutral_gray`: select green only if every channel is neutral within tolerance.
pub fn neutral_gray(a: &Image, tolerance: f32) -> Option<Image> {
    let (h, w, c) = a.dim();
    if c != 3 || h == 0 || w == 0 || !tolerance.is_finite() || tolerance < 0. {
        return None;
    }
    let mut spread = 0f32;
    for row in a.outer_iter() {
        for p in row.outer_iter() {
            let r = (p[0] - p[1]).abs();
            let b = (p[2] - p[1]).abs();
            if !r.is_finite() || !b.is_finite() {
                return None;
            }
            spread = spread.max(r).max(b);
        }
    }
    if spread > tolerance {
        None
    } else {
        Some(Image::from_shape_fn((h, w, 1), |(y, x, _)| a[[y, x, 1]]))
    }
}

/// Port of `luma/engine.py jxl_distance` (including out-of-range finite qualities).
pub fn jxl_distance(quality: f64) -> Result<f64> {
    if !quality.is_finite() {
        return Err(invalid("non-finite quality"));
    }
    Ok(if quality >= 30. { 0.1 + (100. - quality) * 0.09 } else { 6.4 + (30. - quality).powi(2) * 2.5 / 900. })
}

pub(crate) fn box_bytes(kind: &[u8; 4], payload: &[u8]) -> Result<Vec<u8>> {
    let len =
        u32::try_from(payload.len()).ok().and_then(|n| n.checked_add(8)).ok_or_else(|| invalid("box exceeds 4 GiB"))?;
    let mut out = len.to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(payload);
    Ok(out)
}
/// Port of `luma/engine.py _jxl_container`: bare codestream or exact Exif/XML BMFF boxes.
pub fn jxl_container(codestream: &[u8], exif: Option<&[u8]>, xmp: Option<&[u8]>) -> Result<Vec<u8>> {
    let exif = exif.filter(|v| !v.is_empty());
    let xmp = xmp.filter(|v| !v.is_empty());
    if exif.is_none() && xmp.is_none() {
        return Ok(codestream.to_vec());
    }
    let mut out = b"\0\0\0\x0cJXL \r\n\x87\n".to_vec();
    out.extend(box_bytes(b"ftyp", b"jxl \0\0\0\0jxl ")?);
    if let Some(data) = exif {
        let mut p = vec![0; 4];
        p.extend(data.strip_prefix(b"Exif\0\0").unwrap_or(data));
        out.extend(box_bytes(b"Exif", &p)?);
    }
    if let Some(data) = xmp {
        out.extend(box_bytes(b"xml ", data)?);
    }
    out.extend(box_bytes(b"jxlc", codestream)?);
    Ok(out)
}
