//! The interface font: a Korean system font loaded at start (nothing is bundled).
//!
//! The oracle loads `malgun.ttf` / `malgunbd.ttf` from the Windows font folder and uses Apple SD Gothic Neo on
//! macOS (`UI_FONT` and `configure_application` in luma/app.py). The same files are read here, in this order,
//! and the first one found becomes the interface font; egui's built-in fonts stay behind it as a fallback for
//! symbols (arrows, triangles).
//!
//! * Windows: `%WINDIR%\Fonts\malgun.ttf` (regular) and `malgunbd.ttf` (bold), Malgun Gothic.
//! * macOS: `/System/Library/Fonts/AppleSDGothicNeo.ttc` (face 0 regular, the bold face is not picked out:
//!   bold text uses the regular face there), then `/System/Library/Fonts/Supplemental/AppleGothic.ttf`.
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

type Candidates = Vec<(PathBuf, u32)>;

/// Symbol fonts: Segoe UI Symbol (Windows), Apple Symbols (macOS), DejaVu Sans (Linux).
fn symbol_candidates() -> Candidates {
    if cfg!(target_os = "windows") {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        vec![(windir.join("Fonts").join("seguisym.ttf"), 0)]
    } else if cfg!(target_os = "macos") {
        vec![(PathBuf::from("/System/Library/Fonts/Apple Symbols.ttf"), 0)]
    } else {
        vec![(PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"), 0)]
    }
}

fn candidates() -> (Candidates, Candidates) {
    let mut regular = Vec::new();
    let mut bold = Vec::new();
    if cfg!(target_os = "windows") {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let fonts = windir.join("Fonts");
        regular.push((fonts.join("malgun.ttf"), 0));
        bold.push((fonts.join("malgunbd.ttf"), 0));
    } else if cfg!(target_os = "macos") {
        regular.push((PathBuf::from("/System/Library/Fonts/AppleSDGothicNeo.ttc"), 0));
        regular.push((PathBuf::from("/System/Library/Fonts/Supplemental/AppleGothic.ttf"), 0));
    } else {
        for folder in
            ["/usr/share/fonts/opentype/noto", "/usr/share/fonts/noto-cjk", "/usr/share/fonts/google-noto-cjk"]
        {
            regular.push((PathBuf::from(folder).join("NotoSansCJK-Regular.ttc"), 1));
            bold.push((PathBuf::from(folder).join("NotoSansCJK-Bold.ttc"), 1));
        }
        regular.push((PathBuf::from("/usr/share/fonts/truetype/nanum/NanumGothic.ttf"), 0));
        bold.push((PathBuf::from("/usr/share/fonts/truetype/nanum/NanumGothicBold.ttf"), 0));
    }
    (regular, bold)
}

fn first(list: &[(PathBuf, u32)]) -> Option<SystemFont> {
    list.iter().find_map(|(path, index)| {
        std::fs::read(path).ok().map(|bytes| SystemFont { path: path.clone(), index: *index, bytes: Arc::new(bytes) })
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
