//! Aggregated timings of Rust-side work (#655), appended to the
//! frontend's periodic `perf summary` log line so one line covers both
//! sides. Recording is lock-free; the summary drains the counters.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Count / total / max of a span since the last [`SpanAgg::take`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SpanSnapshot {
    pub(crate) count: u64,
    pub(crate) total_us: u64,
    pub(crate) max_us: u64,
}

pub(crate) struct SpanAgg {
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
}

impl SpanAgg {
    pub(crate) const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
        }
    }

    pub(crate) fn record(&self, d: Duration) {
        let us = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
    }

    /// Read and reset. The three swaps are not atomic together, so a
    /// record racing a take can split across two summaries; that skew is
    /// acceptable for a diagnostic line.
    pub(crate) fn take(&self) -> SpanSnapshot {
        SpanSnapshot {
            count: self.count.swap(0, Ordering::Relaxed),
            total_us: self.total_us.swap(0, Ordering::Relaxed),
            max_us: self.max_us.swap(0, Ordering::Relaxed),
        }
    }
}

/// One review-discovery watcher batch (`Discovery::discover`).
pub(crate) static REVIEW_DISCOVERY_BATCH: SpanAgg = SpanAgg::new();

/// `pty_write` body (#639): runs on the host UI thread.
pub(crate) static PTY_WRITE: SpanAgg = SpanAgg::new();

/// `on_event.send` of one PTY data chunk (#639); count = delivered chunks.
pub(crate) static PTY_CHUNK_SEND: SpanAgg = SpanAgg::new();

/// Wait of a probe posted to the host main thread (#639).
pub(crate) static HOST_MAIN_LAG: SpanAgg = SpanAgg::new();

fn ms(us: u64) -> u64 {
    us.saturating_add(500) / 1000
}

/// Append ` | <area>[wall] n=.. total=..ms max=..ms` per non-empty area,
/// matching `perfStats.ts`. `[wall]` because this is worker-thread wall
/// time, not main-thread jank.
pub(crate) fn append_rust_spans(line: &str, areas: &[(&str, SpanSnapshot)]) -> String {
    let mut out = line.to_owned();
    for (name, s) in areas.iter().filter(|(_, s)| s.count > 0) {
        let _ = write!(
            out,
            " | {name}[wall] n={} total={}ms max={}ms",
            s.count,
            ms(s.total_us),
            ms(s.max_us)
        );
    }
    out
}

/// Drain every Rust span area into `line`.
pub(crate) fn drain_and_append(line: &str) -> String {
    append_rust_spans(
        line,
        &[
            ("reviewDiscovery.batch", REVIEW_DISCOVERY_BATCH.take()),
            ("pty.write", PTY_WRITE.take()),
            ("pty.chunkSend", PTY_CHUNK_SEND.take()),
            ("host.mainLag", HOST_MAIN_LAG.take()),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(count: u64, total_us: u64, max_us: u64) -> SpanSnapshot {
        SpanSnapshot {
            count,
            total_us,
            max_us,
        }
    }

    #[test]
    fn record_aggregates_and_take_resets() {
        let a = SpanAgg::new();
        a.record(Duration::from_micros(300));
        a.record(Duration::from_micros(900));
        assert_eq!(a.take(), snap(2, 1200, 900));
        assert_eq!(a.take(), SpanSnapshot::default());
    }

    #[test]
    fn zero_count_area_is_omitted() {
        let out = append_rust_spans("perf summary", &[("a", SpanSnapshot::default())]);
        assert_eq!(out, "perf summary");
    }

    #[test]
    fn rounds_to_nearest_ms() {
        let out = append_rust_spans("x", &[("a", snap(1, 1499, 1500))]);
        assert_eq!(out, "x | a[wall] n=1 total=1ms max=2ms");
    }

    #[test]
    fn example_line() {
        let line = "perf summary reason=tick | area n=3 total=12ms max=8ms";
        let out = append_rust_spans(
            line,
            &[
                ("reviewDiscovery.batch", snap(4, 20_400, 9_600)),
                ("empty", SpanSnapshot::default()),
            ],
        );
        assert_eq!(
            out,
            "perf summary reason=tick | area n=3 total=12ms max=8ms | reviewDiscovery.batch[wall] n=4 total=20ms max=10ms"
        );
    }
}
