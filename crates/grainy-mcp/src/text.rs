//! Caption text of the contact sheets (`photo_images._font`, `ImageDraw.textlength`, `ImageDraw.text` with the
//! anchors `lm` and `mm`), with the pure-Rust rasteriser `ab_glyph`.
//!
//! The oracle draws with Pillow's FreeType (hinted glyphs, basic layout). This module loads the same font as the
//! oracle: Malgun Gothic (`C:/Windows/Fonts/malgun.ttf`), else `DejaVuSans.ttf` found where Pillow's `truetype`
//! looks for a bare name on this system ([`font_folders`]), else Pillow's own font (`ImageFont.load_default`, a
//! subset of Aileron Regular that Pillow embeds; [`BUILTIN`]) — the last is what the oracle draws with on macOS.
//! Text is laid out as Pillow's basic layout does (advances rounded to whole pixels as hinting leaves them, the size
//! metrics rounded outwards) and coverage is blended with Pillow's formula. Glyph shapes are unhinted, so caption
//! pixels differ from the oracle's (antialiasing) and a text's width can be 1 px off Pillow's hinted width (see the
//! font golden tests), which can move the clipping of a caption at the cell edge by one character.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font as _, FontVec, GlyphId, PxScale, ScaleFont};

use crate::pillow::{blend, Rgb};

/// The font files `photo_images._font` tries, in order (Pillow searches its font folders for a bare name).
pub const CANDIDATES: [&str; 2] = ["C:/Windows/Fonts/malgun.ttf", "DejaVuSans.ttf"];

/// The font of `ImageFont.load_default(size)` (Pillow 12.3.0, basic layout), the oracle's caption font when no
/// candidate is found: Aileron Regular (subset), "No Rights Reserved"; see `assets/PILLOW-DEFAULT-FONT-NOTICE.txt`.
pub const BUILTIN: &[u8] = include_bytes!("../assets/pillow-default-font.ttf");

/// Pillow's advances of [`BUILTIN`]'s characters at the caption sizes: FreeType hints this font automatically, which
/// moves advances by whole pixels, and Pillow's basic layout adds them up (no kerning). Made by
/// tools/golden/gen_mcp_builtin_font.py from Pillow itself.
const BUILTIN_ADVANCES: &str = include_str!("../assets/pillow-default-font-advances.json");

/// The advance table of one size: each character's advance and the one of a character the font lacks.
struct Advances {
    of: HashMap<char, f32>,
    missing: f32,
}

/// The table of `size`, when it is one of the caption sizes.
fn builtin_advances(size: u32) -> Option<&'static Advances> {
    static TABLES: OnceLock<HashMap<u32, Advances>> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(BUILTIN_ADVANCES) else { return HashMap::new() };
        let chars: Vec<char> = json["chars"].as_str().unwrap_or_default().chars().collect();
        let mut tables = HashMap::new();
        for (size, list) in json["advances"].as_object().into_iter().flatten() {
            let list: Vec<f32> =
                list.as_array().into_iter().flatten().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
            let (Ok(size), Some(&missing)) = (size.parse::<u32>(), list.last()) else { continue };
            if list.len() == chars.len() + 1 {
                tables.insert(size, Advances { of: chars.iter().copied().zip(list).collect(), missing });
            }
        }
        tables
    });
    tables.get(&size)
}

/// The system, as far as Pillow's font search is concerned (`sys.platform`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSystem {
    /// `win32`.
    Windows,
    /// `darwin`.
    MacOs,
    /// `linux`.
    Linux,
    /// Anything else: Pillow searches no folder.
    Other,
}

