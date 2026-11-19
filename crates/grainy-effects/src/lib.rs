//! Grainy's Effects panel, ported from `luma/engine.py`: film grain (`grain_pattern`, `settings_grain`,
//! `_add_grain`) and the post-crop vignette (`vignette_shape`, `post_crop_vignette`).
//!
//! The grain is numpy's `default_rng(2026).standard_normal(shape)` reproduced bit for bit (private
//! `numpy_random`, BSD-3-Clause material from numpy: see NOTICE), blurred by a private port of cv2's
//! `GaussianBlur`, so photos keep the same grain as in the Python Grainy.

mod blur;
mod grain;
mod npstats;
mod numpy_random;
mod vignette;

pub use grain::{add_grain, grain_key, grain_pattern, settings_grain, GrainKey, GrainPattern, Shape};
pub use vignette::{post_crop_vignette, vignette_shape, VignetteShape};
