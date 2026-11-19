//! The Pillow operations `photo_images` uses on 8-bit RGB images, ported from Pillow 12's C code: the fixed-point
//! convolution resampler (`ImagingResample` with the Lanczos filter, `PRECISION_BITS` 22), `Image.reduce`
//! (`ImagingReduce`), `Image.thumbnail` (aspect rounding and `reducing_gap`), `ImageDraw.rectangle` and `paste`.
//! Exact: the same integer arithmetic as the C code.

use ndarray::Array3;

/// An RGB image: H×W×3 bytes.
pub type Rgb = Array3<u8>;

const PRECISION_BITS: u32 = 32 - 8 - 2;
const SUPPORT: f64 = 3.0;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

fn lanczos(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// `precompute_coeffs` + `normalize_coeffs_8bpc`: per output pixel `(xmin, count)` and the integer weights.
fn coefficients(in_size: usize, in0: f32, in1: f32, out_size: usize) -> (usize, Vec<(usize, usize)>, Vec<i32>) {
    let scale = f64::from(in1 - in0) / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = SUPPORT * filterscale;
    let ksize = (support.ceil() as usize) * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0.0f64; out_size * ksize];
    for xx in 0..out_size {
        let center = f64::from(in0) + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0);
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) - xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        let mut ww = 0.0;
        for (x, slot) in k.iter_mut().enumerate().take(xmax.max(0) as usize) {
            let w = lanczos((x as f64 + xmin as f64 - center + 0.5) * ss);
            *slot = w;
            ww += w;
        }
        for value in k.iter_mut().take(xmax.max(0) as usize) {
            if ww != 0.0 {
                *value /= ww;
            }
        }
        bounds.push((xmin as usize, xmax.max(0) as usize));
    }
    let scale = f64::from(1u32 << PRECISION_BITS);
    let ints = kk.iter().map(|&v| if v < 0.0 { (-0.5 + v * scale) as i32 } else { (0.5 + v * scale) as i32 }).collect();
    (ksize, bounds, ints)
}

