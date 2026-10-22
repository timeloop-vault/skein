//! Host main-thread lag probe (#639). On `WebView2` the JS drift probe
//! lives in the renderer process and cannot see the host UI thread, which
//! receives every IPC request and dispatches every Channel eval. So a
//! background thread posts a no-op closure to the main thread every
//! 250 ms and records how long it waited (`host.mainLag`). At most one
//! probe is in flight: a blocked main thread cannot queue them up, and
//! the stalled probe records its full lag once it finally runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::perf_spans::HOST_MAIN_LAG;

const PROBE_INTERVAL: Duration = Duration::from_millis(250);

/// A probe that waited longer than this is logged as a stall.
const STALL_WARN: Duration = Duration::from_secs(1);

fn is_stall(lag: Duration) -> bool {
    lag > STALL_WARN
}

/// Claim the single in-flight slot; true when the caller may fire a probe.
fn try_claim(in_flight: &AtomicBool) -> bool {
    in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Start the probe thread. Stops quietly once the app is shutting down.
pub(crate) fn start(app: tauri::AppHandle) {
    let in_flight = Arc::new(AtomicBool::new(false));
    let spawned = std::thread::Builder::new()
        .name("perf-main-lag".into())
        .spawn(move || {
            loop {
                std::thread::sleep(PROBE_INTERVAL);
                if !try_claim(&in_flight) {
                    continue;
                }
                let t0 = Instant::now();
                let flag = Arc::clone(&in_flight);
                let posted = app.run_on_main_thread(move || {
                    let lag = t0.elapsed();
                    HOST_MAIN_LAG.record(lag);
                    if is_stall(lag) {
                        let (cpu, disk, queue) = crate::perf_load::current_fields();
                        tracing::warn!(
                            target = "perf",
                            lag_ms = u64::try_from(lag.as_millis()).unwrap_or(u64::MAX),
                            cpu_pct = %cpu,
                            disk_busy_pct = %disk,
                            disk_queue = %queue,
                            "host main thread stalled"
                        );
                    }
                    flag.store(false, Ordering::Release);
                });
                if posted.is_err() {
                    in_flight.store(false, Ordering::Release);
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "perf: main-lag probe thread failed to start");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_threshold_is_exclusive_one_second() {
        assert!(!is_stall(Duration::from_millis(1000)));
        assert!(is_stall(Duration::from_millis(1001)));
        assert!(!is_stall(Duration::ZERO));
    }

    #[test]
    fn only_one_probe_in_flight() {
        let f = AtomicBool::new(false);
        assert!(try_claim(&f));
        assert!(!try_claim(&f));
        f.store(false, Ordering::Release);
        assert!(try_claim(&f));
    }
}
