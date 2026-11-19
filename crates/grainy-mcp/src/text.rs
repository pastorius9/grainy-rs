//! Caption text of the contact sheets (`photo_images._font`, `ImageDraw.textlength`, `ImageDraw.text` with the
//! anchors `lm` and `mm`), with the pure-Rust rasteriser `ab_glyph`.
//!
//! The oracle draws with Pillow's FreeType (hinted glyphs, basic layout). This module loads the same font file
//! (Malgun Gothic on Windows, else DejaVu Sans), lays text out as Pillow's basic layout does (advances rounded to
//! whole pixels as hinting leaves them, the size metrics rounded outwards) and blends coverage
//! with Pillow's formula. Glyph shapes are unhinted, so caption pixels differ from the oracle's (antialiasing) and a
//! text's width can be 1 px off Pillow's hinted width (see the font golden test), which can move the clipping of a
//! caption at the cell edge by one character.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font as _, FontVec, GlyphId, PxScale, ScaleFont};

use crate::pillow::{blend, Rgb};

/// The font files `photo_images._font` tries, in order (Pillow searches its font folders for a bare name).
pub const CANDIDATES: [&str; 2] = ["C:/Windows/Fonts/malgun.ttf", "DejaVuSans.ttf"];

fn font_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(windir) = std::env::var_os("WINDIR") {
        out.push(PathBuf::from(windir).join("Fonts"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        out.push(PathBuf::from(&home).join(".local/share/fonts"));
        out.push(PathBuf::from(&home).join(".fonts"));
        out.push(PathBuf::from(&home).join("Library/Fonts"));
    }
    out.extend(
        ["/usr/share/fonts", "/usr/local/share/fonts", "/Library/Fonts", "/System/Library/Fonts"].map(PathBuf::from),
    );
    out
}

fn find(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    for folder in font_folders() {
        let mut pending = vec![folder];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    pending.push(p);
                } else if p.file_name().is_some_and(|n| n == name) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn loaded() -> Option<&'static (PathBuf, FontVec)> {
    static FONT: OnceLock<Option<(PathBuf, FontVec)>> = OnceLock::new();
    FONT.get_or_init(|| {
        for name in CANDIDATES {
            if let Some(path) = find(name) {
                if let Ok(data) = std::fs::read(&path) {
                    if let Ok(font) = FontVec::try_from_vec(data) {
                        return Some((path, font));
                    }
                }
            }
        }
        None
    })
    .as_ref()
}

/// The font file in use, if any (`None`: no caption text is drawn; Pillow would fall back to its own font).
pub fn font_path() -> Option<&'static Path> {
    loaded().map(|(p, _)| p.as_path())
}

/// One font size (`ImageFont.truetype(name, size)`).
pub struct Font {
    font: &'static FontVec,
    scale: PxScale,
    size: f32,
}

impl Font {
    /// `photo_images._font(size)`.
    pub fn new(size: u32) -> Option<Self> {
        let (_, font) = loaded()?;
        let size = size as f32;
        // Pillow sets the size in pixels per em; ab_glyph scales by ascent - descent.
        let upem = font.units_per_em().unwrap_or(1000.0);
        let scale = PxScale::from(size * font.height_unscaled() / upem);
        Some(Self { font, scale, size })
    }

    fn per_unit(&self) -> f32 {
        self.size / self.font.units_per_em().unwrap_or(1000.0)
    }

    /// `font.getmetrics()`: (ascent, descent) in whole pixels, rounded outwards as FreeType's size metrics.
    pub fn metrics(&self) -> (i32, i32) {
        let k = self.per_unit();
        ((self.font.ascent_unscaled() * k).ceil() as i32, (-self.font.descent_unscaled() * k).ceil() as i32)
    }

    fn glyphs(&self, text: &str) -> Vec<(GlyphId, f32)> {
        let scaled = self.font.as_scaled(self.scale);
        let mut out = Vec::new();
        let mut pen = 0.0f32;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            out.push((id, pen));
            pen += scaled.h_advance(id).round();
        }
        out.push((GlyphId(0), pen));
        out
    }

    /// `draw.textlength(text, font)`: the advance of the text in pixels.
    pub fn length(&self, text: &str) -> f64 {
        f64::from(self.glyphs(text).last().map_or(0.0, |(_, pen)| *pen))
    }

    /// `draw.text((x, y), text, fill, font, anchor)` with anchor `lm` (`middle == false`... left) or `mm`.
    pub fn draw(&self, image: &mut Rgb, x: f64, y: f64, text: &str, fill: [u8; 3], centred: bool) {
        let (ascent, descent) = self.metrics();
        let laid = self.glyphs(text);
        let width = f64::from(laid.last().map_or(0.0, |(_, pen)| *pen));
        let left = (x.trunc() + if centred { -(width / 2.0).trunc() } else { 0.0 }) as f32;
        // Anchor 'm' vertically: halfway between the ascender and the descender.
        let baseline = (y.trunc() + f64::from(ascent - (ascent + descent) / 2)) as f32;
        let (h, w, _) = image.dim();
        for (id, pen) in &laid[..laid.len().saturating_sub(1)] {
            let glyph = id.with_scale_and_position(self.scale, ab_glyph::point(left + pen, baseline));
            let Some(outline) = self.font.outline_glyph(glyph) else { continue };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i64 + i64::from(gx);
                let py = bounds.min.y as i64 + i64::from(gy);
                if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                    return;
                }
                let mask = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                for (c, ink) in fill.iter().enumerate() {
                    let at = [py as usize, px as usize, c];
                    image[at] = blend(image[at], *ink, mask);
                }
            });
        }
    }
}
