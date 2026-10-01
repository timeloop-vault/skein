//! What an agent can actually do.
//!
//! Mostly plain functions over a [`Database`] and a [`Caller`]. Nothing
//! here knows about HTTP or MCP, which is what makes the interesting
//! properties — room scoping, the resolve prohibition, attribution —
//! testable without a server or a Tauri runtime. [`create_room`] (#330)
//! is the one exception that needs more than that: opening a room means
//! asking the webview to do it (#328's `request_frontend`), so it also
//! takes an [`super::state::AgentApiState`] — itself usable with no
//! Tauri runtime via `AgentApiState::for_test`, so the "no server
//! needed" property still holds in tests.
//!
//! Two rules run through all of them:
//!
//! * **The caller's room is the only room.** Every verb that names a
//!   thread re-reads that thread and compares its `room_id`. A thread
//!   id guessed from another room is [`VerbError::NotFound`], not a
//!   silent read of someone else's review.
//! * **Anchoring is not reimplemented here.** Where a comment sits
//!   *now* comes from [`crate::review_surface::query::file_impl`], the
//!   same call the pane makes, so the agent and the user are never
//!   looking at two different answers to the same question.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::auth::Caller;
use super::element;
use super::element_source::SourceGuess;
use super::render::{covering_hunk, read_around, render_file, render_hunk};
use super::state::AgentApiState;
use crate::db::{
    Database, Harness, HarnessMessageRow, ReviewAddressedRow, ReviewCommentRow, ReviewThreadRow,
    Room,
};
use crate::git::{IdentityCheck, check_identity, repo_root_for_path};
use crate::review::now_ms;
use crate::review_surface::Scope;
use crate::review_surface::element::ElementDto;
use crate::review_surface::query::{ScopeFiles, file_impl, scope_impl};
use crate::room_paths::{normalize_path_for_match, path_match_kind};

/// Rendered diff text is capped so a whole-branch `get_diff` on a large
/// change cannot swallow the agent's context. Truncation is reported,
/// never silent.
const MAX_DIFF_BYTES: usize = 256 * 1024;

/// The largest a mailbox message body may be (#327). A message is a
/// nudge, not a document.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// How many messages one room may send in a rolling minute (#327), and
/// the window it is measured over — a runaway loop has to hit a wall
/// before it can flood a sibling harness.
const SEND_RATE_LIMIT: i64 = 30;
const SEND_RATE_WINDOW_MS: i64 = 60_000;

/// How many unread messages one harness may accumulate before a sender
/// is refused (#327) — an inbox nobody is reading is not a queue, it is
/// a leak.
const MAX_UNREAD_MESSAGES: i64 = 100;

/// `message_history`'s (#364) default and cap on how many rows one call
/// returns. A director paging back through a thread after a compaction
/// reads pages, not a firehose; `limit: 0` is treated as "use the
/// default" rather than refused — an agent-supplied 0 almost always
/// means "no opinion", not "give me nothing".
const DEFAULT_HISTORY_LIMIT: u32 = 100;
const MAX_HISTORY_LIMIT: u32 = 500;

/// Why a verb refused. The HTTP and MCP layers each map these into
/// their own vocabulary; the verbs themselves only say what went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerbError {
    /// No such thread *in this room* — the two cases are deliberately
    /// indistinguishable to the caller, so a token cannot be used to
    /// probe another room for thread ids.
    NotFound(String),
    /// The request was understood and refused.
    Refused(String),
    /// The room has no worktree, or git could not answer.
    Unavailable(String),
    /// Something below us failed.
    Internal(String),
}

impl VerbError {
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(m) | Self::Refused(m) | Self::Unavailable(m) | Self::Internal(m) => m,
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Refused(_) => 403,
            Self::Unavailable(_) => 409,
            Self::Internal(_) => 500,
        }
    }
}

type VerbResult<T> = Result<T, VerbError>;

pub(super) fn internal(e: impl std::fmt::Display) -> VerbError {
    VerbError::Internal(e.to_string())
}

// ── wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListArgs {
    /// `unresolved` (default) or `all`.
    #[serde(default)]
    pub status: Option<String>,
    /// Restrict to one worktree-relative path.
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GetCommentArgs {
    pub thread_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiffArgs {
    #[serde(default)]
    pub file: Option<String>,
    /// `branch` (default), `pending` or `commit`.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReplyArgs {
    pub thread_id: String,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AddressedArgs {
    pub thread_id: String,
    /// The commit that did it, when there is one.
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AgentComment {
    pub author: String,
    pub body: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AgentAddressed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub addressed_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AgentThread {
    pub thread_id: String,
    /// `line` | `file` | `commit` | `review` | `element`.
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// An element thread has no line to be unmoved from: `"source_guess"`
    /// when `source` found a guess, `"element"` when it found none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<&'static str>,
    /// A guess at where an element thread's element is written in source
    /// (file, 1-based inclusive lines, and `via` which evidence found
    /// it). Never a position the reviewer confirmed. Absent for other
    /// threads and when nothing matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceGuess>,
    /// An element thread's stored anchor and state, as the review pane
    /// receives it (`anchor`, `lastSeen`, `state` — camelCase, verbatim).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Where the comment sits *now*, 1-based inclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<usize>,
    /// The code could not be found where the comment was written, so
    /// the position is a guess or missing entirely. Read the anchor.
    /// For an element thread this is true unless the design pane last
    /// reported `anchored` and its content stamp still matches the files
    /// on disk.
    pub outdated: bool,
    pub resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addressed: Option<AgentAddressed>,
    pub comments: Vec<AgentComment>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ListOut {
    pub room_id: String,
    pub threads: Vec<AgentThread>,
    /// Unresolved threads in the room, whatever the filter was — the
    /// number the agent is being measured against.
    pub unresolved_total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CommentDetail {
    #[serde(flatten)]
    pub thread: AgentThread,
    /// The code the comment was written against. This is the anchor
    /// itself, not a cache of it: when the thread is outdated it is the
    /// only honest account of what was meant.
    pub anchor_lines: Vec<String>,
    /// What that region of the file looks like today, when the thread
    /// could be placed at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_context: Option<String>,
    /// The hunk covering the comment in the branch diff, unified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_context: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DiffFile {
    pub path: String,
    pub change: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DiffOut {
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_branch: Option<String>,
    pub files: Vec<DiffFile>,
    pub diff: String,
    /// The diff hit [`MAX_DIFF_BYTES`] and stops early. Ask per file.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ReplyOut {
    pub thread_id: String,
    pub comment_id: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AddressedOut {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Repeated on every call, because it is the one thing about this
    /// API an agent is most likely to assume wrongly: marking a thread
    /// addressed is a claim, not a closure. Only the reviewer resolves.
    pub note_to_agent: &'static str,
}

/// The sign-off, as the agent reads it (#214).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusOut {
    /// The reviewer approved, and HEAD is still what they approved.
    /// The only field a "may I land?" decision should read.
    pub approved: bool,
    /// An approval exists but HEAD has moved past it.
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_since_signoff: Option<usize>,
    pub unresolved_count: usize,
    /// Unresolved threads this agent has not yet claimed to have
    /// handled — its own remaining work.
    pub unaddressed_count: usize,
    /// The flags, in words. A model deciding from `approved: false,
    /// stale: true` has to infer the reason; this states it.
    pub guidance: &'static str,
}

// ── the verbs ─────────────────────────────────────────────────────

/// Every comment on this room's review.
pub fn list_comments(db: &Database, caller: &Caller, args: &ListArgs) -> VerbResult<ListOut> {
    let all = matches!(args.status.as_deref(), Some("all"));
    let filter = args.file.as_deref().map(normalize);

    let rows = db
        .review_threads_for_room(&caller.room_id)
        .map_err(internal)?;
    let unresolved_total = rows.iter().filter(|t| t.resolved_ms.is_none()).count();

    let kept: Vec<ReviewThreadRow> = rows
        .into_iter()
        .filter(|t| all || t.resolved_ms.is_none())
        .filter(|t| match filter.as_deref() {
            None => true,
            Some(want) => t.file_path.as_deref().map(normalize).as_deref() == Some(want),
        })
        .collect();

    let ctx = RoomCtx::load(db, caller, &kept)?;
    Ok(ListOut {
        room_id: caller.room_id.clone(),
        threads: kept.iter().map(|t| ctx.to_agent_thread(t)).collect(),
        unresolved_total,
    })
}

/// One comment, with the code it is about — the anchor, today's file
/// around it, and the hunk it lands in.
pub fn get_comment(
    db: &Database,
    caller: &Caller,
    args: &GetCommentArgs,
) -> VerbResult<CommentDetail> {
    let row = thread_in_room(db, caller, &args.thread_id)?;
    // The empty slice skips `RoomCtx`'s own anchoring pass: this
    // function needs the file's hunks as well as its placements, so it
    // does the one `file_impl` call itself and feeds the result back in
    // rather than paying for the same scope diff twice.
    let is_element = element::is_element(&row);
    let mut ctx = RoomCtx::load(db, caller, element::seed(&row))?;

    let mut diff_context = None;
    if let (Some(cwd), Some(path), false) =
        (caller.cwd.as_deref(), row.file_path.as_deref(), is_element)
    {
        if let Ok(detail) = file_impl(db, &caller.room_id, cwd, path, Scope::Branch, None) {
            for t in &detail.threads {
                ctx.placed
                    .insert(t.id.clone(), (t.line_start, t.line_end, t.outdated));
            }
            diff_context = ctx
                .placed
                .get(&row.id)
                .and_then(|(start, _, _)| *start)
                .and_then(|line| covering_hunk(&detail.hunks, line))
                .map(render_hunk);
        }
    }

    let thread = ctx.to_agent_thread(&row);
    let current_context = match (
        &thread.source,
        caller.cwd.as_deref(),
        row.file_path.as_deref(),
    ) {
        (Some(s), Some(cwd), _) => read_around(cwd, &s.file, s.line_start, s.line_end),
        (None, Some(cwd), Some(path)) => match (thread.line_start, thread.line_end) {
            (Some(start), Some(end)) => read_around(cwd, path, start, end),
            _ => None,
        },
        _ => None,
    };

    Ok(CommentDetail {
        thread,
        anchor_lines: parse_anchor(row.anchor_lines.as_deref()),
        current_context,
        diff_context,
    })
}

/// The review's diff, whole or per file.
pub fn get_diff(db: &Database, caller: &Caller, args: &DiffArgs) -> VerbResult<DiffOut> {
    let cwd = caller
        .cwd
        .as_deref()
        .ok_or_else(|| VerbError::Unavailable("this room has no folder to diff".into()))?;
    let scope = match args.scope.as_deref() {
        None | Some("branch") => Scope::Branch,
        Some("pending") => Scope::Pending,
        Some("commit") => Scope::Commit,
        Some(other) => {
            return Err(VerbError::Refused(format!(
                "unknown scope {other:?} — use branch, pending or commit"
            )));
        }
    };
    if scope == Scope::Commit && args.commit_sha.is_none() {
        return Err(VerbError::Refused(
            "the commit scope needs a commit_sha".into(),
        ));
    }

    let summary = scope_impl(db, &caller.room_id, cwd, scope, args.commit_sha.as_deref())
        .map_err(VerbError::Unavailable)?;
    if let Some(err) = summary.error {
        return Err(VerbError::Unavailable(err));
    }

    let wanted = args.file.as_deref().map(normalize);
    let files: Vec<DiffFile> = summary
        .files
        .iter()
        .filter(|f| wanted.as_deref().is_none_or(|w| f.path == w))
        .map(|f| DiffFile {
            path: f.path.clone(),
            change: f.change.to_owned(),
            additions: f.additions,
            deletions: f.deletions,
        })
        .collect();
    if files.is_empty() {
        if let Some(w) = wanted {
            return Err(VerbError::NotFound(format!(
                "{w} is not in this review's diff"
            )));
        }
    }

    // One diff for every file this call needs, not one per file (#171
    // slice (f)) — `files` is already the exact set `render_file` below
    // will draw from, truncation aside. A single requested file gets a
    // literal pathspec; asking for nothing in particular means `files`
    // is the whole change set already, and naming every one of them as
    // a pathspec would cost strictly more than no filter at all for
    // the identical result.
    let paths: Vec<String> = if wanted.is_some() {
        files.iter().map(|f| f.path.clone()).collect()
    } else {
        Vec::new()
    };
    let snapshot = ScopeFiles::load(
        db,
        &caller.room_id,
        cwd,
        scope,
        args.commit_sha.as_deref(),
        &paths,
    )
    .map_err(VerbError::Unavailable)?;

    let mut diff = String::new();
    let mut truncated = false;
    for f in &files {
        if diff.len() >= MAX_DIFF_BYTES {
            truncated = true;
            break;
        }
        let detail = snapshot.file(db, &f.path).map_err(VerbError::Unavailable)?;
        diff.push_str(&render_file(&detail.path, detail.blocked, &detail.hunks));
    }

    Ok(DiffOut {
        scope: scope_name(scope).to_owned(),
        base_ref: summary.base_ref,
        head_branch: summary.head_branch,
        files,
        diff,
        truncated,
    })
}

/// Answer a comment, as the agent.
pub fn reply(db: &Database, caller: &Caller, args: &ReplyArgs) -> VerbResult<ReplyOut> {
    if args.body.trim().is_empty() {
        return Err(VerbError::Refused("a reply needs a body".into()));
    }
    let row = thread_in_room(db, caller, &args.thread_id)?;
    let now = now_ms();
    let comment = ReviewCommentRow {
        id: uuid::Uuid::new_v4().to_string(),
        thread_id: row.id.clone(),
        room_id: caller.room_id.clone(),
        author_kind: "agent".to_owned(),
        author_id: caller.harness_id.clone(),
        body: args.body.clone(),
        created_ms: now,
        updated_ms: now,
    };
    db.insert_review_comment(&comment).map_err(internal)?;
    Ok(ReplyOut {
        thread_id: row.id,
        comment_id: comment.id,
        author: caller
            .harness_label
            .clone()
            .or_else(|| caller.harness_id.clone())
            .unwrap_or_else(|| "agent".to_owned()),
    })
}

/// Claim a comment is handled. Explicitly *not* resolving it.
pub fn mark_addressed(
    db: &Database,
    caller: &Caller,
    args: &AddressedArgs,
) -> VerbResult<AddressedOut> {
    let row = thread_in_room(db, caller, &args.thread_id)?;
    let addressed = ReviewAddressedRow {
        thread_id: row.id.clone(),
        commit_sha: args.commit_sha.clone(),
        harness_id: caller.harness_id.clone().unwrap_or_default(),
        note: args.note.clone(),
        addressed_ms: now_ms(),
    };
    db.set_thread_addressed(&caller.room_id, &addressed)
        .map_err(internal)?;
    Ok(AddressedOut {
        thread_id: row.id,
        commit_sha: args.commit_sha.clone(),
        note_to_agent: "marked as addressed — only the reviewer can resolve the thread",
    })
}

/// Whether the reviewer has signed off — the gate before landing.
///
/// The one verb whose answer is not about comments. #213 let an agent
/// read the reviewer's feedback; this is what lets it ask whether the
/// reviewer is *finished*, so it can land the branch the way this
/// repository lands branches rather than waiting to be told.
///
/// `approved` is deliberately the only field a decision should read.
/// The rest is context for the message the agent writes afterwards.
pub fn review_status(db: &Database, caller: &Caller) -> VerbResult<StatusOut> {
    let Some(cwd) = caller.cwd.as_deref() else {
        return Err(VerbError::Unavailable(
            "this room has no worktree, so there is nothing to sign off on".into(),
        ));
    };
    signoff_block(db, &caller.room_id, cwd)
}

/// The sign-off, as an agent reads it — shared by [`review_status`]
/// (the caller's own room) and [`get_room`] (#356, an arbitrary room a
/// director asks about), so the two verbs can never drift into
/// reporting different things for the same fact. Callers that have no
/// `cwd` at all report that themselves rather than calling this: there
/// is a real difference between "no worktree" and "worktree, but not a
/// repository", and only `status_impl` (via `can_sign_off`) knows the
/// second one.
fn signoff_block(db: &Database, room_id: &str, cwd: &str) -> VerbResult<StatusOut> {
    let s = crate::review_surface::signoff::status_impl(db, room_id, cwd)
        .map_err(VerbError::Internal)?;

    // Said in words, not just flags. A model reading `approved: false,
    // stale: true` has to infer why; this tells it, in the same place
    // it is deciding.
    let guidance = if s.approved {
        "The reviewer has approved this exact commit. You may land the branch \
         however this repository lands branches."
    } else if s.stale {
        "The reviewer approved an earlier commit, and HEAD has moved since. \
         That approval does not cover the new work — do not land. Tell the \
         reviewer what changed and ask them to look again."
    } else {
        "The reviewer has not signed off. Do not land the branch. Address the \
         outstanding comments and wait."
    };

    Ok(StatusOut {
        approved: s.approved,
        stale: s.stale,
        approved_sha: s.approved_sha,
        head_sha: s.head_sha,
        approved_ms: s.approved_ms,
        note: s.note,
        commits_since_signoff: s.commits_since,
        unresolved_count: s.unresolved_count,
        unaddressed_count: s.unaddressed_count,
        guidance,
    })
}

// ── finding rooms by path (issue #354) ──────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FindRoomsForPathArgs {
    pub path: String,
}

// Independent wire flags a caller reads separately, not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RoomPathMatch {
    pub room_id: String,
    pub name: String,
    pub cwd: String,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub archived: bool,
    /// Always exactly `archived`'s value, spelled out as its own field
    /// because "is this folder free to touch" is the question a caller
    /// actually has — an open room is never safe, whatever else is true
    /// about it, and a caller told only `archived` would have to
    /// reconstruct that itself.
    pub safe_to_remove: bool,
    /// The room is retired (#417): archived history that open-from-outside
    /// never matches. Reported only so a sweep can see it; always
    /// `safe_to_remove`.
    pub retired: bool,
    /// The room recorded a repository identity (#418) and its folder now
    /// holds a different repository, or none. Informational only: it never
    /// changes `safe_to_remove`.
    pub repo_mismatch: bool,
    /// `"cwd"` for an exact match, `"inside_room"` when the queried path
    /// sits strictly under the room's cwd, `"contains_room"` when the
    /// room's cwd sits strictly under the queried path — never a bare
    /// bool, so a caller isn't left reconstructing which folder is
    /// inside which.
    #[serde(rename = "match")]
    pub match_kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FindRoomsForPathOut {
    pub rooms: Vec<RoomPathMatch>,
    /// Rooms Skein holds but could not parse into a `Room` — a
    /// `sessions` row that failed to deserialize, or one already parked
    /// in `sessions_quarantine` (#167) — and so could not be checked
    /// against `path` at all. Non-zero means `rooms` may be missing an
    /// entry: a caller must not read that absence as permission to
    /// remove a folder.
    pub unreadable_rooms: u32,
}

/// A repository group's key for comparison: the root canonicalized
/// while it still exists, then normalized. `repoRoot` gets stored in two
/// spellings — `repo_root_for_path` returns a plain checkout's path
/// exactly as it was handed over, but a linked worktree's main checkout
/// the way libgit2 resolved it, with every symlink followed (macOS's own
/// `/var` → `/private/var` is the everyday case) — so comparing the
/// strings splits one repository into two groups. A root that no longer
/// exists keeps its stored spelling: it can't be the same folder as one
/// that does.
fn group_key_for_match(root: &str) -> String {
    let canonical = std::fs::canonicalize(root)
        .map_or_else(|_| root.to_owned(), |p| p.to_string_lossy().into_owned());
    normalize_path_for_match(&canonical)
}

/// Every room — open or archived — whose cwd equals, sits under, or
/// contains `path` (epic #266/#275, the director-room shape).
///
/// Deliberately **not** scoped to the caller's own room, unlike every
/// other verb in this file. The caller still has to authenticate the
/// same as always; only the *answer* crosses rooms. That is a
/// considered exception, not an oversight: the bearer token guards
/// against a leaked-log replay, not against the user's own other
/// agents, and a director room that spans several projects needs
/// exactly this question — "is anyone already working in this folder?"
/// — answered across the whole install, where no single room id could
/// scope it.
pub fn find_rooms_for_path(
    db: &Database,
    args: &FindRoomsForPathArgs,
) -> VerbResult<FindRoomsForPathOut> {
    let path = args.path.trim();
    if path.is_empty() {
        return Err(VerbError::Refused(
            "find_rooms_for_path needs a path".into(),
        ));
    }
    let query = normalize_path_for_match(path);
    let rooms = db.all_rooms().map_err(internal)?;
    let unreadable_rooms = db.unreadable_room_count().map_err(internal)?;

    let mut exact = Vec::new();
    let mut overlapping = Vec::new();
    for room in &rooms {
        let Some(cwd) = room.cwd.as_deref().filter(|c| !c.is_empty()) else {
            continue;
        };
        let Some(kind) = path_match_kind(&query, &normalize_path_for_match(cwd)) else {
            continue;
        };
        let retired = room.retired.is_some();
        let archived = room.archived.is_some();
        let entry = RoomPathMatch {
            room_id: room.id.clone(),
            name: room.name.clone(),
            cwd: cwd.to_owned(),
            repo_root: room.repo_root.clone(),
            branch: room.branch.clone(),
            archived,
            safe_to_remove: archived || retired,
            retired,
            repo_mismatch: check_identity(room.repo_identity.as_ref(), Path::new(cwd))
                == IdentityCheck::Mismatch,
            match_kind: kind.to_owned(),
        };
        if kind == "cwd" {
            exact.push(entry);
        } else {
            overlapping.push(entry);
        }
    }
    exact.extend(overlapping);
    Ok(FindRoomsForPathOut {
        rooms: exact,
        unreadable_rooms,
    })
}

// ── the mailbox (issue #327) ────────────────────────────────────────

/// Whether the named agent, run as `kind` in `cwd`, would see Skein's
/// review MCP tools at all — and, by extension, this mailbox's own
/// tools, since they ride the same connection. A plain function
/// pointer rather than a boxed closure: the real implementation
/// (`crate::agents::agent_sees_mcp`) needs no captures, so the mail
/// verbs pay no lifetime parameter for a seam that exists purely so
/// tests can drive both answers without agent files on disk.
pub type AgentSeesMcp = fn(kind: &str, agent: &str, cwd: &str) -> bool;

/// The parts of the user's spawn settings the mailbox verbs need to
/// decide anything, passed in rather than read from live state (#327).
/// A plain `Copy` value keeps `send_message`/`read_messages` testable
/// with no settings file and no Tauri runtime; the MCP and HTTP call
/// sites build one from the live `SpawnSettings` on every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailPolicy {
    pub messaging_enabled: bool,
    pub claude_injected: bool,
    pub opencode_injected: bool,
}

impl MailPolicy {
    /// Every reachable kind, messaging on — a fresh install's defaults,
    /// and what a test reaches for when the case under test isn't about
    /// the policy at all. Production builds one from the live
    /// `SpawnSettings` instead (`http::mail_context`), so this is
    /// test-only.
    #[cfg(test)]
    pub fn permissive() -> Self {
        Self {
            messaging_enabled: true,
            claude_injected: true,
            opencode_injected: true,
        }
    }
}

/// [`MailPolicy`] plus the disk-lookup seam, bundled for the MCP and
/// HTTP layers that carry both from one request to `call_tool`.
///
/// `app` (#329) is the emitter for the `message_in`/`message_out`
/// `harness_actions` rows `send_message` writes — `None` in a unit
/// test (no Tauri runtime) or when the agent API's own `AgentApiState`
/// has none (`for_test`), in which case the rows still get written,
/// only the live broadcast is skipped, same as every other emit in
/// this codebase. No `Copy`: an `AppHandle` is a handle, not a value.
#[derive(Clone)]
pub struct MailContext {
    pub policy: MailPolicy,
    pub agent_sees_mcp: AgentSeesMcp,
    pub app: Option<tauri::AppHandle>,
}

impl MailContext {
    /// The same defaults as [`MailPolicy::permissive`], for tests that
    /// have no opinion on messaging at all.
    #[cfg(test)]
    pub fn permissive() -> Self {
        Self {
            policy: MailPolicy::permissive(),
            agent_sees_mcp: |_, _, _| true,
            app: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SendMessageArgs {
    /// A harness id (searched across every room) or a room id, resolved
    /// at send time to that room's lead harness.
    pub to: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageOut {
    pub message_id: String,
    pub to_room_id: String,
    pub to_harness_id: String,
    pub to_room_name: String,
    pub to_harness_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReadMessagesArgs {
    /// Return the whole history, oldest first, instead of only what is
    /// unread. Still marks anything unread as read.
    #[serde(default)]
    pub include_read: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub from_room_id: String,
    /// `None` when the sending room no longer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_room_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_id: Option<String>,
    /// `None` when the sender had no `X-Skein-Harness`, or its room or
    /// that harness no longer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_name: Option<String>,
    pub body: String,
    pub created_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadMessagesOut {
    pub messages: Vec<AgentMessage>,
    /// How many of `messages` were unread before this call and just got
    /// marked — the signal the MCP/HTTP layer uses to decide whether to
    /// fire `skein://mail-changed`, without re-deriving it from the
    /// list (a message already read before `include_read` asked for
    /// history must not count again).
    pub newly_marked_read: usize,
}

/// `message_history`'s (#364) paging cursor: a JSON number is a
/// millisecond `created_ms` (exclusive), a JSON string is a message id
/// (strictly after that message in `(created_ms, rowid)` order — see
/// [`Database::harness_message_history`]). Never ambiguous: message ids
/// are UUIDs, never numeric, so `untagged` always picks the right arm.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum HistorySince {
    Ms(i64),
    MessageId(String),
}

/// `message_history`'s (#364) `direction` argument. Defaults to `In` —
/// the same scope `read_messages` has always had — so a caller that
/// never heard of this argument still gets the inbox it expects.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDirection {
    #[default]
    In,
    Out,
    Both,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MessageHistoryArgs {
    /// A room id or harness id — only messages exchanged with that
    /// counterpart. Optional — default: no filter.
    #[serde(default)]
    pub with: Option<String>,
    #[serde(default)]
    pub since: Option<HistorySince>,
    /// Optional — default and cap: [`DEFAULT_HISTORY_LIMIT`] /
    /// [`MAX_HISTORY_LIMIT`].
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub direction: Option<HistoryDirection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub id: String,
    /// `"inbound"` or `"outbound"` — never both, even for the one row
    /// shape that could read as either (a sibling harness in the
    /// caller's own room mailing the caller): see
    /// [`Database::harness_message_history`] for how that is resolved.
    pub direction: &'static str,
    pub from_room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_room_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_harness_name: Option<String>,
    pub to_room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_room_name: Option<String>,
    pub to_harness_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_harness_name: Option<String>,
    pub body: String,
    pub created_ms: i64,
    /// When the recipient read this message — for an outbound row, the
    /// signal that the counterpart has seen it; for an inbound row,
    /// whatever an earlier `read_messages` call already left behind.
    /// This call never sets it: `message_history` never marks anything
    /// read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageHistoryOut {
    pub messages: Vec<HistoryMessage>,
    /// Whether more rows exist after the last one returned here — the
    /// signal for forward paging with `since` set to that row's id.
    pub has_more: bool,
}

/// Send a message to another harness's mailbox.
///
/// `X-Skein-Harness` is attribution here exactly as it is everywhere
/// else in this API (#213): addressing a harness inside a room is
/// routing, not a security boundary, so any harness in any room may
/// send to any other. The lead harness is resolved *now*, not read
/// later, so the stored row names a concrete recipient rather than a
/// room whose lead harness might change before anyone reads it.
///
/// On success, also records one `message_in` `harness_actions` row for
/// the recipient and one `message_out` row for the sender (#329) — the
/// Live Context feed's only way to show who talked to whom, since a
/// mailbox write touches nothing a harness's own transcript tail would
/// ever see. Neither carries the message body. This is the one place
/// both `mcp.rs` and `http.rs` route a send through, so it is the only
/// place that needs to write them. A failure to record either row is
/// logged and swallowed — the send itself already succeeded.
pub fn send_message(
    db: &Database,
    caller: &Caller,
    args: &SendMessageArgs,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
    app: Option<&tauri::AppHandle>,
) -> VerbResult<SendMessageOut> {
    if !policy.messaging_enabled {
        return Err(VerbError::Refused(
            "agent messaging is turned off in Settings".into(),
        ));
    }
    if args.body.is_empty() {
        return Err(VerbError::Refused("a message needs a body".into()));
    }
    if args.body.len() > MAX_MESSAGE_BYTES {
        return Err(VerbError::Refused(format!(
            "a message body is capped at {MAX_MESSAGE_BYTES} bytes; this one is {} bytes",
            args.body.len()
        )));
    }
    let to = args.to.trim();
    if to.is_empty() {
        return Err(VerbError::Refused("send_message needs a to".into()));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let (room, harness) = resolve_mail_target(&rooms, to, policy, agent_sees_mcp)?;

    // The rate check, the unread check and the insert below are three
    // separate lock acquisitions, so concurrent sends can push a count
    // slightly past its cap. Accepted: these are anti-runaway caps, not
    // a security boundary.
    let now = now_ms();
    let sent = db
        .harness_messages_sent_since(&caller.room_id, now - SEND_RATE_WINDOW_MS)
        .map_err(internal)?;
    if sent >= SEND_RATE_LIMIT {
        return Err(VerbError::Refused(format!(
            "rate limit: this room has sent {sent} messages in the last minute \
             (cap {SEND_RATE_LIMIT})"
        )));
    }
    let unread = db
        .unread_harness_message_count(&room.id, &harness.id)
        .map_err(internal)?;
    if unread >= MAX_UNREAD_MESSAGES {
        return Err(VerbError::Refused(format!(
            "{} already has {unread} unread messages (cap {MAX_UNREAD_MESSAGES}); \
             it needs to read before it can receive more",
            harness.name
        )));
    }

    let row = HarnessMessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        room_id: room.id.clone(),
        harness_id: harness.id.clone(),
        from_room_id: caller.room_id.clone(),
        from_harness_id: caller.harness_id.clone(),
        body: args.body.clone(),
        created_ms: now,
        read_ms: None,
    };
    db.insert_harness_message(&row).map_err(internal)?;

    record_mail_actions(db, app, &rooms, caller, &room, &harness, &row);

    Ok(SendMessageOut {
        message_id: row.id,
        to_room_id: room.id,
        to_harness_id: harness.id,
        to_room_name: room.name,
        to_harness_name: harness.name,
    })
}

/// Every unread message for the calling harness, oldest first — or, with
/// `include_read`, the whole history. Marks the unread ones read.
pub fn read_messages(
    db: &Database,
    caller: &Caller,
    args: &ReadMessagesArgs,
    policy: MailPolicy,
) -> VerbResult<ReadMessagesOut> {
    if !policy.messaging_enabled {
        return Err(VerbError::Refused(
            "agent messaging is turned off in Settings".into(),
        ));
    }
    let Some(harness_id) = caller.harness_id.as_deref() else {
        return Err(VerbError::Refused(
            "read_messages needs X-Skein-Harness to say which harness is asking".into(),
        ));
    };
    let include_read = args.include_read.unwrap_or(false);
    let rows = if include_read {
        db.all_harness_messages(&caller.room_id, harness_id)
    } else {
        db.unread_harness_messages(&caller.room_id, harness_id)
    }
    .map_err(internal)?;

    let now = now_ms();
    let mut to_mark = Vec::new();
    let rooms = db.all_rooms().map_err(internal)?;
    let messages: Vec<AgentMessage> = rows
        .into_iter()
        .map(|m| {
            let read_ms = if let Some(r) = m.read_ms {
                Some(r)
            } else {
                to_mark.push(m.id.clone());
                Some(now)
            };
            let sender_room = find_room(&rooms, &m.from_room_id);
            let from_room_name = sender_room.map(|r| r.name.clone());
            let from_harness_name = m.from_harness_id.as_deref().and_then(|hid| {
                sender_room
                    .and_then(|r| r.harnesses.iter().find(|h| h.id == hid))
                    .map(|h| format!("{} · {}", h.kind, h.name))
            });
            AgentMessage {
                id: m.id,
                from_room_id: m.from_room_id,
                from_room_name,
                from_harness_id: m.from_harness_id,
                from_harness_name,
                body: m.body,
                created_ms: m.created_ms,
                read_ms,
            }
        })
        .collect();

    if !to_mark.is_empty() {
        db.mark_harness_messages_read(&to_mark, now)
            .map_err(internal)?;
    }

    Ok(ReadMessagesOut {
        newly_marked_read: to_mark.len(),
        messages,
    })
}

/// Whether `m` sits in the caller's own inbox — the same scope
/// `read_messages` reads (`room_id`/`harness_id`, the recipient
/// columns). `false` with no harness on the caller, same as
/// `read_messages`'s outright refusal in that case, but this function
/// itself never refuses — [`message_history`] decides what to do with
/// that.
fn is_inbound(
    m: &HarnessMessageRow,
    caller_room_id: &str,
    caller_harness_id: Option<&str>,
) -> bool {
    caller_harness_id.is_some_and(|h| m.room_id == caller_room_id && m.harness_id == h)
}

/// Whether `m` sits in the caller's ROOM's outbox (`from_room_id`, not
/// `from_harness_id` — #364's brief is explicit that outbound scope is
/// per room, covering every harness in it, not only the caller's own).
/// Excludes anything [`is_inbound`] already claims, which is what keeps
/// a sibling harness's message to the caller classified inbound rather
/// than counted twice.
fn is_outbound(
    m: &HarnessMessageRow,
    caller_room_id: &str,
    caller_harness_id: Option<&str>,
) -> bool {
    m.from_room_id == caller_room_id && !is_inbound(m, caller_room_id, caller_harness_id)
}

/// [`HarnessMessageRow`] → [`HistoryMessage`], enriching both ends the
/// way [`read_messages`] already enriches the sender — `rooms` is one
/// `all_rooms` read shared across a whole page, not refetched per row.
fn render_history_message(
    m: &HarnessMessageRow,
    rooms: &[Room],
    caller_room_id: &str,
    caller_harness_id: Option<&str>,
) -> HistoryMessage {
    let direction = if is_inbound(m, caller_room_id, caller_harness_id) {
        "inbound"
    } else {
        "outbound"
    };

    let sender_room = find_room(rooms, &m.from_room_id);
    let from_room_name = sender_room.map(|r| r.name.clone());
    let from_harness_name = m.from_harness_id.as_deref().and_then(|hid| {
        sender_room
            .and_then(|r| r.harnesses.iter().find(|h| h.id == hid))
            .map(|h| format!("{} · {}", h.kind, h.name))
    });

    let recipient_room = find_room(rooms, &m.room_id);
    let to_room_name = recipient_room.map(|r| r.name.clone());
    let to_harness_name = recipient_room
        .and_then(|r| r.harnesses.iter().find(|h| h.id == m.harness_id))
        .map(|h| format!("{} · {}", h.kind, h.name));

    HistoryMessage {
        id: m.id.clone(),
        direction,
        from_room_id: m.from_room_id.clone(),
        from_room_name,
        from_harness_id: m.from_harness_id.clone(),
        from_harness_name,
        to_room_id: m.room_id.clone(),
        to_room_name,
        to_harness_id: m.harness_id.clone(),
        to_harness_name,
        body: m.body.clone(),
        created_ms: m.created_ms,
        read_ms: m.read_ms,
    }
}

/// A harness's (or, with `direction: "out"`/`"both"`, its whole room's)
/// mail history — filtered by counterpart, bounded by `since`/`limit`,
/// oldest first (#364). Unlike [`read_messages`], this **never** marks
/// anything read: it exists so a director recovering after its own
/// context is compacted can page back through exactly the thread it
/// needs without touching the unread badge that call maintains.
pub fn message_history(
    db: &Database,
    caller: &Caller,
    args: &MessageHistoryArgs,
    policy: MailPolicy,
) -> VerbResult<MessageHistoryOut> {
    if !policy.messaging_enabled {
        return Err(VerbError::Refused(
            "agent messaging is turned off in Settings".into(),
        ));
    }
    let direction = args.direction.unwrap_or_default();
    let include_inbound = matches!(direction, HistoryDirection::In | HistoryDirection::Both);
    let include_outbound = matches!(direction, HistoryDirection::Out | HistoryDirection::Both);

    if include_inbound && caller.harness_id.is_none() {
        return Err(VerbError::Refused(
            "message_history needs X-Skein-Harness to say which harness's inbox to \
             read — pass direction: \"out\" for the room's own outbox without one"
                .into(),
        ));
    }

    let limit = match args.limit {
        None | Some(0) => DEFAULT_HISTORY_LIMIT,
        Some(n) => n.min(MAX_HISTORY_LIMIT),
    };

    let (since_ms, since_message_id) = match &args.since {
        None => (None, None),
        Some(HistorySince::Ms(ms)) => (Some(*ms), None),
        Some(HistorySince::MessageId(id)) => {
            let row = db.harness_message_by_id(id).map_err(internal)?;
            let visible = row.is_some_and(|m| {
                is_inbound(&m, &caller.room_id, caller.harness_id.as_deref())
                    || is_outbound(&m, &caller.room_id, caller.harness_id.as_deref())
            });
            if !visible {
                return Err(VerbError::Refused(
                    "since does not name a message in this room's own mail history".into(),
                ));
            }
            (None, Some(id.as_str()))
        }
    };

    let rows = db
        .harness_message_history(
            &caller.room_id,
            caller.harness_id.as_deref(),
            include_inbound,
            include_outbound,
            args.with.as_deref(),
            since_ms,
            since_message_id,
            limit + 1,
        )
        .map_err(internal)?;

    let has_more = rows.len() > limit as usize;
    let rooms = db.all_rooms().map_err(internal)?;
    let messages = rows
        .into_iter()
        .take(limit as usize)
        .map(|m| render_history_message(&m, &rooms, &caller.room_id, caller.harness_id.as_deref()))
        .collect();

    Ok(MessageHistoryOut { messages, has_more })
}

/// A harness's unread-mail summary (#329) — how many, and which rooms
/// they're from, for a badge that has to answer both without reading
/// (`read_messages` marks read, which this must never do).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailUnread {
    pub count: i64,
    /// Distinct sender room names, first-seen order (oldest unread
    /// message first). A room that no longer exists falls back to its
    /// id rather than being dropped — the same "over-report rather than
    /// hide" call review baselines make (#221).
    pub from_room_names: Vec<String>,
}

/// Pure derivation over already-fetched rows, so it's testable with no
/// `Database` and reusable by both the Tauri command (`mail_unread`,
/// #329) and, if ever needed, a verb. Never marks anything read — the
/// caller must fetch `messages` with `unread_harness_messages`, not
/// `all_harness_messages`.
pub fn unread_mail(messages: &[HarnessMessageRow], rooms: &[Room]) -> MailUnread {
    let mut from_room_names = Vec::new();
    for m in messages {
        let name = find_room(rooms, &m.from_room_id)
            .map_or_else(|| m.from_room_id.clone(), |r| r.name.clone());
        if !from_room_names.contains(&name) {
            from_room_names.push(name);
        }
    }
    MailUnread {
        count: i64::try_from(messages.len()).unwrap_or(i64::MAX),
        from_room_names,
    }
}

// ── opening a room (issue #330) ──────────────────────────────────────

/// The harness kinds Skein knows how to spawn. Mirrors the TS
/// `HarnessKind` union in `app/src/data.tsx`'s `HARNESS_KINDS` — kept as
/// a plain list here rather than imported, since nothing on the Rust
/// side otherwise needs that registry; `create_room` only needs to
/// reject a typo loudly rather than pass it through to the frontend.
const KNOWN_HARNESS_KINDS: &[&str] = &["claude", "opencode", "copilot", "byoh", "files", "design"];

/// How long to wait for the frontend to resolve a `(kind, agent)` — a
/// folder lookup plus a couple of `localStorage` reads, so this should
/// never be close.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for the frontend to actually create the room —
/// generous because worktree mode runs `git worktree add`, which can be
/// slow on a large repo or a cold disk cache.
const CREATE_ROOM_TIMEOUT: Duration = Duration::from_secs(60);

/// How many `create_room` attempts (successful or refused past this
/// point) one calling room may make in [`ROOM_CREATION_WINDOW`] — a
/// runaway loop has to hit a wall before it can spawn an unbounded
/// number of agent processes.
const ROOM_CREATION_RATE_LIMIT: usize = 5;
const ROOM_CREATION_WINDOW: Duration = Duration::from_secs(60);

/// How many open (non-archived), agent-opened rooms — `created_by`
/// [`Some`] — Skein will hold in one repository group, or in the
/// ungrouped bucket, before `create_room` refuses outright (#375). A
/// hand-opened room, or a room from before #330 that has no
/// `createdBy` at all, never counts: this ceiling exists to stop a
/// runaway *agent* from fanning out geometrically, not to cap how many
/// rooms the user keeps open by hand. It is scoped per repository group
/// — the same key the room strip groups on (#76) — rather than global,
/// so one repository's runaway agent can't starve every other project
/// of room slots; a group is a room's normalized `repoRoot`, and rooms
/// with no `repoRoot` at all share one ungrouped bucket. Each open room
/// is a real worktree and, once its harness spawns, a real process.
const MAX_OPEN_ROOMS_PER_GROUP: usize = 20;

/// `deny_unknown_fields`: a wrong key here — `branch_mode` instead of
/// `branchMode`, say — must be a loud error, not a silently dropped
/// argument the caller has no way to notice (the exact bug the advertised
/// `inputSchema` in `mcp.rs`'s `tool_specs` had until it was caught: the
/// schema advertised the `snake_case` Rust field names, and a client that
/// followed it faithfully had its `branchMode`/`baseBranch` vanish).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRoomArgs {
    /// Absolute path to an existing folder. Omitted → the calling
    /// room's own repo root, falling back to its cwd.
    #[serde(default)]
    pub path: Option<String>,
    /// `"worktree"` (default) or `"current"` — the same two choices the
    /// New Room dialog offers.
    #[serde(default)]
    pub branch_mode: Option<String>,
    /// Worktree mode only. Omitted → the frontend's own branch template
    /// applied to `task`.
    #[serde(default)]
    pub branch: Option<String>,
    /// Worktree mode only. Omitted → the repo's HEAD.
    #[serde(default)]
    pub base_branch: Option<String>,
    pub task: String,
    /// Omitted → the frontend applies the user's own default for the
    /// folder. When given, must be one of [`KNOWN_HARNESS_KINDS`].
    #[serde(default)]
    pub kind: Option<String>,
    /// Omitted → the tool's own default, or the folder's remembered
    /// agent (#247/#248) — the same rule the New Room dialog follows.
    #[serde(default)]
    pub agent: Option<String>,
    /// Queue this as the new harness's first message (#327's mailbox),
    /// once the room exists. Refused up front, before anything is
    /// created, if the resolved harness could never read it.
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRoomOut {
    pub room_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub harness_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Present only when `prompt` was given and successfully queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Present only when the new worktree was branched from a LOCAL base
    /// branch that is behind its upstream (#367) — a lower bound, since
    /// Skein never fetches. A caller that sees this should pull or pass a
    /// remote-tracking ref (e.g. "origin/main") as `baseBranch` and retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_behind_upstream: Option<u32>,
}

/// The frontend's answer to a `"create_room.resolve"` request — the
/// `(kind, agent)` it would actually spawn for `path`, after applying
/// the user's default kind, the per-kind default agent (#248), the
/// folder's own remembered agent, and #247's agent validation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveOut {
    kind: String,
    #[serde(default)]
    agent: Option<String>,
}

/// The frontend's answer to a `"create_room"` request — the room it
/// actually made. `cwd`/`repo`/`branch`/`agent`/`session_id` are
/// nullable: a non-git folder has no repo or branch, and an opencode
/// harness's session id is only captured asynchronously after spawn
/// (see `useHarnessCreation.ts`), so it may still be unknown when this
/// answers.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedRoomOut {
    room_id: String,
    name: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    harness_id: String,
    kind: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    /// #367: forwarded from `createRoomArgs`'s outcome, only when the
    /// worktree's LOCAL base branch is behind its upstream.
    #[serde(default)]
    base_behind_upstream: Option<u32>,
}

/// Open a whole new room — a new worktree, a new spawned harness, and
/// optionally a queued first prompt (#330).
///
/// This is the one verb an agent can use to start *another* agent
/// working unattended, so it is guarded more heavily than anything else
/// in this file: a dedicated Settings kill switch
/// (`allow_agent_room_creation`), a per-room rate cap, and a per-
/// repository-group ceiling on how many agent-opened rooms Skein will
/// hold at once (see [`MAX_OPEN_ROOMS_PER_GROUP`]) — on top of the
/// frontend round trips themselves, which can refuse for reasons only
/// the UI knows (an unreadable folder, a colliding branch name, …).
///
/// Two round trips to the webview, via [`AgentApiState::request_frontend`]:
/// `"create_room.resolve"` first asks what `(kind, agent)` would
/// actually spawn for `path` — the same folder-memory and validation
/// logic `useNewRoomForm.tsx` runs — and only once that answer is in
/// hand does `"create_room"` ask for the room itself, so a prompt that
/// could never be delivered (an unreachable kind, injection turned off)
/// is caught *before* anything is created, never partially.
///
/// `room_creation_enabled` is `allow_agent_room_creation` off the live
/// `SpawnSettings`, read by the caller (`mcp.rs`/`http.rs`) the same way
/// `mail.policy` is — passed in rather than read from `state` here, so
/// this verb stays testable with a plain bool and no managed
/// `SpawnEnvState`, the one piece of Tauri-only state `AgentApiState::
/// for_test` has no way to fake.
#[allow(clippy::too_many_lines)]
pub async fn create_room(
    state: &AgentApiState,
    caller: &Caller,
    args: &CreateRoomArgs,
    mail: &MailContext,
    room_creation_enabled: bool,
) -> VerbResult<CreateRoomOut> {
    let db = &state.db;

    let task = args.task.trim();
    if task.is_empty() {
        return Err(VerbError::Refused("task can't be empty".into()));
    }

    let branch_mode = match args.branch_mode.as_deref() {
        None | Some("worktree") => "worktree",
        Some("current") => "current",
        Some(other) => {
            return Err(VerbError::Refused(format!(
                "unknown branchMode {other:?} — use worktree or current"
            )));
        }
    };

    if let Some(kind) = args.kind.as_deref() {
        if !KNOWN_HARNESS_KINDS.contains(&kind) {
            return Err(VerbError::Refused(format!(
                "unknown kind {kind:?} — use one of {KNOWN_HARNESS_KINDS:?}"
            )));
        }
    }

    if let Some(prompt) = args.prompt.as_deref() {
        if prompt.is_empty() {
            return Err(VerbError::Refused(
                "prompt was given but empty — omit it or give it a body".into(),
            ));
        }
        if prompt.len() > MAX_MESSAGE_BYTES {
            return Err(VerbError::Refused(format!(
                "prompt is capped at {MAX_MESSAGE_BYTES} bytes; this one is {} bytes",
                prompt.len()
            )));
        }
        if !mail.policy.messaging_enabled {
            return Err(VerbError::Refused(
                "a prompt was given, but agent messaging is turned off in Settings, \
                 so it could never be delivered"
                    .into(),
            ));
        }
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let caller_room = rooms
        .iter()
        .find(|r| r.id == caller.room_id)
        .cloned()
        .ok_or_else(|| VerbError::Unavailable("the calling room no longer exists".into()))?;

    let path = match args.path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => p.to_owned(),
        _ => caller_room
            .repo_root
            .clone()
            .or_else(|| caller_room.cwd.clone())
            .ok_or_else(|| {
                VerbError::Refused(
                    "path was omitted and the calling room has no folder to default to".into(),
                )
            })?,
    };
    let path_buf = std::path::Path::new(&path);
    if !path_buf.is_absolute() {
        return Err(VerbError::Refused(format!(
            "path must be absolute: {path:?}"
        )));
    }
    if !path_buf.is_dir() {
        return Err(VerbError::Refused(format!(
            "path does not exist or is not a directory: {path:?}"
        )));
    }
    if branch_mode == "worktree" && !skein_git::Repo::is_repo(path_buf) {
        return Err(VerbError::Refused(format!(
            "{path} is not a git checkout, so branchMode \"worktree\" cannot be used \
             (try \"current\")"
        )));
    }

    if !room_creation_enabled {
        return Err(VerbError::Refused(
            "agent room creation is turned off in Settings".into(),
        ));
    }
    if !state.check_room_creation_rate(
        &caller.room_id,
        ROOM_CREATION_WINDOW,
        ROOM_CREATION_RATE_LIMIT,
    ) {
        return Err(VerbError::Refused(format!(
            "rate limit: this room has attempted {ROOM_CREATION_RATE_LIMIT} room \
             creations in the last minute"
        )));
    }
    // The group the room being created would join — derived from the
    // resolved `path`, before anything else exists, the same way the
    // frontend derives `Room.repoRoot` (`worktreeRoom.ts`) — so the
    // scoping below can't drift from what the new room will actually be
    // grouped under (#375).
    let new_repo_root = repo_root_for_path(path_buf);
    let new_group_key = new_repo_root.as_deref().map(group_key_for_match);
    let scoped_open_count = rooms
        .iter()
        .filter(|r| {
            r.archived.is_none()
                && r.created_by.is_some()
                && r.repo_root.as_deref().map(group_key_for_match) == new_group_key
        })
        .count();
    if scoped_open_count >= MAX_OPEN_ROOMS_PER_GROUP {
        let msg = if let Some(root) = &new_repo_root {
            format!(
                "agents have already opened {scoped_open_count} open rooms in the \
                 repository group {root} (cap {MAX_OPEN_ROOMS_PER_GROUP} per group); \
                 close or archive some before opening another"
            )
        } else {
            format!(
                "agents have already opened {scoped_open_count} open rooms outside \
                 any repository group (cap {MAX_OPEN_ROOMS_PER_GROUP} for ungrouped \
                 rooms); close or archive some before opening another"
            )
        };
        return Err(VerbError::Refused(msg));
    }

    let resolved = state
        .request_frontend(
            "create_room.resolve",
            serde_json::json!({
                "path": path,
                "kind": args.kind,
                "agent": args.agent,
            }),
            RESOLVE_TIMEOUT,
        )
        .await
        .map_err(|e| frontend_error(&e))?;
    let resolved: ResolveOut = serde_json::from_value(resolved).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for create_room.resolve: {e}"
        ))
    })?;
    if !KNOWN_HARNESS_KINDS.contains(&resolved.kind.as_str()) {
        return Err(VerbError::Unavailable(format!(
            "the app resolved an unknown harness kind {:?}",
            resolved.kind
        )));
    }

    if args.prompt.is_some() {
        if let Some(reason) = mail_refusal_for(
            &resolved.kind,
            resolved.agent.as_deref(),
            Some(&path),
            mail.policy,
            mail.agent_sees_mcp,
        ) {
            return Err(VerbError::Refused(format!(
                "cannot queue the prompt: the new harness {reason}"
            )));
        }
    }

    let created = state
        .request_frontend(
            "create_room",
            serde_json::json!({
                "path": path,
                "branchMode": branch_mode,
                "branch": args.branch,
                "baseBranch": args.base_branch,
                "task": task,
                "kind": resolved.kind,
                "agent": resolved.agent,
                "createdBy": {
                    "roomId": caller.room_id,
                    // A `String`, never `null` — the frontend's
                    // `createdBy` contract only requires `roomId`, but
                    // `harnessId` still has to be *a string* to satisfy
                    // its type check when the caller's own
                    // `X-Skein-Harness` was absent, same fallback
                    // `queue_first_prompt`/`send_message` use elsewhere
                    // in this file.
                    "harnessId": caller.harness_id.clone().unwrap_or_default(),
                },
                "requesterRoomName": caller_room.name,
                // #356: a top-level field, not nested under `createdBy`
                // — `parseCreateArgs` (`app/src/agentRequests.ts`) reads
                // it there, alongside `prompt`. The frontend has no
                // other way to learn what this room was asked to do —
                // `task` above is only the tab's short label, and
                // `prompt` itself is never otherwise sent to the
                // webview (it is queued straight into `harness_messages`
                // by `queue_first_prompt`, below, once the room exists).
                // `None` when no `prompt` was given.
                "promptFirstLine": args
                    .prompt
                    .as_deref()
                    .map(first_non_empty_line)
                    .filter(|s| !s.is_empty()),
            }),
            CREATE_ROOM_TIMEOUT,
        )
        .await
        .map_err(|e| frontend_error(&e))?;
    let created: CreatedRoomOut = serde_json::from_value(created).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for create_room: {e}"
        ))
    })?;

    let message_id = args.prompt.as_deref().and_then(|prompt| {
        queue_first_prompt(
            db,
            state,
            caller,
            &caller_room,
            &created.room_id,
            &created.name,
            &created.harness_id,
            &created.kind,
            prompt,
        )
    });

    Ok(CreateRoomOut {
        room_id: created.room_id,
        name: created.name,
        cwd: created.cwd,
        repo: created.repo,
        branch: created.branch,
        harness_id: created.harness_id,
        kind: created.kind,
        agent: created.agent,
        session_id: created.session_id,
        message_id,
        base_behind_upstream: created.base_behind_upstream,
    })
}

