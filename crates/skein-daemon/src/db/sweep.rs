//! Orphan-row sweeping for room-keyed tables. Split out of db.rs (#454).

use super::Database;
use rusqlite::OptionalExtension;
use std::collections::HashSet;

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
    "review_element_proposals",
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
    /// Index-only (#658): the live ids (`sessions` plus
    /// `sessions_quarantine`) are read into a set, each table's distinct
    /// `room_id`s are enumerated by a loose index scan (one index seek per
    /// distinct room, not a pass over every row), and only ids outside the
    /// set are deleted, again by index. Every table in `ROOM_KEYED_TABLES`
    /// therefore needs an index leading with `room_id`.
    ///
    /// One transaction; returns the total number of rows deleted across
    /// every table.
    pub fn sweep_orphans(&self) -> Result<usize, String> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut live: HashSet<String> = HashSet::new();
        for sql in [
            "SELECT id FROM sessions",
            "SELECT id FROM sessions_quarantine",
        ] {
            let mut stmt = tx.prepare(sql).map_err(|e| e.to_string())?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for id in ids {
                live.insert(id.map_err(|e| e.to_string())?);
            }
        }
        let mut total = 0usize;
        for table in ROOM_KEYED_TABLES {
            let mut orphans = Vec::new();
            {
                let mut first = tx
                    .prepare(&first_room_sql(table))
                    .map_err(|e| e.to_string())?;
                let mut next = tx
                    .prepare(&next_room_sql(table))
                    .map_err(|e| e.to_string())?;
                let mut cur: Option<String> = first
                    .query_row([], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?;
                while let Some(id) = cur {
                    cur = next
                        .query_row([&id], |r| r.get(0))
                        .optional()
                        .map_err(|e| e.to_string())?;
                    if !live.contains(&id) {
                        orphans.push(id);
                    }
                }
            }
            let mut del = tx
                .prepare(&delete_room_sql(table))
                .map_err(|e| e.to_string())?;
            for id in orphans {
                total += del.execute([&id]).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(total)
    }
}

/// Loose-scan step 1: the smallest `room_id` in the table.
pub(super) fn first_room_sql(table: &str) -> String {
    format!("SELECT room_id FROM {table} WHERE room_id IS NOT NULL ORDER BY room_id LIMIT 1")
}

/// Loose-scan step n: the next distinct `room_id` after `?1`.
pub(super) fn next_room_sql(table: &str) -> String {
    format!("SELECT room_id FROM {table} WHERE room_id > ?1 ORDER BY room_id LIMIT 1")
}

pub(super) fn delete_room_sql(table: &str) -> String {
    format!("DELETE FROM {table} WHERE room_id = ?1")
}

#[cfg(test)]
mod tests;
