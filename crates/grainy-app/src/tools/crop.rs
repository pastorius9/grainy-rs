//! The crop tool: the interactive frame on the photo and the window's crop functions.
//!
//! * [`CropFrame`]: port of the crop mode of luma/widgets.py `PhotoView` (`set_crop_mode`, `crop_values`,
//!   `canvas_rect`, `set_crop_limit`, `pointer_angle`, `dragged_angle`, `drag_crop`, `show_crop`, `limited_crop`,
//!   `crop_screen_rect`, `handle_points`, `hit_handle`, `normalized_point`, `normalized_aspect`, `set_aspect`,
//!   `reset_crop`, `accept_crop`, `resize_crop`, the mouse handlers and Enter/Esc, `draw_crop`). The frame works in
//!   the view's scene (logical pixels of the shown frame, [`Scene`]) as the oracle's `QGraphicsView` does: screen
//!   points become scene points by a scale and an offset, and scene points become whole screen points (`qRound`).
//! * The window's part (luma/app.py `toggle_crop`, `start_crop`, `apply_crop`, `end_crop`, `cancel_crop`,
//!   `crop_changed`, `sync_crop_limit`, `fit_crop`, `set_straighten`, `drag_straighten`, `set_aspect`, `reset_crop`,
//!   `rotate_photo`, the results of `auto_straighten` and `auto_border_crop`, `start_preview`'s `use_crop` and
//!   `preview_size`): functions on the [`CropSession`] kept in the context (`cx.state::<CropSession>()`).
//!
//! The frame's signals (`cropChanged`, `cropSelected`, `cropCancelled`, `straightenDragged`, `straightenFinished`)
//! are returned as [`Signal`]s and handled after the frame's method returns; in the oracle each `emit` is the last
//! thing its method does, so the order is the same.

use eframe::egui::{self, Color32, CornerRadius, CursorIcon, Key, Modifiers, Painter, Pos2, Rect, Stroke, StrokeKind};
use grainy_core::Settings;
use grainy_engine::Crop as UseCrop;
use grainy_geometry::straighten;
use serde_json::Value;

use super::{CanvasTool, PointerEvent, PointerKind};
use crate::context::{AppContext, Zoom};
use crate::imagespace::ViewTransform;
use crate::panels::Tool;
use crate::text::{tr, tr_with};

/// The aspect choices of the Crop panel (`self.aspect`): Korean label and width / height (0: free).
pub const ASPECTS: [(&str, f64); 9] = [
    ("자유 비율", 0.0),
    ("1 : 1", 1.0),
    ("3 : 2", 1.5),
    ("4 : 3", 4.0 / 3.0),
    ("16 : 9", 16.0 / 9.0),
    ("2 : 3", 2.0 / 3.0),
    ("4 : 5", 0.8),
    ("9 : 16", 9.0 / 16.0),
    ("65 : 24 · 파노라마", 65.0 / 24.0),
];

/// `resize_crop`'s minimum size (fractions; independent of the preview resolution).
const MINIMUM: f64 = 0.015;

// ---- Qt's rectangles and rounding --------------------------------------------------------------------------

/// Qt's `qRound`.
pub fn qround(d: f64) -> f64 {
    if d >= 0.0 {
        (d + 0.5).trunc()
    } else {
        let below = (d - 1.0).trunc();
        (d - below + 0.5).trunc() + below
    }
}

/// Python's `round(x, 1)` (correctly rounded, ties to even on the exact value).
pub fn py_round1(x: f64) -> f64 {
    format!("{x:.1}").parse().unwrap_or(x)
}

fn py_min(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// numpy's `clip(v, low, high)`.
fn clip(v: f64, low: f64, high: f64) -> f64 {
    v.max(low).min(high)
}

/// Qt's `QRectF`: a corner and a size (the size may be negative until `normalized`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectF {
    /// The whole canvas `QRectF(0, 0, 1, 1)`.
    pub const FULL: RectF = RectF { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    /// `QRectF(x0, y0, x1 - x0, y1 - y0)`.
    pub fn from_values(v: [f64; 4]) -> Self {
        Self { x: v[0], y: v[1], w: v[2] - v[0], h: v[3] - v[1] }
    }

    /// `QRectF(QPointF a, QPointF b)`.
    pub fn from_points(a: [f64; 2], b: [f64; 2]) -> Self {
        Self { x: a[0], y: a[1], w: b[0] - a[0], h: b[1] - a[1] }
    }

    pub fn left(&self) -> f64 {
        self.x
    }

    pub fn top(&self) -> f64 {
        self.y
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn center(&self) -> [f64; 2] {
        [self.x + self.w / 2.0, self.y + self.h / 2.0]
    }

    /// `[left, top, right, bottom]` (`crop_values`).
    pub fn values(&self) -> [f64; 4] {
        [self.left(), self.top(), self.right(), self.bottom()]
    }

    /// `QRectF.normalized`.
    pub fn normalized(mut self) -> Self {
        if self.w < 0.0 {
            self.x += self.w;
            self.w = -self.w;
        }
        if self.h < 0.0 {
            self.y += self.h;
            self.h = -self.h;
        }
        self
    }

    /// `QRectF.contains(QPointF)`: edges included, an empty rectangle contains nothing.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let (mut l, mut r) = (self.x, self.x);
        if self.w < 0.0 {
            l += self.w;
        } else {
            r += self.w;
        }
        if l == r || p[0] < l || p[0] > r {
            return false;
        }
        let (mut t, mut b) = (self.y, self.y);
        if self.h < 0.0 {
            t += self.h;
        } else {
            b += self.h;
        }
        !(t == b || p[1] < t || p[1] > b)
    }
}

/// Where the shown frame is: its scene (logical pixels: `image_rect`, and `canvas`, the photo's own canvas inside
/// a frame enlarged for a straightened photo) and the view's map to the screen (`screen = scene * scale + offset`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    pub scale: f64,
    pub offset: [f64; 2],
    /// The frame's (width, height) in logical pixels (`image_rect`).
    pub image: [f64; 2],
    /// `canvas`: (x, y, width, height) in the frame's logical pixels, or `None` (the whole frame).
    pub canvas: Option<[f64; 4]>,
}

