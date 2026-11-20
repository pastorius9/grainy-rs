//! Snapshot comparison shared by every test file, and the shared `settle` for the headless window.
//!
//! A snapshot test **compares** the rendered picture with the committed `tests/snapshots/<name>.png`. It never
//! rewrites the committed file, except with `GRAINY_UPDATE_SNAPSHOTS=1`. When the picture differs it writes the
//! new one next to the old as `<name>.new.png` (ignored by git) and fails.
//!
//! The rule (see `ARCHITECTURE.md`): same size, and at most [`MAX_DIFFERENT_PIXELS`] pixels may differ, where a
//! pixel differs when any of its channels differs by more than [`CHANNEL_TOLERANCE`] levels. That absorbs
//! anti-aliasing and rounding (a few levels on a few pixels) and fails on any changed word, digit or icon.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use grainy_app::GrainyApp;

/// Largest per-channel difference (0..255) a pixel may have and still count as equal.
pub const CHANNEL_TOLERANCE: u8 = 4;
/// How many pixels may differ by more than [`CHANNEL_TOLERANCE`].
pub const MAX_DIFFERENT_PIXELS: usize = 32;

/// `tests/snapshots/`.
pub fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots")
}

/// Whether `GRAINY_UPDATE_SNAPSHOTS=1` asks for the committed pictures to be replaced.
pub fn updating() -> bool {
    std::env::var("GRAINY_UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1")
}

/// The number of pixels that differ by more than the tolerance, and the largest channel difference, or `None`
/// when the sizes differ.
pub fn difference(a: &image::RgbaImage, b: &image::RgbaImage) -> Option<(usize, u8)> {
    if a.dimensions() != b.dimensions() {
        return None;
    }
    let (mut count, mut largest) = (0usize, 0u8);
    for (p, q) in a.pixels().zip(b.pixels()) {
        let d = (0..4).map(|c| p.0[c].abs_diff(q.0[c])).max().unwrap_or(0);
        largest = largest.max(d);
        if d > CHANNEL_TOLERANCE {
            count += 1;
        }
    }
    Some((count, largest))
}

/// Whether this system compares pictures. The committed pictures are Windows renders; other systems draw text
/// with their own font rasteriser and GPU, so there the window is still built, driven and rendered (a missing
/// glyph or a failed render still fails), but the picture is neither compared nor written.
pub const COMPARED: bool = cfg!(windows);

/// Compares `image` with the committed snapshot `name` (or replaces it under `GRAINY_UPDATE_SNAPSHOTS=1`).
/// Panics, after writing `<name>.new.png`, when the picture differs beyond the tolerance or is missing.
/// Windows only ([`COMPARED`]).
pub fn check(image: &image::RgbaImage, name: &str) {
    if !COMPARED {
        return;
    }
    let dir = snapshot_dir();
    std::fs::create_dir_all(&dir).expect("snapshot folder");
    let committed = dir.join(format!("{name}.png"));
    let new = dir.join(format!("{name}.new.png"));
    if updating() {
        if !committed.exists() || difference_from_file(image, &committed) != Some((0, 0)) {
            image.save(&committed).expect("write the snapshot");
        }
        let _ = std::fs::remove_file(&new);
        return;
    }
    let old = match image::open(&committed) {
        Ok(old) => old.to_rgba8(),
        Err(error) => {
            image.save(&new).expect("write the new snapshot");
            panic!("snapshot `{name}` cannot be read ({error}); the new picture is {new:?}; run with GRAINY_UPDATE_SNAPSHOTS=1");
        }
    };
    match difference(image, &old) {
        Some((count, _)) if count <= MAX_DIFFERENT_PIXELS => {
            let _ = std::fs::remove_file(&new);
        }
        other => {
            image.save(&new).expect("write the new snapshot");
            match other {
                Some((count, largest)) => panic!(
                    "snapshot `{name}` differs: {count} pixels over {CHANNEL_TOLERANCE} levels (largest {largest}); \
                     see {new:?}; accept with GRAINY_UPDATE_SNAPSHOTS=1"
                ),
                None => panic!(
                    "snapshot `{name}` changed size ({:?} against {:?}); see {new:?}; accept with GRAINY_UPDATE_SNAPSHOTS=1",
                    image.dimensions(),
                    old.dimensions()
                ),
            }
        }
    }
}

