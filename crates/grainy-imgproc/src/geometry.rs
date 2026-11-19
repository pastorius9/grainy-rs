use ndarray::{Array, Array2, Array3, Axis, Dimension};

use crate::array::{finish, generate, shape, view};
use crate::border::sample;
use crate::{Border, Error, Pixel, Result};

/// Port of cv2 INTER_* flags used by Grainy's resize and geometric transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// INTER_NEAREST (floor in resize, nearest-even coordinates in remap).
    Nearest,
    /// INTER_LINEAR (half-pixel resize; OpenCV 5 continuous warp coordinates for 1/3/4 channels).
    Linear,
    /// INTER_AREA (area integration when shrinking; OpenCV's two-tap fallback otherwise).
    Area,
}

fn area_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let lo = d as f64 * scale;
            let hi = (lo + scale).min(src as f64);
            let width = (scale).min(src as f64 - lo);
            let first = lo.ceil() as usize;
            let last = (hi.floor() as usize).min(src - 1);
            let mut entries = Vec::new();
            if first as f64 - lo > 1e-3 {
                entries.push((first - 1, ((first as f64 - lo) / width) as f32));
            }
            for s in first..last {
                entries.push((s, (1.0 / width) as f32));
            }
            if hi - last as f64 > 1e-3 {
                entries.push((last, ((hi - last as f64).min(1.0).min(width) / width) as f32));
            }
            entries
        })
        .collect()
}

fn linear_weights(src: usize, dst: usize, area: bool) -> Vec<(usize, usize, f32)> {
    let scale = src as f64 / dst as f64;
    let inv = 1.0 / scale;
    (0..dst)
        .map(|d| {
            let (s, mut f) = if area {
                let s = (d as f64 * scale).floor() as i64;
                let t = (d + 1) as f64 - (s + 1) as f64 * inv;
                (s, if t <= 0.0 { 0.0 } else { (t - t.floor()) as f32 })
            } else {
                // OpenCV rounds the coordinate to float before extracting its floor.
                let t = ((d as f64 + 0.5) * scale - 0.5) as f32;
                (t.floor() as i64, t - t.floor())
            };
            let s = if s < 0 {
                f = 0.0;
                0
            } else if s >= src as i64 - 1 {
                f = 0.0;
                src - 1
            } else {
                s as usize
            };
            (s, (s + 1).min(src - 1), f)
        })
        .collect()
}

/// Port of cv2.resize for CV_32F/CV_8U planes or images, explicit output (width,height).
/// Supports INTER_AREA shrinking and enlargement, INTER_LINEAR, and INTER_NEAREST.
pub fn resize<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    size: (usize, usize),
    interpolation: Interpolation,
) -> Result<Array<T, D>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    let (ow, oh) = size;
    shape(oh, ow, c)?;
    if (ow, oh) == (w, h) {
        return Ok(src.clone());
    }
    if interpolation == Interpolation::Nearest {
        return finish(
            generate(oh, ow, c, |y, x, ch| {
                a[[(y as f64 * h as f64 / oh as f64) as usize, (x as f64 * w as f64 / ow as f64) as usize, ch]]
            }),
            src.ndim(),
        );
    }
    if !T::IS_U8 && interpolation == Interpolation::Area && ow <= w && oh <= h {
        let m = crate::exact::Mat { h, w, cn: c, data: a.iter().map(|v| v.float()).collect() };
        let out = crate::exact::area::resize_area(&m, ow, oh);
        return finish(out.into_image().mapv(T::from_float), src.ndim());
    }
    if interpolation == Interpolation::Area && ow <= w && oh <= h {
        if w % ow == 0 && h % oh == 0 {
            let sx = w / ow;
            let sy = h / oh;
            let scale = 1.0 / (sx * sy) as f32;
            return finish(
                generate(oh, ow, c, |y, x, ch| {
                    let mut sum = 0.0f32;
                    for j in 0..sy {
                        for i in 0..sx {
                            sum += a[[y * sy + j, x * sx + i, ch]].float();
                        }
                    }
                    // The uint8 2x2 fast path rounds half upward, unlike cvRound.
                    if T::IS_U8 && sx == 2 && sy == 2 {
                        T::from_float(((sum + 2.0) / 4.0).floor())
                    } else {
                        T::from_float(sum * scale)
                    }
                }),
                src.ndim(),
            );
        }
        let xs = area_weights(w, ow);
        let ys = area_weights(h, oh);
        let row = generate(h, ow, c, |y, x, ch| {
            let mut sum = 0.0;
            for &(i, weight) in &xs[x] {
                sum += a[[y, i, ch]].float() * weight;
            }
            sum
        });
        return finish(
            generate(oh, ow, c, |y, x, ch| {
                let mut sum = 0.0;
                for &(j, weight) in &ys[y] {
                    sum += row[[j, x, ch]] * weight;
                }
                T::from_float(sum)
            }),
            src.ndim(),
        );
    }
    let area = interpolation == Interpolation::Area;
    let xs = linear_weights(w, ow, area);
    let ys = linear_weights(h, oh, area);
    finish(
        generate(oh, ow, c, |y, x, ch| {
            let (x0, x1, fx) = xs[x];
            let (y0, y1, fy) = ys[y];
            if T::IS_U8 {
                let ax0 = ((1.0 - fx) * 2048.0).round_ties_even() as i64;
                let ax1 = (fx * 2048.0).round_ties_even() as i64;
                let ay0 = ((1.0 - fy) * 2048.0).round_ties_even() as i64;
                let ay1 = (fy * 2048.0).round_ties_even() as i64;
                let top = a[[y0, x0, ch]].float() as i64 * ax0 + a[[y0, x1, ch]].float() as i64 * ax1;
                let bottom = a[[y1, x0, ch]].float() as i64 * ax0 + a[[y1, x1, ch]].float() as i64 * ax1;
                T::from_float(((top * ay0 + bottom * ay1 + (1 << 21)) >> 22) as f32)
            } else {
                let top = a[[y0, x0, ch]].float() * (1.0 - fx) + a[[y0, x1, ch]].float() * fx;
                let bottom = a[[y1, x0, ch]].float() * (1.0 - fx) + a[[y1, x1, ch]].float() * fx;
                T::from_float(top * (1.0 - fy) + bottom * fy)
            }
        }),
        src.ndim(),
    )
}

