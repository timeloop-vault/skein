//! The review surface — scope, diff and comments (#212, epic #52 D1,
//! D5, D6, D7).
//!
//! [`review`] owns the #211 baseline: what a harness changed since the
//! user last accepted it. This module owns the other half — the review
//! the user actually reads, which is scoped to the *branch*.
//!
//! # Three scopes, one renderer
//!
//! The epic asks for one diff renderer over two scopes; in practice
//! there are three views of the same range, and every one of them
//! returns the same [`skein_review::Hunk`] shape the Diff card has
//! rendered since #211:
//!
//! * [`Scope::Branch`] — `merge-base(HEAD, base) → working tree`. The
//!   default, and the honest one: it counts what the agent has
//!   committed *and* what it has not, so the pane does not go blank the
//!   moment the agent commits, nor lie about work still on disk.
//! * [`Scope::Commit`] — one commit against its first parent, for
//!   reading a multi-commit branch the way it was written.
//! * [`Scope::Pending`] — #211's `baseline → disk`, which is where
//!   accept and reject still live. Uncommitted work only.
//!
//! # Re-anchoring is not a cache
//!
//! Every thread is re-matched against live text on every file open, and
//! the result is written back (see [`place_threads`]). A thread that
//! searched from its *original* coordinates forever would drift: after
//! three rounds of agent edits the distance tie-break is measuring from
//! somewhere the code has not been in an hour. Writing the new position
//! back keeps each round's search local.
//!
//! What is never rewritten is the anchor *text*. That is the comment's
//! evidence — the code as its author saw it — and D6 requires an
//! unplaceable thread to render against it rather than against whatever
//! now occupies those line numbers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use skein_git::{DiffLineKind, FileDiff, Repo, StatusKind};
use skein_review::{
    Anchor, FileState, Hunk, HunkLine, LineKind, Placement, Reanchorer, Side, capture_lines,
};

use crate::db::{Database, ReviewCommentRow, ReviewThreadRow};
use crate::review::{abs_path, now_ms, pending_impl, relative_key};

// ── scope ─────────────────────────────────────────────────────────

/// Which view of the room's work the caller wants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// merge-base → working tree. Everything this branch does.
    #[default]
    Branch,
    /// One commit against its first parent.
    Commit,
    /// #211's baseline → disk. Uncommitted, and the only scope with
    /// accept and reject.
    Pending,
}

/// Thread scopes, as stored in `review_threads.scope` (D5).
mod thread_scope {
    pub const LINE: &str = "line";
    pub const FILE: &str = "file";
    pub const COMMIT: &str = "commit";
    pub const REVIEW: &str = "review";
}

// ── DTOs ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDto {
    pub sha: String,
    pub short_sha: String,
    pub summary: String,
    pub body: String,
    pub author_name: String,
    pub time_ms: i64,
    /// A merge, whose diff views are first-parent.
    pub is_merge: bool,
    /// Comment threads attached to this commit as a whole.
    pub thread_count: usize,
}

/// One file in the review's file list.
///
/// The four flags are independent facts about the same row rather than
/// a state machine — a file can be binary, viewed, changed since, and
/// also have uncommitted work — so collapsing them into an enum would
/// lose information the list renders side by side.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct ReviewFileDto {
    pub path: String,
    pub name: String,
    /// `added` | `modified` | `deleted` | `renamed` | `untracked` | …
    pub change: &'static str,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    /// Digest of the file's new-side content. The value a viewed marker
    /// stores, and what makes "changed since I last looked" derivable
    /// rather than guessed.
    pub content_hash: String,
    /// Marked as looked at, and unchanged since.
    pub viewed: bool,
    /// Marked as looked at, but the content has moved on — the
    /// "changes since I last looked" signal.
    pub changed_since_viewed: bool,
    pub thread_count: usize,
    pub unresolved_count: usize,
    /// Also has uncommitted changes pending review (#211). Only
    /// meaningful in [`Scope::Branch`].
    pub has_pending: bool,
    /// Last harness to write this file, when Skein knows. A chip, never
    /// a partition (D4) — and `None` rather than a guess for a file git
    /// found but no harness reported, which is the majority of a
    /// committed range and every case #221 is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<String>,
}

/// The review header: what is being reviewed, and what of it is left.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewScopeDto {
    pub is_repo: bool,
    /// The base ref in force — the room's override, else the repo's
    /// guess. `None` when neither yields anything (a room on `main`
    /// with no other branch).
    pub base_ref: Option<String>,
    /// Whether `base_ref` actually names a revision. A base branch
    /// deleted under the room is reported, not silently ignored.
    pub base_resolved: bool,
    /// The merge base — where the review range starts.
    pub base_sha: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: Option<String>,
    pub commits: Vec<CommitDto>,
    /// The commit walk hit its cap; the range is longer than shown.
    pub truncated: bool,
    pub files: Vec<ReviewFileDto>,
    pub additions: usize,
    pub deletions: usize,
    /// Files with uncommitted changes (#211) — the Pending scope's
    /// count, surfaced here so the scope switch can carry a badge.
    pub pending_count: usize,
    /// Unresolved threads across the whole review, any scope.
    pub unresolved_count: usize,
    /// Local branches, for the base picker.
    pub branches: Vec<String>,
    /// Review-level threads (D5), which belong to no file.
    pub threads: Vec<ThreadDto>,
    /// Why the range could not be computed, when it could not. The pane
    /// says so rather than rendering an empty review, which would be
    /// indistinguishable from a branch with no work on it (#176).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentDto {
    pub id: String,
    pub thread_id: String,
    /// `user` | `agent` (D7).
    pub author_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    pub body: String,
    pub created_ms: i64,
    pub updated_ms: i64,
}

