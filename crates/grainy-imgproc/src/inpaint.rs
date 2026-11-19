use std::cmp::Ordering;
use std::collections::BinaryHeap;

use ndarray::Array2;

use crate::{array, Error, Result};

#[derive(Clone, Copy)]
struct Entry {
    time: f32,
    order: usize,
    y: usize,
    x: usize,
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.order == other.order
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.time.total_cmp(&self.time).then_with(|| other.order.cmp(&self.order))
    }
}

fn solve(a: (usize, usize), b: (usize, usize), mask: &Array2<u8>, time: &Array2<f32>) -> f32 {
    let x = time[a] as f64;
    let y = time[b] as f64;
    let t = match (mask[a] != 2, mask[b] != 2) {
        (true, true) if (x - y).abs() < 1.0 => (x + y + (2.0 - (x - y) * (x - y)).sqrt()) * 0.5,
        (true, true) | (false, false) => 1.0 + x.min(y),
        (true, false) => 1.0 + x,
        (false, true) => 1.0 + y,
    };
    t as f32
}

/// Port of cv2.inpaint(..., INPAINT_NS) in luma/processing.py, healing.py and dust.py.
/// Float32 single-channel input, nonzero uint8 hole mask; radius rounds to even and clamps to 1..100.
/// Known pixels and an entirely empty/full mask are preserved exactly.
pub fn inpaint(src: &Array2<f32>, holes: &Array2<u8>, radius: f64) -> Result<Array2<f32>> {
    let (h, w) = src.dim();
    array::shape(h, w, 1)?;
    if src.dim() != holes.dim() {
        return Err(Error::Shape("inpaint mask must match source"));
    }
    if !radius.is_finite() || src.iter().any(|v| !v.is_finite()) {
        return Err(Error::Argument("inpaint requires finite input and radius"));
    }
    let mut out = src.clone();
    if holes.iter().all(|&v| v == 0) || holes.iter().all(|&v| v != 0) {
        return Ok(out);
    }
    // OpenCV's NS gradient stencil reads outside the source on a partially masked single axis.
    if h < 2 || w < 2 {
        return Err(Error::Shape("NS gradient requires at least two rows and columns"));
    }
    let range = radius.round_ties_even().clamp(1.0, 100.0) as usize;
    let mut mask = Array2::<u8>::zeros((h + 2, w + 2));
    let mut time = Array2::<f32>::from_elem((h + 2, w + 2), 1e6);
    for ((y, x), &v) in holes.indexed_iter() {
        if v != 0 {
            mask[[y + 1, x + 1]] = 2;
        }
    }
    let mut queue = BinaryHeap::new();
    let mut order = 0;
    for y in 1..=h {
        for x in 1..=w {
            if mask[[y, x]] == 0
                && [mask[[y - 1, x]], mask[[y + 1, x]], mask[[y, x - 1]], mask[[y, x + 1]]].contains(&2)
            {
                time[[y, x]] = 0.0;
                queue.push(Entry { time: 0.0, order, y, x });
                order += 1;
            }
        }
    }
    while let Some(entry) = queue.pop() {
        mask[[entry.y, entry.x]] = 0;
        for (y, x) in [(entry.y - 1, entry.x), (entry.y, entry.x - 1), (entry.y + 1, entry.x), (entry.y, entry.x + 1)] {
            if y == 0 || x == 0 || y > h || x > w || mask[[y, x]] != 2 {
                continue;
            }
            let t = solve((y - 1, x), (y, x - 1), &mask, &time)
                .min(solve((y + 1, x), (y, x - 1), &mask, &time))
                .min(solve((y - 1, x), (y, x + 1), &mask, &time).min(solve((y + 1, x), (y, x + 1), &mask, &time)));
            time[[y, x]] = t;
            let mut value = 0.0_f32;
            let mut weight = 1e-20_f32;
            for k in y.saturating_sub(range).max(1)..=(y + range).min(h) {
                let km = k - 1 + usize::from(k == 1);
                let kp = k - 1 - usize::from(k == h);
                for l in x.saturating_sub(range).max(1)..=(x + range).min(w) {
                    let dy = y as f32 - k as f32;
                    let dx = x as f32 - l as f32;
                    let length = dx * dx + dy * dy;
                    if mask[[k, l]] == 2 || length > (range * range) as f32 {
                        continue;
                    }
                    let lm = l - 1 + usize::from(l == 1);
                    let lp = l - 1 - usize::from(l == w);
                    let gx = match (mask[[k + 1, l]] != 2, mask[[k - 1, l]] != 2) {
                        (true, true) => {
                            (out[[kp + 1, lm]] - out[[kp, lm]]).abs() + (out[[kp, lm]] - out[[km - 1, lm]]).abs()
                        }
                        (true, false) => (out[[kp + 1, lm]] - out[[kp, lm]]).abs() * 2.0,
                        (false, true) => (out[[kp, lm]] - out[[km - 1, lm]]).abs() * 2.0,
                        _ => 0.0,
                    };
                    let gy = match (mask[[k, l + 1]] != 2, mask[[k, l - 1]] != 2) {
                        (true, true) => {
                            (out[[km, lp + 1]] - out[[km, lm]]).abs() + (out[[km, lm]] - out[[km, lm - 1]]).abs()
                        }
                        (true, false) => (out[[km, lp + 1]] - out[[km, lm]]).abs() * 2.0,
                        (false, true) => (out[[km, lm]] - out[[km, lm - 1]]).abs() * 2.0,
                        _ => 0.0,
                    };
                    let dot = dx * -gx + dy * gy;
                    let direction = if (dot as f64).abs() <= 0.01 {
                        0.000001
                    } else {
                        // The C++ sqrt(float) overload and the quotient both round to float32.
                        (dot / (length * (gx * gx + gy * gy)).sqrt()).abs()
                    };
                    let wt = (1.0 / (length * length + 1.0)) * direction;
                    value += wt * out[[k - 1, l - 1]];
                    weight += wt;
                }
            }
            out[[y - 1, x - 1]] = (value as f64 / weight as f64) as f32;
            mask[[y, x]] = 1;
            queue.push(Entry { time: t, order, y, x });
            order += 1;
        }
    }
    Ok(out)
}
