//! Orphan-row sweeping for room-keyed tables. Split out of db.rs (#454).

use super::Database;

/// Every table beyond `sessions`/`sessions_quarantine` that carries a
/// `room_id` column, for `Database::sweep_orphans` (#237). **A new
/// room-keyed table added to `init_schema` must be added here too**,
/// or its orphan rows will never be swept.
pub(super) const ROOM_KEYED_TABLES: &[&str] = &[
    "harness_events",
    "harness_actions",
    "review_baselines",
    "review_baseline_images",
    "review_threads",
    "review_comments",
    "review_viewed",
    "review_settings",
    "review_addressed",
    "review_element_anchors",
    "agent_tokens",
    "review_signoff",
    "harness_messages",
];

impl Database {
    /// Deletes rows in every room-keyed table (`ROOM_KEYED_TABLES`)
    /// whose `room_id` no longer names a live room (issue #237). Closing
    /// a room forever only drops its `sessions` row — see `App.tsx`'s
    /// `deleteRoomForever` — so without this sweep every sibling table
    /// below grows forever.
    ///
    /// Two #167 cautions govern this:
    /// - Callers MUST only run this after a load has already
    ///   succeeded. A failed load must never be allowed to read as "no
    ///   rooms" — that would let a transient sqlite hiccup sweep away
    ///   every room's history.
    /// - A room parked in `sessions_quarantine` (unparseable, kept for
    ///   recovery) keeps its rows too — they are deliberately excluded
    ///   from the orphan check, because the room may come back once its
    ///   blob is fixed by hand.
    ///
    /// One transaction; returns the total number of rows deleted across
    /// every table.
    pub fn sweep_orphans(&self) -> Result<usize, String> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut total = 0usize;
        for table in ROOM_KEYED_TABLES {
            let sql = format!(
                "DELETE FROM {table} WHERE room_id NOT IN (SELECT id FROM sessions) \
                 AND room_id NOT IN (SELECT id FROM sessions_quarantine)"
            );
            total += tx.execute(&sql, []).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(total)
    }
}

#[cfg(test)]
mod tests;