impl FontSystem {
    /// This computer.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

/// The folders Pillow's `truetype` walks for a bare font name, in order, with `var` reading the environment:
/// Windows `%WINDIR%/fonts`; Linux `$XDG_DATA_HOME` (or `~/.local/share`) and each of `$XDG_DATA_DIRS` (or
/// `/usr/local/share:/usr/share`), each with `fonts`; macOS `/Library/Fonts`, `/System/Library/Fonts`,
/// `~/Library/Fonts`.
pub fn font_folders(system: FontSystem, var: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    let home = || set("HOME").map(PathBuf::from);
    match system {
        FontSystem::Windows => set("WINDIR").map(|windir| PathBuf::from(windir).join("fonts")).into_iter().collect(),
        FontSystem::Linux => {
            let data_home = set("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home().map(|h| h.join(".local").join("share")))
                .unwrap_or_else(|| PathBuf::from("~/.local/share"));
            let data_dirs = set("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
            std::iter::once(data_home)
                .chain(data_dirs.split(':').map(PathBuf::from))
                .map(|folder| folder.join("fonts"))
                .collect()
        }
        FontSystem::MacOs => {
            let mut folders = vec![PathBuf::from("/Library/Fonts"), PathBuf::from("/System/Library/Fonts")];
            folders.push(home().map_or_else(|| PathBuf::from("~/Library/Fonts"), |h| h.join("Library").join("Fonts")));
            folders
        }
        FontSystem::Other => Vec::new(),
    }
}

/// `os.walk(folder)` from the top, looking for a file called `name`: each folder's files first, then its subfolders
/// in listing order; links to folders are not followed.
fn walk(folder: &Path, name: &str) -> Option<PathBuf> {
    let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(folder).ok()?.flatten().collect();
    let mut folders = Vec::new();
    for entry in &entries {
        let path = entry.path();
        if path.is_dir() {
            if !entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                folders.push(path);
            }
        } else if entry.file_name() == name {
            return Some(path);
        }
    }
    folders.iter().find_map(|sub| walk(sub, name))
}

/// Port of `ImageFont.truetype(name)`'s search: the name as given (absolute, or relative to the working folder),
/// else the first file with its base name (`os.path.basename`: `malgun.ttf` of `C:/Windows/Fonts/malgun.ttf` too)
/// in [`font_folders`] of this system.
fn find(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let base = path.file_name()?.to_str()?;
    let var = |key: &str| std::env::var(key).ok();
    font_folders(FontSystem::current(), &var).iter().find_map(|folder| walk(folder, base))
}

fn loaded() -> Option<&'static (Option<PathBuf>, FontVec)> {
    static FONT: OnceLock<Option<(Option<PathBuf>, FontVec)>> = OnceLock::new();
    FONT.get_or_init(|| {
        for name in CANDIDATES {
            if let Some(path) = find(name) {
                if let Ok(data) = std::fs::read(&path) {
                    if let Ok(font) = FontVec::try_from_vec(data) {
                        return Some((Some(path), font));
                    }
                }
            }
        }
        // `ImageFont.load_default(size=size)`.
        FontVec::try_from_vec(BUILTIN.to_vec()).ok().map(|font| (None, font))
    })
    .as_ref()
}

fn builtin() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| FontVec::try_from_vec(BUILTIN.to_vec()).ok()).as_ref()
}

/// The font file in use, `None` when the captions use Pillow's built-in font ([`uses_builtin`]).
pub fn font_path() -> Option<&'static Path> {
    loaded().and_then(|(p, _)| p.as_deref())
}

/// Whether the captions use Pillow's built-in font (no candidate file on this system: macOS, usually).
pub fn uses_builtin() -> bool {
    loaded().is_some_and(|(p, _)| p.is_none())
}

/// One font size (`ImageFont.truetype(name, size)`).
pub struct Font {
    font: &'static FontVec,
    /// Pillow's hinted advances (the built-in font at a caption size); `None`: the font's own, rounded.
    advances: Option<&'static Advances>,
    scale: PxScale,
    size: f32,
}

impl Font {
    /// `photo_images._font(size)`.
    pub fn new(size: u32) -> Option<Self> {
        let (path, font) = loaded()?;
        let advances = if path.is_none() { builtin_advances(size) } else { None };
        Some(Self { advances, ..Self::sized(font, size) })
    }

    /// `ImageFont.load_default(size)`: Pillow's built-in font, whatever this system has.
    pub fn builtin(size: u32) -> Option<Self> {
        Some(Self { advances: builtin_advances(size), ..Self::sized(builtin()?, size) })
    }

    fn sized(font: &'static FontVec, size: u32) -> Self {
        let size = size as f32;
        // Pillow sets the size in pixels per em; ab_glyph scales by ascent - descent.
        let upem = font.units_per_em().unwrap_or(1000.0);
        let scale = PxScale::from(size * font.height_unscaled() / upem);
        Self { font, advances: None, scale, size }
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
            pen += match self.advances {
                Some(table) => table.of.get(&ch).copied().unwrap_or(table.missing),
                None => scaled.h_advance(id).round(),
            };
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
