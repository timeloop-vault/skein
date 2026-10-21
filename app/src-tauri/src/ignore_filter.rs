//! Gitignore filtering shared by review discovery (#636) and the review
//! pane's worktree watcher (#637).
//!
//! A build or an install writes thousands of gitignored files in a burst
//! and the watcher reports every one. Both consumers want to drop those
//! cheaply, so the cache lives here.
//!
//! # The ignore cache
//!
//! [`IgnoreCache`] remembers, per directory, whether git ignores it. That
//! is sound because git cannot re-include a file beneath an excluded
//! directory, and libgit2's status does not descend into ignored
//! directories. A path is checked shallowest-ancestor-first and the walk
//! stops at the first ignored one, so `node_modules/**` costs one libgit2
//! call, once.
//!
//! The cache is **cleared** before a batch that names a `.gitignore` or
//! `.git/info/exclude` ([`IgnoreCache::note_batch`]), on a notify error
//! (the caller's job), and once it is older than [`CACHE_TTL`]. The time
//! bound exists because two inputs are not watched: in a linked worktree
//! `info/exclude` lives in the common git dir outside the watched folder,
//! and the global `core.excludesFile` is never watched at all.
//!
//! An ignore query that errors never drops a path: it is kept (warn log).
//!
//! A tracked or force-added file under an ignored directory is treated as
//! ignored: libgit2's `is_path_ignored` walks parent directories and does
//! not consult the index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::review::relative_key;

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

    /// Call once per batch with its worktree-relative keys, before any
    /// [`is_ignored`](Self::is_ignored): a batch naming a `.gitignore` or
    /// `.git/info/exclude` clears the cache, any other expires it by age.
    pub(crate) fn note_batch(&mut self, keys: &[String]) {
        // Before any `.git` filtering: `.git/info/exclude` is a git path.
        if keys
            .iter()
            .any(|k| k == ".git/info/exclude" || k.rsplit('/').next() == Some(".gitignore"))
        {
            self.clear();
        } else {
            self.expire_if_old();
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

/// Filters absolute watcher paths of one worktree down to the ones git does
/// not ignore. Owns its repo handle (opened lazily, retried while the
/// folder is not yet a repo) and its [`IgnoreCache`].
pub(crate) struct WorktreeIgnore {
    cwd: String,
    repo: Option<skein_git::Repo>,
    cache: IgnoreCache,
}

impl WorktreeIgnore {
    pub(crate) fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_owned(),
            repo: None,
            cache: IgnoreCache::new(),
        }
    }

    /// `paths` minus the gitignored ones, order kept. Always kept: no repo
    /// (nothing to ask), paths outside the worktree (the #563 admin dir),
    /// anything under a `.git` segment, and anything git errors about.
    pub(crate) fn retain_unignored(&mut self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        if self.repo.is_none() {
            self.repo = skein_git::Repo::open(Path::new(&self.cwd)).ok();
        }
        let Some(repo) = &self.repo else {
            return paths;
        };
        let keys: Vec<Option<String>> = paths
            .iter()
            .map(|p| relative_key(&self.cwd, &p.to_string_lossy()))
            .collect();
        let named: Vec<String> = keys.iter().flatten().cloned().collect();
        self.cache.note_batch(&named);
        let mut query = |p: &str| repo.is_path_ignored(p).map_err(|e| e.to_string());
        let cache = &mut self.cache;
        paths
            .into_iter()
            .zip(keys)
            .filter(|(_, key)| match key {
                None => true,
                Some(k) if k.split('/').any(|s| s == ".git") => true,
                Some(k) => !cache.is_ignored(k, &mut query),
            })
            .map(|(p, _)| p)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn init(ignore: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join(".gitignore"), ignore).unwrap();
        dir
    }

    fn cwd(dir: &tempfile::TempDir) -> String {
        dir.path().to_string_lossy().into_owned()
    }

    fn p(dir: &tempfile::TempDir, rel: &str) -> PathBuf {
        dir.path().join(rel)
    }

    #[test]
    fn drops_ignored_keeps_the_rest_in_order() {
        let dir = init("target/\n*.log\n");
        let mut w = WorktreeIgnore::new(&cwd(&dir));
        let outside = std::env::temp_dir().join("elsewhere/x.o");
        let input = vec![
            p(&dir, "target/x"),
            p(&dir, "src/a.rs"),
            p(&dir, "a.log"),
            p(&dir, ".git/HEAD"),
            outside.clone(),
        ];
        let kept = w.retain_unignored(input);
        assert_eq!(
            kept,
            vec![p(&dir, "src/a.rs"), p(&dir, ".git/HEAD"), outside]
        );
    }

    #[test]
    fn non_repo_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = WorktreeIgnore::new(&cwd(&dir));
        let input = vec![p(&dir, "target/x"), p(&dir, "a.log")];
        assert_eq!(w.retain_unignored(input.clone()), input);
    }

    #[test]
    fn editing_gitignore_invalidates_the_cache() {
        let dir = init("*.log\n");
        let mut w = WorktreeIgnore::new(&cwd(&dir));
        let first = w.retain_unignored(vec![p(&dir, "a.log")]);
        assert!(first.is_empty(), "{first:?}");
        fs::write(p(&dir, ".gitignore"), "target/\n").unwrap();
        let kept = w.retain_unignored(vec![p(&dir, ".gitignore"), p(&dir, "a.log")]);
        assert_eq!(kept, vec![p(&dir, ".gitignore"), p(&dir, "a.log")]);
    }
}