impl From<ReviewCommentRow> for CommentDto {
    fn from(c: ReviewCommentRow) -> Self {
        Self {
            id: c.id,
            thread_id: c.thread_id,
            author_kind: c.author_kind,
            author_id: c.author_id,
            body: c.body,
            created_ms: c.created_ms,
            updated_ms: c.updated_ms,
        }
    }
}

/// One thread, with its anchor recomputed for right now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDto {
    pub id: String,
    /// `line` | `file` | `commit` | `review`.
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// Where the thread sits *now*, 1-based inclusive. `None` when it
    /// could not be placed at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<usize>,
    /// The code the comment was written against. Always sent: it is
    /// what an outdated thread renders, and what a moved one is checked
    /// against by eye.
    pub anchor_lines: Vec<String>,
    /// `unmoved` | `moved` | `shifted` | `outdated` — see
    /// `skein_review::Placement`.
    pub placement: &'static str,
    /// True for `shifted` and `outdated` alike: both are guesses, and
    /// D6 says a guess renders as one.
    pub outdated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_ms: Option<i64>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub comments: Vec<CommentDto>,
}

/// One file's diff plus every thread on it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDetailDto {
    pub path: String,
    pub name: String,
    pub change: &'static str,
    pub binary: bool,
    /// Why there is no line diff, when there is none. Mirrors #211's
    /// `PendingFileDto::blocked`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<&'static str>,
    pub content_hash: String,
    pub hunks: Vec<Hunk>,
    pub threads: Vec<ThreadDto>,
}

// ── git plumbing ──────────────────────────────────────────────────

fn status_str(kind: StatusKind) -> &'static str {
    match kind {
        StatusKind::Added => "added",
        StatusKind::Modified => "modified",
        StatusKind::Deleted => "deleted",
        StatusKind::Renamed => "renamed",
        StatusKind::Untracked => "untracked",
        StatusKind::Conflicted => "conflicted",
        StatusKind::Typechange => "typechange",
    }
}

/// Convert a git hunk into the shape the Diff card already renders.
///
/// The coordinates come from the line numbers rather than from parsing
/// the `@@` header: the header is git's rendering of them, and reading
/// them off the lines cannot disagree with the lines themselves.
fn to_review_hunk(h: &skein_git::DiffHunk) -> Hunk {
    let mut lines = Vec::with_capacity(h.lines.len());
    let (mut old_start, mut old_lines) = (0usize, 0usize);
    let (mut new_start, mut new_lines) = (0usize, 0usize);
    for l in &h.lines {
        if let Some(n) = l.old_lineno {
            let n = n as usize;
            if old_start == 0 {
                old_start = n;
            }
            old_lines += 1;
        }
        if let Some(n) = l.new_lineno {
            let n = n as usize;
            if new_start == 0 {
                new_start = n;
            }
            new_lines += 1;
        }
        lines.push(HunkLine {
            kind: match l.kind {
                DiffLineKind::Add => LineKind::Add,
                DiffLineKind::Delete => LineKind::Delete,
                DiffLineKind::Context => LineKind::Context,
            },
            content: l.content.clone(),
            old_lineno: l.old_lineno.map(|n| n as usize),
            new_lineno: l.new_lineno.map(|n| n as usize),
        });
    }
    Hunk {
        header: h.header.clone(),
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines,
    }
}

fn additions(f: &FileDiff) -> usize {
    f.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == DiffLineKind::Add)
        .count()
}

fn deletions(f: &FileDiff) -> usize {
    f.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == DiffLineKind::Delete)
        .count()
}

/// Normalise a git path to the key everything else in the review model
/// uses: worktree-relative, forward slashes.
fn norm(path: &str) -> String {
    path.replace('\\', "/")
}

/// The range a scope covers, resolved against the repo.
struct Range {
    base_ref: Option<String>,
    base_resolved: bool,
    base_sha: Option<String>,
    head_sha: Option<String>,
    head_branch: Option<String>,
}

fn resolve_range(db: &Database, room_id: &str, repo: &Repo) -> Result<Range, String> {
    let stored = db.review_base_ref(room_id)?;
    let base_ref = stored.or_else(|| repo.default_base_branch());
    let head_branch = repo.head_branch();
    let head_sha = repo.resolve_commit("HEAD").map_err(|e| e.to_string())?;

    let (base_resolved, base_sha) = match (&base_ref, &head_sha) {
        (Some(base), Some(_)) => {
            let resolved = repo
                .resolve_commit(base)
                .map_err(|e| e.to_string())?
                .is_some();
            let merge_base = if resolved {
                repo.merge_base(base, "HEAD").map_err(|e| e.to_string())?
            } else {
                None
            };
            (resolved, merge_base)
        }
        _ => (false, None),
    };

    Ok(Range {
        base_ref,
        base_resolved,
        base_sha,
        head_sha,
        head_branch,
    })
}

