//! Element-thread anchors (#434): the `review_element_anchors` table, its
//! row type and the `Database` methods over it. Split out of `db.rs` (#454).

use rusqlite::{Connection, OptionalExtension, params};

use super::{Database, ReviewThreadRow, element_proposals};

/// #434: what an element-scoped thread is anchored to. A sibling
/// table for the same reason as `review_addressed` — no
/// migrations, so a new column on `review_threads` would never
/// reach an existing db. `anchor_json` is the picker's evidence
/// and is written once; `last_seen_json` is what the design pane
/// last computed and is the only part ever updated.
pub(super) fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS review_element_anchors (
                thread_id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                file_path TEXT NOT NULL,
                anchor_json TEXT NOT NULL,
                last_seen_json TEXT,
                updated_ms INTEGER NOT NULL
            )",
        [],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_review_element_anchors_room \
             ON review_element_anchors(room_id, file_path)",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a thread row and its anchor row, if it has one. Called from
/// the thread delete paths in `db.rs`, inside their own lock.
pub(super) fn delete_thread(conn: &Connection, thread_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM review_element_anchors WHERE thread_id = ?1",
        params![thread_id],
    )
    .map_err(|e| e.to_string())?;
    element_proposals::delete_thread(conn, thread_id)?;
    conn.execute(
        "DELETE FROM review_threads WHERE id = ?1",
        params![thread_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Orphan-sweep test seed: one anchor row for the room bound as `?1`.
#[cfg(test)]
pub(super) const SEED_SQL: &str = "INSERT INTO review_element_anchors      (thread_id, room_id, file_path, anchor_json, last_seen_json, updated_ms)      VALUES ('thread-' || ?1, ?1, 'a.html', '{}', NULL, 1)";

/// One row of `review_element_anchors` (#434). Both JSON columns are
/// owned by `review_surface::element`, which is what knows their shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewElementAnchorRow {
    pub thread_id: String,
    pub room_id: String,
    /// The entry HTML — same value as the thread row's `file_path`.
    pub file_path: String,
    /// The picker's evidence. Written once, never updated.
    pub anchor_json: String,
    /// What the design pane last computed, with its content stamp.
    pub last_seen_json: Option<String>,
    pub updated_ms: i64,
}

fn row_to_element_anchor(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewElementAnchorRow> {
    Ok(ReviewElementAnchorRow {
        thread_id: row.get(0)?,
        room_id: row.get(1)?,
        file_path: row.get(2)?,
        anchor_json: row.get(3)?,
        last_seen_json: row.get(4)?,
        updated_ms: row.get(5)?,
    })
}

impl Database {
    /// Create an element thread and its anchor row together (#434): a
    /// thread with no anchor row would render as an element comment with
    /// nothing to point at, so either both exist or neither does.
    #[cfg(test)]
    pub fn insert_review_element_thread(
        &self,
        t: &ReviewThreadRow,
        a: &ReviewElementAnchorRow,
    ) -> Result<(), String> {
        self.insert_review_element_thread_proposal(t, a, None)
    }

    /// [`Database::insert_review_element_thread`], plus the thread's
    /// validated proposal JSON (#436) in the same transaction.
    pub fn insert_review_element_thread_proposal(
        &self,
        t: &ReviewThreadRow,
        a: &ReviewElementAnchorRow,
        proposal_json: Option<&str>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO review_threads \
             (id, room_id, scope, file_path, commit_sha, side, line_start, line_end, \
              anchor_hash, anchor_lines, resolved_ms, created_ms, updated_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                t.id,
                t.room_id,
                t.scope,
                t.file_path,
                t.commit_sha,
                t.side,
                t.line_start,
                t.line_end,
                t.anchor_hash,
                t.anchor_lines,
                t.resolved_ms,
                t.created_ms,
                t.updated_ms,
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO review_element_anchors \
             (thread_id, room_id, file_path, anchor_json, last_seen_json, updated_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                a.thread_id,
                a.room_id,
                a.file_path,
                a.anchor_json,
                a.last_seen_json,
                a.updated_ms,
            ],
        )
        .map_err(|e| e.to_string())?;
        if let Some(json) = proposal_json {
            element_proposals::insert_in_tx(&tx, &a.thread_id, &a.room_id, json, t.created_ms)?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// One thread's element anchor, or `None` for a non-element thread.
    pub fn review_element_anchor(
        &self,
        thread_id: &str,
    ) -> Result<Option<ReviewElementAnchorRow>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT thread_id, room_id, file_path, anchor_json, last_seen_json, updated_ms \
             FROM review_element_anchors WHERE thread_id = ?1",
            params![thread_id],
            row_to_element_anchor,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// Every element anchor in the room — the bulk read the DTO pass
    /// stamps threads from, like [`Database::addressed_for_room`].
    pub fn review_element_anchors_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReviewElementAnchorRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, room_id, file_path, anchor_json, last_seen_json, updated_ms \
                 FROM review_element_anchors WHERE room_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], row_to_element_anchor)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Record what the design pane last computed for an element thread.
    /// Touches `last_seen_json` and `updated_ms` only — `anchor_json` is
    /// the comment's evidence and is never rewritten. Returns `false`
    /// when the thread has no anchor row.
    pub fn set_review_element_last_seen(
        &self,
        thread_id: &str,
        last_seen_json: &str,
        now_ms: i64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE review_element_anchors SET last_seen_json = ?2, updated_ms = ?3 \
             WHERE thread_id = ?1",
            params![thread_id, last_seen_json, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.changes() > 0)
    }
}
