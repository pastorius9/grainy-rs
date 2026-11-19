# grainy-dust

CPU port of the data and image algorithms in `luma/dust.py`, `dust_shapes.py`,
`dust_scratches.py`, `dust_soft.py`, `dust_texture.py`, plus the requested data
functions in `dust_store.py` and `dust_batch.py`. Runtime code executes neither
Python nor OpenCV. The oracle is NumPy 2.5.3 / OpenCV 5.0.0 on Windows, CPU enabled.

## API and oracle coverage

Gray input is an encoded-luminance `grainy_core::Plane` (`Array2<f32>`). Repair
input is an H x W x 3 `grainy_core::Image`. Sizes always refer to source pixels.
Spot dictionaries are `serde_json::Value`; extra metadata and multipart footprints
remain intact. `Options::default()` matches the Python dataclass. Its typed fields
reject wrong JSON types, and `validate()` checks the oracle's bounds.

| Oracle function | Rust item | Golden cases | Equality / largest absolute error |
|---|---|---:|---|
| `Cancelled`, `checkpoint` | `Error::Cancelled`, `checkpoint` | 7 mid-detection cancellations, early/mid-repair checks | Same cancellation/progress sequence |
| `Options`, `Options.validate` | `Options`, `Options::validate` | 56 detector configurations + invalid-option cases | Equal |
| `_median` | `median` | 5 vectors | 0 |
| `_quantile_plan`, `_quantiles` | `quantile_plan`, `quantiles` | 5 vectors, 9 fractions each | 0, including plan arrays |
| `_component_indices` | `component_indices` | 3 statistics/bounds configurations | Equal labels and order |
| `_repeated_detail` | `repeated_detail` | 8 pattern/core/border configurations | Equal booleans |
| `detect` | `detect` | 56 | Exact entire spot lists, totals, truncation, dimensions and progress |
| `spot_bounds` | `spot_bounds` | 7 | 0 |
| `visualize` | `visualize` | 15 | 0 |
| `repair`, smooth | `repair`, `repair_in_place` | 22, both copy modes | 0; full-frame SHA-256 checked |
| `repair`, texture | `repair`, `repair_in_place` | 22, both copy modes | Measured maximum 9.775e-6; tolerance 2e-4 |
| `dust_shapes.ellipses`, `bounds`, `paint` | `shapes::{ellipses,bounds,paint}` | 7 each, 14 mask comparisons | Exact coordinates, bounds and bytes |
| `dust_shapes.make_spot`, `manual`, `MAX_PARTS` | `shapes::{make_spot,manual,MAX_PARTS}` | 1 constructor, 4 manual paths, overflow | Exact dictionaries / same rejection |
| `dust_scratches.cover` | `scratches::cover` | 6: line, crossing, loop, empty, full, overflow | Exact part lists or None |
| `dust_scratches.candidates` | `scratches::candidates` | 2 standalone + integrated detection | Exact dictionaries and progress |
| `dust_soft.candidates`, `merge` | `soft::{candidates,merge}` | 2 standalone + integrated detection; 1 merge | Exact dictionaries and order |
| `dust_texture.restore`, `_prepare` | `texture::{restore,prepare}`, `Prepared::apply` | 5 each | At most 1.788e-7; tolerance 2e-4; exact statistics |
| `dust_store.unpack`, encoded payload | `store::{unpack,pack}` | 4 bidirectional payloads, scanning state, 7 corrupt streams | Equal decoded JSON / rejection |
| `dust_store.target_key` | `store::target_key` | 3 | Exact SHA-256 strings |
| `dust_batch.capture_scope_ids` | `batch::capture_scope_ids` | 4 + live generated catalog | Exact dictionaries |
| `dust_batch.capture_record`, `verify_source` | `batch::{capture_record,verify_source}` | 4 records + live signature/conflict checks | Exact snapshots; same accept/reject outcome |

`repair` is Python's `copy=True`; `repair_in_place` is `copy=False`. Both methods
accept `"smooth"` or `"texture"`. Cancellation is a `Sync` callback (e.g. an
`AtomicBool` reader). Progress is a mutable `(completed,total)` callback.
`Detection.seconds` is measured wall-clock time and is excluded from golden
equality; every other result field is compared without a numeric tolerance.

The texture tolerances cover float32 filter/correlation rounding. They are not
applied to detection metadata, masks, counts or lists. Every numeric comparison's
measured maximum and tolerance is in [golden_errors.json](golden_errors.json),
with raw results in [validation.log](validation.log).

## Fixtures and verification

Committed `golden/dust` is approximately **6.51 MB**, below the 10 MB limit.
It includes smooth/seeded-grain scans, soft blobs, repeated patterns, adjacent
edges, bright/dark specks, straight/curved/crossing/looped hairs, border marks,
and a **700 x 900** scan crossing 640-pixel tile seams. All seven standard inputs
are also covered. Every numeric/string option has its default and two alternatives;
boolean options cover their two possible values. Additional runs cover combinations,
small tiles, truncation, full masks, empty repairs and invalid data.

The generator stores full small handwritten repair fixtures. For detected repairs
on every scan it stores **all selected pixels**, not just a sparse sample; tests
also assert bitwise identity outside the mask. Smooth repairs additionally check
the oracle's full-frame checksum. The generator proves Python's two copy modes
agree; Rust tests prove both copy modes and one/four Rayon workers agree.