impl Default for Scene {
    fn default() -> Self {
        Self { scale: 1.0, offset: [0.0, 0.0], image: [1.0, 1.0], canvas: None }
    }
}

impl Scene {
    /// `mapToScene(position.toPoint())`.
    pub fn to_scene(&self, screen: [f64; 2]) -> [f64; 2] {
        let p = [qround(screen[0]), qround(screen[1])];
        [(p[0] - self.offset[0]) / self.scale, (p[1] - self.offset[1]) / self.scale]
    }

    /// `mapFromScene`: whole screen points.
    pub fn from_scene(&self, p: [f64; 2]) -> [f64; 2] {
        [qround(p[0] * self.scale + self.offset[0]), qround(p[1] * self.scale + self.offset[1])]
    }

    /// Port of `canvas_rect`.
    pub fn canvas_rect(&self) -> RectF {
        match self.canvas {
            Some(c) => RectF { x: c[0], y: c[1], w: c[2], h: c[3] },
            None => RectF { x: 0.0, y: 0.0, w: self.image[0], h: self.image[1] },
        }
    }
}

/// What the pointer grabbed (`drag_handle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
    Move,
    Rotate,
    New,
}

impl Handle {
    /// The oracle's name of the handle.
    pub fn name(self) -> &'static str {
        match self {
            Handle::Nw => "nw",
            Handle::N => "n",
            Handle::Ne => "ne",
            Handle::E => "e",
            Handle::Se => "se",
            Handle::S => "s",
            Handle::Sw => "sw",
            Handle::W => "w",
            Handle::Move => "move",
            Handle::Rotate => "rotate",
            Handle::New => "new",
        }
    }

    fn has(self, letter: char) -> bool {
        self.name().contains(letter)
    }

    fn cursor(self) -> CursorIcon {
        match self {
            Handle::Nw | Handle::Se => CursorIcon::ResizeNwSe,
            Handle::Ne | Handle::Sw => CursorIcon::ResizeNeSw,
            Handle::N | Handle::S => CursorIcon::ResizeVertical,
            Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
            Handle::Move => CursorIcon::Move,
            Handle::Rotate => CursorIcon::Grab,
            Handle::New => CursorIcon::Crosshair,
        }
    }
}

/// The frame's signals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// `cropChanged`.
    Changed([f64; 4]),
    /// `cropSelected` (Enter, 적용, double-click).
    Selected([f64; 4]),
    /// `cropCancelled` (Esc).
    Cancelled,
    /// `straightenDragged`: the angle after dragging outside the frame.
    StraightenDragged(f64),
    /// `straightenFinished`.
    StraightenFinished,
}

/// Port of the crop mode of `PhotoView`.
#[derive(Debug, Clone)]
pub struct CropFrame {
    /// `crop_mode`.
    pub open: bool,
    /// `crop_rect` (fractions of the canvas).
    pub rect: RectF,
    /// `crop_limit`: straighten angle and canvas size in pixels.
    pub limit: (f64, f64, f64),
    /// `straighten_angle`: the photo's angle, where a turn by dragging starts.
    pub straighten_angle: f64,
    /// `aspect` (width / height in pixels, 0: free).
    pub aspect: f64,
    /// `crop_aspect_pending`: the aspect is applied when the next frame arrives.
    pub aspect_pending: bool,
    /// `has_photo`.
    pub has_photo: bool,
    /// `drag_handle`.
    pub handle: Option<Handle>,
    drag_start: [f64; 2],
    drag_rect: RectF,
    rotate_center: [f64; 2],
    rotate_from: f64,
    rotate_start: f64,
    rotate_moved: bool,
    /// The view's scene and screen map.
    pub scene: Scene,
}

impl Default for CropFrame {
    fn default() -> Self {
        Self {
            open: false,
            rect: RectF::FULL,
            limit: (0.0, 1.0, 1.0),
            straighten_angle: 0.0,
            aspect: 0.0,
            aspect_pending: false,
            has_photo: false,
            handle: None,
            drag_start: [0.0, 0.0],
            drag_rect: RectF::FULL,
            rotate_center: [0.0, 0.0],
            rotate_from: 0.0,
            rotate_start: 0.0,
            rotate_moved: false,
            scene: Scene::default(),
        }
    }
}

impl CropFrame {
    /// Port of `set_crop_mode`.
    pub fn set_crop_mode(&mut self, enabled: bool, crop: Option<[f64; 4]>) {
        self.open = enabled;
        self.handle = None;
        if enabled {
            self.rect = RectF::from_values(crop.unwrap_or(straighten::FULL));
            self.aspect_pending = true;
        }
    }

    /// Port of `crop_values`.
    pub fn crop_values(&self) -> [f64; 4] {
        self.rect.values()
    }

    /// Port of `set_crop_limit`.
    pub fn set_crop_limit(&mut self, angle: f64, width: f64, height: f64, current: Option<f64>) {
        self.limit = (angle, width, height);
        self.straighten_angle = current.unwrap_or(angle);
    }

    /// Port of `pointer_angle`: the pointer's direction from `center` in degrees, clockwise on screen.
    pub fn pointer_angle(center: [f64; 2], position: [f64; 2]) -> f64 {
        (position[1] - center[1]).atan2(position[0] - center[0]).to_degrees()
    }

    /// Port of `dragged_angle`.
    pub fn dragged_angle(&self, position: [f64; 2]) -> f64 {
        let turn =
            (Self::pointer_angle(self.rotate_center, position) - self.rotate_from + 180.0).rem_euclid(360.0) - 180.0;
        py_round1(clip(self.rotate_start + turn, -45.0, 45.0))
    }

