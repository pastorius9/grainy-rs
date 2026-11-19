use ndarray::{Array, Array1, Array3, Dimension};

use crate::array::{anchor, finish, generate, view};
use crate::border::{constant, index, sample};
use crate::{Border, Error, Pixel, Result};

/// Port of cv2.getGaussianKernel with CV_64F; includes OpenCV's small sigma<=0 kernels.
pub fn get_gaussian_kernel_f64(size: usize, sigma: f64) -> Result<Array1<f64>> {
    if size == 0 || size > i32::MAX as usize || !sigma.is_finite() {
        return Err(Error::Argument("kernel size must be positive and sigma finite"));
    }
    let fixed: &[f64] = if sigma <= 0.0 {
        match size {
            1 => &[1.0],
            3 => &[0.25, 0.5, 0.25],
            5 => &[0.0625, 0.25, 0.375, 0.25, 0.0625],
            7 => &[0.03125, 0.109375, 0.21875, 0.28125, 0.21875, 0.109375, 0.03125],
            9 => &[
                4.0 / 256.0,
                13.0 / 256.0,
                30.0 / 256.0,
                51.0 / 256.0,
                60.0 / 256.0,
                51.0 / 256.0,
                30.0 / 256.0,
                13.0 / 256.0,
                4.0 / 256.0,
            ],
            _ => &[],
        }
    } else {
        &[]
    };
    if !fixed.is_empty() {
        return Ok(Array1::from_vec(fixed.to_vec()));
    }
    let sigma = if sigma > 0.0 { sigma } else { size as f64 * 0.15 + 0.35 };
    let center = (size - 1) as f64 * 0.5;
    let mut values = Array1::from_shape_fn(size, |i| {
        let x = i as f64 - center;
        if x == 0.0 || (size.is_multiple_of(2) && (i == size / 2 || i + 1 == size / 2)) {
            1.0
        } else {
            (-0.5 * x * x / (sigma * sigma)).exp()
        }
    });
    let sum: f64 = values.sum();
    values.mapv_inplace(|x| x / sum);
    Ok(values)
}

/// Port of cv2.getGaussianKernel(..., cv2.CV_32F), used by luma/native_gpu.py.
pub fn get_gaussian_kernel(size: usize, sigma: f64) -> Result<Array1<f32>> {
    Ok(get_gaussian_kernel_f64(size, sigma)?.mapv(|x| x as f32))
}

fn filter_border(border: Border) -> Result<()> {
    if border == Border::Wrap {
        return Err(Error::Argument("OpenCV filters do not support BORDER_WRAP"));
    }
    Ok(())
}

/// Port of cv2.sepFilter2D with CV_32F input/output and correlation (kernels are not reversed).
/// `anchor=None` means the kernel center; delta is added after the column pass.
pub fn sep_filter_2d<D: Dimension>(
    src: &Array<f32, D>,
    kernel_x: &[f32],
    kernel_y: &[f32],
    at: Option<(usize, usize)>,
    delta: f64,
    border: Border,
) -> Result<Array<f32, D>> {
    sep_filter_2d_impl(src, kernel_x, kernel_y, at, delta, border, true)
}

/// Port of cv2.sepFilter2D with CV_32F input/output and correlation (kernels are not reversed).
/// `anchor=None` means the kernel center; delta is added after the column pass.
fn sep_filter_2d_impl<D: Dimension>(
    src: &Array<f32, D>,
    kernel_x: &[f32],
    kernel_y: &[f32],
    at: Option<(usize, usize)>,
    delta: f64,
    border: Border,
    exact: bool,
) -> Result<Array<f32, D>> {
    let src3 = view(src)?;
    filter_border(border)?;
    let (ax, ay) = anchor((kernel_x.len(), kernel_y.len()), at)?;
    if !delta.is_finite() || kernel_x.iter().chain(kernel_y).any(|x| !x.is_finite()) {
        return Err(Error::Argument("finite kernels and delta required"));
    }
    let (h, w, c) = src3.dim();
    let symmetric_x = symmetry(kernel_x, ax);
    let symmetric_y = symmetry(kernel_y, ay);
    if exact && symmetric_x == 1 && symmetric_y == 1 && border == Border::Reflect101 && delta == 0.0 {
        let mat = crate::exact::Mat::from_image(&src3.to_owned());
        return finish(crate::exact::separable(&mat, kernel_x, kernel_y).into_image(), src.ndim());
    }
    let horizontal = generate(h, w, c, |y, x, c| {
        dot(kernel_x, ax, symmetric_x, |i| sample(src3, y as i64, x as i64 + i as i64 - ax as i64, c, border))
    });
    let sum_x: f32 = kernel_x.iter().sum();
    let out = generate(h, w, c, |y, x, c| {
        dot(kernel_y, ay, symmetric_y, |i| {
            index(y as i64 + i as i64 - ay as i64, h, border)
                .map_or(constant(border, c) * sum_x, |j| horizontal[[j, x, c]])
        }) + delta as f32
    });
    finish(out, src.ndim())
}

