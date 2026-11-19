//! Image-space maths shared by the canvas tools: where a point of the screen is on the displayed frame, and where a
//! point of the displayed frame is on the source photo through the whole geometry (lens and manual transform,
//! quarter turns, flip, straighten, upright, crop).
//!
//! * [`ViewTransform`]: screen points ↔ normalized coordinates of the displayed frame (`PhotoView.normalized_point`,
//!   `free_point` and the inverse used to draw overlays, luma/widgets.py).
//! * [`CoordinateMap`]: displayed frame ↔ source, port of `StudioTools.source_point`, `source_points`,
//!   `frame_affine`, `screen_point` and `gradient_point` (luma/studio.py): the geometry stage is run on a picture
//!   whose pixels hold their own source coordinates, so every geometry the engine knows maps points exactly as it
//!   maps pixels, and an affine map fitted to it carries points that lie off the photo.
//!
//! Normalized coordinates are fractions: (0, 0) the top left corner, (1, 1) the bottom right one.

use eframe::egui::{Pos2, Rect};
use grainy_core::Settings;
use grainy_geometry::UseCrop;
use ndarray::Array3;

/// A 2×3 affine map of normalized coordinates (`[[a, b, c], [d, e, f]]`: x' = a x + b y + c).
pub type Affine = [[f64; 3]; 2];

/// The identity map (`np.eye(2, 3)`).
pub const IDENTITY: Affine = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

/// Applies an affine map to a point.
pub fn apply(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    [m[0][0] * p[0] + m[0][1] * p[1] + m[0][2], m[1][0] * p[0] + m[1][1] * p[1] + m[1][2]]
}

/// Where the displayed frame is on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    /// The frame's rectangle in screen points.
    pub rect: Rect,
}

impl ViewTransform {
    /// Port of `normalized_point`: the point on the frame, clamped to it.
    pub fn to_frame(&self, point: Pos2) -> [f64; 2] {
        let p = self.to_frame_free(point);
        [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]
    }

    /// Port of `free_point`: the point on the frame, off it too (a gradient may start outside the photo).
    pub fn to_frame_free(&self, point: Pos2) -> [f64; 2] {
        let r = self.rect;
        [
            f64::from(point.x - r.left()) / f64::from(r.width().max(f32::EPSILON)),
            f64::from(point.y - r.top()) / f64::from(r.height().max(f32::EPSILON)),
        ]
    }

    /// The screen point of a frame point (overlays).
    pub fn to_screen(&self, p: [f64; 2]) -> Pos2 {
        let r = self.rect;
        Pos2::new(r.left() + (p[0] * f64::from(r.width())) as f32, r.top() + (p[1] * f64::from(r.height())) as f32)
    }

    /// Screen points per frame pixel for a frame `width` pixels wide.
    pub fn scale(&self, width: usize) -> f32 {
        self.rect.width() / width.max(1) as f32
    }
}

/// Errors of the coordinate map: the geometry stage's message.
pub type Error = grainy_geometry::Error;

/// The inverse geometry map of one photo's settings (`coordinates` of luma/studio.py) and its fitted affine maps.
#[derive(Debug, Clone)]
pub struct CoordinateMap {
    /// H×W×3 float32: source x, source y (fractions) and the validity (1 inside the source, less where the
    /// geometry brought in empty canvas).
    coordinates: Array3<f32>,
    /// The source's (width, height) in pixels.
    source: (usize, usize),
    forward: Affine,
    backward: Affine,
}

/// `np.round`-free Python `round` for indices: round half to even.
fn py_round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

impl CoordinateMap {
    /// Builds the map for a source of `width` × `height` pixels (the oracle uses its 720-pixel live source) with
    /// the settings as shown (`panel_keys.effective`: without the lens and transform panels when they are off).
    pub fn new(width: usize, height: usize, settings: &Settings) -> Result<Self, Error> {
        let s = grainy_core::effective(settings);
        let (w, h) = (width.max(1), height.max(1));
        let (dx, dy) = ((w.max(2) - 1) as f32, (h.max(2) - 1) as f32);
        let dx = if w == 1 { 1.0 } else { dx };
        let dy = if h == 1 { 1.0 } else { dy };
        let grid = Array3::from_shape_fn((h, w, 3), |(y, x, c)| match c {
            0 => x as f32 / dx,
            1 => y as f32 / dy,
            _ => 1.0,
        });
        let coordinates = grainy_geometry::geometry(&grid, &s, UseCrop::Yes, None)?.into_owned();
        let (forward, backward) = fit(&coordinates);
        Ok(Self { coordinates, source: (w, h), forward, backward })
    }

