use crate::array::{generate, view};
use crate::{Error, Pixel, Result};
use ndarray::Array3;

/// Port of the cv2 COLOR_* RGB/HLS/HSV/GRAY conversions used by Grainy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorConversion {
    /// COLOR_RGB2HLS: float hue in degrees, lightness/saturation in native float units.
    Rgb2Hls,
    /// COLOR_HLS2RGB (float hue in degrees, uint8 hue in half-degrees).
    Hls2Rgb,
    /// COLOR_RGB2HSV (uint8 H:0..179, S/V:0..255).
    Rgb2Hsv,
    /// COLOR_HSV2RGB, required by luma/dcp.py.
    Hsv2Rgb,
    /// COLOR_RGB2GRAY: BT.601 coefficients, output H×W×1.
    Rgb2Gray,
}

fn from_hue(h: f32, table: [f32; 4]) -> [f32; 3] {
    // OpenCV sector table, ordered RGB rather than BGR.
    const SECTORS: [[usize; 3]; 6] = [[0, 3, 1], [2, 0, 1], [1, 0, 3], [1, 2, 0], [3, 1, 0], [0, 1, 2]];
    let h = (h * (1.0 / 60.0)).rem_euclid(6.0);
    let sector = h.floor() as usize;
    let sector = SECTORS[sector.min(5)];
    [table[sector[0]], table[sector[1]], table[sector[2]]]
}

fn convert(v: [f32; 3], code: ColorConversion, simd: bool) -> [f32; 3] {
    let [a, b, c] = v;
    match code {
        ColorConversion::Rgb2Gray => [a * 0.299 + b * 0.587 + c * 0.114, 0.0, 0.0],
        ColorConversion::Rgb2Hsv => hsv_pixel(a, b, c, simd),
        ColorConversion::Rgb2Hls => {
            let (h, l, s) = crate::hls::rgb_to_hls_pixel(a, b, c, simd);
            [h, l, s]
        }
        ColorConversion::Hsv2Rgb => {
            if b == 0.0 {
                return [c; 3];
            }
            if a < 0.0 && simd {
                // Preserve OpenCV's eight-lane SIMD truncation bug for negative hue.
                let h = a * (1.0 / 60.0);
                let f = h - h.trunc();
                return [c, c * (1.0 - b * (1.0 - f)), c * (1.0 - b)];
            }
            let h = (a * (1.0 / 60.0)).rem_euclid(6.0);
            let f = h - h.floor();
            from_hue(a, [c, c * (1.0 - b), c * (1.0 - b * f), c * (1.0 - b * (1.0 - f))])
        }
        ColorConversion::Hls2Rgb => crate::hls::hls_to_rgb_pixel(a, b, c, simd),
    }
}

fn hsv_u8(v: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = v.map(|x| x as i32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let diff = max - min;
    let sdiv = if max == 0 { 0 } else { ((255 * 4096) as f64 / max as f64).round_ties_even() as i32 };
    let hdiv = if diff == 0 { 0 } else { ((180 * 4096) as f64 / (6 * diff) as f64).round_ties_even() as i32 };
    let s = (diff * sdiv + 2048) >> 12;
    let h = if max == r {
        g - b
    } else if max == g {
        b - r + 2 * diff
    } else {
        r - g + 4 * diff
    };
    let h = (h * hdiv + 2048) >> 12;
    [if h < 0 { h + 180 } else { h } as f32, s as f32, max as f32]
}

/// Port of cv2.cvtColor for RGB↔HLS, RGB↔HSV and RGB→GRAY on f32/u8 H×W×3.
/// RGB→GRAY also accepts RGBA and returns H×W×1; alpha is ignored.
/// Float inputs are not clipped. Nonfinite colour components are rejected.
pub fn cvt_color<T: Pixel>(src: &Array3<T>, code: ColorConversion) -> Result<Array3<T>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    if c != 3 && !(code == ColorConversion::Rgb2Gray && c == 4) {
        return Err(Error::Shape("colour conversion requires three channels (GRAY also accepts four)"));
    }
    if a.iter().any(|v| !v.float().is_finite()) {
        return Err(Error::Argument("finite colour components required"));
    }
    let output_channels = if code == ColorConversion::Rgb2Gray { 1 } else { 3 };
    Ok(generate(h, w, output_channels, |y, x, ch| {
        let mut v = [a[[y, x, 0]].float(), a[[y, x, 1]].float(), a[[y, x, 2]].float()];
        if T::IS_U8 {
            if code == ColorConversion::Rgb2Gray {
                let v = (v[0] as i32 * 9798 + v[1] as i32 * 19235 + v[2] as i32 * 3735 + 16384) >> 15;
                return T::from_float(v as f32);
            }
            if code == ColorConversion::Rgb2Hsv {
                return T::from_float(hsv_u8(v)[ch]);
            }
            if matches!(code, ColorConversion::Hls2Rgb | ColorConversion::Hsv2Rgb) {
                v = [v[0] * 2.0, v[1] * (1.0 / 255.0), v[2] * (1.0 / 255.0)];
            } else {
                v = v.map(|x| x * (1.0 / 255.0));
            }
        }
        let v = convert(v, code, !T::IS_U8 && x < w / 8 * 8);
        let scale = if T::IS_U8 {
            if code == ColorConversion::Rgb2Hls && ch == 0 {
                0.5
            } else {
                255.0
            }
        } else {
            1.0
        };
        T::from_float(v[ch] * scale)
    }))
}

pub(crate) fn hsv_pixel(r: f32, g: f32, b: f32, vector: bool) -> [f32; 3] {
    let v = r.max(g).max(b);
    let vmin = r.min(g).min(b);
    let diff = v - vmin;
    let s = diff / (v.abs() + f32::EPSILON);
    let (d, offset) = if v == r {
        (g - b, 0.0f32)
    } else if v == g {
        (b - r, 120.0)
    } else {
        (r - g, 240.0)
    };
    let h = if vector {
        let offset = if v == r && d < 0.0 { 360.0 } else { offset };
        d.mul_add(60.0 / (diff + f32::EPSILON), offset)
    } else {
        let k = (60.0 / (f64::from(diff) + f64::from(f32::EPSILON))) as f32;
        let h = d * k + offset;
        if h < 0.0 {
            h + 360.0
        } else {
            h
        }
    };
    [h, s, v]
}
