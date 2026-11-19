# grainy-imgproc

Pure Rust tier-1 and tier-2 equivalents of the OpenCV primitives used by Python Grainy.
The numerical oracle is **opencv-python-headless 5.0.0**, NumPy 2.5.3, from
the oracle's virtual environment (`$GRAINY_ORACLE/.venv`, see CONTRIBUTING.md). The library does not link OpenCV or execute Python.
The generator aligns magnitude inputs/output to 64 bytes: OpenCV otherwise
changes its SIMD prefix with allocation addresses, changing some results by
one float ULP between runs. CPU optimizations remain enabled.

## API and support

Most functions accept `&ndarray::Array<T, D>` with either H×W or H×W×C shape
and preserve the rank. `T` is `f32` or `u8` where indicated. Arrays need not
have contiguous storage. Sizes are **(width, height)**, anchors **(x, y)**,
and maps contain absolute source coordinates. Results are newly allocated.
`None` anchors select the center. `Border::default()` is Reflect101.
`cvt_color` accepts H×W×3 (also RGBA for gray) and returns H×W×1 for gray.
Gaussian kernels are one-dimensional arrays, rather than OpenCV's N×1 matrix.

| Oracle function | Rust item | Supported variants |
|---|---|---|
| `GaussianBlur` | `gaussian_blur` | f32 planes/images; automatic `(0,0)` or explicit odd sizes, independent x/y sigma; Reflect101, Reflect, Replicate, Constant |
| `getGaussianKernel` | `get_gaussian_kernel`, `get_gaussian_kernel_f64` | CV_32F/CV_64F; positive sizes including OpenCV's even-size behavior; sigma<=0 small tables and automatic sigma |
| `sepFilter2D` | `sep_filter_2d` | f32 input/output/kernels; symmetric, antisymmetric, asymmetric, sparse wavelet kernels; anchors and delta; same filter borders |
| `boxFilter`, `blur` | `box_filter`, `blur` | f32; normalized/unscaled, odd/even/rectangular sizes, centered or explicit anchors; same filter borders |
| `medianBlur` | `median_blur` | f32 size 1/3/5; u8 odd size 1..255; implicit Replicate |
| `Sobel` | `sobel` | f32; odd sizes 1..31, valid derivative orders, scale/delta; same filter borders |
| `integral` | `integral`, `integral_u8` | f32→f64 and u8→i32; leading zero row/column; i32 accumulation wraps |
| `magnitude` | `magnitude` | f32, equal-shaped inputs; `sqrt(x*x+y*y)` |
| `resize` | `resize` | f32/u8; explicit destination size; Area/Linear/Nearest; shrinking, enlargement, mixed axes, integer/fractional area paths |
| `remap` | `remap`, `remap_interleaved` | f32/u8; separate f32 planes or H×W×2 map; Linear/Nearest; Constant (including per-channel values), Replicate, Reflect, Reflect101, Wrap |
| `warpAffine`, `warpPerspective` | `warp_affine`, `warp_perspective` | f32/u8; f64 matrices, optional inverse-map flag, Linear/Nearest and resampling borders |
| `getRotationMatrix2D` | `get_rotation_matrix_2d` | degree angle, scale, Point2f center; f64 2×3 result |
| `getPerspectiveTransform` | `get_perspective_transform` | four f32 point pairs; default LU, f64 3×3 result; degenerate systems rejected |
| `cvtColor` | `cvt_color` | f32/u8 RGB↔HLS, RGB↔HSV, RGB/RGBA→GRAY; OpenCV hue units, no float clipping |
| `dilate`, `erode`, `morphologyEx` | `dilate`, `erode`, `morphology_ex` | f32/u8; arbitrary nonempty binary kernels, anchors/iterations; erode/dilate/open/close/gradient/top-hat/black-hat; default neutral or explicit borders |
| `getStructuringElement` | `get_structuring_element` | Rect/Ellipse/Cross, odd/even/rectangular sizes, custom anchor |

All source calls and exact expressions are recorded in
[`../../golden/imgproc/oracle_calls.json`](../../golden/imgproc/oracle_calls.json)
(from the workspace root: `golden/imgproc/oracle_calls.json`). The generator
audits `luma/*.py` as text, excluding `selftest.py`. Its standard inputs come
from `common.inputs()`, including the bundled documentation garden example and
the oracle's `to_linear` function. It does not open a catalog, load user settings,
read the owner's photographs, or inspect the oracle's `data/` folder.

### Tier 2 API

The new mask operations accept `Array2<u8>`; inpainting, bilateral filtering,
and distance output use `Array2<f32>`. Contours use `Vec<[i32; 2]>` in (x,y)
order. `contour_area`, `arc_length`, `bounding_rect`, and `is_contour_convex`
also accept float32 coordinates, as required by `studio.py`.

