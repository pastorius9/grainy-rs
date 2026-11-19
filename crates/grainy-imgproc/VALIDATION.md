# Oracle validation

OpenCV 5.0.0, NumPy 2.5.3. 2961 committed cases: 2960 numeric comparisons and 1 deliberate invalid-input rejection.

CPU features: `SSE SSE2 SSE3 *SSE4.1 *SSE4.2 *AVX *FP16 *AVX2 *AVX512-SKX?`. Optimizations and IPP remain enabled; OpenCL is disabled.

All errors are maximum absolute differences in native output units. Uint8 errors are byte levels; HSV/HLS hue errors are degrees. Numbers below retain the test log precision. Every numeric case and its individual tolerance is recorded in `golden_errors.json`.

| Rust function | Variant | Cases | Maximum error | Tolerance ceiling |
|---|---|---:|---:|---:|
| blur | float32, border=constant | 1 | 0 | 1e-06 |
| blur | float32, border=reflect | 1 | 0 | 1e-06 |
| blur | float32, border=reflect101 | 1 | 0 | 1e-06 |
| blur | float32, border=replicate | 1 | 0 | 1e-06 |
| box_filter | float32, border=constant, normalize=True | 1 | 0 | 1e-06 |
| box_filter | float32, border=reflect, normalize=True | 1 | 0 | 1e-06 |
| box_filter | float32, border=reflect101, normalize=False | 120 | 0 | 0.0002 |
| box_filter | float32, border=reflect101, normalize=True | 121 | 0 | 1e-06 |
| box_filter | float32, border=replicate, normalize=True | 1 | 0 | 1e-06 |
| cvt_color | float32, code=HLS2RGB | 22 | 4.768e-07 | 3e-06 |
| cvt_color | float32, code=HSV2RGB | 22 | 0 | 3e-06 |
| cvt_color | float32, code=RGB2GRAY | 18 | 1.192e-07 | 6e-05 |
| cvt_color | float32, code=RGB2HLS | 17 | 3.052e-05 | 6e-05 |
| cvt_color | float32, code=RGB2HSV | 17 | 3.052e-05 | 6e-05 |
| cvt_color | uint8, code=HLS2RGB | 16 | 0 | 1 |
| cvt_color | uint8, code=HSV2RGB | 16 | 0 | 1 |
| cvt_color | uint8, code=RGB2GRAY | 17 | 0 | 1 |
| cvt_color | uint8, code=RGB2HLS | 16 | 1 | 1 |
| cvt_color | uint8, code=RGB2HSV | 16 | 0 | 1 |
| gaussian_blur | float32, border=constant | 1 | 2.98e-08 | 2e-06 |
| gaussian_blur | float32, border=reflect | 1 | 8.941e-08 | 2e-06 |
| gaussian_blur | float32, border=reflect101 | 315 | 3.338e-06 | 5e-06 |
| gaussian_blur | float32, border=replicate | 1 | 1.49e-07 | 2e-06 |
| integral | float32 | 30 | 0 | 1e-12 |
| integral_u8 | uint8 | 12 | 0 | 0 |
| get_gaussian_kernel / _f64 | float32, precision=f32 | 50 | 0 | 1e-07 |
| get_gaussian_kernel / _f64 | float64, precision=f64 | 50 | 1.11022303e-16 | 1e-15 |
| magnitude | float32 | 30 | 4.768e-07 | 1e-06 |
| median_blur | float32 | 90 | 0 | 0 |
| median_blur | uint8 | 48 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=blackhat | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=blackhat, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=blackhat, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=blackhat, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=blackhat, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=close | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=close, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=close, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=close, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=close, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=dilate | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=dilate, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=dilate, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=dilate, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=dilate, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=erode | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=erode, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=erode, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=erode, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=erode, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=gradient | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=gradient, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=gradient, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=gradient, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=gradient, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=open | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=open, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=open, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=open, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=open, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=tophat | 36 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=tophat, border=constant | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=tophat, border=reflect | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=tophat, border=reflect101 | 3 | 0 | 0 |
| erode / dilate / morphology_ex | float32, operation=tophat, border=replicate | 3 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=blackhat | 36 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=close | 36 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=dilate | 36 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=erode | 36 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=gradient | 36 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=open | 36 | 0 | 0 |
| erode / dilate / morphology_ex | uint8, operation=tophat | 36 | 0 | 0 |
| get_perspective_transform | float64 | 2 | 0 | 1e-10 |
| remap | float32, interpolation=linear, border=constant | 15 | 0 | 1e-06 |
| remap | float32, interpolation=linear, border=reflect | 10 | 0 | 1e-06 |
| remap | float32, interpolation=linear, border=reflect101 | 15 | 0 | 1e-06 |
| remap | float32, interpolation=linear, border=replicate | 15 | 0 | 1e-06 |
| remap | float32, interpolation=linear, border=wrap | 10 | 0 | 1e-06 |
| remap | float32, interpolation=nearest, border=constant | 15 | 0 | 1e-06 |
| remap | float32, interpolation=nearest, border=reflect | 10 | 0 | 1e-06 |
| remap | float32, interpolation=nearest, border=reflect101 | 15 | 0 | 1e-06 |
| remap | float32, interpolation=nearest, border=replicate | 15 | 0 | 1e-06 |
| remap | float32, interpolation=nearest, border=wrap | 10 | 0 | 1e-06 |
| remap | uint8, interpolation=linear, border=constant | 15 | 0 | 1 |
| remap | uint8, interpolation=linear, border=reflect | 10 | 0 | 1 |
| remap | uint8, interpolation=linear, border=reflect101 | 15 | 0 | 1 |
| remap | uint8, interpolation=linear, border=replicate | 15 | 0 | 1 |
| remap | uint8, interpolation=linear, border=wrap | 10 | 0 | 1 |
| remap | uint8, interpolation=nearest, border=constant | 15 | 0 | 1 |
| remap | uint8, interpolation=nearest, border=reflect | 10 | 0 | 1 |
| remap | uint8, interpolation=nearest, border=reflect101 | 15 | 0 | 1 |
| remap | uint8, interpolation=nearest, border=replicate | 15 | 0 | 1 |
| remap | uint8, interpolation=nearest, border=wrap | 10 | 0 | 1 |
| remap_interleaved | float32, interpolation=linear, border=constant | 5 | 0 | 1e-06 |
| remap_interleaved | float32, interpolation=linear, border=reflect101 | 5 | 0 | 1e-06 |
| remap_interleaved | float32, interpolation=linear, border=replicate | 5 | 0 | 1e-06 |
| remap_interleaved | float32, interpolation=nearest, border=constant | 5 | 0 | 1e-06 |
| remap_interleaved | float32, interpolation=nearest, border=reflect101 | 5 | 0 | 1e-06 |
| remap_interleaved | float32, interpolation=nearest, border=replicate | 5 | 0 | 1e-06 |
| remap_interleaved | uint8, interpolation=linear, border=constant | 5 | 0 | 1 |
| remap_interleaved | uint8, interpolation=linear, border=reflect101 | 5 | 0 | 1 |
| remap_interleaved | uint8, interpolation=linear, border=replicate | 5 | 0 | 1 |
| remap_interleaved | uint8, interpolation=nearest, border=constant | 5 | 0 | 1 |
| remap_interleaved | uint8, interpolation=nearest, border=reflect101 | 5 | 0 | 1 |
| remap_interleaved | uint8, interpolation=nearest, border=replicate | 5 | 0 | 1 |
| resize | float32, interpolation=area | 101 | 1.192e-07 | 2e-06 |
| resize | float32, interpolation=linear | 101 | 6.557e-07 | 2e-06 |
| resize | float32, interpolation=nearest | 96 | 0 | 2e-06 |
| resize | uint8, interpolation=area | 96 | 1 | 1 |
| resize | uint8, interpolation=linear | 96 | 1 | 1 |
| resize | uint8, interpolation=nearest | 96 | 0 | 1 |
| get_rotation_matrix_2d | float64 | 4 | 1.11022303e-16 | 1e-12 |
| sep_filter_2d | float32, border=constant | 2 | 1.192e-07 | 2e-06 |
| sep_filter_2d | float32, border=reflect | 2 | 1.192e-07 | 2e-06 |
| sep_filter_2d | float32, border=reflect101 | 7 | 4.768e-07 | 2e-06 |
| sep_filter_2d | float32, border=replicate | 2 | 1.192e-07 | 2e-06 |
| sobel | float32, border=reflect101 | 210 | 1.907e-06 | 2e-05 |
| get_structuring_element | uint8, kind=cross | 16 | 0 | 0 |
| get_structuring_element | uint8, kind=ellipse | 16 | 0 | 0 |
| get_structuring_element | uint8, kind=rect | 16 | 0 | 0 |
| warp_affine | float32, interpolation=linear, border=constant | 4 | 0 | 1e-06 |
| warp_affine | float32, interpolation=linear, border=reflect101 | 9 | 0 | 2e-06 |
| warp_affine | float32, interpolation=linear, border=replicate | 4 | 0 | 1e-06 |
| warp_affine | float32, interpolation=nearest, border=constant | 4 | 0 | 1e-06 |
| warp_affine | float32, interpolation=nearest, border=reflect101 | 9 | 0 | 2e-06 |
| warp_affine | float32, interpolation=nearest, border=replicate | 4 | 0 | 1e-06 |
| warp_affine | uint8, interpolation=linear, border=constant | 4 | 0 | 1 |
| warp_affine | uint8, interpolation=linear, border=reflect101 | 9 | 0 | 1 |
| warp_affine | uint8, interpolation=linear, border=replicate | 4 | 0 | 1 |
| warp_affine | uint8, interpolation=nearest, border=constant | 4 | 0 | 1 |
| warp_affine | uint8, interpolation=nearest, border=reflect101 | 9 | 0 | 1 |
| warp_affine | uint8, interpolation=nearest, border=replicate | 4 | 0 | 1 |
| warp_perspective | float32, interpolation=linear, border=constant | 4 | 0 | 1e-06 |
| warp_perspective | float32, interpolation=linear, border=reflect101 | 9 | 0 | 2e-06 |
| warp_perspective | float32, interpolation=linear, border=replicate | 4 | 0 | 1e-06 |
| warp_perspective | float32, interpolation=nearest, border=constant | 4 | 0 | 1e-06 |
| warp_perspective | float32, interpolation=nearest, border=reflect101 | 9 | 0 | 2e-06 |
| warp_perspective | float32, interpolation=nearest, border=replicate | 4 | 0 | 1e-06 |
| warp_perspective | uint8, interpolation=linear, border=constant | 4 | 0 | 1 |
| warp_perspective | uint8, interpolation=linear, border=reflect101 | 9 | 0 | 1 |
| warp_perspective | uint8, interpolation=linear, border=replicate | 4 | 0 | 1 |
| warp_perspective | uint8, interpolation=nearest, border=constant | 4 | 0 | 1 |
| warp_perspective | uint8, interpolation=nearest, border=reflect101 | 9 | 0 | 1 |
| warp_perspective | uint8, interpolation=nearest, border=replicate | 4 | 0 | 1 |

