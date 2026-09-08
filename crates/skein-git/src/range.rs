//! Commit-range queries — the review surface's git half (#212, epic
//! #52 D1).
//!
//! A review's scope is `merge-base(HEAD, <base>)..HEAD`: the work this
//! room's branch adds, which is exactly what the user reviews on GitHub
//! today. Everything here serves that one question, in three shapes the
//! pane needs:
//!
//! * **the range itself** — [`Repo::merge_base`] and
//!   [`Repo::commits_between`], for the commit list and the header count;
//! * **its diff** — [`Repo::diff_trees`] (one commit, or the whole
//!   range) and [`Repo::diff_tree_to_workdir`] (the range *plus*
//!   whatever is still uncommitted, which is the honest view of "what
//!   this branch does");
//! * **content at a revision** — [`Repo::blob_at`], which is what a
//!   comment anchor is recomputed against.
//!
//! Reads only. Every mutation the epic wants — merge, push — is #214,
//! and D9 already decided those go through the git CLI rather than
//! libgit2.

use git2::{Commit, DiffOptions, Oid};

use crate::{FileDiff, GitError, Repo, Result};

/// How many commits a range will report before it stops walking. A
/// review of more than this is not a review, and the revwalk is the one
/// part of the pane that scales with repository history rather than
/// with the change.
pub const MAX_RANGE_COMMITS: usize = 500;

/// One commit in a review range.
///
/// `summary` and `body` are split the way git splits them (first line
/// vs the rest) so the commit list can show one and the detail view the
/// other. Both are lossy-decoded: a commit message that is not UTF-8
/// should render as replacement characters, never vanish from the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    /// First 8 of `sha` — display only, never an identity.
    pub short_sha: String,
    pub summary: String,
    pub body: String,
    pub author_name: String,
    pub author_email: String,
    /// Author time as epoch milliseconds, UTC. The timezone offset is
    /// dropped: the pane formats in local time, and carrying the offset
    /// would only invite applying it twice.
    pub time_ms: i64,
    /// Parent shas. Length > 1 marks a merge commit, which the diff
    /// views treat as first-parent.
    pub parents: Vec<String>,
}

fn to_info(commit: &Commit<'_>) -> CommitInfo {
    let sha = commit.id().to_string();
    let author = commit.author();
    let text = String::from_utf8_lossy(commit.message_bytes());
    let (summary, body) = match text.split_once('\n') {
        Some((first, rest)) => (first.trim_end().to_owned(), rest.trim().to_owned()),
        None => (text.trim_end().to_owned(), String::new()),
    };
    CommitInfo {
        short_sha: sha.chars().take(8).collect(),
        sha,
        summary,
        body,
        author_name: String::from_utf8_lossy(author.name_bytes()).into_owned(),
        author_email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
        time_ms: author.when().seconds().saturating_mul(1000),
        parents: commit.parent_ids().map(|id| id.to_string()).collect(),
    }
}

/// Candidate base branches, most authoritative first. `origin/HEAD` is
/// what the remote itself says its default branch is; the rest are the
/// conventional names, in the order a repo is likely to use them.
const BASE_CANDIDATES: [&str; 5] = ["origin/HEAD", "main", "master", "trunk", "develop"];

impl Repo {
    /// Resolve any revision spec — a branch name, a tag, a sha, `HEAD` —
    /// to a commit sha.
    ///
    /// `None` rather than an error when the revision simply does not
    /// exist: the review pane asks about a base branch the user typed or
    /// that Skein guessed, and "there is no such branch" is a normal
    /// answer it renders, not a failure.
    pub fn resolve_commit(&self, rev: &str) -> Result<Option<String>> {
        let Ok(object) = self.repo.revparse_single(rev) else {
            return Ok(None);
        };
        Ok(object.peel_to_commit().ok().map(|c| c.id().to_string()))
    }