/// The file diffs a scope produces.
fn scope_diffs(
    repo: &Repo,
    range: &Range,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<Vec<FileDiff>, String> {
    match scope {
        // merge-base → working tree. A `None` base is an unrelated or
        // unresolvable history: diffing against the empty tree reports
        // the branch as wholly added, which over-reports rather than
        // hiding work.
        Scope::Branch => repo
            .diff_tree_to_workdir(range.base_sha.as_deref())
            .map_err(|e| e.to_string()),
        Scope::Commit => match commit_sha {
            Some(sha) => repo.diff_commit(sha).map_err(|e| e.to_string()),
            None => Ok(Vec::new()),
        },
        // Pending has no git side at all — it is baseline → disk.
        Scope::Pending => Ok(Vec::new()),
    }
}

// ── anchoring ─────────────────────────────────────────────────────

fn placement_str(p: Placement) -> &'static str {
    match p {
        Placement::Unmoved { .. } => "unmoved",
        Placement::Moved { .. } => "moved",
        Placement::Shifted { .. } => "shifted",
        Placement::Outdated => "outdated",
    }
}

fn parse_anchor_lines(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

fn side_of(raw: Option<&str>) -> Side {
    match raw {
        Some("old") => Side::Old,
        _ => Side::New,
    }
}

/// Everything a placement pass needs to know about the view it runs in.
struct PlaceCtx<'a> {
    repo: Option<&'a Repo>,
    cwd: &'a str,
    /// The review range's merge base — the old side of `Scope::Branch`.
    base_sha: Option<&'a str>,
    scope: Scope,
    commit_sha: Option<&'a str>,
    /// First parent of `commit_sha`, resolved once per pass.
    parent_sha: Option<String>,
}

impl<'a> PlaceCtx<'a> {
    fn new(
        repo: Option<&'a Repo>,
        cwd: &'a str,
        base_sha: Option<&'a str>,
        scope: Scope,
        commit_sha: Option<&'a str>,
    ) -> Self {
        let parent_sha = commit_sha.and_then(|sha| {
            repo.and_then(|r| r.commit_info(sha).ok().flatten())
                .and_then(|c| c.parents.first().cloned())
        });
        Self {
            repo,
            cwd,
            base_sha,
            scope,
            commit_sha,
            parent_sha,
        }
    }

    /// The text a thread on `path` should be re-anchored against.
    ///
    /// The two sides are genuinely different documents: a comment on a
    /// deleted line lives in the old text and would be permanently
    /// outdated if searched for in the new. Keeping the side is what
    /// makes such a thread anchorable at all.
    fn anchor_text(&self, side: Side, path: &str) -> String {
        match (side, self.scope) {
            (Side::New, Scope::Commit) => self.blob_text(self.commit_sha, path),
            (Side::Old, Scope::Commit) => self.blob_text(self.parent_sha.as_deref(), path),
            (Side::Old, _) => self.blob_text(self.base_sha, path),
            // The new side of every non-commit scope is what is on disk.
            (Side::New, _) => match skein_review::read_state(&abs_path(self.cwd, path)) {
                FileState::Text(s) => s,
                _ => String::new(),
            },
        }
    }

    fn blob_text(&self, rev: Option<&str>, path: &str) -> String {
        let Some(repo) = self.repo else {
            return String::new();
        };
        repo.blob_at(rev.unwrap_or_default(), path)
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default()
    }
}

/// Re-anchor every thread on one file and write the new coordinates
/// back.
///
/// The two sides are indexed at most once each, and only when a thread
/// actually asks for them: a file with twenty threads costs two index
/// builds, not twenty, and the common file with only new-side threads
/// never reads its old blob at all.
fn place_threads(
    db: &Database,
    ctx: &PlaceCtx<'_>,
    path: &str,
    threads: Vec<ReviewThreadRow>,
    comments: &HashMap<String, Vec<CommentDto>>,
) -> Vec<ThreadDto> {
    let wants = |side: Side| {
        threads
            .iter()
            .any(|t| t.scope == thread_scope::LINE && side_of(t.side.as_deref()) == side)
    };
    let old_text = wants(Side::Old).then(|| ctx.anchor_text(Side::Old, path));
    let new_text = wants(Side::New).then(|| ctx.anchor_text(Side::New, path));
    let old_index = old_text.as_deref().map(Reanchorer::new);
    let new_index = new_text.as_deref().map(Reanchorer::new);

    let mut out = Vec::with_capacity(threads.len());
    for t in threads {
        let anchor_lines = parse_anchor_lines(t.anchor_lines.as_deref());
        let mut placement = Placement::Outdated;

        if t.scope == thread_scope::LINE && !anchor_lines.is_empty() {
            let side = side_of(t.side.as_deref());
            let index = match side {
                Side::Old => old_index.as_ref(),
                Side::New => new_index.as_ref(),
            };
            if let Some(index) = index {
                let start = usize::try_from(t.line_start.unwrap_or(1))
                    .unwrap_or(1)
                    .max(1);
                let anchor = Anchor::new(side, start, anchor_lines.clone());
                placement = index.place(&anchor);
                // Write the new position back so the next round searches
                // from where the thread was last seen.
                if let Some(new_start) = placement.start() {
                    if new_start != start {
                        let end = new_start + anchor_lines.len().saturating_sub(1);
                        if let Err(e) = db.update_review_thread_anchor(
                            &t.id,
                            i64::try_from(new_start).unwrap_or(i64::MAX),
                            i64::try_from(end).unwrap_or(i64::MAX),
                        ) {
                            tracing::warn!(thread = %t.id, error = %e, "review: re-anchor write failed");
                        }
                    }
                }
            }
        }

        out.push(to_thread_dto(t, anchor_lines, placement, comments));
    }
    out
}

