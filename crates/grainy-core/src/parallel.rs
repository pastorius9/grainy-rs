//! Row parallelism, after the oracle's `luma/pixel_jobs.py`: per-pixel work is split into bands of rows.
//!
//! Results never depend on the number of threads: every pixel is computed from its own inputs.

use ndarray::{ArrayView3, ArrayViewMut3, Axis};
use rayon::prelude::*;

use crate::Image;

/// Images smaller than this many pixels are processed on the calling thread (`pixel_jobs.MIN_PIXELS`).
pub const MIN_PIXELS: usize = 128 * 1024;
/// Rows are handed out in bands of about this many pixels (`pixel_jobs.TILE_PIXELS`).
pub const TILE_PIXELS: usize = 64 * 1024;

fn band_rows(height: usize, width: usize) -> usize {
    if height * width < MIN_PIXELS {
        height.max(1)
    } else {
        (TILE_PIXELS / width.max(1)).clamp(1, height.max(1))
    }
}

/// Computes an image of the same shape as `input` band by band: `f(input_band, output_band)` fills the output
/// rows from the same rows of the input. Port of `pixel_jobs.transform` for functions that work row-locally.
pub fn map_rows<F>(input: &Image, f: F) -> Image
where
    F: Fn(ArrayView3<'_, f32>, ArrayViewMut3<'_, f32>) + Sync,
{
    let (height, width, _) = input.dim();
    let mut output = Image::zeros(input.raw_dim());
    let rows = band_rows(height, width);
    output
        .axis_chunks_iter_mut(Axis(0), rows)
        .into_par_iter()
        .zip(input.axis_chunks_iter(Axis(0), rows).into_par_iter())
        .for_each(|(out, band)| f(band, out));
    output
}

/// Applies `f` to every pixel (a slice of the channels) in place, in parallel bands.
pub fn for_each_pixel<F>(image: &mut Image, f: F)
where
    F: Fn(&mut [f32]) + Sync,
{
    let (height, width, channels) = image.dim();
    let rows = band_rows(height, width);
    image.axis_chunks_iter_mut(Axis(0), rows).into_par_iter().for_each(|mut band| {
        if let Some(slice) = band.as_slice_mut() {
            slice.chunks_exact_mut(channels).for_each(&f);
        } else {
            for mut row in band.outer_iter_mut() {
                for mut pixel in row.outer_iter_mut() {
                    let mut values: Vec<f32> = pixel.iter().copied().collect();
                    f(&mut values);
                    pixel.iter_mut().zip(values).for_each(|(slot, value)| *slot = value);
                }
            }
        }
    });
}

/// Runs `f(first_row, last_row_exclusive)` for bands covering `0..height`, in parallel, and returns the results
/// in row order. For work that needs its own buffers per band.
pub fn bands<T, F>(height: usize, width: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize, usize) -> T + Sync,
{
    let rows = band_rows(height, width);
    let starts: Vec<usize> = (0..height).step_by(rows.max(1)).collect();
    starts.into_par_iter().map(|start| f(start, (start + rows).min(height))).collect()
}