| Oracle function | Rust item | Supported oracle variants |
|---|---|---|
| `inpaint` | `inpaint` | INPAINT_NS, f32 plane, nonzero u8 mask; oracle radius 3, also rounded/clamped radii 1..100 |
| `distanceTransform` | `distance_transform` | DIST_L2, masks 3/5, CV_32F; weights of the installed CPU/IPP reference |
| `connectedComponents` | `connected_components` | 4/8 connectivity, CV_32S, default component order |
| `connectedComponentsWithStats` | `connected_components_with_stats`, `Components` | same labels; LEFT/TOP/WIDTH/HEIGHT/AREA i32 columns and f64 (x,y) centroids |
| `findContours` | `find_contours`, `Contours`, `Retrieval` | u8, RETR_EXTERNAL/RETR_CCOMP, CHAIN_APPROX_SIMPLE, ordered points and hierarchy |
| `contourArea` | `contour_area` | i32/f32, signed or unsigned area |
| `arcLength` | `arc_length` | i32/f32, open or closed perimeter |
| `convexHull` | `convex_hull` | i32, returnPoints, either winding; oracle starting vertex and order |
| `boundingRect` | `bounding_rect` | i32/f32 point sets; [x,y,width,height] |
| `isContourConvex` | `is_contour_convex` | strict convexity for i32/f32 polygons |
| `moments` | `moments`, `Moments` | binary or weighted u8 raster; all 24 f64 spatial/central/normalized values |
| `drawContours` | `draw_contours` | all supplied contours, u8 plane, LINE_8, filled (-1) or thickness 1; even-odd filling |
| `floodFill` | `flood_fill`, `FloodFill` | u8, implicit mask, default 4-connected exact-value fill; area/image/padded mask/rectangle |
| `matchTemplate` | `match_template`, `TemplateMatch` | f32 planes/RGB, TM_SQDIFF/TM_CCOEFF_NORMED, no mask |
| `createLineSegmentDetector(...).detect` | `line_segment_detector` | u8 gray, LSD_REFINE_STD, all default parameters, ordered f32 endpoints |
| `HoughLinesP` | `hough_lines_p` | u8, caller rho/theta/threshold/length/gap; ordered i32 endpoints |
| `bilateralFilter` | `bilateral_filter` | f32 plane, explicit/automatic diameter and color/spatial sigmas, filter border |
| `Laplacian` | `laplacian` | f32 plane/image, default ksize=1/scale=1/delta=0, filter border |
| `absdiff` | `absdiff` | same-shaped f32/u8 planes/images |

These cover every production call in the refreshed inventory. In particular,
the Hough caller is `sky_mask.py`, and the only `drawContours` thickness is -1.
The only `floodFill` call supplies no mask, flags or differences, so a general
tolerance/range-fill API is unnecessary. Caller-side color conversion, mask
padding, resizing, and normalized settings remain with their owning crates.

### Oracle call-site findings

* Gaussian inputs are f32, 1/3 channels; automatic sizes dominate. Explicit
  calls include `(5,5), .8` in `folder_recovery.py` and `(13,13), 1.5` in
  `dust_texture.py`. `processing.blur` clamps sigma to at least .05;
  kernel sizing is performed here, while caller-side clamps stay in callers.
  All observed Gaussian/filter borders are the default/explicit Reflect101.
* `processing._atrous_smooth` uses `[1,4,6,4,1]/16` with 0..4 levels of holes.
  Box filters are normalized with ddepth=-1; `dust_texture.py` uses rectangular
  `(tw,th)` with anchor `(0,0)`. Other boxes are centered `(2*r+1,2*r+1)` or `(5,5)`.
* Float medians are size 5 for Lab chroma and size 3 for RAW chroma residuals.
  Sobel calls use CV_32F, size 3, x/y first derivatives, scale 1 or a pixel-scale
  factor. `magnitude` receives f32. The occupancy integral is u8→CV_32S.
* Resize calls explicitly set destination sizes and Area (downsample), or Linear
  (`tone_response.py` horizontal interpolation). The brief additionally requires
  uint8 and enlargement paths; both are covered. No other interpolation flag
  occurs among these oracle calls. Nearest is provided and tested as an extension.
* Remap calls use f32 planes/images with Linear. Optics/studio/geometry use zero
  Constant, retouch uses Reflect101, and sky coefficients use Replicate.
  **Sky coefficients have five channels** (`sky_mask._apply`), so their remap
  takes a different OpenCV path from ordinary RGB. Optics also supplies a single
  interleaved H×W×2 map. Perspective warps use Linear/zero Constant; the affine
  self-test uses a rotation and `(.6,.6,.6)` Constant.
