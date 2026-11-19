use crate::{array, Error, Result};
use ndarray::Array2;

/// Port of cv2.findContours retrieval modes used by luma/dust_scratches.py.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retrieval {
    /// cv2.RETR_EXTERNAL, outermost contours only.
    External,
    /// cv2.RETR_CCOMP, two-level component/hole hierarchy.
    CComp,
}

/// Port of cv2.findContours return values; points are (x,y) with CHAIN_APPROX_SIMPLE.
#[derive(Debug, Clone)]
pub struct Contours {
    /// cv2.findContours contours in the oracle's order and winding.
    pub contours: Vec<Vec<[i32; 2]>>,
    /// cv2.findContours [next,previous,first_child,parent] indices, -1 when absent.
    pub hierarchy: Vec<[i32; 4]>,
}

const DELTA: [(isize, isize); 8] = [(0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1), (1, 0), (1, 1)];
fn neighbor(p: (usize, usize), s: usize) -> (usize, usize) {
    ((p.0 as isize + DELTA[s % 8].0) as usize, (p.1 as isize + DELTA[s % 8].1) as usize)
}

fn trace(image: &mut Array2<i32>, start: (usize, usize), hole: bool, id: i32) -> Vec<[i32; 2]> {
    let mut s = if hole { 0 } else { 4 };
    let end = s;
    let first = loop {
        s = (s + 7) % 8;
        let p = neighbor(start, s);
        if image[p] != 0 || s == end {
            break p;
        }
    };
    let mut points = Vec::new();
    if s == end {
        image[start] = -id;
        points.push([start.1 as i32 - 1, start.0 as i32 - 1]);
        return points;
    }
    let mut p = start;
    let mut prev = s ^ 4;
    loop {
        let end = s;
        let next = loop {
            s += 1;
            let q = neighbor(p, s);
            if image[q] != 0 {
                break q;
            }
        };
        s %= 8;
        if ((s as i32 - 1) as u32) < end as u32 {
            image[p] = -id;
        } else if image[p] == 1 {
            image[p] = id;
        }
        if s != prev {
            points.push([p.1 as i32 - 1, p.0 as i32 - 1]);
        }
        prev = s;
        if next == start && p == first {
            break;
        }
        p = next;
        s = (s + 4) % 8;
    }
    points
}

/// Port of cv2.findContours(mask, RETR_EXTERNAL/RETR_CCOMP, CHAIN_APPROX_SIMPLE).
/// Input is not modified. The scanner uses unbounded internal border IDs, avoiding byte-ID collisions.
pub fn find_contours(src: &Array2<u8>, mode: Retrieval) -> Result<Contours> {
    let (h, w) = src.dim();
    array::shape(h, w, 1)?;
    if src.len() > i32::MAX as usize - 2 {
        return Err(Error::Shape("too many pixels for contour indices"));
    }
    let mut image = Array2::<i32>::zeros((h + 2, w + 2));
    for ((y, x), &v) in src.indexed_iter() {
        image[[y + 1, x + 1]] = i32::from(v != 0);
    }
    let mut points: Vec<Vec<[i32; 2]>> = vec![Vec::new()];
    let mut parents = vec![0_usize];
    let mut is_hole = vec![true];
    let mut children = vec![Vec::<usize>::new()];
    for y in 1..=h {
        let mut last = 0;
        let mut prev = 0;
        for x in 1..=w {
            let p = image[[y, x]];
            if p == prev {
                continue;
            }
            let mut hole = false;
            let mut found = true;
            if !(prev == 0 && p == 1) {
                if p != 0 || prev < 1 {
                    found = false;
                } else {
                    if prev != 1 {
                        last = x - 1;
                    }
                    hole = true;
                }
            }
            if found && mode == Retrieval::External && (hole || image[[y, last]] > 0) {
                found = false;
            }
            if found {
                let parent = if mode == Retrieval::External || !hole || last == 0 {
                    0
                } else {
                    let border = image[[y, last]].unsigned_abs() as usize;
                    let previous = border.saturating_sub(1);
                    if previous >= points.len() {
                        return Err(Error::Argument("invalid contour topology"));
                    }
                    if is_hole[previous] == hole {
                        parents[previous]
                    } else {
                        previous
                    }
                };
                last = x - usize::from(hole);
                let id = points.len();
                points.push(trace(&mut image, (y, last), hole, id as i32 + 1));
                parents.push(parent);
                is_hole.push(hole);
                children.push(Vec::new());
                children[parent].push(id);
                prev = image[[y, x]];
            } else {
                prev = p;
                if prev != 0 && prev != 1 {
                    last = x;
                }
            }
        }
    }
    // OpenCV inserts each new sibling at the front, then emits depth-first pre-order.
    let mut order = Vec::new();
    let mut stack = children[0].clone();
    while let Some(id) = stack.pop() {
        order.push(id);
        stack.extend(&children[id]);
    }
    let mut mapped = vec![-1_i32; points.len()];
    for (i, &id) in order.iter().enumerate() {
        mapped[id] = i as i32;
    }
    let mut contours = Vec::new();
    let mut hierarchy = Vec::new();
    for &id in &order {
        let siblings = &children[parents[id]];
        let pos = siblings.iter().position(|&x| x == id).ok_or(Error::Argument("invalid contour siblings"))?;
        let next = if pos > 0 { mapped[siblings[pos - 1]] } else { -1 };
        let previous = if pos + 1 < siblings.len() { mapped[siblings[pos + 1]] } else { -1 };
        let child = children[id].last().map_or(-1, |&c| mapped[c]);
        hierarchy.push([next, previous, child, mapped[parents[id]]]);
        contours.push(std::mem::take(&mut points[id]));
    }
    Ok(Contours { contours, hierarchy })
}

