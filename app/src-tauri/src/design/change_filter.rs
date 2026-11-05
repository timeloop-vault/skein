//! Decides from filesystem state whether a watcher batch is a real change
//! (#433), and keeps the bookkeeping for that bounded (#637).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::commands::{list_html_entries, should_reload};

/// Most files `seen` remembers before it is reset.
pub(super) const MAX_SEEN: usize = 4096;
/// Most directories `dirs` remembers before it is reset. Separate from
/// [`MAX_SEEN`] so a flood of file paths never forgets directory stamps.
const MAX_DIRS: usize = 4096;
/// How far behind "now" the start moves on a reset. Covers a write whose
/// event lands in the next debounce batch.
const SEEN_RESET_SLACK: Duration = Duration::from_secs(2);

/// Decides from filesystem state, not event kinds, whether a batch is a
/// real change. On Linux the inotify backend reports opens and reads, so
/// the preview server's own reads (and the entry listing each tick
/// triggers) would otherwise queue the next reload forever.
///
/// A moved-in folder or an old-mtime file arrives as a directory event or
/// as a file with a stale mtime, which the mtime rule alone misses (#709).
/// So a structural path (missing, a new or touched directory, an unseen
/// `.html`/`.htm`) re-walks the entries and ticks when the set differs
/// from `entries`. A directory whose stamp is unchanged never re-walks,
/// or the walk's own opens would loop forever.
///
/// `seen` is capped at [`MAX_SEEN`]: past it it is cleared and `start` moves
/// to just before now, so an evicted file that is re-read unchanged (mtime
/// before the new start) stays quiet while a fresh write ticks. `dirs` has its
/// own cap, [`MAX_DIRS`]; clearing it costs one extra re-walk per directory,
/// which then converges. It is not cleared with `seen`, or on Linux every
/// batch would reset it and the walk's own opens would never settle.
pub struct ChangeFilter {
    root: PathBuf,
    start: SystemTime,
    seen: HashMap<PathBuf, (Option<SystemTime>, u64)>,
    dirs: HashMap<PathBuf, Option<SystemTime>>,
    entries: Vec<String>,
}

fn is_html(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

impl ChangeFilter {
    pub fn new(root: PathBuf, start: SystemTime) -> Self {
        let entries = list_html_entries(&root);
        Self {
            root,
            start,
            seen: HashMap::new(),
            dirs: HashMap::new(),
            entries,
        }
    }

    #[cfg(test)]
    pub(super) fn seen_len(&self) -> usize {
        self.seen.len() + self.dirs.len()
    }

    /// Whether the batch changed anything; every relevant path is
    /// recorded even once a change is found.
    pub fn observe(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        let mut structural = false;
        for p in paths {
            if !should_reload(&self.root, std::slice::from_ref(p)) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(p) else {
                self.seen.remove(p);
                self.dirs.remove(p);
                changed = true;
                structural = true;
                continue;
            };
            if meta.is_dir() {
                let stamp = meta.modified().ok();
                structural |= self.dirs.insert(p.clone(), stamp) != Some(stamp);
                continue;
            }
            let stamp = (meta.modified().ok(), meta.len());
            if let Some(prev) = self.seen.insert(p.clone(), stamp) {
                changed |= prev != stamp;
            } else {
                structural |= is_html(p);
                changed |= stamp.0.is_none_or(|m| m >= self.start);
            }
        }
        if structural {
            let now = list_html_entries(&self.root);
            if now != self.entries {
                self.entries = now;
                changed = true;
            }
        }
        if self.dirs.len() > MAX_DIRS {
            self.dirs.clear();
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

    fn touch_old(root: &Path, rel: &str) -> PathBuf {
        touch(root, rel);
        let p = root.join(rel);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        p
    }

    #[test]
    fn new_dir_with_old_html_ticks_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = filter(dir.path(), 5);
        touch_old(dir.path(), "moved/a.html");
        let batch = [dir.path().join("moved")];
        assert!(f.observe(&batch), "new dir with html");
        assert!(!f.observe(&batch), "same batch again");
    }

    #[test]
    fn unseen_old_html_and_htm_tick() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = filter(dir.path(), 5);
        let a = touch_old(dir.path(), "a.html");
        assert!(f.observe(&[a]));
        let b = touch_old(dir.path(), "B.HTM");
        assert!(f.observe(&[b]));
    }

    #[test]
    fn unseen_old_non_html_does_not_tick() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = filter(dir.path(), 5);
        let x = touch_old(dir.path(), "x.js");
        assert!(!f.observe(&[x]));
    }

    #[test]
    fn removed_and_renamed_folders_tick() {
        let dir = tempfile::tempdir().unwrap();
        touch_old(dir.path(), "one/a.html");
        touch_old(dir.path(), "two/b.html");
        let mut f = filter(dir.path(), 5);
        let one = dir.path().join("one");
        std::fs::remove_dir_all(&one).unwrap();
        assert!(f.observe(&[one]), "removed folder");
        let two = dir.path().join("two");
        let three = dir.path().join("three");
        std::fs::rename(&two, &three).unwrap();
        assert!(f.observe(&[two, three]), "renamed folder");
    }

    #[test]
    fn reobserving_an_unchanged_preexisting_dir_is_quiet() {
        let dir = tempfile::tempdir().unwrap();
        touch_old(dir.path(), "sub/a.html");
        let mut f = filter(dir.path(), 5);
        let batch = [dir.path().join("sub")];
        f.observe(&batch);
        assert!(!f.observe(&batch));
        assert!(!f.observe(&batch));
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

    #[test]
    fn seen_reset_does_not_forget_dirs() {
        let dir = tempfile::tempdir().unwrap();
        touch_old(dir.path(), "sub/a.html");
        let mut f = filter(dir.path(), 5);
        let sub = [dir.path().join("sub")];
        f.observe(&sub);
        let old = SystemTime::now() - Duration::from_secs(3600);
        let paths: Vec<PathBuf> = (0..MAX_SEEN + 10)
            .map(|i| {
                let p = dir.path().join(format!("f{i}.js"));
                let file = std::fs::File::create(&p).unwrap();
                file.set_modified(old).unwrap();
                p
            })
            .collect();
        f.observe(&paths);
        assert!(f.seen.is_empty(), "seen was reset");
        assert_eq!(f.dirs.len(), 1, "dirs survived the reset");
        assert!(!f.observe(&sub));
    }
}
