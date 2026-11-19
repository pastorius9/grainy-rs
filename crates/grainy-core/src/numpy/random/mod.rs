//! numpy's `default_rng(seed)`, bit for bit: `SeedSequence` → `PCG64` → `Generator.standard_normal` (the float64
//! ziggurat of `numpy/random/src/distributions/distributions.c`) and `Generator.random` (`next_double`).
//!
//! The oracle draws its grain from `np.random.default_rng(2026).standard_normal(shape)`; these ports give the same
//! numbers (tolerance 0 against numpy's raw arrays). numpy is BSD-3-Clause: see the crate's NOTICE.

mod ziggurat_tables;

use ziggurat_tables::{FI_DOUBLE, KI_DOUBLE, WI_DOUBLE};

/// numpy's `SeedSequence(entropy)` without spawn keys, with the default pool size of 4 words.
struct SeedSequence {
    pool: [u32; 4],
}

const INIT_A: u32 = 0x43b0_d7e5;
const MULT_A: u32 = 0x931e_8875;
const INIT_B: u32 = 0x8b51_f9dd;
const MULT_B: u32 = 0x58f3_8ded;
const MIX_MULT_L: u32 = 0xca01_f9dd;
const MIX_MULT_R: u32 = 0x4973_f715;
const XSHIFT: u32 = 16;

fn hashmix(value: u32, hash_const: &mut u32) -> u32 {
    let mut value = value ^ *hash_const;
    *hash_const = hash_const.wrapping_mul(MULT_A);
    value = value.wrapping_mul(*hash_const);
    value ^ (value >> XSHIFT)
}

fn mix(x: u32, y: u32) -> u32 {
    let result = MIX_MULT_L.wrapping_mul(x).wrapping_sub(MIX_MULT_R.wrapping_mul(y));
    result ^ (result >> XSHIFT)
}

impl SeedSequence {
    /// `SeedSequence(entropy)` for a non-negative integer: the integer as 32-bit words, least significant first.
    fn new(entropy: u128) -> Self {
        let mut words = Vec::new();
        let mut rest = entropy;
        loop {
            words.push((rest & 0xffff_ffff) as u32);
            rest >>= 32;
            if rest == 0 {
                break;
            }
        }
        let mut pool = [0u32; 4];
        let mut hash_const = INIT_A;
        for (i, slot) in pool.iter_mut().enumerate() {
            *slot = hashmix(words.get(i).copied().unwrap_or(0), &mut hash_const);
        }
        for i_src in 0..4 {
            for i_dst in 0..4 {
                if i_src != i_dst {
                    let mixed = hashmix(pool[i_src], &mut hash_const);
                    pool[i_dst] = mix(pool[i_dst], mixed);
                }
            }
        }
        for &word in words.iter().skip(4) {
            for slot in pool.iter_mut() {
                let mixed = hashmix(word, &mut hash_const);
                *slot = mix(*slot, mixed);
            }
        }
        Self { pool }
    }

    /// `generate_state(n_words, np.uint64)`.
    fn generate_state_u64(&self, n_words: usize) -> Vec<u64> {
        let mut hash_const = INIT_B;
        let words: Vec<u32> = (0..n_words * 2)
            .map(|i| {
                let mut value = self.pool[i % 4] ^ hash_const;
                hash_const = hash_const.wrapping_mul(MULT_B);
                value = value.wrapping_mul(hash_const);
                value ^ (value >> XSHIFT)
            })
            .collect();
        words.as_chunks::<2>().0.iter().map(|&[low, high]| u64::from(low) | (u64::from(high) << 32)).collect()
    }
}

const PCG_MULT: u128 = (2_549_297_995_355_413_924u128 << 64) | 4_865_540_595_714_422_341u128;

/// numpy's `PCG64` (PCG XSL-RR 128/64) bit generator.
pub struct Pcg64 {
    state: u128,
    inc: u128,
    buffered: Option<u32>,
}

impl Pcg64 {
    /// NumPy PCG64 seeded through SeedSequence.
    pub fn new(seed: u64) -> Self {
        Self::from_seed_sequence(&SeedSequence::new(u128::from(seed)))
    }
    /// `next_uint32`: the low half of a 64-bit draw, then its high half.
    pub fn next_u32(&mut self) -> u32 {
        if let Some(v) = self.buffered.take() {
            return v;
        }
        let next = self.next_u64();
        self.buffered = Some((next >> 32) as u32);
        next as u32
    }

