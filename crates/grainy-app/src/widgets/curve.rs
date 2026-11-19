//! The tone curve editor: port of luma/widgets.py `ToneCurve` (the 톤 곡선 panel's curve, the channel curves).
//!
//! [`ToneCurve`] is the widget's model: the same pointer actions give the same point lists, `changed` and
//! `committed` signals as the oracle's (drag a point, Ctrl+click adds one, right-click deletes one, double-click
//! resets; the endpoints move vertically only; the HDR axis when the photo is in HDR mode). [`show`] draws it in
//! egui and feeds it the frame's pointer events in order; the drawn curve is `grainy-tone`'s `curve_extended`,
//! the evaluation the engine applies.

use eframe::egui::{self, Color32, CornerRadius, Event, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use grainy_i18n::tr;
use ndarray::Array3;
use serde_json::Value;

/// The widget's height (`setFixedHeight(165)`).
pub const HEIGHT: f32 = 165.0;
/// The margin around the plotting area (`12` in `point_xy`).
pub const MARGIN: f64 = 12.0;
/// The identity curve the widget starts with and a double-click restores.
pub const DEFAULT_POINTS: [[f64; 2]; 5] = [[0.0, 0.0], [0.25, 0.25], [0.5, 0.5], [0.75, 0.75], [1.0, 1.0]];
/// A point is picked within this Manhattan distance in pixels.
const PICK: f64 = 24.0;

const BACKGROUND: Color32 = Color32::from_rgb(0x17, 0x1b, 0x20);
const GRID: Color32 = Color32::from_rgb(0x31, 0x37, 0x3e);
const DIAGONAL: Color32 = Color32::from_rgb(0x4e, 0x57, 0x60);
const SDR_LINE: Color32 = Color32::from_rgb(0x81, 0x85, 0x8c);
const CURVE: Color32 = Color32::from_rgb(0xdf, 0xc2, 0x9b);

/// The mouse buttons the widget reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// A signal of the widget, in the order it was emitted.
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    /// `changed(list)`: the new points (the window sets them live).
    Changed(Vec<[f64; 2]>),
    /// `committed()`: the interaction ended (the window commits one undo step).
    Committed,
}

/// Port of luma/widgets.py `ToneCurve`: the points, the dragged point and the axis, on a widget of `size`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneCurve {
    pub points: Vec<[f64; 2]>,
    /// The index of the point being dragged (`self.active`).
    pub active: Option<usize>,
    pub hdr: bool,
    /// The widget's (width, height) in pixels.
    pub size: (f64, f64),
}

impl Default for ToneCurve {
    fn default() -> Self {
        Self { points: DEFAULT_POINTS.to_vec(), active: None, hdr: false, size: (260.0, f64::from(HEIGHT)) }
    }
}

impl ToneCurve {
    /// Port of `set_points`.
    pub fn set_points(&mut self, points: &[[f64; 2]]) {
        self.points = points.to_vec();
    }

    /// Port of `set_hdr`.
    pub fn set_hdr(&mut self, enabled: bool) {
        self.hdr = enabled;
    }

    /// Port of `axis`: a value's place on the axis (the HDR axis gives SDR white the middle).
    pub fn axis(&self, value: f64) -> f64 {
        if !self.hdr {
            return value;
        }
        if value <= 1.0 {
            value * 0.5
        } else {
            0.5 + ((value + 0.055) / 1.055).powf(2.4).log2() / 8.0
        }
    }

    /// Port of `axis_value`: the inverse of [`ToneCurve::axis`].
    pub fn axis_value(&self, value: f64) -> f64 {
        if !self.hdr {
            return value;
        }
        if value <= 0.5 {
            value * 2.0
        } else {
            1.055 * 2f64.powf((value - 0.5) * 8.0).powf(1.0 / 2.4) - 0.055
        }
    }

    /// Port of `point_xy`: a point's position in the widget.
    pub fn point_xy(&self, point: [f64; 2]) -> [f64; 2] {
        let (w, h) = self.size;
        [MARGIN + self.axis(point[0]) * (w - 2.0 * MARGIN), MARGIN + (1.0 - self.axis(point[1])) * (h - 2.0 * MARGIN)]
    }

    /// The nearest point by Manhattan distance (`int(np.argmin(distances))`, the first of equals) and its distance.
    fn nearest(&self, position: [f64; 2]) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in self.points.iter().enumerate() {
            let [x, y] = self.point_xy(*p);
            let d = (x - position[0]).abs() + (y - position[1]).abs();
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best
    }

