//! Port of `luma/photo_images.py`: JPEG replies with a size limit, the contact-sheet cell label and the contact
//! sheet. The previews themselves are rendered in [`crate::engine`].

use crate::pillow::{self, Rgb};
use crate::text::Font;

/// `BACKGROUND` (#202020).
pub const BACKGROUND: [u8; 3] = [32, 32, 32];
/// `LABEL_HEIGHT`.
pub const LABEL_HEIGHT: usize = 18;
/// `MAX_COLUMNS`.
pub const MAX_COLUMNS: usize = 6;

/// `round()` of Python (half to even) for a non-negative size.
fn py_round(x: f64) -> usize {
    x.round_ties_even().max(0.0) as usize
}

/// The JPEG bytes of RGB pixels (Pillow `Image.save(format='JPEG', quality=q, optimize=True)`).
///
/// Encoded with libjpeg-turbo through `grainy-jpeg-sys`, whose encoder is the export's (4:4:4, standard Huffman
/// tables). Pillow's defaults here are 4:2:0 chroma subsampling with optimized Huffman tables, so the bytes (and the
/// decoded chroma) differ from the oracle's until `grainy-jpeg-sys` offers those two options (a request in the
/// report). The pixels handed to the encoder are the oracle's.
pub fn encode(image: &Rgb, quality: u32) -> Result<Vec<u8>, String> {
    let (h, w, _) = image.dim();
    let pixels: Vec<u8> = image.iter().copied().collect();
    let options = grainy_jpeg_sys::EncodeOptions {
        width: w as u32,
        height: h as u32,
        channels: 3,
        quality: quality as i32,
        icc: &[],
        exif: &[],
    };
    grainy_jpeg_sys::encode(&pixels, &options).map_err(|e| e.to_string())
}

/// Port of `photo_images.jpeg`: JPEG bytes without an ICC profile, smaller until under `max_bytes`: quality down by
/// 10 to 40, then the size by 0.85 each time (Lanczos).
pub fn jpeg(image: &Rgb, quality: u32, max_bytes: Option<usize>) -> Result<Vec<u8>, String> {
    jpeg_with(image, quality, max_bytes, &mut |_| {}).map(|(data, _, _)| data)
}

/// [`jpeg`] with the width and height of the image encoded last, calling `seen` with the pixels first handed to it.
pub fn jpeg_with(
    image: &Rgb,
    mut quality: u32,
    max_bytes: Option<usize>,
    seen: &mut dyn FnMut(&Rgb),
) -> Result<(Vec<u8>, usize, usize), String> {
    let mut image = image.clone();
    seen(&image);
    loop {
        let data = encode(&image, quality)?;
        let (h, w, _) = image.dim();
        if max_bytes.is_none_or(|m| data.len() <= m) || w.max(h) <= 16 {
            return Ok((data, w, h));
        }
        if quality > 40 {
            quality = 40.max(quality - 10);
        } else {
            let (nw, nh) = (py_round(w as f64 * 0.85).max(1), py_round(h as f64 * 0.85).max(1));
            image = pillow::resize(&image, nw, nh, None);
        }
    }
}

/// Port of `photo_images.mark`: the ASCII label of a contact-sheet cell, e.g. `#123  3*  P`.
pub fn mark(ident: i64, rating: f64, flag: i64) -> String {
    let mut text = format!("#{ident}");
    if rating != 0.0 {
        text += &format!("  {}*", rating.trunc() as i64);
    }
    if flag == 1 {
        text += "  P";
    } else if flag == -1 {
        text += "  X";
    }
    text
}

/// One cell of a contact sheet: the photo id, its pixels (`None`: drawn as "not ready") and its label text.
pub struct Cell {
    /// The photo id.
    pub id: i64,
    /// H×W×3 sRGB bytes, or `None`.
    pub pixels: Option<Rgb>,
    /// The label (with `captions`, up to three lines separated by `\n`).
    pub label: String,
}

