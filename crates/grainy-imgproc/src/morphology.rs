use crate::array::{anchor, finish, generate, view};
use crate::border::sample;
use crate::{Border, Error, Pixel, Result};
use ndarray::{Array, Array2, Array3, Dimension};
use std::collections::VecDeque;

/// Port of cv2 MORPH_RECT, MORPH_CROSS, and MORPH_ELLIPSE kernel shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MorphShape {
    /// MORPH_RECT: every element is active.
    Rect,
    /// MORPH_CROSS: active anchor row and column.
    Cross,
    /// MORPH_ELLIPSE: OpenCV's rounded horizontal ellipse spans.
    Ellipse,
}

/// Port of cv2 morphologyEx operation flags (excluding tier-2 hit/miss segmentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Morphology {
    /// MORPH_ERODE.
    Erode,
    /// MORPH_DILATE.
    Dilate,
    /// MORPH_OPEN: erosion followed by dilation.
    Open,
    /// MORPH_CLOSE: dilation followed by erosion.
    Close,
    /// MORPH_GRADIENT: dilation minus erosion.
    Gradient,
    /// MORPH_TOPHAT: source minus opening.
    TopHat,
    /// MORPH_BLACKHAT: closing minus source.
    BlackHat,
}

/// Port of cv2.getStructuringElement. `size` is (width,height), anchor affects CROSS only.
pub fn get_structuring_element(
    kind: MorphShape,
    size: (usize, usize),
    at: Option<(usize, usize)>,
) -> Result<Array2<u8>> {
    let (ax, ay) = anchor(size, at)?;
    let (w, h) = size;
    let r = h / 2;
    let c = w / 2;
    let mut out = Array2::zeros((h, w));
    for y in 0..h {
        let extent = if r == 0 {
            0
        } else {
            let dy = y as f64 - r as f64;
            (c as f64 * (((r * r) as f64 - dy * dy) / ((r * r) as f64)).max(0.0).sqrt()).round_ties_even() as usize
        };
        for x in 0..w {
            out[[y, x]] = u8::from(match kind {
                MorphShape::Rect => true,
                MorphShape::Cross => x == ax || y == ay,
                MorphShape::Ellipse => x >= c.saturating_sub(extent) && x < (c + extent + 1).min(w),
            });
        }
    }
    Ok(out)
}

fn extrema<T: Pixel>(
    a: ndarray::ArrayView3<'_, T>,
    kernel: &Array2<u8>,
    at: (usize, usize),
    border: Border,
    dilate: bool,
) -> Array3<T> {
    let (h, w, c) = a.dim();
    let (kh, kw) = kernel.dim();
    if kernel.iter().all(|&v| v != 0) {
        let horizontal = line_extrema(a, kw, at.0, border, dilate, false);
        return line_extrema(horizontal.view(), kh, at.1, border, dilate, true);
    }
    let points: Vec<_> = kernel
        .indexed_iter()
        .filter_map(|((y, x), &v)| (v != 0).then_some((y as i64 - at.1 as i64, x as i64 - at.0 as i64)))
        .collect();
    generate(h, w, c, |y, x, ch| {
        let mut value = if dilate { -f32::MAX } else { f32::MAX };
        for &(dy, dx) in &points {
            let v = sample(a, y as i64 + dy, x as i64 + dx, ch, border);
            value = if dilate { value.max(v) } else { value.min(v) };
        }
        T::from_float(value)
    })
}

