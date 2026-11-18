//! `create_room` (#330): opening a whole new room from an agent.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::close_apply::store_of;
use super::find_rooms::group_key_for_match;
use super::harness_build::{NewHarness, build_harness, resolve, spawn_without_client};
use super::mail::MailContext;
use super::mail_routing::{mail_refusal_for, record_and_emit};
use super::room_naming::derive_prompt_first_line;
use super::shared::{MAX_MESSAGE_BYTES, is_known_kind, known_kind_names};
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;
use crate::db::{CreatedBy, Database, HarnessMessageRow, Room};
use crate::git::repo_root_for_path;
use crate::review::now_ms;

mod record;
mod worktree;

pub use record::{AGENT_ROOM_CREATED_EVENT, AgentRoomCreated};
use record::{FIRST_HARNESS_NAME, NewRoom, emit_created, leave_note, store_room};
use worktree::{BranchMode, CheckoutSpec, prepare};

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
    /// Worktree mode only. Omitted → `skein/<slug of task>`.
    #[serde(default)]
    pub branch: Option<String>,
    /// Worktree mode only. Omitted → the repo's HEAD.
    #[serde(default)]
    pub base_branch: Option<String>,
    pub task: String,
    /// Omitted → `claude`. When given, must name a known harness kind.
    #[serde(default)]
    pub kind: Option<String>,
    /// Omitted → the user's default agent for the kind (#248). An empty
    /// string means no agent.
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
    /// Clickable link to the room (#812); absent without a deep-link scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub harness_id: String,
    /// Clickable link to the new harness (#812); absent without a scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_url: Option<String>,
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

