# grainy-raw

CPU port of Grainy's RAW decoding, camera colour, DCP profiles and metadata-based
import rules. The Python oracle and the owner's libraries/settings were not modified
or used as test inputs. Work belongs to branch `port/raw`.

## Windows X-Trans interpolation

On 2026-11-21, three interleaved runs on an i9-12900K compared the 0.3.0
serial LibRaw kernel with its dependency-ordered tile schedule. On the CC0
Fujifilm X-T3 `AFXT2721.RAF`, the median full-resolution sRGB native
postprocess took **16.437 s with raster-order tiles and 4.822 s with the new
schedule**. These measurements used identical optimized MSVC release
archives, with only the X-Trans translation unit replaced, and eight available
OpenMP threads. This particular frame used seven workers. Timings start after
unpacking and stop before checking the RGB16 buffer.

The X-T3 produced the same full-buffer SHA-256 in camera, sRGB and ProPhoto
colour modes at full and half resolution. The sRGB full result also matched
at thread limits 1, 2 and 8. A second CC0 camera, the X-T2
`20170114_173532_TFW04723.RAF`, matched the original serial decoder at both
resolutions. Both originals are in the Fujifilm collection at raw.pixls.us.
The Windows regression test `xtrans_partial_tiles_equal_the_serial_rgb16_decode`
generates a textured 1031 × 1043 mosaic with partial tile edges and checks
the RGB16 digest captured from the original serial kernel.

A second optimization shares each derivative threshold across directions and
computes the 5-by-5 homogeneity score as two 5-sample sums. Three-channel
CIELab conversion uses the decoder's original tables and operation order.
On the same X-T3 frame, three interleaved Windows trials of this change
reduced median sRGB postprocessing from 3.407 s to 2.428 s with identical
full-frame RGB16 hashes. Each pair used the same eight-thread limit.

The schedule limits scratch to eight tile buffers. Each eight-direction
buffer takes 23.5 MiB. Available physical memory and commit space can reduce
the worker count, and a single worker processes tiles in raster order.

## API and oracle mapping

| Oracle | Rust |
| --- | --- |
| `rawpy.imread`, metadata properties, `postprocess` in `engine.load_image` | `RawFile::{open,metadata,postprocess}`, `RawMetadata`, `DecodeMode` |
| RAW branch of `engine.read_metadata` / `_container_tags` | `metadata::read_metadata` |
| `rawcolor.CameraSource` | `rawcolor::CameraSource::{new,pixels,raw_info}` |
| `rawcolor.enabled`, temperature/white-point functions, `default_profile`, `individual_matrix` | same names in `rawcolor` |
| `rawcolor.decode`, `read_dng_calibration`, `reduce_false_color` | same names in `rawcolor` |
| `rawcolor.develop_camera`, `linear_frame`, `retarget` | same names; `develop_plain` / `linear_frame_plain` expose the ordinary-array branches |
| all `dcp` parsing, stored records, matching, interpolation, adaptation, camera matrices, tables and curves | same names in `dcp`; table interpolation is `interpolate_table` |
| `raw_defaults.key`, `camera_key`, `serial`, `is_raw`, `iso_value`, `iso_anchors`, `make_rule`, `resolve`, `builtin_luma_nr`, `builtin_iso_detail`, `linear_raw` | same names in `raw_defaults` |

`CameraInfo` is the JSON-compatible calibration record. `Profile` is an owned
compiled profile; its table fields use explicit dimensions and a flat buffer.
Stored user DCP records remain the original name/camera/copyright/SHA256/base64
objects, and `load_profile` preserves all bytes. `compiled` verifies the digest on
each call; callers can retain the resulting profile (the Python LRU is unnecessary
for correctness). `resolve_white` always includes the white XYZ in its result.

