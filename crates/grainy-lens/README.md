# grainy-lens follow-up report

The four original `ptlens` exceptions are resolved, and the engine's lens cases
are substantially closer to the oracle. **The request to recover automatic
scaling for every case is still incomplete.** The stronger synthetic sweep has
documented failures. Passing the ordinary regression suite does not establish
universal automatic-scale parity.

Only public documentation, XML calibration data, and numeric calls to the
oracle's lensfunpy were used. No lensfun or binding implementation source was
read, opened, searched, or translated. The oracle and engine goldens were left
unchanged. All inputs are generated; no real library or settings were accessed.

## Changes

| Oracle operation | Rust item changed |
| --- | --- |
| `optics._modifier` automatic scale | Private `model::auto_scale`, `Prepared::new` |
| `Modifier.apply_geometry_distortion` | `ProfileModel::geometry_coordinates`, `Prepared::row` |
| `Modifier.apply_subpixel_geometry_distortion` | `ProfileModel::subpixel_coordinates`, `Prepared::row` |
| `Modifier.apply_color_modification` normalization | `ProfileModel::apply_color` via `Prepared::new` |

No new public API, runtime dependency, native library, or `unsafe` code was added.
Requests for shared crates: **none**. Database/profile/interpolation/shading APIs
from the original port remain covered by their existing goldens. XML data
provenance and attribution remain in [NOTICE](NOTICE).

## Engine comparison

Maximum absolute pixel error, 0..1, against the unchanged engine goldens:

| Case | Before | After |
| --- | ---: | ---: |
| `lens_all` | 4.207e-4 | 4.202e-6 |
| `lens_distortion_only` | 3.290e-4 | 3.844e-6 |
| `lens_no_scale` | 4.053e-6 | 9.179e-6 |
| `lens_vignette_only` | 8.345e-7 | 8.345e-7 |
| `lens_portrait` | 3.517e-6 | 3.338e-6 |
| `lens_edits_masks` | 1.552e-3 | 9.984e-6 |

`LENS_TOLERANCE` could be tightened from `2e-3` to **`2e-5` for the existing
engine fixtures**. The engine and its tests were not edited. This recommendation
is not a guarantee for the stronger synthetic profiles below. The scale-off
case's small increase is retained explicitly in the table.

## Measurements and derived rule

`gen_lens.py` identifies the chosen scale by matching the automatic modifier's
maps to explicit-scale modifiers at three independent off-centre rays. It
bisects the explicit scale, searches adjacent float32 values, and retains all
values with identical observations. This accounts for reciprocal rounding: an
output-distance ratio alone cannot undo nonlinear distortion. The Rust solver
is never involved in generating the references.

The fitted solver uses normalized frame half-extents derived from the
`(width-1, height-1)` diagonal, calibration aspect, and crop ratio. It probes
axis extrema and corners, represents the angle and radius as float32, evaluates
trigonometry in float64 and the distortion callback in float32, and makes a
forward finite-difference Newton step of `0.0001`. The perturbed radius is
rounded to float32 **before** evaluating the derivative. The fitted stopping
condition is a `0.001` frame-coordinate residual. The implementation has a
50-step defensive bound; the native iteration limit has not been established.
Angles are represented around the positive full circle. Final scale selection
and the returned radius are float32.

An active correction adds a `1.001` margin. Distortion with TCA adds another
margin; that second multiplication is **float32**, not float64. TCA coefficients
do not influence this margin in the measured cases, and vignetting does not
change it. These are observations of the compatibility oracle, not new optical
rules. The exact native probing/iteration procedure remains unproven, as the
stress failures demonstrate.

The `ptlens` gaps were resolved by normalization and coordinate stepping, without
changing the forward distortion polynomial or introducing an inverse iteration.
The fitted normalization rounds the squared ratio
`(calibration_aspect² + 1) / ((width-1)² + (height-1)²)` to float32 before taking
its float64 square root, multiplying by two, and applying the crop ratio.
Each coordinate is float32; grid advancement adds a **float64 step**, then rounds
the result back to float32. Rounding the step first changed the bin of the
discontinuous radius estimator. This arithmetic is an empirical fit, not a claim
that the inaccessible native implementation has been proven identical. The native
lane-pairing quirk and measured 4096-entry estimator table are preserved.
Single-pixel scalar probes cover radius zero, both sides of the implicated
boundary, and radii beyond the usual frame range. The focused
[coordinate experiment](research/coordinate_precision.py) reproduces this
comparison with public native calls and the generated profiles.

