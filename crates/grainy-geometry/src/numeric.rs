//! Python and numpy number semantics the oracle relies on: `%` on floats, `round`, pairwise summation,
//! the FMA order of numpy's (OpenBLAS) 3×3 products, `np.linalg.inv`, weighted `np.histogram`.

/// Python's float `a % b` (numpy `np.mod`): the result has the sign of `b`.
pub(crate) fn py_mod(a: f64, b: f64) -> f64 {
    let m = a % b;
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m + b
        } else {
            m
        }
    } else {
        0.0f64.copysign(b)
    }
}

/// Python's `round(x)` for a float: ties to even.
pub(crate) fn py_round(x: f64) -> f64 {
    x.round_ties_even()
}

/// Python's `round(x, digits)` for a float: the correctly rounded decimal (ties to even on the exact binary
/// value), read back as the nearest double.
pub(crate) fn py_round_digits(x: f64, digits: usize) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    // The exact value far past the cut decides the rounding; a false tie would need 80 identical digits.
    let long = format!("{:.*}", digits + 80, x.abs());
    let point = long.find('.').unwrap_or(long.len());
    let keep = point + 1 + digits;
    let (head, tail) = long.split_at(keep);
    let tail = tail.as_bytes();
    let up = match tail.first() {
        Some(b'5') => {
            if tail[1..].iter().any(|&d| d != b'0') {
                true
            } else {
                // A tie: round to the even last kept digit.
                let last = head.bytes().rev().find(u8::is_ascii_digit).unwrap_or(b'0');
                (last - b'0') % 2 == 1
            }
        }
        Some(&d) => d > b'5',
        None => false,
    };
    let mut digits_only: Vec<u8> = head.bytes().filter(u8::is_ascii_digit).collect();
    if up {
        let mut i = digits_only.len();
        loop {
            if i == 0 {
                digits_only.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits_only[i] == b'9' {
                digits_only[i] = b'0';
            } else {
                digits_only[i] += 1;
                break;
            }
        }
    }
    let integer_len = digits_only.len() - digits;
    let mut text = String::with_capacity(digits_only.len() + 2);
    text.push_str(std::str::from_utf8(&digits_only[..integer_len]).unwrap_or("0"));
    text.push('.');
    text.push_str(std::str::from_utf8(&digits_only[integer_len..]).unwrap_or("0"));
    let value: f64 = text.parse().unwrap_or(f64::NAN);
    value.copysign(x)
}

pub(crate) use grainy_core::numpy::pairwise as pairwise_f64;

pub(crate) use grainy_core::numpy::pairwise as pairwise_f32;

/// `sum(values[i] for i where mask[i])` as numpy sums a boolean selection (pairwise over the selection).
pub(crate) fn masked_sum(values: &[f64], mask: &[bool]) -> f64 {
    let selected: Vec<f64> = values.iter().zip(mask).filter(|(_, &m)| m).map(|(&v, _)| v).collect();
    pairwise_f64(&selected)
}

/// A three-term dot product in the order of the oracle's OpenBLAS kernels: `fma(a2,b2,fma(a1,b1,a0*b0))`.
pub(crate) fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[2].mul_add(b[2], a[1].mul_add(b[1], a[0] * b[0]))
}

pub(crate) type M3 = [[f64; 3]; 3];

pub(crate) const EYE: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// numpy `a @ b` of 3×3 float64 matrices (OpenBLAS dgemm: FMA accumulation in k order).
pub(crate) fn matmul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = dot3(a[i], [b[0][j], b[1][j], b[2][j]]);
        }
    }
    out
}

/// numpy `m @ v` for a 3×3 matrix and a 3-vector.
pub(crate) fn matvec(m: &M3, v: [f64; 3]) -> [f64; 3] {
    [dot3(m[0], v), dot3(m[1], v), dot3(m[2], v)]
}

/// LU factorisation with partial pivoting, as LAPACK `dgetf2` (reciprocal pivot scaling). Returns the packed
/// factors, the pivot rows and whether a zero pivot occurred.
fn lu<const N: usize>(a: &[[f64; N]; N]) -> ([[f64; N]; N], [usize; N], bool) {
    let mut m = *a;
    let mut pivots = [0usize; N];
    let mut singular = false;
    for j in 0..N {
        let mut p = j;
        for i in j + 1..N {
            if m[i][j].abs() > m[p][j].abs() {
                p = i;
            }
        }
        pivots[j] = p;
        if m[p][j] != 0.0 {
            if p != j {
                m.swap(p, j);
            }
            if m[j][j].abs() >= f64::MIN_POSITIVE {
                let r = 1.0 / m[j][j];
                for row in m.iter_mut().skip(j + 1) {
                    row[j] *= r;
                }
            } else {
                for i in j + 1..N {
                    m[i][j] /= m[j][j];
                }
            }
        } else {
            singular = true;
        }
        for i in j + 1..N {
            let l = m[i][j];
            for k in j + 1..N {
                m[i][k] -= l * m[j][k];
            }
        }
    }
    (m, pivots, singular)
}