/// Queue `prompt` as a harness's first mailbox message (#327), once the
/// frontend round trip that made (`create_room`) or opened
/// (`open_harness`, #411) it has actually answered. Returns the message
/// id, or `None` if the insert itself failed — a caller must never see
/// a `messageId` in the response for a message that does not exist in
/// `harness_messages`.
///
/// Takes the recipient's `(room_id, room_name, harness_id,
/// harness_label)` as plain fields rather than a `CreatedRoomOut` —
/// `open_harness` targets a room that already exists (and so is not
/// one), and `create_room`'s own call site passes its `created`
/// fields through unchanged. One implementation, one mailbox path, for
/// both verbs.
///
/// Deliberately does not go through [`send_message`] — this is not
/// counted against [`SEND_RATE_LIMIT`], since the room that just made
/// the request already paid its own rate cap, and the new harness's
/// unread count is trivially zero. Mirrors [`record_mail_actions`]'s
/// payload shape, built by hand rather than reusing that helper: it
/// takes real `Room`/`Harness` rows for the recipient, and a
/// freshly-`create_room`d room is not yet one — the frontend has not
/// autosaved it (see the module docs on why that is safe).
#[allow(clippy::too_many_arguments)]
fn queue_first_prompt(
    db: &Database,
    state: &AgentApiState,
    caller: &Caller,
    caller_room: &Room,
    to_room_id: &str,
    to_room_name: &str,
    to_harness_id: &str,
    to_harness_label: &str,
    prompt: &str,
) -> Option<String> {
    let now = now_ms();
    let row = HarnessMessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        room_id: to_room_id.to_owned(),
        harness_id: to_harness_id.to_owned(),
        from_room_id: caller.room_id.clone(),
        from_harness_id: caller.harness_id.clone(),
        body: prompt.to_owned(),
        created_ms: now,
        read_ms: None,
    };
    if let Err(e) = db.insert_harness_message(&row) {
        // The room itself was already created/opened — reporting this
        // as the whole call's failure would be worse than the prompt
        // simply not arriving, the same call `send_message` makes for
        // the harness_actions rows below. But the caller must not be
        // told a `messageId` for a row that was never written, so this
        // is the one path that returns `None` rather than
        // `Some(row.id)`.
        tracing::warn!(
            room_id = %to_room_id, harness_id = %to_harness_id, error = %e,
            "agent_api: first-prompt message failed to insert"
        );
        return None;
    }

    let from_harness_label = caller.harness_id.as_deref().and_then(|hid| {
        caller_room
            .harnesses
            .iter()
            .find(|h| h.id == hid)
            .map(|h| format!("{} · {}", h.kind, h.name))
    });
    let payload = serde_json::json!({
        "message_id": row.id,
        "from_room_id": caller.room_id,
        "from_room_name": caller_room.name,
        "from_harness_id": caller.harness_id,
        "from_harness_label": from_harness_label,
        "to_room_id": to_room_id,
        "to_room_name": to_room_name,
        "to_harness_id": to_harness_id,
        "to_harness_label": to_harness_label,
    })
    .to_string();
    record_and_emit(
        db,
        state.app.as_ref(),
        to_harness_id,
        to_room_id,
        now,
        crate::db::action_kind::MESSAGE_IN,
        &payload,
    );
    record_and_emit(
        db,
        state.app.as_ref(),
        caller.harness_id.as_deref().unwrap_or(""),
        &caller.room_id,
        now,
        crate::db::action_kind::MESSAGE_OUT,
        &payload,
    );
    state.notify_mail_changed(to_room_id, to_harness_id);
    Some(row.id)
}