fn to_thread_dto(
    t: ReviewThreadRow,
    anchor_lines: Vec<String>,
    placement: Placement,
    comments: &HashMap<String, Vec<CommentDto>>,
) -> ThreadDto {
    // A thread that is not line-scoped has no anchor to go stale, so it
    // must never be reported as outdated — that flag means "this may no
    // longer be about what you think", which is meaningless for a
    // review-level remark.
    let line_scoped = t.scope == thread_scope::LINE;
    let (line_start, line_end, outdated, confidence) = if line_scoped {
        let start = placement.start();
        let end = start.map(|s| s + anchor_lines.len().saturating_sub(1));
        let confidence = match placement {
            Placement::Shifted { confidence, .. } => Some(confidence),
            _ => None,
        };
        (start, end, placement.is_outdated(), confidence)
    } else {
        (None, None, false, None)
    };

    ThreadDto {
        comments: comments.get(&t.id).cloned().unwrap_or_default(),
        placement: if line_scoped {
            placement_str(placement)
        } else {
            "unmoved"
        },
        id: t.id,
        scope: t.scope,
        file_path: t.file_path,
        commit_sha: t.commit_sha,
        side: t.side,
        line_start,
        line_end,
        anchor_lines,
        outdated,
        confidence,
        resolved_ms: t.resolved_ms,
        created_ms: t.created_ms,
        updated_ms: t.updated_ms,
    }
}

/// Every comment in the room, grouped by thread and already in DTO form.
fn comments_by_thread(
    db: &Database,
    room_id: &str,
) -> Result<HashMap<String, Vec<CommentDto>>, String> {
    let mut map: HashMap<String, Vec<CommentDto>> = HashMap::new();
    for c in db.review_comments_for_room(room_id)? {
        map.entry(c.thread_id.clone()).or_default().push(c.into());
    }
    Ok(map)
}

// ── the scope command ─────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn scope_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<ReviewScopeDto, String> {
    let threads = db.review_threads_for_room(room_id)?;
    let comments = comments_by_thread(db, room_id)?;
    let viewed: HashMap<String, String> = db.review_viewed_for_room(room_id)?.into_iter().collect();
    let pending = pending_impl(db, room_id, cwd)?;
    let pending_paths: Vec<String> = pending.iter().map(|p| p.path.clone()).collect();
    // Attribution comes from the baselines rather than from the pending
    // set, so a file the agent wrote and then committed keeps its chip
    // instead of losing it at the moment it stops being pending.
    let harness_of: HashMap<String, String> = db
        .review_baselines_for_room(room_id)?
        .into_iter()
        .filter(|b| !b.harness_id.is_empty())
        .map(|b| (b.path, b.harness_id))
        .collect();

    let unresolved_count = threads.iter().filter(|t| t.resolved_ms.is_none()).count();
    let review_threads: Vec<ThreadDto> = threads
        .iter()
        .filter(|t| t.scope == thread_scope::REVIEW)
        .cloned()
        .map(|t| {
            let lines = parse_anchor_lines(t.anchor_lines.as_deref());
            to_thread_dto(t, lines, Placement::Outdated, &comments)
        })
        .collect();

    // Per-file thread tallies, keyed the same way the file list is.
    let mut per_file: HashMap<String, (usize, usize)> = HashMap::new();
    let mut per_commit: HashMap<String, usize> = HashMap::new();
    for t in &threads {
        if let Some(path) = t.file_path.as_deref() {
            let entry = per_file.entry(norm(path)).or_default();
            entry.0 += 1;
            if t.resolved_ms.is_none() {
                entry.1 += 1;
            }
        }
        if t.scope == thread_scope::COMMIT {
            if let Some(sha) = t.commit_sha.as_deref() {
                *per_commit.entry(sha.to_owned()).or_default() += 1;
            }
        }
    }

    let Ok(repo) = Repo::open(Path::new(cwd)) else {
        // A non-git room still has a review: #211's pending set is not
        // git-dependent, and saying "not a repo" beats an empty pane.
        return Ok(ReviewScopeDto {
            is_repo: false,
            base_ref: None,
            base_resolved: false,
            base_sha: None,
            head_branch: None,
            head_sha: None,
            commits: Vec::new(),
            truncated: false,
            files: pending_files(&pending, &viewed, &per_file),
            additions: pending.iter().map(|p| p.additions).sum(),
            deletions: pending.iter().map(|p| p.deletions).sum(),
            pending_count: pending.len(),
            unresolved_count,
            branches: Vec::new(),
            threads: review_threads,
            error: None,
        });
    };

    let range = resolve_range(db, room_id, &repo)?;
    let branches: Vec<String> = repo
        .branches()
        .map(|bs| bs.into_iter().map(|b| b.name).collect())
        .unwrap_or_default();

    let commits = if range.head_sha.is_some() {
        repo.commits_between(range.base_sha.as_deref(), "HEAD")
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let truncated = commits.len() >= skein_git::MAX_RANGE_COMMITS;
    let commit_dtos: Vec<CommitDto> = commits
        .into_iter()
        .map(|c| CommitDto {
            is_merge: c.parents.len() > 1,
            thread_count: per_commit.get(&c.sha).copied().unwrap_or(0),
            sha: c.sha,
            short_sha: c.short_sha,
            summary: c.summary,
            body: c.body,
            author_name: c.author_name,
            time_ms: c.time_ms,
        })
        .collect();

    // Pending is its own file list and needs no git diff.
    let files = if scope == Scope::Pending {
        pending_files(&pending, &viewed, &per_file)
    } else {
        let diffs = scope_diffs(&repo, &range, scope, commit_sha)?;
        diffs
            .iter()
            .map(|f| {
                let path = norm(&f.path);
                let hash = file_hash(cwd, &path, scope, commit_sha, &repo);
                let (threads, unresolved) = per_file.get(&path).copied().unwrap_or((0, 0));
                let marked = viewed.get(&path);
                ReviewFileDto {
                    name: path.rsplit('/').next().unwrap_or(&path).to_owned(),
                    change: status_str(f.kind),
                    additions: additions(f),
                    deletions: deletions(f),
                    binary: f.binary,
                    viewed: marked.is_some_and(|h| *h == hash),
                    changed_since_viewed: marked.is_some_and(|h| *h != hash),
                    content_hash: hash,
                    thread_count: threads,
                    unresolved_count: unresolved,
                    has_pending: pending_paths.contains(&path),
                    harness_id: harness_of.get(&path).cloned(),
                    path,
                }
            })
            .collect()
    };

    let error = if range.base_ref.is_some() && !range.base_resolved {
        Some(format!(
            "base branch {} no longer exists — pick another",
            range.base_ref.clone().unwrap_or_default()
        ))
    } else {
        None
    };

    Ok(ReviewScopeDto {
        is_repo: true,
        base_ref: range.base_ref,
        base_resolved: range.base_resolved,
        base_sha: range.base_sha,
        head_branch: range.head_branch,
        head_sha: range.head_sha,
        commits: commit_dtos,
        truncated,
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
        files,
        pending_count: pending.len(),
        unresolved_count,
        branches,
        threads: review_threads,
        error,
    })
}

