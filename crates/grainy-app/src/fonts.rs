//! The interface font: a Korean system font loaded at start (nothing is bundled).
//!
//! The oracle loads `malgun.ttf` / `malgunbd.ttf` from the Windows font folder and uses Apple SD Gothic Neo on
//! macOS (`UI_FONT` and `configure_application` in luma/app.py). The same files are read here, in this order,
//! and the first one found becomes the interface font; egui's built-in fonts stay behind it as a fallback for
//! symbols (arrows, triangles).
//!
//! * Windows: `%WINDIR%\Fonts\malgun.ttf` (regular) and `malgunbd.ttf` (bold), Malgun Gothic.
//! * macOS: `/System/Library/Fonts/AppleSDGothicNeo.ttc`, the upright face of weight 400 (regular) and for bold the
//!   one of weight 600 (SemiBold, what Qt matches `font-weight:600` to), else 700; the faces are told apart by their
//!   `OS/2` weight, not by their order in the collection ([`face_weights`]). Then
//!   `/System/Library/Fonts/Supplemental/AppleGothic.ttf` (regular only: bold text uses it as well).
//! * Linux: Noto Sans CJK (`NotoSansCJK-Regular.ttc` / `-Bold.ttc` in the usual folders), then Nanum Gothic.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Name of the bold family (`font-weight:600` in the stylesheet).
pub const BOLD: &str = "bold";

/// A font file found on this system.
#[derive(Clone)]
pub struct SystemFont {
    /// Where it was read from.
    pub path: PathBuf,
    /// The face inside a collection (`.ttc`).
    pub index: u32,
    bytes: Arc<Vec<u8>>,
}

/// The fonts found: regular and bold (either can be missing), and a symbol font behind them (the marks Qt finds
/// by font fallback: ✓ × ★ of the photo captions).
#[derive(Clone, Default)]
pub struct Loaded {
    pub regular: Option<SystemFont>,
    pub bold: Option<SystemFont>,
    pub symbols: Option<SystemFont>,
}

impl Loaded {
    /// One line for the log: which files are used.
    pub fn describe(&self) -> String {
        let name = |f: &Option<SystemFont>| match f {
            Some(font) => format!("{} (face {})", font.path.display(), font.index),
            None => "none".to_owned(),
        };
        format!("interface font: regular {}, bold {}", name(&self.regular), name(&self.bold))
    }
}