    /// `random_bounded_uint64(off=0, rng, mask=0, use_masked=False)`: uniform in `0..=rng`.
    pub fn bounded(&mut self, rng: u64) -> u64 {
        if rng == 0 {
            0
        } else if rng <= 0xFFFF_FFFF {
            if rng == 0xFFFF_FFFF {
                return u64::from(self.next_u32());
            }
            let rng_excl = rng as u32 + 1;
            let mut m = u64::from(self.next_u32()) * u64::from(rng_excl);
            let mut leftover = m as u32;
            if leftover < rng_excl {
                let threshold = (u32::MAX - rng as u32) % rng_excl;
                while leftover < threshold {
                    m = u64::from(self.next_u32()) * u64::from(rng_excl);
                    leftover = m as u32;
                }
            }
            m >> 32
        } else if rng == u64::MAX {
            self.next_u64()
        } else {
            let rng_excl = rng + 1;
            let mut m = u128::from(self.next_u64()) * u128::from(rng_excl);
            let mut leftover = m as u64;
            if leftover < rng_excl {
                let threshold = (u64::MAX - rng) % rng_excl;
                while leftover < threshold {
                    m = u128::from(self.next_u64()) * u128::from(rng_excl);
                    leftover = m as u64;
                }
            }
            (m >> 64) as u64
        }
    }

    fn shuffle(&mut self, n: usize, first: usize, data: &mut [i64]) {
        for i in (first..n).rev() {
            let j = self.bounded(i as u64) as usize;
            data.swap(i, j);
        }
    }

    /// `Generator.choice(pop_size, size, replace=False)` (uniform, shuffled).
    pub fn choice_without_replacement(&mut self, pop_size: usize, size: usize) -> Vec<i64> {
        let size = size.min(pop_size);
        if pop_size > 10000 && size > pop_size / 50 {
            let mut idx: Vec<i64> = (0..pop_size as i64).collect();
            self.shuffle(pop_size, (pop_size - size).max(1), &mut idx);
            return idx[pop_size - size..].to_vec();
        }
        let mut idx = vec![0i64; size];
        let set_size = (1.2 * size as f64) as u64;
        let mut mask = set_size;
        for shift in [1, 2, 4, 8, 16, 32] {
            mask |= mask >> shift;
        }
        let empty = u64::MAX;
        let mut hash_set = vec![empty; (mask + 1) as usize];
        for j in (pop_size - size)..pop_size {
            let val = self.bounded(j as u64);
            let mut loc = (val & mask) as usize;
            while hash_set[loc] != empty && hash_set[loc] != val {
                loc = (loc + 1) & mask as usize;
            }
            if hash_set[loc] == empty {
                hash_set[loc] = val;
                idx[j + size - pop_size] = val as i64;
            } else {
                let mut loc = (j as u64 & mask) as usize;
                while hash_set[loc] != empty {
                    loc = (loc + 1) & mask as usize;
                }
                hash_set[loc] = j as u64;
                idx[j + size - pop_size] = j as i64;
            }
        }
        self.shuffle(size, 1, &mut idx);
        idx
    }

    fn from_seed_sequence(seq: &SeedSequence) -> Self {
        let val = seq.generate_state_u64(4);
        let initstate = (u128::from(val[0]) << 64) | u128::from(val[1]);
        let initseq = (u128::from(val[2]) << 64) | u128::from(val[3]);
        let mut rng = Self { state: 0, inc: (initseq << 1) | 1, buffered: None };
        rng.step();
        rng.state = rng.state.wrapping_add(initstate);
        rng.step();
        rng
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    /// NumPy PCG64.next_uint64.
    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let high = (self.state >> 64) as u64;
        let low = self.state as u64;
        (high ^ low).rotate_right((self.state >> 122) as u32)
    }