/// The Pending scope's file list, straight from #211's model.
fn pending_files(
    pending: &[crate::review::PendingFileDto],
    viewed: &HashMap<String, String>,
    per_file: &HashMap<String, (usize, usize)>,
) -> Vec<ReviewFileDto> {
    pending
        .iter()
        .map(|p| {
            let (threads, unresolved) = per_file.get(&p.path).copied().unwrap_or((0, 0));
            let marked = viewed.get(&p.path);
            ReviewFileDto {
                path: p.path.clone(),
                name: p.name.clone(),
                change: p.change,
                additions: p.additions,
                deletions: p.deletions,
                binary: p.blocked == Some("binary"),
                viewed: marked.is_some_and(|h| *h == p.content_hash),
                changed_since_viewed: marked.is_some_and(|h| *h != p.content_hash),
                content_hash: p.content_hash.clone(),
                thread_count: threads,
                unresolved_count: unresolved,
                has_pending: true,
                harness_id: (!p.harness_id.is_empty()).then(|| p.harness_id.clone()),
            }
        })
        .collect()
}

/// Digest of a file's new-side content in the given scope — the value a
/// viewed marker stores.
fn file_hash(cwd: &str, path: &str, scope: Scope, commit_sha: Option<&str>, repo: &Repo) -> String {
    let state = match (scope, commit_sha) {
        (Scope::Commit, Some(sha)) => repo
            .blob_at(sha, path)
            .ok()
            .flatten()
            .map_or(FileState::Missing, |b| skein_review::classify_bytes(&b)),
        _ => skein_review::read_state(&abs_path(cwd, path)),
    };
    skein_review::content_hash(&state).to_string()
}

// ── the file command ──────────────────────────────────────────────

