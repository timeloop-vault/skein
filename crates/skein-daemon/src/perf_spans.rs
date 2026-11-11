//! The span aggregate (#655) and the one span the review code records.
//! The remaining statics and the summary line live in the app.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::perf_hist::Histogram;

/// Count / total / max / approximate p50 and p95 of a span since the last [`SpanAgg::take`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpanSnapshot {
    pub count: u64,
    pub total_us: u64,
    pub max_us: u64,
    /// Nearest-rank percentiles, bucket upper bound clamped to `max_us`.
    pub p50_us: u64,
    pub p95_us: u64,
}

pub struct SpanAgg {
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
    hist: Histogram,
}

impl SpanAgg {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
            hist: Histogram::new(),
        }
    }

    pub fn record(&self, d: Duration) {
        let us = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
        self.hist.record(us);
    }

    /// Read and reset. The swaps are not atomic together, so a
    /// record racing a take can split across two summaries; that skew is
    /// acceptable for a diagnostic line.
    pub fn take(&self) -> SpanSnapshot {
        let count = self.count.swap(0, Ordering::Relaxed);
        let total_us = self.total_us.swap(0, Ordering::Relaxed);
        let max_us = self.max_us.swap(0, Ordering::Relaxed);
        let (p50_us, p95_us) = self.hist.take_percentiles(max_us);
        SpanSnapshot {
            count,
            total_us,
            max_us,
            p50_us,
            p95_us,
        }
    }
}

/// One review-discovery watcher batch (`Discovery::discover`).
pub static REVIEW_DISCOVERY_BATCH: SpanAgg = SpanAgg::new();

/// `pty_write` body (#639): runs on the host UI thread.
pub static PTY_WRITE: SpanAgg = SpanAgg::new();

/// `on_event.send` of one PTY data chunk (#639); count = delivered chunks.
pub static PTY_CHUNK_SEND: SpanAgg = SpanAgg::new();

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(count: u64, total_us: u64, max_us: u64, p50_us: u64, p95_us: u64) -> SpanSnapshot {
        SpanSnapshot {
            count,
            total_us,
            max_us,
            p50_us,
            p95_us,
        }
    }

    #[test]
    fn record_aggregates_and_take_resets() {
        let a = SpanAgg::new();
        a.record(Duration::from_micros(300));
        a.record(Duration::from_micros(900));
        assert_eq!(a.take(), snap(2, 1200, 900, 319, 900));
        assert_eq!(a.take(), SpanSnapshot::default());
    }

    #[test]
    fn percentiles_of_1_to_100_ms() {
        let a = SpanAgg::new();
        for ms in 1..=100u64 {
            a.record(Duration::from_millis(ms));
        }
        let s = a.take();
        assert_eq!(s.max_us, 100_000);
        assert!((50_000..=56_250).contains(&s.p50_us), "{}", s.p50_us);
        assert!((95_000..=100_000).contains(&s.p95_us), "{}", s.p95_us);
        assert_eq!(a.take(), SpanSnapshot::default());
    }

    #[test]
    fn single_sample_percentiles_equal_max() {
        let a = SpanAgg::new();
        a.record(Duration::from_micros(7_777));
        let s = a.take();
        assert_eq!((s.p50_us, s.p95_us, s.max_us), (7_777, 7_777, 7_777));
    }
}