fn difference_from_file(image: &image::RgbaImage, path: &std::path::Path) -> Option<(usize, u8)> {
    let old = image::open(path).ok()?.to_rgba8();
    difference(image, &old)
}

/// Renders the harness's frame and checks it against snapshot `name`; returns the picture.
pub fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) -> image::RgbaImage {
    let image = harness.render().expect("render the frame");
    check(&image, name);
    image
}

fn fixture_drive_captions(app: &mut GrainyApp) {
    // The committed fixtures were rendered on C:. Keep the fixture's drive caption
    // stable when TEMP is redirected into a worktree on another drive. Paths, keys,
    // pixels and comparison tolerances are not rewritten.
    let temporary = super::temp_root();
    let state = app.cx.state::<grainy_app::session_state::ColumnState>();
    fn contains_fixture(nodes: &[grainy_app::session_state::TreeNode], temporary: &std::path::Path) -> bool {
        nodes.iter().any(|node| {
            std::path::Path::new(&node.path).starts_with(temporary) || contains_fixture(&node.children, temporary)
        })
    }
    for node in &mut state.nodes {
        if node.drive && node.name != "C:" && contains_fixture(&node.children, &temporary) {
            node.name = "C:".into();
        }
    }
}

/// Runs frames until the app has loaded, rendered and shown everything it asked for (renderer, jobs, folder
/// listings, thumbnails, previews), four quiet frames in a row, then two more so the last answer is drawn.
pub fn settle(harness: &mut Harness<'_, GrainyApp>) {
    settle_until(harness, false);
}

/// [`settle`], and also waits for the first photo frame to arrive (a window that opens a photo asks for it a
/// frame or two after the start, when `idle` is still true).
pub fn settle_frame(harness: &mut Harness<'_, GrainyApp>) {
    settle_until(harness, true);
}

fn settle_until(harness: &mut Harness<'_, GrainyApp>, need_frame: bool) {
    let started = Instant::now();
    let mut quiet = 0;
    while quiet < 4 {
        fixture_drive_captions(harness.state_mut());
        harness.step();
        if harness.state().idle() && (!need_frame || harness.state().frame().is_some()) {
            quiet += 1;
        } else {
            quiet = 0;
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(started.elapsed() < Duration::from_secs(300), "the app did not settle: {}", harness.state().status());
    }
    for _ in 0..2 {
        fixture_drive_captions(harness.state_mut());
        harness.step();
    }
}

/// Opens exactly the folders of the tree whose names are in `open` (and the drives), so a picture does not depend
/// on when the folders appeared during an import (new folders with photos expand by themselves, once, as the
/// listing grows). Call it after `settle`, then `settle` again.
pub fn open_folders(app: &mut GrainyApp, open: &[&str]) {
    use grainy_app::session_state::{ColumnState, TreeNode};
    fn keys(nodes: &[TreeNode], open: &[&str], out: &mut Vec<String>) {
        for node in nodes {
            if node.drive || open.contains(&node.name.as_str()) {
                out.push(node.key.clone());
            }
            keys(&node.children, open, out);
        }
    }
    let state = app.cx.state::<ColumnState>();
    let mut wanted = Vec::new();
    keys(&state.nodes, open, &mut wanted);
    state.expanded.clear();
    state.expanded.extend(wanted);
}

/// Replaces the status line by `text`. The line shows whichever message arrived last, and the import's "complete"
/// and the preview's size line come from independent jobs, so a picture whose subject is not the status line sets it.
pub fn pin_status(app: &mut GrainyApp, text: &str) {
    app.cx.set_status(text);
}
