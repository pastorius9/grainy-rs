//! Pixel moves: quarter turns, flips and EXIF orientation (`np.rot90`, `a[:, ::-1]`, `rasterio.orient_pixels`).

use ndarray::{s, Array3, ArrayView3};

fn owned(view: ArrayView3<'_, f32>) -> Array3<f32> {
    view.as_standard_layout().into_owned()
}

/// Port of `np.rot90(a, k)` on the first two axes (counter-clockwise quarter turns; any integer `k`).
pub fn rot90(a: ArrayView3<'_, f32>, k: i64) -> Array3<f32> {
    match k.rem_euclid(4) {
        1 => owned(a.slice_move(s![.., ..;-1, ..]).permuted_axes([1, 0, 2])),
        2 => owned(a.slice_move(s![..;-1, ..;-1, ..])),
        3 => owned(a.permuted_axes([1, 0, 2]).slice_move(s![.., ..;-1, ..])),
        _ => owned(a),
    }
}

/// Port of `a[:, ::-1]` (the `flip` setting).
pub fn flip_left_right(a: ArrayView3<'_, f32>) -> Array3<f32> {
    owned(a.slice_move(s![.., ..;-1, ..]))
}

/// Port of luma/rasterio.py `orient_pixels`: the EXIF orientation `1..=8` applied to an H×W×C array; any other
/// value (0, 9, …) leaves the pixels as they are.
pub fn orient_pixels(a: ArrayView3<'_, f32>, orientation: i64) -> Array3<f32> {
    match orientation {
        2 => owned(a.slice_move(s![.., ..;-1, ..])),
        3 => owned(a.slice_move(s![..;-1, ..;-1, ..])),
        4 => owned(a.slice_move(s![..;-1, .., ..])),
        5 => owned(a.permuted_axes([1, 0, 2])),
        6 => rot90(a, -1),
        7 => owned(a.permuted_axes([1, 0, 2]).slice_move(s![..;-1, ..;-1, ..])),
        8 => rot90(a, 1),
        _ => owned(a),
    }
}