    /// Port of `drag_crop`.
    pub fn drag_crop(&self, point: [f64; 2]) -> RectF {
        let wanted = self.resize_crop(point);
        let previous = if self.handle == Some(Handle::New) {
            let a = self.drag_start;
            let tiny = |v: f64, origin: f64| origin + (v - origin) * 1e-4;
            let mut xs = [tiny(wanted.left(), a[0]), tiny(wanted.right(), a[0])];
            let mut ys = [tiny(wanted.top(), a[1]), tiny(wanted.bottom(), a[1])];
            xs.sort_by(f64::total_cmp);
            ys.sort_by(f64::total_cmp);
            [xs[0], ys[0], xs[1], ys[1]]
        } else {
            self.crop_values()
        };
        self.limited_crop(wanted, Some(previous))
    }

    /// Port of `show_crop`.
    pub fn show_crop(&mut self, crop: Option<[f64; 4]>) -> Vec<Signal> {
        self.rect = RectF::from_values(crop.unwrap_or(straighten::FULL));
        vec![Signal::Changed(self.crop_values())]
    }

    /// Port of `limited_crop`.
    pub fn limited_crop(&self, rect: RectF, previous: Option<[f64; 4]>) -> RectF {
        let (angle, width, height) = self.limit;
        if angle == 0.0 {
            return rect;
        }
        let candidate = rect.values();
        let v = match previous {
            None => straighten::shrink(candidate, angle, width, height),
            Some(previous) => straighten::limit(candidate, previous, angle, width, height),
        };
        RectF::from_values(v)
    }

    /// Port of `crop_screen_rect`.
    pub fn crop_screen_rect(&self) -> RectF {
        let (r, b) = (self.rect, self.scene.canvas_rect());
        let top = self.scene.from_scene([b.left() + r.left() * b.w, b.top() + r.top() * b.h]);
        let bottom = self.scene.from_scene([b.left() + r.right() * b.w, b.top() + r.bottom() * b.h]);
        RectF::from_points(top, bottom).normalized()
    }

    /// Port of `handle_points`.
    pub fn handle_points(rect: &RectF) -> [(Handle, [f64; 2]); 8] {
        let c = rect.center();
        [
            (Handle::Nw, [rect.left(), rect.top()]),
            (Handle::N, [c[0], rect.top()]),
            (Handle::Ne, [rect.right(), rect.top()]),
            (Handle::E, [rect.right(), c[1]]),
            (Handle::Se, [rect.right(), rect.bottom()]),
            (Handle::S, [c[0], rect.bottom()]),
            (Handle::Sw, [rect.left(), rect.bottom()]),
            (Handle::W, [rect.left(), c[1]]),
        ]
    }

    /// Port of `hit_handle`.
    pub fn hit_handle(&self, position: [f64; 2]) -> Handle {
        let rect = self.crop_screen_rect();
        for (name, point) in Self::handle_points(&rect) {
            if (position[0] - point[0]).abs() + (position[1] - point[1]).abs() <= 15.0 {
                return name;
            }
        }
        let (x, y) = (position[0], position[1]);
        if rect.top() - 6.0 <= y && y <= rect.bottom() + 6.0 {
            if (x - rect.left()).abs() <= 6.0 {
                return Handle::W;
            }
            if (x - rect.right()).abs() <= 6.0 {
                return Handle::E;
            }
        }
        if rect.left() - 6.0 <= x && x <= rect.right() + 6.0 {
            if (y - rect.top()).abs() <= 6.0 {
                return Handle::N;
            }
            if (y - rect.bottom()).abs() <= 6.0 {
                return Handle::S;
            }
        }
        if rect.contains(position) {
            Handle::Move
        } else {
            Handle::Rotate
        }
    }

    /// Port of `normalized_point`: a scene point as a fraction of the canvas, clamped.
    pub fn normalized_point(&self, point: [f64; 2]) -> [f64; 2] {
        let b = self.scene.canvas_rect();
        [clip((point[0] - b.left()) / b.w, 0.0, 1.0), clip((point[1] - b.top()) / b.h, 0.0, 1.0)]
    }

    /// Port of `normalized_aspect`.
    pub fn normalized_aspect(&self) -> f64 {
        let b = self.scene.canvas_rect();
        if self.aspect != 0.0 && self.has_photo {
            self.aspect * b.h / b.w
        } else {
            0.0
        }
    }

    /// Port of `set_aspect`.
    pub fn set_aspect(&mut self, aspect: f64) -> Vec<Signal> {
        self.aspect = aspect;
        if !self.open || !self.has_photo || aspect == 0.0 {
            return Vec::new();
        }
        let ratio = self.normalized_aspect();
        let c = self.rect.center();
        let width = py_min(self.rect.w, self.rect.h * ratio);
        let height = width / ratio;
        self.rect =
            self.limited_crop(RectF { x: c[0] - width / 2.0, y: c[1] - height / 2.0, w: width, h: height }, None);
        vec![Signal::Changed(self.crop_values())]
    }

    /// Port of `reset_crop` (the frame's own).
    pub fn reset_crop(&mut self) -> Vec<Signal> {
        self.rect = RectF::FULL;
        let mut signals = self.set_aspect(self.aspect);
        self.rect = self.limited_crop(self.rect, None);
        signals.push(Signal::Changed(self.crop_values()));
        signals
    }

    /// Port of `accept_crop`.
    pub fn accept_crop(&self) -> Vec<Signal> {
        if self.open {
            vec![Signal::Selected(self.crop_values())]
        } else {
            Vec::new()
        }
    }

