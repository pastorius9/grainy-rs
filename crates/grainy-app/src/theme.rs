//! Grainy's dark style. Port of the `STYLE` stylesheet in luma/app.py: the same colours, square corners and
//! font sizes, applied to egui's visuals and used directly by the custom-drawn widgets in [`crate::widgets`].

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};

/// `#rrggbb` as a colour (const).
const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// `QWidget { background }`: panels, columns.
pub const BG: Color32 = hex(0x1b1f24);
/// `QWidget { color }`: ordinary text.
pub const TEXT: Color32 = hex(0xdce0e4);
/// `QMainWindow { background }`: the window behind everything, the photo view's black background.
pub const WINDOW: Color32 = hex(0x101215);
/// `QMenu` background and border.
pub const MENU_BG: Color32 = hex(0x20252b);
pub const MENU_BORDER: Color32 = hex(0x39414b);
/// Selected menu item / checked button.
pub const CHECKED: Color32 = hex(0x4d453a);
pub const CHECKED_BORDER: Color32 = hex(0xb89c76);
pub const CHECKED_TEXT: Color32 = hex(0xffe7c8);
/// Disabled text.
pub const DISABLED: Color32 = hex(0x5f6770);
/// `QLabel#brand`.
pub const BRAND: Color32 = hex(0xf4e7d4);
/// `QLabel#muted`.
pub const MUTED: Color32 = hex(0x89929d);
/// `QLabel#section`, `QToolButton#foldout`.
pub const SECTION: Color32 = hex(0xd2b991);
/// `QPushButton`.
pub const BUTTON: Color32 = hex(0x272d34);
pub const BUTTON_BORDER: Color32 = hex(0x373e47);
pub const BUTTON_HOVER: Color32 = hex(0x363e47);
pub const BUTTON_HOVER_BORDER: Color32 = hex(0x68717a);
pub const BUTTON_DISABLED_TEXT: Color32 = hex(0x59616a);
/// `QPushButton#primary` (Export, Apply).
pub const PRIMARY: Color32 = hex(0xdec49f);
pub const PRIMARY_HOVER: Color32 = hex(0xf0d8b5);
pub const PRIMARY_TEXT: Color32 = hex(0x211d17);
/// `QPushButton#feedback`.
pub const FEEDBACK_BORDER: Color32 = hex(0x3f7f86);
pub const FEEDBACK_TEXT: Color32 = hex(0x8fd3d6);
pub const FEEDBACK_HOVER: Color32 = hex(0x1f3a3f);
pub const FEEDBACK_HOVER_BORDER: Color32 = hex(0x6fc0c6);
pub const FEEDBACK_HOVER_TEXT: Color32 = hex(0xc9f1f2);
/// `QLineEdit, QComboBox`.
pub const INPUT_BG: Color32 = hex(0x15191e);
pub const INPUT_BORDER: Color32 = hex(0x39414b);
/// Focus border of text fields; the accent of tool tabs and the primary button.
pub const ACCENT: Color32 = hex(0xdec49f);
/// `QDoubleSpinBox#adjustmentValue`.
pub const VALUE: Color32 = hex(0xaeb6bf);
/// `QListWidget`, `QTreeWidget` background.
pub const LIST_BG: Color32 = hex(0x15181d);
/// `QToolButton#developPanelHeader`.
pub const PANEL_HEADER: Color32 = hex(0x25292e);
pub const PANEL_HEADER_HOVER: Color32 = hex(0x30353b);
pub const PANEL_HEADER_LINE: Color32 = hex(0x3a3e43);
pub const PANEL_HEADER_TEXT: Color32 = hex(0xd9dcdf);
pub const PANEL_OFF_TEXT: Color32 = hex(0x6f767e);
/// `QWidget#developPanelBody`.
pub const PANEL_BODY: Color32 = hex(0x1d2126);
/// `QToolButton#developTool`.
pub const TOOL_TEXT: Color32 = hex(0xa4aab2);
pub const TOOL_CHECKED: Color32 = hex(0xf0d6b2);
/// `QSlider` groove and handle.
pub const GROOVE: Color32 = hex(0x444c55);
pub const HANDLE: Color32 = hex(0xdfc29b);
/// `QScrollBar::handle`.
pub const SCROLL_HANDLE: Color32 = hex(0x48515b);
/// `QStatusBar`.
pub const STATUS_BG: Color32 = hex(0x14171b);
pub const STATUS_TEXT: Color32 = hex(0x99a3ad);
/// `QToolTip`.
pub const TOOLTIP_BG: Color32 = hex(0x343c45);
pub const TOOLTIP_BORDER: Color32 = hex(0x66717d);
/// `EditedDot` in luma/develop_panels.py.
pub const DOT: Color32 = hex(0xa48e6b);
pub const DOT_OFF: Color32 = hex(0x5b6168);
/// `PanelSwitch`: track and knob, on and off.
pub const SWITCH_TRACK_ON: Color32 = hex(0x5f6770);
pub const SWITCH_TRACK_OFF: Color32 = hex(0x41484f);
pub const SWITCH_KNOB_ON: Color32 = hex(0xaab1b8);
pub const SWITCH_KNOB_OFF: Color32 = hex(0x555c64);
/// `Histogram` background (luma/widgets.py).
pub const HISTOGRAM_BG: Color32 = hex(0x171b20);
/// Tree item selection (`QTreeWidget::item:selected`) and list selection.
pub const LIST_SELECTED: Color32 = hex(0x35322e);
pub const LIST_SELECTED_BORDER: Color32 = hex(0xd3b68d);
pub const LIST_HOVER: Color32 = hex(0x29313a);
/// Filmstrip thumbnail placeholder (`PhotoDelegate`).
pub const THUMB_EMPTY: Color32 = hex(0x222930);

