//! Cheap system-load sampling for Skein's perf telemetry (#639): system CPU
//! %, and the system drive's busy % and queue length.
//!
//! This crate allows `unsafe` (like `skein-winnotify` and `skein-webshot`)
//! because the Windows PDH performance-counter API is raw FFI. Everywhere
//! else the sampler never yields and spawns no thread.

#[cfg(windows)]
mod pdh;
mod window;

use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use window::{LoadSample, WindowStats};

#[derive(Default)]
pub(crate) struct Shared {
    latest: Option<LoadSample>,
    acc: window::Accum,
}

/// Background sampler. Dropping it stops the thread.
pub struct Sampler {
    shared: Arc<Mutex<Shared>>,
    #[cfg(windows)]
    _stop: std::sync::mpsc::Sender<()>,
}

impl Sampler {
    /// Start sampling every `interval`. On non-Windows, or if PDH setup
    /// fails, the sampler simply never yields.
    pub fn start(interval: Duration) -> Sampler {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        #[cfg(windows)]
        {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let sh = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name("sysload".into())
                .spawn(move || pdh::run(&sh, interval, &rx));
            drop(spawned);
            Sampler { shared, _stop: tx }
        }
        #[cfg(not(windows))]
        {
            let _ = interval;
            Sampler { shared }
        }
    }

    /// Most recent sample, if any.
    pub fn latest(&self) -> Option<LoadSample> {
        self.shared.lock().ok()?.latest
    }

    /// Stats since the previous call (or start); resets the window.
    pub fn take_window(&self) -> Option<WindowStats> {
        self.shared.lock().ok()?.acc.take()
    }
}

#[cfg(windows)]
pub(crate) fn record(shared: &Mutex<Shared>, s: LoadSample) {
    if let Ok(mut g) = shared.lock() {
        g.latest = Some(s);
        g.acc.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn unsupported_never_yields() {
        let s = Sampler::start(Duration::from_millis(10));
        assert!(s.latest().is_none());
        assert!(s.take_window().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn smoke_samples_in_range() {
        let s = Sampler::start(Duration::from_millis(500));
        std::thread::sleep(Duration::from_millis(2500));
        let l = s.latest().expect("a sample");
        assert!((0.0..=100.0).contains(&l.cpu_pct));
        assert!((0.0..=100.0).contains(&l.disk_busy_pct));
        assert!(l.disk_queue >= 0.0);
        let w = s.take_window().expect("a window");
        assert!(w.samples >= 1);
        assert!(s.take_window().is_none());
    }
}
