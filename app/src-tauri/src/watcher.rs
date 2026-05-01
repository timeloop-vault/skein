//! Filesystem watchers — one per active worktree.
//!
//! Each subscriber gets a debounced callback (200ms quiet window) when
//! anything in the watched directory tree changes. We deliberately don't
//! filter `.git/`: the index, HEAD ref, and tag refs all live there and
//! all of them are interesting signals for a "refresh status" trigger.
//! Debouncing absorbs the burst of events a single git operation
//! produces, so the noise never reaches the frontend.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use notify_debouncer_mini::notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer, notify::RecursiveMode};
use parking_lot::Mutex;

const DEBOUNCE_MS: u64 = 200;

#[derive(Debug)]
pub struct WatcherError(pub String);

impl std::fmt::Display for WatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WatcherError {}

impl WatcherError {
    fn from_err<E: std::fmt::Display>(e: E) -> Self {
        Self(e.to_string())
    }
}

/// `Debouncer` ties together the `notify` watcher and the timer that
/// flushes batched events. We just need to keep it alive for as long as
/// the watcher should run — dropping it stops the watch.
type ManagedDebouncer = Debouncer<RecommendedWatcher>;

#[derive(Default)]
pub struct WatcherManager {
    inner: Mutex<HashMap<String, ManagedDebouncer>>,
}

impl WatcherManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start watching `path` recursively. `on_change` is invoked from a
    /// background thread (the debouncer's flush thread) every time a
    /// quiet window passes after a real filesystem change.
    pub fn start<F>(&self, id: String, path: &Path, on_change: F) -> Result<(), WatcherError>
    where
        F: Fn() + Send + 'static,
    {
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| {
                // Errors from notify are usually transient (dropped
                // events when the queue overflows). We surface them as
                // a refresh anyway — better stale-but-honest than
                // silently missing a change.
                if result.is_ok() || result.is_err() {
                    on_change();
                }
            },
        )
        .map_err(WatcherError::from_err)?;

        debouncer
            .watcher()
            .watch(path, RecursiveMode::Recursive)
            .map_err(WatcherError::from_err)?;

        self.inner.lock().insert(id, debouncer);
        Ok(())
    }

    /// Stop the watcher with `id`. No-op if `id` is unknown — callers
    /// retrying on an already-stopped watcher shouldn't see an error.
    pub fn stop(&self, id: &str) {
        // Dropping the debouncer ends the watch.
        self.inner.lock().remove(id);
    }
}
