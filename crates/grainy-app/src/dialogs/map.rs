//! The GPS map (port of luma/extras.py `ExtraTools.map` and `MapView`): the countries of Natural Earth 1:110m
//! (`assets/world.geojson`, public domain, copied from the Python Grainy's `assets/`) drawn offline in plate
//! carree (x = longitude, y = -latitude, in degrees), one marker for each photo of the list that has a
//! latitude and a longitude, wheel to zoom, drag to pan, double-click on a marker to open the photo in the
//! Develop screen. Non-modal.
//!
//! The land is filled with its own ear-clipping triangulation ([`triangulate`]; egui fills only convex shapes):
//! the outer rings are filled first, the rings of holes (South Africa around Lesotho) are painted in the sea
//! colour over them, then every ring's outline (a pen of 0.2 degrees, as the oracle's, so it scales with the
//! zoom) and the markers.

use std::sync::{Arc, OnceLock};

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use grainy_catalog::Catalog;
use serde_json::Value;

use super::Dialog;
use crate::context::{AppContext, Mode};
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets;

/// `setBackgroundBrush(QColor('#121b26'))`.
pub const SEA: Color32 = Color32::from_rgb(0x12, 0x1b, 0x26);
/// The land's fill and outline.
pub const LAND: Color32 = Color32::from_rgb(0x28, 0x3b, 0x48);
pub const COAST: Color32 = Color32::from_rgb(0x53, 0x64, 0x76);
/// A marker's fill and pen.
pub const MARKER: Color32 = Color32::from_rgb(0xe5, 0xa8, 0x53);
pub const MARKER_PEN: Color32 = Color32::from_rgb(0xf7, 0xe3, 0xbd);
/// Radius of a marker in pixels (`addEllipse(-3, -3, 6, 6)`, which ignores the view's scale).
pub const MARKER_RADIUS: f32 = 3.0;
/// The dialog's size (`dialog.resize(1050, 650)`).
pub const SIZE: Vec2 = Vec2::new(1050.0, 650.0);

const WORLD: &str = include_str!("../../assets/world.geojson");

// ---- the land -------------------------------------------------------------------------------------------------

/// The map's land in scene coordinates `[longitude, -latitude]`.
pub struct World {
    /// Triangles (three points each) of the outer rings.
    pub land: Vec<[[f32; 2]; 3]>,
    /// Triangles of the rings of holes.
    pub holes: Vec<[[f32; 2]; 3]>,
    /// Every ring (closed by the drawing, the first point is not repeated).
    pub rings: Vec<Vec<[f32; 2]>>,
    /// The outlines: every ring as one closed path, or, where a ring turns back on itself (a cusp: a stroke would
    /// get a miter spike across the map), as open runs that end and start at the cusp. `true` = closed.
    pub strokes: Vec<(Vec<[f32; 2]>, bool)>,
    /// How many polygons there are (the oracle makes one path item for each).
    pub polygons: usize,
}

impl World {
    /// The bundled map, parsed once.
    pub fn load() -> Arc<World> {
        static CACHE: OnceLock<Arc<World>> = OnceLock::new();
        CACHE.get_or_init(|| Arc::new(World::parse(WORLD))).clone()
    }

    /// Port of the GeoJSON loop of `MapView.__init__`: the polygons of a `Polygon` or `MultiPolygon` feature, a
    /// ring's points as `(lon, -lat)`. A file that is not GeoJSON gives an empty map (the oracle shows none).
    pub fn parse(text: &str) -> World {
        let mut world =
            World { land: Vec::new(), holes: Vec::new(), rings: Vec::new(), strokes: Vec::new(), polygons: 0 };
        let Ok(value) = serde_json::from_str::<Value>(text) else { return world };
        let features = value["features"].as_array().cloned().unwrap_or_default();
        for feature in features {
            let geometry = &feature["geometry"];
            let polygons: Vec<&Value> = match geometry["type"].as_str() {
                Some("Polygon") => vec![&geometry["coordinates"]],
                Some("MultiPolygon") => {
                    geometry["coordinates"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
                }
                _ => continue,
            };
            for polygon in polygons {
                world.polygons += 1;
                for (n, ring) in polygon.as_array().into_iter().flatten().enumerate() {
                    let mut points: Vec<[f32; 2]> = ring
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| Some([p[0].as_f64()? as f32, -p[1].as_f64()? as f32]))
                        .collect();
                    if points.len() > 1 && points.first() == points.last() {
                        points.pop();
                    }
                    let target = if n == 0 { &mut world.land } else { &mut world.holes };
                    target.extend(triangulate(&points));
                    world.strokes.extend(split_at_cusps(&points));
                    world.rings.push(points);
                }
            }
        }
        world
    }
}