/// Map a [`AgentApiState::request_frontend`] error string to the right
/// [`VerbError`]. Infrastructure problems — no webview, a timeout, a
/// disconnect mid-wait (see `PendingRequestGuard`'s doc for why that one
/// is possible at all) — are [`VerbError::Unavailable`]: not the
/// caller's fault, and plausibly transient. Anything else is the
/// frontend actively saying no to this exact request (an unresolvable
/// folder, a bad branch, …), which the caller could fix and retry, so
/// it is [`VerbError::Refused`].
fn frontend_error(message: &str) -> VerbError {
    if message.contains("no webview is listening")
        || message.contains("timed out after")
        || message.contains("dropped the request")
    {
        VerbError::Unavailable(message.to_owned())
    } else {
        VerbError::Refused(message.to_owned())
    }
}

// ── closing a room (issue #411) ───────────────────────────────────────

/// How many `close_room` attempts (successful or refused past this
/// point) one calling room may make in [`CLOSE_ROOM_WINDOW`] — the same
/// anti-runaway reasoning as `create_room`'s own
/// [`ROOM_CREATION_RATE_LIMIT`], on its own bucket
/// (`AgentApiState::check_verb_rate`'s `"close_room"`) so the two calls
/// never share a budget.
const CLOSE_ROOM_RATE_LIMIT: usize = 5;
const CLOSE_ROOM_WINDOW: Duration = Duration::from_secs(60);

