//! The `harness_events` and `harness_actions` logs: one record command
//! and the recent-rows queries for each.

use std::sync::Arc;

use crate::db::{Database, HarnessAction, HarnessEvent};

/// Append one row to the `harness_events` log. Epic #50 L6.
///
/// Called by the frontend's transition listener — once per real
/// phase change. Fire-and-forget from the TS side; we still surface
/// errors as `String` so the caller can console.warn if something
/// goes wrong (most likely "disk full" or a corrupted DB; nothing
/// actionable from the user's perspective beyond seeing a log).
///
/// `source` is reserved for L7 attribution ("which adapter event
/// drove this transition"). For v1 the frontend passes `None`.
///
/// Async: an sqlite insert — off the main thread (#171).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn db_record_harness_event(
    harness_id: String,
    room_id: String,
    from_phase: String,
    to_phase: String,
    timestamp_ms: i64,
    has_user_input: bool,
    source: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.record_harness_event(
            &harness_id,
            &room_id,
            &from_phase,
            &to_phase,
            timestamp_ms,
            has_user_input,
            source.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent events for a single harness. Newest-first. Epic #50 L6.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_recent_harness_events_by_harness(
    harness_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessEvent>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_events_by_harness(&harness_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent events across every harness in a room. Newest-first.
/// Epic #50 L6 — foundation for the L7 activity feed.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_recent_harness_events_by_room(
    room_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessEvent>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_events_by_room(&room_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Append one row to the `harness_actions` log. Issue #80.
///
/// Adapters call this per extracted action. `kind` should be one of
/// the `action_kind` constants. `payload` is an opaque JSON string —
/// the canonical shape per kind is documented in the design brief.
/// `source` carries the adapter event id (mirrors the L7a `source`
/// column on `harness_events`).
///
/// Async: an sqlite insert, called once per extracted action — off
/// the main thread so an agent's tool-call storm can't queue behind
/// it (#171).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn db_record_harness_action(
    harness_id: String,
    room_id: String,
    timestamp_ms: i64,
    kind: String,
    payload: String,
    source: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.record_harness_action(
            &harness_id,
            &room_id,
            timestamp_ms,
            &kind,
            &payload,
            source.as_deref(),
        )
        .map(drop)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent actions for a single harness. Newest-first.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_recent_harness_actions_by_harness(
    harness_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessAction>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_actions_by_harness(&harness_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent actions across every harness in a room. Newest-first.
/// Backs the Activity card.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_recent_harness_actions_by_room(
    room_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessAction>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_actions_by_room(&room_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent actions of a single `kind` in a room. Backs the Plan
/// card (`kind = "plan_change"`) and other per-kind surfaces.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_recent_harness_actions_by_room_and_kind(
    room_id: String,
    kind: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessAction>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_actions_by_room_and_kind(&room_id, &kind, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}