fn symmetry(k: &[f32], anchor: usize) -> i8 {
    let centered = k.len() % 2 == 1 && anchor == k.len() / 2;
    if centered && k.iter().eq(k.iter().rev()) {
        1
    } else if centered && k.iter().zip(k.iter().rev()).all(|(&a, &b)| a == -b) {
        -1
    } else {
        0
    }
}

fn dot<F: Fn(usize) -> f32>(k: &[f32], anchor: usize, symmetry: i8, get: F) -> f32 {
    // OpenCV's symmetric filter path accumulates pairs from the center outward.
    if symmetry == 1 {
        let mut value = get(anchor) * k[anchor];
        for i in 1..=anchor {
            value += (get(anchor - i) + get(anchor + i)) * k[anchor + i];
        }
        value
    } else if symmetry == -1 {
        let mut value = 0.0;
        for i in 1..=anchor {
            value += (get(anchor + i) - get(anchor - i)) * k[anchor + i];
        }
        value
    } else {
        let mut value = 0.0;
        for (i, &weight) in k.iter().enumerate() {
            value += get(i) * weight;
        }
        value
    }
}

/// Port of cv2.GaussianBlur for float32 planes/images. Zero kernel dimensions use
/// cvRound(sigma*8+1)|1; sigma_y<=0 inherits sigma_x; nonpositive sigma uses size.
pub fn gaussian_blur<D: Dimension>(
    src: &Array<f32, D>,
    size: (usize, usize),
    sigma_x: f64,
    sigma_y: f64,
    border: Border,
) -> Result<Array<f32, D>> {
    gaussian_blur_impl(src, size, sigma_x, sigma_y, border, true)
}

/// Port of cv2.GaussianBlur for float32 planes/images. Zero kernel dimensions use
/// cvRound(sigma*8+1)|1; sigma_y<=0 inherits sigma_x; nonpositive sigma uses size.
fn gaussian_blur_impl<D: Dimension>(
    src: &Array<f32, D>,
    size: (usize, usize),
    sigma_x: f64,
    sigma_y: f64,
    border: Border,
    exact: bool,
) -> Result<Array<f32, D>> {
    let a = view(src)?;
    if !sigma_x.is_finite() || !sigma_y.is_finite() {
        return Err(Error::Argument("finite Gaussian sigma required"));
    }
    filter_border(border)?;
    let sy = if sigma_y <= 0.0 { sigma_x } else { sigma_y };
    let resolve = |n: usize, s: f64| -> Result<usize> {
        let n = if n == 0 {
            if s <= 0.0 || s * 8.0 + 1.0 >= i32::MAX as f64 {
                return Err(Error::Argument("positive bounded sigma required for automatic kernel"));
            }
            ((s * 8.0 + 1.0).round_ties_even() as usize) | 1
        } else {
            n
        };
        if n % 2 == 0 {
            return Err(Error::Argument("GaussianBlur kernel dimensions must be odd"));
        }
        Ok(n)
    };
    let mut kw = resolve(size.0, sigma_x)?;
    let mut kh = resolve(size.1, sy)?;
    if !matches!(border, Border::Constant(_) | Border::ConstantChannels(_)) {
        if a.shape()[1] == 1 {
            kw = 1;
        }
        if a.shape()[0] == 1 {
            kh = 1;
        }
    }
    if kw == 1 && kh == 1 {
        return Ok(src.clone());
    }
    let kx = get_gaussian_kernel(kw, sigma_x)?;
    let ky = get_gaussian_kernel(kh, sy)?;
    sep_filter_2d_impl(src, &kx.to_vec(), &ky.to_vec(), None, 0.0, border, exact)
}

