//! Linear and radial gradients between the displayed frame and the source photo, and their handles on screen.
//!
//! Port of luma/studio.py `gradient_shape`, `radial_from_screen`, `radial_on_screen`, `component_handles`,
//! `move_component_handles` (the maths part) and of luma/widgets.py `PhotoView`'s handle hit test and drag
//! (`mousePressEvent`, `mouseMoveEvent`, `mouseReleaseEvent` in `edit_component` mode).
//!
//! Points are fractions of the displayed frame (screen side) or of the source (stored side); the maps come from
//! [`CoordinateMap`] (`frame_affine`, `gradient_point`, `screen_point`).

use serde_json::{Map, Value};

use crate::imagespace::{apply, CoordinateMap};

/// A JSON object.
type Obj = Map<String, Value>;

/// Screen pixels around a handle that pick it (`manhattanLength() <= 14`).
pub const HANDLE_REACH: f32 = 14.0;

/// Python's `round(x, 3)`: the decimal rounding of the exact binary value.
fn round3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap_or(x)
}

fn points_of(component: &Obj) -> Vec<[f64; 2]> {
    component
        .get("points")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])
                })
                .collect()
        })
        .unwrap_or_default()
}

fn point_value(p: [f64; 2]) -> Value {
    Value::Array(vec![Value::from(p[0]), Value::from(p[1])])
}

/// The frame's pixel spans `(max(1, dw-1), max(1, dh-1))`.
fn spans(map: &CoordinateMap) -> (f64, f64) {
    let (dw, dh) = map.frame_size();
    ((dw.max(2) - 1) as f64, (dh.max(2) - 1) as f64)
}

fn matvec(m: [[f64; 2]; 2], v: [f64; 2]) -> [f64; 2] {
    [m[0][0] * v[0] + m[0][1] * v[1], m[1][0] * v[0] + m[1][1] * v[1]]
}

/// Port of `radial_from_screen`: a radial component from its shape on screen: `centre` (frame fractions),
/// `half` axes in frame pixels, `angle` in degrees. The result is `base` with `points` (the ellipse's corners in
/// its own frame, in the source) and `angle` (its turn in the source, degrees, rounded to 3 places).
pub fn radial_from_screen(map: &CoordinateMap, centre: [f64; 2], half: [f64; 2], angle: f64, base: &Obj) -> Obj {
    let (forward, _) = map.affine();
    let (sw, sh) = map.source_size();
    let (sw, sh) = (sw as f64, sh as f64);
    let (fx, fy) = spans(map);
    // m = diag(sw, sh) @ forward[:, :2] @ diag(1/fx, 1/fy)
    let m = [
        [sw * forward[0][0] * (1.0 / fx), sw * forward[0][1] * (1.0 / fy)],
        [sh * forward[1][0] * (1.0 / fx), sh * forward[1][1] * (1.0 / fy)],
    ];
    let a = angle.to_radians();
    let u = matvec(m, [a.cos() * half[0].max(0.5), a.sin() * half[0].max(0.5)]);
    let v = matvec(m, [-a.sin() * half[1].max(0.5), a.cos() * half[1].max(0.5)]);
    let c = apply(&forward, centre);
    let r = [(u[0].hypot(u[1]) / sw).max(0.002), (v[0].hypot(v[1]) / sh).max(0.002)];
    let mut out = base.clone();
    out.insert(
        "points".into(),
        Value::Array(vec![point_value([c[0] - r[0], c[1] - r[1]]), point_value([c[0] + r[0], c[1] + r[1]])]),
    );
    out.insert("angle".into(), Value::from(round3(u[1].atan2(u[0]).to_degrees())));
    out
}

/// Port of `radial_on_screen`: (centre in frame fractions, half axes in frame pixels, angle in degrees) of a
/// radial component as shown.
pub fn radial_on_screen(map: &CoordinateMap, component: &Obj) -> ([f64; 2], [f64; 2], f64) {
    let (_, backward) = map.affine();
    let (sw, sh) = map.source_size();
    let (sw, sh) = (sw as f64, sh as f64);
    let (fx, fy) = spans(map);
    let points = points_of(component);
    let start = points.first().copied().unwrap_or([0.5, 0.5]);
    let end = points.last().copied().unwrap_or([0.5, 0.5]);
    let c = [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0];
    let r = [(end[0] - start[0]).abs() / 2.0, (end[1] - start[1]).abs() / 2.0];
    let a = component.get("angle").and_then(Value::as_f64).unwrap_or(0.0).to_radians();
    // m = diag(fx, fy) @ backward[:, :2] @ diag(1/sw, 1/sh)
    let m = [
        [fx * backward[0][0] * (1.0 / sw), fx * backward[0][1] * (1.0 / sh)],
        [fy * backward[1][0] * (1.0 / sw), fy * backward[1][1] * (1.0 / sh)],
    ];
    let u = matvec(m, [a.cos() * r[0] * sw, a.sin() * r[0] * sw]);
    let v = matvec(m, [-a.sin() * r[1] * sh, a.cos() * r[1] * sh]);
    (apply(&backward, c), [u[0].hypot(u[1]), v[0].hypot(v[1])], u[1].atan2(u[0]).to_degrees())
}