fn round_i32(v: f64) -> i64 {
    // cvRound's SIMD conversion returns INT_MIN on overflow/nonfinite coordinates.
    if !v.is_finite() || v.round_ties_even() > i32::MAX as f64 || v.round_ties_even() < i32::MIN as f64 {
        i32::MIN as i64
    } else {
        v.round_ties_even() as i64
    }
}

fn mapped<T: Pixel>(
    a: ndarray::ArrayView3<'_, T>,
    qx: i64,
    qy: i64,
    ch: usize,
    interpolation: Interpolation,
    border: Border,
) -> T {
    if interpolation == Interpolation::Nearest {
        return T::from_float(sample(a, qy.clamp(-32768, 32767), qx.clamp(-32768, 32767), ch, border));
    }
    let x = (qx >> 5).clamp(-32768, 32767);
    let y = (qy >> 5).clamp(-32768, 32767);
    let fx = (qx & 31) as f32 / 32.0;
    let fy = (qy & 31) as f32 / 32.0;
    let weights = [(1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy];
    let v = sample(a, y, x, ch, border) * weights[0]
        + sample(a, y, x + 1, ch, border) * weights[1]
        + sample(a, y + 1, x, ch, border) * weights[2]
        + sample(a, y + 1, x + 1, ch, border) * weights[3];
    if T::IS_U8 {
        T::from_float((v + 0.5).floor())
    } else {
        T::from_float(v)
    }
}

fn warp_check(interpolation: Interpolation, h: usize, w: usize) -> Result<()> {
    if interpolation == Interpolation::Area {
        return Err(Error::Argument("remap/warp supports only Linear or Nearest"));
    }
    if h >= 32767 || w >= 32767 {
        return Err(Error::Shape("OpenCV remap dimensions must be less than 32767"));
    }
    Ok(())
}

fn continuous<T: Pixel>(
    a: ndarray::ArrayView3<'_, T>,
    sx: f32,
    sy: f32,
    ch: usize,
    interpolation: Interpolation,
    border: Border,
    fused: bool,
) -> T {
    if interpolation == Interpolation::Nearest {
        return T::from_float(sample(a, round_i32(sy as f64), round_i32(sx as f64), ch, border));
    }
    let x = sx.floor() as i64;
    let y = sy.floor() as i64;
    let fx = sx - x as f32;
    let fy = sy - y as f32;
    let p00 = sample(a, y, x, ch, border);
    let p01 = sample(a, y, x + 1, ch, border);
    let p10 = sample(a, y + 1, x, ch, border);
    let p11 = sample(a, y + 1, x + 1, ch, border);
    let lerp = |a: f32, b: f32, f: f32| if fused { f.mul_add(b - a, a) } else { a + f * (b - a) };
    T::from_float(lerp(lerp(p00, p01, fx), lerp(p10, p11, fx), fy))
}

fn coordinate(m: [f32; 3], x: usize, y: usize, fused: bool) -> f32 {
    if fused {
        m[0].mul_add(x as f32, y as f32 * m[1] + m[2])
    } else {
        (x as f32 * m[0] + y as f32 * m[1]) + m[2]
    }
}

fn coordinate_ok(v: f32) -> bool {
    v.is_finite() && v.abs() <= (i32::MAX - 256) as f32
}

/// Port of cv2.remap with separate CV_32F x/y maps. OpenCV 5 uses continuous
/// coordinates for 1/3/4 channels; other channel counts keep its 1/32-pixel table.
pub fn remap<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    map_x: &Array2<f32>,
    map_y: &Array2<f32>,
    interpolation: Interpolation,
    border: Border,
) -> Result<Array<T, D>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    let (oh, ow) = map_x.dim();
    shape(oh, ow, c)?;
    warp_check(interpolation, h, w)?;
    warp_check(interpolation, oh, ow)?;
    if map_x.dim() != map_y.dim() {
        return Err(Error::Shape("remap maps must have equal shapes"));
    }
    if map_x.iter().chain(map_y).any(|&v| !v.is_finite() || v.abs() > (i32::MAX - 256) as f32) {
        return Err(Error::Argument("finite remap coordinates within int32 range required"));
    }
    let scale = if interpolation == Interpolation::Nearest { 1.0 } else { 32.0 };
    finish(
        generate(oh, ow, c, |y, x, ch| {
            if matches!(c, 1 | 3 | 4) {
                return continuous(a, map_x[[y, x]], map_y[[y, x]], ch, interpolation, border, x < ow / 16 * 16);
            }
            mapped(
                a,
                round_i32((map_x[[y, x]] * scale) as f64),
                round_i32((map_y[[y, x]] * scale) as f64),
                ch,
                interpolation,
                border,
            )
        }),
        src.ndim(),
    )
}

