//! Latency histograms for the reference workloads (design D4 of
//! `add-reference-workloads`): fixed memory, constant-time recording, and
//! quantiles within 1/16 of the true value, so the kernel can record every
//! scheduling decision and system call of a run with 10,000 cells.

/// Sub-buckets per power of two: values are kept to 4 significant bits.
const SUB_BITS: u32 = 4;
const SUB: u64 = 1 << SUB_BITS;
/// Buckets for every `u64`: 16 exact ones, then 16 per power of two.
const BUCKETS: usize = ((64 - SUB_BITS + 1) * SUB as u32) as usize;

/// A log-linear histogram of `u64` values (nanoseconds, cycles…).
///
/// A value `v` lands in a bucket no wider than `v / 16`; a quantile is the
/// top of its bucket, so it is never below the true quantile and at most
/// 1/16 above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    buckets: [u32; BUCKETS],
    count: u64,
    sum: u64,
    max: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

/// The bucket of `v`.
fn index(v: u64) -> usize {
    if v < SUB {
        return v as usize;
    }
    let e = 63 - v.leading_zeros();
    let sub = (v >> (e - SUB_BITS)) & (SUB - 1);
    ((e - SUB_BITS + 1) as u64 * SUB + sub) as usize
}

/// The largest value of bucket `i`.
fn top(i: usize) -> u64 {
    let i = i as u64;
    if i < SUB {
        return i;
    }
    let shift = i / SUB - 1;
    let low = (SUB + i % SUB) << shift;
    low + ((1u64 << shift) - 1)
}

impl Histogram {
    /// An empty histogram.
    pub const fn new() -> Self {
        Self {
            buckets: [0; BUCKETS],
            count: 0,
            sum: 0,
            max: 0,
        }
    }

    /// Records one value.
    pub fn record(&mut self, v: u64) {
        let b = &mut self.buckets[index(v)];
        *b = b.saturating_add(1);
        self.count += 1;
        self.sum = self.sum.saturating_add(v);
        self.max = self.max.max(v);
    }

    /// Number of values recorded.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Sum of the values recorded (saturating).
    pub fn sum(&self) -> u64 {
        self.sum
    }

    /// Largest value recorded (0 when empty).
    pub fn max(&self) -> u64 {
        self.max
    }

    /// The value below which `per_million` millionths of the values lie
    /// (nearest rank, rounded up to the top of its bucket, never above the
    /// largest value); 0 when empty. `500_000` is the median, `999_000`
    /// the 99.9th percentile.
    pub fn quantile(&self, per_million: u32) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let rank = (self.count as u128 * per_million.min(1_000_000) as u128).div_ceil(1_000_000);
        let rank = (rank as u64).max(1);
        let mut seen = 0u64;
        for (i, &n) in self.buckets.iter().enumerate() {
            seen += n as u64;
            if seen >= rank {
                return top(i).min(self.max);
            }
        }
        self.max
    }

    /// Adds every value of `other`.
    pub fn merge(&mut self, other: &Self) {
        for (a, b) in self.buckets.iter_mut().zip(&other.buckets) {
            *a = a.saturating_add(*b);
        }
        self.count += other.count;
        self.sum = self.sum.saturating_add(other.sum);
        self.max = self.max.max(other.max);
    }

    /// Forgets every value.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn buckets_cover_every_value_in_order() {
        assert_eq!(index(0), 0);
        assert_eq!(index(15), 15);
        assert_eq!(index(16), 16);
        assert_eq!(index(u64::MAX), BUCKETS - 1);
        assert_eq!(top(BUCKETS - 1), u64::MAX);
        for i in 1..BUCKETS {
            assert!(top(i) > top(i - 1), "bucket {i} after bucket {}", i - 1);
            assert_eq!(index(top(i - 1) + 1), i, "first value of bucket {i}");
            assert_eq!(index(top(i)), i, "last value of bucket {i}");
        }
    }

    #[test]
    fn small_values_are_exact() {
        let mut h = Histogram::new();
        for v in [3, 1, 2, 9, 5] {
            h.record(v);
        }
        assert_eq!(h.count(), 5);
        assert_eq!(h.sum(), 20);
        assert_eq!(h.max(), 9);
        assert_eq!(h.quantile(500_000), 3);
        assert_eq!(h.quantile(1_000_000), 9);
        assert_eq!(h.quantile(0), 1);
        assert_eq!(Histogram::new().quantile(500_000), 0);
    }

    #[test]
    fn merge_and_clear() {
        let (mut a, mut b) = (Histogram::new(), Histogram::new());
        a.record(10);
        b.record(1000);
        b.record(2000);
        a.merge(&b);
        assert_eq!((a.count(), a.sum(), a.max()), (3, 3010, 2000));
        a.clear();
        assert_eq!(a, Histogram::new());
    }

    proptest! {
        /// Every quantile is at least the exact one and at most 1/16 above.
        #[test]
        fn quantiles_are_close_from_above(
            values in proptest::collection::vec(0u64..1 << 40, 1..400),
            q in 0u32..=1_000_000,
        ) {
            let mut h = Histogram::new();
            for &v in &values {
                h.record(v);
            }
            let mut sorted = values.clone();
            sorted.sort_unstable();
            let rank = ((values.len() as u128 * q as u128).div_ceil(1_000_000) as usize).max(1);
            let exact = sorted[rank - 1];
            let got = h.quantile(q);
            prop_assert!(got >= exact, "{got} < {exact}");
            prop_assert!(got <= exact + exact / 16, "{got} too far above {exact}");
        }
    }
}