`RawMetadata` includes raw/output dimensions, margins, orientation/flip, pixel
aspect, CFA descriptor, colour count/description, camera/daylight WB, both colour
matrices, adjusted per-channel black levels, white/linear maxima, and LibRaw's
make/model/serial/lens/exposure fields. The rawpy-used metadata is golden-tested
exactly. `metadata::read_metadata` separately provides the oracle's EXIF JSON keys:
camera/make, exposure, ISO, aperture, focal length, subject distance, date, lens,
creator, copyright, body serial and GPS. It handles TIFF RAWs, Olympus/Panasonic
headers and ISO fallback, CR3 CMT boxes, and RAF's embedded EXIF. The JPEG is read
only for metadata; previews and thumbnails are not decoded.

## Native build

`build.rs` uses only `cc` and MSVC Build Tools on Windows. It compiles the official
unmodified LibRaw 0.22.1 release with `LIBRAW_NODLL`, `USE_JPEG`, `USE_ZLIB`,
`USE_LCMS2`, `_CRT_SECURE_NO_WARNINGS`, `/O2`, `/fp:precise`, and `/EHsc` for C++.
The `_ph.cpp` placeholder alternatives are excluded as in upstream's build.
Only `sys.rs` permits unsafe Rust; C++ exceptions are contained in the C ABI shim.
Windows paths use native UTF-16. Decoder handles are owned and not `Send`/`Sync`.

Optional codecs are statically built from source: zlib 1.3.1 for floating-point
deflate DNG and libjpeg-turbo 2.1.5.1 (8-bit libjpeg ABI 62, scalar IDCT, arithmetic
coding and memory sources) for lossy DNG. LittleCMS 2.19 comes exclusively from
`grainy-cms`: its `DEP_LCMS2_INCLUDE` supplies headers and its `lcms2` static
library supplies both LibRaw and the C++ shim. No assembler is needed.
The scalar JPEG implementation produced bit-identical pixels on the JPEG DNG
fixture. LittleCMS also provides the actual serialized-ICC, relative-colorimetric,
float ProPhoto-to-sRGB conversion used by the oracle; it is not an approximate
matrix substitution. The non-MSVC compiler branch supports clang-style flags but
was not run on macOS in this Windows task.

LibRaw's `dcraw_process` calls `apply_profile` only when `camera_profile` is set;
the oracle parameters never set camera/output profiles, so that LibRaw ICC step
cannot affect these decodes. `USE_LCMS2` is retained. The shim's separate
ProPhoto-to-sRGB transform **does** affect pixels and still uses LittleCMS.

JPEG/zlib static archives are named `grainy_raw_jpeg` / `grainy_raw_zlib`.
The compiler forcibly includes `src/native_symbols.h` in the shim, LibRaw and
codec compilation units; all 190 JPEG and 71 zlib external C symbols are prefixed
`grainy_raw_`. MSVC archive symbol inspection verifies this, excluding compiler
CRT/COMDAT symbols. Another normally named JPEG/zlib copy can coexist; another
copy using the same `grainy_raw_` prefix would collide. No upstream source bytes
were patched, and no Python is needed to build or run this crate.

The oracle reports OpenMP enabled; this build uses serial LibRaw and Rayon for
Rust image rows, without an OpenMP runtime dependency. All tested decodes remain
bit-exact. Both sides disable GPL demosaic packs and RawSpeed. rawpy also advertises
the historical REDCINECODEC flag, but the tagged 0.22.1 source contains no Redcine/
Jasper/OpenJPEG decoder. No unused OpenJPEG library is linked; R3D is outside the
oracle's `RAW_EXTENSIONS` list. X3F/Raspberry Pi support follows the upstream source.

Both postprocess paths explicitly match rawpy 0.27.1 `Params` / `apply_params`:

| Parameter | Legacy (`Srgb` / `ProPhoto`) | Camera |
| --- | --- | --- |
| `output_color` | 1 / 4 | 0 |
| `use_camera_wb`, `use_auto_wb` | true, false | false, false |
| `user_mul` | `[0;4]` | `[1;4]` |
| `adjust_maximum_thr` | .75 | 0 |
| `output_bps`, `gamm[0..2]`, `no_auto_bright` | 16, `[1,1]`, true | same |
| `half_size` | caller-selected | caller-selected |