/// How long to wait for the frontend to actually archive the room.
/// Shorter than `create_room`'s own [`CREATE_ROOM_TIMEOUT`] — archiving
/// touches no git worktree machinery, only a `filesRegistry` check and
/// the same in-memory archive the user's own close performs.
const CLOSE_ROOM_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CloseRoomArgs {
    pub room: String,
}

/// Attribution for a `close_room` or `close_harness` call (#411) —
/// echoed back to the caller. `close_room`'s frontend writes it onto
/// `Room.closedBy`; there is no equivalent `Harness.closedBy` field, so
/// `close_harness`'s copy exists only in the reply and in
/// [`log_close_harness_outcome`]'s `tracing::info!` line. `harness_id`
/// is `None` when the caller sent no `X-Skein-Harness`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentClosedBy {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseRoomOut {
    pub room_id: String,
    /// Archive timestamp (epoch ms), as the frontend recorded it —
    /// mirrors `Room.archived`.
    pub archived: i64,
    pub closed_by: AgentClosedBy,
}

/// The frontend's answer to a `"close_room"` request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClosedRoomOut {
    room_id: String,
    archived: i64,
}

/// One `tracing::info!` per call outcome (#176) — an archive triggered
/// through the agent API is exactly the shape of action a later
/// incident would need reconstructed from the log, not the database.
fn log_close_room_outcome(caller_room: &str, target_room: &str, outcome: &str) {
    tracing::info!(
        caller_room = %caller_room,
        target_room = %target_room,
        outcome = %outcome,
        "agent_api: close_room"
    );
}

/// Archive a room this room created, once it is signed off on its
/// current HEAD (#411) — the narrow amendment to D9 recorded in
/// `CLAUDE.md`: closing a room is archiving it, and archiving is
/// reversible (the worktree, the branch and the room record all stay;
/// Reopen restores it, #153), so it no longer needs the blanket refusal
/// `archive_room`/`remove_worktree`/`delete_room` still carry.
///
/// Guards run in order, cheapest first: the Settings kill switch, a
/// per-calling-room rate cap, then a chain of facts about the TARGET
/// room that must all hold before anything is asked of the frontend —
/// unknown, self, not the creator, archived, not signed off, a stale
/// sign-off. Only once every one of those passes does this ask the
/// webview to actually archive it, exactly like the user's own close
/// (no confirm dialog, respecting `filesRegistry`).
///
/// Every refusal message starts with a stable `snake_case` code (see
/// `docs/agent-api.md`), so a frontend refusal — `"unsaved_files: …"`,
/// forwarded verbatim by [`frontend_error`] — and a guard refused here
/// read the same way to the caller.
pub async fn close_room(
    state: &AgentApiState,
    caller: &Caller,
    args: &CloseRoomArgs,
    room_closing_enabled: bool,
) -> VerbResult<CloseRoomOut> {
    let db = &state.db;
    let room_id = args.room.trim();
    if room_id.is_empty() {
        return Err(VerbError::Refused("close_room needs a room id".into()));
    }

    if !room_closing_enabled {
        log_close_room_outcome(&caller.room_id, room_id, "disabled");
        return Err(VerbError::Refused(
            "disabled: agent room closing is turned off in Settings".into(),
        ));
    }
    if !state.check_verb_rate(
        "close_room",
        &caller.room_id,
        CLOSE_ROOM_WINDOW,
        CLOSE_ROOM_RATE_LIMIT,
    ) {
        log_close_room_outcome(&caller.room_id, room_id, "rate_limited");
        return Err(VerbError::Refused(format!(
            "rate_limited: this room has attempted {CLOSE_ROOM_RATE_LIMIT} close_room \
             calls in the last minute"
        )));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let Some(target) = rooms.iter().find(|r| r.id == room_id) else {
        log_close_room_outcome(&caller.room_id, room_id, "not_found");
        return Err(VerbError::NotFound(format!(
            "not_found: no such room: {room_id:?}"
        )));
    };

    if target.id == caller.room_id {
        log_close_room_outcome(&caller.room_id, room_id, "self");
        return Err(VerbError::Refused(
            "self: a room cannot close itself — close_room only closes a room \
             this one created"
                .into(),
        ));
    }
    let is_creator = target
        .created_by
        .as_ref()
        .is_some_and(|c| c.room_id == caller.room_id);
    if !is_creator {
        log_close_room_outcome(&caller.room_id, room_id, "not_creator");
        return Err(VerbError::Refused(format!(
            "not_creator: {} was not created by this room with create_room, so it \
             cannot close it",
            target.name
        )));
    }
    if target.archived.is_some() {
        log_close_room_outcome(&caller.room_id, room_id, "archived");
        return Err(VerbError::Refused(format!(
            "archived: {} is already archived",
            target.name
        )));
    }

    let signed_off = match target.cwd.as_deref() {
        None => None,
        Some(cwd) => Some(signoff_block(db, &target.id, cwd)?),
    };
    match signed_off {
        Some(s) if s.approved => {}
        Some(s) if s.stale => {
            log_close_room_outcome(&caller.room_id, room_id, "stale_signoff");
            return Err(VerbError::Refused(format!(
                "stale_signoff: {}'s sign-off is stale — the reviewer approved an \
                 earlier commit and HEAD has moved since. Ask them to look again \
                 before closing it.",
                target.name
            )));
        }
        _ => {
            log_close_room_outcome(&caller.room_id, room_id, "not_signed_off");
            return Err(VerbError::Refused(format!(
                "not_signed_off: {} is not signed off yet — the reviewer has not \
                 approved its current HEAD",
                target.name
            )));
        }
    }

    let closed_by = AgentClosedBy {
        room_id: caller.room_id.clone(),
        harness_id: caller.harness_id.clone(),
    };
    let reply = match state
        .request_frontend(
            "close_room",
            serde_json::json!({
                "roomId": target.id,
                "closedBy": {
                    "roomId": closed_by.room_id,
                    "harnessId": closed_by.harness_id,
                },
            }),
            CLOSE_ROOM_TIMEOUT,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let err = frontend_error(&e);
            log_close_room_outcome(&caller.room_id, room_id, err.message());
            return Err(err);
        }
    };
    let closed: ClosedRoomOut = serde_json::from_value(reply).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for close_room: {e}"
        ))
    })?;

    log_close_room_outcome(&caller.room_id, room_id, "ok");
    Ok(CloseRoomOut {
        room_id: closed.room_id,
        archived: closed.archived,
        closed_by,
    })
}

// ── opening a harness (issue #411) ─────────────────────────────────────

/// How many `open_harness`/`close_harness` attempts (successful or
/// refused past this point), combined, one calling room may make in
/// [`HARNESS_CONTROL_WINDOW`] — the two verbs share one bucket
/// (`AgentApiState::check_verb_rate`'s `"harness_control"`) because they
/// are the same amount of trust: either one changes what harnesses a
/// room has.
const HARNESS_CONTROL_RATE_LIMIT: usize = 10;
const HARNESS_CONTROL_WINDOW: Duration = Duration::from_secs(60);

