//! Aggregated timings of Rust-side work (#655), appended to the
//! frontend's periodic `perf summary` log line so one line covers both
//! sides. Recording is lock-free; the summary drains the counters.
//! p50/p95 come from a log-linear histogram (`perf_hist`), so they are
//! upper bounds within 12.5% of the true value, never above the max.

pub(crate) use skein_daemon::perf_spans::{
    PTY_CHUNK_SEND, PTY_WRITE, REVIEW_DISCOVERY_BATCH, SpanAgg, SpanSnapshot,
};
use std::fmt::Write as _;

/// Wait of a probe posted to the host main thread (#639).
pub(crate) static HOST_MAIN_LAG: SpanAgg = SpanAgg::new();

fn ms(us: u64) -> u64 {
    us.saturating_add(500) / 1000
}

/// Append ` | <area>[wall] n=.. total=..ms max=..ms p50=..ms p95=..ms` per non-empty area,
/// matching `perfStats.ts`. `[wall]` because this is worker-thread wall
/// time, not main-thread jank.
pub(crate) fn append_rust_spans(line: &str, areas: &[(&str, SpanSnapshot)]) -> String {
    let mut out = line.to_owned();
    for (name, s) in areas.iter().filter(|(_, s)| s.count > 0) {
        let _ = write!(
            out,
            " | {name}[wall] n={} total={}ms max={}ms p50={}ms p95={}ms",
            s.count,
            ms(s.total_us),
            ms(s.max_us),
            ms(s.p50_us),
            ms(s.p95_us)
        );
    }
    out
}

/// Like [`append_rust_spans`] for main-thread areas (per-command times,
/// #639): no `[wall]` suffix, and [`crate::perf_cmds::CMD_REST`] prints
/// no percentiles.
pub(crate) fn append_main_spans(line: &str, areas: &[(String, SpanSnapshot)]) -> String {
    let mut out = line.to_owned();
    for (name, s) in areas.iter().filter(|(_, s)| s.count > 0) {
        let _ = write!(
            out,
            " | {name} n={} total={}ms max={}ms",
            s.count,
            ms(s.total_us),
            ms(s.max_us)
        );
        if name != crate::perf_cmds::CMD_REST {
            let _ = write!(out, " p50={}ms p95={}ms", ms(s.p50_us), ms(s.p95_us));
        }
    }
    out
}

/// Drain every Rust span area into `line`.
pub(crate) fn drain_and_append(line: &str) -> String {
    let line = append_rust_spans(
        line,
        &[
            ("reviewDiscovery.batch", REVIEW_DISCOVERY_BATCH.take()),
            ("pty.write", PTY_WRITE.take()),
            ("pty.chunkSend", PTY_CHUNK_SEND.take()),
            ("host.mainLag", HOST_MAIN_LAG.take()),
        ],
    );
    append_main_spans(&line, &crate::perf_cmds::drain())
}

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
    fn zero_count_area_is_omitted() {
        let out = append_rust_spans("perf summary", &[("a", SpanSnapshot::default())]);
        assert_eq!(out, "perf summary");
    }

    #[test]
    fn rounds_to_nearest_ms() {
        let out = append_rust_spans("x", &[("a", snap(1, 1499, 1500, 1499, 1500))]);
        assert_eq!(out, "x | a[wall] n=1 total=1ms max=2ms p50=1ms p95=2ms");
    }

    #[test]
    fn example_line() {
        let line = "perf summary reason=tick | area n=3 total=12ms max=8ms";
        let out = append_rust_spans(
            line,
            &[
                (
                    "reviewDiscovery.batch",
                    snap(4, 20_400, 9_600, 4_400, 9_600),
                ),
                ("empty", SpanSnapshot::default()),
            ],
        );
        assert_eq!(
            out,
            "perf summary reason=tick | area n=3 total=12ms max=8ms | reviewDiscovery.batch[wall] n=4 total=20ms max=10ms p50=4ms p95=10ms"
        );
    }
}