fn clip8(value: i32) -> u8 {
    (value >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Port of `ImagingResample(im, (xsize, ysize), LANCZOS, box)` for 8-bit RGB.
pub fn resample(image: &Rgb, width: usize, height: usize, area: [f32; 4]) -> Rgb {
    let (in_h, in_w, _) = image.dim();
    let need_horizontal = width as f32 != in_w as f32 || area[0] != 0.0 || area[2] != width as f32;
    let need_vertical = height as f32 != in_h as f32 || area[1] != 0.0 || area[3] != height as f32;
    let (kh, bounds_h, kk_h) = coefficients(in_w, area[0], area[2], width);
    let (kv, mut bounds_v, kk_v) = coefficients(in_h, area[1], area[3], height);
    let first = bounds_v[0].0;
    let last = bounds_v[height - 1].0 + bounds_v[height - 1].1;
    let mut current = image.clone();
    if need_horizontal {
        for b in &mut bounds_v {
            b.0 -= first;
        }
        let mut temp = Rgb::zeros((last - first, width, 3));
        for yy in 0..last - first {
            for (xx, &(xmin, count)) in bounds_h.iter().enumerate() {
                let k = &kk_h[xx * kh..xx * kh + count];
                for c in 0..3 {
                    let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                    for (x, weight) in k.iter().enumerate() {
                        ss = ss.wrapping_add(i32::from(image[[yy + first, x + xmin, c]]).wrapping_mul(*weight));
                    }
                    temp[[yy, xx, c]] = clip8(ss);
                }
            }
        }
        current = temp;
    }
    if need_vertical {
        let cw = current.dim().1;
        let mut out = Rgb::zeros((height, cw, 3));
        for (yy, &(ymin, count)) in bounds_v.iter().enumerate() {
            let k = &kk_v[yy * kv..yy * kv + count];
            for xx in 0..cw {
                for c in 0..3 {
                    let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                    for (y, weight) in k.iter().enumerate() {
                        ss = ss.wrapping_add(i32::from(current[[y + ymin, xx, c]]).wrapping_mul(*weight));
                    }
                    out[[yy, xx, c]] = clip8(ss);
                }
            }
        }
        current = out;
    }
    current
}

/// `division_UINT32(divider, 8)`.
fn multiplier(divider: u32) -> u32 {
    let max_dividend = (1u32 << 8).wrapping_mul(divider);
    let max_int: f32 = (1u32 << 30) as f32 * 4.0;
    (max_int / max_dividend as f32) as u32
}

fn block_mean(image: &Rgb, x0: usize, x1: usize, y0: usize, y1: usize, c: usize) -> u8 {
    let count = ((x1 - x0) * (y1 - y0)) as u32;
    let mut ss = count / 2;
    for y in y0..y1 {
        for x in x0..x1 {
            ss += u32::from(image[[y, x, c]]);
        }
    }
    (ss.wrapping_mul(multiplier(count)) >> 24) as u8
}

/// Port of `Image.reduce((fx, fy), box)` (`ImagingReduce` with its corner pass) for 8-bit RGB.
pub fn reduce(image: &Rgb, fx: usize, fy: usize, area: [usize; 4]) -> Rgb {
    let (bw, bh) = (area[2] - area[0], area[3] - area[1]);
    let (ow, oh) = (bw.div_ceil(fx), bh.div_ceil(fy));
    let mut out = Rgb::zeros((oh, ow, 3));
    for y in 0..oh {
        let y0 = area[1] + y * fy;
        let y1 = (y0 + fy).min(area[1] + bh);
        for x in 0..ow {
            let x0 = area[0] + x * fx;
            let x1 = (x0 + fx).min(area[0] + bw);
            for c in 0..3 {
                out[[y, x, c]] = block_mean(image, x0, x1, y0, y1, c);
            }
        }
    }
    out
}

/// `Image.resize(size, LANCZOS, box=None, reducing_gap)`.
pub fn resize(image: &Rgb, width: usize, height: usize, reducing_gap: Option<f64>) -> Rgb {
    let (h, w, _) = image.dim();
    if (w, h) == (width, height) {
        return image.clone();
    }
    let mut area = [0.0f64, 0.0, w as f64, h as f64];
    let mut source = image.clone();
    if let Some(gap) = reducing_gap {
        let fx = (((area[2] - area[0]) / width as f64 / gap) as usize).max(1);
        let fy = (((area[3] - area[1]) / height as f64 / gap) as usize).max(1);
        if fx > 1 || fy > 1 {
            // _get_safe_box
            let support = SUPPORT - 0.5;
            let (sx, sy) =
                ((area[2] - area[0]) / width as f64 * support, (area[3] - area[1]) / height as f64 * support);
            let safe = [
                ((area[0] - sx) as i64).max(0) as usize,
                ((area[1] - sy) as i64).max(0) as usize,
                ((area[2] + sx).ceil() as usize).min(w),
                ((area[3] + sy).ceil() as usize).min(h),
            ];
            source = reduce(&source, fx, fy, safe);
            area = [
                (area[0] - safe[0] as f64) / fx as f64,
                (area[1] - safe[1] as f64) / fy as f64,
                (area[2] - safe[0] as f64) / fx as f64,
                (area[3] - safe[1] as f64) / fy as f64,
            ];
        }
    }
    resample(&source, width, height, area.map(|v| v as f32))
}

/// Port of `Image.thumbnail(size, LANCZOS)` (reducing gap 2): the picture shrunk to fit, aspect kept; unchanged
/// when it fits already.
pub fn thumbnail(image: &Rgb, size: (usize, usize)) -> Rgb {
    let (h, w, _) = image.dim();
    let (x, y) = size;
    if x >= w && y >= h {
        return image.clone();
    }
    let aspect = w as f64 / h as f64;
    let round_aspect = |number: f64, key: &dyn Fn(f64) -> f64| -> usize {
        let (lo, hi) = (number.floor(), number.ceil());
        let pick = if key(hi) < key(lo) { hi } else { lo };
        (pick as i64).max(1) as usize
    };
    let (fw, fh) = if x as f64 / y as f64 >= aspect {
        (round_aspect(y as f64 * aspect, &|n| (aspect - n / y as f64).abs()), y)
    } else {
        let xf = x as f64;
        (x, round_aspect(xf / aspect, &|n| if n == 0.0 { 0.0 } else { (aspect - xf / n).abs() }))
    };
    resize(image, fw, fh, Some(2.0))
}

/// `ImageDraw.rectangle([x0, y0, x1, y1], fill)` (inclusive corners, clipped to the image).
pub fn rectangle(image: &mut Rgb, x0: i64, y0: i64, x1: i64, y1: i64, fill: [u8; 3]) {
    let (h, w, _) = image.dim();
    for y in y0.max(0)..=y1.min(h as i64 - 1) {
        for x in x0.max(0)..=x1.min(w as i64 - 1) {
            for c in 0..3 {
                image[[y as usize, x as usize, c]] = fill[c];
            }
        }
    }
}

/// `Image.paste(picture, (x, y))`.
pub fn paste(image: &mut Rgb, picture: &Rgb, x: i64, y: i64) {
    let (h, w, _) = image.dim();
    let (ph, pw, _) = picture.dim();
    for py in 0..ph {
        let ty = y + py as i64;
        if ty < 0 || ty >= h as i64 {
            continue;
        }
        for px in 0..pw {
            let tx = x + px as i64;
            if tx < 0 || tx >= w as i64 {
                continue;
            }
            for c in 0..3 {
                image[[ty as usize, tx as usize, c]] = picture[[py, px, c]];
            }
        }
    }
}

/// Pillow's `BLEND` of an ink over a pixel with an 8-bit coverage mask (`fill_mask_L`).
pub fn blend(pixel: u8, ink: u8, mask: u8) -> u8 {
    let value = u32::from(pixel) * (255 - u32::from(mask)) + u32::from(ink) * u32::from(mask);
    let tmp = value + 128;
    (((tmp >> 8) + tmp) >> 8) as u8
}
