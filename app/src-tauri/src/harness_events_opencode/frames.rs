//! SSE framing and decoding: split the byte buffer into frames, parse each
//! into an `OpencodeEvent`, and persist the action rows it carries.

use std::collections::HashMap;

use parking_lot::Mutex;

use super::OpencodeEvent;
use super::session::{
    decide_root_session_prompted, parse_session_updated_parent, user_message_session_id,
};
use crate::db::Database;
use crate::harness_actions_opencode;
use crate::harness_kind::HarnessKind;

/// Walk `buf` for complete SSE frames (`...\n\n`), parse each, emit
/// matched events. Leaves any trailing partial frame in `buf` for
/// the next chunk to extend.
///
/// Returns the session ids of any user-role `message.updated` whose
/// root/child status isn't in `root_cache` yet — the caller (the only
/// place in this module allowed to do network I/O) resolves those via
/// `GET /session/<id>` and re-emits `RootSessionPrompted` itself. Kept
/// out of this function so it stays synchronous and testable without a
/// network.
#[allow(clippy::too_many_arguments)]
pub(super) fn process_buffer(
    buf: &mut Vec<u8>,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
    cwd: &str,
    root_cache: &Mutex<HashMap<String, bool>>,
    prompts: &Mutex<harness_actions_opencode::UserPromptTracker>,
) -> Vec<String> {
    let mut unresolved = Vec::new();
    loop {
        let Some(sep) = find_double_newline(buf) else {
            return unresolved;
        };
        let frame: Vec<u8> = buf.drain(..sep + 2).collect();
        let Ok(frame_str) = std::str::from_utf8(&frame[..frame.len().saturating_sub(2)]) else {
            continue;
        };
        let mut payload = String::new();
        for line in frame_str.split('\n') {
            if let Some(rest) = line.strip_prefix("data:") {
                if !payload.is_empty() {
                    payload.push('\n');
                }
                payload.push_str(rest.trim_start());
            }
        }
        if payload.is_empty() {
            continue;
        }
        // Root/child cache maintenance (#116). `session.created` only
        // fires at session birth; `session.updated` re-states the same
        // fact and is the only signal available for a session already
        // alive when the adapter attaches, so it's read here even
        // though it never surfaces its own `OpencodeEvent`.
        if let Some((session_id, parent_id)) = parse_session_updated_parent(&payload) {
            root_cache.lock().insert(session_id, parent_id.is_none());
        }
        // Phase event (existing path).
        if let Some(event) = parse_event(&payload) {
            if let OpencodeEvent::SessionCreated {
                session_id,
                parent_id,
            } = &event
            {
                root_cache
                    .lock()
                    .insert(session_id.clone(), parent_id.is_none());
            }
            on_event(event);
        }
        // Root-session follow decision (#116): independent of
        // `UserMessageAgent` above, which requires an `agent` field —
        // this must not, since the only thing that matters here is
        // "a user typed something in a session we know is root."
        if let Some(session_id) = user_message_session_id(&payload) {
            match root_cache.lock().get(&session_id).copied() {
                Some(is_root) => {
                    if let Some(event) = decide_root_session_prompted(&session_id, Some(is_root)) {
                        on_event(event);
                    }
                }
                None => unresolved.push(session_id),
            }
        }
        // Action extraction (issue #80). Live rows broadcast to the
        // frontend; backfill (in attach()) is silent.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
            let mut actions = harness_actions_opencode::extract_from_sse(&value);
            // The human's prompt text (#525). A known child session's
            // "user" message is the parent's task text, so drop it. An
            // unknown session is emitted: a missing prompt makes the
            // feed look dead (the #525 bug), so the safe direction is to
            // show it. A child announces itself via `session.created`
            // with a `parentID` before its first message, so unknown is
            // almost always a root seen after a reconnect or a mid-life
            // attach.
            if let Some(prompt) = prompts.lock().observe(&value)
                && should_emit_prompt(root_cache.lock().get(&prompt.session_id).copied())
            {
                actions.push(prompt.action);
            }
            for action in actions {
                record_and_emit(db, app, harness_id, room_id, cwd, &action);
            }
        }
    }
}

/// Whether a user prompt is shown: dropped only for a known child
/// session (`Some(false)`); root or unknown is emitted.
pub(super) fn should_emit_prompt(root: Option<bool>) -> bool {
    root != Some(false)
}

/// Persist one live action and broadcast it. SSE rows are live by
/// definition; the DB backfill path deliberately does not capture
/// baselines (see review.rs).
pub(super) fn record_and_emit(
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
    cwd: &str,
    action: &harness_actions_opencode::ExtractedAction,
) {
    match db.record_harness_action(
        harness_id,
        room_id,
        action.timestamp_ms,
        action.kind,
        &action.payload,
        action.source.as_deref(),
        Some(HarnessKind::Opencode.as_str()),
    ) {
        Ok(id) => {
            if action.kind == crate::db::action_kind::PATCH {
                crate::review::note_patch(db, room_id, cwd, harness_id, &action.payload);
            }
            if let Some(app) = app {
                crate::harness_action_event::emit(
                    app,
                    id,
                    harness_id,
                    room_id,
                    action.timestamp_ms,
                    action.kind,
                    &action.payload,
                    action.source.as_deref(),
                    Some(HarnessKind::Opencode.as_str()),
                );
            }
        }
        Err(e) => {
            tracing::trace!(harness_id, kind = action.kind, error = %e,
                "opencode_events: record_harness_action failed");
        }
    }
}