/// Which face of a font file.
#[derive(Debug, Clone, Copy)]
enum Face {
    /// The face at this index.
    Index(u32),
    /// The first upright face with the first of these `OS/2` weights that the file has.
    Weight(&'static [u16]),
}

type Candidates = Vec<(PathBuf, Face)>;

/// The faces of a font file or collection (`.ttc`): (index, `OS/2` `usWeightClass`, italic), read from the tables
/// directly; a face without an `OS/2` table is left out, an unreadable file has none.
pub fn face_weights(bytes: &[u8]) -> Vec<(u32, u16, bool)> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let starts: Vec<usize> = if bytes.starts_with(b"ttcf") {
        let count = u32_at(8).unwrap_or(0).min(1024);
        (0..count).filter_map(|i| u32_at(12 + 4 * i as usize).map(|o| o as usize)).collect()
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    for (index, start) in starts.into_iter().enumerate() {
        let tables = u16_at(start + 4).unwrap_or(0) as usize;
        let os2 = (0..tables)
            .map(|t| start + 12 + 16 * t)
            .find(|&record| bytes.get(record..record + 4) == Some(b"OS/2".as_slice()))
            .and_then(|record| u32_at(record + 8));
        let Some(os2) = os2.map(|o| o as usize) else { continue };
        if let (Some(weight), Some(selection)) = (u16_at(os2 + 4), u16_at(os2 + 62)) {
            faces.push((index as u32, weight, selection & 1 != 0));
        }
    }
    faces
}

/// The first upright face with the first of `weights` that `bytes` has.
fn face_with_weight(bytes: &[u8], weights: &[u16]) -> Option<u32> {
    let faces = face_weights(bytes);
    weights.iter().find_map(|w| faces.iter().find(|(_, weight, italic)| weight == w && !italic).map(|(i, _, _)| *i))
}

/// Symbol fonts: Segoe UI Symbol (Windows), Apple Symbols (macOS), DejaVu Sans (Linux).
fn symbol_candidates() -> Candidates {
    if cfg!(target_os = "windows") {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        vec![(windir.join("Fonts").join("seguisym.ttf"), Face::Index(0))]
    } else if cfg!(target_os = "macos") {
        vec![(PathBuf::from("/System/Library/Fonts/Apple Symbols.ttf"), Face::Index(0))]
    } else {
        vec![(PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"), Face::Index(0))]
    }
}

fn candidates() -> (Candidates, Candidates) {
    let mut regular = Vec::new();
    let mut bold = Vec::new();
    if cfg!(target_os = "windows") {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let fonts = windir.join("Fonts");
        regular.push((fonts.join("malgun.ttf"), Face::Index(0)));
        bold.push((fonts.join("malgunbd.ttf"), Face::Index(0)));
    } else if cfg!(target_os = "macos") {
        let neo = PathBuf::from("/System/Library/Fonts/AppleSDGothicNeo.ttc");
        regular.push((neo.clone(), Face::Weight(&[400])));
        regular.push((PathBuf::from("/System/Library/Fonts/Supplemental/AppleGothic.ttf"), Face::Index(0)));
        bold.push((neo, Face::Weight(&[600, 700])));
    } else {
        for folder in
            ["/usr/share/fonts/opentype/noto", "/usr/share/fonts/noto-cjk", "/usr/share/fonts/google-noto-cjk"]
        {
            regular.push((PathBuf::from(folder).join("NotoSansCJK-Regular.ttc"), Face::Index(1)));
            bold.push((PathBuf::from(folder).join("NotoSansCJK-Bold.ttc"), Face::Index(1)));
        }
        regular.push((PathBuf::from("/usr/share/fonts/truetype/nanum/NanumGothic.ttf"), Face::Index(0)));
        bold.push((PathBuf::from("/usr/share/fonts/truetype/nanum/NanumGothicBold.ttf"), Face::Index(0)));
    }
    (regular, bold)
}

fn first(list: &[(PathBuf, Face)]) -> Option<SystemFont> {
    list.iter().find_map(|(path, face)| {
        let bytes = std::fs::read(path).ok()?;
        let index = match face {
            Face::Index(index) => *index,
            Face::Weight(weights) => face_with_weight(&bytes, weights)?,
        };
        Some(SystemFont { path: path.clone(), index, bytes: Arc::new(bytes) })
    })
}

/// The system fonts, read once per process.
pub fn system() -> &'static Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    LOADED.get_or_init(|| {
        let (regular, bold) = candidates();
        Loaded { regular: first(&regular), bold: first(&bold), symbols: first(&symbol_candidates()) }
    })
}

fn data(font: &SystemFont) -> Arc<FontData> {
    let blob: egui::epaint::text::Blob = font.bytes.clone();
    Arc::new(FontData::from_blob(blob, font.index))
}

