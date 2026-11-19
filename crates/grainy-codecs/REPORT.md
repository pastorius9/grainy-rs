# JPEG parity and codec port report

Branch: `port/jpeg`. The JPEG follow-up replaces the original pure Rust JPEG implementation with one shared, vendored libjpeg-turbo 2.1.5.1. No source was downloaded. BRIEF.md and Cargo.lock are excluded; no push or merge. All fixtures use generated pixels, and the oracle repository, user library, settings and data are untouched.

## JPEG results

Pillow 12.3.0 reports libjpeg-turbo **3.1.4.1**. The existing **2.1.5.1** produces identical results for every tested variant below. There are **no differing JPEG markers, entropy bytes or decoded samples**, so a version upgrade is unnecessary for this corpus. This is a measured corpus result, not a claim about every possible JPEG.

| Variant | Decode equal to Pillow? | Encode equal to Pillow? |
|---|---|---|
| Baseline/progressive RGB; 4:4:4, 4:2:2, 4:2:0; quality 35/95/100; noise/smooth/odd dimensions | Yes, full and every attainable draft scale | Baseline 4:4:4 RGB: yes, whole file |
| 4:1:1 and 4:4:0 sampling, direct RGB components | Yes, full and reduced | These source encodings are decode-only; oracle exports 4:4:4 YCbCr |
| Gray, CMYK, YCCK, CMYK without Adobe APP14 | Yes, full and reduced | Gray: yes; CMYK/YCCK are not oracle export modes |
| Restart rows/blocks, progressive restart, sequential/progressive arithmetic with restarts | Yes, full and reduced | Decode-only; oracle exports baseline Huffman JPEG |
| Tiny/thin/block-boundary sizes; EXIF orientations 1–8; split EXIF; reordered/incomplete/duplicate/late ICC; unknown APP/COM | Yes | Gray/RGB metadata, ICC chunk boundaries and IPTC: yes |
| 12-bit JPEG | Rejected by both | Not an oracle export mode |

- **60 readable JPEG files, 291 full/reduced decodes**, all native samples exactly equal (maximum error **0**, tolerance **0**). One additional 12-bit source is refused. Draft checks include all attainable 1/1, 1/2, 1/4 and 1/8 denominators; 1-pixel/thin images cannot attain every reduction.
- **45 direct encoder goldens**: RGB/gray, default quality (-1), 0/1/35/50/95/100, no metadata/EXIF/EXIF+ICC, ICC sizes 65,519 / 65,520 / 131,039 bytes. Every output byte matches (maximum byte error **0**, tolerance **0**).
- **24 `export_image` JPEG goldens**: colour, gray+dither, plain+dither and 1×1; qualities 0/1/35/50/95/100. Complete files, including EXIF, ICC and inserted IPTC APP13, are byte-identical.
- JPEG `load_image` linear pixels and checked thumbnails have maximum error **0**. CLI `src_jpeg_png16`, `src_jpeg_tiff8_mix` and JPEG export comparisons now assert exact Pillow pixels; all measure **0**. No engine test contained an old JPEG deviation bound.
- The independent Pillow check reads all 34 exported files: 20 lossless exports recover exact quantized pixels, all 11 JPEG exports are byte-identical to freshly encoded Pillow references; three AVIF exports remain a different lossy encoder.
- `golden/codecs`: **261 cases, 7,157,024 bytes**, below 10 MB. [Decode measurements](verification/decode.json) and [independent export measurements](verification/exports.json) are observations, not accepted deviation baselines.

## Implementation and preserved behavior

| Oracle operation | Rust implementation |
|---|---|
| `rasterio.read_bitmap` → Pillow JPEG load/draft | `grainy_jpeg_sys::decode`, used by `grainy_codecs::decode_bitmap` / `load_image` / `thumbnail` |
| `engine.export_image` → Pillow JPEG save | `grainy_jpeg_sys::encode`, called after existing `grainy_codecs::export_image` quantization |
| LibRaw `USE_JPEG` (lossy DNG) | Shared native JPEG headers/symbols/library, existing LibRaw shim unchanged |

The upstream source review used Pillow's release-pinned [JpegImagePlugin.py](https://github.com/python-pillow/Pillow/blob/12.3.0/src/PIL/JpegImagePlugin.py), [decode.c](https://github.com/python-pillow/Pillow/blob/12.3.0/src/decode.c), [JpegDecode.c](https://github.com/python-pillow/Pillow/blob/12.3.0/src/libImaging/JpegDecode.c) and [JpegEncode.c](https://github.com/python-pillow/Pillow/blob/12.3.0/src/libImaging/JpegEncode.c). The installed plugin and live oracle were also read without writing to the oracle.

