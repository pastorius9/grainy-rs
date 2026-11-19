//! The numpy reductions `sky_mask.py` relies on, with numpy 2.5's rounding: pairwise summation (`np.sum`,
//! `np.mean`, `np.average`), `np.percentile` (linear method) and `np.median`.

pub(crate) use grainy_core::numpy::pairwise;

pub(crate) use grainy_core::numpy::mean_f32;

pub(crate) use grainy_core::numpy::percentile as percentile_f32;

pub(crate) use grainy_core::numpy::percentile_i64;

pub(crate) use grainy_core::numpy::median_f32;