/// Port of `photo_images.contact_sheet`: one image of cells, at most 6 columns; each cell is `cell`×`cell` with the
/// picture letterboxed on #202020 above an 18 px label strip (three caption lines with `captions`).
pub fn contact_sheet(cells: &[Cell], cell: usize, captions: bool) -> Rgb {
    let count = cells.len().max(1);
    let columns = MAX_COLUMNS.min(count);
    let rows = count.div_ceil(columns);
    let gap = 2;
    let (sw, sh) = (columns * cell + (columns - 1) * gap, rows * cell + (rows - 1) * gap);
    let mut sheet = Rgb::from_elem((sh, sw, 3), 12);
    let font_size = if captions { 13.max(22.min(cell / 20)) } else { 13 };
    let font = Font::new(font_size as u32);
    let line = font_size + 5;
    let height = if captions { line * 3 + 4 } else { LABEL_HEIGHT };
    let area = (cell, cell.saturating_sub(height));
    for (index, item) in cells.iter().enumerate() {
        let x = (index % columns) * (cell + gap);
        let y = (index / columns) * (cell + gap);
        let (xi, yi) = (x as i64, y as i64);
        pillow::rectangle(&mut sheet, xi, yi, xi + cell as i64 - 1, yi + cell as i64 - 1, BACKGROUND);
        match &item.pixels {
            None => {
                pillow::rectangle(
                    &mut sheet,
                    xi + 4,
                    yi + 4,
                    xi + cell as i64 - 5,
                    yi + area.1 as i64 - 4,
                    [70, 70, 70],
                );
                if let Some(font) = &font {
                    let (cx, cy) = ((x + cell / 2) as f64, (y + area.1 / 2) as f64);
                    font.draw(&mut sheet, cx, cy, "not ready", [210, 210, 210], true);
                }
            }
            Some(pixels) => {
                let picture = pillow::thumbnail(pixels, area);
                let (ph, pw, _) = picture.dim();
                let px = x as i64 + (area.0 as i64 - pw as i64).div_euclid(2);
                let py = y as i64 + (area.1 as i64 - ph as i64).div_euclid(2);
                pillow::paste(&mut sheet, &picture, px, py);
            }
        }
        let lines: Vec<&str> =
            if captions { item.label.split('\n').take(3).collect() } else { vec![item.label.as_str()] };
        let Some(font) = &font else { continue };
        for (n, text) in lines.iter().enumerate() {
            let mut text: Vec<char> = text.chars().collect();
            // Clip within this cell, including long file names and labels.
            while !text.is_empty() && font.length(&text.iter().collect::<String>()) > (cell as f64 - 8.0) {
                if text.len() > 1 {
                    text.truncate(text.len().saturating_sub(2));
                    text.push('…');
                } else {
                    text.clear();
                }
            }
            let step = if captions { line } else { LABEL_HEIGHT } as f64;
            let ty = (y + area.1) as f64 + (n as f64 + 0.5) * step;
            font.draw(&mut sheet, (x + 4) as f64, ty, &text.iter().collect::<String>(), [235, 235, 235], false);
        }
    }
    sheet
}

/// Decode JPEG bytes to RGB pixels as Pillow's `Image.open(...).convert('RGB')` (libjpeg-turbo).
pub fn decode(data: &[u8]) -> Result<Rgb, String> {
    let decoded = grainy_jpeg_sys::decode(data, None).map_err(|e| e.to_string())?;
    let (h, w, channels) = (decoded.height as usize, decoded.width as usize, decoded.channels as usize);
    let mut out = Rgb::zeros((h, w, 3));
    for (i, pixel) in decoded.pixels.chunks(channels.max(1)).enumerate().take(h * w) {
        for c in 0..3 {
            out[[i / w, i % w, c]] = pixel[if channels == 1 { 0 } else { c }];
        }
    }
    Ok(out)
}
