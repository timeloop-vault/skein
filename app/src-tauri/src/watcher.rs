//! Filesystem watchers — one per active worktree.
//!
//! Each subscriber gets a debounced callback (200ms quiet window) when
//! anything in the watched directory tree changes. We deliberately don't
//! filter `.git/`: the index, HEAD ref, and tag refs all live there and
//! all of them are interesting signals for a "refresh status" trigger.
//! Debouncing absorbs the burst of events a single git operation
//! produces, so the noise never reaches the frontend. A caller can ask
//! for gitignored worktree paths to be dropped as well
//! ([`WatcherManager::start_worktree_watch`], #637); `.git/` stays
//! unfiltered either way.
//!
//! A linked worktree's `.git` is a file, so its HEAD, index and
//! `logs/HEAD` live in `<main>/.git/worktrees/<name>/`, outside the
//! watched folder. `start_with_extra` lets `git_watch_start` add that
//! per-worktree admin dir (never the shared common dir, which would tick
//! every sibling room) so a commit whose files were already on disk still
//! produces a tick (#563).
//!
//! `start_head_watch` is the cheap variant for the status-bar branch (#595):
//! one non-recursive watch of the git dir that holds HEAD, firing only when
//! a batch touches `HEAD` or `HEAD.lock` (git rewrites HEAD by renaming
//! the lock over it). A build in the worktree never reaches it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::notify::RecommendedWatcher;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer, notify::RecursiveMode};
use parking_lot::Mutex;

use crate::ignore_filter::WorktreeIgnore;

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

    /// Start watching `path` recursively and hand `on_change` the paths the
    /// debounced batch actually touched — review discovery (#221) needs to
    /// know *which* files changed, not just that something did, so it can
    /// skip everything else without re-walking the whole worktree on every
    /// tick.
    ///
    /// `Some(paths)` for a normal batch (one entry per path notify
    /// coalesced events for); `None` when notify itself reports an
    /// error (a dropped-events overflow, say) — the caller falls back to
    /// a full re-scan in that case, the same "stale but honest" choice
    /// [`WatcherManager::start_with_extra`] makes by refreshing on an
    /// error too.
    pub fn start_with_paths<F>(
        &self,
        id: String,
        path: &Path,
        on_change: F,
    ) -> Result<(), WatcherError>
    where
        F: Fn(Option<Vec<PathBuf>>) + Send + 'static,
    {
        self.start_inner(id, path, &[], on_change)
    }

    /// Start watching `path` recursively; the same debouncer also watches
    /// `extra` (recursively). `on_change` is invoked from the debouncer's
    /// flush thread after each quiet window that follows a real change.
    /// Failing to watch an extra path is logged and skipped — it never
    /// fails the main watch (#563).
    pub fn start_with_extra<F>(
        &self,
        id: String,
        path: &Path,
        extra: &[PathBuf],
        on_change: F,
    ) -> Result<(), WatcherError>
    where
        F: Fn() + Send + 'static,
    {
        self.start_inner(id, path, extra, move |_paths| on_change())
    }

    /// The worktree watch behind `git_watch_start`: [`start_with_extra`]
    /// semantics, but with `skip_ignored` a batch ticks only if some
    /// reported path survives [`WorktreeIgnore`] (a notify error always
    /// ticks). `.git/` and the `extra` admin dir are never filtered out.
    ///
    /// [`start_with_extra`]: WatcherManager::start_with_extra
    pub fn start_worktree_watch<F>(
        &self,
        id: String,
        path: &Path,
        extra: &[PathBuf],
        skip_ignored: bool,
        tick: F,
    ) -> Result<(), WatcherError>
    where
        F: Fn() + Send + 'static,
    {
        if !skip_ignored {
            return self.start_with_extra(id, path, extra, tick);
        }
        let ignore = Mutex::new(WorktreeIgnore::new(&path.to_string_lossy()));
        self.start_inner(id, path, extra, move |paths| {
            if should_tick(paths, &ignore) {
                tick();
            }
        })
    }

    fn start_inner<F>(
        &self,
        id: String,
        path: &Path,
        extra: &[PathBuf],
        on_change: F,
    ) -> Result<(), WatcherError>
    where
        F: Fn(Option<Vec<PathBuf>>) + Send + 'static,
    {
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| match result {
                Ok(events) => on_change(Some(events.into_iter().map(|e| e.path).collect())),
                Err(_) => on_change(None),
            },
        )
        .map_err(WatcherError::from_err)?;

        debouncer
            .watcher()
            .watch(path, RecursiveMode::Recursive)
            .map_err(WatcherError::from_err)?;

        for extra_path in extra {
            if let Err(e) = debouncer
                .watcher()
                .watch(extra_path, RecursiveMode::Recursive)
            {
                tracing::warn!(path = %extra_path.display(), error = %e, "extra watch failed");
            }
        }

        self.inner.lock().insert(id, debouncer);
        Ok(())
    }

    /// Watch `dir` non-recursively; call `on_change` only when a batch
    /// touched `HEAD` (see [`touches_head`]). A notify error fires too.
    pub fn start_head_watch<F>(
        &self,
        id: String,
        dir: &Path,
        on_change: F,
    ) -> Result<(), WatcherError>
    where
        F: Fn() + Send + 'static,
    {
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| {
                let fire = match result {
                    Ok(events) => touches_head(events.iter().map(|e| e.path.as_path())),
                    Err(_) => true,
                };
                if fire {
                    on_change();
                }
            },
        )
        .map_err(WatcherError::from_err)?;
        debouncer
            .watcher()
            .watch(dir, RecursiveMode::NonRecursive)
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

