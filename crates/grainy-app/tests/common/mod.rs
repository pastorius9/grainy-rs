//! Generated color and resolution charts and the engine's own render of them, for comparisons with the UI.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use grainy_core::{normalized, Settings};
use grainy_engine::files::load_photo;
use grainy_engine::{develop, Options};
use ndarray::Array3;

pub mod snapshot;

/// The interface language is process-wide, so every test that renders, switches the language or asserts on
/// translated text holds this lock for its whole body (never take it twice in one test: it is not re-entrant).
pub fn language_lock() -> std::sync::MutexGuard<'static, ()> {
    offline();
    static LANGUAGE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LANGUAGE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Switches every network use of the program off for this test process (the switch `--offline` /
/// `GRAINY_OFFLINE=1` sets; ARCHITECTURE.md "Network"). The helpers every test uses call it, so no test can reach
/// the network even through a real transport.
pub fn offline() {
    grainy_platform::net::set_offline();
}

/// A fresh, empty folder for one test, named `name`. The unique part (the process id) is in the folder above, so
/// a path shown on screen (folder tree, path lines) reads the same in every run: the folder tree starts at the
/// first folder that holds photos, and that is this one or one inside it.
pub fn temp_dir(name: &str) -> PathBuf {
    offline();
    let root = temp_root().join("grainy-app-tests").join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("temp folder");
    dir
}

/// The app's data folder for a window test on photos in `dir`: beside `dir`, not in it. The folder tree lists the
/// photo folder's subfolders, and the data folder exists only once the app has written something into it, so a
/// data folder inside `dir` showed in the tree or not depending on timing.
pub fn data_dir(dir: &Path) -> PathBuf {
    dir.parent()
        .unwrap_or(dir)
        .join(format!("{}-data", dir.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())))
}

/// The system's temporary folder in its real, long form, the form the app stores after resolving a path: a
/// Windows `TEMP` written with 8.3 short names (`C:\Users\RUNNER~1\...` on the GitHub runner) is spelled out, and
/// macOS's `/var/folders/...` is `/private/var/folders/...` (`/var` is a link). Without the `\\?\` prefix.
pub fn temp_root() -> PathBuf {
    let temp = std::env::temp_dir();
    match std::fs::canonicalize(&temp) {
        Ok(real) => {
            let text = real.to_string_lossy().into_owned();
            match text.strip_prefix(r"\\?\UNC\") {
                Some(rest) => PathBuf::from(format!(r"\\{rest}")),
                None => PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)),
            }
        }
        Err(_) => temp,
    }
}

/// A path of a golden as this system's oracle spells it. The goldens were made on Windows, where the oracle's
/// paths read `\`; on macOS the oracle joins the same folders with `/` (`pathlib`, `os.path.join`). Only for texts
/// that are paths (or path lists) built below a placeholder such as `<ROOT>`: other backslashes would change too.
pub fn native(text: &str) -> String {
    if cfg!(windows) {
        text.to_owned()
    } else {
        text.replace('\\', "/")
    }
}

/// A fixed absolute Windows path of a golden (`Z:\GrainyGolden\meta`) as an absolute path of this system: as it
/// is on Windows, without its drive and with `/` elsewhere (`/GrainyGolden/meta`), so that the system's own path
/// rules (file names, parent folders) apply to it as they applied to the Windows path in the oracle.
pub fn native_absolute(windows: &str) -> String {
    if cfg!(windows) {
        return windows.to_owned();
    }
    let bytes = windows.as_bytes();
    let rest =
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() { &windows[2..] } else { windows };
    native(rest)
}

/// [`native`] for every string in a JSON value.
pub fn native_json(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => Value::String(native(s)),
        Value::Array(a) => Value::Array(a.iter().map(native_json).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), native_json(v))).collect()),
        other => other.clone(),
    }
}

/// A text file of a golden as this system's oracle writes it: Python's text mode writes `\r\n` on Windows only.
pub fn native_lines(text: &str) -> String {
    if cfg!(windows) {
        text.to_owned()
    } else {
        text.replace("\r\n", "\n")
    }
}