/// How many harnesses one room may hold before `open_harness` refuses
/// outright (#411) — the same ceiling the room's own tab strip would
/// make impractical by hand, applied so a runaway agent can't fan a
/// single room out into an unbounded number of processes either.
const MAX_HARNESSES_PER_ROOM: usize = 8;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenHarnessArgs {
    pub room: String,
    /// Omitted → the frontend applies the user's own default for the
    /// folder, the same as `create_room`. When given, must be one of
    /// [`KNOWN_HARNESS_KINDS`].
    #[serde(default)]
    pub kind: Option<String>,
    /// Omitted → the tool's own default, or the folder's remembered
    /// agent (#247/#248).
    #[serde(default)]
    pub agent: Option<String>,
    /// Queue this as the new harness's first message (#327's mailbox),
    /// once it exists. Refused up front, before anything is spawned, if
    /// the resolved harness could never read it.
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenHarnessOut {
    pub room_id: String,
    pub harness_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub name: String,
    /// Present only when `prompt` was given and successfully queued —
    /// same convention as `CreateRoomOut::message_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// The frontend's answer to an `"open_harness"` request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenedHarnessOut {
    harness_id: String,
    kind: String,
    #[serde(default)]
    agent: Option<String>,
    name: String,
}

/// One `tracing::info!` per call outcome (#176), the same shape as
/// [`log_close_room_outcome`].
fn log_open_harness_outcome(caller_room: &str, target_room: &str, outcome: &str) {
    tracing::info!(
        caller_room = %caller_room,
        target_room = %target_room,
        outcome = %outcome,
        "agent_api: open_harness"
    );
}

/// Add a harness to this room, or to a room this room opened with
/// `create_room` (#411) — the way "+ harness" does, in the background:
/// it never takes focus and never switches the target room's own active
/// harness.
///
/// Guards run in order, cheapest first: the Settings kill switch, the
/// shared `harness_control` rate cap, then facts about the TARGET room
/// — unknown, out of scope, archived, already at the harness ceiling.
/// Only once every one of those passes does this round-trip to the
/// webview twice, the same two-step `create_room` uses and for the same
/// reason: `"open_harness.resolve"` learns what `(kind, agent)` would
/// actually spawn *before* `"open_harness"` is asked to add anything, so
/// a `prompt` that could never be delivered is caught first.
pub async fn open_harness(
    state: &AgentApiState,
    caller: &Caller,
    args: &OpenHarnessArgs,
    mail: &MailContext,
    harness_control_enabled: bool,
) -> VerbResult<OpenHarnessOut> {
    let db = &state.db;
    let room_id = args.room.trim();
    if room_id.is_empty() {
        return Err(VerbError::Refused("open_harness needs a room id".into()));
    }
    if let Some(kind) = args.kind.as_deref() {
        if !KNOWN_HARNESS_KINDS.contains(&kind) {
            return Err(VerbError::Refused(format!(
                "unknown kind {kind:?} — use one of {KNOWN_HARNESS_KINDS:?}"
            )));
        }
    }
    if let Some(prompt) = args.prompt.as_deref() {
        if prompt.is_empty() {
            return Err(VerbError::Refused(
                "prompt was given but empty — omit it or give it a body".into(),
            ));
        }
        if prompt.len() > MAX_MESSAGE_BYTES {
            return Err(VerbError::Refused(format!(
                "prompt is capped at {MAX_MESSAGE_BYTES} bytes; this one is {} bytes",
                prompt.len()
            )));
        }
        if !mail.policy.messaging_enabled {
            return Err(VerbError::Refused(
                "a prompt was given, but agent messaging is turned off in Settings, \
                 so it could never be delivered"
                    .into(),
            ));
        }
    }

    if !harness_control_enabled {
        log_open_harness_outcome(&caller.room_id, room_id, "disabled");
        return Err(VerbError::Refused(
            "disabled: agent harness control is turned off in Settings".into(),
        ));
    }
    if !state.check_verb_rate(
        "harness_control",
        &caller.room_id,
        HARNESS_CONTROL_WINDOW,
        HARNESS_CONTROL_RATE_LIMIT,
    ) {
        log_open_harness_outcome(&caller.room_id, room_id, "rate_limited");
        return Err(VerbError::Refused(format!(
            "rate_limited: this room has attempted {HARNESS_CONTROL_RATE_LIMIT} \
             open_harness/close_harness calls in the last minute"
        )));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let caller_room = rooms
        .iter()
        .find(|r| r.id == caller.room_id)
        .cloned()
        .ok_or_else(|| VerbError::Unavailable("the calling room no longer exists".into()))?;
    let Some(target) = rooms.iter().find(|r| r.id == room_id) else {
        log_open_harness_outcome(&caller.room_id, room_id, "not_found");
        return Err(VerbError::NotFound(format!(
            "not_found: no such room: {room_id:?}"
        )));
    };

    let in_scope = target.id == caller.room_id
        || target
            .created_by
            .as_ref()
            .is_some_and(|c| c.room_id == caller.room_id);
    if !in_scope {
        log_open_harness_outcome(&caller.room_id, room_id, "not_in_scope");
        return Err(VerbError::Refused(format!(
            "not_in_scope: {} is neither this room nor one it opened with \
             create_room, so it cannot open a harness there",
            target.name
        )));
    }
    if target.archived.is_some() {
        log_open_harness_outcome(&caller.room_id, room_id, "archived");
        return Err(VerbError::Refused(format!(
            "archived: {} is archived",
            target.name
        )));
    }
    if let Some(cwd) = target.cwd.as_deref() {
        if check_identity(target.repo_identity.as_ref(), Path::new(cwd)) == IdentityCheck::Mismatch
        {
            log_open_harness_outcome(&caller.room_id, room_id, "repo_mismatch");
            return Err(VerbError::Refused(format!(
                "repo_mismatch: the folder of {} now holds a different repository \
                 than the room was made for; the user must resolve the room's \
                 \"different repository\" card before a harness can open there",
                target.name
            )));
        }
    }
    if target.harnesses.len() >= MAX_HARNESSES_PER_ROOM {
        log_open_harness_outcome(&caller.room_id, room_id, "room_full");
        return Err(VerbError::Refused(format!(
            "room_full: {} already has {} harnesses (cap {MAX_HARNESSES_PER_ROOM}); \
             close one before opening another",
            target.name,
            target.harnesses.len()
        )));
    }

    let resolved = match state
        .request_frontend(
            "open_harness.resolve",
            serde_json::json!({
                "roomId": target.id,
                "kind": args.kind,
                "agent": args.agent,
                "prompt": args.prompt.is_some(),
            }),
            RESOLVE_TIMEOUT,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let err = frontend_error(&e);
            log_open_harness_outcome(&caller.room_id, room_id, err.message());
            return Err(err);
        }
    };
    let resolved: ResolveOut = serde_json::from_value(resolved).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for open_harness.resolve: {e}"
        ))
    })?;
    if !KNOWN_HARNESS_KINDS.contains(&resolved.kind.as_str()) {
        return Err(VerbError::Unavailable(format!(
            "the app resolved an unknown harness kind {:?}",
            resolved.kind
        )));
    }

    if args.prompt.is_some() {
        if let Some(reason) = mail_refusal_for(
            &resolved.kind,
            resolved.agent.as_deref(),
            target.cwd.as_deref(),
            mail.policy,
            mail.agent_sees_mcp,
        ) {
            log_open_harness_outcome(&caller.room_id, room_id, "prompt_unreachable");
            return Err(VerbError::Refused(format!(
                "cannot queue the prompt: the new harness {reason}"
            )));
        }
    }

    let opened = match state
        .request_frontend(
            "open_harness",
            serde_json::json!({
                "roomId": target.id,
                "kind": resolved.kind,
                "agent": resolved.agent,
                "createdBy": {
                    "roomId": caller.room_id,
                    // A `String`, never `null` — see `create_room`'s
                    // identical comment on its own `createdBy.harnessId`.
                    "harnessId": caller.harness_id.clone().unwrap_or_default(),
                },
            }),
            CREATE_ROOM_TIMEOUT,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let err = frontend_error(&e);
            log_open_harness_outcome(&caller.room_id, room_id, err.message());
            return Err(err);
        }
    };
    let opened: OpenedHarnessOut = serde_json::from_value(opened).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for open_harness: {e}"
        ))
    })?;

    let message_id = args.prompt.as_deref().and_then(|prompt| {
        let label = format!("{} · {}", opened.kind, opened.name);
        queue_first_prompt(
            db,
            state,
            caller,
            &caller_room,
            &target.id,
            &target.name,
            &opened.harness_id,
            &label,
            prompt,
        )
    });

    log_open_harness_outcome(&caller.room_id, room_id, "ok");
    Ok(OpenHarnessOut {
        room_id: target.id.clone(),
        harness_id: opened.harness_id,
        kind: opened.kind,
        agent: opened.agent,
        name: opened.name,
        message_id,
    })
}

// ── closing a harness (issue #411) ──────────────────────────────────

/// How long to wait for the frontend to actually remove the harness.
/// Same reasoning as [`CLOSE_ROOM_TIMEOUT`] — this touches no git
/// worktree machinery either, only a phase read, a `filesRegistry`
/// check and the same in-memory removal the user's own tab-close
/// performs.
const CLOSE_HARNESS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseHarnessArgs {
    pub harness: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseHarnessOut {
    pub room_id: String,
    pub harness_id: String,
    /// The phase the harness was in immediately before the close —
    /// closing mid-turn is allowed, so this is how the caller learns
    /// whether it interrupted anything.
    pub phase: String,
    pub closed_by: AgentClosedBy,
}

/// The frontend's answer to a `"close_harness"` request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClosedHarnessOut {
    harness_id: String,
    phase: String,
}

/// One `tracing::info!` per call outcome (#176), the same shape as
/// [`log_close_room_outcome`]. This is the only durable record of who
/// closed a harness — see [`close_harness`]'s doc comment for why no
/// database row backs it.
fn log_close_harness_outcome(caller_room: &str, target_harness: &str, outcome: &str) {
    tracing::info!(
        caller_room = %caller_room,
        target_harness = %target_harness,
        outcome = %outcome,
        "agent_api: close_harness"
    );
}

/// Stop a harness the way closing its tab does (#411): a harness in its
/// own room (never itself), or any harness in a room this room opened
/// with `create_room`. Closing mid-turn is allowed — the reply reports
/// the phase the harness was in when it went.
///
/// Guards run in order, cheapest first: the Settings kill switch, the
/// `harness_control` rate cap shared with [`open_harness`] (the two are
/// the same amount of trust), then facts learned by searching every
/// room for the target harness id — unknown, out of scope, then (only
/// inside the caller's OWN room, where "not itself" needs proving) an
/// unidentified caller or a self-close, then archived, then the room's
/// last harness (that's `close_room`'s job). Only once every one of
/// those passes does this ask the webview to actually remove it,
/// exactly like the user's own close (no confirm dialog, respecting
/// `filesRegistry` and an open permission dialog).
///
/// Unlike `close_room`, no database row records the attribution:
/// `harness_events` is filled exclusively from the frontend's own
/// `harnessActivity` transition stream (`db.rs`'s
/// `record_harness_event` doc comment — "the activity store is the
/// source of truth and will only emit real transitions"), and this
/// close does not go through that store. Writing a second, Rust-
/// originated row for the same close risks a duplicate or conflicting
/// entry in a log the eventual cross-harness activity feed reads, for
/// an event the reply already carries in full. [`log_close_harness_outcome`]
/// is the attribution trail instead — one `tracing::info!` naming
/// caller, target and outcome, satisfying #176 the same way every other
/// verb here does.
pub async fn close_harness(
    state: &AgentApiState,
    caller: &Caller,
    args: &CloseHarnessArgs,
    harness_control_enabled: bool,
) -> VerbResult<CloseHarnessOut> {
    let db = &state.db;
    let harness_id = args.harness.trim();
    if harness_id.is_empty() {
        return Err(VerbError::Refused(
            "close_harness needs a harness id".into(),
        ));
    }

    if !harness_control_enabled {
        log_close_harness_outcome(&caller.room_id, harness_id, "disabled");
        return Err(VerbError::Refused(
            "disabled: agent harness control is turned off in Settings".into(),
        ));
    }
    if !state.check_verb_rate(
        "harness_control",
        &caller.room_id,
        HARNESS_CONTROL_WINDOW,
        HARNESS_CONTROL_RATE_LIMIT,
    ) {
        log_close_harness_outcome(&caller.room_id, harness_id, "rate_limited");
        return Err(VerbError::Refused(format!(
            "rate_limited: this room has attempted {HARNESS_CONTROL_RATE_LIMIT} \
             open_harness/close_harness calls in the last minute"
        )));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let Some(target_room) = rooms
        .iter()
        .find(|r| r.harnesses.iter().any(|h| h.id == harness_id))
    else {
        log_close_harness_outcome(&caller.room_id, harness_id, "not_found");
        return Err(VerbError::NotFound(format!(
            "not_found: no such harness: {harness_id:?}"
        )));
    };

    let in_scope = target_room.id == caller.room_id
        || target_room
            .created_by
            .as_ref()
            .is_some_and(|c| c.room_id == caller.room_id);
    if !in_scope {
        log_close_harness_outcome(&caller.room_id, harness_id, "not_in_scope");
        return Err(VerbError::Refused(format!(
            "not_in_scope: {} is neither this room nor one it opened with \
             create_room, so it cannot close a harness there",
            target_room.name
        )));
    }

    if target_room.id == caller.room_id {
        let Some(caller_harness_id) = caller.harness_id.as_deref() else {
            log_close_harness_outcome(&caller.room_id, harness_id, "caller_unknown");
            return Err(VerbError::Refused(
                "caller_unknown: this call carried no X-Skein-Harness identity, so \
                 there is no way to tell it isn't closing itself"
                    .into(),
            ));
        };
        if caller_harness_id == harness_id {
            log_close_harness_outcome(&caller.room_id, harness_id, "self");
            return Err(VerbError::Refused(
                "self: a harness cannot close itself".into(),
            ));
        }
    }

    if target_room.archived.is_some() {
        log_close_harness_outcome(&caller.room_id, harness_id, "archived");
        return Err(VerbError::Refused(format!(
            "archived: {} is archived",
            target_room.name
        )));
    }
    if target_room.harnesses.len() <= 1 {
        log_close_harness_outcome(&caller.room_id, harness_id, "last_harness");
        return Err(VerbError::Refused(format!(
            "last_harness: {harness_id} is the only harness in {} — use close_room \
             instead",
            target_room.name
        )));
    }

    let closed_by = AgentClosedBy {
        room_id: caller.room_id.clone(),
        harness_id: caller.harness_id.clone(),
    };
    let reply = match state
        .request_frontend(
            "close_harness",
            serde_json::json!({
                "roomId": target_room.id,
                "harnessId": harness_id,
                "closedBy": {
                    "roomId": closed_by.room_id,
                    "harnessId": closed_by.harness_id,
                },
            }),
            CLOSE_HARNESS_TIMEOUT,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let err = frontend_error(&e);
            log_close_harness_outcome(&caller.room_id, harness_id, err.message());
            return Err(err);
        }
    };
    let closed: ClosedHarnessOut = serde_json::from_value(reply).map_err(|e| {
        VerbError::Unavailable(format!(
            "the app returned an unexpected answer for close_harness: {e}"
        ))
    })?;

    log_close_harness_outcome(&caller.room_id, harness_id, "ok");
    Ok(CloseHarnessOut {
        room_id: target_room.id.clone(),
        harness_id: closed.harness_id,
        phase: closed.phase,
        closed_by,
    })
}

