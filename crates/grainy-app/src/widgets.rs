//! Widgets drawn to look like the oracle's Qt widgets under `STYLE`: buttons, section titles, fold-outs, check
//! boxes, the panel switch and edited dot, and the slider row (`Adjustment` in luma/widgets.py).

pub(crate) mod basic_keys;
pub mod curve;

use std::sync::Arc;

use eframe::egui::{
    self, text::LayoutJob, Align2, Color32, CornerRadius, FontId, Galley, Id, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, TextFormat, Ui, Vec2,
};
use grainy_i18n::tr;

use crate::adjustments::Spec;
use crate::theme as t;

/// Lays out one line of text.
pub fn galley(ui: &Ui, text: impl Into<String>, font: FontId, color: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.into(), font, color)
}

/// Lays out text wrapped at `width`.
pub fn wrapped(ui: &Ui, text: impl Into<String>, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    ui.painter().layout(text.into(), font, color, width)
}

/// Text with extra letter spacing (`letter-spacing` in the stylesheet).
pub fn spaced(ui: &Ui, text: &str, font: FontId, color: Color32, spacing: f32) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    ui.painter().layout_job(job)
}

/// The kinds of `QPushButton` in the stylesheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `QPushButton`.
    Normal,
    /// `QPushButton#primary`.
    Primary,
    /// `QPushButton#feedback`.
    Feedback,
    /// `QPushButton#nav`: left-aligned, taller.
    Nav,
}

impl Kind {
    fn padding(self, ui: &Ui) -> Vec2 {
        match self {
            Kind::Normal if t::in_develop_column(ui) => ui.spacing().button_padding,
            Kind::Normal => Vec2::new(11.0, 7.0),
            Kind::Primary => Vec2::new(17.0, 9.0),
            Kind::Feedback => Vec2::new(12.0, 5.0),
            Kind::Nav => Vec2::new(10.0, 10.0),
        }
    }
}

/// A push button. `checked` draws the checked state; `width` fixes the width (else it fits the text).
#[derive(Debug, Clone, Copy)]
pub struct Button<'a> {
    pub text: &'a str,
    pub kind: Kind,
    pub checked: bool,
    pub enabled: bool,
    pub width: Option<f32>,
}

impl<'a> Button<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text, kind: Kind::Normal, checked: false, enabled: true, width: None }
    }
    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// The button's size for its text.
    pub fn size(&self, ui: &Ui) -> Vec2 {
        let text = galley(ui, self.text, self.font(), Color32::WHITE);
        let pad = self.kind.padding(ui);
        let border = if self.kind == Kind::Primary { 0.0 } else { 2.0 };
        let height = text.size().y.max(16.0) + 2.0 * pad.y + border;
        Vec2::new(self.width.unwrap_or(text.size().x + 2.0 * pad.x + border), height)
    }

    fn font(&self) -> FontId {
        if self.kind == Kind::Primary {
            t::bold(t::FONT_SIZE)
        } else {
            t::font(t::FONT_SIZE)
        }
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let size = self.size(ui);
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let (rect, response) = ui.allocate_exact_size(size, sense);
        self.paint(ui, rect, &response);
        response
    }

    /// Paints the button in `rect` (already allocated).
    pub fn paint(&self, ui: &Ui, rect: Rect, response: &Response) {
        if !ui.is_rect_visible(rect) {
            return;
        }
        let hover = self.enabled && response.hovered();
        let pressed = self.enabled && response.is_pointer_button_down_on();
        let (fill, border, text) = match self.kind {
            Kind::Primary => (if hover { t::PRIMARY_HOVER } else { t::PRIMARY }, None, t::PRIMARY_TEXT),
            Kind::Feedback if pressed => (
                Color32::from_rgb(0x2a, 0x4f, 0x55),
                Some(t::FEEDBACK_HOVER_BORDER),
                Color32::from_rgb(0xe2, 0xfb, 0xfb),
            ),
            Kind::Feedback if hover => (t::FEEDBACK_HOVER, Some(t::FEEDBACK_HOVER_BORDER), t::FEEDBACK_HOVER_TEXT),
            Kind::Feedback => (Color32::TRANSPARENT, Some(t::FEEDBACK_BORDER), t::FEEDBACK_TEXT),
            _ if !self.enabled => (t::BUTTON, Some(Color32::from_rgb(0x2c, 0x32, 0x39)), t::BUTTON_DISABLED_TEXT),
            _ if pressed || self.checked => (t::CHECKED, Some(t::CHECKED_BORDER), t::CHECKED_TEXT),
            _ if hover => (t::BUTTON_HOVER, Some(t::BUTTON_HOVER_BORDER), t::TEXT),
            _ => (t::BUTTON, Some(t::BUTTON_BORDER), t::TEXT),
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::ZERO,
            fill,
            border.map_or(Stroke::NONE, |c| Stroke::new(1.0, c)),
            StrokeKind::Inside,
        );
        let pad = self.kind.padding(ui);
        let max_width = rect.width() - 2.0 * pad.x.min(8.0);
        let mut label = galley(ui, self.text, self.font(), text);
        if label.size().x > max_width {
            label = elided(ui, self.text, self.font(), text, max_width);
        }
        let pos = if self.kind == Kind::Nav {
            Pos2::new(rect.left() + pad.x, rect.center().y - label.size().y / 2.0)
        } else {
            rect.center() - label.size() / 2.0
        };
        painter.galley(pos, label, text);
    }
}

