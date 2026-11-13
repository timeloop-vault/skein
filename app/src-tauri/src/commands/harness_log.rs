//! The `harness_events` and `harness_actions` logs: the recent-rows
//! queries for each. The daemon writes `harness_events` itself.

use std::sync::Arc;

use crate::db::{Database, HarnessAction, HarnessEvent};
use skein_daemon::harness_log::{HarnessContextSize, context_sizes};

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

/// Current context size per harness, from each one's newest `turn_cost`
/// row. Harnesses with no row, or none we can read, are omitted (#368).
///
/// Async: sqlite queries — off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_harness_context_sizes(
    harness_ids: Vec<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessContextSize>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || context_sizes(&db, &harness_ids))
        .await
        .map_err(|e| e.to_string())?
}
