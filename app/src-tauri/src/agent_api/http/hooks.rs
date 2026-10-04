//! The hook routes: pings from the injected Claude plugin's hooks
//! (`PermissionRequest` #86, `SessionStart` #273). Not agent verbs.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::super::state::AgentApiState;
use super::{authenticate, error_body, refuse};

/// The injected Claude plugin's `PermissionRequest` hook posts here
/// (#86) — not an agent verb, so it carries no MCP tool and is not in
/// `mcp.rs`. Unlike the review verbs, the harness id here is not mere
/// attribution: it *is* what the event is about, so a header that is
/// absent or names a harness the room doesn't contain is a 400, not a
/// silent no-op.
///
/// The body is Claude Code's own hook payload shape, not ours to
/// police: only `tool_name`, `agent_type` and `agent_id` are read out
/// of it, and a body that fails to parse as JSON at all still succeeds
/// — a hook that changes shape in a future Claude Code release must not
/// start breaking the harness's turn. `tool_input` is deliberately
/// never read; it can hold secrets a permission dialog is about to ask
/// on.
pub(super) async fn api_harness_permission(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    // `harness_label` is Some only when `X-Skein-Harness` named a
    // harness the room actually contains (see `identify_harness`) — the
    // same signal `harness_id` alone can't give, since an unknown hint
    // is still kept as attribution.
    let (Some(harness_id), Some(_)) = (caller.harness_id.clone(), caller.harness_label.as_ref())
    else {
        return error_body(
            StatusCode::BAD_REQUEST,
            "X-Skein-Harness must name a harness this room actually contains",
        );
    };
    let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let tool_name = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let agent_type = payload
        .get("agent_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let agent_id = payload
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let session_id = valid_session_id(&payload);
    tracing::info!(
        harness_id = %harness_id,
        room_id = %caller.room_id,
        tool_name = tool_name.as_deref(),
        agent_type = agent_type.as_deref(),
        agent_id = agent_id.as_deref(),
        session_id = session_id.as_deref(),
        "agent api: harness permission ping"
    );
    state.notify_harness_permission(
        &caller.room_id,
        &harness_id,
        tool_name,
        agent_type,
        agent_id,
        session_id,
    );
    StatusCode::NO_CONTENT.into_response()
}

/// The injected Claude plugin's `SessionStart` hook posts here (#273)
/// — like the permission route above, this is not an agent verb: it
/// carries no MCP tool and is not in `mcp.rs`.
///
/// The body is Claude Code's own hook payload, not ours to police: a
/// body that fails to parse as JSON at all still succeeds — a future
/// Claude Code payload change must not start breaking a harness's
/// launch. Since #116, `session_id` and `source` are forwarded (see
/// `session_start_fields`) so the frontend can follow a `/clear` or an
/// in-tool `/resume` onto the new conversation id — `source == "clear"`
/// and `source == "resume"` (and `fork`) follow at once. A `startup`
/// with a different id is adopted only once its transcript exists while
/// the bound one does not (#539) — the #78455 phantom never writes one.
///
/// Duplicate fires are expected and must stay harmless. Upstream
/// anthropics/claude-code#78455 reports `SessionStart` firing twice
/// within a few hundred ms for the same project — once for a
/// "phantom" session that never materialises, with a payload
/// indistinguishable from the real one — so nothing here or
/// downstream may hang a consume-once side effect on this route:
/// emitting the event twice is fine, since the frontend handler
/// (`noteLaunchSignal`) is idempotent — it always records a
/// timestamp, and only moves the phase when the transcript tail
/// hasn't already spoken for itself (or when recovering a harness its
/// own launch-silent watchdog gave up on), never overriding
/// `permission` or `exited`.
pub(super) async fn api_harness_session_start(
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
    let (session_id, source) = session_start_fields(&body);
    tracing::info!(
        harness_id = %harness_id,
        room_id = %caller.room_id,
        session_id = session_id.as_deref(),
        source = source.as_deref(),
        "agent api: harness session-start ping"
    );
    state.notify_harness_session_start(&caller.room_id, &harness_id, session_id, source);
    StatusCode::NO_CONTENT.into_response()
}

/// The largest `source` value passed through — Claude's own values
/// (`startup`/`resume`/`clear`/`compact`/`fork`) are all well under
/// this; a longer value is treated the same as absent rather than
/// truncated.
const MAX_SOURCE_LEN: usize = 32;

/// The largest plausible session id. Claude's ids are UUIDs (36
/// chars); this is generous headroom, not a format promise.
const MAX_SESSION_ID_LEN: usize = 128;

/// Pulls `session_id` and `source` out of a `SessionStart` hook body,
/// tolerating a body that isn't JSON at all (returns `(None, None)`).
///
/// `session_id` is validated, not merely extracted: the frontend turns
/// it straight into a transcript file name (`<sid>.jsonl`), so a value
/// containing a path separator or `..` must never get through. Only a
/// non-empty string of at most [`MAX_SESSION_ID_LEN`] ASCII
/// alphanumerics, `-` and `_` is accepted; anything else — including a
/// non-string JSON value — becomes `None`. `source` is passed through
/// verbatim (no trimming) when it is a string of at most
/// [`MAX_SOURCE_LEN`]; longer or non-string values become `None`.
fn session_start_fields(body: &str) -> (Option<String>, Option<String>) {
    let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    (
        valid_session_id(&payload),
        short_string_field(&payload, "source"),
    )
}

/// `session_id` from a hook payload, validated as described on
/// [`session_start_fields`].
fn valid_session_id(payload: &Value) -> Option<String> {
    payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= MAX_SESSION_ID_LEN
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .map(str::to_owned)
}

/// A string field of at most [`MAX_SOURCE_LEN`], verbatim; else `None`.
fn short_string_field(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| s.len() <= MAX_SOURCE_LEN)
        .map(str::to_owned)
}