    /// Whether point `index` may move sideways and be deleted: not an endpoint, except the HDR points above white.
    fn inner(&self, index: usize) -> bool {
        (0 < index && index + 1 < self.points.len()) || (self.hdr && index > 0 && self.points[index][0] > 1.0)
    }

    /// The pointer's x as a fraction of the plotting area.
    fn fraction_x(&self, position: [f64; 2]) -> f64 {
        (position[0] - MARGIN) / (self.size.0 - 2.0 * MARGIN)
    }

    /// The pointer's height as a fraction of the plotting area, clipped to 0–1.
    fn fraction_y(&self, position: [f64; 2]) -> f64 {
        clip(1.0 - (position[1] - MARGIN) / (self.size.1 - 2.0 * MARGIN), 0.0, 1.0)
    }

    /// Port of `mousePressEvent` (widget coordinates; `ctrl`: the Control modifier, Command on macOS).
    pub fn press(&mut self, button: Button, position: [f64; 2], ctrl: bool) -> Vec<Signal> {
        let Some((index, distance)) = self.nearest(position) else { return Vec::new() };
        match button {
            Button::Right => {
                if self.inner(index) && distance < PICK {
                    self.points.remove(index);
                    return vec![Signal::Changed(self.points.clone()), Signal::Committed];
                }
                Vec::new()
            }
            Button::Left => {
                if distance < PICK {
                    self.active = Some(index);
                    Vec::new()
                } else if ctrl {
                    let x = self.axis_value(clip(self.fraction_x(position), 0.001, 0.999));
                    let y = self.axis_value(self.fraction_y(position));
                    self.points.push([x, y]);
                    // Python sorts the [x, y] lists lexicographically (stable).
                    self.points.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    self.active = self.points.iter().position(|p| *p == [x, y]);
                    vec![Signal::Changed(self.points.clone())]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Port of `mouseMoveEvent` (the widget gets moves only while a button is held).
    pub fn drag(&mut self, position: [f64; 2]) -> Vec<Signal> {
        let Some(active) = self.active else { return Vec::new() };
        if self.inner(active) {
            let upper =
                if active + 1 < self.points.len() { self.points[active + 1][0] - 0.001 } else { self.axis_value(1.0) };
            let lower = self.points[active - 1][0] + 0.001;
            self.points[active][0] = clip(self.axis_value(self.fraction_x(position)), lower, upper);
        }
        self.points[active][1] = self.axis_value(self.fraction_y(position));
        vec![Signal::Changed(self.points.clone())]
    }

    /// Port of `mouseReleaseEvent`.
    pub fn release(&mut self) -> Vec<Signal> {
        if self.active.take().is_some() {
            vec![Signal::Committed]
        } else {
            Vec::new()
        }
    }

    /// Port of `mouseDoubleClickEvent`: back to the identity curve.
    pub fn double_click(&mut self) -> Vec<Signal> {
        self.set_points(&DEFAULT_POINTS);
        vec![Signal::Changed(self.points.clone()), Signal::Committed]
    }

    /// The curve `paintEvent` draws: 100 inputs spread over the axis and `curve_extended` of them (float32, as the
    /// engine evaluates the curve).
    pub fn drawn_curve(&self) -> Vec<[f64; 2]> {
        let xs: Vec<f32> = (0..100)
            .map(|i| {
                let v = if i == 99 { 1.0 } else { f64::from(i) * (1.0 / 99.0) };
                self.axis_value(v) as f32
            })
            .collect();
        let points: grainy_tone::hdr::Points = self.points.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
        if points.len() < 2 {
            return xs.iter().map(|x| [f64::from(*x), f64::from(*x)]).collect();
        }
        let image = Array3::from_shape_fn((1, xs.len(), 3), |(_, i, _)| xs[i]);
        let ys = grainy_tone::hdr::curve_extended(&image, &points);
        xs.iter().enumerate().map(|(i, x)| [f64::from(*x), f64::from(ys[[0, i, 0]])]).collect()
    }
}

/// `np.clip` of one value: `minimum(maximum(v, low), high)`.
fn clip(v: f64, low: f64, high: f64) -> f64 {
    v.max(low).min(high)
}

/// The points of a settings value (`[[x, y], …]`); the identity curve when it is not a point list.
pub fn points_of(value: &Value) -> Vec<[f64; 2]> {
    let Some(list) = value.as_array() else { return DEFAULT_POINTS.to_vec() };
    let points: Vec<[f64; 2]> = list
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])
        })
        .collect();
    if points.is_empty() {
        DEFAULT_POINTS.to_vec()
    } else {
        points
    }
}

