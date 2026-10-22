//! Platform-independent parts: sample/window types, aggregation, the
//! idle-to-busy conversion and counter path construction. The conversion and
//! path helpers are only called from the Windows PDH sampler, but they are
//! platform-neutral, so they stay here and are unit-tested on every OS.

/// One reading of system load.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadSample {
    /// System-wide CPU utilisation, 0..=100.
    pub cpu_pct: f32,
    /// System drive busy percentage, 0..=100.
    pub disk_busy_pct: f32,
    /// System drive current queue length (>= 0).
    pub disk_queue: f32,
}

/// Aggregate of the samples taken since the previous `take_window`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowStats {
    pub cpu_mean: f32,
    pub cpu_max: f32,
    pub disk_busy_mean: f32,
    pub disk_busy_max: f32,
    pub queue_max: f32,
    pub samples: u32,
}

/// Running accumulator behind `take_window`.
#[derive(Debug, Default)]
pub(crate) struct Accum {
    n: u32,
    cpu_sum: f64,
    cpu_max: f32,
    busy_sum: f64,
    busy_max: f32,
    queue_max: f32,
}

impl Accum {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn push(&mut self, s: LoadSample) {
        self.n += 1;
        self.cpu_sum += f64::from(s.cpu_pct);
        self.busy_sum += f64::from(s.disk_busy_pct);
        self.cpu_max = self.cpu_max.max(s.cpu_pct);
        self.busy_max = self.busy_max.max(s.disk_busy_pct);
        self.queue_max = self.queue_max.max(s.disk_queue);
    }

    /// Returns the stats and resets; `None` when no samples arrived.
    pub(crate) fn take(&mut self) -> Option<WindowStats> {
        if self.n == 0 {
            return None;
        }
        let n = f64::from(self.n);
        #[allow(clippy::cast_possible_truncation)]
        let out = WindowStats {
            cpu_mean: (self.cpu_sum / n) as f32,
            cpu_max: self.cpu_max,
            disk_busy_mean: (self.busy_sum / n) as f32,
            disk_busy_max: self.busy_max,
            queue_max: self.queue_max,
            samples: self.n,
        };
        *self = Self::default();
        Some(out)
    }
}

/// Clamp a percentage into 0..=100; NaN becomes 0.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn clamp_pct(v: f64) -> f32 {
    if v.is_nan() {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let c = v.clamp(0.0, 100.0) as f32;
    c
}

/// Busy % from `% Idle Time` (`% Disk Time` can exceed 100, so it is unused).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn busy_from_idle(idle: f64) -> f32 {
    clamp_pct(100.0 - idle)
}

/// Clamp a queue length to a non-negative, non-NaN value.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn clamp_queue(v: f64) -> f32 {
    if v.is_nan() {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let c = v.max(0.0) as f32;
    c
}

/// Normalise a `SystemDrive` value ("C:", "c:\\", "") to "C:".
#[cfg_attr(not(windows), allow(dead_code))]
pub fn normalize_drive(raw: Option<&str>) -> String {
    let t = raw.unwrap_or("").trim();
    let mut chars = t.chars();
    match (chars.next(), chars.next()) {
        (Some(l), Some(':')) if l.is_ascii_alphabetic() => {
            format!("{}:", l.to_ascii_uppercase())
        }
        _ => "C:".to_string(),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub const CPU_PATH: &str = r"\Processor(_Total)\% Processor Time";

#[cfg_attr(not(windows), allow(dead_code))]
pub fn idle_path(drive: &str) -> String {
    format!(r"\LogicalDisk({drive})\% Idle Time")
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn queue_path(drive: &str) -> String {
    format!(r"\LogicalDisk({drive})\Current Disk Queue Length")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(c: f32, b: f32, q: f32) -> LoadSample {
        LoadSample {
            cpu_pct: c,
            disk_busy_pct: b,
            disk_queue: q,
        }
    }

    #[test]
    fn empty_window_is_none() {
        assert!(Accum::default().take().is_none());
    }

    #[test]
    fn aggregates_and_resets() {
        let mut a = Accum::default();
        a.push(s(10.0, 20.0, 1.0));
        a.push(s(30.0, 60.0, 4.0));
        let w = a.take().unwrap();
        assert_eq!(w.samples, 2);
        assert!((w.cpu_mean - 20.0).abs() < 1e-4);
        assert!((w.cpu_max - 30.0).abs() < 1e-4);
        assert!((w.disk_busy_mean - 40.0).abs() < 1e-4);
        assert!((w.disk_busy_max - 60.0).abs() < 1e-4);
        assert!((w.queue_max - 4.0).abs() < 1e-4);
        assert!(a.take().is_none());
    }

    #[test]
    fn idle_to_busy_clamps() {
        assert!((busy_from_idle(75.0) - 25.0).abs() < 1e-4);
        assert!(busy_from_idle(120.0).abs() < 1e-4);
        assert!((busy_from_idle(-5.0) - 100.0).abs() < 1e-4);
        assert!(busy_from_idle(f64::NAN).abs() < 1e-4);
    }

    #[test]
    fn queue_clamps() {
        assert!((clamp_queue(-1.0)).abs() < 1e-4);
        assert!((clamp_queue(2.5) - 2.5).abs() < 1e-4);
    }

    #[test]
    fn drive_normalisation_and_paths() {
        assert_eq!(normalize_drive(Some("d:")), "D:");
        assert_eq!(normalize_drive(Some("E:\\")), "E:");
        assert_eq!(normalize_drive(Some("")), "C:");
        assert_eq!(normalize_drive(None), "C:");
        assert_eq!(idle_path("C:"), r"\LogicalDisk(C:)\% Idle Time");
        assert_eq!(
            queue_path("D:"),
            r"\LogicalDisk(D:)\Current Disk Queue Length"
        );
    }
}
