//! Root/child session tracking (#116): which session a frame belongs to and
//! whether it is a root session the harness should follow.

use std::time::Duration;

use super::OpencodeEvent;

/// Budget for the `GET /session/<id>` root/child lookup (#116),
/// used only when a user message arrives in a session the adapter has
/// no cached answer for. Short: it blocks the SSE read loop from the
/// next frame while it runs, and a slow/failed lookup should give up
/// fast rather than stall the loop.
pub(super) const SESSION_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// `session.updated`'s root/child fact (#116), extracted the same way
/// as `session.created`'s but never surfaced as its own
/// `OpencodeEvent` — the frontend has no use for "session metadata
/// changed." Read purely to backfill the root/child cache for a
/// session the adapter attached to mid-life, whose `session.created`
/// (fired once, at birth) it never saw.
pub(super) fn parse_session_updated_parent(payload: &str) -> Option<(String, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("session.updated") {
        return None;
    }
    let props = value.get("properties")?;
    // Both the id and the parent fact live under `info` — unlike
    // `session.created`, `session.updated` has no top-level
    // `sessionID` to fall back on, so a frame without `info` carries
    // nothing usable and is skipped rather than guessed at.
    let info = props.get("info")?;
    let session_id = info
        .get("id")
        .or_else(|| info.get("sessionID"))
        .and_then(serde_json::Value::as_str)?
        .to_owned();
    let parent_id = info
        .get("parentID")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some((session_id, parent_id))
}

/// Session id of a user-role `message.updated`, independent of the
/// `agent`/`mode` field that `UserMessageAgent` requires (#116) — the
/// root/child follow decision cares only that a user typed something,
/// not what agent it went to.
pub(super) fn user_message_session_id(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("message.updated") {
        return None;
    }
    let props = value.get("properties")?;
    let info = props.get("info")?;
    if info.get("role").and_then(serde_json::Value::as_str) != Some("user") {
        return None;
    }
    info.get("sessionID")
        .or_else(|| props.get("sessionID"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Turn "we now know whether `session_id` is root" into the follow
/// event, or not (#116). Pure, so the cache-hit path (in
/// `process_buffer`) and the lookup-hit path (after the async
/// `GET /session/<id>`, in `stream_events`) share one decision, and it
/// is testable without touching the network: `is_root = None` stands
/// in for a failed or ambiguous lookup, which must never be followed
/// on a guess.
pub(super) fn decide_root_session_prompted(
    session_id: &str,
    is_root: Option<bool>,
) -> Option<OpencodeEvent> {
    is_root
        .filter(|&root| root)
        .map(|_| OpencodeEvent::RootSessionPrompted {
            session_id: session_id.to_owned(),
        })
}

/// opencode session ids look like `ses_<alnum>`. Reject anything else
/// before it goes into a URL path segment — defense in depth, since
/// the id already came off the wire as a JSON string, not user input,
/// but a malformed one has no business being ranged over `/session/`.
pub(super) fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Seed value for the harness's own attached/resumed session (#116
/// follow-up). Pure so the seeding decision is testable without
/// touching the cache/`Mutex` it feeds. `None` when there's no session
/// yet (fresh spawn) or the id fails validation — the same guard the
/// network lookup uses, so a malformed id is refused everywhere it
/// could reach a cache or a URL, not just one of them.
pub(super) fn seed_own_session_root(session_id: Option<&str>) -> Option<(String, bool)> {
    let sid = session_id?;
    is_valid_session_id(sid).then(|| (sid.to_owned(), true))
}

/// Authoritative root/child lookup for a session the adapter has no
/// cached answer for (attached mid-life, after both `session.created`
/// and any `session.updated` for it had already passed) — `GET
/// /session/<id>` against opencode's own HTTP API. `None` on any
/// failure (bad id, timeout, network error, non-200, unparseable
/// body): conservative, since guessing risks following the wrong
/// session. Bounded by `SESSION_LOOKUP_TIMEOUT` so a wedged opencode
/// can't stall the SSE read loop indefinitely.
pub(super) async fn lookup_session_is_root(
    client: &reqwest::Client,
    port: u16,
    session_id: &str,
) -> Option<bool> {
    if !is_valid_session_id(session_id) {
        tracing::warn!(
            session_id,
            "opencode_events: refusing malformed session id for root lookup"
        );
        return None;
    }
    let url = format!("http://127.0.0.1:{port}/session/{session_id}");
    let response = tokio::time::timeout(SESSION_LOOKUP_TIMEOUT, client.get(&url).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    Some(
        value
            .get("parentID")
            .and_then(serde_json::Value::as_str)
            .is_none(),
    )
}
