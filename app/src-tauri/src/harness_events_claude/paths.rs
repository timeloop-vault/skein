//! Where a session transcript lives on disk and which directories the tail must watch for it.

use std::fs;
use std::path::{Path, PathBuf};

/// `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`. The encoding
/// and layout live in `skein-harness` (#209); chapter 5 pre-allocates
/// the session uuid, so a fresh spawn can only compute the path — the
/// file does not exist anywhere until the first prompt.
///
/// A transcript that already exists wins over the computed path. That
/// is resume, and the scan is what `resume.rs` already trusts: when the
/// two disagree the encoder has drifted from Claude's (#259 was exactly
/// that), and tailing the computed path would watch a directory Claude
/// never writes to. `None` when the home dir can't be resolved (exotic
/// environments) so the caller can no-op cleanly.
pub(super) fn session_jsonl_path(cwd: &str, session_id: &str) -> Option<PathBuf> {
    let home = crate::home_dir()?;
    let computed = skein_harness::claude::session_jsonl_path(&home, cwd, session_id);
    if computed.is_file() {
        return Some(computed);
    }
    match skein_harness::claude::find_session_jsonl(&home, session_id) {
        Some(found) => {
            tracing::warn!(
                computed = %computed.display(),
                found = %found.display(),
                "claude_events: transcript is not where the cwd encodes to; tailing where it is"
            );
            Some(found)
        }
        None => Some(computed),
    }
}

/// Identifies a directory *instance*, not just its path (#362). A
/// delete followed by a recreate at the same path — the failure this
/// issue is about — must read as a change: on Windows,
/// `ReadDirectoryChangesW` on a directory that gets deleted just stops
/// delivering, silently and with no error, so nothing short of noticing
/// the identity changed will ever re-arm the watch. `dev`+`ino` is the
/// authoritative identity on Unix; Windows exposes no cheap equivalent
/// through `std`, so birth time is the next best signal — good enough
/// given a delete+recreate cycle is seconds apart, not
/// sub-timestamp-resolution apart. (Also covers the symmetric inotify
/// case: a deleted watched directory emits `IN_IGNORED` and the watch
/// is simply gone — `rearm` treats "no longer in `armed`'s desired set"
/// and "identity changed" the same way, so nothing extra was needed for
/// Linux/macOS.)
///
/// Two known ways this identity can fail to *distinguish* a
/// delete+recreate, both accepted because dead-tail detection
/// (`Adapter::check_dead_tail`) is the backstop for either:
///
/// - `of` can't always read one at all — `meta.created()` errors on
///   some filesystems/platforms (review finding on #410: the original
///   version silently dropped the directory from the watch set
///   whenever this failed, which is worse than a merely-imprecise
///   identity — a directory that's never watched can never re-arm even
///   on an ordinary identity change). `None` here means exactly that:
///   "exists, but this platform/filesystem won't say when it was
///   born" — still watched, just unable to distinguish instance A from
///   a same-path instance B. Two `None`s compare equal, which is
///   required for `rearm`'s `desired == armed` fast path to stay quiet
///   on a genuinely unchanged directory whose identity simply can't be
///   read.
/// - On Linux, a rapid delete+recreate can hand back the exact same
///   `(dev, ino)` pair (inode numbers get reused) — `rearm` would then
///   see no change at all, same as the `None` case above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DirId(
    #[cfg(unix)] Option<(u64, u64)>,
    #[cfg(not(unix))] Option<std::time::SystemTime>,
);

impl DirId {
    /// Never fails for a path that exists — `desired_watches` already
    /// checked that with `is_dir()`. A metadata read that fails anyway
    /// (a race, or the identity simply isn't available on this
    /// platform/filesystem) degrades to the "unknown" identity rather
    /// than silently excluding the directory from the watch set — see
    /// the type's own doc comment for what that costs and why it's
    /// accepted.
    pub(super) fn of(path: &Path) -> Self {
        let Ok(meta) = fs::metadata(path) else {
            return Self(None);
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self(Some((meta.dev(), meta.ino())))
        }
        #[cfg(not(unix))]
        {
            Self(meta.created().ok())
        }
    }
}

/// Walks `start` and its ancestors, returning the first that exists as
/// a directory right now. Used when the transcript's own parent isn't
/// there — the desired watch set still needs *some* existing point in
/// the filesystem to sit on, so a later `rearm` pass can notice the
/// real parent appear (as a direct child of whatever this returns, or
/// closer, once Claude creates more of the path).
pub(super) fn nearest_existing_dir(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(p) = cur {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// The watch set an adapter *should* have right now, computed fresh
/// from disk every time it's called — never carried forward (#362: that
/// nothing ever did this after attach was the whole bug). `parent` is
/// the main transcript's directory; `subagents_dir` is the sibling
/// dir, when a path for it could be derived at all (see
/// `TailState::subagents_dir`'s doc comment) — included only when it
/// currently exists, since an armed watch on a directory that isn't
/// there is meaningless.
///
/// Never creates anything: Claude owns both directories, and
/// pre-creating them was part of #362 itself — a watch on a directory
/// Skein invented could point at nothing Claude ever writes to, if the
/// encoder and Claude's own layout ever drift (#259 was exactly that
/// for the main file).
pub(super) fn desired_watches(
    parent: &Path,
    subagents_dir: Option<&Path>,
) -> Vec<(PathBuf, DirId)> {
    let mut out = Vec::new();
    let main_dir = if parent.is_dir() {
        Some(parent.to_path_buf())
    } else {
        parent.parent().and_then(nearest_existing_dir)
    };
    if let Some(dir) = main_dir {
        let id = DirId::of(&dir);
        out.push((dir, id));
    }
    if let Some(dir) = subagents_dir
        && dir.is_dir()
    {
        out.push((dir.to_path_buf(), DirId::of(dir)));
    }
    out
}