pub(super) fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Parse one SSE `data:` payload into an `OpencodeEvent`. Returns
/// `None` for events we don't surface (`server.heartbeat`,
/// `session.updated`, etc.) and for malformed JSON. Server schema
/// catalog lives in `docs/epic-50-l2c-2-recon.md`.
pub(super) fn parse_event(payload: &str) -> Option<OpencodeEvent> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let ty = value.get("type")?.as_str()?;
    let props = value.get("properties");

    match ty {
        "session.status" => {
            let status_type = props
                .and_then(|p| p.get("status"))
                .and_then(|s| s.get("type"))
                .and_then(serde_json::Value::as_str)?;
            match status_type {
                "busy" => Some(OpencodeEvent::SessionBusy),
                "idle" => Some(OpencodeEvent::SessionIdle),
                // Other status sub-types observed in the wild ("text"
                // showed up briefly during dogfooding) — ignore until
                // we know they're meaningful for the state machine.
                _ => None,
            }
        }
        "session.created" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            // `properties.info` is the full session object; `parentID`
            // set means a subagent (task-tool) child, absent means a
            // root session — verified upstream, no ambiguous case
            // (#116). Leniently optional: a `session.created` shape
            // that ever omits `info` still yields a `SessionCreated`
            // for resume-id capture, just with an unknown parent.
            let parent_id = props
                .and_then(|p| p.get("info"))
                .and_then(|info| info.get("parentID"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            Some(OpencodeEvent::SessionCreated {
                session_id,
                parent_id,
            })
        }
        "message.part.delta" => Some(OpencodeEvent::MessageDelta),
        "message.updated" => {
            // Shape measured against opencode 1.18.30 (#248):
            // `{"type":"message.updated","properties":{"sessionID":…,
            //   "info":{"role":"user","sessionID":…,"agent":"plan",…}}}`.
            // `mode` is the older name for the same field, as in
            // `skein_harness::opencode`.
            let info = props.and_then(|p| p.get("info"))?;
            if info.get("role").and_then(serde_json::Value::as_str) != Some("user") {
                return None;
            }
            let session_id = info
                .get("sessionID")
                .or_else(|| props.and_then(|p| p.get("sessionID")))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let agent = info
                .get("agent")
                .or_else(|| info.get("mode"))
                .and_then(serde_json::Value::as_str)
                .filter(|a| !a.is_empty())?
                .to_owned();
            Some(OpencodeEvent::UserMessageAgent { session_id, agent })
        }
        // `id` names the request on `*.asked`; `requestID` names it on
        // the matching `*.replied`/`*.rejected` — verified against a
        // live opencode 1.18.30 binary (#86). Not the same field, and
        // not a typo to "fix" into consistency.
        "permission.asked" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("id"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::PermissionAsked {
                request_id,
                session_id,
            })
        }
        "permission.replied" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("requestID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::PermissionReplied {
                request_id,
                session_id,
            })
        }
        "question.asked" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("id"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::QuestionAsked {
                request_id,
                session_id,
            })
        }
        "question.replied" | "question.rejected" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("requestID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::QuestionResolved {
                request_id,
                session_id,
            })
        }
        "message.part.updated" => {
            // Look at the part type — tool calls surface here as a
            // sub-object with type=tool. Other part types
            // (step-start, step-finish, text) are noisy for our
            // purposes today.
            let part = props.and_then(|p| p.get("part"))?;
            let part_type = part.get("type").and_then(serde_json::Value::as_str)?;
            match part_type {
                "tool" => {
                    let name = part
                        .get("name")
                        .and_then(serde_json::Value::as_str)?
                        .to_owned();
                    // Some tool rows are mid-call updates; for v1 we
                    // emit one per row and let the frontend dedupe by
                    // staying in `running`. L7 will need finer
                    // resolution.
                    Some(OpencodeEvent::ToolUseStart { name })
                }
                _ => None,
            }
        }
        // Everything else — `session.idle` (redundant with the
        // `session.status` idle that fires alongside it),
        // `session.updated` (read separately by
        // `parse_session_updated_parent` for the #116 root/child
        // cache, but it has no `OpencodeEvent` of its own),
        // `session.diff`, `server.heartbeat`, `mcp.tools.changed`,
        // future event types — is silent at the policy layer. See
        // recon §3 for the catalog. Permission and question events
        // are handled above, not here.
        _ => None,
    }
}
