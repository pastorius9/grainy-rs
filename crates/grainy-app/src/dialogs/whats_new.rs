//! 이번 버전의 달라진 점… and the box shown once after an update (luma/whats_new.py `show`): the change log of the
//! version (or of the versions since the one opened before) as formatted text, with the thanks for feedback sent
//! from this computer above it. The notes come from `grainy_platform::whats_new`.

use eframe::egui::{self, text::LayoutJob, Color32, FontFamily, FontId, TextFormat};
use grainy_platform::whats_new::{self, Entry};
use grainy_platform::APP_VERSION;

use super::Dialog;
use crate::commands::settings::notes_root;
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// The box.
pub struct WhatsNew {
    entries: Vec<Entry>,
    refs: Vec<String>,
}

/// What the box says (`windowTitle`, the heading label, the thanks label, the text given to the text view).
#[derive(Debug, Clone, PartialEq)]
pub struct Texts {
    pub title: String,
    pub heading: String,
    pub thanks: Option<String>,
    pub markdown: String,
}

impl WhatsNew {
    /// A box for `entries`, with the thanks for the feedback references `refs`.
    pub fn new(entries: Vec<Entry>, refs: Vec<String>) -> Self {
        Self { entries, refs }
    }

    /// 설정 → 이번 버전의 달라진 점…: `show(parent)` without entries shows this version's own notes.
    pub fn this_version() -> Self {
        Self::new(whats_new::notes(None, APP_VERSION, None, &notes_root()), Vec::new())
    }

    /// The texts of the box in the interface language.
    pub fn texts(&self) -> Texts {
        Texts {
            title: tr_with("Grainy {0}의 달라진 점", &[APP_VERSION]),
            heading: tr_with("Grainy가 {0} 버전으로 업데이트되었습니다.", &[APP_VERSION]),
            thanks: (!self.refs.is_empty()).then(|| whats_new::thanks(&self.refs, grainy_i18n::language())),
            markdown: if self.entries.is_empty() {
                tr("이 버전의 변경 기록을 찾지 못했습니다.")
            } else {
                whats_new::markdown(&self.entries)
            },
        }
    }
}

// ---- the formatted text -------------------------------------------------------------------------------------------

/// A run of text inside a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub code: bool,
}

/// One block of the markdown the change logs use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `## 0.6.8 — title`: the level and the text.
    Heading(usize, Vec<Span>),
    /// A list item: its depth (two spaces per level) and its text.
    Bullet(usize, Vec<Span>),
    /// A paragraph or a numbered line.
    Paragraph(Vec<Span>),
    /// A table row (`| a | b |`), without the separator row.
    Row(Vec<String>),
}

fn plain(text: String) -> Span {
    Span { text, bold: false, code: false }
}

/// Bold (`**x**`) and code (`` `x` ``) runs of a line; a link `[text](address)` keeps its text.
pub fn spans(line: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut text = String::new();
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        let found = if let Some(inner) = rest.strip_prefix("**") {
            inner
                .find("**")
                .filter(|e| *e > 0)
                .map(|e| (Span { text: inner[..e].to_owned(), bold: true, code: false }, 4 + e))
        } else if let Some(inner) = rest.strip_prefix('`') {
            inner
                .find('`')
                .filter(|e| *e > 0)
                .map(|e| (Span { text: inner[..e].to_owned(), bold: false, code: true }, 2 + e))
        } else if let Some(inner) = rest.strip_prefix('[') {
            inner.find("](").and_then(|close| {
                inner[close + 2..].find(')').map(|end| (plain(inner[..close].to_owned()), 1 + close + 2 + end + 1))
            })
        } else {
            None
        };
        match found {
            Some((span, used)) if span.bold || span.code => {
                if !text.is_empty() {
                    out.push(plain(std::mem::take(&mut text)));
                }
                out.push(span);
                rest = &rest[used..];
            }
            Some((span, used)) => {
                text.push_str(&span.text);
                rest = &rest[used..];
            }
            None => {
                text.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !text.is_empty() {
        out.push(plain(text));
    }
    out
}

/// The blocks of a markdown text: headings, list items (a more indented line continues the item above),
/// paragraphs and table rows.
pub fn blocks(markdown: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for raw in markdown.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() {
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            out.push(Block::Heading(hashes, spans(trimmed[hashes..].trim())));
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            out.push(Block::Bullet(indent / 2, spans(item)));
        } else if trimmed.starts_with('|') {
            let cells: Vec<String> = trimmed.trim_matches('|').split('|').map(|c| c.trim().to_owned()).collect();
            if !cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
                out.push(Block::Row(cells));
            }
        } else if indent > 0 && matches!(out.last(), Some(Block::Bullet(..))) {
            if let Some(Block::Bullet(_, last)) = out.last_mut() {
                last.push(plain(" ".into()));
                last.extend(spans(trimmed));
            }
        } else {
            out.push(Block::Paragraph(spans(trimmed)));
        }
    }
    out
}