The generated SQLite fixture contains fictional source paths. Live signature and
Python/Rust interoperability tests create only synthetic bytes under
`target/dust-roundtrip`. No real library, settings, photographs, or oracle `data/`
directory is opened. Imports disable Python bytecode writes.
Fixture import timestamps and unrelated random preview tokens are fixed; repeat
generation is checked byte-for-byte by `tools/check_generation.py` in this crate.

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_dust.py
cargo fmt -p grainy-dust
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B crates/grainy-dust/tools/verify.py
```

`verify.py` runs `cargo fmt -p grainy-dust -- --check`,
`cargo clippy -p grainy-dust --all-targets -- -D warnings`, `cargo test -p grainy-dust`,
and the oracle's `--check-rust` reader. It saves UTF-8 logs and the per-case error
report. Python reads Rust's zlib streams, compares decoded values, reads the
Rust-produced catalog snapshot and verifies its source fingerprint. Compressed
deflate bytes need not be identical across encoders; the payload format and content
are interoperable.

Final validation: **13 tests passed**, **314 recorded comparisons**, formatting
and Clippy passed, and the Python reader accepted all Rust round-trip artifacts.
The largest error is texture repair on the smooth soft-blob scan
(`detected_repair/7/texture`); ordinary smooth repair is exact in every case.

## Decisions and retained oracle behavior

* Float32 median addition, scalar-percentile weak promotion, backward quantile
  interpolation at weights >= .5, and Python decimal rounding are preserved.
* Ordinary detections keep their priority over review candidates. Detailed-mode
  spatial suppression deliberately retains the oracle's different width versus
  width-minus-one coordinate conventions. Scratch proposals reserve limit space.
  Soft detection remains gated by `detailed and soft`; suppress-grain only changes
  its optional noise threshold.
* Scratch parents are enclosing boxes, but repair paints only their individual
  discs. Scale changes thickness about each disc's center. Union grouping, padding,
  wave dependencies, sequential ordering and nonnegative inpaint clipping match
  the oracle. Texture donors are prepared against immutable original/smooth inputs
  and applied in block order. Full masks retain the oracle's fallback behavior.
* An observed shared-primitive difference was corrected without weakening list
  equality: using `grainy-imgproc::gaussian_blur` made standalone
  `candidates/0` (`soft`, sensitivity 90, tile 96) report score **2.1071** instead
  of **2.1072**, among other last-digit differences. It also made
  `visualize/5` differ by **1.144409e-5**. The authorized private AVX2 arithmetic
  from `grainy-detail/src/cv/sep.rs`, using `grainy-imgproc`'s kernel generator,
  resolves these differences exactly. No tested list or threshold difference
  remains from template matching or distance transform. Texture repair continues
  to use the shared Gaussian, box filter and template matcher; its measured errors
  are documented above.
* The oracle's packed-review `scanning` state becomes `pending`. Target identities
  retain Unicode casefold, Python JSON spacing and sorted unique selection IDs.
  Capture deliberately preserves missing selected IDs while excluding videos
  from folder scans. Source verification checks both edit/path state and file
  signature before accepting a captured record.
* Invalid/nonfinite image data and malformed geometry return `Result::Err`.
  Geometry extents must fit signed 32-bit pixel coordinates, as in OpenCV;
  oversized standalone shapes are rejected before interpolation arithmetic.
  Empty median/quantile inputs and out-of-range texture block indices are rejected.
  Shared inpaint's safety rejection for partially masked single-row/column images
  remains in effect. No unsafe code or runtime foreign-language bridge is used.

## Excluded by the brief

`DustReviewStore`, its transaction/migration/review validation behavior,
`read_work`, `apply_reviewed`, batch undo/redo `restore`, and both Qt dialogs belong
to the service/UI workers. `pack` handles the zlib+JSON data format; catalog profile
asset externalization and review-state validation are service/catalog responsibilities.
Standalone unpack returns the oracle's UTF-8 text and does not resolve database
profile references. There are no other unimplemented functions in the assigned
algorithm files. No full-resolution throughput or universal cross-OpenCV-build
bitwise claim is made beyond these fixtures.

## Dependencies and requests

Uses existing `grainy-core`, `grainy-imgproc`, `grainy-catalog`, `ndarray`, `rayon`,
`serde`, `serde_json`, `thiserror`, and `grainy-testkit` for tests. Additional direct
dependencies are `sha2` 0.10 (MIT/Apache-2.0), `flate2` 1.1 with only `rust_backend`
(MIT/Apache-2.0), and `unicode-casefold` 0.2 (MIT/Apache-2.0). They are pure Rust;
the already-authorized catalog dependency supplies SQLite. No workspace dependency
or other crate was edited, and Cargo.lock is excluded from the commit.

Requests for shared crates:

1. Consolidate the private exact Gaussian arithmetic into `grainy-imgproc` after
   comparing its full suite; dust's metadata requires the AVX2 rounding order.
2. Expose catalog's private `resolved_path`/`path_text` helpers for native
   `Path.resolve(strict=False)` handling. Dust currently keeps a private equivalent
   for `target_key`, while reusing catalog's public Python JSON codec/equality.
