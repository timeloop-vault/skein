//! Watcher-driven baseline discovery (#221), made cheap by #636.
//!
//! A build or an install can write thousands of gitignored files into a
//! worktree in a burst, and the watcher reports every one. The old shape
//! reopened the repo per batch and took the global DB lock per path
//! *before* asking git whether the path was ignored. This module keeps
//! the work proportional to what survives the filters:
//!
//! 1. [`filter_batch`] — key, dedup, drop `.git`, drop ignored. No DB.
//! 2. ONE `review_baseline_paths_present` lookup for the survivors.
//! 3. stat / HEAD blob / disk state per new path, with no lock held.
//! 4. ONE `insert_review_baselines_if_absent` (one lock, one transaction).
//!
//! The ignore cache lives in [`crate::ignore_filter`] (shared with the
//! review pane's watcher, #637).
//!
//! The opened `Repo` is held for the watcher's life, so a repo
//! re-initialised in place is only picked up when the watcher restarts,
//! which a room repoint does.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use skein_review::FileState;

use super::{abs_path, capture_state_with_repo, now_ms, relative_key};
use crate::db::{Database, NewBaseline};
use crate::ignore_filter::{IgnoreCache, IgnoreQuery};

/// What [`filter_batch`] kept and how many it dropped for each reason.
#[derive(Debug, Default)]
pub(crate) struct Filtered {
    pub survivors: Vec<String>,
    /// Paths inside the worktree's `.git`.
    pub under_git: usize,
    pub ignored: usize,
}

/// The pre-DB half of a batch. `is_ignored` is `None` to skip ignore
/// checking (no repo, or `catch_up`, whose paths came from `git status`).
pub(crate) fn filter_batch(
    cwd: &str,
    reported: impl IntoIterator<Item = String>,
    cache: &mut IgnoreCache,
    mut is_ignored: Option<&mut IgnoreQuery<'_>>,
) -> Filtered {
    let mut seen = HashSet::new();
    let keys: Vec<String> = reported
        .into_iter()
        .filter_map(|r| relative_key(cwd, &r))
        .filter(|k| seen.insert(k.clone()))
        .collect();
    cache.note_batch(&keys);
    let mut out = Filtered::default();
    for key in keys {
        if key.split('/').any(|seg| seg == ".git") {
            out.under_git += 1;
            continue;
        }
        if let Some(q) = is_ignored.as_deref_mut()
            && cache.is_ignored(&key, q)
        {
            out.ignored += 1;
            continue;
        }
        out.survivors.push(key);
    }
    out
}

/// Per-batch counters for the one debug line.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub inserted: usize,
    /// New paths skipped because baseline == disk (nothing pending).
    pub unchanged: usize,
    pub needs_catch_up: bool,
}

/// One room's discovery state, owned by its watcher closure behind a
/// `Mutex` (a git2 `Repository` is `Send` but not `Sync`).
pub(crate) struct Discovery {
    room_id: String,
    cwd: String,
    /// Opened once. If the open failed (a non-git room, or a repo that
    /// appeared later via `git init`) it is retried on each later batch:
    /// a failed open is a cheap stat, and it lets a room that becomes a
    /// repo start ignoring and diffing against HEAD without a restart.
    repo: Option<skein_git::Repo>,
    cache: IgnoreCache,
}

impl Discovery {
    pub(crate) fn new(room_id: &str, cwd: &str) -> Self {
        Self {
            room_id: room_id.to_owned(),
            cwd: cwd.to_owned(),
            repo: None,
            cache: IgnoreCache::new(),
        }
    }

    fn ensure_repo(&mut self) {
        if self.repo.is_none() {
            self.repo = skein_git::Repo::open(Path::new(&self.cwd)).ok();
        }
    }

    /// A batch of watcher-reported paths. Returns baselines captured.
    pub(crate) fn discover(
        &mut self,
        db: &Database,
        reported: impl IntoIterator<Item = String>,
    ) -> usize {
        if self.cwd.is_empty() {
            return 0;
        }
        let started = std::time::Instant::now();
        self.ensure_repo();
        let out = self.run(db, reported, true);
        let mut inserted = out.inserted;
        if out.needs_catch_up {
            inserted += self.catch_up(db);
        }
        crate::perf_spans::REVIEW_DISCOVERY_BATCH.record(started.elapsed());
        inserted
    }

