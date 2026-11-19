# grainy-geometry

The geometry stage of the develop pipeline (Python oracle: `luma/engine.py geometry`, `processing.optical_geometry`,
`straighten.py`, `upright.py`, `scan_border.py`, `rasterio.orient_pixels`, the pixel path of `optics.py`).
Frames and masks are `Array3<f32>` H×W×C with 1 or 3 channels; masks go through the same functions.

| Oracle | Rust | Golden cases | Largest error |
|---|---|---|---|
| `engine.geometry` | `geometry(a, s, UseCrop, lens)` | 197 rotation × flip × crop (1/3 channels, odd sizes, degenerate crops), 43 straighten, 15 upright, 2 combined | 0 (bit-exact) |
| PIL `Image.rotate(-angle, BICUBIC)` on `'F'` | `pillow::rotate_bicubic` | in the 43 straighten cases (incl. 0/90/180/270/360 fast paths, padding) | 0 |
| `processing.optical_geometry` | `optical_geometry` | 45 (each key alone, combined, CA, guides, 1/3 channels) | 0 |
| `upright.apply` | `upright::apply` | 15 (crop/no crop, zoom limits, invalid matrices) | 0 |
| `upright.estimate` after LSD | `upright::estimate_from_lines` | 76 (4 modes × real LSD segments and chosen segments, errors) | 2.1e-15 (matrix) |
| `_roll`, `_horizon_roll`, `_plumb_rolls`, `_wide_roll`, `_level`, `_vanishing`, `_points`, `_crop_scale` | `upright::*` | 18 segment sets, 12 crop scales | 9.1e-15 |
| numpy `default_rng(42).choice(n, 500, replace=False)` | `rng::choice` | 7 (Floyd and tail-shuffle paths) | exact |
| `straighten.*` | `straighten::*` | 78 angle×size records × aspects/crops | 0 |
| `scan_border.detect`, `crop_for` (from the linear frame) | `scan_border::detect`, `crop_for_frame` | 10 + 2 | exact |
| `rasterio.orient_pixels` | `orient_pixels` | 20 | 0 |
| `optics.lens_geometry`, `lens_shading`, `_sensor_view`, `_modifier` | `lens::*` | 23 (full map, 128-row strips, orientations, errors) | 0 |

## What callers inject

* **Lens model.** lensfun is not ported. `LensModel` mirrors the three `lensfunpy.Modifier` calls
  (`geometry_coordinates`, `subpixel_coordinates` with `y`/`rows` for the strips, `apply_color`); each call gets the
  `LensRequest` that `_modifier` would build (frame size in the sensor's orientation, conditions, scale, flags,
  and the stored profile with its XML). With `lensfun_enabled` and a stored profile, no model is
  `Error::LensModelMissing`, never a silent skip. Goldens replay lensfunpy's recorded maps.
* **LSD segments.** `estimate_from_lines(width, height, lines, mode)` takes `cv2.createLineSegmentDetector(
  cv2.LSD_REFINE_STD).detect(gray)[0]` as N×4 `f32` (`None` when it found nothing), where `gray` is
  `uint8(clip(to_srgb(image) @ [.2126,.7152,.0722], 0, 1)*255+.5)`.
* **Scan frame.** `crop_for_frame` starts from `rawcolor.linear_frame(engine.resize_float(source, 1200), settings)`.

## Oracle behaviour kept

* Straighten is Pillow's affine bicubic (a = −1) with Pillow's matrix (`round(cos, 15)`), its half-pixel
  centres, edge clamping, zero outside, and the float32 arithmetic of the first `BICUBIC` pass (its arguments are
  `FLOAT32`); 0° copies, 180° and square 90°/270° are exact transposes.
* The manual map is float32 with numpy's weak-scalar rounding (`f32(p/160)*u`, …) and the oracle's evaluation order.
* numpy's 3×3 products use OpenBLAS's FMA order (`fma(a2,b2,fma(a1,b1,a0*b0))`), sums are numpy's pairwise sums,
  `np.histogram` uses its cumulative-weight path, and `_vanishing` samples pairs with numpy's PCG64 `choice`.
* Scan-border band means are float32 pairwise sums in numpy's reduction order (buffers of 8192 for strided views);
  `v < .10` compares in float32.
* `geometry(copy=True)` copies an untouched frame; Rust returns it borrowed (`Cow`), same pixels.

## Known limits

* `numpy.argsort` (default kind) is not stable on this machine (AVX2 sort network); a stable sort is used. Only
  ties can differ: `_wide_roll` evaluates the 12 highest smoothed histogram bins, so equal bins at the 12th place
  could pick another candidate; equal segment lengths beyond 160 segments could order differently. No golden hits it.
* `np.linalg.inv/det/lstsq/svd` are LU, LU, centred weighted least squares and one-sided Jacobi: results agree to
  ~1e-14, not bit for bit. Threshold decisions on such values could in theory flip.
* Inputs Python would reject with numpy's own messages (ragged lists, strings in matrices) give this crate's
  `Error::Value` with the closest oracle message; a missing `crop`/`focal` in a lens profile (Python `KeyError`)
  gives the profile-conditions error.