/// Splits a ring at its cusps (a turn of more than about 170 degrees): without a cusp the ring is one closed path.
pub fn split_at_cusps(ring: &[[f32; 2]]) -> Vec<(Vec<[f32; 2]>, bool)> {
    let n = ring.len();
    if n < 3 {
        return vec![(ring.to_vec(), false)];
    }
    let direction = |a: [f32; 2], b: [f32; 2]| {
        let d = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
        let length = d[0].hypot(d[1]);
        if length == 0.0 {
            [0.0, 0.0]
        } else {
            [d[0] / length, d[1] / length]
        }
    };
    let cusps: Vec<usize> = (0..n)
        .filter(|&i| {
            let (a, b) = (direction(ring[(i + n - 1) % n], ring[i]), direction(ring[i], ring[(i + 1) % n]));
            a[0] * b[0] + a[1] * b[1] < -0.985
        })
        .collect();
    if cusps.is_empty() {
        return vec![(ring.to_vec(), true)];
    }
    let mut runs = Vec::new();
    for (k, &start) in cusps.iter().enumerate() {
        let end = cusps[(k + 1) % cusps.len()];
        let length = (end + n - start) % n;
        let length = if length == 0 { n } else { length };
        runs.push(((0..=length).map(|j| ring[(start + j) % n]).collect(), false));
    }
    runs
}

/// Ear-clipping triangulation of a simple ring (any orientation, the first point not repeated). A ring that
/// degenerates (collinear or repeated points) loses the offending points instead of looping.
pub fn triangulate(ring: &[[f32; 2]]) -> Vec<[[f32; 2]; 3]> {
    let mut points: Vec<[f64; 2]> = Vec::with_capacity(ring.len());
    for p in ring {
        let p = [f64::from(p[0]), f64::from(p[1])];
        if points.last() != Some(&p) {
            points.push(p);
        }
    }
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f64 = (0..n).map(|i| cross2(points[i], points[(i + 1) % n])).sum();
    if area < 0.0 {
        points.reverse();
    }
    let side = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let mut order: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut at = 0;
    let mut misses = 0;
    while order.len() > 3 {
        let m = order.len();
        let (a, b, c) = (order[(at + m - 1) % m], order[at % m], order[(at + 1) % m]);
        let (pa, pb, pc) = (points[a], points[b], points[c]);
        let ear = side(pa, pb, pc) > 1e-12
            && !order.iter().any(|&j| {
                let p = points[j];
                j != a
                    && j != b
                    && j != c
                    && p != pa
                    && p != pb
                    && p != pc
                    && side(pa, pb, p) >= 0.0
                    && side(pb, pc, p) >= 0.0
                    && side(pc, pa, p) >= 0.0
            });
        if ear {
            out.push([pa, pb, pc]);
            order.remove(at % m);
            at = (at + m - 1) % (m - 1);
            misses = 0;
        } else {
            at = (at + 1) % m;
            misses += 1;
            if misses >= m {
                // No ear left (touching rings): drop a vertex and go on.
                order.remove(at % m);
                at %= order.len();
                misses = 0;
            }
        }
    }
    if order.len() == 3 {
        let (pa, pb, pc) = (points[order[0]], points[order[1]], points[order[2]]);
        if side(pa, pb, pc).abs() > 1e-12 {
            out.push([pa, pb, pc]);
        }
    }
    out.into_iter().map(|t| t.map(|p| [p[0] as f32, p[1] as f32])).collect()
}

fn cross2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - b[0] * a[1]
}

// ---- the photos -----------------------------------------------------------------------------------------------

/// A photo with a position.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPoint {
    pub id: u64,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

impl MapPoint {
    /// The tooltip (`f'{name}\n{lat:.6f}, {lon:.6f}'`).
    pub fn tip(&self) -> String {
        format!("{}\n{:.6}, {:.6}", self.name, self.lat, self.lon)
    }
}

/// Python's `float(value)` of a value read from JSON; `None` where it raises (`KeyError` aside).
pub fn python_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(text) => {
            let text = text.trim();
            // Underscores are allowed between digits only.
            let bytes = text.as_bytes();
            for (i, b) in bytes.iter().enumerate() {
                if *b == b'_' {
                    let digit = |j: Option<usize>| j.and_then(|j| bytes.get(j)).is_some_and(u8::is_ascii_digit);
                    if !(digit(i.checked_sub(1)) && digit(Some(i + 1))) {
                        return None;
                    }
                }
            }
            text.replace('_', "").parse().ok()
        }
        _ => None,
    }
}

