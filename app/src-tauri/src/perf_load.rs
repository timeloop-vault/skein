//! Machine-load context for the perf telemetry (#639). A process-wide
//! `skein-sysload` sampler (system CPU %, system-drive busy % and queue)
//! so a stall WARN or a summary line can say whether the machine itself
//! was loaded. Windows only; elsewhere every accessor returns `None`.

use std::sync::OnceLock;
use std::time::Duration;

use skein_sysload::{LoadSample, Sampler, WindowStats};

const INTERVAL: Duration = Duration::from_secs(1);

static SAMPLER: OnceLock<Sampler> = OnceLock::new();

/// Start the sampler once; later calls are no-ops.
pub(crate) fn start() {
    SAMPLER.get_or_init(|| Sampler::start(INTERVAL));
}

/// The most recent sample, if the sampler runs and has one.
pub(crate) fn current() -> Option<LoadSample> {
    SAMPLER.get().and_then(Sampler::latest)
}

/// Drain the window since the last call as a summary-line segment.
pub(crate) fn window_segment() -> Option<String> {
    format_window(SAMPLER.get()?.take_window())
}

/// Load fields for a WARN line: `cpu_pct`, `disk_busy_pct`, `disk_queue`,
/// each `-` when no sample exists.
pub(crate) fn current_fields() -> (String, String, String) {
    format_fields(current())
}

fn format_window(w: Option<WindowStats>) -> Option<String> {
    let w = w?;
    Some(format!(
        " | sys cpu_mean={:.0}% cpu_max={:.0}% disk_busy_mean={:.0}% disk_busy_max={:.0}% disk_queue_max={:.1} samples={}",
        w.cpu_mean, w.cpu_max, w.disk_busy_mean, w.disk_busy_max, w.queue_max, w.samples
    ))
}

fn format_fields(s: Option<LoadSample>) -> (String, String, String) {
    match s {
        Some(s) => (
            format!("{:.0}%", s.cpu_pct),
            format!("{:.0}%", s.disk_busy_pct),
            format!("{:.1}", s.disk_queue),
        ),
        None => ("-".into(), "-".into(), "-".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_segment_format_and_rounding() {
        let w = WindowStats {
            cpu_mean: 41.6,
            cpu_max: 97.2,
            disk_busy_mean: 60.0,
            disk_busy_max: 100.0,
            queue_max: 12.34,
            samples: 300,
        };
        assert_eq!(
            format_window(Some(w)).as_deref(),
            Some(
                " | sys cpu_mean=42% cpu_max=97% disk_busy_mean=60% disk_busy_max=100% disk_queue_max=12.3 samples=300"
            )
        );
    }

    #[test]
    fn window_none_is_omitted() {
        assert_eq!(format_window(None), None);
    }

    #[test]
    fn fields_format_and_absent() {
        let s = LoadSample {
            cpu_pct: 12.4,
            disk_busy_pct: 99.6,
            disk_queue: 0.26,
        };
        assert_eq!(
            format_fields(Some(s)),
            ("12%".to_string(), "100%".to_string(), "0.3".to_string())
        );
        assert_eq!(
            format_fields(None),
            ("-".to_string(), "-".to_string(), "-".to_string())
        );
    }
}
