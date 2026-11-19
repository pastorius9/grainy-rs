use crate::{array, Error, Result};
use ndarray::Array2;

fn clip(mut a: [i64; 2], mut b: [i64; 2], w: i64, h: i64) -> Option<([i64; 2], [i64; 2])> {
    let code = |p: [i64; 2]| {
        i32::from(p[0] < 0) + 2 * i32::from(p[0] > w - 1) + 4 * i32::from(p[1] < 0) + 8 * i32::from(p[1] > h - 1)
    };
    let mut c1 = code(a);
    let mut c2 = code(b);
    if c1 & c2 == 0 && c1 | c2 != 0 {
        if c1 & 12 != 0 {
            let y = if c1 < 8 { 0 } else { h - 1 };
            a[0] += ((y - a[1]) as f64 * (b[0] - a[0]) as f64 / (b[1] - a[1]) as f64) as i64;
            a[1] = y;
            c1 = code(a);
        }
        if c2 & 12 != 0 {
            let y = if c2 < 8 { 0 } else { h - 1 };
            b[0] += ((y - b[1]) as f64 * (b[0] - a[0]) as f64 / (b[1] - a[1]) as f64) as i64;
            b[1] = y;
            c2 = code(b);
        }
        if c1 & c2 == 0 && c1 | c2 != 0 {
            if c1 != 0 {
                let x = if c1 == 1 { 0 } else { w - 1 };
                a[1] += ((x - a[0]) as f64 * (b[1] - a[1]) as f64 / (b[0] - a[0]) as f64) as i64;
                a[0] = x;
                c1 = 0;
            }
            if c2 != 0 {
                let x = if c2 == 1 { 0 } else { w - 1 };
                b[1] += ((x - b[0]) as f64 * (b[1] - a[1]) as f64 / (b[0] - a[0]) as f64) as i64;
                b[0] = x;
                c2 = 0;
            }
        }
    }
    if c1 | c2 == 0 {
        Some((a, b))
    } else {
        None
    }
}

fn line(dst: &mut Array2<u8>, a: [i64; 2], b: [i64; 2], value: u8) {
    let Some((mut a, mut b)) = clip(a, b, dst.ncols() as i64, dst.nrows() as i64) else {
        return;
    };
    if a[0] > b[0] {
        std::mem::swap(&mut a, &mut b);
    }
    let mut dx = b[0] - a[0];
    let mut dy = b[1] - a[1];
    let sy = if dy < 0 { -1 } else { 1 };
    dy = dy.abs();
    let steep = dy > dx;
    if steep {
        std::mem::swap(&mut dx, &mut dy);
    }
    let mut err = dx - 2 * dy;
    for _ in 0..=dx {
        dst[[a[1] as usize, a[0] as usize]] = value;
        let diagonal = err < 0;
        err += if diagonal { 2 * dx - 2 * dy } else { -2 * dy };
        if steep {
            a[1] += sy;
            if diagonal {
                a[0] += 1;
            }
        } else {
            a[0] += 1;
            if diagonal {
                a[1] += sy;
            }
        }
    }
}

/// Port of cv2.drawContours(polygon, contours, -1, value, -1) in luma/dust_scratches.py.
/// Draws all integer contours with LINE_8, using filled (-1) or one-pixel (1) thickness.
/// Filled contours use the even-odd rule together, as OpenCV does without an explicit hierarchy.
pub fn draw_contours(src: &Array2<u8>, contours: &[Vec<[i32; 2]>], value: u8, thickness: i32) -> Result<Array2<u8>> {
    array::view(src)?;
    if thickness != -1 && thickness != 1 {
        return Err(Error::Argument("supported contour thicknesses are -1 and 1"));
    }
    let (h, w) = src.dim();
    let mut out = src.clone();
    let mut edges = Vec::<(i64, i64, i128, i128)>::new();
    for contour in contours {
        let Some(&last) = contour.last() else {
            continue;
        };
        let mut a = last.map(i64::from);
        for &point in contour {
            let b = point.map(i64::from);
            line(&mut out, a, b, value);
            if thickness < 0 && a[1] != b[1] {
                let (mut ac, mut bc) = (a, b);
                if let Some((ca, cb)) = clip(a, b, w as i64, h as i64) {
                    ac[0] = ca[0];
                    bc[0] = cb[0];
                    if ca[1] != cb[1] {
                        ac[1] = ca[1];
                        bc[1] = cb[1];
                    }
                }
                let slope = (((bc[0] - ac[0]) as i128) << 16) / (bc[1] - ac[1]) as i128;
                let (y0, y1, x) = if a[1] < b[1] {
                    (a[1], b[1], ((ac[0] as i128) << 16) + (a[1] - ac[1]) as i128 * slope)
                } else {
                    (b[1], a[1], ((bc[0] as i128) << 16) + (b[1] - bc[1]) as i128 * slope)
                };
                edges.push((y0, y1, x, slope));
            }
            a = b;
        }
    }
    if thickness < 0 {
        for y in 0..h as i64 {
            let mut xs: Vec<_> =
                edges.iter().filter(|e| e.0 <= y && y < e.1).map(|e| e.2 + (y - e.0) as i128 * e.3).collect();
            xs.sort_unstable();
            for pair in xs.as_chunks::<2>().0 {
                let x0 = ((pair[0] + 65535) >> 16).max(0);
                let x1 = (pair[1] >> 16).min(w as i128 - 1);
                for x in x0..=x1 {
                    out[[y as usize, x as usize]] = value;
                }
            }
        }
    }
    Ok(out)
}