/// `font-size:12px`: the interface text.
pub const FONT_SIZE: f32 = 12.0;

/// The bold family registered by [`crate::fonts::install`].
pub fn bold_family() -> FontFamily {
    FontFamily::Name(crate::fonts::BOLD.into())
}

/// Interface text at `size` px.
pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Bold interface text (`font-weight:600`) at `size` px.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, bold_family())
}

/// Applies the style to egui's own widgets (menus, combo boxes, text fields, scroll bars, tooltips).
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, font(10.0)),
            (TextStyle::Body, font(FONT_SIZE)),
            (TextStyle::Button, font(FONT_SIZE)),
            (TextStyle::Heading, bold(14.0)),
            (TextStyle::Monospace, FontId::new(11.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(5.0, 5.0);
        style.spacing.button_padding = Vec2::new(11.0, 7.0);
        style.spacing.menu_margin = Margin::symmetric(0, 4);
        style.spacing.interact_size.y = 22.0;
        // `QScrollBar`: 7 px, a flat #48515b handle, always shown when there is something to scroll.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 7.0;
        style.spacing.scroll.handle_min_length = 24.0;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.spacing.scroll.foreground_color = false;
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = MENU_BG;
        v.window_stroke = Stroke::new(1.0, MENU_BORDER);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.extreme_bg_color = BG; // the scroll bar's track (`QScrollBar { background }`)
        v.text_edit_bg_color = Some(INPUT_BG);
        v.faint_bg_color = LIST_BG;
        v.code_bg_color = INPUT_BG;
        v.hyperlink_color = FEEDBACK_TEXT;
        v.selection.bg_fill = CHECKED;
        v.selection.stroke = Stroke::new(1.0, CHECKED_TEXT);
        v.text_cursor.stroke = Stroke::new(1.0, BRAND);
        v.striped = false;
        v.slider_trailing_fill = false;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG;
        w.noninteractive.weak_bg_fill = BG;
        w.noninteractive.bg_stroke = Stroke::new(1.0, MENU_BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_fill = SCROLL_HANDLE; // the scroll bar handle (egui's own sliders are not used)
        w.inactive.weak_bg_fill = INPUT_BG;
        w.inactive.bg_stroke = Stroke::new(1.0, INPUT_BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = BUTTON_HOVER_BORDER;
        w.hovered.weak_bg_fill = CHECKED;
        w.hovered.bg_stroke = Stroke::new(1.0, BUTTON_HOVER_BORDER);
        w.hovered.fg_stroke = Stroke::new(1.0, CHECKED_TEXT);
        w.active.bg_fill = CHECKED_BORDER;
        w.active.weak_bg_fill = CHECKED;
        w.active.bg_stroke = Stroke::new(1.0, CHECKED_BORDER);
        w.active.fg_stroke = Stroke::new(1.0, CHECKED_TEXT);
        w.open.bg_fill = INPUT_BG;
        w.open.weak_bg_fill = INPUT_BG;
        w.open.bg_stroke = Stroke::new(1.0, ACCENT);
        w.open.fg_stroke = Stroke::new(1.0, TEXT);
        for visuals in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            visuals.corner_radius = CornerRadius::ZERO;
            visuals.expansion = 0.0;
        }
    });
}
