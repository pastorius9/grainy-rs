# GPU follow-up report — 2026-11-13

Branch: `port/gpu`; new work on top of `5a72748`, without rewriting history, merging or pushing.
The accepted port's shaders and pixel arithmetic are unchanged. `BRIEF.md` and `Cargo.lock` are excluded.

## Fixtures and CPU references

`golden/gpu`: **49,712,914 bytes / 1,232 files → 156,552 bytes / 13 files** (99.69% smaller, below 3 MB).
Deleted obsolete files with `git rm`, including the tracked `verification.txt`. No expected image arrays remain.
Regenerating with the oracle interpreter reproduces every remaining file byte for byte (SHA-256 comparison).

- `index/case.json`: oracle defaults once, deduplicated setting changes, case descriptors, support decisions,
  and tone/colour matrices. The defaults are checked against `grainy_core::defaults`.
- `inputs/wheel.npy` and its metadata: one 24×45 custom hue wheel from the colourgrade fixtures, preserving
  the original float32 HSV boundary pixels and scalar tails. Standard, negative, tiny, row, column, encoded
  and tiled inputs are now loaded or constructed in Rust.
- `gaussian/case.json` and nine small kernel arrays: oracle `native_gpu._gaussian` taps. The exact kernel
  helper in `grainy-detail` is private, so these still independently check the GPU's private tap generator.

Both adapters exercise 81 tone, 169 colour, 28 grain, 14 basic-chain, 13 fallback, **276 detail** and **7 complete
detail-chain** cases. Added the ten process-2 bilateral cases and four missing standard-input complete chains.
The 44 tone + 122 colour settings run on seven standard images and a 1024×683 tiled garden: **1,328 matrix
comparisons per adapter**. All six large detail cases now compare every pixel, replacing stride-17 sampling.
`grainy-testkit` loads the metadata/inputs and measures errors. A CPU-only test enforces the 3 MB fixture limit.

| Oracle function → GPU item | Live Rust reference |
|---|---|
| `native_gpu.tone` → `Gpu::tone`, `tone_frame` | `grainy_tone::develop_tone` |
| `native_gpu.color` → `Gpu::color`, `color_frame` | `grainy_colorgrade::develop_color` |
| `native_gpu.detail` → `Gpu::detail`, `detail_frame` | `grainy_detail::develop_detail`, grain disabled |
| Resident grain/clip → `grain_clip` | `grainy_effects::settings_grain`, `add_grain`, final clip |
| `native_gpu.develop_resident` / `develop` → corresponding methods | CPU tone → colour → detail (including grain once) → clip |

The complete-chain reference is the stage sequence used by `grainy_engine::develop` for working pixels.
This avoids engine normalization or pre-tone geometry at the GPU's already-normalized, post-geometry API
boundary. There is no output colour conversion. No runtime dependency on `grainy-engine` is needed.

## Measured maximum absolute errors

NVIDIA GeForce RTX 3070 / DX12, driver 32.0.16.1088; Microsoft Basic Render Driver / DX12 forced fallback (WARP).
These are maxima against the **live Rust CPU path**, including the full setting matrices on both adapters.

| Stage | RTX 3070 | WARP | Tolerance |
|---|---:|---:|---:|
| Tone | 1.072883606e-6 | 6.556510925e-7 | 1e-5 |
| Colour | 7.569789886e-6 | 4.649162292e-6 | 1e-5 |
| Detail | 1.398986205e-5 | 8.128583431e-6 | 2e-5 |
| Supplied grain + clip | 0 | 0 | 0 |
| Basic tone/colour/grain chain | 4.768371582e-7 | 4.768371582e-7 | 1e-5 |
| Complete chain including detail | 2.324581146e-6 | 3.453344107e-6 | 2e-5 |

Upload/readback, inactive colour/detail defaults and Gaussian taps are exact (tap tolerance 1e-7).
Tone defaults encode sRGB and are not a no-op. Hardware-versus-WARP complete-chain error: **3.185123205e-6**.
Per-case error and tolerance are printed by the tests; generated logs are not committed. Metal was not exercised.

### Why some detail comparisons increased

Before deleting the old fixtures, compared GPU, Rust CPU and Python oracle using the **same saved input pixels**.
Only the following case maxima increased when switching reference (all remain below 2e-5):

| Case | Adapter | Against old oracle | Against Rust CPU |
|---|---|---:|---:|
| ramp / bilateral | RTX | 5.364418030e-7 | 6.556510925e-7 |
| patches / bilateral | RTX | 6.133690476e-6 | 7.711350918e-6 |
| patches / bilateral | WARP | 3.850553185e-6 | 4.045665264e-6 |
| edges / bilateral | WARP | 7.450580597e-7 | 8.344650269e-7 |
| column / combo | RTX | 1.650303602e-6 | 1.903623343e-6 |
| column / combo | WARP | 2.084299922e-6 | 2.909451723e-6 |

