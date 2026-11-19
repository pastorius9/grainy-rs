# grainy-codecs

Port of Grainy 0.6.8's non-RAW file pipeline, with injected ICC conversion. See [REPORT.md](REPORT.md) for measured parity, limitations and dependencies. JPEG decoding and reduced-IDCT previews match Pillow exactly in the generated corpus; JPEG exports are byte-identical for the same quantized pixels and metadata. Both this crate and LibRaw use the shared vendored `grainy-jpeg-sys`. JPEG XL's built-in encoder remains explicitly lossless.

```rust,ignore
use grainy_codecs::{load_image, export_image, LoadOptions, ExportOptions};

// cms implements ColorManager, supplied by grainy-cms/the application.
let source = load_image("generated-input.png", &LoadOptions::default(), &cms)?;
// The engine develops source.pixels. Pass encoded working RGB as develop(output_space=None) produces.
export_image(&developed, "generated-input.png", "new-output.jpg", &ExportOptions::default(), &cms)?;
```

`load_image_with_decoder` accepts a custom decoder closure. The built-in JPEG backend is `JpegDecoder::LibjpegTurbo`; the unused pure Rust alternatives were removed. `DecodedBitmap.samples` contains interpreted native sample values; `divisor` normalizes them. Palette, Lab and YCbCr TIFFs also retain `native_samples` for diagnostics. TIFF floating samples are already linear, except Lab.

`ColorManager` receives RGB/gray/Lab/CMYK float32 samples (CMYK in percentages), source ICC bytes, destination ICC bytes, and relative-colorimetric intent semantics. It also supplies named or calibrated profiles and their model/description. Calibrated profiles with neither gamma nor transfer data require the oracle's 65,536-entry float32 sRGB transfer table. This crate does not approximate ICC conversion. Tests record the calls and return converted arrays saved by the real oracle.

Exports create a new destination exclusively. Existing files and source files are preserved. Failure before creation leaves no file; write/flush failure removes the file created by that invocation. Ordinary inputs are encoded working RGB; HDR TIFF inputs are already scene-linear and require `hdr_enabled`. Standalone quantization functions preserve their input, unlike the oracle's writable-buffer optimization.

Reproduce on Windows from the worktree root:

```powershell
$env:PYTHONDONTWRITEBYTECODE = '1'
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_codecs.py
$env:PATH = "$env:USERPROFILE/.cargo/bin;$env:PATH"
cargo fmt --all -- --check
cargo clippy --offline -p grainy-jpeg-sys -p grainy-codecs -p grainy-raw -p grainy-engine -p grainy-cli --all-targets -- -D warnings
cargo test --offline -p grainy-jpeg-sys -p grainy-codecs -p grainy-raw -p grainy-engine -p grainy-cli -- --nocapture
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_codecs.py --check crates/grainy-codecs/test-output/exports
```

The generator writes `golden/codecs/`, `crates/grainy-codecs/assets/exif-types.json` and ignored test/build artifacts. Arithmetic fixtures use a small coefficient transcoder built offline from the vendored JPEG library; Pillow supplies the expected pixels. The generator disables Python bytecode writes before importing the oracle; no library/settings/data paths are used. ICC creation timestamps/IDs and fixture mtimes are fixed. Rust fingerprint tests compare against live file stats. The generator records oracle module hashes and Pillow's libjpeg-turbo version in `golden/codecs/versions.json`.

Generated test outputs stay in ignored `test-output/`. Measurements are committed in `verification/`. Every JPEG sample, complete encoded file and CLI JPEG-source export is tested with zero tolerance. No documented-deviation baseline is accepted.
