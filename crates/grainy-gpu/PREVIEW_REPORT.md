# GPU preview integration (work package 27)

Branch: `port/app-gpuprev`.

## Implementation

- `luma/engine.py develop(cache=...)` -> feature-gated
  `grainy_engine::gpu::develop_gpu_cached` and `GpuCache`. RAW, negative, retouch, lens shading and geometry
  use the existing CPU implementations and keys. Source token, cache version, geometry, crop mode and image
  shape bind resident frames. Exposure invalidates tone onward; colour edits reuse tone; detail edits reuse
  tone and colour; grain edits only dispatch finish; mask edits reuse the device prefix.
- `native_gpu.develop_resident` -> `Gpu::develop_resident_prefix`. It returns the first unsupported stage and
  the supported resident prefix. The existing all-or-none convenience API remains available. Grain is added
  exactly once. A new finish parameter delays clipping until after CPU masks.
- `native_gpu.set_enabled/status`, `app.set_gpu/update_gpu_action` -> `gpu_worker` plus the worker option.
  The production constructor installs the GPU worker, compiles shaders on its background thread, reads the
  Rust app's `use_gpu` preference, and honors `GRAINY_GPU=0` / `warp`. The existing CPU constructors remain
  deterministic reference renderers. The settings action rerenders the current request and writes only the
  explicit Rust app settings path; `--memory` does not read or write preferences.
- Adapter absence, initialization failure and device loss use CPU rendering. A GPU failure retries the same
  request on the CPU; a lost context and resident frames are discarded. Input/resource limits fall back for
  that request without disabling the adapter. Thumbnails and exports retain their existing CPU path.

## Precision decisions and fallbacks

The stage implementations and existing CPU `develop` / `develop_cached` functions and tests are unchanged.
The integration reuses existing fixtures; no golden or generator was changed.

Known film looks, DRT, unsupported tone curves, HDR, enabled photo filters, unsupported colour parameters,
and detail radius/scale limits retain the GPU crate's fallback policy. Unsupported stages and all successors
execute on the CPU. Their input is also replayed through cached CPU prefix stages so the **entire fallback
result is bit-exact**. The first GPU prefix remains resident for a later supported edit. This is deliberate:
feeding the GPU's tone pixels directly to the CPU colour/detail tail produced `1.12235546e-4` error in
`all_garden`, despite the upstream tone error being small. No readback is needed for this canonical replay.

Nonidentity output ICC transforms also use the canonical CPU chain. LittleCMS table quantization amplified
GPU rounding to `1.22070312e-4` in the initial `mask_wide_p3` experiment. Returning working-space pixels still
supports the GPU in ProPhoto. Output ICC conversion and HDR are reported as a fallback beginning at tone.
The strict integration tolerances are not relaxed to accommodate either finding.

There is no GPU local stage. Every mask (shape, range, brush, auto brush and sky) uses the existing CPU local
stage after GPU detail/grain readback, before the final clip. A regression fixture explicitly distinguishes
this ordering from the incorrect early-clipping result.

## Display boundary and remaining work

Both eframe 0.37 and grainy-gpu resolve to wgpu 30.0.2. This integration deliberately retains the current
readback -> CPU histogram/clipping/RGBA conversion -> egui upload boundary. It does **not** claim zero-copy
display or a shared device. The current `Renderer::Frame` contract carries CPU bytes, and `Gpu` constructs
and owns its compute device. Sharing requires passing eframe's `RenderState` through startup, requesting
the compute/storage limits at device creation (including 24 MP binding sizes), a constructor for borrowed
device/queue ownership with coordinated loss handling, RGB-storage-to-display-texture conversion and egui
texture lifetime management. GPU histogram/clipping reductions would be needed to remove the CPU frame
dependency. Those touch display/view/startup contracts beyond the small renderer hooks in this package.

Remaining: shared-device display, GPU local masks, compact GPU noise-statistic reductions (the existing GPU
crate still performs host float64 reductions for luminance denoising). These are performance opportunities;
their CPU/readback paths are implemented and checked here.

## Shared hooks and dependencies

- Engine: optional local `grainy-gpu` dependency, `gpu` feature, and gated `pub mod gpu` only.
- App: enables that feature and adds local `grainy-gpu`; `lib.rs` registers `gpu_worker`; `app.rs` adds one
  production-startup call; `menus.rs` delegates its existing GPU entry; `render.rs` adds two default renderer
  methods; `worker.rs` adds optional GPU state, startup, toggle rerender, per-request fallback and diagnostics.
- GPU: resident-prefix result types/API and the finish shader's clip switch.
- No new third-party dependencies, unsafe code, shared pipeline helpers or shared-crate requests.

## Reproduction

Use generated inputs only. Add the user's Cargo bin directory to PATH.

```text
cargo fmt --check
cargo clippy -p grainy-gpu -p grainy-engine -p grainy-app --all-targets --all-features -- -D warnings
cargo test -p grainy-gpu -p grainy-engine -p grainy-app --all-features
cargo test -p grainy-engine --features gpu --lib gpu::tests::engine_matrix_matches_cpu_worker -- --nocapture
cargo test -p grainy-engine --features gpu --lib gpu::tests::timings -- --ignored --nocapture
```

