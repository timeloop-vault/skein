//! Lock-free log-linear histogram of microsecond durations (#639), used
//! for the p50/p95 of the Rust span areas. Values below 16 us get an exact
//! bucket; above that each power of two is split into 8 sub-buckets, so a
//! bucket's width is at most 12.5% of its lower bound. A percentile is the
//! bucket's upper bound (clamped to the exact max by the caller), i.e. an
//! over-estimate by at most 12.5%.

use std::sync::atomic::{AtomicU64, Ordering};

const EXACT: usize = 16;
const EXACT_U64: u64 = 16;
const SUB_BITS: u32 = 3;
const SUBS: usize = 1 << SUB_BITS;
const SUBS_U64: u64 = 1 << SUB_BITS;
/// 16 exact buckets + 8 per power of two for exponents 4..=63.
pub(crate) const BUCKETS: usize = EXACT + 60 * SUBS;

pub(crate) struct Histogram {
    buckets: [AtomicU64; BUCKETS],
}

/// Bucket holding `us`; monotonic non-decreasing in `us`.
pub(crate) fn bucket_index(us: u64) -> usize {
    if us < EXACT_U64 {
        return usize::try_from(us).unwrap_or(0);
    }
    let p = us.ilog2();
    let sub = usize::try_from((us >> (p - SUB_BITS)) & (SUBS_U64 - 1)).unwrap_or(0);
    EXACT + (usize::try_from(p).unwrap_or(0) - 4) * SUBS + sub
}

/// Largest value (inclusive) that lands in bucket `idx`.
pub(crate) fn bucket_upper(idx: usize) -> u64 {
    if idx < EXACT {
        return u64::try_from(idx).unwrap_or(0);
    }
    let j = idx - EXACT;
    let p = u32::try_from(j / SUBS).unwrap_or(0) + 4;
    let sub = u64::try_from(j % SUBS).unwrap_or(0);
    let shift = p - SUB_BITS;
    ((SUBS_U64 + sub) << shift) + ((1u64 << shift) - 1)
}

impl Histogram {
    pub(crate) const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; BUCKETS],
        }
    }

    pub(crate) fn record(&self, us: u64) {
        self.buckets[bucket_index(us)].fetch_add(1, Ordering::Relaxed);
    }

    /// Swap every bucket to zero and return `(p50, p95)` by nearest rank,
    /// each clamped to `max_us`. `(0, 0)` when empty.
    pub(crate) fn take_percentiles(&self, max_us: u64) -> (u64, u64) {
        let counts: Vec<u64> = self
            .buckets
            .iter()
            .map(|b| b.swap(0, Ordering::Relaxed))
            .collect();
        let n: u64 = counts.iter().sum();
        if n == 0 {
            return (0, 0);
        }
        let rank = |p: u64| (p * n).div_ceil(100).max(1);
        let (r50, r95) = (rank(50), rank(95));
        let (mut p50, mut p95) = (None, None);
        let mut seen = 0u64;
        for (idx, c) in counts.iter().enumerate() {
            seen += c;
            if p50.is_none() && seen >= r50 {
                p50 = Some(bucket_upper(idx).min(max_us));
            }
            if seen >= r95 {
                p95 = Some(bucket_upper(idx).min(max_us));
                break;
            }
        }
        (p50.unwrap_or(max_us), p95.unwrap_or(max_us))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_monotonic_and_in_range() {
        let mut prev = 0;
        let mut v = 0u64;
        while v < 1 << 40 {
            let i = bucket_index(v);
            assert!(i >= prev && i < BUCKETS, "v={v}");
            prev = i;
            v += 1 + v / 64;
        }
        assert_eq!(bucket_index(u64::MAX), BUCKETS - 1);
        assert_eq!(bucket_upper(BUCKETS - 1), u64::MAX);
    }

    #[test]
    fn value_fits_its_bucket_within_error() {
        let mut v = 0u64;
        while v < 1 << 40 {
            let up = bucket_upper(bucket_index(v));
            assert!(v <= up, "v={v} up={up}");
            if v >= EXACT_U64 {
                assert!(up - v <= v / 8, "v={v} up={up}");
            } else {
                assert_eq!(up, v);
            }
            v += 1 + v / 97;
        }
    }

    #[test]
    fn bucket_bounds_are_contiguous() {
        for idx in 1..BUCKETS {
            assert_eq!(bucket_index(bucket_upper(idx - 1) + 1), idx);
        }
    }

    #[test]
    fn percentiles_of_1_to_100_ms() {
        let h = Histogram::new();
        for ms in 1..=100u64 {
            h.record(ms * 1000);
        }
        let (p50, p95) = h.take_percentiles(100_000);
        assert!((50_000..=56_250).contains(&p50), "p50={p50}");
        assert!((95_000..=106_875).contains(&p95), "p95={p95}");
        assert_eq!(h.take_percentiles(0), (0, 0));
    }

    #[test]
    fn single_sample_is_clamped_to_max() {
        let h = Histogram::new();
        h.record(123_457);
        assert_eq!(h.take_percentiles(123_457), (123_457, 123_457));
    }
}
