//! OpenCV float HLS conversions, including AVX2 body and scalar-tail rounding.

/// Whether column `x` of a row `width` pixels wide goes through OpenCV's SIMD code.
#[inline]
pub fn simd_column(x: usize, width: usize) -> bool {
    x < width - width % 8
}

/// OpenCV `RGB2HLS_f` for one pixel already in 0..1: (hue in degrees, lightness, saturation).
#[inline]
pub fn rgb_to_hls_pixel(r: f32, g: f32, b: f32, simd: bool) -> (f32, f32, f32) {
    if simd {
        let vmax = r.max(g).max(b);
        let vmin = r.min(g).min(b);
        let diff = vmax - vmin;
        let sum = vmax + vmin;
        let l = sum * 0.5;
        let s = diff / if l < 0.5 { sum } else { 2.0 - sum };
        let (x, base) = if vmax == r {
            (g - b, if g < b { 360.0 } else { 0.0 })
        } else if vmax == g {
            (b - r, 120.0)
        } else {
            (r - g, 240.0)
        };
        let h = x.mul_add(60.0 / diff, base);
        if diff > f32::EPSILON {
            (h, l, s)
        } else {
            (0.0, l, 0.0)
        }
    } else {
        let mut vmax = r;
        let mut vmin = r;
        if vmax < g {
            vmax = g;
        }
        if vmax < b {
            vmax = b;
        }
        if vmin > g {
            vmin = g;
        }
        if vmin > b {
            vmin = b;
        }
        let mut diff = vmax - vmin;
        let l = (vmax + vmin) * 0.5;
        let mut h = 0.0;
        let mut s = 0.0;
        if diff > f32::EPSILON {
            s = if l < 0.5 { diff / (vmax + vmin) } else { diff / (2.0 - vmax - vmin) };
            diff = 60.0 / diff;
            h = if vmax == r {
                (g - b) * diff
            } else if vmax == g {
                (b - r) * diff + 120.0
            } else {
                (r - g) * diff + 240.0
            };
            if h < 0.0 {
                h += 360.0;
            }
        }
        (h, l, s)
    }
}

/// OpenCV `HLS2RGB_f` for one pixel (hue in degrees): `[r, g, b]`.
#[inline]
pub fn hls_to_rgb_pixel(h: f32, l: f32, s: f32, simd: bool) -> [f32; 3] {
    if !h.is_finite() {
        return [f32::NAN; 3];
    }
    const HSCALE: f32 = 6.0 / 360.0;
    // OpenCV's AVX2 selector falls through to sector zero for negative hue.
    // The processing.py adapter wraps hue first, but raw cvtColor callers do not.
    if simd && h < 0.0 {
        let p2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p1 = 2.0 * l - p2;
        let raw = h * HSCALE;
        return [p2, p1 + (p2 - p1) * (raw - raw.trunc()), p1];
    }
    // (b, g, r) indices into [p2, p1, falling, rising] per sector.
    const SECTOR: [[usize; 3]; 6] = [[1, 3, 0], [1, 0, 2], [3, 0, 1], [0, 2, 1], [0, 1, 3], [2, 1, 0]];
    let (tab, sector) = if simd {
        let ls = l * s;
        let e0 = if l <= 0.5 { ls } else { s - ls };
        let raw = h * HSCALE;
        let pre = raw.trunc();
        let hs = raw - pre;
        let sector = pre - 6.0 * (raw * (1.0f32 / 6.0)).trunc();
        let e1 = hs + hs;
        let tab0 = l + e0;
        let tab1 = l - e0;
        ([tab0, tab1, tab0 - e0 * e1, tab1 + e0 * e1], sector)
    } else {
        if s == 0.0 {
            return [l, l, l];
        }
        let p2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p1 = 2.0 * l - p2;
        let mut hh = h * HSCALE;
        if hh < 0.0 {
            while hh < 0.0 {
                hh += 6.0;
            }
        } else if hh >= 6.0 {
            while hh >= 6.0 {
                hh -= 6.0;
            }
        }
        let sector = hh.floor();
        hh -= sector;
        ([p2, p1, p1 + (p2 - p1) * (1.0 - hh), p1 + (p2 - p1) * hh], sector)
    };
    let index = if sector.is_finite() { (sector as i64).rem_euclid(6) as usize } else { 0 };
    let [b, g, r] = SECTOR[index];
    [tab[r], tab[g], tab[b]]
}