mod sealed {
    pub trait Coordinate: Copy {
        fn f32(self) -> f32;
        fn f64(self) -> f64;
        fn turn(a: [Self; 2], b: [Self; 2], c: [Self; 2]) -> i32;
    }
    impl Coordinate for i32 {
        fn f32(self) -> f32 {
            self as f32
        }
        fn f64(self) -> f64 {
            self as f64
        }
        fn turn(a: [Self; 2], b: [Self; 2], c: [Self; 2]) -> i32 {
            let dx0 = b[0].wrapping_sub(a[0]);
            let dy0 = b[1].wrapping_sub(a[1]);
            let dx = c[0].wrapping_sub(b[0]);
            let dy = c[1].wrapping_sub(b[1]);
            match dy.wrapping_mul(dx0).cmp(&dx.wrapping_mul(dy0)) {
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => 2,
                _ => 3,
            }
        }
    }
    impl Coordinate for f32 {
        fn f32(self) -> f32 {
            self
        }
        fn f64(self) -> f64 {
            self as f64
        }
        fn turn(a: [Self; 2], b: [Self; 2], c: [Self; 2]) -> i32 {
            let u = (c[1] - b[1]) * (b[0] - a[0]);
            let v = (c[0] - b[0]) * (b[1] - a[1]);
            if u > v {
                1
            } else if u < v {
                2
            } else {
                3
            }
        }
    }
}
/// CV_32S/CV_32F coordinate types for ports of cv2 contour geometry functions.
pub trait Coordinate: sealed::Coordinate {}
impl Coordinate for i32 {}
impl Coordinate for f32 {}

fn validate<T: Coordinate>(p: &[[T; 2]]) -> Result<()> {
    if p.iter().flatten().any(|v| !v.f64().is_finite()) {
        return Err(Error::Argument("finite contour coordinates required"));
    }
    Ok(())
}

/// Port of cv2.contourArea in luma/dust_scratches.py and studio.py; optional signed area.
pub fn contour_area<T: Coordinate>(p: &[[T; 2]], oriented: bool) -> Result<f64> {
    validate(p)?;
    let mut area = 0.0;
    if let Some(&last) = p.last() {
        let mut prev = last;
        for &q in p {
            area += prev[0].f32() as f64 * q[1].f32() as f64 - prev[1].f32() as f64 * q[0].f32() as f64;
            prev = q;
        }
    }
    area *= 0.5;
    Ok(if oriented { area } else { area.abs() })
}

/// Port of cv2.arcLength in luma/dust_scratches.py; segment norms round to float32 before summation.
pub fn arc_length<T: Coordinate>(p: &[[T; 2]], closed: bool) -> Result<f64> {
    validate(p)?;
    if p.len() < 2 {
        return Ok(0.0);
    }
    let mut prev = if closed { p[p.len() - 1] } else { p[0] };
    let mut sum = 0.0;
    for &q in p {
        let dx = q[0].f32() - prev[0].f32();
        let dy = q[1].f32() - prev[1].f32();
        sum += (dx * dx + dy * dy).sqrt() as f64;
        prev = q;
    }
    Ok(sum)
}

/// Port of cv2.boundingRect(contour) in luma/dust_scratches.py; returns (x,y,width,height).
pub fn bounding_rect<T: Coordinate>(p: &[[T; 2]]) -> Result<[i32; 4]> {
    validate(p)?;
    if p.is_empty() {
        return Ok([0; 4]);
    }
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for q in p {
        for c in 0..2 {
            lo[c] = lo[c].min(q[c].f64().floor());
            hi[c] = hi[c].max(q[c].f64().floor());
        }
    }
    if (0..2).any(|c| lo[c] < i32::MIN as f64 || hi[c] > i32::MAX as f64 || hi[c] - lo[c] + 1.0 > i32::MAX as f64) {
        return Err(Error::Argument("contour bounds exceed int32"));
    }
    Ok([lo[0] as i32, lo[1] as i32, (hi[0] - lo[0] + 1.0) as i32, (hi[1] - lo[1] + 1.0) as i32])
}