/// `np.linalg.det` (LU product with the permutation sign).
pub(crate) fn det<const N: usize>(a: &[[f64; N]; N]) -> f64 {
    let (m, pivots, _) = lu(a);
    let mut d = 1.0;
    for (j, &p) in pivots.iter().enumerate() {
        d *= m[j][j];
        if p != j {
            d = -d;
        }
    }
    d
}

/// `np.linalg.inv` (LAPACK `dgesv` with the identity); `None` for a singular matrix (numpy's LinAlgError).
pub(crate) fn inv<const N: usize>(a: &[[f64; N]; N]) -> Option<[[f64; N]; N]> {
    let (m, pivots, singular) = lu(a);
    if singular {
        return None;
    }
    let mut out = [[0.0; N]; N];
    for col in 0..N {
        let mut b = [0.0; N];
        b[col] = 1.0;
        for (j, &p) in pivots.iter().enumerate() {
            b.swap(j, p);
        }
        for i in 0..N {
            for k in 0..i {
                b[i] -= m[i][k] * b[k];
            }
        }
        for i in (0..N).rev() {
            for k in i + 1..N {
                b[i] -= m[i][k] * b[k];
            }
            b[i] /= m[i][i];
        }
        for i in 0..N {
            out[i][col] = b[i];
        }
    }
    Some(out)
}

/// Indices that sort `values` ascending (stable; numpy's default sort is not, see the crate README).
pub(crate) fn argsort(values: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    order
}

/// `np.searchsorted(sorted, v, side)`.
pub(crate) fn searchsorted(sorted: &[f64], v: f64, right: bool) -> usize {
    if right {
        sorted.partition_point(|&x| x <= v)
    } else {
        sorted.partition_point(|&x| x < v)
    }
}

/// `np.histogram(values, edges, weights=weights)[0]` for explicit edges: numpy's cumulative-weight path.
pub(crate) fn histogram(values: &[f64], weights: &[f64], edges: &[f64]) -> Vec<f64> {
    let order = argsort(values);
    let sorted: Vec<f64> = order.iter().map(|&i| values[i]).collect();
    let mut cumulative = Vec::with_capacity(values.len() + 1);
    cumulative.push(0.0);
    let mut running = 0.0;
    for &i in &order {
        running += weights[i];
        cumulative.push(running);
    }
    let last = edges.len() - 1;
    let cum: Vec<f64> =
        edges.iter().enumerate().map(|(k, &e)| cumulative[searchsorted(&sorted, e, k == last)]).collect();
    cum.windows(2).map(|w| w[1] - w[0]).collect()
}

/// `np.convolve(values, np.ones(5), mode='same')`: each output is the sum of up to five neighbours, added left
/// to right as numpy's correlate does.
pub(crate) fn convolve_ones5(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(2);
            let hi = (i + 3).min(n);
            let mut s = 0.0;
            for &v in &values[lo..hi] {
                s += v;
            }
            s
        })
        .collect()
}

/// `np.ptp` of a non-empty slice.
pub(crate) fn ptp(values: &[f64]) -> f64 {
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    max - min
}

/// `np.argmax`: the first largest value.
pub(crate) fn argmax(values: &[f64]) -> usize {
    let mut best = 0;
    for (i, &v) in values.iter().enumerate() {
        if v > values[best] {
            best = i;
        }
    }
    best
}

/// `np.median` (mean of the two middle values for an even count).
pub(crate) fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_round_digits() {
        assert_eq!(py_round_digits(0.015625, 5), 0.01562);
        assert_eq!(py_round_digits(0.015635, 5), 0.01563);
        assert_eq!(py_round_digits(2.675, 2), 2.67);
        assert_eq!(py_round_digits(-0.5, 0), -0.0);
        assert_eq!(py_round_digits(0.999999, 5), 1.0);
        assert_eq!(py_round_digits(0.1 + 0.2, 15), 0.3);
        assert_eq!(py_mod(-1.0, 4.0), 3.0);
        assert_eq!(py_mod(-90.0, 360.0), 270.0);
        assert_eq!(py_round(2.5), 2.0);
    }

    #[test]
    fn small_inverse() {
        let a = [[2.0, 1.0, 0.0], [1.0, 3.0, 1.0], [0.0, 1.0, 4.0]];
        let b = inv(&a).unwrap();
        let p = matmul(&a, &b);
        for (i, row) in p.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-15);
            }
        }
        assert!((det(&a) - 18.0).abs() < 1e-12);
        assert!(inv(&[[1.0, 2.0], [2.0, 4.0]]).is_none());
    }
}
