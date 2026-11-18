//! The per-harness mark behind the context-limit nudge (#689): which
//! `turn_cost` row the last check saw, and whether the next crossing may
//! nudge. The decision lives in `prompt_delivery::context_nudge`.

use rusqlite::{Connection, OptionalExtension, params};

use super::Database;

/// One `context_nudge_marks` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextNudgeMarkRow {
    pub harness_id: String,
    pub room_id: String,
    /// `harness_actions.id` of the newest `turn_cost` row seen.
    pub last_row_id: Option<i64>,
    pub armed: bool,
}

pub(super) fn init_schema(conn: &Connection) -> Result<(), String> {
    // A sibling table, like the other per-room state: no migration
    // machinery exists, so a column on an old table would never appear.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS context_nudge_marks (
            harness_id TEXT PRIMARY KEY,
            room_id TEXT NOT NULL,
            last_row_id INTEGER,
            armed INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    // `sweep_orphans` needs an index leading with `room_id` (#658).
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_context_nudge_marks_room \
         ON context_nudge_marks(room_id)",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

impl Database {
    /// The harness's mark, or `None` when it has never been checked.
    pub fn context_nudge_mark(
        &self,
        harness_id: &str,
    ) -> Result<Option<ContextNudgeMarkRow>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT harness_id, room_id, last_row_id, armed \
             FROM context_nudge_marks WHERE harness_id = ?1",
            params![harness_id],
            |row| {
                Ok(ContextNudgeMarkRow {
                    harness_id: row.get(0)?,
                    room_id: row.get(1)?,
                    last_row_id: row.get(2)?,
                    armed: row.get::<_, i64>(3)? != 0,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// Insert or overwrite the harness's mark.
    pub fn set_context_nudge_mark(
        &self,
        harness_id: &str,
        room_id: &str,
        last_row_id: Option<i64>,
        armed: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO context_nudge_marks (harness_id, room_id, last_row_id, armed) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(harness_id) DO UPDATE SET \
               room_id = excluded.room_id, last_row_id = excluded.last_row_id, \
               armed = excluded.armed",
            params![harness_id, room_id, last_row_id, i64::from(armed)],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The newest `turn_cost` row of one harness as `(row id, context
    /// tokens)`. `None` when there is no row or its kind has no context
    /// size. Same row and same number as `harness_log::context_sizes`.
    pub fn newest_context_tokens(&self, harness_id: &str) -> Result<Option<(i64, u64)>, String> {
        let rows = self.latest_turn_costs(&[harness_id.to_owned()])?;
        Ok(rows.first().and_then(|row| {
            crate::harness_log::context_tokens_of(row).map(|tokens| (row.row_id, tokens))
        }))
    }
}

#[cfg(test)]
mod tests;