Other rawpy defaults: `user_qual=-1` (LibRaw chooses AHD), four-colour/DCB/FBDD/
noise threshold/median passes = 0; `user_flip/user_black/user_sat=-1`;
`user_cblack` remains LibRaw's sentinel; `no_auto_scale=0`; default auto-bright
threshold .01; brightness 1; highlight clipping 0; `exp_correc=-1`, shift 1,
preservation 0; red/blue aberration factors 1; no bad-pixel file or ICC overrides.

## Verification

Run from the worktree root in PowerShell:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:OMP_NUM_THREADS = '1'
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_raw.py
cargo fmt --all -- --check
cargo clippy -p grainy-raw --all-targets -- -D warnings
cargo test -p grainy-raw -- --nocapture
cargo test -p grainy-cms
```

All 17 RAW tests pass. There are **1,325 golden case folders** and **1,410 image/decode
comparisons**, plus 974 full-frame mean/min/max comparisons for sampled outputs.
[VALIDATION.json](VALIDATION.json) records each comparison's measured
maximum and tolerance, plus function maxima. The generator sets CPU mode and
disables Python bytecode writes before importing the oracle.

The follow-up reduced `golden/raw` from **155,260,375 bytes / 4,189 files** to
**19,813,334 bytes / 2,745 files** (decimal 19.81 MB, below the 20,000,000-byte
limit). Vendored files fell from **49,730,103 bytes / 1,806 files** to
**4,533,808 bytes / 230 files**. These are actual on-disk file lengths, including
the original ignored downloaded archives and unused OpenJPEG tree. The archives
and OpenJPEG were recycled; unneeded tracked upstream files were removed with
`git rm`. All licences/readmes in the three retained source trees remain.

The development matrix now has 963 cases instead of 1,524. Every original file,
setting value, WB-mode/profile-toggle/working-space pair, and file/setting pair
is retained. Defaults and both extreme WB settings retain no-profile/all-on/
all-off cases in both working spaces; one Bayer file retains the whole matrix.
The test suite validates the covering design and its actual settings. Omitted
coverage is redundant higher-order cross-products, not previously tested branches.

RAW/DCP/EXIF files live once in `_files`; content-addressed NPY arrays live once
in `_arrays`. Shared settings retain a single oracle-default snapshot plus deltas.
All 114 uint16 decode comparisons and 38 camera-source comparisons retain their
full arrays, so no checksum substitute is needed. Bulk development outputs use
every fourth pixel in both dimensions **and** full-frame float64 channel means,
minima and maxima at the original 1e-5 tolerance. Representative outputs remain
full-frame for every branch; table/curve/median/negative checks remain full-frame.
The Unicode and defaults JSON cases remain because they exercise all Unicode
fold/normalization boundaries and compare complete resolved settings/rule IDs.

`reference/verify_fixtures.py --snapshot`, regeneration, then `--check` verifies
every filename/byte plus generated Unicode source, the size limit and array pool
integrity. [FIXTURE_VALIDATION.json](FIXTURE_VALIDATION.json) records the result.
For an occasional full audit, `gen_raw.py --exhaustive` restores the original
matrix/full arrays using the same oracle inputs. Regenerate without that flag
before committing. [FULL_CORPUS_VALIDATION.json](FULL_CORPUS_VALIDATION.json)
records that the final native build passed all 1,971 original full-frame
comparisons with exactly the same reported errors as revision `1ad2337`.

Nineteen supported DNG files each have six bit-exact uint16 comparisons (three
spaces, full/half): four Bayer patterns × 12/14/16 bits, big-endian DNG, linear DNG,
dual-calibrated linear DNG, Bayer and linear float-deflate DNG, lossy JPEG DNG,
and the supplied triple-light DNG. A twentieth integer-deflate fixture is rejected
as unsupported by both implementations. Profiles include both supplied assets,
dual/single/triple/custom illuminants, forward and inverse matrices, both byte
orders, both table encodings, hue wrap, identity and truncated tone curves.

| Function(s) | Largest measured absolute error |
| --- | ---: |
| `postprocess` uint16 pixels; rawpy metadata | **0** (required tolerance 0) |
| `metadata::read_metadata` EXIF JSON | **0** (exact values and strings) |
| `raw_defaults` keys, ISO anchors, rules and resolved settings | **0** (exact JSON comparison) |
| `decode`, `reduce_false_color` | **0** |
| `read_tags`, `parse`, matrix/table interpolation, `weight`, calibration/individual matrix | **0** |
| `temperature_xyz`, `xyz_temperature` | **0** |
| `adapt_white`, `camera_to_xyz` | 1.066e-14 |
| `neutral_white` (Kelvin included) | 6.622e-10 |
| `resolve_white` (Kelvin included) | 8.186e-12 |
| `table_map` | 1.192e-6 |
| `tone_map` | **0** |
| `develop_camera` (both working spaces) | 2.980e-6 |
| `linear_frame` for camera sources | 4.768e-7 |
| `linear_frame_plain` (12-stop negative conversion stress case) | 5.245e-6 |

All image arithmetic uses **1e-5 absolute tolerance**, including the OpenCV-derived
HSV and median primitives; the median is additionally required to be exact.
Scalar/matrix tests use tighter 1e-10/1e-8 where appropriate. Stored records,
camera matching, defaults/rules, notes, Unicode keys, no-ops and rejection paths
are also checked. One- and four-thread median results are identical. Bad shapes,
malformed/truncated profiles, zero rational denominators, invalid calibration,
unsupported HDR/gain tables, and corrupted pinned DCP records return errors.
BigTIFF colour tags and direct SubIFD overrides are tested independently of decode.

Workspace formatting validation and crate Clippy pass without warnings.

## Preserved decisions and quirks

- `rawcolor` uses Robertson's `.24792` at 325 mired; `grainy-color::illuminants`
  uses the SDK's `.24702`. They intentionally remain different.
- NumPy f32 operation order is kept. Triple-light table interpolation promotes to
  f64; two-light interpolation stays f32. Tone-map LUT lookup mixes f32 positions
  with f64 fractional arithmetic. Identity tone curves leave out-of-range pixels
  untouched.
- Photographed negatives bypass the camera matrix, then use `negative.apply`.
- Camera defaults retain the oracle's 30-step white solve, signature matching,
  green-normalized neutral, extreme-tint floor and half-size false-colour bypass.
- ISO interpolation uses stops and Python ties-to-even rounding; rule IDs use
  Python's JSON whitespace/ASCII escaping. Unicode 15.1 casefold/alphanumeric data
  is generated from the oracle and normalization is pinned to matching 0.1.23.
- LibRaw's deflate reader assumes LONG TileByteCounts. The synthetic float-DNG
  writer explicitly emits LONGs; LibRaw itself is not patched.
- The oracle does not recognize GPS names for root-IFD fields in CMT4 blocks;
  this quirk is retained. Nested GPS IFDs are handled normally.
- Goldens exercise synthetic data only. Real-camera corpus coverage and macOS
  execution are not claimed.

## Dependencies, exclusions and shared-crate requests

Added Rust dependencies: `cc` (build only), `base64`, `sha2`, and pinned
`unicode-normalization` (MIT/Apache-2.0), plus `kamadak-exif` (BSD-2-Clause).
`serde_json` enables `float_roundtrip`
so exact f32 metadata is not perturbed by JSON parsing. Existing core/color,
ndarray, serde, thiserror and testkit/Rayon dependencies are reused. This follow-up
adds only the existing workspace `grainy-cms` dependency and test-only
`ndarray-npy` (already used transitively by testkit). Native sources,
licenses and SHA256 provenance are detailed in [NOTICE](NOTICE).

Profile discovery/storage, Qt, embedded previews/thumbnails, non-RAW metadata,
and `raw_defaults.import_preview` (UI/engine orchestration, not pure metadata
logic) are excluded as specified. Rendering orchestration can call `decode`,
`develop_camera` and `linear_frame` directly.

**Requests:** consolidate the private `negative.apply` helper with `grainy-film`,
the exact ICC conversion API with `grainy-cms`, RAW EXIF with
the codecs owner, and the small matrix/TIFF/median/OpenCV-HSV primitives with shared crates when equivalents are
available. This port does not modify those crates or the workspace manifest.