/// Port of cv2.isContourConvex in luma/studio.py; collinear or repeated edges are not strictly convex.
pub fn is_contour_convex<T: Coordinate>(p: &[[T; 2]]) -> Result<bool> {
    validate(p)?;
    if p.len() < 3 {
        return Ok(false);
    }
    let mut orientation = 0;
    for i in 0..p.len() {
        orientation |= T::turn(p[(i + p.len() - 2) % p.len()], p[(i + p.len() - 1) % p.len()], p[i]);
        if orientation == 3 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn sklansky(p: &[[i32; 2]], start: usize, end: usize, nsign: i64, sign2: i128) -> Vec<usize> {
    if start == end || p[start] == p[end] {
        return vec![start];
    }
    let inc = if end > start { 1_isize } else { -1 };
    let mut previous = start as isize;
    let mut current = previous + inc;
    let mut next = current + inc;
    let mut stack = vec![previous, current, next];
    let after = end as isize + inc;
    while next != after {
        let a = p[previous as usize];
        let b = p[current as usize];
        let c = p[next as usize];
        let by = c[1] as i64 - b[1] as i64;
        if by.signum() != nsign {
            let ax = b[0] as i64 - a[0] as i64;
            let ay = b[1] as i64 - a[1] as i64;
            let bx = c[0] as i64 - b[0] as i64;
            let cross = ay as i128 * bx as i128 - ax as i128 * by as i128;
            if cross.signum() == sign2 && (ax != 0 || ay != 0) {
                previous = current;
                current = next;
                next += inc;
                stack.push(next);
            } else if previous == start as isize {
                current = next;
                stack[1] = current;
                next += inc;
                stack[2] = next;
            } else {
                let n = stack.len();
                stack[n - 2] = next;
                current = previous;
                previous = stack[n - 4];
                stack.pop();
            }
        } else {
            next += inc;
            let n = stack.len();
            stack[n - 1] = next;
        }
    }
    stack.pop();
    stack.into_iter().map(|i| i as usize).collect()
}

/// Port of cv2.convexHull(contour), integer points, default returnPoints, selectable winding.
/// Preserves OpenCV's starting vertex and cyclic index-order adjustment.
pub fn convex_hull(points: &[[i32; 2]], clockwise: bool) -> Result<Vec<[i32; 2]>> {
    if points.is_empty() {
        return Ok(Vec::new());
    }
    let mut indices: Vec<usize> = (0..points.len()).collect();
    indices.sort_by_key(|&i| (points[i][0], points[i][1], i));
    let p: Vec<_> = indices.iter().map(|&i| points[i]).collect();
    let n = p.len();
    if p[0] == p[n - 1] {
        return Ok(vec![points[0]]);
    }
    let mut miny = 0;
    let mut maxy = 0;
    for i in 1..n {
        if p[i][1] < p[miny][1] {
            miny = i;
        }
        if p[i][1] > p[maxy][1] {
            maxy = i;
        }
    }
    let mut tl = sklansky(&p, 0, maxy, -1, 1);
    let mut tr = sklansky(&p, n - 1, maxy, -1, -1);
    if !clockwise {
        std::mem::swap(&mut tl, &mut tr);
    }
    let mut hull = tl[..tl.len() - 1].to_vec();
    hull.extend(tr[1..].iter().rev());
    let stop = if tr.len() > 2 {
        Some(tr[1])
    } else if tl.len() > 2 {
        Some(tl[tl.len() - 2])
    } else {
        None
    };
    let mut bl = sklansky(&p, 0, miny, 1, -1);
    let mut br = sklansky(&p, n - 1, miny, 1, 1);
    if clockwise {
        std::mem::swap(&mut bl, &mut br);
    }
    if let Some(stop) = stop {
        let check = if bl.len() > 2 {
            Some(bl[1])
        } else if bl.len() + br.len() > 2 {
            Some(br[2 - bl.len()])
        } else {
            None
        };
        if check.is_some_and(|c| p[c] == p[stop]) {
            bl.truncate(2);
            br.truncate(2);
        }
    }
    hull.extend(&bl[..bl.len() - 1]);
    hull.extend(br[1..].iter().rev());
    for i in &mut hull {
        *i = indices[*i];
    }
    let n = hull.len();
    if n >= 3 {
        let (mut mini, mut maxi, mut lt) = (0, 0, 0);
        for i in 1..n {
            lt += usize::from(hull[i - 1] < hull[i]);
            if lt > 1 && lt + 2 <= i {
                break;
            }
            if hull[i] < hull[mini] {
                mini = i;
            }
            if hull[i] > hull[maxi] {
                maxi = i;
            }
        }
        let dist = maxi.abs_diff(mini);
        if (dist == 1 || dist == n - 1) && (lt <= 1 || lt >= n - 2) {
            let ascending = (maxi + 1) % n == mini;
            let start = if ascending { mini } else { maxi };
            if start > 0 && (0..n - 1).all(|i| ascending == (hull[(start + i) % n] < hull[(start + i + 1) % n])) {
                hull.rotate_left(start);
            }
        }
    }
    Ok(hull.iter().map(|&i| points[i]).collect())
}