/// Port of the marker loop of `MapView.__init__`: the photos of the list with a latitude in -90..90 and a
/// longitude in -180..180, the entered metadata (`user_metadata`) over the file's (`info`). `photos` are
/// `(id, name)`; a photo that is not in the catalogue is left out.
pub fn gps_points(catalog: &Catalog, photos: &[(i64, String)]) -> Result<Vec<MapPoint>, String> {
    let mut points = Vec::new();
    for (id, name) in photos {
        let Some(row) = catalog.photo(*id).map_err(|e| e.to_string())? else { continue };
        let mut info = row["info"].as_object().cloned().unwrap_or_default();
        if let Some(entered) = row["user_metadata"].as_object() {
            info.extend(entered.clone());
        }
        let (Some(lat), Some(lon)) =
            (info.get("latitude").and_then(python_float), info.get("longitude").and_then(python_float))
        else {
            continue;
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            continue;
        }
        points.push(MapPoint { id: *id as u64, name: name.clone(), lat, lon });
    }
    Ok(points)
}

// ---- the view -------------------------------------------------------------------------------------------------

/// Zoom and pan (`QGraphicsView`: `fitInView`, wheel zoom of 1.3 under the mouse, hand drag, the scene rectangle
/// -180..180 by -90..90 as the scroll range).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapView {
    /// Pixels per degree (`transform().m11()`).
    pub scale: f32,
    /// The scene point at the middle of the view.
    pub center: Vec2,
}

impl Default for MapView {
    fn default() -> Self {
        Self { scale: 1.0, center: Vec2::ZERO }
    }
}

impl MapView {
    /// `fitInView(sceneRect, KeepAspectRatio)`: Qt keeps a margin of 2 pixels.
    pub fn fit(size: Vec2) -> Self {
        let w = (size.x - 4.0).max(1.0);
        let h = (size.y - 4.0).max(1.0);
        Self { scale: (w / 360.0).min(h / 180.0), center: Vec2::ZERO }
    }

    /// Keeps the scene rectangle in the scroll range: centred where it is smaller than the view.
    pub fn clamp(&mut self, size: Vec2) {
        let limit = |half_scene: f32, view: f32, scale: f32| {
            let slack = half_scene - view / (2.0 * scale);
            if slack > 0.0 {
                Some(slack)
            } else {
                None
            }
        };
        self.center.x = limit(180.0, size.x, self.scale).map_or(0.0, |s| self.center.x.clamp(-s, s));
        self.center.y = limit(90.0, size.y, self.scale).map_or(0.0, |s| self.center.y.clamp(-s, s));
    }

    /// `wheelEvent`: a factor of 1.3 (or 1/1.3) when the scale stays between 0.5 and 2000; the scene point under
    /// the pointer (`offset` from the view's middle, in pixels) stays there.
    pub fn zoom(&mut self, size: Vec2, offset: Vec2, up: bool) {
        let factor = if up { 1.3 } else { 1.0 / 1.3 };
        let scale = self.scale * factor;
        if scale > 0.5 && scale < 2000.0 {
            let anchor = self.center + offset / self.scale;
            self.scale = scale;
            self.center = anchor - offset / scale;
            self.clamp(size);
        }
    }

    /// Hand drag by `delta` pixels.
    pub fn pan(&mut self, size: Vec2, delta: Vec2) {
        self.center -= delta / self.scale;
        self.clamp(size);
    }

    /// Screen position of the scene point `[x, y]` in a view occupying `rect`.
    pub fn to_screen(&self, rect: Rect, p: [f32; 2]) -> Pos2 {
        rect.center() + (Vec2::new(p[0], p[1]) - self.center) * self.scale
    }

    /// The marker under `pos`, the topmost (the last added) first: `QGraphicsView.items(pos)` with the ellipse's
    /// outline included.
    pub fn marker_at(&self, rect: Rect, points: &[MapPoint], pos: Pos2) -> Option<usize> {
        let reach = MARKER_RADIUS + 0.5;
        (0..points.len()).rev().find(|&i| {
            let at = self.to_screen(rect, [points[i].lon as f32, -points[i].lat as f32]);
            at.distance(pos) <= reach
        })
    }
}

/// The map window.
pub struct MapDialog {
    pub points: Vec<MapPoint>,
    world: Arc<World>,
    pub view: MapView,
    /// Whether the first fit happened (`QTimer.singleShot(0, fitInView)`).
    fitted: bool,
    /// The photo a double-click asked for (the dialog is non-modal: the window opens it after the frame).
    pub opened: Option<u64>,
}

impl MapDialog {
    pub fn new(points: Vec<MapPoint>, world: Arc<World>) -> Self {
        Self { points, world, view: MapView::default(), fitted: false, opened: None }
    }

    /// `GPS 사진 {0}장 · …` line.
    pub fn summary(&self) -> String {
        tr_with("GPS 사진 {0}장 · 휠 확대 / 드래그 이동 / 점 더블클릭으로 사진 열기", &[&self.points.len().to_string()])
    }