/// Open a whole new room — a new worktree, a new spawned harness, and
/// optionally a queued first prompt (#330).
///
/// This is the one verb an agent can use to start *another* agent
/// working unattended, so it is guarded more heavily than anything else
/// in this file: the Rooms tool group's Settings switch
/// (`tool_groups.rooms`), a per-room rate cap, and a per-
/// repository-group ceiling on how many agent-opened rooms Skein will
/// hold at once (see [`MAX_OPEN_ROOMS_PER_GROUP`]).
///
/// Once those pass, the daemon does the rest itself (#734, no round trip to
/// a webview): it resolves `(kind, agent)`, refuses a `prompt` that could
/// never be delivered *before* anything exists, cuts the worktree, stores
/// the room with its first harness, tells clients about it
/// ([`AGENT_ROOM_CREATED_EVENT`]) and, with no client attached to mount it,
/// starts the harness.
///
/// `room_creation_enabled` is the Rooms tool group off the live
/// `SpawnSettings`, read by the caller (`mcp.rs`/`http.rs`) the same way
/// `mail.policy` is — passed in rather than read from `state` here, so
/// this verb stays testable with a plain bool.
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
        None | Some("worktree") => BranchMode::Worktree,
        Some("current") => BranchMode::Current,
        Some(other) => {
            return Err(VerbError::Refused(format!(
                "unknown branchMode {other:?} — use worktree or current"
            )));
        }
    };

    if let Some(kind) = args.kind.as_deref()
        && !is_known_kind(kind)
    {
        return Err(VerbError::Refused(format!(
            "unknown kind {kind:?} — use one of {:?}",
            known_kind_names()
        )));
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
                "a prompt was given, but the Mail tools are turned off in Skein's Settings, \
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
    if branch_mode == BranchMode::Worktree && !skein_git::Repo::is_repo(path_buf) {
        return Err(VerbError::Refused(format!(
            "{path} is not a git checkout, so branchMode \"worktree\" cannot be used \
             (try \"current\")"
        )));
    }

    if !room_creation_enabled {
        return Err(VerbError::Refused(
            "the Rooms tools are turned off in Skein's Settings".into(),
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
    // A snapshot: concurrent creates can overshoot this ceiling slightly. It
    // was never atomic, and the locked apply below does not change that.
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

    let resolved = resolve(
        state,
        args.kind.as_deref(),
        args.agent.as_deref(),
        Some(&path),
    )
    .await?;
    let kind = resolved.kind.as_str();

    if args.prompt.is_some()
        && let Some(reason) = mail_refusal_for(
            kind,
            resolved.agent.as_deref(),
            Some(&path),
            mail.policy,
            mail.agent_sees_mcp,
        )
    {
        return Err(VerbError::Refused(format!(
            "cannot queue the prompt: the new harness {reason}"
        )));
    }

    let store = store_of(state)?;
    let checkout = {
        let db = std::sync::Arc::clone(db);
        let path = path.clone();
        let task = task.to_owned();
        let branch = args.branch.clone();
        let base_branch = args.base_branch.clone();
        tokio::task::spawn_blocking(move || {
            prepare(
                &db,
                &CheckoutSpec {
                    path: &path,
                    mode: branch_mode,
                    task: &task,
                    branch: branch.as_deref(),
                    base_branch: base_branch.as_deref(),
                },
            )
        })
        .await
        .map_err(|e| internal(format!("creating the worktree panicked: {e}")))??
    };

    // From here a failure leaves the worktree in place, so each says where.
    let harness = build_harness(
        state,
        store,
        NewHarness {
            kind: resolved.kind,
            agent: resolved.agent.clone(),
            name: FIRST_HARNESS_NAME.to_owned(),
            cwd: Some(checkout.cwd.clone()),
            created_by: None,
        },
    )
    .await
    .map_err(|e| leave_note(e, &checkout))?;
    let created_by = CreatedBy {
        room_id: caller.room_id.clone(),
        // A `String`, never absent, like `queue_first_prompt`'s fallback
        // when the request carried no `X-Skein-Harness`.
        harness_id: caller.harness_id.clone().unwrap_or_default(),
        prompt_first_line: derive_prompt_first_line(None, args.prompt.as_deref()),
        base_sha: checkout.base_sha.clone(),
    };
    let new_room = store_room(state, NewRoom { task, created_by }, &checkout, harness)
        .await
        .map_err(|e| leave_note(e, &checkout))?;
    emit_created(state, &new_room, &caller_room.name);
    let harness = new_room
        .harnesses
        .first()
        .cloned()
        .ok_or_else(|| VerbError::Internal("the new room has no first harness".to_owned()))?;
    spawn_without_client(state, &new_room.id, &harness).await;

    let message_id = args.prompt.as_deref().and_then(|prompt| {
        queue_first_prompt(
            db,
            state,
            caller,
            &caller_room,
            &new_room.id,
            &new_room.name,
            &harness.id,
            kind,
            kind,
            prompt,
        )
    });

    let url = state.room_link(&new_room.id);
    let harness_url = state.harness_link(&new_room.id, &harness.id);
    Ok(CreateRoomOut {
        room_id: new_room.id,
        url,
        name: new_room.name,
        cwd: new_room.cwd,
        repo: new_room.repo,
        branch: new_room.branch,
        harness_id: harness.id,
        harness_url,
        kind: kind.to_owned(),
        agent: harness.agent,
        session_id: harness.session_id,
        message_id,
        base_behind_upstream: checkout.base_behind_upstream,
    })
}

/// Queue `prompt` as a harness's first mailbox message (#327), once the
/// daemon has stored the harness that `create_room` made or `open_harness`
/// (#411) opened (#734: no client round trip). Returns the message
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
/// takes real `Room`/`Harness` rows for the recipient, while this
/// takes the plain fields both verbs have to hand.
#[allow(clippy::too_many_arguments)]
pub(super) fn queue_first_prompt(
    db: &Database,
    state: &AgentApiState,
    caller: &Caller,
    caller_room: &Room,
    to_room_id: &str,
    to_room_name: &str,
    to_harness_id: &str,
    to_harness_label: &str,
    to_harness_kind: &str,
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

    let from_harness_kind = caller.harness_id.as_deref().and_then(|hid| {
        caller_room
            .harnesses
            .iter()
            .find(|h| h.id == hid)
            .map(|h| h.kind.as_str())
    });
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
        state.events.as_deref(),
        to_harness_id,
        to_room_id,
        now,
        crate::db::action_kind::MESSAGE_IN,
        &payload,
        // Parsed, not passed through: the kind string came from the
        // frontend, and only a known kind is ever stored.
        crate::harness_kind::HarnessKind::from_name(to_harness_kind)
            .map(crate::harness_kind::HarnessKind::as_str),
    );
    record_and_emit(
        db,
        state.events.as_deref(),
        caller.harness_id.as_deref().unwrap_or(""),
        &caller.room_id,
        now,
        crate::db::action_kind::MESSAGE_OUT,
        &payload,
        from_harness_kind,
    );
    state.notify_mail_changed(to_room_id, to_harness_id);
    Some(row.id)
}