/// A push button of the default kind.
pub fn button(ui: &mut Ui, text: &str) -> Response {
    Button::new(text).show(ui)
}

/// Text cut with "…" at the end to fit `width`.
pub fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        let g = galley(ui, candidate, font.clone(), color);
        if g.size().x <= width {
            return g;
        }
    }
    galley(ui, "…", font, color)
}

/// Text cut with "…" in the middle to fit `width` (`Qt.TextElideMode.ElideMiddle`).
pub fn elided_middle(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let full = galley(ui, text, font.clone(), color);
    if full.size().x <= width {
        return full;
    }
    let chars: Vec<char> = text.chars().collect();
    let candidate = |keep: usize| {
        let head = keep.div_ceil(2);
        let tail = keep / 2;
        let value =
            chars[..head].iter().collect::<String>() + "…" + &chars[chars.len() - tail..].iter().collect::<String>();
        galley(ui, value, font.clone(), color)
    };
    // Preserve the original fitting order for normal labels. Bound shaping work for long metadata names.
    if chars.len() <= 64 {
        return (1..chars.len()).rev().map(candidate).find(|g| g.size().x <= width).unwrap_or_else(|| candidate(0));
    }
    let mut low = 0;
    let mut high = chars.len();
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if candidate(mid).size().x <= width {
            low = mid;
        } else {
            high = mid;
        }
    }
    candidate(low)
}

/// Buttons side by side with no gap, filling `width` in equal parts (a row of QPushButtons in a QHBoxLayout
/// whose borders meet). Returns the index of the clicked button.
pub fn button_row(ui: &mut Ui, buttons: &[Button<'_>], width: f32, gap: f32) -> Option<usize> {
    let count = buttons.len().max(1) as f32;
    let each = ((width - gap * (count - 1.0)) / count).floor();
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, b) in buttons.iter().enumerate() {
            if b.width(each).show(ui).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// `QLabel#section`: small spaced capitals in the accent colour, with room above and below.
pub fn section(ui: &mut Ui, text: &str) {
    let g = spaced(ui, text, t::bold(10.0), t::SECTION, 2.0);
    let (above, below) = if t::in_develop_column(ui) { (4.0, 2.0) } else { (12.0, 6.0) };
    let width = if ui.layout().main_dir().is_horizontal() { g.size().x } else { ui.available_width() };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, above + g.size().y + below), Sense::hover());
    ui.painter().galley(Pos2::new(rect.left(), rect.top() + above), g, t::SECTION);
}

/// `QLabel#muted`, wrapped to the available width.
pub fn muted(ui: &mut Ui, text: &str) -> Response {
    label(ui, text, t::MUTED, t::FONT_SIZE)
}

/// A wrapped label in a colour.
pub fn label(ui: &mut Ui, text: &str, color: Color32, size: f32) -> Response {
    let g = wrapped(ui, text, t::font(size), color, ui.available_width());
    let (rect, response) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, color);
    response
}