/// Port of cv2.boxFilter(..., ddepth=-1) on float32, including unnormalized sums
/// and the noncentral (0,0) anchor used by luma/dust_texture.py.
pub fn box_filter<D: Dimension>(
    src: &Array<f32, D>,
    size: (usize, usize),
    at: Option<(usize, usize)>,
    normalize: bool,
    border: Border,
) -> Result<Array<f32, D>> {
    let _timing = grainy_core::timing::Span::new("imgproc.box_filter");
    let a = view(src)?;
    filter_border(border)?;
    let (ax, ay) = anchor(size, at)?;
    let (h, w, c) = a.dim();
    // Double precision sliding sums are the CV_32F box-filter accumulator.
    let bands = grainy_core::parallel::bands(h, w, |first, last| {
        let mut row = Array3::<f64>::zeros((last - first, w, c));
        for y in first..last {
            for ch in 0..c {
                let mut sum = 0.0;
                for k in 0..size.0 {
                    sum += sample(a, y as i64, k as i64 - ax as i64, ch, border) as f64;
                }
                for x in 0..w {
                    if size.0 == 3 || size.0 == 5 {
                        sum = (0..size.0)
                            .map(|k| f64::from(sample(a, y as i64, x as i64 + k as i64 - ax as i64, ch, border)))
                            .sum();
                    }
                    row[[y - first, x, ch]] = sum;
                    sum += sample(a, y as i64, x as i64 + size.0 as i64 - ax as i64, ch, border) as f64
                        - sample(a, y as i64, x as i64 - ax as i64, ch, border) as f64;
                }
            }
        }
        row
    });
    let mut horizontal = Array3::<f64>::zeros((h, w, c));
    let mut first = 0;
    for band in bands {
        let last = first + band.shape()[0];
        horizontal.slice_mut(ndarray::s![first..last, .., ..]).assign(&band);
        first = last;
    }
    let scale = if normalize { 1.0 / (size.0 as f64 * size.1 as f64) } else { 1.0 };
    let bands = grainy_core::parallel::bands(w, h, |first, last| {
        let mut out = Array3::<f32>::zeros((h, last - first, c));
        let mut sums = vec![0.0; (last - first) * c];
        let get =
            |y: Option<usize>, x, ch| y.map_or(constant(border, ch) as f64 * size.0 as f64, |y| horizontal[[y, x, ch]]);
        // Keep the original top-to-bottom sum order for every column, while
        // visiting neighboring columns together instead of striding a full frame.
        for k in 0..size.1 - 1 {
            let y = index(k as i64 - ay as i64, h, border);
            for x in first..last {
                for ch in 0..c {
                    sums[(x - first) * c + ch] += get(y, x, ch);
                }
            }
        }
        for y in 0..h {
            let add = index(y as i64 + size.1 as i64 - 1 - ay as i64, h, border);
            let remove = index(y as i64 - ay as i64, h, border);
            for x in first..last {
                for ch in 0..c {
                    let sum = &mut sums[(x - first) * c + ch];
                    let total = *sum + get(add, x, ch);
                    out[[y, x - first, ch]] = (total * scale) as f32;
                    *sum = total - get(remove, x, ch);
                }
            }
        }
        out
    });
    let mut out = Array3::zeros((h, w, c));
    let mut first = 0;
    for band in bands {
        let last = first + band.shape()[1];
        out.slice_mut(ndarray::s![.., first..last, ..]).assign(&band);
        first = last;
    }
    finish(out, src.ndim())
}

/// Port of cv2.blur: normalized CV_32F box filtering.
pub fn blur<D: Dimension>(
    src: &Array<f32, D>,
    size: (usize, usize),
    at: Option<(usize, usize)>,
    border: Border,
) -> Result<Array<f32, D>> {
    box_filter(src, size, at, true, border)
}

/// Port of cv2.medianBlur with BORDER_REPLICATE. CV_32F supports sizes 1,3,5;
/// CV_8U supports positive odd sizes.
pub fn median_blur<T: Pixel, D: Dimension>(src: &Array<T, D>, size: usize) -> Result<Array<T, D>> {
    let a = view(src)?;
    if size == 0 || size.is_multiple_of(2) || size > 255 || (!T::IS_U8 && size > 5) {
        return Err(Error::Argument("median size must be odd; float32 supports at most 5, uint8 at most 255"));
    }
    if size == 1 {
        return Ok(src.clone());
    }
    let (h, w, c) = a.dim();
    let radius = (size / 2) as i64;
    let out = generate(h, w, c, |y, x, ch| {
        let mut small = [0.0f32; 25];
        let mut large = if size > 5 { vec![0.0; size * size] } else { Vec::new() };
        let values = if size <= 5 { &mut small[..size * size] } else { &mut large[..] };
        let mut index = 0;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                values[index] = sample(a, y as i64 + dy, x as i64 + dx, ch, Border::Replicate);
                index += 1;
            }
        }
        let middle = values.len() / 2;
        values.select_nth_unstable_by(middle, f32::total_cmp);
        T::from_float(values[middle])
    });
    finish(out, src.ndim())
}

fn derivative(size: usize, order: usize) -> Vec<f32> {
    let mut k = vec![1.0];
    for i in 0..size - 1 {
        let mut next = vec![0.0; k.len() + 1];
        for (j, &v) in k.iter().enumerate() {
            next[j] += if i < order { -v } else { v };
            next[j + 1] += v;
        }
        k = next;
    }
    k
}

