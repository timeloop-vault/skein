//! The Hermes shell-hook routes (#625). Hermes' hook script POSTs to the
//! two Claude hook URLs for the events that have a Claude twin
//! (`pre_approval_request`/`post_approval_response` →
//! `/api/harness/permission`, `on_session_start` →
//! `/api/harness/session-start`) and to `/api/harness/hermes-turn` for
//! the rest. Every one of them becomes the single
//! `skein://harness-hermes-hook` event; the Claude-shaped events are
//! never emitted for a Hermes body.
//!
//! The payload shape belongs to Hermes, so nothing here fails the
//! hook: an unreadable or non-Hermes body answers `204` and emits
//! nothing. `tool_input` and prompt text are never read, only the
//! bounded fields [`HarnessHermesHook`] names.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use skein_harness::hermes::hooks::{HermesHook, parse_hook};

use super::super::hermes_event::HarnessHermesHook;
use super::super::state::AgentApiState;
use super::hooks::plausible_session_id;
use super::{authenticate, error_body, refuse};

/// Largest accepted body on the three hook routes. Hermes' `pre_llm_call`
/// carries the whole conversation history and the `.sh` hook posts it
/// raw, so this is far above the 1 MiB default; the fields we read are
/// still capped individually.
pub(super) const MAX_HOOK_BODY: usize = 64 * 1024 * 1024;

/// Cap for `description` (the matched dangerous-command pattern).
const MAX_DESCRIPTION_CHARS: usize = 500;
/// Cap for every other forwarded string.
const MAX_FIELD_CHARS: usize = 128;

/// The hook payload when `body` is a Hermes one: JSON whose top-level
/// `hook_event_name` is a string in Hermes' `snake_case` spelling.
/// Claude's own names (`PermissionRequest`, `SessionStart`, …) also sit
/// in a `hook_event_name` field, but start uppercase, which is what
/// keeps the shared URLs unambiguous.
pub(super) fn parse_hermes_body(body: &str) -> Option<(String, HermesHook)> {
    let payload: Value = serde_json::from_str(body).ok()?;
    let name = payload.get("hook_event_name")?.as_str()?;
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        return None;
    }
    let hook = parse_hook(&payload)?;
    Some((cap(name, MAX_FIELD_CHARS), hook))
}

fn cap(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn capped(s: Option<String>, max_chars: usize) -> Option<String> {
    s.map(|s| cap(&s, max_chars))
}

fn valid_id(s: Option<String>) -> Option<String> {
    s.filter(|s| plausible_session_id(s))
}

/// Log and emit one Hermes hook. Never logs description or command text.
pub(super) fn emit(
    state: &AgentApiState,
    room_id: &str,
    harness_id: &str,
    event: String,
    hook: HermesHook,
) {
    let session_id = valid_id(hook.session_id);
    let turn_id = capped(hook.turn_id, MAX_FIELD_CHARS);
    let surface = capped(hook.surface, MAX_FIELD_CHARS);
    tracing::info!(
        harness_id = %harness_id,
        room_id = %room_id,
        event = %event,
        session_id = session_id.as_deref(),
        turn_id = turn_id.as_deref(),
        surface = surface.as_deref(),
        "agent api: harness hermes hook"
    );
    state.notify_harness_hermes_hook(HarnessHermesHook {
        room_id: room_id.to_owned(),
        harness_id: harness_id.to_owned(),
        event,
        session_id,
        turn_id,
        tool_call_id: capped(hook.tool_call_id, MAX_FIELD_CHARS),
        tool_name: capped(hook.tool_name, MAX_FIELD_CHARS),
        platform: capped(hook.platform, MAX_FIELD_CHARS),
        surface,
        description: capped(hook.description, MAX_DESCRIPTION_CHARS),
        choice: capped(hook.choice, MAX_FIELD_CHARS),
        completed: hook.completed,
        interrupted: hook.interrupted,
        failed: hook.failed,
        turn_exit_reason: capped(hook.turn_exit_reason, MAX_FIELD_CHARS),
        model: capped(hook.model, MAX_FIELD_CHARS),
        child_session_id: valid_id(hook.child_session_id),
    });
}

/// `POST /api/harness/hermes-turn`: turn, tool and subagent events.
/// Same auth and harness rule as the other hook routes; a body that is
/// not a Hermes hook still answers `204`.
pub(super) async fn api_harness_hermes_turn(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let (Some(harness_id), Some(_)) = (caller.harness_id.clone(), caller.harness_label.as_ref())
    else {
        return error_body(
            StatusCode::BAD_REQUEST,
            "X-Skein-Harness must name a harness this room actually contains",
        );
    };
    if let Some((event, hook)) = parse_hermes_body(&body) {
        emit(&state, &caller.room_id, &harness_id, event, hook);
    }
    StatusCode::NO_CONTENT.into_response()
}