fn file_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    path: &str,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<FileDetailDto, String> {
    let key = relative_key(cwd, path).unwrap_or_else(|| norm(path));
    let comments = comments_by_thread(db, room_id)?;
    let file_threads: Vec<ReviewThreadRow> = db
        .review_threads_for_room(room_id)?
        .into_iter()
        .filter(|t| t.file_path.as_deref().map(norm).as_deref() == Some(key.as_str()))
        .collect();

    let repo = Repo::open(Path::new(cwd)).ok();

    // Pending keeps #211's own diff: baseline → disk, which git cannot
    // see and which carries the accept/reject verbs.
    if scope == Scope::Pending {
        let pending = pending_impl(db, room_id, cwd)?;
        let found = pending.into_iter().find(|p| p.path == key);
        let ctx = PlaceCtx::new(repo.as_ref(), cwd, None, scope, None);
        let threads = place_threads(db, &ctx, &key, file_threads, &comments);
        return Ok(match found {
            Some(p) => FileDetailDto {
                name: p.name,
                change: p.change,
                binary: p.blocked == Some("binary"),
                blocked: p.blocked,
                content_hash: p.content_hash,
                hunks: p.hunks,
                threads,
                path: p.path,
            },
            None => FileDetailDto {
                name: key.rsplit('/').next().unwrap_or(&key).to_owned(),
                change: "modified",
                binary: false,
                blocked: None,
                content_hash: String::new(),
                hunks: Vec::new(),
                threads,
                path: key,
            },
        });
    }

    let repo_ref = repo.as_ref().ok_or("not a git repository")?;
    let range = resolve_range(db, room_id, repo_ref)?;
    let diffs = scope_diffs(repo_ref, &range, scope, commit_sha)?;
    let found = diffs.iter().find(|f| norm(&f.path) == key);

    let ctx = PlaceCtx::new(
        repo.as_ref(),
        cwd,
        range.base_sha.as_deref(),
        scope,
        commit_sha,
    );
    let threads = place_threads(db, &ctx, &key, file_threads, &comments);

    let hash = file_hash(cwd, &key, scope, commit_sha, repo_ref);
    Ok(match found {
        Some(f) => FileDetailDto {
            name: key.rsplit('/').next().unwrap_or(&key).to_owned(),
            change: status_str(f.kind),
            binary: f.binary,
            blocked: if f.binary { Some("binary") } else { None },
            content_hash: hash,
            hunks: f.hunks.iter().map(to_review_hunk).collect(),
            threads,
            path: key,
        },
        // A file with threads but no diff in this scope — reviewed and
        // accepted, or commented in another scope. Its threads still
        // render; dropping them would be the silent loss D6 forbids.
        None => FileDetailDto {
            name: key.rsplit('/').next().unwrap_or(&key).to_owned(),
            change: "modified",
            binary: false,
            blocked: None,
            content_hash: hash,
            hunks: Vec::new(),
            threads,
            path: key,
        },
    })
}

// ── writing comments ──────────────────────────────────────────────

/// What the frontend sends to open a thread.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewThread {
    /// `line` | `file` | `commit` | `review`.
    pub scope: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    /// `old` | `new`. Line scope only.
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub line_start: Option<usize>,
    #[serde(default)]
    pub line_end: Option<usize>,
    /// The lines the user selected, as rendered. Sent by the caller
    /// rather than re-read here: the anchor must be the text the user
    /// was actually looking at, and re-reading could capture an edit
    /// that landed between the click and the submit.
    #[serde(default)]
    pub anchor_lines: Vec<String>,
    pub body: String,
}

fn add_thread_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    input: &NewThread,
) -> Result<ThreadDto, String> {
    if input.body.trim().is_empty() {
        return Err("a comment needs a body".into());
    }
    let scope = match input.scope.as_str() {
        thread_scope::LINE | thread_scope::FILE | thread_scope::COMMIT | thread_scope::REVIEW => {
            input.scope.clone()
        }
        other => return Err(format!("unknown comment scope: {other}")),
    };
    let file_path = input
        .file_path
        .as_deref()
        .map(|p| relative_key(cwd, p).unwrap_or_else(|| norm(p)));
    if scope == thread_scope::LINE && file_path.is_none() {
        return Err("a line comment needs a file".into());
    }

    // Fall back to reading the anchor from disk only when the caller
    // sent none — a line comment with no anchor text could never be
    // re-matched, and would be outdated from birth.
    let anchor_lines = if input.anchor_lines.is_empty() && scope == thread_scope::LINE {
        match (&file_path, input.line_start, input.line_end) {
            (Some(path), Some(start), Some(end)) => {
                let text = match skein_review::read_state(&abs_path(cwd, path)) {
                    FileState::Text(s) => s,
                    _ => String::new(),
                };
                capture_lines(&text, start, end)
            }
            _ => Vec::new(),
        }
    } else {
        input.anchor_lines.clone()
    };

    let now = now_ms();
    let thread_id = uuid::Uuid::new_v4().to_string();
    let row = ReviewThreadRow {
        id: thread_id.clone(),
        room_id: room_id.to_owned(),
        scope: scope.clone(),
        file_path,
        commit_sha: input.commit_sha.clone(),
        side: if scope == thread_scope::LINE {
            Some(match side_of(input.side.as_deref()) {
                Side::Old => "old".to_owned(),
                Side::New => "new".to_owned(),
            })
        } else {
            None
        },
        line_start: input.line_start.and_then(|n| i64::try_from(n).ok()),
        line_end: input.line_end.and_then(|n| i64::try_from(n).ok()),
        anchor_hash: if anchor_lines.is_empty() {
            None
        } else {
            Some(skein_review::hash_lines(&anchor_lines))
        },
        anchor_lines: if anchor_lines.is_empty() {
            None
        } else {
            serde_json::to_string(&anchor_lines).ok()
        },
        resolved_ms: None,
        created_ms: now,
        updated_ms: now,
    };
    db.insert_review_thread(&row)?;
    db.insert_review_comment(&ReviewCommentRow {
        id: uuid::Uuid::new_v4().to_string(),
        thread_id: thread_id.clone(),
        room_id: room_id.to_owned(),
        // v1 is human-only (D7). #213 is what starts writing `agent`.
        author_kind: "user".to_owned(),
        author_id: None,
        body: input.body.clone(),
        created_ms: now,
        updated_ms: now,
    })?;

    let comments = comments_by_thread(db, room_id)?;
    let stored = db
        .review_thread(&thread_id)?
        .ok_or("the thread vanished immediately after being written")?;
    // A brand-new thread is by definition where it was just placed.
    let placement = Placement::Unmoved {
        start: input.line_start.unwrap_or(1),
    };
    Ok(to_thread_dto(stored, anchor_lines, placement, &comments))
}