    /// The displayed frame's (width, height) in map pixels.
    pub fn frame_size(&self) -> (usize, usize) {
        let (h, w, _) = self.coordinates.dim();
        (w, h)
    }

    /// The source's (width, height).
    pub fn source_size(&self) -> (usize, usize) {
        self.source
    }

    /// Port of `source_point`: the source point under a frame point, or `None` where the frame shows empty canvas.
    pub fn source_point(&self, point: [f64; 2]) -> Option<[f64; 2]> {
        let (h, w, _) = self.coordinates.dim();
        let y = (py_round(point[1] * (h - 1) as f64).max(0) as usize).min(h - 1);
        let x = (py_round(point[0] * (w - 1) as f64).max(0) as usize).min(w - 1);
        let v = self.coordinates.slice(ndarray::s![y, x, ..]);
        (v[2] > 0.9).then(|| [f64::from(v[0].clamp(0.0, 1.0)), f64::from(v[1].clamp(0.0, 1.0))])
    }

    /// Port of `source_points`: the source points of a stroke (points clamped to the frame; those on empty canvas
    /// dropped).
    pub fn source_points(&self, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
        let (h, w, _) = self.coordinates.dim();
        points
            .iter()
            .filter_map(|p| {
                let x = py_round(p[0].clamp(0.0, 1.0) * (w - 1) as f64) as usize;
                let y = py_round(p[1].clamp(0.0, 1.0) * (h - 1) as f64) as usize;
                let v = self.coordinates.slice(ndarray::s![y.min(h - 1), x.min(w - 1), ..]);
                (v[2] > 0.9).then(|| [f64::from(v[0].clamp(0.0, 1.0)), f64::from(v[1].clamp(0.0, 1.0))])
            })
            .collect()
    }

    /// Port of `frame_affine`: (frame → source, source → frame) affine maps fitted to the inverse geometry map.
    pub fn affine(&self) -> (Affine, Affine) {
        (self.forward, self.backward)
    }

    /// Port of `gradient_point`: a frame point in the source, exact on the photo, carried by the fitted map off it.
    pub fn gradient_point(&self, point: [f64; 2]) -> [f64; 2] {
        let inside = (0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1]);
        inside.then(|| self.source_point(point)).flatten().unwrap_or_else(|| apply(&self.forward, point))
    }

    /// Port of `screen_point`: where a source point is on the displayed frame: the nearest map pixel inside the
    /// crop, the fitted map outside it.
    pub fn screen_point(&self, point: [f64; 2]) -> [f64; 2] {
        let (h, w, _) = self.coordinates.dim();
        if (0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1]) {
            let mut best = (f64::INFINITY, 0usize, 0usize);
            for y in 0..h {
                for x in 0..w {
                    if self.coordinates[[y, x, 2]] < 0.9 {
                        continue;
                    }
                    let dx = f64::from(self.coordinates[[y, x, 0]]) - point[0];
                    let dy = f64::from(self.coordinates[[y, x, 1]]) - point[1];
                    let d = dx * dx + dy * dy;
                    if d < best.0 {
                        best = (d, x, y);
                    }
                }
            }
            if best.0.is_finite() && best.0 <= 0.001 {
                return [best.1 as f64 / (w.max(2) - 1) as f64, best.2 as f64 / (h.max(2) - 1) as f64];
            }
        }
        apply(&self.backward, point)
    }
}

