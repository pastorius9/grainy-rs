//! numpy's float32 reductions as the oracle uses them: `np.sum` (pairwise summation in float32, numpy's
//! `FLOAT_pairwise_sum`) and `ndarray.std()` (numpy's `_var` recipe), bit for bit.

pub(crate) use grainy_core::numpy::std;
