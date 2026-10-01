//! Mail plumbing shared by the mailbox verbs and the room/harness verbs that
//! send mail: resolving a recipient, refusing sends, recording and emitting.

use super::mail::{AgentSeesMcp, MailPolicy};
use super::{VerbError, VerbResult};
use crate::agent_api::auth::Caller;
use crate::db::{Database, Harness, HarnessMessageRow, Room};

/// Resolve `to` into a concrete `(room, harness)` — a harness id first,
/// searched across every room, then a room id resolved to its lead
/// harness. Takes the room list rather than a `Database` so
/// `send_message` can reuse the one `all_rooms` read for the sender's
/// own name too (#329).
pub(super) fn resolve_mail_target(
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
pub(super) fn mail_refusal(
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
pub(super) fn mail_refusal_for(
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
pub(super) fn mail_lead_harness(
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
pub(super) fn find_room<'a>(rooms: &'a [Room], room_id: &str) -> Option<&'a Room> {
    rooms.iter().find(|r| r.id == room_id)
}

/// Record the two `harness_actions` rows a successful `send_message`
/// leaves behind (#329): `message_in` for the recipient, `message_out`
/// for the sender, same timestamp, same payload — everything a feed row
/// needs to say who talked to whom, and nothing a mailbox reply needs
/// hidden (no body).
pub(super) fn record_mail_actions(
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
pub(super) fn record_and_emit(
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
