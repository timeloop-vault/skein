//! The wire shapes the review pane renders.
//!
//! Every type here has a hand-maintained mirror in
//! `app/src/review/api.ts`; when one changes, both change. (#69 tracks
//! generating them instead.) They live in their own module because the
//! frontend contract is the thing most likely to be read on its own —
//! answering "what does the pane actually receive" should not mean
//! reading the query logic that fills it in.

use serde::{Deserialize, Serialize};
use skein_review::Hunk;

use crate::db::ReviewCommentRow;

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
    /// meaningful in `Scope::Branch`.
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
