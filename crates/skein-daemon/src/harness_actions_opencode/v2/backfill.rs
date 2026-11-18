//! opencode v2 history → `harness_actions` rows at attach (#678). The v2
//! counterpart of `super::super::backfill_from_db`: own session only, silent
//! (no live broadcast, no review baseline capture).
//!
//! Dedupe is by `source`, not a timestamp watermark: a live row and its
//! stored twin carry different timestamps, and a source set also fills the
//! holes an SSE reconnect leaves (it replays nothing).

use rusqlite::Connection;
use skein_harness::opencode::v2::store::session_messages;

use super::rows_from_stored;
use crate::db::{Database, NewHarnessAction};
use crate::harness_kind::HarnessKind;

/// Insert the rows of `session_id`'s stored messages that this harness has
/// not recorded yet. Returns how many were inserted; every failure (no
/// opencode.db, a v1-only database, a read error) is logged and reads as 0.
pub fn backfill_v2_from_db(
    session_id: &str,
    harness_id: &str,
    room_id: &str,
    skein_db: &Database,
) -> usize {
    let Some(db_path) = crate::resume::opencode_db_path() else {
        return 0;
    };
    backfill_from_path(&db_path, session_id, harness_id, room_id, skein_db)
}

pub(super) fn backfill_from_path(
    db_path: &std::path::Path,
    session_id: &str,
    harness_id: &str,
    room_id: &str,
    skein_db: &Database,
) -> usize {
    if !db_path.exists() {
        tracing::debug!(harness_id, path = %db_path.display(), "opencode v2 backfill: no opencode.db");
        return 0;
    }
    let conn = match skein_harness::opencode::open_read_only(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(harness_id, error = %e, "opencode v2 backfill: open failed");
            return 0;
        }
    };
    backfill_from_conn(&conn, session_id, harness_id, room_id, skein_db)
}

pub(super) fn backfill_from_conn(
    conn: &Connection,
    session_id: &str,
    harness_id: &str,
    room_id: &str,
    skein_db: &Database,
) -> usize {
    let messages = match session_messages(conn, session_id) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(harness_id, error = %e, "opencode v2 backfill: read failed");
            return 0;
        }
    };
    let known = match skein_db.harness_action_sources(harness_id) {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(harness_id, error = %e, "opencode v2 backfill: source read failed");
            return 0;
        }
    };
    let rows: Vec<NewHarnessAction> = rows_from_stored(&messages)
        .into_iter()
        .filter(|r| r.source.as_ref().is_none_or(|s| !known.contains(s)))
        .map(|r| NewHarnessAction {
            timestamp_ms: r.timestamp_ms,
            kind: r.kind,
            payload: r.payload,
            source: r.source,
        })
        .collect();
    match skein_db.record_harness_actions(
        harness_id,
        room_id,
        Some(HarnessKind::Opencode.as_str()),
        &rows,
    ) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(harness_id, error = %e, "opencode v2 backfill: insert failed");
            0
        }
    }
}