The matrix compares every full frame from `golden/engine/_index` against the CPU worker's cached develop,
including RAW, geometry, masks and output spaces. Tolerance is `1e-5` for tone/colour, `2e-5` with detail or
masks, and zero for fallback. GPU tests skip explicitly on machines without an adapter; shader or device
initialization failures with an adapter are errors. App tests compare displayed frames (at most one RGBA8
quantization step), exact CPU-toggle results and exact thumbnails. An injected loss transition verifies
resident release and CPU selection; it does not pretend to physically disconnect a GPU.

## Measured pixel results

NVIDIA GeForce RTX 3070, all 139 engine fixture settings, full frames (not the subsampled stored
oracle images). The CPU comparison uses `develop_cached`, which is also the reference worker's entry point.

| Case group | Cases | Maximum absolute error against CPU |
|---|---:|---:|
| RAW on GPU | 9 | 1.19209290e-7 |
| Tone/colour and other supported combinations | 43 | 1.01327896e-6 |
| Detail | 8 | 4.07546759e-6 |
| Geometry/retouch/lens | 25 | 5.36441803e-7 |
| CPU masks after GPU detail/grain | 19 | 4.38094139e-6 |
| Negative | 4 | 1.78813934e-7 |
| Colour fallback | 3 | 0 (exact) |
| Tone/output-conversion fallback | 28 | 0 (exact) |

Additional checks cover source/crop invalidation, unchanged requests, exposure/colour/detail/grain/mask
slider reuse, support/fallback transitions, output-space changes, and a zero-byte host-cache budget.
Mask-only edits also reuse the cached, unclipped host detail/grain result, as in the oracle's
`resident_layers is None` branch: the regression test asserts both unchanged GPU dispatch counts and the host
cache hit. Initial mask activation still requires readback; later mask edits do not.
The clipping-order regression is exact on this adapter and detects the incorrect early-clip implementation.
Core normalization currently turns HDR off, as in the dormant oracle feature; the engine's extended-range
fallback is retained for when that feature is enabled, and the GPU crate separately tests HDR support limits.

## Measured timings

Milliseconds, median of three fresh-cache sequences on the RTX 3070, Cargo's test/dev profile (`opt-level=1`,
debug info). Brackets give the complete observed range. Concurrent workstation builds caused substantial
variation; these are observations under that load, **not stable latency targets or a clean-machine speedup
claim**. The first run had two other `rustc` processes active. No other worker or process was stopped.

Timing includes develop, source upload on the first GPU request, and final readback. It excludes image decode,
GPU/device/shader initialization, histogram/RGBA conversion and egui texture upload. Sources are generated,
2048x1365 and 6000x4000, with original size 6000x4000. The sequence is defaults, exposure 0.4, vignette -25,
adding a linear mask with exposure -0.5, then changing that mask to -0.25. Every measured frame is also compared
in full against the CPU: maximum error `4.172325e-7`, tolerance `2e-5`.

| Source | Edit | CPU median [min..max] ms | GPU median [min..max] ms |
|---|---|---:|---:|
| 2048x1365 | First render | 409.50 [84.74..712.15] | 224.98 [57.28..424.34] |
| 2048x1365 | Exposure | 357.65 [159.61..501.07] | 118.88 [18.76..130.32] |
| 2048x1365 | Late stage (vignette) | 100.90 [97.55..220.95] | 131.32 [38.26..226.78] |
| 2048x1365 | Add mask | 300.07 [84.65..772.97] | 641.94 [168.54..803.32] |
| 2048x1365 | Edit existing mask | 268.53 [134.51..279.73] | 255.22 [130.51..317.24] |
| 6000x4000 | First render | 1754.83 [1096.80..3220.88] | 977.56 [733.88..2561.17] |
| 6000x4000 | Exposure | 1137.92 [1027.19..2146.04] | 252.35 [230.86..547.91] |
| 6000x4000 | Late stage (vignette) | 564.91 [492.75..717.98] | 335.49 [210.84..339.24] |
| 6000x4000 | Add mask | 1052.55 [832.25..1754.93] | 1016.48 [779.05..1266.60] |
| 6000x4000 | Edit existing mask | 1072.83 [633.26..1108.11] | 628.46 [616.88..1387.36] |

GPU processing is not faster for every case in this run. Initial mask activation needs the unclipped readback
and host-cache copy, and all local pixels still execute on the CPU. Subsequent mask edits reuse the readback;
the test proves that reuse independently of noisy timing. Device sharing and a GPU local stage remain the
ways to remove these boundaries, rather than changing pixel math or weakening tolerances.

## Final checks

- `cargo fmt --check`: passed, preserving shared files' existing line endings.
- Required all-targets/all-features clippy command: passed with `-D warnings`.
- Required all-features GPU/engine/app test command: **105 passed**, zero failed, two opt-in timing tests
  ignored by the normal suite. The new timing test was then run explicitly three times and passed each time.
- All 19 existing app snapshot tests passed; every existing snapshot PNG is byte-unchanged in Git.
- All 139 engine matrix cases passed; hardware and software GPU comparisons, explicit no-adapter cases,
  device destruction/recovery, prefix fallbacks, cache transitions, CPU toggle and thumbnails passed.
- `cargo tree -p grainy-cli --edges normal`: no `grainy-gpu` dependency in the default CLI build.
- No owner photos, libraries, settings or oracle data were opened. No oracle source, fixture, generator,
  shared pipeline implementation, `BRIEF.md` or `Cargo.lock` is included in this change.
- Local command logs are retained under `target/gpu-preview-validation/` (untracked build artifacts).
