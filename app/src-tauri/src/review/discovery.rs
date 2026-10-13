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
//! # The ignore cache
//!
//! [`IgnoreCache`] remembers, per directory, whether git ignores it. That
//! is sound because git cannot re-include a file beneath an excluded
//! directory, and libgit2's status does not descend into ignored
//! directories, so [`catch_up`](Discovery::catch_up) agrees. A path is
//! checked shallowest-ancestor-first and the walk stops at the first
//! ignored one, so `node_modules/**` costs one libgit2 call, once.
//!
//! The cache is **cleared** before a batch that names a `.gitignore` or
//! `.git/info/exclude`, on a notify error, and once it is older than
//! [`CACHE_TTL`]. The time bound exists because two inputs are not
//! watched: in a linked worktree `info/exclude` lives in the common git
//! dir outside the watched folder, and the global `core.excludesFile` is
//! never watched at all.
//!
//! An ignore query that errors never drops a path: it is kept (warn log).
//!
//! A tracked or force-added file under an ignored directory is treated as
//! ignored. That is unchanged from before #636: libgit2's `is_path_ignored`
//! already walks parent directories and does not consult the index.
//!
//! The opened `Repo` is held for the watcher's life, so a repo
//! re-initialised in place is only picked up when the watcher restarts,
//! which a room repoint does.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use skein_review::FileState;

use super::{abs_path, capture_state_with_repo, now_ms, relative_key};
use crate::db::{Database, NewBaseline};

/// How long a cached ignore decision may be trusted. See the module docs
/// for the inputs this bounds.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// The "is this repo-relative path ignored" query. A trailing `/` asks
/// about a directory. A seam so tests can count and fail queries.
pub(crate) type IgnoreQuery<'a> = dyn FnMut(&str) -> Result<bool, String> + 'a;

/// Directory-level ignore decisions. See the module docs.
pub(crate) struct IgnoreCache {
    /// dir relpath -> ignored? (`pub(crate)` so tests can inspect it)
    pub(crate) dirs: HashMap<String, bool>,
    born: Instant,
}

impl IgnoreCache {
    pub(crate) fn new() -> Self {
        Self {
            dirs: HashMap::new(),
            born: Instant::now(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.dirs.clear();
        self.born = Instant::now();
    }

    fn expire_if_old(&mut self) {
        if self.born.elapsed() >= CACHE_TTL {
            self.clear();
        }
    }

    /// Is `key` ignored? Ancestors shallowest-first, cached; then the
    /// file itself, uncached (file patterns such as `*.log`). An error
    /// anywhere means "not proven ignored".
    pub(crate) fn is_ignored(&mut self, key: &str, query: &mut IgnoreQuery<'_>) -> bool {
        let mut prefix = String::new();
        let mut segs = key.split('/').peekable();
        while let Some(seg) = segs.next() {
            if segs.peek().is_none() {
                break; // the last segment is the file itself
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(seg);
            match self.dirs.get(&prefix) {
                Some(true) => return true,
                Some(false) => continue,
                None => {}
            }
            // Trailing slash: libgit2 then treats the query as a
            // directory even when it is gone from disk.
            match query(&format!("{prefix}/")) {
                Ok(true) => {
                    self.dirs.insert(prefix, true);
                    return true;
                }
                Ok(false) => {
                    self.dirs.insert(prefix.clone(), false);
                }
                Err(e) => {
                    tracing::warn!(path = %prefix, error = %e, "review: discovery dir ignore check failed");
                }
            }
        }
        match query(key) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(path = %key, error = %e, "review: discovery ignore check failed");
                false
            }
        }
    }
}

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
    // Before the `.git` filter: `.git/info/exclude` is a git path.
    if keys
        .iter()
        .any(|k| k == ".git/info/exclude" || k.rsplit('/').next() == Some(".gitignore"))
    {
        cache.clear();
    } else {
        cache.expire_if_old();
    }
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
        self.ensure_repo();
        let out = self.run(db, reported, true);
        let mut inserted = out.inserted;
        if out.needs_catch_up {
            inserted += self.catch_up(db);
        }
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