* HLS in processing, HSV in sky/DCP, HSV→RGB in DCP, and RGB→gray in dust use
  float32. Uint8 conversions are additionally covered. Hue is degrees for float
  and half-degrees for the ordinary uint8 HSV/HLS formats.
* Morphology uses float response planes and uint8 masks, centered anchors, one
  iteration, and OpenCV's neutral constant border. Kernels are Rect, Ellipse,
  all-one rectangles and vertical lines; dust requests top-hat/black-hat.

## Numerical decisions and retained quirks

* **Version matters:** OpenCV 5 uses continuous float coordinates and nested
  linear interpolation for 1/3/4-channel remap/warps. Its other-channel fallback
  still uses a 1/32-pixel table; affine fallback first rounds in 1/1024 units.
  Both paths are implemented. Warp matrices are inverted in f64 then converted
  to f32 on the continuous path, exactly as the oracle does.
* The tested Windows oracle uses AVX2. We reproduce its 16-pixel warp groups
  with explicit `mul_add` and scalar tails, independent of the Rust target's
  vector ISA or Rayon worker count. This is necessary at rounding boundaries.
* Inverse float HLS/HSV has an oracle bug: negative hues in the eight-pixel
  SIMD portion of a row use truncation and do not wrap like the scalar tail.
  The Rust implementation intentionally keeps this width-dependent behavior.
  Normalized caller hues do not hit it. Cross-platform OpenCV builds with
  different vector widths can disagree for these out-of-domain negative hues.
* Gaussian sigma<=0 tables include the non-binomial size-7/9 coefficients.
  Even `getGaussianKernel` sizes force both center weights to one before
  normalization. GaussianBlur itself still requires odd sizes.
* Area enlargement uses OpenCV's area-emulation two-tap coordinates, not the
  half-pixel Linear mapping. Integer 2×2 uint8 area averages round half upward.
  General uint8 Linear resize and RGB→HLS are within **one byte level**; no
  bit-identical claim is made for those variants.
* Float64 box accumulators, nearest-even rounding, saturating u8 subtraction,
  Point2f rounding, derivative kernel scaling order, and even ellipse asymmetry
  are preserved. Float operations do not clamp to 0..1.

### Explicit limits / rejected inputs

Empty/wrong-rank images, invalid sizes/anchors, mismatched maps, unsupported
median sizes, invalid derivative orders, and invalid transform parameters return
`Result::Err`. Maps must be finite and within signed 32-bit coordinate range;
remap/warp source and destination axes must be below 32767, like OpenCV remap.
Colour conversion requires finite components. Image arithmetic otherwise assumes
finite photograph/mask values; NaN ordering in filters is not a compatibility target.

**Degenerate perspective correspondence sets do not port OpenCV 5's SVD
fallback.** That result is nonunique. An all-zero perspective matrix with
Reflect101 stalled the oracle generator until its isolated process was stopped.
Rust rejects degenerate correspondences, singular transforms, projection
horizons crossing the output, and coordinates outside int32 range. The singular
correspondence golden keeps the actual oracle matrix and asserts Rust rejection.
No stalled-oracle call is rerun by the generator.

OpenCV depth-conversion overloads, explicit resize scale arguments (`fx/fy`),
fixed-point input maps, relative maps, transparent borders/destination buffers,
Cubic/Lanczos/LinearExact, Scharr, and morphology hit/miss are outside this API;
none is used by the audited tier-1 calls. The kernel/median size bounds are
documented above. Caller-specific normalization remains outside this crate.

## Verification

See [VALIDATION.md](VALIDATION.md) for every function/variant's measured maximum
and tolerance, and [golden_errors.json](golden_errors.json) for every numeric case.
There are **2,961** tier-1 cases (about 9 MB of array/metadata data),
covering standard generated inputs and their planes, complete standard images,
signed/HDR values, constant fields, tiny and single-axis inputs, odd/even sizes,
sigma .05/.3/2.5/40, resize .5/⅓/.37/2/3.7, borders, anchors, iterations,
inverse maps, SIMD/scalar tails, and five-channel fallback.