/// Installs the interface font in `ctx`. Returns what was loaded.
pub fn install(ctx: &egui::Context) -> Loaded {
    let loaded = system().clone();
    let mut defs = FontDefinitions::default();
    let builtin_proportional = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut proportional = Vec::new();
    let mut bold = Vec::new();
    if let Some(font) = &loaded.regular {
        defs.font_data.insert("ui".into(), data(font));
        proportional.push("ui".to_owned());
    }
    if let Some(font) = &loaded.bold {
        defs.font_data.insert("ui-bold".into(), data(font));
        bold.push("ui-bold".to_owned());
    }
    if let Some(font) = &loaded.symbols {
        defs.font_data.insert("symbols".into(), data(font));
        proportional.push("symbols".to_owned());
    }
    // Bold falls back to the regular face, then to egui's fonts; regular falls back to egui's fonts.
    bold.extend(proportional.iter().cloned());
    proportional.extend(builtin_proportional.iter().cloned());
    bold.extend(builtin_proportional.iter().cloned());
    defs.families.insert(FontFamily::Proportional, proportional);
    defs.families.insert(FontFamily::Name(BOLD.into()), bold);
    if loaded.regular.is_some() {
        // Korean in monospace text (the debug overlay) comes from the interface font.
        defs.families.entry(FontFamily::Monospace).or_default().push("ui".to_owned());
    }
    ctx.set_fonts(defs);
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font collection of `faces` (weight, italic), each with nothing but an `OS/2` table.
    fn collection(faces: &[(u16, bool)]) -> Vec<u8> {
        let mut out = b"ttcf".to_vec();
        out.extend(0x0001_0000u32.to_be_bytes());
        out.extend((faces.len() as u32).to_be_bytes());
        let header = 12 + 4 * faces.len();
        let font = 12 + 16 + 64; // table directory, one record, the OS/2 table
        for i in 0..faces.len() {
            out.extend(((header + i * font) as u32).to_be_bytes());
        }
        for (i, (weight, italic)) in faces.iter().enumerate() {
            let start = header + i * font;
            out.extend(0x0001_0000u32.to_be_bytes());
            out.extend(1u16.to_be_bytes());
            out.extend([0u8; 6]);
            out.extend(b"OS/2");
            out.extend(0u32.to_be_bytes());
            out.extend(((start + 28) as u32).to_be_bytes());
            out.extend(64u32.to_be_bytes());
            let mut os2 = [0u8; 64];
            os2[4..6].copy_from_slice(&weight.to_be_bytes());
            os2[62..64].copy_from_slice(&u16::from(*italic).to_be_bytes());
            out.extend(os2);
        }
        out
    }

    #[test]
    fn faces_are_chosen_by_weight_not_by_order() {
        // As a system collection may be ordered: thin, bold, regular, semibold, an italic regular.
        let bytes = collection(&[(100, false), (700, false), (400, true), (400, false), (600, false)]);
        assert_eq!(
            face_weights(&bytes),
            vec![(0, 100, false), (1, 700, false), (2, 400, true), (3, 400, false), (4, 600, false)]
        );
        assert_eq!(face_with_weight(&bytes, &[400]), Some(3), "the upright regular face");
        assert_eq!(face_with_weight(&bytes, &[600, 700]), Some(4), "SemiBold first");
        let no_semibold = collection(&[(400, false), (700, false)]);
        assert_eq!(face_with_weight(&no_semibold, &[600, 700]), Some(1), "else Bold");
        assert_eq!(face_with_weight(&no_semibold, &[900]), None);
        assert_eq!(face_weights(b"not a font"), Vec::new());
        assert_eq!(face_weights(&[]), Vec::new());
        // Cut short anywhere: no face, no panic.
        for end in 0..bytes.len() {
            let _ = face_weights(&bytes[..end]);
        }
    }

    /// The real files of this system, when they are there: Malgun Gothic on Windows, Apple SD Gothic Neo on macOS.
    #[test]
    fn the_systems_korean_font_has_its_weights() {
        let (regular, bold) = candidates();
        if let Some(font) = first(&regular) {
            let faces = face_weights(&font.bytes);
            assert!(faces.iter().any(|&(i, w, _)| i == font.index && w == 400), "{}: {faces:?}", font.path.display());
        }
        if let Some(font) = first(&bold) {
            let faces = face_weights(&font.bytes);
            assert!(faces.iter().any(|&(i, w, _)| i == font.index && w >= 600), "{}: {faces:?}", font.path.display());
        }
    }
}
