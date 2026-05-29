//! Live `harness_actions` broadcast. Issue #80 D1.
//!
//! When an adapter persists a *live* action row (i.e. one that landed
//! after Skein attached — not a backfilled history row), it emits a
//! Tauri event so the Live Context pane can append it without
//! re-querying. Backfilled rows are NOT broadcast: the frontend loads
//! those once via `db_recent_harness_actions_by_room` on mount.
//!
//! The event is global (`EVENT_NAME`); the payload carries `roomId`
//! so the frontend filters to the active room. Payload field names are
//! camelCase to match the rest of the Tauri/serde boundary.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Global event name. Frontend listens once and filters by `roomId`.
pub const EVENT_NAME: &str = "harness-action";

/// Broadcast payload for one live action row. Mirrors the columns of
/// `harness_actions` plus the freshly-inserted row `id` so the
/// frontend can de-dupe against its initial query.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessActionEvent<'a> {
    pub id: i64,
    pub harness_id: &'a str,
    pub room_id: &'a str,
    pub timestamp_ms: i64,
    pub kind: &'a str,
    pub payload: &'a str,
    pub source: Option<&'a str>,
}

/// Emit one live action to the frontend. Best-effort: a failed emit
/// (no window yet, IPC torn down) is logged at trace and dropped —
/// the row is already persisted, so the worst case is the pane shows
/// it on next mount instead of live.
#[allow(clippy::too_many_arguments)]
pub fn emit(
    app: &AppHandle,
    id: i64,
    harness_id: &str,
    room_id: &str,
    timestamp_ms: i64,
    kind: &str,
    payload: &str,
    source: Option<&str>,
) {
    let event = HarnessActionEvent {
        id,
        harness_id,
        room_id,
        timestamp_ms,
        kind,
        payload,
        source,
    };
    if let Err(e) = app.emit(EVENT_NAME, event) {
        tracing::trace!(error = %e, "harness_action_event: emit failed");
    }
}