/// `None` (notify error, events may have been lost) ticks; a batch ticks
/// when at least one path is not gitignored.
fn should_tick(paths: Option<Vec<PathBuf>>, ignore: &Mutex<WorktreeIgnore>) -> bool {
    paths.is_none_or(|p| !ignore.lock().retain_unignored(p).is_empty())
}

/// True when any path's file name is `HEAD` or `HEAD.lock` (not
/// `ORIG_HEAD`, `FETCH_HEAD`, ...). `HEAD.lock` counts because git writes
/// HEAD by renaming the lock over it, and a platform that reports a rename
/// by its source path alone (`FSEvents` can) would otherwise miss it. The
/// rename lands inside the same debounce window, so the refresh sees the
/// new HEAD; a lock that is never renamed costs one spare refresh.
fn touches_head<'a>(paths: impl IntoIterator<Item = &'a Path>) -> bool {
    paths.into_iter().any(|p| {
        p.file_name()
            .is_some_and(|n| n == "HEAD" || n == "HEAD.lock")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(rx: &std::sync::mpsc::Receiver<()>) {
        while rx.try_recv().is_ok() {}
    }

    fn write(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn filtered_watch_skips_ignored_but_ticks_on_source_and_git() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let manager = WatcherManager::new();
        manager
            .start_worktree_watch("t".into(), &root, &[], true, move || {
                let _ = tx.send(());
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        drain(&rx);

        write(&root, "target/a.o");
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err());

        write(&root, "src/a.rs");
        assert!(rx.recv_timeout(Duration::from_secs(3)).is_ok());
        drain(&rx);

        write(&root, ".git/refs/heads/x");
        assert!(rx.recv_timeout(Duration::from_secs(3)).is_ok());
    }

    fn hit(paths: &[&str]) -> bool {
        touches_head(paths.iter().map(Path::new))
    }

    #[test]
    fn head_matches_exactly() {
        assert!(hit(&["/r/.git/HEAD"]));
        assert!(hit(&["/r/.git/index.lock", "/r/.git/HEAD"]));
        assert!(hit(&["/r/.git/HEAD.lock"]));
    }

    #[test]
    fn near_misses_do_not_match() {
        assert!(!hit(&[]));
        assert!(!hit(&["/r/.git/ORIG_HEAD", "/r/.git/FETCH_HEAD"]));
        assert!(!hit(&["/r/.git/index", "/r/.git/HEAD/x"]));
    }
}