Regenerate goldens with the oracle interpreter and `-B`; run `cargo test -p grainy-imgproc -- --nocapture > crates/grainy-imgproc/validation.log 2>&1`, then run this script. The test suite also checks invalid inputs, exact identities, noncontiguous layouts, and identical output with one and four Rayon threads.

The degenerate perspective correspondence golden retains OpenCV 5's nonunique SVD matrix for inspection; the Rust API deliberately returns an error. See README.md.

## Tier 2

14 packed folders, 863 variants; the line variants each test both LSD and three Hough parameter sets. All arrays load through `grainy-testkit`. All structural and integer comparisons use exact equality.

| Rust function | Variants | Cases | Largest error | Tolerance |
|---|---|---:|---:|---:|
| absdiff | dtype=float32 | 22 | equal | 0 |
| absdiff | dtype=uint8 | 22 | equal | 0 |
| bilateral_filter | all golden inputs | 66 | 3.815e-05 | 0.0002 |
| connected_components / connected_components_with_stats | connectivity=4 | 15 | equal | 0 |
| connected_components / connected_components_with_stats | connectivity=8 | 15 | equal | 0 |
| find_contours + contour geometry | mode=ccomp | 14 | equal | 0 |
| find_contours + contour geometry | mode=external | 14 | equal | 0 |
| distance_transform | size=3 | 16 | 0.0001984 | 0.0002 |
| distance_transform | size=5 | 16 | 1.526e-05 | 0.0002 |
| draw_contours | thickness=-1 | 17 | equal | 0 |
| draw_contours | thickness=1 | 17 | equal | 0 |
| flood_fill | all golden inputs | 84 | equal | 0 |
| hough_lines_p | oracle and alternate rho/theta/gap | 90 | equal | 0 |
| convex_hull | all golden inputs | 28 | equal | 0 |
| inpaint | all golden inputs | 224 | equal | 0 |
| laplacian | all golden inputs | 88 | equal | 1e-05 |
| line_segment_detector | LSD_REFINE_STD/defaults | 30 | equal | 0.0002 |
| moments | binary=False | 14 | 1.28056854e-09 | 1e-05 |
| moments | binary=True | 14 | equal | 1e-05 |
| contour_area / arc_length / bounding_rect / is_contour_convex | all golden inputs | 7 | equal | 0 |
| match_template | method=ccoeff | 70 | 3.469e-05 | 0.0002 |
| match_template | method=sqdiff | 70 | 4.538e-05 | 0.0002 |

Every contour case also checks signed/unsigned area, open/closed arc length, bounding rectangle, strict convexity, and both convex-hull windings, all exactly. Each components case tests both public functions. The private LSD Gaussian and INTER_LINEAR_EXACT stages are separately byte-exact on every line input.

Tier 2 also checks invalid inputs, empty/full-mask identities, strided storage, extreme offscreen drawing coordinates, and one/four-thread equality. No global error bound or full-resolution throughput claim is implied by the sampled goldens. The float32 IPP distance reference and cross-correlation rounding can diverge more as dimensions and value magnitudes increase; the listed measurements retain native units.

Run the generator with `--tier2` to regenerate only the added cases and call-site inventory. The ordinary generator invocation regenerates both tiers. `golden_errors_tier2.json` contains every tier 2 measured error; the `case.json` files specify the oracle arguments.