/// Resolve `to` into a concrete `(room, harness)` — a harness id first,
/// searched across every room, then a room id resolved to its lead
/// harness. Takes the room list rather than a `Database` so
/// `send_message` can reuse the one `all_rooms` read for the sender's
/// own name too (#329).
fn resolve_mail_target(
    rooms: &[Room],
    to: &str,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
) -> VerbResult<(Room, Harness)> {
    for room in rooms {
        if let Some(h) = room.harnesses.iter().find(|h| h.id == to) {
            if room.archived.is_some() {
                return Err(VerbError::Refused(format!(
                    "{} is archived and cannot receive messages",
                    room.name
                )));
            }
            return match mail_refusal(h, policy, room.cwd.as_deref(), agent_sees_mcp) {
                None => Ok((room.clone(), h.clone())),
                Some(reason) => Err(VerbError::Refused(format!(
                    "{} cannot read messages: {reason}",
                    h.name
                ))),
            };
        }
    }

    if let Some(room) = rooms.iter().find(|r| r.id == to) {
        if room.archived.is_some() {
            return Err(VerbError::Refused(format!(
                "{} is archived and cannot receive messages",
                room.name
            )));
        }
        return match mail_lead_harness(room, policy, agent_sees_mcp) {
            Some(h) => Ok((room.clone(), h.clone())),
            None => Err(VerbError::Refused(format!(
                "{} has no harness that can read messages",
                room.name
            ))),
        };
    }

    Err(VerbError::NotFound(format!("no harness or room {to:?}")))
}

/// Why `h` cannot read mail right now, or `None` when it can.
fn mail_refusal(
    h: &Harness,
    policy: MailPolicy,
    cwd: Option<&str>,
    agent_sees_mcp: AgentSeesMcp,
) -> Option<String> {
    mail_refusal_for(&h.kind, h.agent.as_deref(), cwd, policy, agent_sees_mcp)
}

/// The core of [`mail_refusal`], over a bare `(kind, agent)` pair rather
/// than a `Harness` — [`create_room`] (#330) needs the identical rule
/// applied to a harness that does not exist yet: the prompt-messaging
/// check has to run *before* the room (and so the harness row) is
/// created, using the kind/agent the frontend resolved.
fn mail_refusal_for(
    kind: &str,
    agent: Option<&str>,
    cwd: Option<&str>,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
) -> Option<String> {
    let injected = match kind {
        "claude" => policy.claude_injected,
        "opencode" => policy.opencode_injected,
        _ => {
            return Some(format!(
                "{kind} harnesses have no MCP connection to receive messages on"
            ));
        }
    };
    if !injected {
        return Some(format!(
            "config injection for {kind} is turned off in Settings, so it cannot see \
             the messaging tool"
        ));
    }
    if let (Some(agent), Some(cwd)) = (agent, cwd) {
        if !agent_sees_mcp(kind, agent, cwd) {
            return Some(format!(
                "the agent {agent:?} this harness runs hides MCP tools behind its \
                 own tool allowlist"
            ));
        }
    }
    None
}

/// The first harness in room order that can read mail — "lead" in the
/// sense that resolving a room id has to name someone concrete.
fn mail_lead_harness(
    room: &Room,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
) -> Option<&Harness> {
    room.harnesses
        .iter()
        .find(|h| mail_refusal(h, policy, room.cwd.as_deref(), agent_sees_mcp).is_none())
}

/// The room named `room_id`, if it still exists. Shared by
/// `read_messages`'s per-message sender enrichment and `unread_mail`'s
/// room-name summary (#329) — both look up a mailbox row's `from_room_id`
/// the same way.
fn find_room<'a>(rooms: &'a [Room], room_id: &str) -> Option<&'a Room> {
    rooms.iter().find(|r| r.id == room_id)
}

/// Record the two `harness_actions` rows a successful `send_message`
/// leaves behind (#329): `message_in` for the recipient, `message_out`
/// for the sender, same timestamp, same payload — everything a feed row
/// needs to say who talked to whom, and nothing a mailbox reply needs
/// hidden (no body).
fn record_mail_actions(
    db: &Database,
    app: Option<&tauri::AppHandle>,
    rooms: &[Room],
    caller: &Caller,
    to_room: &Room,
    to_harness: &Harness,
    message: &HarnessMessageRow,
) {
    let from_room = find_room(rooms, &caller.room_id);
    let from_room_name = from_room.map_or_else(|| caller.room_id.clone(), |r| r.name.clone());
    let from_harness_label = caller.harness_id.as_deref().and_then(|hid| {
        from_room
            .and_then(|r| r.harnesses.iter().find(|h| h.id == hid))
            .map(|h| format!("{} · {}", h.kind, h.name))
    });
    let to_harness_label = format!("{} · {}", to_harness.kind, to_harness.name);

    let payload = serde_json::json!({
        "message_id": message.id,
        "from_room_id": caller.room_id,
        "from_room_name": from_room_name,
        "from_harness_id": caller.harness_id,
        "from_harness_label": from_harness_label,
        "to_room_id": to_room.id,
        "to_room_name": to_room.name,
        "to_harness_id": to_harness.id,
        "to_harness_label": to_harness_label,
    })
    .to_string();

    record_and_emit(
        db,
        app,
        &to_harness.id,
        &to_room.id,
        message.created_ms,
        crate::db::action_kind::MESSAGE_IN,
        &payload,
    );
    record_and_emit(
        db,
        app,
        caller.harness_id.as_deref().unwrap_or(""),
        &caller.room_id,
        message.created_ms,
        crate::db::action_kind::MESSAGE_OUT,
        &payload,
    );
}

/// Insert one `harness_actions` row and, when a Tauri runtime is
/// attached, broadcast it live the same way every other adapter does
/// (`harness_action_event::emit`). A write that fails is logged and
/// dropped — the mailbox write it is describing already succeeded, and
/// a missing feed row is recoverable, unlike a lost message.
fn record_and_emit(
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
    timestamp_ms: i64,
    kind: &str,
    payload: &str,
) {
    match db.record_harness_action(harness_id, room_id, timestamp_ms, kind, payload, None) {
        Ok(id) => {
            if let Some(app) = app {
                crate::harness_action_event::emit(
                    app,
                    id,
                    harness_id,
                    room_id,
                    timestamp_ms,
                    kind,
                    payload,
                    None,
                );
            }
        }
        Err(e) => {
            tracing::warn!(harness_id, room_id, kind, error = %e,
                "agent_api: failed to record a mailbox harness_actions row");
        }
    }
}

// ── shared plumbing ───────────────────────────────────────────────

/// A thread, but only if it belongs to the caller's room.
///
/// "Wrong room" and "no such thread" return the same error on purpose:
/// distinguishing them would turn a token into an oracle for other
/// rooms' thread ids.
fn thread_in_room(db: &Database, caller: &Caller, thread_id: &str) -> VerbResult<ReviewThreadRow> {
    match db.review_thread(thread_id).map_err(internal)? {
        Some(t) if t.room_id == caller.room_id => Ok(t),
        _ => Err(VerbError::NotFound(format!(
            "no comment thread {thread_id} in this room"
        ))),
    }
}

pub(super) fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Branch => "branch",
        Scope::Commit => "commit",
        Scope::Pending => "pending",
    }
}

/// The per-room reads a listing needs, done once.
///
/// The anchoring pass is the expensive part and the reason this exists:
/// every file that carries threads is re-anchored through one
/// `ScopeFiles` snapshot (#171 slice (f)) rather than once per thread —
/// and, since that slice, rather than once per file either: the whole
/// batch costs one scope diff, not one per commented file. Reading the
/// stored coordinates instead would be free and sometimes wrong, which
/// is the one thing D6 does not allow.
struct RoomCtx {
    comments: BTreeMap<String, Vec<ReviewCommentRow>>,
    addressed: BTreeMap<String, ReviewAddressedRow>,
    /// harness id → "claude · main", for bylines.
    labels: BTreeMap<String, String>,
    /// thread id → (start, end, outdated) as of right now.
    placed: BTreeMap<String, (Option<usize>, Option<usize>, bool)>,
    /// Element threads' anchor and state (#434).
    elements: BTreeMap<String, ElementDto>,
    /// Element threads' guessed source, by thread id (#435).
    sources: BTreeMap<String, SourceGuess>,
}

impl RoomCtx {
    fn load(db: &Database, caller: &Caller, threads: &[ReviewThreadRow]) -> VerbResult<Self> {
        let mut comments: BTreeMap<String, Vec<ReviewCommentRow>> = BTreeMap::new();
        for c in db
            .review_comments_for_room(&caller.room_id)
            .map_err(internal)?
        {
            comments.entry(c.thread_id.clone()).or_default().push(c);
        }
        let addressed = db
            .addressed_for_room(&caller.room_id)
            .map_err(internal)?
            .into_iter()
            .map(|a| (a.thread_id.clone(), a))
            .collect();
        let labels = db
            .room_by_id(&caller.room_id)
            .map_err(internal)?
            .map(|r| {
                r.harnesses
                    .into_iter()
                    .map(|h| (h.id, format!("{} · {}", h.kind, h.name)))
                    .collect()
            })
            .unwrap_or_default();

        let mut placed = BTreeMap::new();
        if let Some(cwd) = caller.cwd.as_deref() {
            let paths: Vec<String> = files_of(threads).into_iter().collect();
            // A snapshot that cannot be built at all (not a repo, and so
            // on) is not a failure of the listing — every thread just
            // falls back to its stored coordinates below, the same as
            // when each file's own `file_impl` would have failed.
            if let Ok(snapshot) =
                ScopeFiles::load(db, &caller.room_id, cwd, Scope::Branch, None, &paths)
            {
                for path in &paths {
                    // A single file we cannot diff (deleted, unreadable)
                    // must not sink the rest of the batch — its threads
                    // fall back the same way.
                    let Ok(detail) = snapshot.file(db, path) else {
                        continue;
                    };
                    for t in detail.threads {
                        placed.insert(t.id, (t.line_start, t.line_end, t.outdated));
                    }
                }
            }
        }

        let elements = element::load(db, caller, threads)?;
        let sources = element::locate_all(caller, threads, &elements);

        Ok(Self {
            comments,
            addressed,
            labels,
            placed,
            elements,
            sources,
        })
    }

