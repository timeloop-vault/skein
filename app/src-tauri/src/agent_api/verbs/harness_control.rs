//! `open_harness` and `close_harness` (#411): an agent changing which harnesses
//! a room has.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::mail::MailContext;
use super::mail_routing::mail_refusal_for;
use super::room_close::AgentClosedBy;
use super::room_create::{CREATE_ROOM_TIMEOUT, ResolveOut, queue_first_prompt};
use super::shared::{KNOWN_HARNESS_KINDS, MAX_MESSAGE_BYTES, RESOLVE_TIMEOUT, frontend_error};
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;
use crate::git::{IdentityCheck, check_identity};

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