/// A small filled triangle pointing right (closed) or down (open), Qt's arrow icons.
pub fn arrow(ui: &Ui, center: Pos2, open: bool, color: Color32) {
    let s = 3.5;
    let points = if open {
        vec![center + Vec2::new(-s, -s * 0.6), center + Vec2::new(s, -s * 0.6), center + Vec2::new(0.0, s * 0.8)]
    } else {
        vec![center + Vec2::new(-s * 0.6, -s), center + Vec2::new(s * 0.8, 0.0), center + Vec2::new(-s * 0.6, s)]
    };
    ui.painter().add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// `QToolButton#foldout`: an arrow and a bold title that show or hide what follows.
pub fn foldout(ui: &mut Ui, title: &str, open: &mut bool) -> Response {
    let g = galley(ui, title, t::bold(t::FONT_SIZE), t::SECTION);
    let height = if t::in_develop_column(ui) { (g.size().y + 2.0).max(20.0) } else { g.size().y + 18.0 };
    let width = (g.size().x + 26.0).min(ui.available_width());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    arrow(ui, Pos2::new(rect.left() + 8.0, rect.center().y), *open, t::SECTION);
    ui.painter().galley(Pos2::new(rect.left() + 20.0, rect.center().y - g.size().y / 2.0), g, t::SECTION);
    if response.clicked() {
        *open = !*open;
    }
    response
}

/// `QCheckBox`: a dark square indicator and the text.
pub fn checkbox(ui: &mut Ui, value: &mut bool, text: &str) -> Response {
    let g = galley(ui, text, t::font(t::FONT_SIZE), t::TEXT);
    let height = if t::in_develop_column(ui) { g.size().y.max(20.0) } else { g.size().y.max(13.0) + 10.0 };
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(13.0 + 7.0 + g.size().x, height), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let indicator = Rect::from_center_size(Pos2::new(rect.left() + 6.5, rect.center().y), Vec2::splat(13.0));
    let border = if response.hovered() { t::BUTTON_HOVER_BORDER } else { t::INPUT_BORDER };
    ui.painter().rect(indicator, CornerRadius::ZERO, t::INPUT_BG, Stroke::new(1.0, border), StrokeKind::Inside);
    if *value {
        let c = indicator.center();
        let stroke = Stroke::new(1.6, t::TEXT);
        ui.painter().line_segment([c + Vec2::new(-3.5, 0.0), c + Vec2::new(-1.0, 2.8)], stroke);
        ui.painter().line_segment([c + Vec2::new(-1.0, 2.8), c + Vec2::new(3.8, -3.0)], stroke);
    }
    ui.painter().galley(Pos2::new(rect.left() + 20.0, rect.center().y - g.size().y / 2.0), g, t::TEXT);
    response
}

/// `PanelSwitch`: a flat track with a square knob, right when on.
pub fn paint_switch(ui: &Ui, rect: Rect, on: bool, hover: bool) {
    let (mut track, mut knob) =
        if on { (t::SWITCH_TRACK_ON, t::SWITCH_KNOB_ON) } else { (t::SWITCH_TRACK_OFF, t::SWITCH_KNOB_OFF) };
    if hover {
        track = lighter(track, 1.3);
        knob = lighter(knob, 1.15);
    }
    let painter = ui.painter();
    painter.rect_stroke(rect.shrink(0.5), CornerRadius::ZERO, Stroke::new(1.0, track), StrokeKind::Middle);
    let x = if on { rect.right() - 9.0 } else { rect.left() + 3.0 };
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x, rect.top() + 3.0), Vec2::splat(6.0)),
        CornerRadius::ZERO,
        knob,
    );
}

/// `EditedDot`: the panel holds values other than the defaults (grey while the panel is off).
pub fn paint_dot(ui: &Ui, center: Pos2, off: bool) {
    ui.painter().circle_filled(center, 3.0, if off { t::DOT_OFF } else { t::DOT });
}

/// `QColor.lighter(factor)`, approximately (scales the value channel).
pub fn lighter(color: Color32, factor: f32) -> Color32 {
    let f = |c: u8| (f32::from(c) * factor).min(255.0) as u8;
    Color32::from_rgb(f(color.r()), f(color.g()), f(color.b()))
}

/// A single-line text field in the `QLineEdit` style, full width.
pub fn line_edit(ui: &mut Ui, id: &str, text: &mut String, hint: &str) -> Response {
    let frame = egui::Frame::new()
        .fill(t::INPUT_BG)
        .stroke(Stroke::new(1.0, t::INPUT_BORDER))
        .inner_margin(egui::Margin::symmetric(7, 7));
    egui::TextEdit::singleline(text)
        .id_salt(id)
        .hint_text(egui::RichText::new(hint).color(t::MUTED))
        .frame(frame)
        .desired_width(f32::INFINITY)
        .text_color(t::TEXT)
        .show(ui)
        .response
        .response
}