    /// Port of `resize_crop`.
    pub fn resize_crop(&self, point: [f64; 2]) -> RectF {
        let r = self.drag_rect;
        let Some(h) = self.handle else { return r };
        let ratio = self.normalized_aspect();
        let (px, py) = (point[0], point[1]);
        if h == Handle::Move {
            let (dx, dy) = (px - self.drag_start[0], py - self.drag_start[1]);
            let x = clip(r.left() + dx, 0.0, 1.0 - r.w);
            let y = clip(r.top() + dy, 0.0, 1.0 - r.h);
            return RectF { x, y, w: r.w, h: r.h };
        }
        if h == Handle::New {
            let anchor = self.drag_start;
            let sx = if px >= anchor[0] { 1.0 } else { -1.0 };
            let sy = if py >= anchor[1] { 1.0 } else { -1.0 };
            let (mut width, mut height) = ((px - anchor[0]).abs(), (py - anchor[1]).abs());
            if ratio != 0.0 {
                width = py_max(width, height * ratio);
                let room_x = if sx > 0.0 { 1.0 - anchor[0] } else { anchor[0] };
                let room_y = if sy > 0.0 { 1.0 - anchor[1] } else { anchor[1] };
                width = py_min(py_min(width, room_x), room_y * ratio);
                height = width / ratio;
            }
            let end = [anchor[0] + sx * width, anchor[1] + sy * height];
            return RectF::from_points(anchor, end).normalized();
        }
        if ratio == 0.0 {
            let (mut left, mut right, mut top, mut bottom) = (r.left(), r.right(), r.top(), r.bottom());
            if h.has('w') {
                left = py_min(px, right - MINIMUM);
            }
            if h.has('e') {
                right = py_max(px, left + MINIMUM);
            }
            if h.has('n') {
                top = py_min(py, bottom - MINIMUM);
            }
            if h.has('s') {
                bottom = py_max(py, top + MINIMUM);
            }
            return RectF::from_points([left, top], [right, bottom]);
        }
        if h.name().len() == 2 {
            let anchor = [if h.has('w') { r.right() } else { r.left() }, if h.has('n') { r.bottom() } else { r.top() }];
            let sx = if h.has('w') { -1.0 } else { 1.0 };
            let sy = if h.has('n') { -1.0 } else { 1.0 };
            let mut width = py_max(py_max(MINIMUM, sx * (px - anchor[0])), sy * (py - anchor[1]) * ratio);
            let room_x = if sx > 0.0 { 1.0 - anchor[0] } else { anchor[0] };
            let room_y = if sy > 0.0 { 1.0 - anchor[1] } else { anchor[1] };
            width = py_min(py_min(width, room_x), room_y * ratio);
            return RectF::from_points(anchor, [anchor[0] + sx * width, anchor[1] + sy * width / ratio]).normalized();
        }
        let c = r.center();
        if h == Handle::W || h == Handle::E {
            let anchor = if h == Handle::W { r.right() } else { r.left() };
            let sign = if h == Handle::W { -1.0 } else { 1.0 };
            let mut width = py_max(MINIMUM, sign * (px - anchor));
            let room = if sign < 0.0 { anchor } else { 1.0 - anchor };
            width = py_min(py_min(width, room), 2.0 * py_min(c[1], 1.0 - c[1]) * ratio);
            return RectF {
                x: py_min(anchor, anchor + sign * width),
                y: c[1] - width / ratio / 2.0,
                w: width,
                h: width / ratio,
            };
        }
        let anchor = if h == Handle::N { r.bottom() } else { r.top() };
        let sign = if h == Handle::N { -1.0 } else { 1.0 };
        let mut height = py_max(MINIMUM, sign * (py - anchor));
        let room = if sign < 0.0 { anchor } else { 1.0 - anchor };
        height = py_min(py_min(height, room), 2.0 * py_min(c[0], 1.0 - c[0]) / ratio);
        RectF {
            x: c[0] - height * ratio / 2.0,
            y: py_min(anchor, anchor + sign * height),
            w: height * ratio,
            h: height,
        }
    }

    /// The crop branch of `mousePressEvent` (`position`: screen point).
    pub fn press(&mut self, position: [f64; 2]) {
        if !self.has_photo || !self.open {
            return;
        }
        let handle = self.hit_handle(position);
        self.handle = Some(handle);
        self.drag_start = self.normalized_point(self.scene.to_scene(position));
        self.drag_rect = self.rect;
        if handle == Handle::Rotate {
            // The centre stays where it was pressed: the view refits while the photo turns.
            self.rotate_center = self.crop_screen_rect().center();
            self.rotate_from = Self::pointer_angle(self.rotate_center, position);
            self.rotate_start = self.straighten_angle;
            self.rotate_moved = false;
        }
    }

    /// The crop branch of `mouseMoveEvent` with the button down (without it: [`CropFrame::hit_handle`]).
    pub fn drag(&mut self, position: [f64; 2]) -> Vec<Signal> {
        if !self.has_photo || !self.open {
            return Vec::new();
        }
        match self.handle {
            Some(Handle::Rotate) => {
                let angle = self.dragged_angle(position);
                if angle != self.straighten_angle {
                    self.rotate_moved = true;
                    self.straighten_angle = angle;
                    return vec![Signal::StraightenDragged(angle)];
                }
                Vec::new()
            }
            Some(_) => {
                let point = self.normalized_point(self.scene.to_scene(position));
                self.rect = self.drag_crop(point);
                vec![Signal::Changed(self.crop_values())]
            }
            None => Vec::new(),
        }
    }

    /// The crop branch of `mouseReleaseEvent`.
    pub fn release(&mut self, position: [f64; 2]) -> Vec<Signal> {
        match self.handle {
            Some(Handle::Rotate) => {
                self.handle = None;
                if self.rotate_moved {
                    vec![Signal::StraightenFinished]
                } else {
                    Vec::new()
                }
            }
            Some(_) => {
                // Releasing the pointer keeps the crop editable until Enter/Apply.
                let point = self.normalized_point(self.scene.to_scene(position));
                self.rect = self.drag_crop(point);
                if self.rect.w < MINIMUM || self.rect.h < MINIMUM {
                    self.rect = self.drag_rect;
                }
                self.handle = None;
                self.rect = self.limited_crop(self.rect, None);
                vec![Signal::Changed(self.crop_values())]
            }
            None => Vec::new(),
        }
    }

    /// The crop branch of `mouseDoubleClickEvent`.
    pub fn double_click(&mut self) -> Vec<Signal> {
        if !self.open {
            return Vec::new();
        }
        self.handle = None;
        self.accept_crop()
    }

    /// The crop branch of `keyPressEvent`: Enter applies, Esc cancels.
    pub fn key(&self, key: Key) -> Option<Vec<Signal>> {
        if !self.open {
            return None;
        }
        match key {
            Key::Enter => Some(self.accept_crop()),
            Key::Escape => Some(vec![Signal::Cancelled]),
            _ => None,
        }
    }
}

