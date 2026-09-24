//! What an agent can actually do.
//!
//! Five verbs, all plain functions over a [`Database`] and a
//! [`Caller`]. Nothing here knows about HTTP or MCP, which is what
//! makes the interesting properties — room scoping, the resolve
//! prohibition, attribution — testable without a server or a Tauri
//! runtime.
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

use serde::{Deserialize, Serialize};
use skein_review::{FileState, Hunk, LineKind};

use super::auth::Caller;
use crate::db::{
    Database, Harness, HarnessMessageRow, ReviewAddressedRow, ReviewCommentRow, ReviewThreadRow,
    Room,
};
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
#[derive(Debug, Clone, Copy)]
pub struct MailContext {
    pub policy: MailPolicy,
    pub agent_sees_mcp: AgentSeesMcp,
}

impl MailContext {
    /// The same defaults as [`MailPolicy::permissive`], for tests that
    /// have no opinion on messaging at all.
    #[cfg(test)]
    pub fn permissive() -> Self {
        Self {
            policy: MailPolicy::permissive(),
            agent_sees_mcp: |_, _, _| true,
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
pub fn send_message(
    db: &Database,
    caller: &Caller,
    args: &SendMessageArgs,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
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

    let (room, harness) = resolve_mail_target(db, to, policy, agent_sees_mcp)?;

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
            let sender_room = rooms.iter().find(|r| r.id == m.from_room_id);
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

/// Resolve `to` into a concrete `(room, harness)` — a harness id first,
/// searched across every room, then a room id resolved to its lead
/// harness.
fn resolve_mail_target(
    db: &Database,
    to: &str,
    policy: MailPolicy,
    agent_sees_mcp: AgentSeesMcp,
) -> VerbResult<(Room, Harness)> {
    let rooms = db.all_rooms().map_err(internal)?;

    for room in &rooms {
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
    let injected = match h.kind.as_str() {
        "claude" => policy.claude_injected,
        "opencode" => policy.opencode_injected,
        _ => {
            return Some(format!(
                "{} harnesses have no MCP connection to receive messages on",
                h.kind
            ));
        }
    };
    if !injected {
        return Some(format!(
            "config injection for {} is turned off in Settings, so it cannot see \
             the messaging tool",
            h.kind
        ));
    }
    if let (Some(agent), Some(cwd)) = (h.agent.as_deref(), cwd) {
        if !agent_sees_mcp(&h.kind, agent, cwd) {
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
