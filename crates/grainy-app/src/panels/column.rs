//! The right column of the Develop screen: the histogram with its clipping buttons, the photo's line, the tool
//! strip and the accordion of panels (port of luma/develop_panels.py `DevelopPanels` and the histogram of
//! luma/widgets.py). The panels themselves are the registered [`Panel`]s.

use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, Panel as EguiPanel, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use grainy_i18n::{tr, tr_with};

use super::controls::hint;
use super::{Panel, Tool, PANELS, TOOLS, TOOL_STRIP};
use crate::context::AppContext;
use crate::theme as t;
use crate::widgets::{self, Button};

/// Draws the column.
pub fn develop_column(ui: &mut Ui, cx: &mut AppContext, panels: &mut [Box<dyn Panel>]) {
    EguiPanel::bottom("develop_footer")
        .frame(Frame::new().inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 0 }))
        .show_separator_line(false)
        .resizable(false)
        .show(ui, |ui| {
            let width = ui.available_width();
            let labels = [tr("동기화"), tr("초기화")];
            let row = [Button::new(&labels[0]), Button::new(&labels[1])];
            match widgets::button_row(ui, &row, width, 4.0) {
                Some(0) => {
                    cx.run("sync.selected");
                }
                Some(_) => {
                    cx.run("develop.reset");
                }
                None => {}
            }
        });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(5.0, 6.0);
        widgets::section(ui, "HISTOGRAM");
        histogram(ui, cx);
        let info = match cx.doc.current() {
            Some(photo) => match photo.original {
                Some((w, h)) => format!("{}  ·  {w} × {h}", photo.name),
                None => photo.name.clone(),
            },
            None if cx.picture_size.x > 0.0 => {
                format!("{} × {}", cx.picture_size.x as u32, cx.picture_size.y as u32)
            }
            None => tr("사진 정보"),
        };
        widgets::label(ui, &info, t::MUTED, t::FONT_SIZE);
        tool_strip(ui, cx);
        egui::ScrollArea::vertical().id_salt("develop_panels").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Room for the scroll bar (`column.setContentsMargins(0,0,5,0)`).
            let width = ui.available_width() - 5.0;
            ui.set_max_width(width);
            for (key, title) in PANELS {
                if TOOLS.contains(&key) && cx.tool.panel() != Some(key) {
                    continue;
                }
                panel(ui, cx, key, title, panels);
            }
        });
    });
}