/// What a slider row did this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RowEvent {
    /// A new value to set (live, without committing).
    pub set: Option<f64>,
    /// The interaction ended: commit one undo step (`committed` of the oracle's `Adjustment`).
    pub commit: bool,
}

#[derive(Debug, Clone, Copy)]
struct Grab {
    /// Pointer x minus the handle centre when the handle was pressed; `None` after a page step on the groove.
    offset: Option<f32>,
}

/// Width of the label column in a compact row (`Adjustment.compact`).
pub const LABEL_WIDTH: f32 = 88.0;
/// Width of the number field in a compact row.
pub const VALUE_WIDTH: f32 = 55.0;

/// The compact slider row of the oracle's `Adjustment`: label (88 px, wraps), slider, signed number field (55 px).
/// Dragging sets values live and commits on release; a double-click resets; typing a number and pressing Enter or
/// leaving the field sets and commits.
pub fn adjustment(ui: &mut Ui, spec: &Spec, value: f64, enabled: bool) -> RowEvent {
    adjustment_row(ui, spec.key, &tr(spec.label), spec, value, enabled)
}

/// [`adjustment`] for a row whose label and id are given (rows bound to list entries).
pub fn adjustment_row(ui: &mut Ui, id: &str, label: &str, spec: &Spec, value: f64, enabled: bool) -> RowEvent {
    let mut event = RowEvent::default();
    let width = ui.available_width();
    let text_color = if enabled { t::TEXT } else { t::BUTTON_DISABLED_TEXT };
    let label_galley = wrapped(ui, label, t::font(t::FONT_SIZE), text_color, LABEL_WIDTH);
    let compact = t::in_develop_column(ui);
    let field_height = if compact { 18.0 } else { 21.0 };
    let height = label_galley.size().y.max(if compact { 18.0 } else { 23.0 }) + 2.0;
    let (rect, label_response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let inner = rect.shrink2(Vec2::new(0.0, 1.0));
    let base = Id::unique(("adjustment", id));
    ui.painter().galley(
        Pos2::new(inner.left(), inner.center().y - label_galley.size().y / 2.0),
        label_galley,
        text_color,
    );
    label_response.on_hover_text(label);

    let value_rect = Rect::from_min_max(Pos2::new(inner.right() - VALUE_WIDTH, inner.top()), inner.max);
    let slider_rect = Rect::from_min_max(
        Pos2::new(inner.left() + LABEL_WIDTH + 5.0, inner.top()),
        Pos2::new(value_rect.left() - 5.0, inner.bottom()),
    );

    // Slider
    let (low, high) = spec.steps();
    let position = spec.position(value);
    let span = (high - low).max(1) as f32;
    let handle_size = if compact { Vec2::new(8.0, 9.0) } else { Vec2::new(10.0, 11.0) };
    let track = slider_rect.shrink2(Vec2::new(handle_size.x / 2.0, 0.0));
    let x_of = |p: i64| track.left() + (p - low) as f32 / span * track.width();
    let p_of = |x: f32| low + (((x - track.left()) / track.width()).clamp(0.0, 1.0) * span).round() as i64;
    let slider_id = base.with("slider");
    let sense = if enabled { Sense::click_and_drag() } else { Sense::hover() };
    let response = ui.interact(slider_rect, slider_id, sense);
    let handle_rect = Rect::from_center_size(Pos2::new(x_of(position), slider_rect.center().y), handle_size);
    if enabled {
        let (pressed, released, down, pointer) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.primary_down(),
                i.pointer.interact_pos(),
            )
        });
        let mut grab: Option<Grab> = ui.data(|d| d.get_temp(slider_id));
        if pressed && response.hovered() {
            if let Some(p) = pointer {
                let grab_rect = if compact {
                    Rect::from_center_size(handle_rect.center(), Vec2::new(14.0, slider_rect.height()))
                } else {
                    handle_rect.expand(2.0)
                };
                if grab_rect.contains(p) {
                    grab = Some(Grab { offset: Some(p.x - handle_rect.center().x) });
                } else {
                    // A click on the groove moves one page step towards it (QAbstractSlider).
                    let page = (spec.scale as i64).max(1);
                    let target = if p.x < handle_rect.center().x { position - page } else { position + page };
                    let target = target.clamp(low, high);
                    if target != position {
                        event.set = Some(spec.value_at(target));
                    }
                    grab = Some(Grab { offset: None });
                }
            }
        } else if let (Some(Grab { offset: Some(offset) }), true, Some(p)) = (grab, down, pointer) {
            let target = p_of(p.x - offset);
            if target != position {
                event.set = Some(spec.value_at(target));
            }
        }
        if grab.is_some() && (released || !down) {
            event.commit = true;
            grab = None;
        }
        ui.data_mut(|d| match grab {
            Some(g) => {
                d.insert_temp(slider_id, g);
            }
            None => d.remove::<Grab>(slider_id),
        });
        if response.double_clicked() {
            event.set = Some(spec.reset_value());
            event.commit = true;
        }
    }
    if compact && enabled && ui.is_enabled() {
        basic_keys::on_row(&response, spec, value, &mut event);
    }
    let painter = ui.painter();
    let groove =
        Rect::from_center_size(slider_rect.center(), Vec2::new(slider_rect.width(), if compact { 2.0 } else { 3.0 }));
    painter.rect_filled(groove, CornerRadius::ZERO, t::GROOVE);
    let shown = event.set.map_or(position, |v| spec.position(v));
    let handle = Rect::from_center_size(Pos2::new(x_of(shown), slider_rect.center().y), handle_size);
    let handle_color = if !enabled {
        t::GROOVE
    } else if response.has_focus() {
        t::BRAND
    } else {
        t::HANDLE
    };
    painter.rect_filled(handle, CornerRadius::ZERO, handle_color);
    response.on_hover_text(tr("더블클릭하면 기본값으로 돌아갑니다."));

    // Number field
    let field_id = base.with("value");
    let focused = ui.memory(|m| m.has_focus(field_id));
    let shown_value = spec.value_at(shown);
    let mut text: String = if focused {
        ui.data(|d| d.get_temp::<String>(field_id)).unwrap_or_else(|| spec.format(shown_value))
    } else {
        spec.format(shown_value)
    };
    let frame = if focused {
        egui::Frame::new().fill(t::INPUT_BG).stroke(Stroke::new(1.0, t::CHECKED_BORDER))
    } else {
        egui::Frame::new().stroke(Stroke::new(1.0, Color32::TRANSPARENT))
    }
    .inner_margin(egui::Margin::symmetric(2, 0));
    let field_rect = Rect::from_center_size(value_rect.center(), Vec2::new(value_rect.width(), field_height));
    let output = ui
        .scope_builder(egui::UiBuilder::new().max_rect(field_rect), |ui| {
            egui::TextEdit::singleline(&mut text)
                .id(field_id)
                .frame(frame)
                .align(Align2::RIGHT_CENTER)
                .desired_width(field_rect.width())
                .min_size(field_rect.size())
                .text_color(if focused {
                    t::BRAND
                } else if enabled {
                    t::VALUE
                } else {
                    t::BUTTON_DISABLED_TEXT
                })
                .font(t::font(t::FONT_SIZE))
                .interactive(enabled)
                .show(ui)
        })
        .inner;
    let field = output.response.response;
    if field.gained_focus() {
        // Select everything, as AdjustmentNumber.focusInEvent does.
        let mut state = output.state;
        let all =
            egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), field_id);
    }
    if field.has_focus() {
        ui.data_mut(|d| d.insert_temp(field_id, text.clone()));
    }
    if field.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(field_id));
        if let Some(number) = spec.parse(&text) {
            if spec.position(number) != shown {
                event.set = Some(number);
            }
        }
        event.commit = true;
    }
    field.on_hover_text(tr("숫자를 입력한 뒤 Enter 또는 다른 곳을 클릭해 적용합니다."));
    event
}

/// The horizontal alignment helper: a rect of `width` at the left of the remaining row.
pub fn take_row(ui: &mut Ui, height: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    rect
}

/// Paints text centred vertically at the left of `rect`.
pub fn paint_left(ui: &Ui, rect: Rect, text: &str, font: FontId, color: Color32) {
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text, font, color);
}