/// The settings value of a point list.
pub fn points_value(points: &[[f64; 2]]) -> Value {
    Value::from(points.iter().map(|p| Value::from(vec![p[0], p[1]])).collect::<Vec<_>>())
}

/// One curve editor on screen: the model and whether it holds the pointer (Qt's implicit grab).
#[derive(Debug, Clone, Default)]
pub struct CurveEditor {
    pub model: ToneCurve,
    grabbed: bool,
    /// Where it was drawn at the last frame (tests aim the pointer with it).
    pub rect: Option<Rect>,
}

/// Draws the editor at the full width of `ui` showing `points` (unless a point is being dragged), feeds it this
/// frame's pointer events and returns the signals in the order the widget emitted them.
pub fn show(ui: &mut Ui, editor: &mut CurveEditor, points: &[[f64; 2]], hdr: bool) -> Vec<Signal> {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click_and_drag());
    let response = response.on_hover_text(tr("점을 위아래로 드래그해 톤을 조정합니다. 더블클릭: 초기화"));
    editor.rect = Some(rect);
    let model = &mut editor.model;
    model.size = (f64::from(rect.width()), f64::from(rect.height()));
    model.set_hdr(hdr);
    if model.active.is_none() {
        model.set_points(points);
    }
    let local = |pos: Pos2| [f64::from(pos.x - rect.left()), f64::from(pos.y - rect.top())];
    let mut signals = Vec::new();
    let hovered = response.contains_pointer();
    let events = ui.input(|i| i.events.clone());
    for event in events {
        match event {
            Event::PointerButton { pos, button, pressed: true, modifiers } => {
                if !(hovered && rect.contains(pos)) {
                    continue;
                }
                editor.grabbed = true;
                let button = match button {
                    PointerButton::Primary => Button::Left,
                    PointerButton::Secondary => Button::Right,
                    _ => continue,
                };
                signals.extend(model.press(button, local(pos), modifiers.command));
            }
            Event::PointerMoved(pos) if editor.grabbed => signals.extend(model.drag(local(pos))),
            Event::PointerButton { pressed: false, .. } if editor.grabbed => {
                editor.grabbed = false;
                signals.extend(model.release());
            }
            _ => {}
        }
    }
    if response.double_clicked() {
        signals.extend(model.double_click());
    }
    paint(ui, rect, model);
    signals
}

/// Port of `paintEvent`.
fn paint(ui: &Ui, rect: Rect, model: &ToneCurve) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    let at = |p: [f64; 2]| {
        let [x, y] = model.point_xy(p);
        Pos2::new(rect.left() + x as f32, rect.top() + y as f32)
    };
    painter.rect_filled(rect, CornerRadius::ZERO, BACKGROUND);
    let maximum = model.axis_value(1.0);
    let grid = Stroke::new(1.0, GRID);
    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let value = model.axis_value(t);
        painter.line_segment([at([value, 0.0]), at([value, maximum])], grid);
        painter.line_segment([at([0.0, value]), at([maximum, value])], grid);
    }
    // Qt's DashLine: dashes of 4 and gaps of 2 pen widths.
    painter.extend(Shape::dashed_line(&[at([0.0, 0.0]), at([maximum, maximum])], Stroke::new(1.0, DIAGONAL), 4.0, 2.0));
    if model.hdr {
        let line = Stroke::new(1.0, SDR_LINE);
        painter.line_segment([at([1.0, 0.0]), at([1.0, maximum])], line);
        painter.line_segment([at([0.0, 1.0]), at([maximum, 1.0])], line);
        painter.text(
            Pos2::new(rect.left() + 15.0, rect.top() + 23.0),
            egui::Align2::LEFT_BOTTOM,
            "SDR / HDR +4 EV",
            crate::theme::font(crate::theme::FONT_SIZE),
            SDR_LINE,
        );
    }
    let margin = MARGIN as f32;
    let plot = Rect::from_min_max(rect.min + Vec2::splat(margin), rect.max - Vec2::splat(margin));
    let line: Vec<Pos2> = model.drawn_curve().into_iter().map(at).collect();
    ui.painter_at(plot).add(Shape::line(line, Stroke::new(2.0, CURVE)));
    for p in &model.points {
        if (0.0..=maximum).contains(&p[0]) && (0.0..=maximum).contains(&p[1]) {
            painter.circle(at(*p), 4.0, CURVE, Stroke::new(2.0, CURVE));
        }
    }
}