## Verification and remaining gaps

- Original 110 map cases: all pass without the four former named-case caps.
  Coordinate tolerance is `2e-4` px (float32 arithmetic); developed-pixel tolerance
  is now `1e-5`, down from `1e-4` and the former Sony exception of `2.7e-4`.
  The original Nikon cases are at `3.0517578e-5` px; Sony is at `7.6293945e-6` px.
- Direct automatic scales: **310 profile/flag cases**, maximum **one float32
  ULP (`1.1920929e-7`)**, with an asserted one-ULP limit. These cover the seven
  original profiles plus the engine profile, focal interpolation/clamping,
  crop changes, sensor aspect, portrait orientation, image size, distortion,
  TCA, and vignetting flags.
- All **310 profile map cases** pass a separate regression test at `1e-4` px,
  or `2e-4` for dimensions above 1024 pixels; maximum `1.2207031e-4` px.
- Expanded map audit: **940 cases**, including **630 stronger synthetic cases**.
  It checks sparse geometry and RGB maps against native results and runs a
  separate explicit-scale control. It deliberately returns failure when the
  stricter `1e-4` px target (or `2e-4` above 1024 pixels) is missed.
- **62 strict audit misses remain**, all among the synthetic cases, maximum
  `2.5375366e-2` px. The explicit-scale controls for all 630 synthetic cases
  stay below `9.0599060e-5` px: the remaining gap is in automatic scaling.
  Neither these cases nor their failures are removed from the generated data.
  Exact native probe/iteration behavior remains unexplained; the eight-ray
  Newton rule above is a fit with known counterexamples, not a complete recovery.
- Existing coefficient interpolation (`1e-5` tolerance), gains/shading (`2e-4`),
  remapping (`1e-4`), exact JSON comparisons, exact default no-ops, bad-input
  handling, and one/four-thread equality are retained. Per-function maxima are
  in [verification.json](verification.json); per-case strict audit errors are in
  [scale-audit.json](scale-audit.json).

Required checks pass: `cargo fmt --all -- --check`,
`cargo clippy -p grainy-lens --all-targets -- -D warnings`,
`cargo test -p grainy-lens` (2 unit and 8 integration tests), and the full
`cargo test -p grainy-engine`. The strict diagnostic example intentionally
exits 1 for the 62 documented failures; it is not represented as a passing test.

`golden/lens` is **8,171,856 bytes**, below 9 MB. The generator changes only
`golden/lens` and this crate's existing XML extract/radius table. No other crate,
workspace manifest, `BRIEF.md`, or `Cargo.lock` is part of this change.

## Reproduce

From the worktree root in PowerShell:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:PYTHONDONTWRITEBYTECODE = '1'
& "$env:GRAINY_ORACLE/.venv/Scripts/python.exe" tools/golden/gen_lens.py
cargo fmt --all -- --check
cargo clippy -p grainy-lens --all-targets -- -D warnings
cargo test -p grainy-lens
cargo test -p grainy-engine
# Separate strict accuracy audit: currently exits 1, as documented above.
cargo run -p grainy-lens --example scale_audit -- crates/grainy-lens/scale-audit.json
```

The public [Modifier documentation](https://lensfun.github.io/manual/v0.3.2/structlfModifier.html)
describes automatic scaling as approximate and does not specify probe locations,
iteration details, tolerance, or safety margin. The
[correction-order documentation](https://lensfun.github.io/manual/v0.3.2/corrections.html)
explains forward pixel lookup, and the
[published models](https://lensfun.github.io/manual/v0.3.2/group__Lens.html)
describe the radial formulas. Numerical details above come from black-box
measurements, not undocumented implementation claims.