Tier 2 adds **14 packed case folders / 863 variants** (2,196 files, about 7.1 MB),
with **953 comparison records** plus per-contour measurements and private LSD
preprocessing checks. Inputs include empty/full/single-pixel/border masks,
holes and nested islands, checkerboard ties, many components, random masks,
signed textures, sparse and crossing lines, gaps, curves, antialiased lines,
and textured line drawings. LSD checks 393 segments; Hough checks 11,643.
`--tier2` regenerates just these additions and the full call-site inventory.
Every new result is loaded through `grainy-testkit`. Exact integer/structural
equality is required. Inpainting uses zero tolerance after matching the
oracle's float32 square root. See `golden_errors_tier2.json` for all results.

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B tools/golden/gen_imgproc.py
cargo fmt --all -- --check
cargo clippy -p grainy-imgproc --all-targets -- -D warnings
cargo test -p grainy-imgproc -- --nocapture --test-threads=1 > crates/grainy-imgproc/validation.log 2>&1
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" -B crates/grainy-imgproc/tools/summarize_goldens.py
```

The 39 Rust tests also cover exact identities, strided owned arrays, invalid
inputs, and equal results using one/four Rayon workers. Row/band work uses
`grainy_core::parallel::bands`; double sliding box sums and deque rectangle
morphology avoid kernel-area work. Small median windows use stack storage.
General ellipse/sparse morphology remains O(pixels × active kernel elements),
and Gaussian remains separable O(pixels × kernel width). No 24 MP throughput
claim is made; further SIMD/large-kernel optimization needs its own benchmark
and these goldens.

## Tier 2 decisions, retained quirks, and limits

* NS retains the source's asymmetric border-gradient indices, absolute gradient
  differences, `abs(dot)<=.01` fallback, stable fast-marching insertion ties,
  and float32 square root/division followed by float64 final averaging.
  A fully marked image has no starting band and remains unchanged, as in cv2.
  Partially marked single-row/column images return `Err`: the oracle NS stencil
  reads outside its source on these inputs. Empty/full masks on these shapes
  remain exact identities. This safety exception is outside the audited callers.
* The 8-connected default labels visit 2x2 blocks; 4-connected labels use raster
  order. An absent background has stats `[-1, INT_MAX, 0, 0, 0]` and NaN centroids.
  Zero-free distance masks return `FLT_MAX`, preserving the installed IPP path.
  Mask-3 weights are .955/1.3693; mask-5 weights are 1/1.4/2.1969. Float summation
  order can differ from IPP; the largest measured distance error is 1.984e-4.
* Contour scanning uses unique internal border IDs instead of wrapping byte
  labels, retaining the same border tracing, sibling insertion, point and
  hierarchy order. Hulls retain Sklansky ordering and cyclic index adjustment.
  Collinear/repeated polygon edges are not strictly convex. Raster drawing uses
  fixed-point scan conversion and clipping; wide intermediates prevent overflow.
* LSD preserves the byte Gaussian kernel quantization/error diffusion,
  INTER_LINEAR_EXACT .8 sampling, polynomial fastAtan2, float32 region angle
  accumulation, stable gradient ordering, density refinement and swap-removal.
  The tested endpoints, counts and order are exact. Hough preserves OpenCV's
  local RNG seed and fixed-point walk, including clearing rejected short lines.
* Template matching retains the float32 cross-correlation intermediate and
  OpenCV's variance/near-unit guards. Constant templates give ones for normalized
  correlation, including a one-pixel template. Its direct correlation sums in
  float64, so rounding can differ from the oracle's IPP/DFT route. It is
  O(output pixels x template pixels x channels); no large-search speed claim is
  made. Bilateral filtering retains the range LUT, circular support and center
  contribution, with scalar summation instead of the oracle SIMD grouping.
* Unsupported, unused overloads include Telea/RGB inpaint, precise/labeled
  distance transforms, other contour retrieval/approximation modes, float hulls,
  contour moments, explicit flood-fill masks/ranges/flags, thick/antialiased
  drawing, other template methods/masks, custom LSD parameters/refinement, RGB
  bilateral filtering and nondefault Laplacian kernels. They return errors when
  representable as invalid arguments or are absent from the typed API.
* Numeric bounds are measured on these goldens, not a proof for arbitrary image
  size or magnitude. Distances and unnormalized correlation can accumulate more
  rounding at larger scales. The library rejects invalid shapes/parameters,
  nonfinite inputs where used for ordering/indexing, and excessive Hough grids.

## Scope, dependencies, and requests

Dependencies: existing workspace `grainy-core`, `ndarray`, `rayon`, `thiserror`;
dev-only `grainy-testkit` and `serde_json`. No new third-party dependency or
workspace edit. No unsafe code. No changes to shared crates, the oracle, or
real user data. **Requests:** no shared-crate API change is needed. The private
LSD preprocessing implements u8 Gaussian and INTER_LINEAR_EXACT resize; the
orchestrator may promote these to shared primitives if another crate needs them.
All other wave-2 callers can reuse the exported operations directly.

Source references and licensing are in [NOTICE](NOTICE).
