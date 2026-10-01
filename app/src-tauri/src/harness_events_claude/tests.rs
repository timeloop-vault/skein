//! Helpers shared by the sibling `*_tests` modules.

use super::*;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};
use tempfile::TempDir;

/// In-memory Database for tests that want to construct a
/// `ClaudeEventsManager` without touching disk. We don't use the
/// action sink in these phase-focused tests (each `attach_at`
/// passes `None`); the manager just needs *a* db to hold.
pub(super) fn test_manager() -> ClaudeEventsManager {
    let dir = tempfile::TempDir::new().unwrap();
    let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
    // Leak the TempDir — only needed for the duration of the
    // test, and not worth a per-test handle.
    std::mem::forget(dir);
    ClaudeEventsManager::new_for_test(Arc::new(db))
}

/// Drive `attach_at` against a tempdir, returning a receiver the
/// test collects events from. The manager is returned so the test
/// can hold it (dropping ends the watch).
///
/// We use the path-injected variant rather than the env-var-driven
/// `attach()` so tests stay hermetic — `unsafe_code = forbid`
/// means `std::env::set_var` is off the table in this crate, and
/// it would also race across the test binary's parallel threads.
pub(super) fn make_adapter(
    dir: &TempDir,
) -> (ClaudeEventsManager, PathBuf, mpsc::Receiver<ClaudeEvent>) {
    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    let path = dir.path().join("session.jsonl");
    manager
        .attach_at(
            "harness-1",
            path.clone(),
            move |event| {
                tx.send(event).unwrap();
            },
            None,
        )
        .unwrap();
    (manager, path, rx)
}

/// Collect events that arrive within a 2 s window. Returns
/// whatever we got. The 50 ms debounce gives us sub-100 ms ideal
/// latency, but macOS `FSEvents` has a ~500 ms-1 s coarse delivery
/// floor — especially for second-modifications on a file the
/// kernel just saw activity on. 2 s is the headroom we need so
/// flakiness doesn't bite on CI.
pub(super) fn drain(rx: &mpsc::Receiver<ClaudeEvent>) -> Vec<ClaudeEvent> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(ev) => out.push(ev),
            Err(_) => break,
        }
    }
    out
}

/// Variant for assertions of *absence*: we only want to wait long
/// enough to be sure nothing fires. 300 ms is comfortable margin
/// over the debounce without dragging out tests that should fail
/// fast when an event leaks through.
pub(super) fn drain_brief(rx: &mpsc::Receiver<ClaudeEvent>) -> Vec<ClaudeEvent> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(300);
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(ev) => out.push(ev),
            Err(_) => break,
        }
    }
    out
}

pub(super) fn append_lines(path: &Path, lines: &[&str]) {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    f.sync_all().unwrap();
}

/// Windows can briefly refuse to delete or recreate a directory a
/// live `ReadDirectoryChangesW` watch still holds a handle open on
/// — the delete lands in a "pending delete" state until the handle
/// closes. Retries both directions of the #362 repro in `resync_tests::tail_survives_watched_dir_deleted_and_recreated` rather
/// than assume either side succeeds on the first try.
pub(super) fn retry_fs_op(mut op: impl FnMut() -> std::io::Result<()>, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match op() {
            Ok(()) => return,
            Err(e) if std::time::Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
                let _ = e;
            }
            Err(e) => panic!("{what} still failing after retrying for 2s: {e}"),
        }
    }
}