/// Port of cv2.remap(src, CV_32FC2_map, None, ...), used by luma/optics.py.
/// The final map axis holds (x,y); identical numerics to the separate-map API.
pub fn remap_interleaved<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    map: &Array3<f32>,
    interpolation: Interpolation,
    border: Border,
) -> Result<Array<T, D>> {
    if map.shape()[2] != 2 {
        return Err(Error::Shape("interleaved remap requires HxWx2 coordinates"));
    }
    let mx = map.index_axis(Axis(2), 0).to_owned();
    let my = map.index_axis(Axis(2), 1).to_owned();
    remap(src, &mx, &my, interpolation, border)
}

/// Port of cv2.getRotationMatrix2D (positive degrees rotate counterclockwise).
/// Center is converted to Point2f, as in OpenCV's public API.
pub fn get_rotation_matrix_2d(center: (f64, f64), angle: f64, scale: f64) -> Result<[[f64; 3]; 2]> {
    if [center.0, center.1, angle, scale].iter().any(|x| !x.is_finite()) {
        return Err(Error::Argument("finite rotation arguments required"));
    }
    let x = center.0 as f32 as f64;
    let y = center.1 as f32 as f64;
    let a = angle * std::f64::consts::PI / 180.0;
    if !x.is_finite() || !y.is_finite() || !a.is_finite() {
        return Err(Error::Argument("rotation arguments overflow the oracle's numeric types"));
    }
    let alpha = a.cos() * scale;
    let beta = a.sin() * scale;
    let matrix = [[alpha, beta, (1.0 - alpha) * x - beta * y], [-beta, alpha, beta * x + (1.0 - alpha) * y]];
    if matrix.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Error::Argument("rotation matrix overflow"));
    }
    Ok(matrix)
}

fn inverse_affine(m: [[f64; 3]; 2]) -> [[f64; 3]; 2] {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let r = if det != 0.0 { 1.0 / det } else { 0.0 };
    let mut inv = [[m[1][1] * r, -m[0][1] * r, 0.0], [-m[1][0] * r, m[0][0] * r, 0.0]];
    inv[0][2] = -inv[0][0] * m[0][2] - inv[0][1] * m[1][2];
    inv[1][2] = -inv[1][0] * m[0][2] - inv[1][1] * m[1][2];
    inv
}