/// Port of `gradient_shape`: `{points}` (linear) or `{points, angle}` (radial) of a gradient drawn on screen from
/// the first to the last point. `None` without points.
pub fn gradient_shape(map: &CoordinateMap, kind: &str, points: &[[f64; 2]]) -> Option<Obj> {
    let (start, end) = (*points.first()?, *points.last()?);
    if kind == "linear" {
        let mut out = Obj::new();
        out.insert(
            "points".into(),
            Value::Array(vec![point_value(map.gradient_point(start)), point_value(map.gradient_point(end))]),
        );
        return Some(out);
    }
    let (fx, fy) = spans(map);
    let half = [(end[0] - start[0]).abs() / 2.0 * fx, (end[1] - start[1]).abs() / 2.0 * fy];
    let centre = [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0];
    Some(radial_from_screen(map, centre, half, 0.0, &Obj::new()))
}

/// Port of `component_handles`: (handles on screen, angle on screen) of a gradient: its two ends, or the corners
/// of a radial in its own frame.
pub fn component_handles(map: &CoordinateMap, component: &Obj) -> (Vec<[f64; 2]>, f64) {
    if component.get("type").and_then(Value::as_str) == Some("radial") {
        let (fx, fy) = spans(map);
        let (centre, half, angle) = radial_on_screen(map, component);
        let a = angle.to_radians();
        let corner = [(half[0] * a.cos() - half[1] * a.sin()) / fx, (half[0] * a.sin() + half[1] * a.cos()) / fy];
        return (
            vec![[centre[0] - corner[0], centre[1] - corner[1]], [centre[0] + corner[0], centre[1] + corner[1]]],
            angle,
        );
    }
    let points = points_of(component);
    let ends = [points.first().copied().unwrap_or([0.5, 0.5]), points.last().copied().unwrap_or([0.5, 0.5])];
    (ends.iter().map(|p| map.screen_point(*p)).collect(), 0.0)
}

/// Port of the maths of `move_component_handles`: the component after its handles were dragged to `handles`
/// (screen fractions); `angle` is the radial's angle on screen (`handle_angle`).
pub fn moved_component(map: &CoordinateMap, component: &Obj, handles: &[[f64; 2]], angle: f64) -> Obj {
    let (Some(&first), Some(&second)) = (handles.first(), handles.get(1)) else { return component.clone() };
    if component.get("type").and_then(Value::as_str) == Some("radial") {
        let (fx, fy) = spans(map);
        let centre = [(first[0] + second[0]) / 2.0, (first[1] + second[1]) / 2.0];
        let d = [(second[0] - centre[0]) * fx, (second[1] - centre[1]) * fy];
        let a = angle.to_radians();
        let local = [(d[0] * a.cos() + d[1] * a.sin()).abs(), (-d[0] * a.sin() + d[1] * a.cos()).abs()];
        return radial_from_screen(map, centre, local, angle, component);
    }
    let mut updated = component.clone();
    updated
        .insert("points".into(), Value::Array(handles.iter().map(|p| point_value(map.gradient_point(*p))).collect()));
    updated
}

/// The handles the view hit-tests: the two ends and their middle (`tool_handles + [mean]`).
pub fn hit_points(handles: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out = handles.to_vec();
    if !handles.is_empty() {
        let n = handles.len() as f64;
        out.push([handles.iter().map(|p| p[0]).sum::<f64>() / n, handles.iter().map(|p| p[1]).sum::<f64>() / n]);
    }
    out
}

/// Port of the press test of `PhotoView.mousePressEvent` in `edit_component`: the first handle (ends, then the
/// middle) whose screen position is within [`HANDLE_REACH`] (Manhattan) of the pointer. `to_screen` maps a frame
/// point to screen points.
pub fn hit_handle(
    handles: &[[f64; 2]],
    pointer: eframe::egui::Pos2,
    to_screen: impl Fn([f64; 2]) -> eframe::egui::Pos2,
) -> Option<usize> {
    hit_points(handles).iter().position(|p| {
        let s = to_screen(*p);
        // Qt rounds both to whole pixels (`toPoint`) before the Manhattan length.
        (s.x.round() - pointer.x.round()).abs() + (s.y.round() - pointer.y.round()).abs() <= HANDLE_REACH
    })
}

/// Port of the drag of `PhotoView.mouseMoveEvent` / `mouseReleaseEvent`: handle `index` < 2 goes to `point`, the
/// middle (2) moves both ends by `point - start`.
pub fn dragged(original: &[[f64; 2]], index: usize, start: [f64; 2], point: [f64; 2]) -> Vec<[f64; 2]> {
    let mut values = original.to_vec();
    if index < 2 {
        if let Some(v) = values.get_mut(index) {
            *v = point;
        }
    } else {
        for v in &mut values {
            v[0] += point[0] - start[0];
            v[1] += point[1] - start[1];
        }
    }
    values
}

/// `np.allclose(values, original)`: |a − b| ≤ 1e-8 + 1e-5 |b| everywhere.
pub fn allclose(values: &[[f64; 2]], original: &[[f64; 2]]) -> bool {
    values.len() == original.len()
        && values.iter().zip(original).all(|(a, b)| (0..2).all(|i| (a[i] - b[i]).abs() <= 1e-8 + 1e-5 * b[i].abs()))
}