fn encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The UI fixture keeps its historical 240 × 160 dimensions and filenames. It only uses
/// generated colors: gradients for tone/white balance, reference patches and line pairs for detail.
pub fn garden(scale: usize, gain: [f32; 3], flip: bool) -> Array3<f32> {
    const WIDTH: usize = 240;
    const HEIGHT: usize = 160;
    const PATCHES: [[u32; 3]; 8] = [
        [928, 76, 48],
        [48, 740, 126],
        [73, 142, 935],
        [880, 590, 31],
        [508, 76, 690],
        [38, 632, 701],
        [492, 312, 178],
        [384, 384, 384],
    ];
    let chart = Array3::from_shape_fn((HEIGHT, WIDTH, 3), |(y, x, c)| {
        let level = if y < 16 {
            (1023 * x / (WIDTH - 1)) as u32
        } else if y >= 120 {
            // Four checkerboard frequencies, each at high and low contrast.
            let period = [1, 2, 4, 8][x / 60];
            let contrast = if y < 140 { 432 } else { 64 };
            if (x / period + y / period).is_multiple_of(2) {
                512 - contrast
            } else {
                512 + contrast
            }
        } else if x >= 176 {
            PATCHES[(y - 16) / 26 * 2 + (x - 176) / 32][c]
        } else {
            let red = 48 + 890 * x / 175;
            let green = 32 + 820 * (y - 16) / 103;
            let blue = 48 + 5 * (x.abs_diff(43) + y.abs_diff(78));
            [red, green, blue.min(994)][c] as u32
        };
        level as f32 / 1023.0
    });
    Array3::from_shape_fn((HEIGHT * scale, WIDTH * scale, 3), |(y, x, c)| {
        let sx = if flip { WIDTH - 1 - x / scale } else { x / scale };
        chart[[y / scale, sx, c]] * gain[c]
    })
}

/// Saves a scene-linear image as an 8-bit sRGB PNG.
pub fn save_png(image: &Array3<f32>, path: &Path) {
    let (h, w, _) = image.dim();
    let buffer = image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let p = |c: usize| (encode(image[[y as usize, x as usize, c]]) * 255.0 + 0.5) as u8;
        image::Rgb([p(0), p(1), p(2)])
    });
    buffer.save(path).expect("write the PNG");
}

/// Saves a scene-linear image as a 16-bit sRGB TIFF.
pub fn save_tiff(image: &Array3<f32>, path: &Path) {
    let (h, w, _) = image.dim();
    let buffer: image::ImageBuffer<image::Rgb<u16>, Vec<u16>> =
        image::ImageBuffer::from_fn(w as u32, h as u32, |x, y| {
            let p = |c: usize| (encode(image[[y as usize, x as usize, c]]) * 65535.0 + 0.5) as u16;
            image::Rgb([p(0), p(1), p(2)])
        });
    buffer.save(path).expect("write the TIFF");
}

/// Eight PNG/TIFF chart variants in `dir`, with different exposures, color casts and orientations.
pub fn eight_photos(dir: &Path) -> Vec<PathBuf> {
    let variants: [([f32; 3], bool, &str); 8] = [
        ([1.0, 1.0, 1.0], false, "garden-01.png"),
        ([0.5, 0.5, 0.5], false, "garden-02.png"),
        ([2.0, 2.0, 2.0], false, "garden-03.tif"),
        ([1.3, 1.0, 0.7], false, "garden-04.png"),
        ([0.7, 1.0, 1.4], true, "garden-05.tif"),
        ([1.0, 1.0, 1.0], true, "garden-06.png"),
        ([1.6, 0.8, 1.6], false, "garden-07.png"),
        ([0.25, 0.25, 0.25], true, "garden-08.png"),
    ];
    variants
        .iter()
        .map(|(gain, flip, name)| {
            let path = dir.join(name);
            let image = garden(2, *gain, *flip);
            if name.ends_with(".tif") {
                save_tiff(&image, &path);
            } else {
                save_png(&image, &path);
            }
            path
        })
        .collect()
}

/// Settings from JSON, normalized.
pub fn settings(json: serde_json::Value) -> Settings {
    match json {
        serde_json::Value::Object(map) => normalized(&map),
        _ => normalized(&serde_json::Map::new()),
    }
}

/// What the engine makes of `path` with `settings` at the worker's sizes: RGBA bytes, width, height.
pub fn engine_render(path: &Path, max_size: Option<usize>, settings: &Settings) -> (Vec<u8>, usize, usize) {
    let loaded = load_photo(path, max_size, settings).expect("load");
    let (h, w, _) = loaded.photo.pixels().dim();
    let size = |k: &str, f: usize| loaded.metadata.get(k).and_then(serde_json::Value::as_u64).unwrap_or(f as u64);
    let options =
        Options { original_size: Some((size("width", w) as f64, size("height", h) as f64)), ..Options::default() };
    let image = develop(loaded.photo.source(), &settings.0, &options).expect("develop");
    let converted = grainy_app::histogram::convert(&image);
    (converted.rgba, converted.width, converted.height)
}

/// Reproduce a preview develop at its actual source sampling, before rotation, cropping and retouch.
pub fn engine_render_for_frame(
    path: &Path,
    frame: &grainy_app::render::Frame,
    settings: &Settings,
) -> (Vec<u8>, usize, usize) {
    let longest = (f64::from(frame.original.0.max(frame.original.1)) / frame.scale).round() as usize;
    engine_render(path, Some(longest), settings)
}
