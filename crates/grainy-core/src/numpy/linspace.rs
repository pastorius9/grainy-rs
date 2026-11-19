/// `np.linspace(start, stop, num)` (endpoint included) of float64 vectors, row by row: numpy's step form, or
/// `i / div * delta` when any component's step is zero, and the last row set to `stop`.
pub fn linspace_vector<const N: usize>(start: [f64; N], stop: [f64; N], num: usize) -> Vec<[f64; N]> {
    let div = num.saturating_sub(1);
    let mut delta = [0f64; N];
    let mut step = [0f64; N];
    for c in 0..N {
        delta[c] = stop[c] - start[c];
        step[c] = delta[c] / div as f64;
    }
    let any_zero = div > 0 && step.contains(&0.0);
    let mut out = Vec::with_capacity(num);
    for i in 0..num {
        let mut row = [0f64; N];
        for c in 0..N {
            let y = if div == 0 {
                i as f64 * delta[c]
            } else if any_zero {
                i as f64 / div as f64 * delta[c]
            } else {
                i as f64 * step[c]
            };
            row[c] = y + start[c];
        }
        out.push(row);
    }
    if num > 1 {
        out[num - 1] = stop;
    }
    out
}

/// `np.linspace(0, n - 1, num, dtype=int)`: float64 positions floored to integers.
pub fn linspace_int(n: usize, num: usize) -> Vec<usize> {
    linspace_vector([0.0], [(n as f64) - 1.0], num).into_iter().map(|[v]| v.floor().max(0.0) as usize).collect()
}