// ---- the window's part --------------------------------------------------------------------------------------

/// The crop tool's state between frames (the window's crop attributes and the frame).
#[derive(Debug, Clone, Default)]
pub struct CropSession {
    /// The frame on the photo.
    pub frame: CropFrame,
    /// The aspect box's index in [`ASPECTS`].
    pub aspect_index: usize,
    /// `crop_size`: the crop's size in pixels.
    pub size_text: String,
    /// `auto_level_note` when it is not the default text.
    pub level_note: Option<String>,
    /// `crop_entry`: crop, straighten and upright when the tool opened (Cancel returns to them).
    entry: Option<[Value; 3]>,
    /// The photo the open frame belongs to.
    photo: u64,
    /// Whether the strip had the crop tool selected at the last look.
    was_crop: bool,
    /// The settings' crop at the last look (a change from elsewhere, an undo, is shown).
    known_crop: Value,
    /// The last frame seen (`set_image`).
    last_frame: u64,
}

/// Runs `f` with the crop session taken out of the context (put back afterwards).
pub fn with_session<R>(cx: &mut AppContext, f: impl FnOnce(&mut CropSession, &mut AppContext) -> R) -> R {
    // The shown frame's scene (`image_rect`, `canvas`), read while the session is still in the context.
    let scene = scene_for(cx, Rect::from_min_size(Pos2::ZERO, egui::vec2(1.0, 1.0)));
    let mut s = std::mem::take(cx.state::<CropSession>());
    s.frame.has_photo = cx.doc.current().is_some();
    s.frame.scene.image = scene.image;
    s.frame.scene.canvas = scene.canvas;
    let result = f(&mut s, cx);
    s.known_crop = cx.settings().value("crop").clone();
    *cx.state::<CropSession>() = s;
    result
}

/// The session as it is (read-only).
pub fn session(cx: &AppContext) -> Option<&CropSession> {
    cx.state_ref::<CropSession>()
}

/// Whether the crop frame is open (`crop_mode`).
pub fn is_open(cx: &AppContext) -> bool {
    session(cx).is_some_and(|s| s.frame.open)
}

/// Python truthiness of a settings value (`settings.get('upright')`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A settings crop as four numbers (`None`: no crop).
pub fn crop_of(settings: &Settings) -> Option<[f64; 4]> {
    let list = settings.value("crop").as_array()?;
    if list.len() != 4 {
        return None;
    }
    let mut v = [0.0; 4];
    for (slot, item) in v.iter_mut().zip(list) {
        *slot = item.as_f64()?;
    }
    Some(v)
}

fn crop_value(crop: Option<[f64; 4]>) -> Value {
    crop.map_or(Value::Null, |c| Value::from(c.to_vec()))
}

fn has_source(cx: &AppContext) -> bool {
    cx.doc.current().is_some()
}

/// Port of `canvas_size`: the oriented size of the uncropped photo in pixels.
pub fn canvas_size(cx: &AppContext) -> Option<(usize, usize)> {
    let photo = cx.doc.current()?;
    let (w, h) = photo.original.or_else(|| cx.frame().map(|f| f.original))?;
    let (w, h) = (w as usize, h as usize);
    Some(if cx.settings().i64("rotation").rem_euclid(2) == 1 { (h, w) } else { (w, h) })
}

/// `set_setting`: a value, live, with its panel switched on.
fn set_setting(cx: &mut AppContext, key: &str, value: impl Into<Value>) {
    cx.doc.edit_mut().set(key, value);
    cx.before = false;
}

/// `self.settings[key] = value`: a value without switching panels.
fn assign(cx: &mut AppContext, key: &str, value: Value) {
    let mut next = cx.settings().clone();
    next.set(key, value);
    cx.doc.edit_mut().set_all(next);
}

fn commit(cx: &mut AppContext, label: &str) {
    cx.doc.edit_mut().commit(label);
}

fn ratio(s: &CropSession) -> Option<f64> {
    let r = ASPECTS[s.aspect_index.min(ASPECTS.len() - 1)].1;
    (r != 0.0).then_some(r)
}

/// Handles the frame's signals as the window's connections do.
fn handle(s: &mut CropSession, cx: &mut AppContext, signals: Vec<Signal>) {
    for signal in signals {
        match signal {
            Signal::Changed(crop) => crop_changed(s, cx, crop),
            Signal::Selected(crop) => apply_crop(s, cx, crop),
            Signal::Cancelled => cancel_crop(s, cx),
            Signal::StraightenDragged(angle) => set_straighten(s, cx, angle),
            Signal::StraightenFinished => commit(cx, "보정 변경"),
        }
    }
}

/// Port of `crop_changed`: the size label.
fn crop_changed(s: &mut CropSession, cx: &mut AppContext, crop: [f64; 4]) {
    let Some((w, h)) = canvas_size(cx) else { return };
    let pixels = |v: f64| grouped(v.round_ties_even() as i64);
    s.size_text = format!("{} × {} px", pixels((crop[2] - crop[0]) * w as f64), pixels((crop[3] - crop[1]) * h as f64));
}

/// Python's `f'{n:,}'`.
fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Port of `sync_crop_limit`: how far the photo is turned (perspective alignment has its own framing).
fn sync_crop_limit(s: &mut CropSession, cx: &AppContext) {
    let Some((w, h)) = canvas_size(cx) else { return };
    let settings = cx.settings();
    let straighten = settings.f64("straighten");
    let angle = if truthy(settings.value("upright")) { 0.0 } else { straighten };
    s.frame.set_crop_limit(angle, w as f64, h as f64, Some(straighten));
}