/// The histogram of the picture on screen (`Histogram.paintEvent`) with the two clipping buttons.
fn histogram(ui: &mut Ui, cx: &mut AppContext) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 104.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, t::HISTOGRAM_BG);
    if let Some(bins) = &cx.bins {
        let high = bins.iter().map(crate::histogram::percentile98).fold(0.0f32, f32::max);
        let high = if high > 0.0 { high } else { 1.0 };
        let colors = [(0xda, 0x86, 0x6b), (0x88, 0xbc, 0xa1), (0x7b, 0xaa, 0xd1)];
        for (channel, (r, g, b)) in bins.iter().zip(colors) {
            let mut points = vec![Pos2::new(rect.left(), rect.bottom())];
            for (i, v) in channel.iter().enumerate() {
                let x = rect.left() + i as f32 / 127.0 * rect.width();
                let y = rect.bottom() - (*v as f32 / high).min(1.0) * (rect.height() - 25.0);
                points.push(Pos2::new(x, y));
            }
            points.push(Pos2::new(rect.right(), rect.bottom()));
            let line = Color32::from_rgb(r, g, b);
            // Filled area: vertical strips (the outline is not convex).
            for pair in points[1..points.len() - 1].windows(2) {
                let quad =
                    vec![pair[0], pair[1], Pos2::new(pair[1].x, rect.bottom()), Pos2::new(pair[0].x, rect.bottom())];
                painter.add(egui::Shape::convex_polygon(
                    quad,
                    Color32::from_rgba_unmultiplied(r, g, b, 85),
                    Stroke::NONE,
                ));
            }
            painter.add(egui::Shape::line(points, Stroke::new(0.6, line)));
        }
    }
    let hdr = cx.frame.as_ref().and_then(|f| f.hdr);
    if let Some(info) = &hdr {
        crate::hdr::paint_histogram(&painter, rect, info);
    }
    for (index, active) in [(0, cx.clip_shadows), (1, cx.clip_highlights)] {
        let min =
            if index == 0 { rect.min + Vec2::new(3.0, 3.0) } else { Pos2::new(rect.right() - 25.0, rect.top() + 3.0) };
        let button = Rect::from_min_size(min, Vec2::new(22.0, 19.0));
        let response = ui.interact(button, egui::Id::unique(("clip", index)), Sense::click());
        let fill = if response.hovered() { t::BUTTON_HOVER } else { t::BUTTON };
        painter.rect(button, CornerRadius::ZERO, fill, Stroke::new(1.0, t::BUTTON_BORDER), StrokeKind::Inside);
        // `refresh_indicators`: the clipped channels' colour, grey when none.
        let channels = cx.frame.as_ref().map(|f| if index == 0 { f.channels.0 } else { f.channels.1 });
        let color = match channels.filter(|c| c.iter().any(|v| *v)) {
            Some(c) => {
                Color32::from_rgb(if c[0] { 255 } else { 0 }, if c[1] { 255 } else { 0 }, if c[2] { 255 } else { 0 })
            }
            None => Color32::from_rgb(0x59, 0x63, 0x6f),
        };
        let hdr_indicator = hdr.filter(|_| index == 1).map(|info| crate::hdr::indicator(&info));
        let color = hdr_indicator.as_ref().map_or(color, |(c, _)| *c);
        if active {
            painter.rect_stroke(
                button,
                CornerRadius::ZERO,
                Stroke::new(1.0, Color32::from_rgb(0xf4, 0xe7, 0xd4)),
                StrokeKind::Inside,
            );
        }
        let c = button.center();
        painter.add(egui::Shape::convex_polygon(
            vec![c + Vec2::new(0.0, -4.0), c + Vec2::new(5.0, 4.0), c + Vec2::new(-5.0, 4.0)],
            color,
            Stroke::NONE,
        ));
        let name = match hdr_indicator {
            Some((_, title)) => title,
            None if index == 0 => tr("검정 클리핑"),
            None => tr("흰색 클리핑"),
        };
        if response.on_hover_text(name).clicked() {
            if index == 0 {
                cx.clip_shadows = !cx.clip_shadows;
            } else {
                cx.clip_highlights = !cx.clip_highlights;
            }
        }
    }
}

/// The tool strip: 보정 / 크롭 / 복구 / 마스크 (`developTool` buttons, equal widths).
fn tool_strip(ui: &mut Ui, cx: &mut AppContext) {
    ui.add_space(4.0);
    let width = ui.available_width();
    let gap = 3.0;
    let each = (width - gap * 3.0) / 4.0;
    let (strip, _) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
    for (i, (tool, title)) in TOOL_STRIP.iter().enumerate() {
        let rect = Rect::from_min_size(strip.min + Vec2::new(i as f32 * (each + gap), 0.0), Vec2::new(each, 36.0));
        let response = ui.interact(rect, egui::Id::unique(("tool", i)), Sense::click());
        let checked = cx.tool == *tool;
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, CornerRadius::ZERO, t::PANEL_HEADER_HOVER);
        }
        if checked {
            let line = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 2.0), rect.max);
            painter.rect_filled(line, CornerRadius::ZERO, t::ACCENT);
        }
        let color = if checked { t::TOOL_CHECKED } else { t::TOOL_TEXT };
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, tr(title), t::font(t::FONT_SIZE), color);
        // Healing and masks: their dot in the strip, since their panel shows only while the tool is open.
        if let Some(panel) = tool.panel().filter(|p| super::has_switch(p)) {
            if super::edited(cx.settings(), panel) {
                let off = !super::is_on(cx.settings(), panel);
                widgets::paint_dot(ui, Pos2::new(rect.right() - 8.0, rect.top() + 8.0), off);
            }
        }
        let response = match tool {
            Tool::Healing => response.on_hover_text(format!("{} ({})", tr(title), hint("Q"))),
            Tool::Masks => response.on_hover_text(tr_with!(
                "마스크 · 새 마스크: 브러시 {0} · 선형 {1} · 방사형 {2}",
                hint("K"),
                hint("M"),
                hint("Shift+M")
            )),
            _ => response,
        };
        if response.clicked() {
            cx.select_tool(*tool);
        }
    }
    ui.add_space(7.0);
}

