//! Bootstrap statistics for the retrieval report (#788, ADR-0788).
//!
//! Everything here is pure, dependency-free and bit-reproducible: a hand-rolled SplitMix64 PRNG
//! (so the stream cannot change under a crate upgrade or differ between platforms), per-question
//! resampling with a fixed seed and resample count, and a percentile interval. Each interval's
//! seed is `base_seed ^ fnv1a(label)` so adding a configuration, category or metric never changes
//! any other interval.
//!
//! Resample index is `next_u64() % n` (the modulo bias at n ≈ 10² is ~1e-17 and irrelevant).

use std::collections::BTreeMap;

/// Default base seed (the issue number).
pub const DEFAULT_SEED: u64 = 788;
/// Default number of per-question resamples.
pub const DEFAULT_RESAMPLES: usize = 10_000;
/// Interval level; the percentile bounds are 2.5 % and 97.5 %.
pub const LEVEL: f64 = 0.95;
/// Categories with fewer questions than this are flagged "low n" in the report.
pub const MIN_CATEGORY_N: usize = 20;
/// A category is "at the ceiling" when every ran configuration's nDCG@k point estimate is ≥ this.
pub const CEILING: f64 = 0.98;

/// SplitMix64 (Steele, Lea & Flood 2014). Tiny, fast and statistically adequate for bootstrap
/// index draws and for the deterministic corpus generator.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; `n` must be non-zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: usize, den: usize) -> bool {
        self.below(den) < num
    }
}

/// 64-bit FNV-1a, used to derive an independent seed per interval label.
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A point estimate with its percentile interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub n: usize,
    pub mean: f64,
    pub lo: f64,
    pub hi: f64,
}

fn percentile_bounds(resamples: usize) -> (usize, usize) {
    let alpha = (1.0 - LEVEL) / 2.0;
    let lo = (resamples as f64 * alpha).floor() as usize;
    let hi = ((resamples as f64 * (1.0 - alpha)).ceil() as usize).saturating_sub(1);
    (lo.min(resamples - 1), hi.min(resamples - 1))
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn resampled_means(values: &[f64], seed: u64, resamples: usize) -> Vec<f64> {
    let n = values.len();
    let mut rng = SplitMix64::new(seed);
    let mut means = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut sum = 0.0;
        for _ in 0..n {
            sum += values[rng.below(n)];
        }
        means.push(sum / n as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).expect("bootstrap means are finite"));
    means
}

/// Percentile bootstrap CI of the mean. `None` when fewer than 2 values (an interval over one
/// question is not an interval) or when `resamples` is zero.
pub fn bootstrap_ci(values: &[f64], seed: u64, resamples: usize) -> Option<Interval> {
    if values.len() < 2 || resamples == 0 {
        return None;
    }
    let means = resampled_means(values, seed, resamples);
    let (lo, hi) = percentile_bounds(resamples);
    Some(Interval {
        n: values.len(),
        mean: mean(values),
        lo: means[lo],
        hi: means[hi],
    })
}

/// How a paired comparison reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The 95 % interval of (a − b) lies wholly above zero.
    AHigher,
    /// The 95 % interval of (a − b) lies wholly below zero.
    BHigher,
    /// The interval includes zero: not distinguishable from noise.
    Noise,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::AHigher => "a higher",
            Verdict::BHigher => "b higher",
            Verdict::Noise => "not distinguishable from noise",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairedDiff {
    pub n: usize,
    pub mean_diff: f64,
    pub lo: f64,
    pub hi: f64,
    pub verdict: Verdict,
}

impl PairedDiff {
    /// The same comparison seen from the other side: `b − a`.
    pub fn flipped(self) -> Self {
        Self {
            n: self.n,
            mean_diff: -self.mean_diff,
            lo: -self.hi,
            hi: -self.lo,
            verdict: match self.verdict {
                Verdict::AHigher => Verdict::BHigher,
                Verdict::BHigher => Verdict::AHigher,
                Verdict::Noise => Verdict::Noise,
            },
        }
    }
}