/// The least-squares affine fit of `frame_affine` (numpy's `lstsq` on every `step`-th valid pixel) and its inverse.
fn fit(coordinates: &Array3<f32>) -> (Affine, Affine) {
    let (h, w, _) = coordinates.dim();
    let step = ((h.max(w) as f64 / 96.0) as usize).max(1);
    // Normal equations A^T A x = A^T b with rows (u, v, 1).
    let mut ata = [[0.0f64; 3]; 3];
    let mut atb = [[0.0f64; 2]; 3];
    let mut count = 0usize;
    let (fw, fh) = ((w.max(2) - 1) as f64, (h.max(2) - 1) as f64);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            if coordinates[[y, x, 2]] <= 0.9 {
                continue;
            }
            let row = [x as f64 / fw, y as f64 / fh, 1.0];
            let target = [f64::from(coordinates[[y, x, 0]]), f64::from(coordinates[[y, x, 1]])];
            for i in 0..3 {
                for j in 0..3 {
                    ata[i][j] += row[i] * row[j];
                }
                for k in 0..2 {
                    atb[i][k] += row[i] * target[k];
                }
            }
            count += 1;
        }
    }
    let forward = if count >= 3 {
        match invert3(&ata) {
            Some(inv) => {
                let mut f = [[0.0; 3]; 2];
                for k in 0..2 {
                    for i in 0..3 {
                        f[k][i] = (0..3).map(|j| inv[i][j] * atb[j][k]).sum();
                    }
                }
                f
            }
            None => IDENTITY,
        }
    } else {
        IDENTITY
    };
    let square = [forward[0], forward[1], [0.0, 0.0, 1.0]];
    match (det3(&square).abs() >= 1e-9).then(|| invert3(&square)).flatten() {
        Some(inv) => (forward, [inv[0], inv[1]]),
        None => (IDENTITY, IDENTITY),
    }
}

fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn invert3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let d = det3(m);
    if d == 0.0 || !d.is_finite() {
        return None;
    }
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    Some([
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    #[test]
    fn view_transform_round_trip() {
        let t = ViewTransform { rect: Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(400.0, 200.0)) };
        assert_eq!(t.to_frame(Pos2::new(300.0, 150.0)), [0.5, 0.5]);
        assert_eq!(t.to_frame(Pos2::new(0.0, 0.0)), [0.0, 0.0]);
        assert_eq!(t.to_frame_free(Pos2::new(0.0, 0.0)), [-0.25, -0.25]);
        assert_eq!(t.to_screen([0.25, 1.0]), Pos2::new(200.0, 250.0));
    }

    #[test]
    fn plain_settings_map_points_to_themselves() {
        let s = grainy_core::defaults().clone();
        let map = CoordinateMap::new(97, 65, &s).unwrap();
        assert_eq!(map.source_point([0.5, 0.5]), Some([0.5, 0.5]));
        let (forward, backward) = map.affine();
        for (a, b) in forward.iter().flatten().zip(IDENTITY.iter().flatten()) {
            assert!((a - b).abs() < 1e-9);
        }
        assert_eq!(map.screen_point([0.25, 0.75]), [0.25, 0.75]);
        let off = map.screen_point([1.5, -0.5]);
        assert!((off[0] - 1.5).abs() < 1e-9 && (off[1] + 0.5).abs() < 1e-9, "{off:?} {backward:?}");
    }

    #[test]
    fn quarter_turn_and_crop() {
        let mut s = grainy_core::defaults().clone();
        s.set("rotation", 1);
        let map = CoordinateMap::new(41, 21, &s).unwrap();
        // A clockwise quarter turn: the frame's top left corner is the source's bottom left.
        assert_eq!(map.frame_size(), (21, 41));
        assert_eq!(map.source_point([0.0, 0.0]), Some([0.0, 1.0]));
        assert_eq!(map.source_point([1.0, 0.0]), Some([0.0, 0.0]));
        s.set("rotation", 0);
        s.set("crop", serde_json::json!([0.5, 0.0, 1.0, 1.0]));
        let map = CoordinateMap::new(41, 21, &s).unwrap();
        assert_eq!(map.source_point([0.0, 0.5]), Some([0.5, 0.5]));
        let p = map.gradient_point([-1.0, 0.5]);
        assert!((p[0] + 0.0).abs() < 1e-6 && (p[1] - 0.5).abs() < 1e-6, "{p:?}");
    }
}