/// Port of cv2.warpAffine, with OpenCV 5 float coordinates for 1/3/4 channels
/// and its 10-bit intermediate coordinates for other channel counts.
/// `inverse_map=true` corresponds to WARP_INVERSE_MAP.
pub fn warp_affine<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    matrix: [[f64; 3]; 2],
    size: (usize, usize),
    interpolation: Interpolation,
    border: Border,
    inverse_map: bool,
) -> Result<Array<T, D>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    let (ow, oh) = size;
    shape(oh, ow, c)?;
    warp_check(interpolation, h, w)?;
    warp_check(interpolation, oh, ow)?;
    if matrix.iter().flatten().any(|x| !x.is_finite()) {
        return Err(Error::Argument("finite affine matrix required"));
    }
    if matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0] == 0.0 {
        return Err(Error::Argument("singular affine matrix"));
    }
    let m = if inverse_map { matrix } else { inverse_affine(matrix) };
    let mf = m.map(|r| r.map(|v| v as f32));
    if mf.iter().flatten().any(|x| !x.is_finite()) {
        return Err(Error::Argument("affine matrix overflows float32"));
    }
    for (x, y) in [(0, 0), (ow - 1, 0), (0, oh - 1), (ow - 1, oh - 1)] {
        if mf.iter().any(|&r| !coordinate_ok(coordinate(r, x, y, false))) {
            return Err(Error::Argument("affine coordinates exceed int32 range"));
        }
    }
    let nearest = interpolation == Interpolation::Nearest;
    let delta = if nearest { 512 } else { 16 };
    let shift = if nearest { 10 } else { 5 };
    finish(
        generate(oh, ow, c, |y, x, ch| {
            if matches!(c, 1 | 3 | 4) {
                let fused = x < ow / 16 * 16;
                return continuous(
                    a,
                    coordinate(mf[0], x, y, fused),
                    coordinate(mf[1], x, y, fused),
                    ch,
                    interpolation,
                    border,
                    fused,
                );
            }
            let qx =
                (round_i32((m[0][1] * y as f64 + m[0][2]) * 1024.0) + delta + round_i32(m[0][0] * x as f64 * 1024.0))
                    >> shift;
            let qy =
                (round_i32((m[1][1] * y as f64 + m[1][2]) * 1024.0) + delta + round_i32(m[1][0] * x as f64 * 1024.0))
                    >> shift;
            mapped(a, qx, qy, ch, interpolation, border)
        }),
        src.ndim(),
    )
}

fn inverse3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut cofactor = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            cofactor[j][i] = m[(i + 1) % 3][(j + 1) % 3] * m[(i + 2) % 3][(j + 2) % 3]
                - m[(i + 1) % 3][(j + 2) % 3] * m[(i + 2) % 3][(j + 1) % 3];
        }
    }
    let det = m[0][0] * cofactor[0][0] + m[0][1] * cofactor[1][0] + m[0][2] * cofactor[2][0];
    let scale = if det == 0.0 { 0.0 } else { 1.0 / det };
    for row in &mut cofactor {
        for v in row {
            *v *= scale;
        }
    }
    cofactor
}

