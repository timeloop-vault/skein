//! Main-thread occupancy of every Tauri command (#639). Non-async
//! commands run their whole body inside the invoke handler on the host
//! main thread; async ones only deserialize and spawn there. Timing the
//! handler call therefore measures main-thread time either way. One
//! wrapper in `lib.rs` feeds [`record`]; the perf summary drains the
//! heaviest commands through [`drain`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::perf_spans::{SpanAgg, SpanSnapshot};

/// A call strictly longer than this logs a WARN at once.
pub(crate) const SLOW_CMD_MS: u64 = 200;
/// Commands printed individually; the rest fold into [`CMD_REST`].
const TOP_N: usize = 10;
/// Area name of the folded tail. It carries no p50/p95: histograms of
/// different commands are not merged, and a percentile of per-command
/// percentiles would be dishonest. The summary omits them for this area.
pub(crate) const CMD_REST: &str = "cmd:rest";

fn is_slow(d: Duration) -> bool {
    d > Duration::from_millis(SLOW_CMD_MS)
}

pub(crate) struct CmdSlot {
    name: Box<str>,
    agg: SpanAgg,
}

impl CmdSlot {
    pub(crate) fn record(&self, d: Duration) {
        self.agg.record(d);
        if is_slow(d) {
            let (cpu, disk, queue) = crate::perf_load::current_fields();
            tracing::warn!(
                target = "perf",
                cmd = %self.name,
                ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
                cpu_pct = %cpu,
                disk_busy_pct = %disk,
                disk_queue = %queue,
                "slow main-thread command"
            );
        }
    }
}

static REGISTRY: Mutex<Option<HashMap<Box<str>, Arc<CmdSlot>>>> = Mutex::new(None);

/// The slot for `name`, created on first use. Allocates only the first
/// time a command is seen.
pub(crate) fn slot(name: &str) -> Arc<CmdSlot> {
    let mut g = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(s) = map.get(name) {
        return Arc::clone(s);
    }
    let s = Arc::new(CmdSlot {
        name: name.into(),
        agg: SpanAgg::new(),
    });
    map.insert(name.into(), Arc::clone(&s));
    s
}

/// Wrap the generated invoke handler so every call is timed by command
/// name. Adds one `Instant` pair and one map lookup; never alters the
/// inner handler's result.
pub(crate) fn timed<R: tauri::Runtime>(
    inner: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        let slot = slot(invoke.message.command());
        let start = std::time::Instant::now();
        let handled = inner(invoke);
        slot.record(start.elapsed());
        handled
    }
}

/// Top commands by total time as `cmd:<name>`, the tail folded into
/// [`CMD_REST`]. Empty entries are omitted.
pub(crate) fn fold(mut all: Vec<(String, SpanSnapshot)>) -> Vec<(String, SpanSnapshot)> {
    all.retain(|(_, s)| s.count > 0);
    all.sort_by(|a, b| b.1.total_us.cmp(&a.1.total_us).then_with(|| a.0.cmp(&b.0)));
    let mut out: Vec<(String, SpanSnapshot)> = Vec::new();
    let mut rest = SpanSnapshot::default();
    for (i, (name, s)) in all.into_iter().enumerate() {
        if i < TOP_N {
            out.push((format!("cmd:{name}"), s));
        } else {
            rest.count += s.count;
            rest.total_us = rest.total_us.saturating_add(s.total_us);
            rest.max_us = rest.max_us.max(s.max_us);
        }
    }
    if rest.count > 0 {
        out.push((CMD_REST.to_owned(), rest));
    }
    out
}

/// Read and reset every command's aggregate.
pub(crate) fn drain() -> Vec<(String, SpanSnapshot)> {
    let slots: Vec<Arc<CmdSlot>> = {
        let g = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        g.as_ref()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    };
    fold(
        slots
            .iter()
            .map(|s| (s.name.to_string(), s.agg.take()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf_spans::append_main_spans;

    fn snap(count: u64, total_us: u64, max_us: u64) -> SpanSnapshot {
        SpanSnapshot {
            count,
            total_us,
            max_us,
            p50_us: max_us,
            p95_us: max_us,
        }
    }

    #[test]
    fn top_n_and_rest_folding() {
        let all: Vec<(String, SpanSnapshot)> = (1..=13u64)
            .map(|i| (format!("c{i}"), snap(i, i * 1000, i * 10)))
            .chain([("zero".to_owned(), SpanSnapshot::default())])
            .collect();
        let out = fold(all);
        assert_eq!(out.len(), 11);
        assert_eq!(out[0].0, "cmd:c13");
        assert_eq!(out[9].0, "cmd:c4");
        let (name, rest) = &out[10];
        assert_eq!(name, CMD_REST);
        assert_eq!((rest.count, rest.total_us, rest.max_us), (6, 6000, 30));
    }

    #[test]
    fn no_rest_when_few() {
        let out = fold(vec![("a".into(), snap(1, 5, 5))]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "cmd:a");
    }

    #[test]
    fn threshold_is_exclusive() {
        assert!(!is_slow(Duration::from_millis(SLOW_CMD_MS)));
        assert!(is_slow(Duration::from_micros(SLOW_CMD_MS * 1000 + 1)));
    }

    #[test]
    fn slot_records_and_drains() {
        let s = slot("perf_cmds_test_cmd");
        s.record(Duration::from_millis(3));
        let out = drain();
        let hit = out.iter().find(|(n, _)| n == "cmd:perf_cmds_test_cmd");
        assert_eq!(hit.map(|(_, s)| s.count), Some(1));
    }

    #[test]
    fn format_has_no_wall_suffix() {
        let areas = vec![
            ("cmd:git_watch_start".to_owned(), snap(2, 20_000, 15_000)),
            (CMD_REST.to_owned(), snap(4, 8_000, 3_000)),
        ];
        let out = append_main_spans("x", &areas);
        assert_eq!(
            out,
            "x | cmd:git_watch_start n=2 total=20ms max=15ms p50=15ms p95=15ms | cmd:rest n=4 total=8ms max=3ms"
        );
        assert!(!out.contains("[wall]"));
    }
}