    /// Draws the land and the markers into `rect`.
    pub fn paint(&self, painter: &egui::Painter, rect: Rect) {
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, SEA);
        let view = self.view;
        let mesh = |triangles: &[[[f32; 2]; 3]], color: Color32| {
            let mut mesh = Mesh::default();
            for tri in triangles {
                let p: Vec<Pos2> = tri.iter().map(|q| view.to_screen(rect, *q)).collect();
                let low = p[0].min(p[1]).min(p[2]);
                let high = p[0].max(p[1]).max(p[2]);
                if !rect.intersects(Rect::from_min_max(low, high)) {
                    continue;
                }
                let base = mesh.vertices.len() as u32;
                for q in p {
                    mesh.colored_vertex(q, color);
                }
                mesh.indices.extend([base, base + 1, base + 2]);
            }
            Shape::mesh(mesh)
        };
        painter.add(mesh(&self.world.land, LAND));
        painter.add(mesh(&self.world.holes, SEA));
        let stroke = Stroke::new(0.2 * view.scale, COAST);
        for (ring, closed) in &self.world.strokes {
            let points: Vec<Pos2> = ring.iter().map(|p| view.to_screen(rect, *p)).collect();
            let low = points.iter().fold(Pos2::new(f32::MAX, f32::MAX), |a, p| a.min(*p));
            let high = points.iter().fold(Pos2::new(f32::MIN, f32::MIN), |a, p| a.max(*p));
            if points.len() > 1 && rect.expand(stroke.width).intersects(Rect::from_min_max(low, high)) {
                painter.add(if *closed { Shape::closed_line(points, stroke) } else { Shape::line(points, stroke) });
            }
        }
        for point in &self.points {
            let at = view.to_screen(rect, [point.lon as f32, -point.lat as f32]);
            if rect.expand(MARKER_RADIUS + 1.0).contains(at) {
                painter.circle(at, MARKER_RADIUS, MARKER, Stroke::new(1.0, MARKER_PEN));
            }
        }
    }

    /// The map's own interaction in `rect`: the first fit, wheel, drag, tooltip and double-click.
    pub fn interact(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let response = ui.interact(rect, egui::Id::unique("gps_map_view"), Sense::click_and_drag());
        if !self.fitted {
            self.view = MapView::fit(rect.size());
            self.fitted = true;
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            self.view.pan(rect.size(), response.drag_delta());
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if let Some(pos) = response.hover_pos() {
            // Sum wheel scroll per frame and cap step count so high-frequency trackpad pixel events
            // zoom smoothly instead of jumping across many zoom levels in a single swipe.
            let scroll_y: f32 = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { unit, delta, .. } if delta.y != 0.0 => {
                            let step = match unit {
                                egui::MouseWheelUnit::Point => delta.y / 50.0,
                                egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta.y,
                            };
                            Some(step)
                        }
                        _ => None,
                    })
                    .sum()
            });
            if scroll_y.abs() >= 0.25 {
                let steps = scroll_y.abs().round().clamp(1.0, 2.0) as usize;
                for _ in 0..steps {
                    self.view.zoom(rect.size(), pos - rect.center(), scroll_y > 0.0);
                }
            }
            if let Some(i) = self.view.marker_at(rect, &self.points, pos) {
                response.clone().on_hover_text(self.points[i].tip());
            }
        }
        if response.double_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some(i) = self.view.marker_at(rect, &self.points, pos) {
                    self.opened = Some(self.points[i].id);
                }
            }
        }
        self.paint(ui.painter(), rect);
    }
}

impl Dialog for MapDialog {
    fn id(&self) -> &'static str {
        "map"
    }

    fn modal(&self) -> bool {
        false
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        let mut open = true;
        let screen = ctx.content_rect();
        let size = SIZE + Vec2::new(32.0, 80.0);
        egui::Window::new(tr("GPS 지도"))
            .id(egui::Id::unique("gps_map_window"))
            .open(&mut open)
            .frame(super::frame())
            .default_size(size)
            .default_pos(Pos2::new(
                (screen.center().x - size.x / 2.0).max(0.0),
                (screen.center().y - size.y / 2.0).max(0.0),
            ))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let labels = 2.0 * (t::FONT_SIZE + 10.0);
                let height = (ui.available_height() - labels).max(100.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
                self.interact(ui, rect);
                widgets::label(ui, &self.summary(), t::TEXT, t::FONT_SIZE);
                widgets::label(
                    ui,
                    &tr("오프라인 지도 · Natural Earth 1:110m · 촬영 위치를 외부로 전송하지 않습니다."),
                    t::TEXT,
                    t::FONT_SIZE,
                );
            });
        // `lambda ident: (self.w.activate(ident), self.w.set_mode(0))`.
        if let Some(id) = self.opened.take() {
            if let Some(index) = cx.doc.index_of(id) {
                cx.select_photo(index);
            }
            cx.set_mode(Mode::Develop);
        }
        open
    }
}