/// Port of `toggle_crop`.
fn toggle_crop(s: &mut CropSession, cx: &mut AppContext, checked: bool) {
    if !has_source(cx) {
        return;
    }
    if !checked {
        let crop = s.frame.crop_values();
        apply_crop(s, cx, crop);
        return;
    }
    commit(cx, "보정 변경");
    cx.open_tool(Tool::Crop);
    cx.before = false;
    cx.wb_picker = false;
    let crop = crop_of(cx.settings());
    s.frame.set_crop_mode(true, crop);
    let settings = cx.settings();
    s.entry =
        Some([settings.value("crop").clone(), settings.value("straighten").clone(), settings.value("upright").clone()]);
    sync_crop_limit(s, cx);
    cx.set_zoom(Zoom::Fit);
    let values = s.frame.crop_values();
    crop_changed(s, cx, values);
    s.photo = cx.doc.current_id();
    s.was_crop = true;
}

/// Port of `start_crop`.
fn start_crop(s: &mut CropSession, cx: &mut AppContext) {
    if has_source(cx) && !s.frame.open {
        toggle_crop(s, cx, true);
    }
}

/// Port of `end_crop`.
fn end_crop(s: &mut CropSession) {
    s.frame.set_crop_mode(false, None);
}

/// Port of `apply_crop`.
fn apply_crop(s: &mut CropSession, cx: &mut AppContext, crop: [f64; 4]) {
    set_setting(cx, "crop", Value::from(crop.to_vec()));
    end_crop(s);
    commit(cx, "보정 변경");
}

/// Port of `cancel_crop`.
fn cancel_crop(s: &mut CropSession, cx: &mut AppContext) {
    if !s.frame.open {
        return;
    }
    let keys = ["crop", "straighten", "upright"];
    let changed = s.entry.as_ref().is_some_and(|entry| {
        keys.iter().zip(entry.iter()).any(|(k, v)| !crate::state::py_eq(cx.settings().value(k), v))
    });
    if let (true, Some(entry)) = (changed, s.entry.clone()) {
        let mut next = cx.settings().clone();
        for (key, value) in keys.iter().zip(entry) {
            next.set(key, value);
        }
        cx.doc.edit_mut().set_all(next);
        end_crop(s);
        commit(cx, "크롭 취소");
    } else {
        end_crop(s);
    }
}

/// Port of `fit_crop`: after the angle changed, the crop stays on the turned photo (`straighten.refit`).
fn fit_crop(s: &mut CropSession, cx: &mut AppContext, previous: f64) {
    if !has_source(cx) || truthy(cx.settings().value("upright")) {
        return;
    }
    let Some((w, h)) = canvas_size(cx) else { return };
    let current = if s.frame.open { Some(s.frame.crop_values()) } else { crop_of(cx.settings()) };
    let angle = cx.settings().f64("straighten");
    let crop = straighten::refit(current, previous, angle, w as f64, h as f64, ratio(s));
    assign(cx, "crop", crop_value(crop));
    sync_crop_limit(s, cx);
    if s.frame.open {
        let signals = s.frame.show_crop(crop);
        handle(s, cx, signals);
    }
}

/// Port of `set_straighten`: the angle slider opens the tool, and the crop frame follows.
fn set_straighten(s: &mut CropSession, cx: &mut AppContext, value: f64) {
    if !has_source(cx) {
        return;
    }
    start_crop(s, cx);
    let previous = cx.settings().f64("straighten");
    set_setting(cx, "straighten", value);
    fit_crop(s, cx, previous);
}

/// Port of `set_aspect` (the aspect box changed).
fn set_aspect(s: &mut CropSession, cx: &mut AppContext, index: usize) {
    if index == s.aspect_index || index >= ASPECTS.len() {
        return;
    }
    s.aspect_index = index;
    let signals = s.frame.set_aspect(ASPECTS[index].1);
    handle(s, cx, signals);
}

/// Port of `reset_crop` (크롭 초기화): crop and angle back, a level alignment removed.
fn reset_crop(s: &mut CropSession, cx: &mut AppContext) {
    if !has_source(cx) {
        return;
    }
    commit(cx, "보정 변경");
    end_crop(s);
    set_aspect(s, cx, 0);
    s.size_text.clear();
    set_setting(cx, "crop", Value::Null);
    set_setting(cx, "straighten", 0);
    let level = cx.settings().value("upright").get("mode").and_then(Value::as_str) == Some("level");
    if level {
        set_setting(cx, "upright", Value::Null);
    }
    commit(cx, "크롭·수평 초기화");
    cx.set_zoom(Zoom::Fit);
}

/// Port of `rotate_photo` (시계 방향으로 90° 회전): the crop is cleared, a straightened photo keeps its crop
/// on the turned photo.
fn rotate_photo(s: &mut CropSession, cx: &mut AppContext) {
    if !has_source(cx) {
        return;
    }
    set_setting(cx, "crop", Value::Null);
    let rotation = (cx.settings().i64("rotation") + 1).rem_euclid(4);
    set_setting(cx, "rotation", rotation);
    let straighten = cx.settings().f64("straighten");
    if straighten != 0.0 {
        fit_crop(s, cx, straighten);
    }
    cx.set_zoom(Zoom::Fit);
    commit(cx, "보정 변경");
}

/// The result of `auto_straighten` (`finished`): the photo turned by `angle` (the estimate's `-rotation`) on the
/// angle slider, the crop frame following.
fn auto_level(s: &mut CropSession, cx: &mut AppContext, angle: f64) {
    let angle = py_round1(clip(angle, -45.0, 45.0));
    commit(cx, "보정 변경");
    start_crop(s, cx);
    let previous = cx.settings().f64("straighten");
    set_setting(cx, "upright", Value::Null);
    set_setting(cx, "straighten", angle);
    fit_crop(s, cx, previous);
    commit(cx, "자동 수평 맞추기");
    s.level_note =
        Some(tr_with("자동 보정 {0}° · 아래 슬라이더로 미세 조절할 수 있습니다.", &[&format!("{angle:+.1}")]));
}

// ---- entry points (commands, the panel, the tests) ----------------------------------------------------------

/// The 크롭 편집 button: opens the frame, or applies it when it is open.
pub fn toggle(cx: &mut AppContext) {
    with_session(cx, |s, cx| {
        let open = s.frame.open;
        toggle_crop(s, cx, !open);
    });
}

