use git2::{Status, StatusOptions};

use crate::{Repo, Result};

/// What kind of change a status entry represents. A single file can have
/// both staged and unstaged modifications — see [`StatusEntry::staged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// New file, staged in the index. Untracked files are
    /// [`StatusKind::Untracked`].
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Brand new file that hasn't been added to the index.
    Untracked,
    Conflicted,
    /// Type changed (e.g. file → symlink). Rare.
    Typechange,
}

/// One entry in the worktree's status. We collapse libgit2's bitfield
/// of possible flags down to a single [`StatusKind`] + a `staged` flag —
/// good enough for a "what changed?" pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub kind: StatusKind,
    /// `true` if the change is in the index (staged), `false` if it's
    /// only in the working tree. A file with both an index change and a
    /// workdir change shows up twice — once with `staged: true` and
    /// once with `staged: false`.
    pub staged: bool,
}

impl Repo {
    /// Enumerate every changed path in the worktree relative to HEAD.
    ///
    /// Untracked files are included (recursing into untracked
    /// directories), but ignored files are not. A file with both staged
    /// and unstaged changes produces two entries — one for each — so
    /// the UI can render them distinctly without re-querying.
    ///
    /// Returned entries are sorted by path for stable rendering.
    pub fn status(&self) -> Result<Vec<StatusEntry>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        opts.include_ignored(false);
        opts.renames_head_to_index(true);
        opts.renames_index_to_workdir(true);

        let statuses = self.repo.statuses(Some(&mut opts))?;
        let mut out = Vec::new();
        for entry in statuses.iter() {
            let path = match entry.path() {
                Some(p) => p.to_owned(),
                None => continue, // non-UTF-8 path — skip rather than surface
            };
            if self.is_dir_entry(&path) {
                continue;
            }
            let s = entry.status();

            // Index-side (staged) view. The order of these checks
            // matters: a single status flag can encode multiple bits,
            // so we prefer the most specific kind first.
            if let Some(kind) = classify_index(s) {
                out.push(StatusEntry {
                    path: path.clone(),
                    kind,
                    staged: true,
                });
            }
            // Workdir-side (unstaged) view, including untracked.
            if let Some(kind) = classify_workdir(s) {
                out.push(StatusEntry {
                    path,
                    kind,
                    staged: false,
                });
            }
        }
        out.sort_by(|a, b| (a.path.as_str(), a.staged).cmp(&(b.path.as_str(), b.staged)));
        Ok(out)
    }
}

fn classify_index(s: Status) -> Option<StatusKind> {
    if s.contains(Status::INDEX_NEW) {
        Some(StatusKind::Added)
    } else if s.contains(Status::INDEX_MODIFIED) {
        Some(StatusKind::Modified)
    } else if s.contains(Status::INDEX_DELETED) {
        Some(StatusKind::Deleted)
    } else if s.contains(Status::INDEX_RENAMED) {
        Some(StatusKind::Renamed)
    } else if s.contains(Status::INDEX_TYPECHANGE) {
        Some(StatusKind::Typechange)
    } else if s.contains(Status::CONFLICTED) {
        // Conflicted is reported as an index-side thing — not strictly
        // staged, but it lives in the index half of the status.
        Some(StatusKind::Conflicted)
    } else {
        None
    }
}

fn classify_workdir(s: Status) -> Option<StatusKind> {
    if s.contains(Status::WT_NEW) {
        Some(StatusKind::Untracked)
    } else if s.contains(Status::WT_MODIFIED) {
        Some(StatusKind::Modified)
    } else if s.contains(Status::WT_DELETED) {
        Some(StatusKind::Deleted)
    } else if s.contains(Status::WT_RENAMED) {
        Some(StatusKind::Renamed)
    } else if s.contains(Status::WT_TYPECHANGE) {
        Some(StatusKind::Typechange)
    } else {
        None
    }
}