/// Port of cv2.Sobel with CV_32F output, odd sizes 1..31, scale and delta.
pub fn sobel<D: Dimension>(
    src: &Array<f32, D>,
    dx: usize,
    dy: usize,
    size: usize,
    scale: f64,
    delta: f64,
    border: Border,
) -> Result<Array<f32, D>> {
    if size == 0
        || size.is_multiple_of(2)
        || size > 31
        || (dx == 0 && dy == 0)
        || dx >= size.max(3)
        || dy >= size.max(3)
        || !scale.is_finite()
    {
        return Err(Error::Argument("invalid Sobel derivative order, size, or scale"));
    }
    if size == 3 && dx + dy == 1 && border == Border::Reflect101 && delta == 0.0 {
        let a = view(src)?;
        let mut out = a.to_owned();
        if scale == 1.0 {
            for c in 0..a.dim().2 {
                let p = a.index_axis(ndarray::Axis(2), c).to_owned();
                let p = if dx == 1 { crate::exact::deriv::sobel_x(&p) } else { crate::exact::deriv::sobel_y(&p) };
                out.index_axis_mut(ndarray::Axis(2), c).assign(&p);
            }
        } else {
            out = crate::exact::scaled_deriv::sobel(&crate::exact::Mat::from_image(&out), dx == 1, scale).into_image();
        }
        return finish(out, src.ndim());
    }
    let mut kx = derivative(if size == 1 && dx > 0 { 3 } else { size }, dx);
    let mut ky = derivative(if size == 1 && dy > 0 { 3 } else { size }, dy);
    let scaled = if dx == 0 { &mut kx } else { &mut ky };
    for v in scaled {
        *v = (*v as f64 * scale) as f32;
    }
    sep_filter_2d(src, &kx, &ky, None, delta, border)
}

/// Port of cv2.integral on float32 with CV_64F output, including the leading zero row/column.
pub fn integral<D: Dimension>(src: &Array<f32, D>) -> Result<Array<f64, D>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    crate::array::shape(h + 1, w + 1, c)?;
    let mut out = Array3::<f64>::zeros((h + 1, w + 1, c));
    for y in 0..h {
        for ch in 0..c {
            let mut sum = 0.0;
            for x in 0..w {
                sum += a[[y, x, ch]] as f64;
                out[[y + 1, x + 1, ch]] = out[[y, x + 1, ch]] + sum;
            }
        }
    }
    finish(out, src.ndim())
}

/// Port of cv2.integral(..., sdepth=cv2.CV_32S) on uint8, as in luma/dust_texture.py.
/// Accumulation wraps at 32 bits, matching OpenCV's signed integral buffer.
pub fn integral_u8<D: Dimension>(src: &Array<u8, D>) -> Result<Array<i32, D>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    crate::array::shape(h + 1, w + 1, c)?;
    let mut out = Array3::<i32>::zeros((h + 1, w + 1, c));
    for y in 0..h {
        for ch in 0..c {
            let mut sum = 0i32;
            for x in 0..w {
                sum = sum.wrapping_add(a[[y, x, ch]] as i32);
                out[[y + 1, x + 1, ch]] = out[[y, x + 1, ch]].wrapping_add(sum);
            }
        }
    }
    finish(out, src.ndim())
}

/// Port of cv2.magnitude on float32 (sqrt(x*x+y*y), not rescaled hypot).
pub fn magnitude<D: Dimension>(x: &Array<f32, D>, y: &Array<f32, D>) -> Result<Array<f32, D>> {
    let a = view(x)?;
    let b = view(y)?;
    if a.dim() != b.dim() {
        return Err(Error::Shape("magnitude inputs must have equal shapes"));
    }
    let (h, w, c) = a.dim();
    finish(
        generate(h, w, c, |j, i, ch| (a[[j, i, ch]] * a[[j, i, ch]] + b[[j, i, ch]] * b[[j, i, ch]]).sqrt()),
        x.ndim(),
    )
}

/// Port of cv2.GaussianBlur with the historical unfused accumulation.
/// Kept only for dust texture donor selection: switching rounding changes its frozen repair results.
pub fn gaussian_blur_unfused<D: Dimension>(
    src: &Array<f32, D>,
    size: (usize, usize),
    sigma_x: f64,
    sigma_y: f64,
    border: Border,
) -> Result<Array<f32, D>> {
    gaussian_blur_impl(src, size, sigma_x, sigma_y, border, false)
}

/// `cv2.magnitude(x, y)` as OpenCV computes it without IPP (see the module notes).
pub fn magnitude_values(x: &[f32], y: &[f32]) -> Vec<f32> {
    x.iter().zip(y).map(|(&a, &b)| a.mul_add(a, b * b).sqrt()).collect()
}