/// 적용 (`view.accept_crop`).
pub fn accept(cx: &mut AppContext) {
    with_session(cx, |s, cx| {
        let signals = s.frame.accept_crop();
        handle(s, cx, signals);
    });
}

/// 취소 (`cancel_crop`).
pub fn cancel(cx: &mut AppContext) {
    with_session(cx, cancel_crop);
}

/// 크롭 초기화 (`reset_crop`).
pub fn reset(cx: &mut AppContext) {
    with_session(cx, reset_crop);
}

/// 시계 방향으로 90° 회전 (`rotate_photo`).
pub fn rotate(cx: &mut AppContext) {
    with_session(cx, rotate_photo);
}

/// 좌우 반전: `set_setting('flip', not flip)`, committed as the oracle's commit timer does.
pub fn flip(cx: &mut AppContext) {
    if !has_source(cx) {
        return;
    }
    let flip = cx.settings().bool("flip");
    set_setting(cx, "flip", !flip);
    commit(cx, "보정 변경");
}

/// The aspect box (`set_aspect`).
pub fn choose_aspect(cx: &mut AppContext, index: usize) {
    with_session(cx, |s, cx| set_aspect(s, cx, index));
}

/// The angle slider (`set_straighten`); the slider's release commits.
pub fn straighten_slider(cx: &mut AppContext, value: f64) {
    with_session(cx, |s, cx| set_straighten(s, cx, value));
}

/// The result of 자동 수평 맞추기 with the estimate's angle (`-rotation`).
pub fn auto_level_result(cx: &mut AppContext, angle: f64) {
    with_session(cx, |s, cx| auto_level(s, cx, angle));
}

/// The start of 스캔 테두리 자동 자르기: commits and closes the frame (the job runs on the committed settings).
pub fn border_begin(cx: &mut AppContext) {
    with_session(cx, |s, cx| {
        commit(cx, "보정 변경");
        end_crop(s);
    });
}

/// The result of 스캔 테두리 자동 자르기: the crop found, one undo step.
pub fn border_result(cx: &mut AppContext, crop: [f64; 4]) {
    set_setting(cx, "crop", Value::from(crop.to_vec()));
    commit(cx, "스캔 테두리 자동 자르기");
}

/// The note under 자동 수평 맞추기.
pub fn set_level_note(cx: &mut AppContext, note: Option<String>) {
    cx.state::<CropSession>().level_note = note;
}

/// `set_image`'s part: a new frame applies an aspect waiting since the frame opened.
pub fn frame_arrived(cx: &mut AppContext) {
    with_session(cx, |s, cx| {
        if s.frame.open && s.frame.aspect_pending {
            s.frame.aspect_pending = false;
            let aspect = s.frame.aspect;
            let signals = s.frame.set_aspect(aspect);
            handle(s, cx, signals);
        }
    });
}

/// Every frame: opens the frame when the strip selects the tool (`select_tool` → 크롭 편집), applies it when the
/// strip leaves the tool, closes it without applying when another photo opens (`activate`), shows a crop changed
/// elsewhere (`apply_restored`: undo, redo) and applies a waiting aspect when a new frame arrived.
pub fn sync(cx: &mut AppContext) {
    let frame = cx.frame().map(|f| f.id);
    let arrived = with_session(cx, |s, cx| {
        let tool_crop = cx.tool() == Tool::Crop;
        if s.frame.open && (!has_source(cx) || s.photo != cx.doc.current_id()) {
            end_crop(s);
            s.size_text.clear();
        }
        if s.frame.open && !tool_crop {
            let signals = s.frame.accept_crop();
            handle(s, cx, signals);
        } else if tool_crop && !s.was_crop && !s.frame.open {
            toggle_crop(s, cx, true);
        }
        s.was_crop = cx.tool() == Tool::Crop;
        if s.frame.open && !crate::state::py_eq(cx.settings().value("crop"), &s.known_crop) {
            let crop = crop_of(cx.settings());
            let signals = s.frame.show_crop(crop);
            handle(s, cx, signals);
        }
        match frame {
            Some(id) if id != s.last_frame => {
                s.last_frame = id;
                true
            }
            _ => false,
        }
    });
    if arrived {
        frame_arrived(cx);
    }
}

/// `start_preview`'s `use_crop` for the open photo: uncropped while the frame is open, on the enlarged canvas when
/// the photo is straightened (without a perspective alignment). Also runs [`sync`] (the preview request is made
/// every frame).
pub fn preview_crop(cx: &mut AppContext) -> UseCrop {
    sync(cx);
    use_crop(cx)
}

fn use_crop(cx: &AppContext) -> UseCrop {
    if !is_open(cx) {
        return UseCrop::Cropped;
    }
    let s = cx.settings();
    let straighten = if cx.before { 0.0 } else { s.f64("straighten") };
    if straighten != 0.0 && !truthy(s.value("upright")) {
        UseCrop::All
    } else {
        UseCrop::Uncropped
    }
}

/// The scene of the shown frame (`preview_size` and `canvas` of `start_preview`) on screen at `rect`.
pub fn scene_for(cx: &AppContext, rect: Rect) -> Scene {
    let Some((w, h)) = canvas_size(cx) else {
        return Scene { offset: [f64::from(rect.min.x), f64::from(rect.min.y)], ..Scene::default() };
    };
    let (image, canvas) = if use_crop(cx) == UseCrop::All {
        let (px, py) = straighten::padding(cx.settings().f64("straighten"), w, h);
        ([(w + 2 * px) as f64, (h + 2 * py) as f64], Some([px as f64, py as f64, w as f64, h as f64]))
    } else {
        ([w as f64, h as f64], None)
    };
    Scene {
        scale: f64::from(rect.width()) / image[0].max(1.0),
        offset: [f64::from(rect.min.x), f64::from(rect.min.y)],
        image,
        canvas,
    }
}

// ---- the canvas tool ----------------------------------------------------------------------------------------

/// The crop tool on the photo view.
#[derive(Debug, Default)]
pub struct CropTool;