/// Paired per-question bootstrap on `a − b`. The two series are **joined by question id** (a
/// configuration with failed queries has fewer entries, so positional pairing would be wrong); only
/// ids present in both count. `None` when fewer than 2 questions are shared.
pub fn paired_diff_ci(
    a: &[(String, f64)],
    b: &[(String, f64)],
    seed: u64,
    resamples: usize,
) -> Option<PairedDiff> {
    let bm: BTreeMap<&str, f64> = b.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let mut keyed: Vec<(&str, f64)> = a
        .iter()
        .filter_map(|(k, v)| bm.get(k.as_str()).map(|w| (k.as_str(), v - w)))
        .collect();
    // Order by id so the resample stream is independent of the input order.
    keyed.sort_by(|x, y| x.0.cmp(y.0));
    let diffs: Vec<f64> = keyed.into_iter().map(|(_, d)| d).collect();
    let iv = bootstrap_ci(&diffs, seed, resamples)?;
    let verdict = if iv.lo > 0.0 {
        Verdict::AHigher
    } else if iv.hi < 0.0 {
        Verdict::BHigher
    } else {
        Verdict::Noise
    };
    Some(PairedDiff {
        n: iv.n,
        mean_diff: iv.mean,
        lo: iv.lo,
        hi: iv.hi,
        verdict,
    })
}

/// Seed for the interval named by `label` (`config|category|metric`).
pub fn seed_for(base: u64, label: &str) -> u64 {
    base ^ fnv1a(label)
}

/// True when there is at least one estimate and every one is ≥ [`CEILING`].
pub fn at_ceiling(points: &[f64]) -> bool {
    !points.is_empty() && points.iter().all(|p| *p >= CEILING)
}

