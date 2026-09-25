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
use std::fmt::Write as _;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use skein_review::{FileState, Hunk, LineKind};

use super::auth::Caller;
use super::state::AgentApiState;
use crate::db::{
    Database, Harness, HarnessMessageRow, ReviewAddressedRow, ReviewCommentRow, ReviewThreadRow,
    Room,
};
use crate::git::repo_root_for_path;
use crate::review::{abs_path, now_ms};
use crate::review_surface::Scope;
use crate::review_surface::query::{ScopeFiles, file_impl, scope_impl};

/// Rendered diff text is capped so a whole-branch `get_diff` on a large
/// change cannot swallow the agent's context. Truncation is reported,
/// never silent.
const MAX_DIFF_BYTES: usize = 256 * 1024;

/// How many lines of today's file to show either side of a comment.
const CONTEXT_RADIUS: usize = 6;

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

fn internal(e: impl std::fmt::Display) -> VerbError {
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
    /// `line` | `file` | `commit` | `review`.
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Where the comment sits *now*, 1-based inclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<usize>,
    /// The code could not be found where the comment was written, so
    /// the position is a guess or missing entirely. Read the anchor.
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
    let mut ctx = RoomCtx::load(db, caller, &[])?;

    let mut diff_context = None;
    if let (Some(cwd), Some(path)) = (caller.cwd.as_deref(), row.file_path.as_deref()) {
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
    let current_context = match (caller.cwd.as_deref(), row.file_path.as_deref()) {
        (Some(cwd), Some(path)) => match (thread.line_start, thread.line_end) {
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
    let s = crate::review_surface::signoff::status_impl(db, &caller.room_id, cwd)
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

/// Normalize a filesystem path for comparison, mirroring the frontend's
/// `normalizePath` (`app/src/roomGroups.ts`) exactly: backslashes
/// become forward slashes, one trailing slash is stripped (never down
/// to an empty string), and the result is lowercased. Kept in
/// lock-step with that function rather than shared with it — this side
/// reads sqlite JSON, that side reads `Room[]`, and there is no module
/// to share across the FFI boundary.
fn normalize_path_for_match(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    if s.chars().count() > 1 && s.ends_with('/') {
        s.pop();
    }
    s.to_lowercase()
}

/// Whether normalized `a` sits under normalized `b` as a strict
/// descendant, on path-segment boundaries — `/a/foo` must never match
/// under `/a/foobar`. `b` already ending in `/` (a normalized root)
/// is not given a second one.
fn is_strictly_under(a: &str, b: &str) -> bool {
    let prefix = if b.ends_with('/') {
        b.to_owned()
    } else {
        format!("{b}/")
    };
    a.starts_with(&prefix)
}

/// `"cwd"` for an exact match, `"inside_room"` when the query sits
/// strictly under the room's cwd (the query is somewhere inside the
/// room), `"contains_room"` when the room's cwd sits strictly under the
/// query (the query is a parent folder of the room), `None` otherwise.
fn path_match_kind(query_norm: &str, cwd_norm: &str) -> Option<&'static str> {
    if query_norm == cwd_norm {
        Some("cwd")
    } else if is_strictly_under(query_norm, cwd_norm) {
        Some("inside_room")
    } else if is_strictly_under(cwd_norm, query_norm) {
        Some("contains_room")
    } else {
        None
    }
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
        let archived = room.archived.is_some();
        let entry = RoomPathMatch {
            room_id: room.id.clone(),
            name: room.name.clone(),
            cwd: cwd.to_owned(),
            repo_root: room.repo_root.clone(),
            branch: room.branch.clone(),
            archived,
            safe_to_remove: archived,
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
const KNOWN_HARNESS_KINDS: &[&str] = &["claude", "opencode", "copilot", "byoh", "files"];

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
    let new_group_key = new_repo_root.as_deref().map(normalize_path_for_match);
    let scoped_open_count = rooms
        .iter()
        .filter(|r| {
            r.archived.is_none()
                && r.created_by.is_some()
                && r.repo_root.as_deref().map(normalize_path_for_match) == new_group_key
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

    let message_id = args
        .prompt
        .as_deref()
        .and_then(|prompt| queue_first_prompt(db, state, caller, &caller_room, &created, prompt));

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

/// Queue `prompt` as the new harness's first mailbox message (#327),
/// once `create_room`'s second round trip has actually made it. Returns
/// the message id, or `None` if the insert itself failed — a caller
/// must never see a `messageId` in the response for a message that does
/// not exist in `harness_messages`.
///
/// Deliberately does not go through [`send_message`] — this is not
/// counted against [`SEND_RATE_LIMIT`], since the room that just made
/// the request already paid [`ROOM_CREATION_RATE_LIMIT`], and the new
/// harness's unread count is trivially zero. Mirrors
/// [`record_mail_actions`]'s payload shape, built by hand rather than
/// reusing that helper: it takes real `Room`/`Harness` rows for the
/// recipient, and the room `create_room` just made is not yet one — the
/// frontend has not autosaved it (see the module docs on why that is
/// safe).
fn queue_first_prompt(
    db: &Database,
    state: &AgentApiState,
    caller: &Caller,
    caller_room: &Room,
    created: &CreatedRoomOut,
    prompt: &str,
) -> Option<String> {
    let now = now_ms();
    let row = HarnessMessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        room_id: created.room_id.clone(),
        harness_id: created.harness_id.clone(),
        from_room_id: caller.room_id.clone(),
        from_harness_id: caller.harness_id.clone(),
        body: prompt.to_owned(),
        created_ms: now,
        read_ms: None,
    };
    if let Err(e) = db.insert_harness_message(&row) {
        // The room itself was already created — reporting this as a
        // `create_room` failure would be worse than the prompt simply
        // not arriving, the same call `send_message` makes for the
        // harness_actions rows below. But the caller must not be told a
        // `messageId` for a row that was never written, so this is the
        // one path that returns `None` rather than `Some(row.id)`.
        tracing::warn!(
            room_id = %created.room_id, harness_id = %created.harness_id, error = %e,
            "agent_api: create_room's first-prompt message failed to insert"
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
        "to_room_id": created.room_id,
        "to_room_name": created.name,
        "to_harness_id": created.harness_id,
        "to_harness_label": created.kind,
    })
    .to_string();
    record_and_emit(
        db,
        state.app.as_ref(),
        &created.harness_id,
        &created.room_id,
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
    state.notify_mail_changed(&created.room_id, &created.harness_id);
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

        Ok(Self {
            comments,
            addressed,
            labels,
            placed,
        })
    }

    fn to_agent_thread(&self, t: &ReviewThreadRow) -> AgentThread {
        let (line_start, line_end, outdated) =
            self.placed.get(&t.id).copied().unwrap_or_else(|| {
                (
                    t.line_start.and_then(|n| usize::try_from(n).ok()),
                    t.line_end.and_then(|n| usize::try_from(n).ok()),
                    false,
                )
            });
        AgentThread {
            thread_id: t.id.clone(),
            scope: t.scope.clone(),
            file: t.file_path.clone(),
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

/// The hunk whose new-side range covers `line`.
fn covering_hunk(hunks: &[Hunk], line: usize) -> Option<&Hunk> {
    hunks
        .iter()
        .find(|h| line >= h.new_start && line < h.new_start + h.new_lines)
}

/// `line_start..=line_end` in the file on disk, with a few lines either
/// side and 1-based numbers, so the agent sees what is there now and
/// not only what was there when the comment was written.
fn read_around(cwd: &str, path: &str, start: usize, end: usize) -> Option<String> {
    let FileState::Text(text) = skein_review::read_state(&abs_path(cwd, path)) else {
        return None;
    };
    let lines: Vec<&str> = text.lines().collect();
    let from = start.saturating_sub(CONTEXT_RADIUS).max(1);
    let to = (end + CONTEXT_RADIUS).min(lines.len());
    if from > to || from > lines.len() {
        return None;
    }
    let mut out = String::new();
    for (offset, line) in lines[from - 1..to].iter().enumerate() {
        let n = from + offset;
        let marker = if n >= start && n <= end { '>' } else { ' ' };
        let _ = writeln!(out, "{marker} {n:>5} | {line}");
    }
    Some(out)
}

/// One file as a unified diff, the way `git diff` would print it.
///
/// Text, not the [`Hunk`] tree the pane renders: an agent reads a patch
/// natively and would have to reconstruct one from the JSON anyway.
fn render_file(path: &str, blocked: Option<&'static str>, hunks: &[Hunk]) -> String {
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    if let Some(reason) = blocked {
        let _ = writeln!(out, "(no line diff: {reason})");
        return out;
    }
    if hunks.is_empty() {
        out.push_str("(no changes in this scope)\n");
        return out;
    }
    for h in hunks {
        out.push_str(&render_hunk(h));
    }
    out
}

fn render_hunk(h: &Hunk) -> String {
    let mut out = format!("{}\n", h.header);
    for l in &h.lines {
        let sign = match l.kind {
            LineKind::Add => '+',
            LineKind::Delete => '-',
            LineKind::Context => ' ',
        };
        out.push(sign);
        out.push_str(&l.content);
        out.push('\n');
    }
    out
}