/// One panel: the header (arrow, title, switch, edited dot, context menu) and, when expanded, its body.
fn panel(ui: &mut Ui, cx: &mut AppContext, key: &'static str, title: &str, panels: &mut [Box<dyn Panel>]) {
    let expanded = if TOOLS.contains(&key) { true } else { cx.expanded.contains(&key) };
    let settings = cx.settings();
    let switch = super::has_switch(key);
    let on = !switch || super::is_on(settings, key);
    let edited = switch && super::edited(settings, key);
    let width = ui.available_width();
    let title_text = tr(title);
    let galley = widgets::galley(ui, title_text.clone(), t::bold(t::FONT_SIZE), t::PANEL_HEADER_TEXT);
    let height = galley.size().y + 20.0 + 1.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let switch_rect =
        Rect::from_min_size(Pos2::new(rect.right() - 9.0 - 22.0, rect.center().y - 6.0), Vec2::new(22.0, 12.0));
    let switch_response = switch.then(|| {
        ui.interact(switch_rect.expand(3.0), egui::Id::unique(("panel_switch", key)), Sense::click())
            .on_hover_text(tr("이 패널의 보정을 켜고 끕니다. 꺼도 값은 그대로 남습니다."))
    });
    let switch_hover = switch_response.as_ref().is_some_and(|r| r.hovered());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::ZERO,
        if response.hovered() && !switch_hover { t::PANEL_HEADER_HOVER } else { t::PANEL_HEADER },
    );
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, t::PANEL_HEADER_LINE));
    let text_color = if on { t::PANEL_HEADER_TEXT } else { t::PANEL_OFF_TEXT };
    widgets::arrow(ui, Pos2::new(rect.left() + 16.0, rect.center().y + 0.5), expanded, text_color);
    let text_galley = widgets::galley(ui, title_text.clone(), t::bold(t::FONT_SIZE), text_color);
    ui.painter().galley(
        Pos2::new(rect.left() + 28.0, rect.center().y - text_galley.size().y / 2.0 + 0.5),
        text_galley,
        text_color,
    );
    if switch {
        widgets::paint_switch(ui, switch_rect, on, switch_hover);
        if edited {
            let dot = Pos2::new(switch_rect.left() - 9.0 - 3.0, rect.center().y);
            widgets::paint_dot(ui, dot, !on);
            let dot_rect = Rect::from_center_size(dot, Vec2::splat(8.0));
            ui.interact(dot_rect, egui::Id::unique(("panel_dot", key)), Sense::hover())
                .on_hover_text(tr("기본값과 다른 보정이 있습니다"));
        }
    }
    if let Some(r) = &switch_response {
        if r.clicked() {
            cx.doc.edit_mut().switch_panel(key, !on);
        }
    }
    let switch_clicked = switch_response.as_ref().is_some_and(|r| r.clicked());
    if response.clicked() && !switch_clicked {
        cx.toggle_panel(key, !expanded);
    }
    response.context_menu(|ui| {
        let mut solo = cx.solo;
        if ui.add(egui::Button::new(tr("단일 패널 모드")).selected(solo)).clicked() {
            solo = !solo;
            cx.solo = solo;
            if solo {
                cx.expanded.truncate(1);
            }
            ui.close();
        }
        if ui.button(tr("모든 패널 접기")).clicked() {
            cx.expanded.clear();
            ui.close();
        }
    });
    if expanded {
        Frame::new().fill(t::PANEL_BODY).inner_margin(Margin { left: 8, right: 8, top: 9, bottom: 12 }).show(
            ui,
            |ui| {
                ui.set_width(width - 16.0);
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                match panels.iter_mut().find(|p| p.key() == key) {
                    Some(panel) => panel.ui(ui, cx),
                    None => cx.sliders(ui, crate::adjustments::Place::Panel(key)),
                }
            },
        );
    }
}
