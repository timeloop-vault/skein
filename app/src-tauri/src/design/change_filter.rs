//! Decides from filesystem state whether a watcher batch is a real change
//! (#433), and keeps the bookkeeping for that bounded (#637).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::commands::should_reload;

/// Most files `seen` remembers before it is reset.
pub(super) const MAX_SEEN: usize = 4096;
/// How far behind "now" the start moves on a reset. Covers a write whose
/// event lands in the next debounce batch.
const SEEN_RESET_SLACK: Duration = Duration::from_secs(2);

/// Decides from filesystem state, not event kinds, whether a batch is a
/// real change. On Linux the inotify backend reports opens and reads, so
/// the preview server's own reads (and the entry listing each tick
/// triggers) would otherwise queue the next reload forever.
///
/// `seen` is capped at [`MAX_SEEN`]: past it the map is cleared and `start`
/// moves to just before now, so an evicted file that is re-read unchanged
/// (mtime before the new start) stays quiet while a fresh write ticks.
pub struct ChangeFilter {
    root: PathBuf,
    start: SystemTime,
    seen: HashMap<PathBuf, (Option<SystemTime>, u64)>,
}

impl ChangeFilter {
    pub fn new(root: PathBuf, start: SystemTime) -> Self {
        Self {
            root,
            start,
            seen: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn seen_len(&self) -> usize {
        self.seen.len()
    }

    /// Whether the batch changed anything; every relevant path is
    /// recorded even once a change is found.
    pub fn observe(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        for p in paths {
            if !should_reload(&self.root, std::slice::from_ref(p)) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(p) else {
                self.seen.remove(p);
                changed = true;
                continue;
            };
            if meta.is_dir() {
                // A file added or removed inside reports its own path.
                continue;
            }
            let stamp = (meta.modified().ok(), meta.len());
            match self.seen.insert(p.clone(), stamp) {
                Some(prev) => changed |= prev != stamp,
                None => changed |= stamp.0.is_none_or(|m| m >= self.start),
            }
        }
        if self.seen.len() > MAX_SEEN {
            self.seen.clear();
            self.start = SystemTime::now()
                .checked_sub(SEEN_RESET_SLACK)
                .unwrap_or(SystemTime::UNIX_EPOCH);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    fn filter(root: &Path, start_offset_secs: i64) -> ChangeFilter {
        let start = if start_offset_secs >= 0 {
            SystemTime::now() + Duration::from_secs(start_offset_secs.unsigned_abs())
        } else {
            SystemTime::now() - Duration::from_secs(start_offset_secs.unsigned_abs())
        };
        ChangeFilter::new(root.to_path_buf(), start)
    }

    #[test]
    fn filter_ignores_a_reread_of_an_unchanged_file() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.html");
        let p = dir.path().join("a.html");
        let mut f = filter(dir.path(), -5);
        assert!(f.observe(std::slice::from_ref(&p)), "new file after start");
        assert!(!f.observe(std::slice::from_ref(&p)));
        assert!(!f.observe(std::slice::from_ref(&p)));
    }

    #[test]
    fn filter_ticks_on_rewrite_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.html");
        let p = dir.path().join("a.html");
        let mut f = filter(dir.path(), 5);
        assert!(!f.observe(std::slice::from_ref(&p)), "old unseen file");
        std::fs::write(&p, "longer body").unwrap();
        assert!(f.observe(std::slice::from_ref(&p)), "len changed");
        assert!(!f.observe(std::slice::from_ref(&p)));
        std::fs::remove_file(&p).unwrap();
        assert!(f.observe(std::slice::from_ref(&p)), "removed");
    }

    #[test]
    fn filter_ignores_directories_and_skipped_paths() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "sub/a.html");
        touch(dir.path(), ".git/x");
        let mut f = filter(dir.path(), -5);
        assert!(!f.observe(&[dir.path().join("sub")]));
        assert!(!f.observe(&[dir.path().join(".git/x")]));
    }

    #[test]
    fn seen_is_bounded_and_eviction_stays_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        let paths: Vec<PathBuf> = (0..MAX_SEEN * 2 + 10)
            .map(|i| {
                let p = dir.path().join(format!("f{i}.js"));
                let file = std::fs::File::create(&p).unwrap();
                file.set_modified(old).unwrap();
                p
            })
            .collect();
        let mut f = filter(dir.path(), 5);
        for chunk in paths.chunks(1000) {
            f.observe(chunk);
            assert!(f.seen_len() <= MAX_SEEN);
        }
        // Evicted and unchanged: no tick. Rewritten: ticks.
        let p = &paths[0];
        assert!(!f.observe(std::slice::from_ref(p)), "evicted unchanged");
        std::fs::write(p, "rewritten, longer").unwrap();
        assert!(f.observe(std::slice::from_ref(p)), "fresh write");
    }
}