    /// The `git status` sweep. Also the reaction to a notify error
    /// (dropped events), so it forgets cached ignore decisions.
    pub(crate) fn catch_up(&mut self, db: &Database) -> usize {
        if self.cwd.is_empty() {
            return 0;
        }
        self.cache.clear();
        self.ensure_repo();
        let Some(repo) = &self.repo else {
            return 0; // non-git: nothing to walk
        };
        let entries = match repo.status() {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(cwd = %self.cwd, error = %e, "review: discovery catch-up status failed");
                return 0;
            }
        };
        // `status()` paths exist or are real tracked deletions, so the
        // needs-catch-up flag is moot; and it already excludes ignored
        // files, so no ignore re-check.
        self.run(db, entries.into_iter().map(|e| e.path), false)
            .inserted
    }

    fn run(
        &mut self,
        db: &Database,
        reported: impl IntoIterator<Item = String>,
        check_ignores: bool,
    ) -> Outcome {
        let started = Instant::now();
        let reported: Vec<String> = reported.into_iter().collect();
        let n_reported = reported.len();
        let repo = self.repo.as_ref();
        let mut query = |p: &str| {
            repo.map_or(Ok(false), |r| {
                r.is_path_ignored(p).map_err(|e| e.to_string())
            })
        };
        let ignore: Option<&mut IgnoreQuery<'_>> = if check_ignores && repo.is_some() {
            Some(&mut query)
        } else {
            None
        };
        let f = filter_batch(&self.cwd, reported, &mut self.cache, ignore);
        let (out, already, max_wait) =
            capture_survivors(db, &self.room_id, &self.cwd, repo, &f.survivors);
        tracing::debug!(
            room_id = %self.room_id,
            reported = n_reported,
            under_git = f.under_git,
            ignored = f.ignored,
            already_baselined = already,
            unchanged = out.unchanged,
            inserted = out.inserted,
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            max_lock_wait_ms = u64::try_from(max_wait.as_millis()).unwrap_or(u64::MAX),
            "review: discovery batch"
        );
        out
    }
}

/// A new path's captured baseline, awaiting the batch insert.
struct Pending {
    key: String,
    kind: &'static str,
    content: Option<String>,
    image: Option<Vec<u8>>,
}

/// Steps 2-4 for already-filtered keys. Returns the outcome, how many
/// were already baselined, and the longest DB lock wait.
fn capture_survivors(
    db: &Database,
    room_id: &str,
    cwd: &str,
    repo: Option<&skein_git::Repo>,
    survivors: &[String],
) -> (Outcome, usize, Duration) {
    let mut out = Outcome::default();
    if survivors.is_empty() {
        return (out, 0, Duration::ZERO);
    }
    let (present, wait1) = match db.review_baseline_paths_present(room_id, survivors) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "review: discovery baseline lookup failed");
            return (out, 0, Duration::ZERO);
        }
    };
    // Computed with no lock held.
    let mut pending = Vec::new();
    for key in survivors.iter().filter(|k| !present.contains(*k)) {
        let abs = abs_path(cwd, key);
        // `symlink_metadata` so a symlink-to-directory doesn't get
        // followed into "yes, it's a directory".
        let meta = std::fs::symlink_metadata(&abs);
        if meta.as_ref().is_ok_and(std::fs::Metadata::is_dir) {
            continue; // not a file to baseline
        }
        let exists = meta.is_ok();
        let (base, image) = capture_state_with_repo(repo, key);
        if !exists && base == FileState::Missing {
            let was_dir_at_head = repo.is_some_and(|r| match r.head_is_dir(key) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(path = %key, error = %e, "review: discovery head_is_dir check failed");
                    false
                }
            });
            if was_dir_at_head {
                out.needs_catch_up = true;
            }
        }
        if base == skein_review::read_state(&abs) {
            out.unchanged += 1;
            continue; // nothing pending — a later write fires another event
        }
        let (kind, content) = skein_review::encode(&base);
        pending.push(Pending {
            key: key.clone(),
            kind,
            content,
            image,
        });
    }
    let already = present.len();
    if pending.is_empty() {
        return (out, already, wait1);
    }
    let rows: Vec<NewBaseline<'_>> = pending
        .iter()
        .map(|p| NewBaseline {
            path: &p.key,
            kind: p.kind,
            content: p.content.as_deref(),
            image: p.image.as_deref(),
        })
        .collect();
    match db.insert_review_baselines_if_absent(room_id, &rows, "", now_ms()) {
        Ok((inserted, wait2)) => {
            out.inserted = inserted.len();
            (out, already, wait1.max(wait2))
        }
        Err(e) => {
            tracing::warn!(error = %e, "review: discovery baseline capture failed");
            (out, already, wait1)
        }
    }
}

#[cfg(test)]
mod tests;