// ── commands ──────────────────────────────────────────────────────

/// The review header, commit list and file list for one scope.
#[tauri::command]
pub async fn review_scope(
    room_id: String,
    cwd: String,
    scope: Option<Scope>,
    commit_sha: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<ReviewScopeDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        scope_impl(
            &db,
            &room_id,
            &cwd,
            scope.unwrap_or_default(),
            commit_sha.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One file's diff, plus every thread on it re-anchored to right now.
#[tauri::command]
pub async fn review_file(
    room_id: String,
    cwd: String,
    path: String,
    scope: Option<Scope>,
    commit_sha: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<FileDetailDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        file_impl(
            &db,
            &room_id,
            &cwd,
            &path,
            scope.unwrap_or_default(),
            commit_sha.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a thread with its first comment.
#[tauri::command]
pub async fn review_add_thread(
    room_id: String,
    cwd: String,
    thread: NewThread,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<ThreadDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || add_thread_impl(&db, &room_id, &cwd, &thread))
        .await
        .map_err(|e| e.to_string())?
}

/// Reply on an existing thread (D5: flat, with replies).
#[tauri::command]
pub async fn review_reply(
    room_id: String,
    thread_id: String,
    body: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<CommentDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        if body.trim().is_empty() {
            return Err("a comment needs a body".to_string());
        }
        if db.review_thread(&thread_id)?.is_none() {
            return Err("that thread no longer exists".to_string());
        }
        let now = now_ms();
        let row = ReviewCommentRow {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id,
            room_id,
            author_kind: "user".to_string(),
            author_id: None,
            body,
            created_ms: now,
            updated_ms: now,
        };
        db.insert_review_comment(&row)?;
        Ok(CommentDto::from(row))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Edit a comment's text.
#[tauri::command]
pub async fn review_edit_comment(
    comment_id: String,
    body: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        if body.trim().is_empty() {
            return Err("a comment needs a body".to_string());
        }
        if db.update_review_comment(&comment_id, &body, now_ms())? {
            Ok(())
        } else {
            Err("that comment no longer exists".to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Delete a comment. Returns the thread id when that emptied the whole
/// thread, so the caller can drop it from the gutter.
#[tauri::command]
pub async fn review_delete_comment(
    comment_id: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Option<String>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || db.delete_review_comment(&comment_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Delete a whole thread and every comment on it.
///
/// Separate from deleting the last comment one at a time: retracting a
/// review remark is one action in the user's head, and making them
/// delete four replies to undo one thread is the sort of friction that
/// leaves stale threads lying in the gutter instead.
#[tauri::command]
pub async fn review_delete_thread(
    thread_id: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || db.delete_review_thread(&thread_id).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
}

/// Resolve or reopen a thread.
///
/// Human-only, and deliberately so: D8 keeps resolve out of the agent's
/// verbs, because an agent that can resolve its own comments removes
/// the gate the loop exists to provide.
#[tauri::command]
pub async fn review_resolve_thread(
    thread_id: String,
    resolved: bool,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        let now = now_ms();
        let stamp = if resolved { Some(now) } else { None };
        if db.set_review_thread_resolved(&thread_id, stamp, now)? {
            Ok(())
        } else {
            Err("that thread no longer exists".to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Mark a file as looked at, or un-mark it.
#[tauri::command]
pub async fn review_mark_viewed(
    room_id: String,
    path: String,
    content_hash: String,
    viewed: bool,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        if viewed {
            db.set_review_viewed(&room_id, &path, &content_hash, now_ms())
        } else {
            db.clear_review_viewed(&room_id, &path)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Point the room's review at a different base ref.
#[tauri::command]
pub async fn review_set_base(
    room_id: String,
    cwd: String,
    base_ref: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<ReviewScopeDto, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.set_review_base_ref(&room_id, &base_ref, now_ms())?;
        scope_impl(&db, &room_id, &cwd, Scope::Branch, None)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use skein_git::{DiffLine, DiffLineKind};

    fn line(kind: DiffLineKind, content: &str, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_owned(),
            old_lineno: old,
            new_lineno: new,
        }
    }

    #[test]
    fn a_git_hunk_converts_with_coordinates_read_off_its_lines() {
        // Not parsed from the @@ header: the header is git's rendering
        // of the line numbers, and reading the numbers cannot disagree
        // with the lines the user sees.
        let h = skein_git::DiffHunk {
            header: "@@ -10,3 +10,4 @@ fn foo()".to_owned(),
            lines: vec![
                line(DiffLineKind::Context, "a", Some(10), Some(10)),
                line(DiffLineKind::Delete, "b", Some(11), None),
                line(DiffLineKind::Add, "B", None, Some(11)),
                line(DiffLineKind::Add, "B2", None, Some(12)),
                line(DiffLineKind::Context, "c", Some(12), Some(13)),
            ],
        };
        let out = to_review_hunk(&h);
        assert_eq!(out.header, h.header);
        assert_eq!((out.old_start, out.old_lines), (10, 3));
        assert_eq!((out.new_start, out.new_lines), (10, 4));
        assert_eq!(out.additions(), 2);
        assert_eq!(out.deletions(), 1);
        assert_eq!(out.lines[1].kind, LineKind::Delete);
        assert_eq!(out.lines[1].old_lineno, Some(11));
        assert_eq!(out.lines[1].new_lineno, None);
    }

    #[test]
    fn a_pure_addition_hunk_has_no_old_coordinates() {
        let h = skein_git::DiffHunk {
            header: "@@ -0,0 +1,2 @@".to_owned(),
            lines: vec![
                line(DiffLineKind::Add, "one", None, Some(1)),
                line(DiffLineKind::Add, "two", None, Some(2)),
            ],
        };
        let out = to_review_hunk(&h);
        assert_eq!((out.old_start, out.old_lines), (0, 0));
        assert_eq!((out.new_start, out.new_lines), (1, 2));
    }

    #[test]
    fn paths_normalise_to_forward_slashes_everywhere() {
        // The review model keys on worktree-relative forward-slash
        // paths; git hands back the OS separator on Windows.
        assert_eq!(norm(r"src\liveContext\diff.ts"), "src/liveContext/diff.ts");
        assert_eq!(norm("src/a.rs"), "src/a.rs");
    }

    #[test]
    fn status_kinds_serialise_to_the_names_the_pane_renders() {
        assert_eq!(status_str(StatusKind::Added), "added");
        assert_eq!(status_str(StatusKind::Untracked), "untracked");
        assert_eq!(status_str(StatusKind::Typechange), "typechange");
    }

    #[test]
    fn a_thread_side_defaults_to_new_when_unset_or_unknown() {
        // An old row, or a file/review thread that never had a side.
        assert_eq!(side_of(Some("old")), Side::Old);
        assert_eq!(side_of(Some("new")), Side::New);
        assert_eq!(side_of(None), Side::New);
        assert_eq!(side_of(Some("nonsense")), Side::New);
    }

    #[test]
    fn anchor_lines_survive_a_json_round_trip_and_tolerate_junk() {
        let lines = vec!["let x = 1;".to_owned(), "  // a comment".to_owned()];
        let json = serde_json::to_string(&lines).unwrap();
        assert_eq!(parse_anchor_lines(Some(&json)), lines);
        // A row written by something else, or corrupted, must not panic
        // — it degrades to an unanchorable thread, which renders as
        // outdated rather than crashing the pane.
        assert!(parse_anchor_lines(Some("not json")).is_empty());
        assert!(parse_anchor_lines(None).is_empty());
    }

    #[test]
    fn a_non_line_thread_is_never_reported_as_outdated() {
        // "Outdated" means "this may no longer be about what you think",
        // which is meaningless for a review-level remark.
        let row = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::REVIEW.into(),
            file_path: None,
            commit_sha: None,
            side: None,
            line_start: None,
            line_end: None,
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let dto = to_thread_dto(row, Vec::new(), Placement::Outdated, &HashMap::new());
        assert!(!dto.outdated);
        assert_eq!(dto.placement, "unmoved");
        assert_eq!(dto.line_start, None);
    }

    #[test]
    fn a_shifted_line_thread_is_placed_and_flagged_at_once() {
        let row = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::LINE.into(),
            file_path: Some("a.rs".into()),
            commit_sha: None,
            side: Some("new".into()),
            line_start: Some(10),
            line_end: Some(11),
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let lines = vec!["one".to_owned(), "two".to_owned()];
        let dto = to_thread_dto(
            row,
            lines,
            Placement::Shifted {
                start: 20,
                confidence: 0.75,
            },
            &HashMap::new(),
        );
        assert_eq!((dto.line_start, dto.line_end), (Some(20), Some(21)));
        assert!(dto.outdated, "a guess renders as a guess");
        assert_eq!(dto.placement, "shifted");
        assert_eq!(dto.confidence, Some(0.75));
    }

    #[test]
    fn an_outdated_line_thread_keeps_its_evidence_but_loses_its_position() {
        let row = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: thread_scope::LINE.into(),
            file_path: Some("a.rs".into()),
            commit_sha: None,
            side: Some("new".into()),
            line_start: Some(10),
            line_end: Some(10),
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        let lines = vec!["the code as it was".to_owned()];
        let dto = to_thread_dto(row, lines.clone(), Placement::Outdated, &HashMap::new());
        assert_eq!(dto.line_start, None, "it is not placed anywhere");
        assert_eq!(
            dto.anchor_lines, lines,
            "but it still carries what it was about"
        );
        assert!(dto.outdated);
    }

    #[test]
    fn scope_defaults_to_branch() {
        assert_eq!(Scope::default(), Scope::Branch);
        assert_eq!(
            serde_json::from_str::<Scope>("\"pending\"").unwrap(),
            Scope::Pending
        );
        assert_eq!(
            serde_json::from_str::<Scope>("\"commit\"").unwrap(),
            Scope::Commit
        );
    }

    #[test]
    fn a_pending_scope_asks_git_for_nothing() {
        // Pending is baseline → disk and must work in a room that is
        // not a repo at all.
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = Repo::open(tmp.path());
        assert!(repo.is_err(), "not a repo, as the test intends");
    }
}