    fn to_agent_thread(&self, t: &ReviewThreadRow) -> AgentThread {
        let view = element::view(&self.elements, &self.sources, t);
        let (line_start, line_end, outdated) = match &view {
            Some(v) => (None, None, v.outdated),
            None => self.placed.get(&t.id).copied().unwrap_or_else(|| {
                (
                    t.line_start.and_then(|n| usize::try_from(n).ok()),
                    t.line_end.and_then(|n| usize::try_from(n).ok()),
                    false,
                )
            }),
        };
        AgentThread {
            thread_id: t.id.clone(),
            scope: t.scope.clone(),
            file: t.file_path.clone(),
            placement: view.as_ref().map(|v| v.placement),
            source: view.as_ref().and_then(|v| v.source.clone()),
            element: view.and_then(|v| v.json),
            commit_sha: t.commit_sha.clone(),
            line_start,
            line_end,
            outdated,
            resolved: t.resolved_ms.is_some(),
            addressed: self.addressed.get(&t.id).map(|a| AgentAddressed {
                commit_sha: a.commit_sha.clone(),
                by: self
                    .labels
                    .get(&a.harness_id)
                    .cloned()
                    .unwrap_or_else(|| "agent".to_owned()),
                note: a.note.clone(),
                addressed_ms: a.addressed_ms,
            }),
            comments: self
                .comments
                .get(&t.id)
                .map(|cs| {
                    cs.iter()
                        .map(|c| AgentComment {
                            author: self.author_of(c),
                            body: c.body.clone(),
                            created_ms: c.created_ms,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// "reviewer" for the human, the harness label for an agent. The
    /// agent reading this needs to know which lines are its own.
    fn author_of(&self, c: &ReviewCommentRow) -> String {
        if c.author_kind != "agent" {
            return "reviewer".to_owned();
        }
        c.author_id
            .as_ref()
            .and_then(|id| self.labels.get(id).cloned())
            .unwrap_or_else(|| "agent".to_owned())
    }
}

/// The distinct files a set of threads touches, normalized.
fn files_of(threads: &[ReviewThreadRow]) -> BTreeSet<String> {
    threads
        .iter()
        .filter(|t| !element::is_element(t))
        .filter_map(|t| t.file_path.as_deref().map(normalize))
        .collect()
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn parse_anchor(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

// ── cross-room room and harness listing (issue #356) ────────────────
//
// Three read-only verbs, all deliberately **not** scoped to the
// caller's own room — the same considered exception `find_rooms_for_path`
// (#354) documents: a director that opened several rooms with
// `create_room` needs to see them, and no single room id could scope
// that question. `list_rooms` alone takes a room-scoped filter
// (`created_by: "me"`), applied against the *caller's* room rather than
// widening what a token can see.

/// Cap on how much of a message or prompt becomes a one-line title —
/// enough to read as a summary, short enough that a title never turns
/// into a second message body.
const FIRST_LINE_CAP: usize = 200;

/// The first non-empty line of `s`, trimmed and capped at
/// [`FIRST_LINE_CAP`] **characters** (never bytes, so a cut never lands
/// mid multi-byte character). `""` when `s` has no non-empty line —
/// callers that mean "absent" rather than "blank" filter that out
/// themselves, the same way `create_room`'s `promptFirstLine` does.
fn first_non_empty_line(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > FIRST_LINE_CAP {
        line.chars().take(FIRST_LINE_CAP).collect()
    } else {
        line.to_owned()
    }
}

/// Harness activity phases the frontend's `ActivityPhase` union can
/// report (`app/src/harnessActivity.ts`). Anything else a webview
/// answer names is untrusted and becomes `"unknown"` rather than passed
/// through — this backend has no independent way to check it.
const KNOWN_PHASES: &[&str] = &[
    "spawning",
    "running",
    "idle",
    "waiting",
    "permission",
    "exited",
];

/// How long [`list_rooms`], [`get_room`] and [`list_harnesses`] wait
/// for the webview to answer the `"harness_phases"` round trip before
/// giving up and reporting every requested harness `"unknown"`. Short,
/// deliberately: unlike `create_room`'s minute-long round trips, a
/// read-only listing call must never hang on a busy or absent webview
/// — a phase this backend cannot vouch for is exactly what `"unknown"`
/// is for.
const HARNESS_PHASE_TIMEOUT: Duration = Duration::from_secs(3);

/// Ask the webview what phase each of `harness_ids` is in right now
/// (#328's request/answer round trip), and fall back to `"unknown"` for
/// every one of them on any failure — no webview, a timeout, an answer
/// that doesn't parse, a harness id the answer never named, or a value
/// outside [`KNOWN_PHASES`]. Never `"idle"` as a default: that is a
/// real phase, and guessing it would be a lie the frontend never told.
///
/// At most one round trip per call, and none at all when `harness_ids`
/// is empty — every caller already filters out archived-room harnesses
/// (no PTY is mounted for one, so there is nothing to ask) before
/// building that list.
async fn harness_phases(state: &AgentApiState, harness_ids: &[String]) -> BTreeMap<String, String> {
    let mut phases: BTreeMap<String, String> = harness_ids
        .iter()
        .map(|id| (id.clone(), "unknown".to_owned()))
        .collect();
    if harness_ids.is_empty() {
        return phases;
    }
    let Ok(answer) = state
        .request_frontend(
            "harness_phases",
            serde_json::json!({}),
            HARNESS_PHASE_TIMEOUT,
        )
        .await
    else {
        return phases;
    };
    let Some(reported) = answer.get("phases").and_then(serde_json::Value::as_object) else {
        return phases;
    };
    for (id, phase) in &mut phases {
        if let Some(p) = reported.get(id).and_then(serde_json::Value::as_str) {
            if KNOWN_PHASES.contains(&p) {
                p.clone_into(phase);
            }
        }
    }
    phases
}

/// The room named `room_id`, refusing the way every #356 single-room
/// verb needs to: an id nobody recognises names itself in the refusal
/// (this trio is cross-room by design, so there is no "wrong room"
/// ambiguity to protect the way thread lookups do), and one that
/// resolves to an archived room says so explicitly — never a bare
/// `None` two very different failures could hide behind.
fn require_open_room<'a>(rooms: &'a [Room], room_id: &str) -> VerbResult<&'a Room> {
    let room = rooms
        .iter()
        .find(|r| r.id == room_id)
        .ok_or_else(|| VerbError::NotFound(format!("no such room: {room_id:?}")))?;
    if room.archived.is_some() {
        return Err(VerbError::Refused(format!(
            "room {room_id:?} is archived; its harnesses are no longer live"
        )));
    }
    Ok(room)
}

/// `CreatedBy`, as an agent reads it — never the extra fields
/// `promptFirstLine`/`baseSha` carry on the wire the room is stored
/// with, which are surfaced as their own top-level fields instead so a
/// caller reading only `created_by` still gets attribution and nothing
/// more.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AgentCreatedBy {
    pub room_id: String,
    pub harness_id: String,
}

impl From<&crate::db::CreatedBy> for AgentCreatedBy {
    fn from(c: &crate::db::CreatedBy) -> Self {
        Self {
            room_id: c.room_id.clone(),
            harness_id: c.harness_id.clone(),
        }
    }
}

/// The latest message a room sent the caller, as [`list_rooms`] reports
/// it — enough for a director to rebuild its table without replaying
/// any mail.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LastStatus {
    pub first_line: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListRoomsArgs {
    /// The only accepted value is `"me"` — every other non-`None` value
    /// is refused rather than silently ignored, since a typo here
    /// (`"mine"`, `"self"`) would otherwise read as "show me
    /// everything" and a director would never notice its filter never
    /// applied.
    #[serde(default)]
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RoomSummary {
    pub room_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub archived: bool,
    pub harness_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<AgentCreatedBy>,
    /// The harness `send_message` would actually reach if addressed to
    /// this room id — `None` when nothing in the room could read mail
    /// at all (see `mail_lead_harness`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_harness_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_first_line: Option<String>,
    /// `"archived"`, one of [`KNOWN_PHASES`] (the lead harness's own),
    /// or `"unknown"` — never a guess.
    pub lifecycle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<LastStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ListRoomsOut {
    pub rooms: Vec<RoomSummary>,
    /// See [`FindRoomsForPathOut::unreadable_rooms`] — the same
    /// "rooms Skein holds but could not parse" count, so a director
    /// reading a short list here knows whether it might be short for
    /// that reason rather than because it truly created no more.
    pub unreadable_rooms: u32,
}

/// Every room Skein holds, across every project — archived rooms
/// included, and never dropped: "that room landed and was archived" is
/// exactly what a director needs to see after its own context is
/// compacted (issue #356's addendum). `created_by: "me"` narrows that
/// to only the rooms the *caller's* room created.
pub async fn list_rooms(
    state: &AgentApiState,
    caller: &Caller,
    args: &ListRoomsArgs,
    mail: &MailContext,
) -> VerbResult<ListRoomsOut> {
    let db = &state.db;
    if let Some(cb) = args.created_by.as_deref() {
        if cb != "me" {
            return Err(VerbError::Refused(format!(
                "unknown created_by {cb:?} — the only supported value is \"me\""
            )));
        }
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let unreadable_rooms = db.unreadable_room_count().map_err(internal)?;

    let filtered: Vec<&Room> = if args.created_by.is_some() {
        rooms
            .iter()
            .filter(|r| {
                r.created_by
                    .as_ref()
                    .is_some_and(|c| c.room_id == caller.room_id)
            })
            .collect()
    } else {
        rooms.iter().collect()
    };

    let needed: Vec<String> = filtered
        .iter()
        .filter(|r| r.archived.is_none())
        .filter_map(|r| {
            mail_lead_harness(r, mail.policy, mail.agent_sees_mcp).map(|h| h.id.clone())
        })
        .collect();
    let phases = harness_phases(state, &needed).await;

    let mut out = Vec::with_capacity(filtered.len());
    for r in filtered {
        let lead = mail_lead_harness(r, mail.policy, mail.agent_sees_mcp);
        let lifecycle = if r.archived.is_some() {
            "archived".to_owned()
        } else {
            lead.map_or_else(
                || "unknown".to_owned(),
                |h| {
                    phases
                        .get(&h.id)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_owned())
                },
            )
        };
        let last_status = db
            .latest_message_from_room(&r.id, &caller.room_id)
            .map_err(internal)?
            .map(|m| LastStatus {
                first_line: first_non_empty_line(&m.body),
                created_ms: m.created_ms,
            });
        out.push(RoomSummary {
            room_id: r.id.clone(),
            name: r.name.clone(),
            cwd: r.cwd.clone(),
            repo_root: r.repo_root.clone(),
            branch: r.branch.clone(),
            archived: r.archived.is_some(),
            harness_count: r.harnesses.len(),
            created_by: r.created_by.as_ref().map(AgentCreatedBy::from),
            lead_harness_id: lead.map(|h| h.id.clone()),
            base_sha: r.created_by.as_ref().and_then(|c| c.base_sha.clone()),
            prompt_first_line: r
                .created_by
                .as_ref()
                .and_then(|c| c.prompt_first_line.clone()),
            lifecycle,
            last_status,
        });
    }

    Ok(ListRoomsOut {
        rooms: out,
        unreadable_rooms,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GetRoomArgs {
    pub room: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HarnessSummary {
    pub harness_id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct GetRoomOut {
    pub room_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<AgentCreatedBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_first_line: Option<String>,
    pub harnesses: Vec<HarnessSummary>,
    /// `None` exactly when `signoff_unavailable` is `Some` — the room
    /// has no worktree, so there is nothing `status_impl` could read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signoff: Option<StatusOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signoff_unavailable: Option<String>,
}

/// One room, by id — archived or unknown ids are refused loudly (never
/// a bare `None`) since #356's whole point is a director inspecting a
/// room it does not own. Not scoped to the caller's own room, the same
/// considered exception as [`find_rooms_for_path`] and [`list_rooms`].
pub async fn get_room(state: &AgentApiState, args: &GetRoomArgs) -> VerbResult<GetRoomOut> {
    let db = &state.db;
    let room_id = args.room.trim();
    if room_id.is_empty() {
        return Err(VerbError::Refused("get_room needs a room id".into()));
    }
    let rooms = db.all_rooms().map_err(internal)?;
    let room = require_open_room(&rooms, room_id)?;

    let harness_ids: Vec<String> = room.harnesses.iter().map(|h| h.id.clone()).collect();
    let phases = harness_phases(state, &harness_ids).await;
    let harnesses = room
        .harnesses
        .iter()
        .map(|h| HarnessSummary {
            harness_id: h.id.clone(),
            name: h.name.clone(),
            kind: h.kind.clone(),
            agent: h.agent.clone(),
            session_id: h.session_id.clone(),
            phase: phases
                .get(&h.id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
        })
        .collect();

    let (signoff, signoff_unavailable) = match room.cwd.as_deref() {
        Some(cwd) => (Some(signoff_block(db, &room.id, cwd)?), None),
        None => (
            None,
            Some("this room has no worktree, so there is nothing to sign off on".to_owned()),
        ),
    };

    Ok(GetRoomOut {
        room_id: room.id.clone(),
        name: room.name.clone(),
        cwd: room.cwd.clone(),
        repo_root: room.repo_root.clone(),
        branch: room.branch.clone(),
        created_by: room.created_by.as_ref().map(AgentCreatedBy::from),
        base_sha: room.created_by.as_ref().and_then(|c| c.base_sha.clone()),
        prompt_first_line: room
            .created_by
            .as_ref()
            .and_then(|c| c.prompt_first_line.clone()),
        harnesses,
        signoff,
        signoff_unavailable,
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListHarnessesArgs {
    /// Omitted — every harness in every open room. Given, and blank
    /// after trimming, is treated the same as omitted.
    #[serde(default)]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HarnessListing {
    pub room_id: String,
    pub room_name: String,
    pub harness_id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ListHarnessesOut {
    pub harnesses: Vec<HarnessListing>,
}

/// Every harness in every open room — archived rooms are skipped
/// entirely, the same as an absent `room` filter skips them, since none
/// of their harnesses have a live PTY to ask a phase of. `room` narrows
/// to one room, with the same unknown/archived refusals as
/// [`get_room`]. Not scoped to the caller's own room, the same
/// considered exception as [`find_rooms_for_path`], [`list_rooms`] and
/// [`get_room`].
pub async fn list_harnesses(
    state: &AgentApiState,
    args: &ListHarnessesArgs,
) -> VerbResult<ListHarnessesOut> {
    let db = &state.db;
    let rooms = db.all_rooms().map_err(internal)?;

    let selected: Vec<&Room> = match args
        .room
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(room_id) => vec![require_open_room(&rooms, room_id)?],
        None => rooms.iter().filter(|r| r.archived.is_none()).collect(),
    };

    let all_harness_ids: Vec<String> = selected
        .iter()
        .flat_map(|r| r.harnesses.iter().map(|h| h.id.clone()))
        .collect();
    let phases = harness_phases(state, &all_harness_ids).await;

    let mut out = Vec::new();
    for r in selected {
        for h in &r.harnesses {
            out.push(HarnessListing {
                room_id: r.id.clone(),
                room_name: r.name.clone(),
                harness_id: h.id.clone(),
                name: h.name.clone(),
                kind: h.kind.clone(),
                agent: h.agent.clone(),
                session_id: h.session_id.clone(),
                phase: phases
                    .get(&h.id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_owned()),
            });
        }
    }
    Ok(ListHarnessesOut { harnesses: out })
}