impl CropTool {
    /// The pointer with the shown frame at `view` (what [`CanvasTool::pointer`] does with the photo's rectangle).
    pub fn pointer_with_view(&mut self, event: &PointerEvent, view: &ViewTransform, cx: &mut AppContext) -> bool {
        let position = [f64::from(event.screen.x), f64::from(event.screen.y)];
        let double = event.kind == PointerKind::Click
            && cx.egui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
        let scene = scene_for(cx, view.rect);
        with_session(cx, |s, cx| {
            if !s.frame.open || !s.frame.has_photo {
                return false;
            }
            s.frame.scene = scene;
            let signals = match event.kind {
                PointerKind::Press => {
                    s.frame.press(position);
                    if let Some(h) = s.frame.handle {
                        cx.egui.set_cursor_icon(h.cursor());
                    }
                    Vec::new()
                }
                PointerKind::Drag => s.frame.drag(position),
                PointerKind::Release => s.frame.release(position),
                PointerKind::Click if double => s.frame.double_click(),
                PointerKind::Click => {
                    s.frame.press(position);
                    s.frame.release(position)
                }
                PointerKind::Hover => {
                    if s.frame.handle.is_none() {
                        cx.egui.set_cursor_icon(s.frame.hit_handle(position).cursor());
                    }
                    Vec::new()
                }
            };
            handle(s, cx, signals);
            true
        })
    }

    /// A double-click on the photo (`mouseDoubleClickEvent`): applies the crop.
    pub fn double_click(&mut self, cx: &mut AppContext) {
        with_session(cx, |s, cx| {
            let signals = s.frame.double_click();
            handle(s, cx, signals);
        });
    }
}

impl CanvasTool for CropTool {
    fn name(&self) -> &'static str {
        "crop"
    }

    fn tool(&self) -> Option<Tool> {
        Some(Tool::Crop)
    }

    fn edits(&self) -> &'static [&'static str] {
        &["crop", "rotation", "straighten", "flip"]
    }

    fn pointer(&mut self, event: &PointerEvent, cx: &mut AppContext) -> bool {
        let Some(rect) = cx.photo_rect() else { return false };
        self.pointer_with_view(event, &ViewTransform { rect }, cx)
    }

    fn key(&mut self, key: Key, _modifiers: Modifiers, cx: &mut AppContext) -> bool {
        with_session(cx, |s, cx| match s.frame.key(key) {
            Some(signals) => {
                handle(s, cx, signals);
                true
            }
            None => false,
        })
    }

    fn paint(&self, painter: &Painter, view: &ViewTransform, cx: &AppContext) {
        let Some(s) = session(cx).filter(|s| s.frame.open) else { return };
        let mut frame = s.frame.clone();
        frame.scene = scene_for(cx, view.rect);
        draw_crop(painter, &frame);
    }
}

/// Port of `draw_crop`: the dimmed surroundings, thirds (ninths while turning), the frame, its handles and the hint.
fn draw_crop(painter: &Painter, frame: &CropFrame) {
    let r = frame.crop_screen_rect();
    let rect =
        Rect::from_min_max(Pos2::new(r.left() as f32, r.top() as f32), Pos2::new(r.right() as f32, r.bottom() as f32));
    let outer = painter.clip_rect();
    let shade = Color32::from_black_alpha(155);
    for part in [
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, rect.top())),
        Rect::from_min_max(Pos2::new(outer.min.x, rect.bottom()), outer.max),
        Rect::from_min_max(Pos2::new(outer.min.x, rect.top()), Pos2::new(rect.left(), rect.bottom())),
        Rect::from_min_max(Pos2::new(rect.right(), rect.top()), Pos2::new(outer.max.x, rect.bottom())),
    ] {
        if part.is_positive() {
            painter.rect_filled(part, CornerRadius::ZERO, shade);
        }
    }
    let grid = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 135));
    let parts: Vec<f32> = if frame.handle == Some(Handle::Rotate) {
        (1..9).map(|i| i as f32 / 9.0).collect()
    } else {
        vec![1.0 / 3.0, 2.0 / 3.0]
    };
    for part in parts {
        let (x, y) = (rect.left() + rect.width() * part, rect.top() + rect.height() * part);
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], grid);
    }
    let accent = Color32::from_rgb(0xf7, 0xdf, 0xbc);
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.5, accent), StrokeKind::Middle);
    for (_, p) in CropFrame::handle_points(&r) {
        let square = Rect::from_center_size(Pos2::new(p[0] as f32, p[1] as f32), egui::vec2(8.0, 8.0));
        painter.rect(
            square,
            CornerRadius::ZERO,
            accent,
            Stroke::new(1.0, Color32::from_rgb(0x20, 0x23, 0x27)),
            StrokeKind::Middle,
        );
    }
    let hint = tr("안쪽 이동 · 모서리 조절 · 바깥 끌어 회전   |   Enter 적용 · Esc 취소");
    let font = egui::FontId::proportional(12.0);
    let galley = painter.layout_no_wrap(hint, font, accent);
    let width = (galley.size().x + 28.0).max(420.0);
    let top = (rect.top() - 31.0).max(outer.top() + 8.0);
    let label = Rect::from_min_size(Pos2::new(rect.center().x - width / 2.0, top), egui::vec2(width, 24.0));
    painter.rect_filled(label, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(20, 23, 27, 225));
    painter.galley(label.center() - galley.size() / 2.0, galley, accent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qt_rounding_and_python_round() {
        assert_eq!(qround(2.5), 3.0);
        assert_eq!(qround(-2.5), -2.0);
        assert_eq!(qround(-2.6), -3.0);
        assert_eq!(py_round1(0.25), 0.2);
        assert_eq!(py_round1(2.34), 2.3);
        assert_eq!(grouped(1500), "1,500");
        assert_eq!(grouped(15), "15");
    }

    #[test]
    fn rect_contains_like_qt() {
        let r = RectF { x: 0.0, y: 0.0, w: 2.0, h: 1.0 };
        assert!(r.contains([2.0, 1.0]));
        assert!(!RectF { x: 0.0, y: 0.0, w: 0.0, h: 1.0 }.contains([0.0, 0.5]));
    }
}