/// Port of cv2.warpPerspective, with constant/replicate/reflect/wrap borders and
/// WARP_INVERSE_MAP. Singular matrices and horizons crossing the destination
/// are rejected: OpenCV 5 can hang on those inputs with reflective borders.
pub fn warp_perspective<T: Pixel, D: Dimension>(
    src: &Array<T, D>,
    matrix: [[f64; 3]; 3],
    size: (usize, usize),
    interpolation: Interpolation,
    border: Border,
    inverse_map: bool,
) -> Result<Array<T, D>> {
    let a = view(src)?;
    let (h, w, c) = a.dim();
    let (ow, oh) = size;
    shape(oh, ow, c)?;
    warp_check(interpolation, h, w)?;
    warp_check(interpolation, oh, ow)?;
    if matrix.iter().flatten().any(|x| !x.is_finite()) {
        return Err(Error::Argument("finite perspective matrix required"));
    }
    let m = if inverse_map { matrix } else { inverse3(matrix) };
    let mf = m.map(|r| r.map(|v| v as f32));
    if mf.iter().flatten().any(|x| !x.is_finite()) {
        return Err(Error::Argument("perspective matrix overflows float32"));
    }
    let d = [
        coordinate(mf[2], 0, 0, false),
        coordinate(mf[2], ow - 1, 0, false),
        coordinate(mf[2], 0, oh - 1, false),
        coordinate(mf[2], ow - 1, oh - 1, false),
    ];
    if d.iter().any(|v| !v.is_finite())
        || (d.iter().copied().fold(f32::INFINITY, f32::min) <= 0.0
            && d.iter().copied().fold(f32::NEG_INFINITY, f32::max) >= 0.0)
    {
        return Err(Error::Argument("singular perspective matrix or horizon in output"));
    }
    for (x, y) in [(0, 0), (ow - 1, 0), (0, oh - 1), (ow - 1, oh - 1)] {
        let d = coordinate(mf[2], x, y, false);
        if mf[..2].iter().any(|&r| !coordinate_ok(coordinate(r, x, y, false) / d)) {
            return Err(Error::Argument("perspective coordinates exceed int32 range"));
        }
    }
    let factor = if interpolation == Interpolation::Nearest { 1.0 } else { 32.0 };
    finish(
        generate(oh, ow, c, |y, x, ch| {
            if matches!(c, 1 | 3 | 4) {
                let fused = x < ow / 16 * 16;
                let d = coordinate(mf[2], x, y, fused);
                return continuous(
                    a,
                    coordinate(mf[0], x, y, fused) / d,
                    coordinate(mf[1], x, y, fused) / d,
                    ch,
                    interpolation,
                    border,
                    fused,
                );
            }
            let x = x as f64;
            let y = y as f64;
            let denom = m[2][0] * x + (m[2][1] * y + m[2][2]);
            let r = if denom == 0.0 { 0.0 } else { factor / denom };
            let qx = round_i32(((m[0][0] * x + (m[0][1] * y + m[0][2])) * r).clamp(i32::MIN as f64, i32::MAX as f64));
            let qy = round_i32(((m[1][0] * x + (m[1][1] * y + m[1][2])) * r).clamp(i32::MIN as f64, i32::MAX as f64));
            mapped(a, qx, qy, ch, interpolation, border)
        }),
        src.ndim(),
    )
}

/// Port of cv2.getPerspectiveTransform with four Point2f correspondences and
/// the default LU solve. Degenerate point systems return an error instead of
/// OpenCV 5's nonunique SVD fallback (unsafe to pass into reflective warps).
pub fn get_perspective_transform(src: [[f32; 2]; 4], dst: [[f32; 2]; 4]) -> Result<[[f64; 3]; 3]> {
    if src.iter().chain(dst.iter()).flatten().any(|v| !v.is_finite()) {
        return Err(Error::Argument("finite perspective points required"));
    }
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let [x, y] = src[i].map(f64::from);
        let [u, v] = dst[i].map(f64::from);
        a[i] = [x, y, 1.0, 0.0, 0.0, 0.0, (-src[i][0] * dst[i][0]) as f64, (-src[i][1] * dst[i][0]) as f64, u];
        a[i + 4] = [0.0, 0.0, 0.0, x, y, 1.0, (-src[i][0] * dst[i][1]) as f64, (-src[i][1] * dst[i][1]) as f64, v];
    }
    for k in 0..8 {
        let mut p = k;
        for i in k + 1..8 {
            if a[i][k].abs() > a[p][k].abs() {
                p = i;
            }
        }
        if a[p][k].abs() < f64::EPSILON {
            return Err(Error::Argument("degenerate perspective correspondences"));
        }
        a.swap(k, p);
        let inverse = -1.0 / a[k][k];
        for i in k + 1..8 {
            let alpha = a[i][k] * inverse;
            for j in k + 1..9 {
                a[i][j] += alpha * a[k][j];
            }
        }
    }
    let mut solution = [0.0; 9];
    solution[8] = 1.0;
    for i in (0..8).rev() {
        let mut value = a[i][8];
        for j in i + 1..8 {
            value -= a[i][j] * solution[j];
        }
        solution[i] = value / a[i][i];
    }
    Ok([
        [solution[0], solution[1], solution[2]],
        [solution[3], solution[4], solution[5]],
        [solution[6], solution[7], solution[8]],
    ])
}