    /// The merge base of two revisions — where the review range starts.
    ///
    /// `None` when either revision is unresolvable, or when the two have
    /// no common ancestor at all (unrelated histories, which a grafted
    /// or freshly re-initialised repo really does produce). The caller
    /// treats that as "diff against the empty tree", so an orphan branch
    /// reviews as entirely added rather than as nothing.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let (Some(a_sha), Some(b_sha)) = (self.resolve_commit(a)?, self.resolve_commit(b)?) else {
            return Ok(None);
        };
        let (Ok(a_oid), Ok(b_oid)) = (Oid::from_str(&a_sha), Oid::from_str(&b_sha)) else {
            return Ok(None);
        };
        Ok(self
            .repo
            .merge_base(a_oid, b_oid)
            .ok()
            .map(|oid| oid.to_string()))
    }

    /// Commits reachable from `head_rev` but not from `base_sha`,
    /// newest first — the review's commit list.
    ///
    /// `base_sha` `None` walks the whole history of `head_rev`, which is
    /// what an unrelated-histories range degrades to. Either way the
    /// walk stops at [`MAX_RANGE_COMMITS`]; the pane says the range was
    /// truncated rather than pretending it is shorter than it is.
    pub fn commits_between(
        &self,
        base_sha: Option<&str>,
        head_rev: &str,
    ) -> Result<Vec<CommitInfo>> {
        let Some(head_sha) = self.resolve_commit(head_rev)? else {
            return Ok(Vec::new());
        };
        let mut walk = self.repo.revwalk()?;
        walk.push(Oid::from_str(&head_sha)?)?;
        if let Some(base) = base_sha {
            // A base that no longer resolves (a branch deleted under the
            // room) must not empty the list — the range just widens.
            if let Ok(oid) = Oid::from_str(base) {
                let _ = walk.hide(oid);
            }
        }
        let mut out = Vec::new();
        for oid in walk {
            let Ok(oid) = oid else { continue };
            let Ok(commit) = self.repo.find_commit(oid) else {
                continue;
            };
            out.push(to_info(&commit));
            if out.len() >= MAX_RANGE_COMMITS {
                break;
            }
        }
        Ok(out)
    }

    /// One commit's metadata, or `None` when the revision is unknown.
    pub fn commit_info(&self, rev: &str) -> Result<Option<CommitInfo>> {
        let Some(sha) = self.resolve_commit(rev)? else {
            return Ok(None);
        };
        let commit = self.repo.find_commit(Oid::from_str(&sha)?)?;
        Ok(Some(to_info(&commit)))
    }

    /// Diff between two revisions' trees. `from` `None` means the empty
    /// tree, so the result is `to` in its entirety as additions.
    ///
    /// Serves both granularities the review has: a single commit
    /// (`parent → commit`) and the whole committed range
    /// (`merge-base → HEAD`).
    pub fn diff_trees(&self, from: Option<&str>, to: &str) -> Result<Vec<FileDiff>> {
        let from_tree = match from {
            Some(rev) => Some(self.tree_at(rev)?),
            None => None,
        };
        let to_tree = self.tree_at(to)?;
        let mut opts = diff_options();
        let diff =
            self.repo
                .diff_tree_to_tree(from_tree.as_ref(), Some(&to_tree), Some(&mut opts))?;
        let mut files = self.collect_file_diffs(&diff)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// Diff a revision's tree against what is on disk right now, index
    /// changes folded in and untracked files included.
    ///
    /// This is the review's default view: `merge-base → working tree`
    /// answers "what does this branch do", counting work the agent has
    /// committed and work it has not. It is the same question a PR
    /// answers, and the only one that does not lie by omission while the
    /// agent is still mid-task.
    pub fn diff_tree_to_workdir(&self, from: Option<&str>) -> Result<Vec<FileDiff>> {
        let from_tree = match from {
            Some(rev) => Some(self.tree_at(rev)?),
            None => None,
        };
        let mut opts = diff_options();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        opts.show_untracked_content(true);
        let diff = self
            .repo
            .diff_tree_to_workdir_with_index(from_tree.as_ref(), Some(&mut opts))?;
        let mut files = self.collect_file_diffs(&diff)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// One commit's own diff — against its first parent, or against the
    /// empty tree for a root commit.
    ///
    /// First-parent for merges on purpose: a merge's diff against both
    /// parents re-reports every change from the merged branch, which in
    /// a review reads as the agent having written work it only pulled in.
    pub fn diff_commit(&self, rev: &str) -> Result<Vec<FileDiff>> {
        let Some(sha) = self.resolve_commit(rev)? else {
            return Ok(Vec::new());
        };
        let commit = self.repo.find_commit(Oid::from_str(&sha)?)?;
        let parent = commit.parent_id(0).ok().map(|id| id.to_string());
        self.diff_trees(parent.as_deref(), &sha)
    }

    /// The content of `relpath` (repo-relative, forward slashes) at
    /// `rev`, or `None` when that revision has no such blob.
    ///
    /// The generalisation of [`Repo::head_blob`], and what a comment
    /// anchor is recomputed against: a thread written against an older
    /// commit needs the text it was written on, not the text there now.
    pub fn blob_at(&self, rev: &str, relpath: &str) -> Result<Option<Vec<u8>>> {
        let Ok(object) = self.repo.revparse_single(rev) else {
            return Ok(None);
        };
        let Ok(tree) = object.peel_to_tree() else {
            return Ok(None);
        };
        let Ok(entry) = tree.get_path(std::path::Path::new(relpath)) else {
            return Ok(None);
        };
        let object = entry.to_object(&self.repo)?;
        Ok(object.as_blob().map(|b| b.content().to_vec()))
    }

    /// Skein's guess at the branch a review should be scoped against.
    ///
    /// The remote's own answer first, then the conventional names. The
    /// guess is only ever a default — #212 persists a per-room override
    /// precisely because a stacked branch's real base is the branch
    /// below it, which no heuristic can know.
    ///
    /// Never returns the current HEAD branch: a review of a branch
    /// against itself is empty, and an empty pane is the one outcome
    /// that gives the user nothing to correct.
    pub fn default_base_branch(&self) -> Option<String> {
        let head = self.head_branch();
        for candidate in BASE_CANDIDATES {
            let name = if candidate == "origin/HEAD" {
                // Symbolic: `origin/HEAD` points at `origin/<default>`.
                // Missing on a clone that never fetched it, which is
                // common — fall through to the conventional names.
                let Some(target) = self
                    .repo
                    .find_reference("refs/remotes/origin/HEAD")
                    .ok()
                    .and_then(|r| r.symbolic_target().map(str::to_owned))
                    .and_then(|t| t.strip_prefix("refs/remotes/").map(str::to_owned))
                else {
                    continue;
                };
                target
            } else {
                candidate.to_owned()
            };
            if head.as_deref() == Some(name.as_str()) {
                continue;
            }
            if matches!(self.resolve_commit(&name), Ok(Some(_))) {
                return Some(name);
            }
        }
        None
    }

    fn tree_at(&self, rev: &str) -> Result<git2::Tree<'_>> {
        let object = self
            .repo
            .revparse_single(rev)
            .map_err(|_| GitError::BranchNotFound(rev.to_owned()))?;
        Ok(object.peel_to_tree()?)
    }
}

fn diff_options() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.context_lines(3);
    opts
}
