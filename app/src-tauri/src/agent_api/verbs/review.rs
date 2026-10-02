//! The review verbs: reading and answering the reviewer's comments, the diff
//! under review and the sign-off state.

use serde::{Deserialize, Serialize};

use super::review_ctx::{RoomCtx, normalize, parse_anchor, scope_name, thread_in_room};
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::element;
use crate::agent_api::element_source::SourceGuess;
use crate::agent_api::render::{covering_hunk, read_around, render_file, render_hunk};
use crate::db::{Database, ReviewAddressedRow, ReviewCommentRow, ReviewThreadRow};
use crate::review::now_ms;
use crate::review_surface::Scope;
use crate::review_surface::query::{ScopeFiles, file_impl, scope_impl};

/// Rendered diff text is capped so a whole-branch `get_diff` on a large
/// change cannot swallow the agent's context. Truncation is reported,
/// never silent.
const MAX_DIFF_BYTES: usize = 256 * 1024;

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

    // An element thread lives on its entry HTML, but the file the agent
    // must edit is the JSX the element was rendered from, so a filter
    // also matches that source. Anchors load only when a filter is given.
    let elements = if filter.is_some() {
        element::load(db, caller, &rows)?
    } else {
        std::collections::BTreeMap::new()
    };

    let kept: Vec<ReviewThreadRow> = rows
        .into_iter()
        .filter(|t| all || t.resolved_ms.is_none())
        .filter(|t| match filter.as_deref() {
            None => true,
            Some(want) => {
                t.file_path.as_deref().map(normalize).as_deref() == Some(want)
                    || elements
                        .get(&t.id)
                        .and_then(|e| e.anchor.source.as_ref())
                        .is_some_and(|s| normalize(&s.file) == want)
            }
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
        && let Ok(detail) = file_impl(db, &caller.room_id, cwd, path, Scope::Branch, None)
    {
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
    if files.is_empty()
        && let Some(w) = wanted
    {
        return Err(VerbError::NotFound(format!(
            "{w} is not in this review's diff"
        )));
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
pub(super) fn signoff_block(db: &Database, room_id: &str, cwd: &str) -> VerbResult<StatusOut> {
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