- `draft` chooses the largest 8/4/2/1 denominator retaining at least twice the requested size in **both** dimensions. It keeps ISLOW DCT and fancy upsampling; Pillow's internal fast-draft flag is zero. Libjpeg detects RGB/YCbCr/CMYK/YCCK; output is L/RGB/CMYK. Pillow unconditionally inverts all four CMYK channels, including files without Adobe markers. The port preserves this behavior.
- JPEG export uses libjpeg's standard quality tables, force-baseline quantization, 1×1 sampling, ISLOW DCT, no Huffman optimization, no smoothing, no progressive/restart coding. Order: SOI, JFIF, EXIF APP1, ICC APP2 chunks, tables/scan/EOI. Existing oracle-compatible IPTC injection prepends APP13 immediately after SOI.
- ICC chunks contain at most 65,519 profile bytes. Pillow's loader sorts full payloads, checks only the first chunk's total count, retains duplicate sequence numbers, ignores incomplete profiles, and ignores ICC appearing after SOF. Split EXIF markers are concatenated before orientation parsing. These quirks are golden-tested.
- The scalar vendored source and compiler flags are unchanged from the RAW crate. `git mv` moves all retained upstream files without modifying their bytes. `links = "grainy_jpeg"` exports one header directory via `DEP_GRAINY_JPEG_INCLUDE`; namespaced symbols avoid collisions. The RAW Cargo dependency and two-line Rust link-retention import are necessary JPEG wiring in addition to the brief's named RAW paths.
- Unsafe Rust is confined to `grainy-jpeg-sys/src/ffi.rs` for this change. A C shim confines setjmp/longjmp, owns native allocations through failure cleanup, suppresses console diagnostics and converts failures into `Result`. Truncated/malformed inputs, bad shapes, excessive metadata and oversized allocations are tested; decoder sample buffers are capped at 512 MiB. Existing unsafe RAW/CMS code was not expanded.
- Arithmetic input fixtures use a test-only C coefficient transcoder built from the already-vendored library by the generator. Pillow supplies every expected decoded sample. The transcoder is not a runtime API or dependency.

## Other codec behavior retained

PNG/BMP/WebP/TIFF coverage and APIs remain as before. Native samples are exact in the existing corpus; the largest non-JPEG linear error is **4.7683716e-7** for TIFF YCbCr (tolerance 1e-5). Bilinear/Lanczos resize, quantization, dither, gray detection, EXIF/IPTC/XMP serialization and TIFF metadata append retain their existing goldens. ICC conversion is injected through `ColorManager` and tested using captured oracle calls; this crate does not approximate the CMS.

Known limits remain: AVIF uses ravif rather than Pillow's encoder; JPEG XL is explicit lossless-only because distance-mode libjxl is not ported. Unsupported TIFF and colour-profile cases return errors. EXIF MakerNote/Interop removal, TIFF ASCII replacement, and the previously documented metadata-parser quirks outside JPEG orientation are unchanged. No new non-JPEG feature was attempted.

## Dependencies and requests

New workspace crate: `grainy-jpeg-sys`, using the existing `cc` build dependency (MIT/Apache-2.0) and relocated libjpeg-turbo (IJG/BSD-3-Clause/zlib; see [NOTICE](../grainy-jpeg-sys/NOTICE)). No new registry package or native source was fetched. Core Rust dependencies and other codecs are unchanged.

The direct `jpeg-decoder` and `zune-jpeg` alternatives were removed, along with their enum variants; `JpegDecoder::LibjpegTurbo` is the sole/default standalone JPEG backend. `tiff` still uses its transitive pure Rust `zune-jpeg` for JPEG-compressed TIFF strips/tiles. `zune-core` remains required by the JPEG XL encoder. No standalone JPEG operation uses a pure Rust decoder.

Requests: none for JPEG. Existing future work to share the private Pillow resampler/metadata helpers and supply distance-mode JPEG XL remains outside this brief.

## Validation

All five crates (`grainy-jpeg-sys`, `grainy-codecs`, `grainy-raw`, `grainy-engine`, `grainy-cli`) pass `cargo test` and `cargo clippy --all-targets -- -D warnings`; workspace `cargo fmt --all -- --check` passes. RAW goldens and all other crates' sources are unchanged apart from the explicitly required RAW dependency/link wiring and CLI JPEG assertions. The RAW decode remains bit-exact. Tests include concurrent decoder calls and existing one/four-thread and non-contiguous image checks.
