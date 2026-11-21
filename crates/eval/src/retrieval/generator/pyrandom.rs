//! A Python-compatible `random.Random` (MT19937), just enough of it to reproduce `shuffle` (#821).
//!
//! The E13 graph question set and its dev/test split were produced by Python's
//! `random.Random(788).shuffle` and `random.Random(815).shuffle` (`gen-graph-questions.py` on the
//! maintainer's probe branch). The fixture's own generator RNG ([`super::world::Rng`], SplitMix64)
//! cannot reproduce them, so the graph question generator uses this one. It is deterministic in the
//! same sense as the rest of the generator (ADR-0788): no clock, no hash-map iteration, and the same
//! seed gives the same sequence on every platform. See ADR-0821.

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER: u32 = 0x8000_0000;
const LOWER: u32 = 0x7fff_ffff;

pub struct PyRandom {
    mt: [u32; N],
    mti: usize,
}

impl PyRandom {
    /// `random.Random(seed)` for a non-negative integer seed below 2^64 (CPython seeds with
    /// `init_by_array` over the seed's 32-bit words, least significant first).
    pub fn new(seed: u64) -> Self {
        let lo = seed as u32;
        let hi = (seed >> 32) as u32;
        let key: Vec<u32> = if hi == 0 { vec![lo] } else { vec![lo, hi] };
        let mut r = PyRandom { mt: [0; N], mti: N };
        r.init_by_array(&key);
        r
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.mti = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..N.max(key.len()) {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
        }
        self.mt[0] = 0x8000_0000;
    }

    /// `getrandbits(32)` (`genrand_uint32`).
    pub fn next_u32(&mut self) -> u32 {
        if self.mti >= N {
            for k in 0..N {
                let y = (self.mt[k] & UPPER) | (self.mt[(k + 1) % N] & LOWER);
                let mag = if y & 1 == 1 { MATRIX_A } else { 0 };
                self.mt[k] = self.mt[(k + M) % N] ^ (y >> 1) ^ mag;
            }
            self.mti = 0;
        }
        let mut y = self.mt[self.mti];
        self.mti += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `_randbelow(n)` for `0 < n < 2^32`: rejection sampling over `getrandbits(n.bit_length())`.
    pub fn randbelow(&mut self, n: usize) -> usize {
        assert!(n > 0 && n <= u32::MAX as usize, "randbelow({n})");
        let k = usize::BITS - n.leading_zeros();
        loop {
            let r = (self.next_u32() >> (32 - k)) as usize;
            if r < n {
                return r;
            }
        }
    }

    /// `shuffle(x)`: Fisher–Yates from the end, exactly as CPython.
    pub fn shuffle<T>(&mut self, x: &mut [T]) {
        for i in (1..x.len()).rev() {
            let j = self.randbelow(i + 1);
            x.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from CPython 3:
    //   python3 -c "import random
    //   for s in (788,815,821):
    //       r=random.Random(s); print([r.getrandbits(32) for _ in range(4)])
    //       r=random.Random(s); x=list(range(20)); r.shuffle(x); print(x)"
    const VECTORS: [(u64, [u32; 4], [usize; 20]); 3] = [
        (
            788,
            [2519703842, 2073703901, 4010676617, 3482599471],
            [
                13, 9, 4, 17, 6, 1, 7, 14, 19, 12, 3, 8, 2, 16, 10, 5, 11, 0, 15, 18,
            ],
        ),
        (
            815,
            [3846755485, 54715754, 1234447615, 167425397],
            [
                6, 14, 5, 2, 19, 15, 12, 4, 11, 3, 13, 16, 8, 17, 7, 10, 18, 1, 9, 0,
            ],
        ),
        (
            821,
            [2449375847, 2785940286, 4226206501, 2111454802],
            [
                0, 13, 19, 16, 3, 2, 5, 17, 4, 8, 10, 6, 14, 12, 1, 7, 11, 9, 15, 18,
            ],
        ),
    ];

    #[test]
    fn matches_cpython_getrandbits_and_shuffle() {
        for (seed, words, shuffled) in VECTORS {
            let mut r = PyRandom::new(seed);
            let got: Vec<u32> = (0..4).map(|_| r.next_u32()).collect();
            assert_eq!(got, words, "seed {seed}");
            let mut r = PyRandom::new(seed);
            let mut x: Vec<usize> = (0..20).collect();
            r.shuffle(&mut x);
            assert_eq!(x, shuffled, "seed {seed}");
        }
    }

    #[test]
    fn shuffle_of_short_slices_is_a_no_op_or_permutation() {
        let mut r = PyRandom::new(1);
        let mut e: Vec<u8> = vec![];
        r.shuffle(&mut e);
        let mut one = vec![7];
        r.shuffle(&mut one);
        assert_eq!(one, vec![7]);
        // Survives a state refill (> 624 draws).
        let mut big: Vec<usize> = (0..2000).collect();
        r.shuffle(&mut big);
        big.sort();
        assert_eq!(big, (0..2000).collect::<Vec<_>>());
    }
}
