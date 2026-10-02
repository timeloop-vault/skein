//! Proposed edits on element threads (#436): the `review_element_proposals`
//! table and the `Database` methods over it. The JSON is owned by
//! `review_surface::proposal`, which validates it before it gets here.

#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Transaction, params};

use super::Database;

/// A sibling table for the same reason as `review_element_anchors` — no
/// migrations, so a new column on `review_threads` would never reach an
/// existing db. Written once, in the thread's own transaction.
pub(super) fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS review_element_proposals (
                thread_id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                proposal_json TEXT NOT NULL,
                created_ms INTEGER NOT NULL
            )",
        [],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_review_element_proposals_room \
             ON review_element_proposals(room_id)",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Insert a thread's proposal inside the caller's transaction, so the
/// thread, its anchor and its proposal exist together or not at all.
pub(super) fn insert_in_tx(
    tx: &Transaction<'_>,
    thread_id: &str,
    room_id: &str,
    proposal_json: &str,
    now_ms: i64,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO review_element_proposals \
         (thread_id, room_id, proposal_json, created_ms) VALUES (?1, ?2, ?3, ?4)",
        params![thread_id, room_id, proposal_json, now_ms],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a thread's proposal row, if it has one. Called from
/// `element_anchors::delete_thread`, inside the same lock.
pub(super) fn delete_thread(conn: &Connection, thread_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM review_element_proposals WHERE thread_id = ?1",
        params![thread_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Orphan-sweep test seed: one proposal row for the room bound as `?1`.
#[cfg(test)]
pub(super) const SEED_SQL: &str = "INSERT INTO review_element_proposals      (thread_id, room_id, proposal_json, created_ms)      VALUES ('thread-' || ?1, ?1, '{}', 1)";

impl Database {
    /// One thread's proposal JSON, or `None` for a thread without one.
    #[cfg(test)]
    pub fn review_element_proposal(&self, thread_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT proposal_json FROM review_element_proposals WHERE thread_id = ?1",
            params![thread_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// Every `(thread_id, proposal_json)` in the room — the bulk read the
    /// DTO pass stamps threads from.
    pub fn review_element_proposals_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<(String, String)>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, proposal_json FROM review_element_proposals \
                 WHERE room_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
