use crate::{array, Error, Result};
use ndarray::Array2;
use std::collections::VecDeque;

/// Port of cv2.distanceTransform(..., DIST_L2, maskSize), float32 CPU/IPP chamfer path.
/// Mask size is 3 or 5; zero pixels seed distances, and an all-nonzero mask returns FLT_MAX.
pub fn distance_transform(src: &Array2<u8>, mask_size: usize) -> Result<Array2<f32>> {
    let (h, w) = src.dim();
    array::shape(h, w, 1)?;
    let offsets: &[(isize, isize, f32)] = match mask_size {
        3 => &[(-1, -1, 1.3693), (-1, 0, 0.955), (-1, 1, 1.3693), (0, -1, 0.955)],
        5 => &[
            (-2, -1, 2.1969),
            (-2, 1, 2.1969),
            (-1, -2, 2.1969),
            (-1, -1, 1.4),
            (-1, 0, 1.0),
            (-1, 1, 1.4),
            (-1, 2, 2.1969),
            (0, -1, 1.0),
        ],
        _ => return Err(Error::Argument("L2 mask size must be 3 or 5")),
    };
    let mut dst = src.mapv(|v| if v == 0 { 0.0 } else { f32::MAX });
    for y in 0..h {
        for x in 0..w {
            for &(dy, dx, cost) in offsets {
                let yy = y as isize + dy;
                let xx = x as isize + dx;
                if yy >= 0 && xx >= 0 && yy < h as isize && xx < w as isize {
                    dst[[y, x]] = dst[[y, x]].min(dst[[yy as usize, xx as usize]] + cost);
                }
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            for &(dy, dx, cost) in offsets {
                let yy = y as isize - dy;
                let xx = x as isize - dx;
                if yy >= 0 && xx >= 0 && yy < h as isize && xx < w as isize {
                    dst[[y, x]] = dst[[y, x]].min(dst[[yy as usize, xx as usize]] + cost);
                }
            }
        }
    }
    Ok(dst)
}

/// Port of the return tuple of cv2.connectedComponentsWithStats (including background label zero).
#[derive(Debug, Clone)]
pub struct Components {
    /// Number of labels from cv2.connectedComponents, including background.
    pub count: usize,
    /// CV_32S labels, using OpenCV default Spaghetti scan ordering.
    pub labels: Array2<i32>,
    /// cv2 CC_STAT_LEFT, TOP, WIDTH, HEIGHT, AREA columns, in that order.
    pub stats: Array2<i32>,
    /// cv2 float64 (x,y) centroids; absent background is (NaN,NaN).
    pub centroids: Array2<f64>,
}

fn label(src: &Array2<u8>, connectivity: usize) -> Result<(usize, Array2<i32>)> {
    let (h, w) = src.dim();
    array::shape(h, w, 1)?;
    if connectivity != 4 && connectivity != 8 {
        return Err(Error::Argument("connectivity must be 4 or 8"));
    }
    if src.len() > i32::MAX as usize {
        return Err(Error::Shape("too many pixels for CV_32S component labels"));
    }
    let mut labels = Array2::<i32>::zeros((h, w));
    let mut count = 1;
    let mut queue = VecDeque::new();
    // 8-connected Spaghetti visits 2x2 blocks; 4-connected visits individual raster pixels.
    let step = if connectivity == 8 { 2 } else { 1 };
    for by in (0..h).step_by(step) {
        for bx in (0..w).step_by(step) {
            for y in by..(by + step).min(h) {
                for x in bx..(bx + step).min(w) {
                    if src[[y, x]] == 0 || labels[[y, x]] != 0 {
                        continue;
                    }
                    labels[[y, x]] = count;
                    queue.push_back((y, x));
                    while let Some((yy, xx)) = queue.pop_front() {
                        for dy in -1_isize..=1 {
                            for dx in -1_isize..=1 {
                                if dy == 0 && dx == 0 || connectivity == 4 && dy != 0 && dx != 0 {
                                    continue;
                                }
                                let ny = yy as isize + dy;
                                let nx = xx as isize + dx;
                                if ny < 0 || nx < 0 || ny >= h as isize || nx >= w as isize {
                                    continue;
                                }
                                let p = [ny as usize, nx as usize];
                                if src[p] != 0 && labels[p] == 0 {
                                    labels[p] = count;
                                    queue.push_back((p[0], p[1]));
                                }
                            }
                        }
                    }
                    count += 1;
                }
            }
        }
    }
    Ok((count as usize, labels))
}

/// Port of cv2.connectedComponents, CV_32S output, 4/8 connectivity, default algorithm.
pub fn connected_components(src: &Array2<u8>, connectivity: usize) -> Result<(usize, Array2<i32>)> {
    label(src, connectivity)
}

/// Port of cv2.connectedComponentsWithStats in luma/sky_mask.py and dust.py.
pub fn connected_components_with_stats(src: &Array2<u8>, connectivity: usize) -> Result<Components> {
    let (count, labels) = label(src, connectivity)?;
    let mut stats = Array2::<i32>::zeros((count, 5));
    let mut sums = vec![[0_u64; 2]; count];
    for i in 0..count {
        stats[[i, 0]] = i32::MAX;
        stats[[i, 1]] = i32::MAX;
        stats[[i, 2]] = i32::MIN;
        stats[[i, 3]] = i32::MIN;
    }
    for ((y, x), &id) in labels.indexed_iter() {
        let i = id as usize;
        stats[[i, 0]] = stats[[i, 0]].min(x as i32);
        stats[[i, 1]] = stats[[i, 1]].min(y as i32);
        stats[[i, 2]] = stats[[i, 2]].max(x as i32);
        stats[[i, 3]] = stats[[i, 3]].max(y as i32);
        stats[[i, 4]] += 1;
        sums[i][0] += x as u64;
        sums[i][1] += y as u64;
    }
    let mut centroids = Array2::from_elem((count, 2), f64::NAN);
    for i in 0..count {
        if stats[[i, 4]] == 0 {
            stats[[i, 0]] = -1;
            stats[[i, 2]] = 0;
            stats[[i, 3]] = 0;
        } else {
            stats[[i, 2]] -= stats[[i, 0]] - 1;
            stats[[i, 3]] -= stats[[i, 1]] - 1;
            for c in 0..2 {
                centroids[[i, c]] = sums[i][c] as f64 / stats[[i, 4]] as f64;
            }
        }
    }
    Ok(Components { count, labels, stats, centroids })
}

/// Port of the cv2.floodFill return tuple (area, image, mask, bounding rectangle).
#[derive(Debug, Clone)]
pub struct FloodFill {
    /// Number of pixels filled by cv2.floodFill.
    pub area: usize,
    /// Filled image from cv2.floodFill.
    pub image: Array2<u8>,
    /// cv2.floodFill mask, including its one-pixel border (height+2,width+2).
    pub mask: Array2<u8>,
    /// cv2.floodFill component bounds (x,y,width,height).
    pub rect: [i32; 4],
}

/// Port of cv2.floodFill(outside, None, seed, new_value) in luma/sky_mask.py.
/// Exact-value, 4-connected uint8 fill with the implicit mask (the only oracle call pattern).
pub fn flood_fill(src: &Array2<u8>, seed: (usize, usize), new_value: u8) -> Result<FloodFill> {
    let (h, w) = src.dim();
    array::shape(h, w, 1)?;
    if seed.0 >= w || seed.1 >= h {
        return Err(Error::Argument("flood-fill seed outside image"));
    }
    let mut image = src.clone();
    let mut mask = Array2::<u8>::zeros((h + 2, w + 2));
    for y in 0..h + 2 {
        mask[[y, 0]] = 1;
        mask[[y, w + 1]] = 1;
    }
    for x in 0..w + 2 {
        mask[[0, x]] = 1;
        mask[[h + 1, x]] = 1;
    }
    let old = src[[seed.1, seed.0]];
    let mut queue = VecDeque::from([(seed.1, seed.0)]);
    mask[[seed.1 + 1, seed.0 + 1]] = 1;
    let (mut x0, mut y0, mut x1, mut y1) = (seed.0, seed.1, seed.0, seed.1);
    let mut area = 0;
    while let Some((y, x)) = queue.pop_front() {
        image[[y, x]] = new_value;
        area += 1;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
        for (dy, dx) in [(-1_isize, 0_isize), (0, -1), (1, 0), (0, 1)] {
            let ny = (y as isize + dy + 1) as usize;
            let nx = (x as isize + dx + 1) as usize;
            if mask[[ny, nx]] == 0 && src[[ny - 1, nx - 1]] == old {
                mask[[ny, nx]] = 1;
                queue.push_back((ny - 1, nx - 1));
            }
        }
    }
    Ok(FloodFill { area, image, mask, rect: [x0 as i32, y0 as i32, (x1 - x0 + 1) as i32, (y1 - y0 + 1) as i32] })
}
