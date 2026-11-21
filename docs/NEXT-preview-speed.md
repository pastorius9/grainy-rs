# Next: preview speed against Lightroom Classic, and a denser Develop panel

Hand-over note, 2026-11-21. Branch `preview-speed`, commit "Preview speed with detail edits: GPU under a monitor
profile, fit previews at the view's size". **Not yet run after these changes: the test suite and the on-screen
measurement of the view-size change.**

## Develop UI update

The product goal is a Rust photo editor that surpasses Lightroom Classic in daily editing. For the Basic panel,
we measure how many complete controls a photographer can see and use before scrolling. On the same default DNG
at 1920×1080 and 125% scaling, the Korean panel went from 4 fully visible adjustment rows to all 15.
The English panel also fits all 15. The regression test `basic_raw_sliders_fit_full_hd` opens the DNG,
checks the slider hit areas and types a value into the last field, including with only 1000 client pixels of height.
These measurements compare the two Grainy builds. A side-by-side editing session with Lightroom Classic is still pending.

## What the owner reported (after 0.2.1)

1. On a 24 MP Leica SL DNG, Lightroom Classic reacts far faster. Moving **Exposure** updates the filmstrip
   thumbnail quickly, but the main photo view changes much later.
2. The Develop panel's text and controls are much larger than Lightroom's, so it needs a lot more scrolling.
   Goal: as dense as Lightroom Classic (the whole Basic panel visible without scrolling on a 1920×1080 screen;
   today only White Balance and Exposure fit).
3. Every speed claim must be checked against the real Lightroom Classic on the same photo, several tests.

## Findings

- **The 0.2.1 speed work measured the wrong settings.** RAW photos are imported with detail edits
  (`sharpen` 60, `noise_luma` 4, `noise_color` 25: `tools/perf/detail_settings.json` is a real photo's edits).
  The 0.2.1 benchmark used `defaults()` without them, so every case took the exact Basic shortcut
  (`basic_preview.rs`, ~50 ms) and the detail stage was never timed.
- **A monitor profile switched the GPU off** (`worker.rs`, `PreviewState::render`: `monitor.is_none()`), and the
  automatic monitor profile is the default. Every edit then re-ran the detail stage on the CPU at 2048 px:
  `detail.chroma_guided` 1.4 s + `detail.luma_wavelet` 0.76 s + `detail.sharpen` 0.12 s, **~1.5 s per Exposure
  change**. The thumbnail (320 px) comes back in ~0.1 s: the reported symptom.
- **Fixed in this commit:** the GPU develops the working pixels with or without a monitor profile; the profile is
  applied on the CPU afterwards. Exports still develop on the CPU; the screen may differ from the CPU develop by a
  rounding step. Benchmark, back to back: 1.5 s → ~0.12 s.
- **On screen it is still ~0.48 s** from moving the slider to a changed main view (real window, 1080p, maximized,
  measured with `tools/perf/screenlat.py` on the build with the GPU change only). Cause: **GPU idle clocks**. An
  isolated change after a pause is ~3× slower than changes back to back
  (`GRAINY_BENCH_GAP_MS`: 0 → 113 ms, 300 → 190 ms, 1500 → 320 ms). In the app's log the first
  `gpu.readback.wait` of each render is ~315 ms.
- **The fit preview was developed at 2048 px while the screen shows ~1010×674 device pixels** on a 1080p screen.
  **Also in this commit (not yet measured on screen):** fit previews are developed at the fitted view's long side
  (device pixels, rounded up to 128; `AppContext::view_need` → `Renderer::set_view_size`) from a Lanczos-reduced
  copy of the 2048 fit source, when that is under 85% of it (`PreviewState::reduce`). The 2048 source itself,
  auto tone's analysis source, masks, point colour, zoomed views and exports are unchanged.

## Next steps, in order

1. **Tests:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` and
   `cargo test --workspace --all-features`. Snapshot tests may change if one shows a large photo through the real
   worker. `preview_bench verify` asserts exact monitor bytes with the GPU on: Shadows may now differ by a rounding
   step (make it compare the CPU path, or allow one level).
2. **Measure on screen** with the view-size change (below). Target: the main view follows a slider like
   Lightroom's, well under 100 ms from an idle start.
3. **Remaining GPU cost.** Each render does 4 full-frame readbacks: 3 for the luminance-noise statistics inside
   the detail stage (`grainy-gpu/src/detail.rs`, `resident.rs`) and the result. Ideas: reduce the statistics on the
   GPU and read back only the numbers; wake the GPU's clocks as soon as a slider is pressed; smaller buffers.
4. **Compare with Lightroom Classic** using the same tool (below): same photo, Develop module, fit view, drag
   Exposure; undo afterwards (Ctrl+Z). The owner opens Lightroom on the photo first.
5. **Zoom:** the first full-resolution render took 18 s on the CPU before the GPU change; measure it again.
6. **Develop panel comparison** (report 2): compare the compact layout with Lightroom Classic at the same screen
   size, especially the precision of fine slider movements and the time needed to find an adjustment.

## How to measure

Benchmark (no window; `<icc>` can be `C:\Windows\System32\spool\drivers\color\sRGB Color Space Profile.icm`):

```
cargo build --release -p grainy-app --example preview_bench
set GRAINY_BENCH_SETTINGS=tools\perf\detail_settings.json
set GRAINY_BENCH_FIT_ONLY=1
set GRAINY_BENCH_GAP_MS=1500
target\release\examples\preview_bench <copy of a 24 MP RAW> <icc> worker-case on icc
```

`GRAINY_TIMING=1` prints each stage's time on stderr.

On screen (real window; the same method works for Lightroom Classic):

1. Start a test instance on its own data folder with a **copy** of the photo:
   `set GRAINY_RS_HOME=<empty temp folder>`, `set GRAINY_TIMING=1`, `set RUST_LOG=info`,
   `target\release\grainy-app.exe --offline <copy of the RAW>` (stderr to a file for the render log).
2. Maximize it and open the photo in Develop (`python tools/perf/screenlat.py click pid:<pid> x y`, `dclick` to
   open).
3. `python tools/perf/screenlat.py shot out.png` and read the coordinates of the Exposure handle, an area inside
   the photo, and the open photo's filmstrip thumbnail.
4. A config such as `{"title": "pid:<pid>", "handle": [1775, 924], "dx": 30, "main": [700, 250, 420, 320],
   "thumb": [316, 878, 88, 56]}` and `python tools/perf/screenlat.py step cfg.json 6 results.jsonl`
   (`sweep` drags continuously for one second and counts the visible updates).

The tool moves the real mouse: run it only when the owner agrees and is not using the computer.

## Rules that still apply

- Original photos are never written, moved or deleted; tests use copies. Test instances use their own
  `GRAINY_RS_HOME`, never the real library or settings.
- Exports stay on the CPU develop path and keep their exact pixels.
