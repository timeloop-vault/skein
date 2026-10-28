//! The `harness_events` and `harness_actions` logs: one record command
//! and the recent-rows queries for each.

use std::sync::Arc;

use crate::db::{Database, HarnessAction, HarnessEvent, LatestTurnCost};

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
    harness_kind: Option<String>,
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
            harness_kind.as_deref(),
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

/// A harness's current context size: what the model was handed on the
/// latest turn's last API response (#368). Not a cost, not a sum.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessContextSize {
    pub harness_id: String,
    pub tokens: u64,
    /// Epoch ms of the source row. 0 for opencode rows.
    pub timestamp_ms: i64,
}

/// Context tokens of one `turn_cost` row, by the harness kind that wrote
/// it. Only Claude (`usage`, the terminal response's own) and opencode v1
/// (`tokens`, per step) qualify; Hermes writes a cumulative delta and any
/// other kind is not a source. A row from before the kind column (#538)
/// is guessed from its payload shape.
fn context_tokens_of(row: &LatestTurnCost) -> Option<u64> {
    let payload: serde_json::Value = serde_json::from_str(&row.payload).ok()?;
    let claude = || {
        payload
            .get("usage")
            .and_then(skein_harness::claude::context_tokens)
    };
    let opencode = || {
        payload
            .get("tokens")
            .and_then(skein_harness::opencode::context_tokens)
    };
    match row.harness_kind.as_deref() {
        Some("claude") => claude(),
        Some("opencode") => opencode(),
        Some(_) => None,
        None => claude().or_else(opencode),
    }
}

pub(crate) fn context_sizes(
    db: &Database,
    harness_ids: &[String],
) -> Result<Vec<HarnessContextSize>, String> {
    Ok(db
        .latest_turn_costs(harness_ids)?
        .into_iter()
        .filter_map(|row| {
            Some(HarnessContextSize {
                tokens: context_tokens_of(&row)?,
                harness_id: row.harness_id,
                timestamp_ms: row.timestamp_ms,
            })
        })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::fresh_db;

    fn put(db: &Database, h: &str, ts: i64, kind: Option<&str>, payload: &str) {
        db.record_harness_action(h, "r1", ts, "turn_cost", payload, None, kind)
            .unwrap();
    }

    #[test]
    fn sizes_use_newest_row_per_kind_and_skip_unusable_harnesses() {
        let (_dir, db) = fresh_db();
        put(
            &db,
            "c",
            5,
            Some("claude"),
            r#"{"usage":{"input_tokens":1}}"#,
        );
        put(
            &db,
            "c",
            9,
            Some("claude"),
            r#"{"usage":{"input_tokens":2,"cache_read_input_tokens":30,"cache_creation_input_tokens":4}}"#,
        );
        // opencode: timestamps all 0, so id order decides.
        put(&db, "o", 0, Some("opencode"), r#"{"tokens":{"input":900}}"#);
        put(
            &db,
            "o",
            0,
            Some("opencode"),
            r#"{"tokens":{"input":1,"cache":{"read":10,"write":5}}}"#,
        );
        put(
            &db,
            "h",
            7,
            Some("hermes"),
            r#"{"usage":{"input_tokens":50}}"#,
        );
        put(&db, "old", 3, None, r#"{"usage":{"input_tokens":8}}"#);
        db.record_harness_action("c", "r1", 99, "tool_call", "{}", None, Some("claude"))
            .unwrap();
        let ids: Vec<String> = ["c", "o", "h", "old", "none"].map(String::from).to_vec();
        let got = context_sizes(&db, &ids).unwrap();
        let sizes: Vec<_> = got
            .iter()
            .map(|s| (s.harness_id.as_str(), s.tokens, s.timestamp_ms))
            .collect();
        assert_eq!(sizes, vec![("c", 36, 9), ("o", 16, 0), ("old", 8, 3)]);
    }
}