fn line_extrema<T: Pixel>(
    a: ndarray::ArrayView3<'_, T>,
    length: usize,
    anchor: usize,
    border: Border,
    dilate: bool,
    vertical: bool,
) -> Array3<T> {
    let (h, w, c) = a.dim();
    let lines = if vertical { w } else { h };
    let pixels = if vertical { h } else { w };
    let bands = grainy_core::parallel::bands(lines, pixels, |first, last| {
        let mut out = Array3::default((last - first, pixels, c));
        for line in first..last {
            for ch in 0..c {
                let mut queue: VecDeque<(usize, f32)> = VecDeque::new();
                for i in 0..pixels + length - 1 {
                    let p = i as i64 - anchor as i64;
                    let v = if vertical {
                        sample(a, p, line as i64, ch, border)
                    } else {
                        sample(a, line as i64, p, ch, border)
                    };
                    while queue.back().is_some_and(|&(_, old)| if dilate { old <= v } else { old >= v }) {
                        queue.pop_back();
                    }
                    queue.push_back((i, v));
                    if i + 1 >= length {
                        let start = i + 1 - length;
                        while queue.front().is_some_and(|&(p, _)| p < start) {
                            queue.pop_front();
                        }
                        if let Some(&(_, v)) = queue.front() {
                            out[[line - first, start, ch]] = T::from_float(v);
                        }
                    }
                }
            }
        }
        out
    });
    let mut out = Array3::default((h, w, c));
    let mut first = 0;
    for band in bands {
        for ((line, p, ch), &v) in band.indexed_iter() {
            if vertical {
                out[[p, line + first, ch]] = v;
            } else {
                out[[line + first, p, ch]] = v;
            }
        }
        first += band.shape()[0];
    }
    out
}

fn run<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    kernel: &Array2<u8>,
    at: Option<(usize, usize)>,
    iterations: usize,
    border: Option<Border>,
    dilate: bool,
) -> Result<Array<T, D>> {
    let a = view(src)?;
    let at = anchor((kernel.shape()[1], kernel.shape()[0]), at)?;
    if kernel.iter().all(|&v| v == 0) {
        return Err(Error::Argument("morphology kernel must have a nonzero element"));
    }
    if iterations > i32::MAX as usize {
        return Err(Error::Argument("too many morphology iterations"));
    }
    if iterations == 0 {
        return Ok(src.clone());
    }
    let border = border.unwrap_or(Border::Constant(if dilate { -f32::MAX } else { f32::MAX }));
    let mut out = extrema(a, kernel, at, border, dilate);
    for _ in 1..iterations {
        out = extrema(out.view(), kernel, at, border, dilate);
    }
    finish(out, src.ndim())
}

/// Port of cv2.dilate on float32/uint8; None border is morphologyDefaultBorderValue.
pub fn dilate<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    kernel: &Array2<u8>,
    at: Option<(usize, usize)>,
    iterations: usize,
    border: Option<Border>,
) -> Result<Array<T, D>> {
    run(src, kernel, at, iterations, border, true)
}

/// Port of cv2.erode on float32/uint8; None border is morphologyDefaultBorderValue.
pub fn erode<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    kernel: &Array2<u8>,
    at: Option<(usize, usize)>,
    iterations: usize,
    border: Option<Border>,
) -> Result<Array<T, D>> {
    run(src, kernel, at, iterations, border, false)
}

/// Port of cv2.morphologyEx on float32/uint8, preserving saturating uint8 subtraction.
pub fn morphology_ex<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    op: Morphology,
    kernel: &Array2<u8>,
    at: Option<(usize, usize)>,
    iterations: usize,
    border: Option<Border>,
) -> Result<Array<T, D>> {
    let erode = |a: &Array<T, D>| erode(a, kernel, at, iterations, border);
    let dilate = |a: &Array<T, D>| dilate(a, kernel, at, iterations, border);
    let subtract = |a: Array<T, D>, b: &Array<T, D>| {
        let mut out = a;
        for (v, &b) in out.iter_mut().zip(b) {
            *v = T::from_float(v.float() - b.float());
        }
        Ok(out)
    };
    match op {
        Morphology::Erode => erode(src),
        Morphology::Dilate => dilate(src),
        Morphology::Open => dilate(&erode(src)?),
        Morphology::Close => erode(&dilate(src)?),
        Morphology::Gradient => subtract(dilate(src)?, &erode(src)?),
        Morphology::TopHat => subtract(src.clone(), &dilate(&erode(src)?)?),
        Morphology::BlackHat => subtract(erode(&dilate(src)?)?, src),
    }
}