/// True when a category's sample is below [`MIN_CATEGORY_N`].
pub fn low_n(n: usize) -> bool {
    n < MIN_CATEGORY_N
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed(xs: &[f64]) -> Vec<(String, f64)> {
        xs.iter()
            .enumerate()
            .map(|(i, v)| (format!("q{i:03}"), *v))
            .collect()
    }

    #[test]
    fn splitmix64_matches_reference_vector() {
        // Reference values for seed 0 (from the SplitMix64 reference implementation).
        let mut r = SplitMix64::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn same_seed_same_interval_bit_for_bit() {
        let v: Vec<f64> = (0..50).map(|i| (i % 7) as f64 / 7.0).collect();
        let a = bootstrap_ci(&v, 788, 2000).unwrap();
        let b = bootstrap_ci(&v, 788, 2000).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.lo.to_bits(), b.lo.to_bits());
        let c = bootstrap_ci(&v, 789, 2000).unwrap();
        assert_ne!(a.lo, c.lo);
    }

    #[test]
    fn interval_brackets_the_mean_and_narrows_with_n() {
        let small: Vec<f64> = (0..10).map(|i| (i % 2) as f64).collect();
        let large: Vec<f64> = (0..400).map(|i| (i % 2) as f64).collect();
        let s = bootstrap_ci(&small, 1, 2000).unwrap();
        let l = bootstrap_ci(&large, 1, 2000).unwrap();
        assert!(s.lo <= s.mean && s.mean <= s.hi);
        assert!(l.lo <= l.mean && l.mean <= l.hi);
        assert!(
            l.hi - l.lo < s.hi - s.lo,
            "more questions → narrower interval"
        );
        // Known distribution: Bernoulli(0.5), n = 400 → SE 0.025 → ±0.049.
        assert!((l.hi - l.lo) > 0.07 && (l.hi - l.lo) < 0.13, "{l:?}");
    }

    #[test]
    fn degenerate_inputs_give_none_not_a_zero_width_interval() {
        assert_eq!(bootstrap_ci(&[], 1, 100), None);
        assert_eq!(bootstrap_ci(&[0.5], 1, 100), None);
        assert_eq!(bootstrap_ci(&[0.5, 0.5], 1, 0), None);
        // Two identical values are a genuine (zero-width) estimate, e.g. a ceiling.
        let iv = bootstrap_ci(&[1.0, 1.0], 1, 100).unwrap();
        assert_eq!((iv.lo, iv.hi), (1.0, 1.0));
    }

    #[test]
    fn paired_difference_of_identical_series_is_noise() {
        let a = keyed(&[0.1, 0.9, 0.5, 0.3, 0.7, 0.2]);
        let d = paired_diff_ci(&a, &a, 1, 500).unwrap();
        assert_eq!(d.verdict, Verdict::Noise);
        assert_eq!(d.mean_diff, 0.0);
    }

    #[test]
    fn paired_difference_detects_a_consistent_gap() {
        let a = keyed(
            &(0..40)
                .map(|i| 0.5 + (i % 5) as f64 / 20.0)
                .collect::<Vec<_>>(),
        );
        let b = keyed(
            &(0..40)
                .map(|i| 0.4 + (i % 5) as f64 / 20.0)
                .collect::<Vec<_>>(),
        );
        let d = paired_diff_ci(&a, &b, 1, 1000).unwrap();
        assert_eq!(d.verdict, Verdict::AHigher);
        assert!(d.lo > 0.0 && (d.mean_diff - 0.1).abs() < 1e-9);
        let r = paired_diff_ci(&b, &a, 1, 1000).unwrap();
        assert_eq!(r.verdict, Verdict::BHigher);
    }

    #[test]
    fn a_gap_of_one_question_in_forty_is_noise() {
        let a = keyed(&vec![1.0; 40]);
        let mut bv = vec![1.0; 40];
        bv[0] = 0.0;
        let b = keyed(&bv);
        let d = paired_diff_ci(&a, &b, 1, 2000).unwrap();
        assert_eq!(d.verdict, Verdict::Noise, "{d:?}");
    }

    #[test]
    fn paired_comparison_joins_by_id_not_position() {
        let a = keyed(&[0.9, 0.8, 0.7, 0.6]);
        // b is missing q001 (a failed query); positional pairing would misalign q002/q003.
        let b: Vec<(String, f64)> = vec![
            ("q000".into(), 0.9),
            ("q002".into(), 0.7),
            ("q003".into(), 0.6),
        ];
        let d = paired_diff_ci(&a, &b, 1, 200).unwrap();
        assert_eq!(d.n, 3);
        assert_eq!(d.mean_diff, 0.0);
        // Input order does not matter.
        let mut rev = b.clone();
        rev.reverse();
        assert_eq!(paired_diff_ci(&a, &rev, 1, 200), Some(d));
    }

    #[test]
    fn too_few_shared_questions_give_none() {
        let a = keyed(&[0.1, 0.2]);
        let b: Vec<(String, f64)> = vec![("q000".into(), 0.1), ("zzz".into(), 0.2)];
        assert_eq!(paired_diff_ci(&a, &b, 1, 100), None);
    }

    #[test]
    fn ceiling_and_low_n_helpers() {
        assert!(at_ceiling(&[1.0, 0.99, 0.98]));
        assert!(!at_ceiling(&[1.0, 0.97]));
        assert!(!at_ceiling(&[]));
        assert!(low_n(MIN_CATEGORY_N - 1));
        assert!(!low_n(MIN_CATEGORY_N));
    }

    #[test]
    fn seeds_are_label_specific_and_stable() {
        assert_eq!(
            seed_for(788, "qmd|direct|ndcg"),
            seed_for(788, "qmd|direct|ndcg")
        );
        assert_ne!(
            seed_for(788, "qmd|direct|ndcg"),
            seed_for(788, "qmd|direct|mrr")
        );
        // FNV-1a reference: hash of "" is the offset basis.
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
    }
}