1. Legacy bilateral: `grainy-detail/src/cv/bilateral.rs` reproduces OpenCV's non-IPP filter, while the oracle
   uses Intel IPP. Their float32 rounding differs; measured CPU/oracle RGB error was at most 4.620524123e-6
   in these fixtures. The GPU implementation is unchanged.
2. Degenerate wavelets: `grainy-detail/src/cv/sep.rs::sep_filter` skips any axis of length one, including
   `sepFilter2D` wavelets. OpenCV's `sepFilter2D` still evaluates those taps, unlike `GaussianBlur`'s shortcut.
   A temporary in-memory oracle diagnostic replaced only the length-one kernels with `[1]`: CPU/oracle error
   became **exactly zero** for column/combo, column/wavelet and tiny/wavelet, from 8.251518011e-7,
   5.662441254e-7 and 1.192092896e-7 respectively. No oracle files were modified. This is a CPU-reference
   quirk, left unchanged under crate ownership; the GPU keeps the accepted arithmetic.

## Buffer reuse and 24 MP benchmark

Per-device, exact-size output and readback pools replace repeated full-frame allocation. The output lease is
shared by `Frame` clones and returns only after the last clone drops. Already-submitted work precedes any reuse
on the same queue; no synchronization is added. Staging returns only after successful mapping/copy/unmapping.
Failures discard staging buffers, and concurrent readbacks obtain independent buffers. Cache retention is bounded:
outputs ≤1 GiB / 16 idle buffers; staging ≤512 MiB / 2; oldest entries are evicted. Oversized buffers are discarded.
Source uploads, small parameter tables and live caller-retained frames are outside these idle pools.

Tests verify stable allocation counts after warm-up, bounded retention, exact-size reuse, retained-frame pixels,
queued slider changes without waits, concurrent readbacks, two residents and image-size replacement. Device-loss
and foreign-context checks remain. The existing benchmark source is unchanged for the before/after comparison.

6000×4000 generated float32 RGB, RTX 3070 DX12, release build; exposure +0.5, saturation +20, vibrance +15,
no NR/local tone maps. Five warm upload/compute/readback samples and seven changes per resident slider, medians
in milliseconds. Both runs were made in this session without concurrent parity tests; compilation is excluded.

| Operation | Before pooling | After pooling |
|---|---:|---:|
| Upload | 68.575 | 77.850 |
| Tone + colour | 45.078 | 35.435 |
| Readback | 94.671 | 69.205 |
| Complete upload → tone → colour → readback | 216.395 | 192.054 |
| Resident exposure, GPU complete | 50.941 | 21.081 |
| Resident exposure, with readback | 155.250 | 99.396 |
| Resident saturation, GPU complete | 34.255 | 6.569 |
| Resident saturation, with readback | 135.570 | 82.125 |

These include synchronization and host copying, not display presentation; they are not comparable to the
previous Python on-screen figure of approximately 69 ms. Luminance NR statistics still require host readbacks.
Resident latency including readback fell by 36.0% for exposure and 39.4% for saturation in these samples.
Source upload is unchanged; its slower measurement illustrates run-to-run variability, not a pooling gain.

## Verification, dependencies and requests

- Passed `cargo fmt -p grainy-gpu` and workspace-wide `cargo fmt --check`.
- Passed `cargo clippy -p grainy-gpu --all-targets -- -D warnings`.
- Passed the final unfiltered `cargo test -p grainy-gpu`: **5 unit + 11 integration tests**, no failures.
  Both hardware and WARP were exercised. Empty-backend initialization returns `None` for both policies;
  adapter-dependent tests retain their explicit skip path. The detailed measured run also passed.
- Existing workspace warning: unused `workspace.package.repository`; shared files were not edited.
- Added only a **dev dependency** on the existing `grainy-detail` crate. No new external dependency. Existing
  wgpu 30.0.2 and pollster 0.4.0 remain MIT/Apache-2.0; bytemuck remains MIT/Apache-2.0/Zlib. No unsafe was added.
- Preserved the accepted port's CPU fallbacks, border handling, float32 operation order, curve/HLS quirks and
  deterministic grain. Film/DRT/photo-filter GPU shaders, local adjustments, optical geometry and app display
  remain outside this follow-up; their fallback behavior is unchanged.
- **Requests:** have the detail owner review the length-one `sepFilter2D` shortcut; expose exact Gaussian taps
  and noise-profile helpers if consolidating private implementations. A GPU display bridge and small GPU
  statistics reductions remain future performance work; output/readback pooling is now complete.