/// `session_id` and `reason` out of a `SessionEnd` hook body (#318),
/// validated the same way as [`session_start_fields`].
fn session_end_fields(body: &str) -> (Option<String>, Option<String>) {
    let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    (
        valid_session_id(&payload),
        short_string_field(&payload, "reason"),
    )
}

/// The injected plugin's `SessionEnd` hook posts here (#318). Mirrors
/// [`api_harness_session_start`]: same auth, same `X-Skein-Harness`
/// rule, a non-JSON body still answers `204`.
pub(super) async fn api_harness_session_end(
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
    let (session_id, reason) = session_end_fields(&body);
    tracing::info!(
        harness_id = %harness_id,
        room_id = %caller.room_id,
        session_id = session_id.as_deref(),
        reason = reason.as_deref(),
        "agent api: harness session-end ping"
    );
    state.notify_harness_session_end(&caller.room_id, &harness_id, session_id, reason);
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_start_fields_reads_a_valid_id_and_source() {
        let body = r#"{"session_id":"9c1f2e3a-1111-2222-3333-444455556666","source":"clear"}"#;
        assert_eq!(
            session_start_fields(body),
            (
                Some("9c1f2e3a-1111-2222-3333-444455556666".to_owned()),
                Some("clear".to_owned())
            )
        );
    }

    #[test]
    fn session_start_fields_tolerates_missing_fields() {
        assert_eq!(session_start_fields("{}"), (None, None));
    }

    #[test]
    fn session_start_fields_tolerates_a_non_json_body() {
        assert_eq!(session_start_fields("not json at all"), (None, None));
    }

    #[test]
    fn session_start_fields_rejects_a_traversal_attempt() {
        for bad in ["../x", "a/b", "a\\b"] {
            let body = json!({ "session_id": bad }).to_string();
            let (session_id, _) = session_start_fields(&body);
            assert_eq!(session_id, None, "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn session_start_fields_rejects_an_overlong_id() {
        let body = json!({ "session_id": "a".repeat(MAX_SESSION_ID_LEN + 1) }).to_string();
        assert_eq!(session_start_fields(&body).0, None);
    }

    #[test]
    fn session_start_fields_rejects_a_non_string_session_id() {
        let body = json!({ "session_id": 12345 }).to_string();
        assert_eq!(session_start_fields(&body).0, None);
    }

    #[test]
    fn session_start_fields_ignores_an_overlong_source() {
        let body = json!({ "source": "x".repeat(MAX_SOURCE_LEN + 1) }).to_string();
        assert_eq!(session_start_fields(&body).1, None);
    }

    #[test]
    fn session_end_fields_reads_a_valid_id_and_reason() {
        let body = r#"{"session_id":"9c1f2e3a-1111","reason":"prompt_input_exit"}"#;
        assert_eq!(
            session_end_fields(body),
            (
                Some("9c1f2e3a-1111".to_owned()),
                Some("prompt_input_exit".to_owned())
            )
        );
    }

    #[test]
    fn session_end_fields_tolerates_garbage_and_rejects_bad_values() {
        assert_eq!(session_end_fields("not json"), (None, None));
        assert_eq!(session_end_fields("{}"), (None, None));
        let body =
            json!({ "session_id": "../x", "reason": "x".repeat(MAX_SOURCE_LEN + 1) }).to_string();
        assert_eq!(session_end_fields(&body), (None, None));
    }

    #[test]
    fn valid_session_id_is_what_the_permission_route_reads() {
        assert_eq!(
            valid_session_id(&json!({ "session_id": "abc-1_2" })),
            Some("abc-1_2".to_owned())
        );
        assert_eq!(valid_session_id(&json!({ "session_id": "a/b" })), None);
        assert_eq!(valid_session_id(&json!({ "session_id": 5 })), None);
    }
}
