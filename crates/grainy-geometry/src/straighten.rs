//! Port of luma/straighten.py: crop limits for a straightened photo (the crop always stays inside the turned
//! photo). Crops are `[x0, y0, x1, y1]` in fractions of the canvas; `width`/`height` are the canvas in pixels.

/// Pixels of the photo's edge left out when it is turned (`MARGIN`).
pub const MARGIN: f64 = 2.0;
/// The whole canvas (`FULL`).
pub const FULL: [f64; 4] = [0.0, 0.0, 1.0, 1.0];

fn extent(angle: f64, width: f64, height: f64) -> (f64, f64, f64, f64) {
    let a = angle.to_radians();
    let margin = if angle.abs() > 1e-9 { MARGIN } else { 0.0 };
    (py_max(1e-6, width / 2.0 - margin), py_max(1e-6, height / 2.0 - margin), a.cos(), a.sin())
}

/// Python's `max(a, b)`: `a` unless `b > a`.
fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// Python's `min(a, b)`: `a` unless `b < a`.
fn py_min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// Port of `padding`: pixels to add (left and right, top and bottom) so the whole turned photo stays visible.
pub fn padding(angle: f64, width: usize, height: usize) -> (usize, usize) {
    let a = angle.to_radians();
    let (cos, sin) = (a.cos().abs(), a.sin().abs());
    let (w, h) = (width as f64, height as f64);
    let x = ((w * cos + h * sin - w) / 2.0).ceil();
    let y = ((w * sin + h * cos - h) / 2.0).ceil();
    (if x > 0.0 { x as usize } else { 0 }, if y > 0.0 { y as usize } else { 0 })
}

/// Port of `valid` (tolerance 1e-6): every corner of the crop lies on the turned photo and on the canvas.
pub fn valid(crop: [f64; 4], angle: f64, width: f64, height: f64) -> bool {
    valid_with(crop, angle, width, height, 1e-6)
}

/// Port of `valid` with an explicit tolerance.
pub fn valid_with(crop: [f64; 4], angle: f64, width: f64, height: f64, tolerance: f64) -> bool {
    let [x0, y0, x1, y1] = crop;
    if x0 < -tolerance || y0 < -tolerance || x1 > 1.0 + tolerance || y1 > 1.0 + tolerance || x1 <= x0 || y1 <= y0 {
        return false;
    }
    let (half_width, half_height, cos, sin) = extent(angle, width, height);
    let slack = tolerance * py_max(width, height);
    for u in [x0, x1] {
        for v in [y0, y1] {
            let (x, y) = ((u - 0.5) * width, (v - 0.5) * height);
            if (x * cos + y * sin).abs() > half_width + slack || (-x * sin + y * cos).abs() > half_height + slack {
                return false;
            }
        }
    }
    true
}

/// Port of `ratio`: width over height of a crop in pixels.
pub fn ratio(crop: [f64; 4], width: f64, height: f64) -> f64 {
    (crop[2] - crop[0]) * width / py_max(1e-9, (crop[3] - crop[1]) * height)
}

/// Port of `largest`: the largest centred crop of the given aspect (width over height in pixels). `None` or a
/// zero aspect (falsy in Python) means the canvas's own aspect.
pub fn largest(angle: f64, width: f64, height: f64, aspect: Option<f64>) -> [f64; 4] {
    let aspect = match aspect {
        Some(a) if a != 0.0 => a,
        _ => width / height,
    };
    let (half_width, half_height, cos, sin) = extent(angle, width, height);
    let (cos, sin) = (cos.abs(), sin.abs());
    let mut y = half_width / (aspect * cos + sin);
    y = py_min(y, half_height / (aspect * sin + cos));
    y = py_min(y, height / 2.0);
    y = py_min(y, width / 2.0 / aspect);
    let x = aspect * y;
    [0.5 - x / width, 0.5 - y / height, 0.5 + x / width, 0.5 + y / height]
}

fn scaled(crop: [f64; 4], factor: f64) -> [f64; 4] {
    let (cx, cy) = ((crop[0] + crop[2]) / 2.0, (crop[1] + crop[3]) / 2.0);
    let (w, h) = ((crop[2] - crop[0]) * factor / 2.0, (crop[3] - crop[1]) * factor / 2.0);
    [cx - w, cy - h, cx + w, cy + h]
}

/// Port of `shrink`: the crop made smaller about its own centre until it fits; a centre off the photo gives the
/// largest crop of its ratio.
pub fn shrink(crop: [f64; 4], angle: f64, width: f64, height: f64) -> [f64; 4] {
    if valid(crop, angle, width, height) {
        return crop;
    }
    if !valid(scaled(crop, 1e-3), angle, width, height) {
        return largest(angle, width, height, Some(ratio(crop, width, height)));
    }
    let (mut low, mut high) = (1e-3, 1.0);
    for _ in 0..40 {
        let middle = (low + high) / 2.0;
        if valid(scaled(crop, middle), angle, width, height) {
            low = middle;
        } else {
            high = middle;
        }
    }
    scaled(crop, low)
}

/// Port of `limit`: as far from the previous crop towards the candidate as stays on the photo (for dragging).
pub fn limit(candidate: [f64; 4], previous: [f64; 4], angle: f64, width: f64, height: f64) -> [f64; 4] {
    if valid(candidate, angle, width, height) || !valid(previous, angle, width, height) {
        return candidate;
    }
    let lerp = |t: f64| -> [f64; 4] { std::array::from_fn(|i| previous[i] + (candidate[i] - previous[i]) * t) };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..30 {
        let middle = (low + high) / 2.0;
        if valid(lerp(middle), angle, width, height) {
            low = middle;
        } else {
            high = middle;
        }
    }
    lerp(low)
}

/// Port of `close` (tolerance 2e-3; pairs beyond the shorter slice are ignored, as `zip`).
pub fn close(a: &[f64], b: &[f64]) -> bool {
    close_with(a, b, 2e-3)
}

/// Port of `close` with an explicit tolerance.
pub fn close_with(a: &[f64], b: &[f64], tolerance: f64) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
}

/// Port of `refit`: the crop after the angle changed from `previous_angle`; `None` means the whole canvas.
/// `crop: None` is no crop; `aspect` `None` or 0 means the crop's own ratio.
pub fn refit(
    crop: Option<[f64; 4]>,
    previous_angle: f64,
    angle: f64,
    width: f64,
    height: f64,
    aspect: Option<f64>,
) -> Option<[f64; 4]> {
    let current = crop.unwrap_or(FULL);
    let current_ratio = ratio(current, width, height);
    let automatic =
        close(&current, &FULL) || close(&current, &largest(previous_angle, width, height, Some(current_ratio)));
    let result = if automatic {
        let aspect = match aspect {
            Some(a) if a != 0.0 => a,
            _ => current_ratio,
        };
        largest(angle, width, height, Some(aspect))
    } else {
        shrink(current, angle, width, height)
    };
    if close_with(&result, &FULL, 1e-9) {
        None
    } else {
        Some(result)
    }
}