    fn next_double(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

const ZIGGURAT_NOR_R: f64 = 3.654_152_885_361_009; // numpy: 3.6541528853610088
const ZIGGURAT_NOR_INV_R: f64 = 0.273_661_237_329_758_3; // numpy: 0.27366123732975828

/// numpy's `Generator(PCG64(SeedSequence(seed)))`: what `np.random.default_rng(seed)` returns.
pub struct Generator {
    bits: Pcg64,
}

impl Generator {
    /// `np.random.default_rng(seed)` for a non-negative integer seed.
    pub fn default_rng(seed: u128) -> Self {
        Self { bits: Pcg64::from_seed_sequence(&SeedSequence::new(seed)) }
    }

    /// `random()`: one float64 uniform in [0, 1). Unused by the grain; kept (and golden-tested) for ports
    /// that draw uniforms.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn random(&mut self) -> f64 {
        self.bits.next_double()
    }

    /// `standard_normal()`: numpy's `random_standard_normal` (float64 ziggurat).
    pub fn standard_normal(&mut self) -> f64 {
        loop {
            let mut r = self.bits.next_u64();
            let idx = (r & 0xff) as usize;
            r >>= 8;
            let sign = r & 1;
            let rabs = (r >> 1) & 0x000f_ffff_ffff_ffff;
            let mut x = rabs as f64 * WI_DOUBLE[idx];
            if sign & 1 == 1 {
                x = -x;
            }
            if rabs < KI_DOUBLE[idx] {
                return x;
            }
            if idx == 0 {
                loop {
                    let xx = -ZIGGURAT_NOR_INV_R * (-self.bits.next_double()).ln_1p();
                    let yy = -(-self.bits.next_double()).ln_1p();
                    if yy + yy > xx * xx {
                        return if (rabs >> 8) & 1 == 1 { -(ZIGGURAT_NOR_R + xx) } else { ZIGGURAT_NOR_R + xx };
                    }
                }
            } else if (FI_DOUBLE[idx - 1] - FI_DOUBLE[idx]) * self.bits.next_double() + FI_DOUBLE[idx]
                < (-0.5 * x * x).exp()
            {
                return x;
            }
        }
    }

    /// Fills `out` in order, as `standard_normal(out.len())` (C order for a reshaped array).
    pub fn fill_standard_normal(&mut self, out: &mut [f64]) {
        for slot in out {
            *slot = self.standard_normal();
        }
    }
}

/// Port of `np.random.default_rng(seed).choice(pop_size, size, replace=False)`.
pub fn choice(seed: u64, pop_size: usize, size: usize) -> Vec<i64> {
    Pcg64::new(seed).choice_without_replacement(pop_size, size)
}

#[cfg(test)]
mod tests {
    use super::Generator;
    use grainy_testkit::Case;
    use ndarray::Array1;

    fn seed_of(case: &Case) -> u128 {
        case.json()["seed"].as_str().and_then(|s| s.parse().ok()).expect("seed")
    }

    fn assert_bits(actual: &[f64], expected: &Array1<f64>, what: &str) {
        assert_eq!(actual.len(), expected.len(), "{what}: length");
        let differing = actual.iter().zip(expected.iter()).filter(|(a, e)| a.to_bits() != e.to_bits()).count();
        println!("{what}: {differing} of {} values differ in any bit (tolerance 0)", actual.len());
        assert_eq!(differing, 0, "{what}");
    }

    #[test]
    fn standard_normal_matches_numpy_bit_for_bit() {
        for (seed, n) in [
            ("2026", 4000),
            ("0", 1000),
            ("1", 1),
            ("12345", 3000),
            ("1099511627783", 1000),
            ("1180591620717411303427", 500),
        ] {
            let case = Case::open("effects", &format!("rng_normal_{seed}_{n}"));
            let expected: Array1<f64> = case.array("values");
            let mut rng = Generator::default_rng(seed_of(&case));
            let mut actual = vec![0.0; n];
            rng.fill_standard_normal(&mut actual);
            assert_bits(&actual, &expected, &format!("standard_normal seed {seed} n {n}"));
        }
    }

    #[test]
    fn a_million_normals_match_numpy() {
        for seed in ["2026", "99"] {
            let case = Case::open("effects", &format!("rng_normal_{seed}_million"));
            let mut rng = Generator::default_rng(seed_of(&case));
            let mut values = vec![0.0; 1_000_000];
            rng.fill_standard_normal(&mut values);
            let sum = values.iter().fold(0u64, |acc, v| acc.wrapping_add(v.to_bits()));
            let expected: u64 = case.json()["bits_sum"].as_str().and_then(|s| s.parse().ok()).expect("bits_sum");
            let last: Vec<f64> = case.json()["last_bits"]
                .as_array()
                .expect("last_bits")
                .iter()
                .filter_map(|v| v.as_str()?.parse().ok().map(f64::from_bits))
                .collect();
            assert_eq!(last.len(), 5);
            println!("standard_normal seed {seed}, 1e6 draws: bit-pattern sum {sum} (numpy {expected})");
            assert_eq!(sum, expected);
            assert_bits(&values[values.len() - 5..], &Array1::from(last), "last five");
        }
    }

    #[test]
    fn random_matches_numpy_bit_for_bit() {
        for (seed, n) in [("7", 1000), ("2026", 100)] {
            let case = Case::open("effects", &format!("rng_random_{seed}_{n}"));
            let expected: Array1<f64> = case.array("values");
            let mut rng = Generator::default_rng(seed_of(&case));
            let actual: Vec<f64> = (0..n).map(|_| rng.random()).collect();
            assert_bits(&actual, &expected, &format!("random seed {seed} n {n}"));
        }
    }
}