fn job(parts: &[Span], size: f32, bold_all: bool, color: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    for span in parts {
        let font = if span.code {
            FontId::new(size - 1.0, FontFamily::Monospace)
        } else if span.bold || bold_all {
            t::bold(size)
        } else {
            t::font(size)
        };
        let mut format =
            TextFormat { font_id: font, color: if span.code { t::CHECKED_TEXT } else { color }, ..Default::default() };
        if span.code {
            format.background = t::INPUT_BG;
        }
        job.append(&span.text, 0.0, format);
    }
    job
}

/// Draws the blocks in a column as wide as `ui` is.
pub fn draw(ui: &mut egui::Ui, blocks: &[Block]) {
    let width = ui.available_width();
    for block in blocks {
        match block {
            Block::Heading(level, parts) => {
                let size = match level {
                    1 => 20.0,
                    2 => 17.0,
                    _ => 14.0,
                };
                ui.add_space(if *level <= 2 { 12.0 } else { 6.0 });
                let galley = ui.painter().layout_job(job(parts, size, true, t::BRAND, width));
                let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
                ui.painter().galley(rect.min, galley, t::BRAND);
                ui.add_space(4.0);
            }
            Block::Bullet(depth, parts) => {
                let left = 14.0 + 18.0 * *depth as f32;
                let galley = ui.painter().layout_job(job(parts, t::FONT_SIZE, false, t::TEXT, width - left));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, galley.size().y + 3.0), egui::Sense::hover());
                let bullet = if *depth == 0 { "•" } else { "◦" };
                ui.painter().text(
                    rect.min + egui::vec2(left - 12.0, 0.0),
                    egui::Align2::LEFT_TOP,
                    bullet,
                    t::font(t::FONT_SIZE),
                    t::MUTED,
                );
                ui.painter().galley(rect.min + egui::vec2(left, 0.0), galley, t::TEXT);
            }
            Block::Paragraph(parts) => {
                let galley = ui.painter().layout_job(job(parts, t::FONT_SIZE, false, t::TEXT, width));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, galley.size().y + 6.0), egui::Sense::hover());
                ui.painter().galley(rect.min, galley, t::TEXT);
            }
            Block::Row(cells) => {
                widgets::label(ui, &cells.join("   |   "), t::VALUE, t::FONT_SIZE);
            }
        }
    }
}

impl Dialog for WhatsNew {
    fn id(&self) -> &'static str {
        "whats_new"
    }

    fn show(&mut self, ctx: &egui::Context, _cx: &mut AppContext) -> bool {
        let texts = self.texts();
        let parsed = blocks(&texts.markdown);
        let mut close = false;
        let response = egui::Modal::new(egui::Id::unique("whats_new_box")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.label(egui::RichText::new(&texts.title).font(t::bold(14.0)).color(t::BRAND));
            ui.add_space(8.0);
            widgets::label(ui, &texts.heading, t::TEXT, t::FONT_SIZE);
            if let Some(thanks) = &texts.thanks {
                ui.add_space(6.0);
                widgets::label(ui, thanks, t::SECTION, t::FONT_SIZE);
            }
            ui.add_space(8.0);
            egui::Frame::new().fill(t::INPUT_BG).stroke(egui::Stroke::new(1.0, t::INPUT_BORDER)).inner_margin(10).show(
                ui,
                |ui| {
                    egui::ScrollArea::vertical().max_height(380.0).id_salt("notes").show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        draw(ui, &parsed);
                    });
                },
            );
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::new(&tr("닫기")).kind(Kind::Primary).show(ui).clicked() {
                    close = true;
                }
            });
        });
        !(close || response.should_close())
    }
}
