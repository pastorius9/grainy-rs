//! `cv2.resize(src, (dw, dh), interpolation=cv2.INTER_AREA)` for float32 shrinking, as OpenCV computes it:
//!
//! - integer factors (`resizeAreaFast_`): `sum = 0; sum += S0 + S1 + … (unrolled by 4); D = sum·(1/area)` in
//!   float, with the AVX2 body for 2×2 on one channel `((a + b) + (c + d))·0.25`; columns or rows beyond the last
//!   whole block average what is there (`sum/count`);
//! - other factors (`resizeArea_`): OpenCV's decimation tables (`computeResizeAreaTab`), each source row summed
//!   with its weights into a float buffer, then rows accumulated as `sum += beta·buf` and stored when the
//!   destination row changes.

use super::Mat;

struct Tap {
    di: usize,
    si: usize,
    alpha: f32,
}

/// `computeResizeAreaTab` (with `cn = 1`).
fn table(ssize: usize, dsize: usize, scale: f64) -> Vec<Tap> {
    let mut tab = Vec::new();
    for dx in 0..dsize {
        let fsx1 = dx as f64 * scale;
        let fsx2 = fsx1 + scale;
        let cell = scale.min(ssize as f64 - fsx1);
        let mut sx1 = fsx1.ceil() as i64;
        let mut sx2 = fsx2.floor() as i64;
        sx2 = sx2.min(ssize as i64 - 1);
        sx1 = sx1.min(sx2);
        if sx1 as f64 - fsx1 > 1e-3 {
            tab.push(Tap { di: dx, si: (sx1 - 1) as usize, alpha: ((sx1 as f64 - fsx1) / cell) as f32 });
        }
        for sx in sx1..sx2 {
            tab.push(Tap { di: dx, si: sx as usize, alpha: (1.0 / cell) as f32 });
        }
        if fsx2 - sx2 as f64 > 1e-3 {
            tab.push(Tap { di: dx, si: sx2 as usize, alpha: ((fsx2 - sx2 as f64).min(1.0).min(cell) / cell) as f32 });
        }
    }
    tab
}

fn fast(src: &Mat, dw: usize, dh: usize, sx: usize, sy: usize) -> Mat {
    let (h, w, cn) = (src.h, src.w, src.cn);
    let mut out = Mat::zeros(dh, dw, cn);
    let area = sx * sy;
    let scale = 1.0f32 / area as f32;
    let width = dw * cn;
    // Whole blocks per row (`dwidth1`), in interleaved values.
    let whole = (w / sx) * cn;
    let ofs: Vec<(usize, usize)> = (0..sy).flat_map(|y| (0..sx).map(move |x| (y, x * cn))).collect();
    let vector = sx == 2 && sy == 2 && cn == 1;
    for dy in 0..dh {
        let sy0 = dy * sy;
        let dst = &mut out.data[dy * width..(dy + 1) * width];
        if sy0 >= h {
            continue;
        }
        let limit = if sy0 + sy <= h { whole } else { 0 };
        let mut dx = 0;
        if vector && limit > 0 {
            // AVX2 body for 2×2 on one channel: eight outputs at a time.
            let r0 = src.row(sy0);
            let r1 = src.row(sy0 + 1);
            while dx + 8 <= limit {
                for (d, slot) in dst.iter_mut().enumerate().skip(dx).take(8) {
                    let s = 2 * d;
                    *slot = ((r0[s] + r0[s + 1]) + (r1[s] + r1[s + 1])) * 0.25;
                }
                dx += 8;
            }
        }
        while dx < limit {
            let (i, c) = (dx / cn, dx % cn);
            let base = i * sx * cn + c;
            let mut sum = 0f32;
            let mut k = 0;
            while k + 4 <= area {
                let v = |k: usize| src.row(sy0 + ofs[k].0)[base + ofs[k].1];
                sum += v(k) + v(k + 1) + v(k + 2) + v(k + 3);
                k += 4;
            }
            while k < area {
                sum += src.row(sy0 + ofs[k].0)[base + ofs[k].1];
                k += 1;
            }
            dst[dx] = sum * scale;
            dx += 1;
        }
        while dx < width {
            let (i, c) = (dx / cn, dx % cn);
            let sx0 = i * sx * cn + c;
            let mut sum = 0f32;
            let mut count = 0u32;
            for yy in 0..sy {
                if sy0 + yy >= h {
                    break;
                }
                let row = src.row(sy0 + yy);
                let mut xx = 0;
                while xx < sx * cn {
                    if sx0 + xx >= w * cn {
                        break;
                    }
                    sum += row[sx0 + xx];
                    count += 1;
                    xx += cn;
                }
            }
            dst[dx] = if count > 0 { sum / count as f32 } else { 0.0 };
            dx += 1;
        }
    }
    out
}

fn general(src: &Mat, dw: usize, dh: usize) -> Mat {
    let (h, w, cn) = (src.h, src.w, src.cn);
    let xtab = table(w, dw, w as f64 / dw as f64);
    let ytab = table(h, dh, h as f64 / dh as f64);
    let width = dw * cn;
    let mut out = Mat::zeros(dh, dw, cn);
    let mut sum = vec![0f32; width];
    let mut buf = vec![0f32; width];
    let mut prev = ytab.first().map_or(0, |t| t.di);
    for t in &ytab {
        let row = src.row(t.si);
        buf.iter_mut().for_each(|v| *v = 0.0);
        for x in &xtab {
            for c in 0..cn {
                buf[x.di * cn + c] += row[x.si * cn + c] * x.alpha;
            }
        }
        if t.di != prev {
            let dst = &mut out.data[prev * width..(prev + 1) * width];
            for i in 0..width {
                dst[i] = sum[i];
                sum[i] = t.alpha * buf[i];
            }
            prev = t.di;
        } else {
            for i in 0..width {
                sum[i] += t.alpha * buf[i];
            }
        }
    }
    out.data[prev * width..(prev + 1) * width].copy_from_slice(&sum);
    out
}

/// INTER_AREA reduction to `dw × dh` (each no larger than the source's side).
pub(crate) fn resize_area(src: &Mat, dw: usize, dh: usize) -> Mat {
    if dw == src.w && dh == src.h {
        return src.clone();
    }
    let scale_x = src.w as f64 / dw as f64;
    let scale_y = src.h as f64 / dh as f64;
    let (ix, iy) = (scale_x.round() as usize, scale_y.round() as usize);
    if (scale_x - ix as f64).abs() < f64::EPSILON && (scale_y - iy as f64).abs() < f64::EPSILON {
        fast(src, dw, dh, ix, iy)
    } else {
        general(src, dw, dh)
    }
}
