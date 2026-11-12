//! Per-row room persistence (#729): write only the rows a mutation touched,
//! never wipe and re-insert the table the way `save_all` does.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;

use super::Database;
use super::rooms::Room;

impl Database {
    /// Persist a batch of room changes in ONE transaction: upsert each room
    /// (a brand-new row sorts after every existing row, an existing row keeps
    /// its position), delete each removed id, then, if `order` is given,
    /// rewrite `created_at` so `load_all` returns the listed ids first in the
    /// listed order and every other row after them in its old relative order.
    /// Ids in `order` that have no row are ignored.
    ///
    /// Does not touch the `loaded_ok` guard: the caller enforces
    /// load-before-write. Rows not named in the call are left untouched
    /// unless `order` is given, which rewrites only their `created_at`.
    pub fn write_room_rows(
        &self,
        upserts: &[Room],
        removed: &[String],
        order: Option<&[String]>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX));
        for r in upserts {
            let json = serde_json::to_string(r).map_err(|e| e.to_string())?;
            let max: Option<i64> = tx
                .query_row("SELECT MAX(created_at) FROM sessions", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            let next = max.map_or(base, |m| m.saturating_add(1).max(base));
            tx.execute(
                "INSERT INTO sessions (id, data, created_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                params![r.id, json, next],
            )
            .map_err(|e| e.to_string())?;
        }
        for id in removed {
            tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        if let Some(order) = order {
            let existing: Vec<String> = {
                let mut stmt = tx
                    .prepare("SELECT id FROM sessions ORDER BY created_at, id")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| e.to_string())?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?
            };
            let present: HashSet<&str> = existing.iter().map(String::as_str).collect();
            let mut seen = HashSet::new();
            let mut final_order: Vec<&str> = order
                .iter()
                .map(String::as_str)
                .filter(|id| present.contains(id) && seen.insert(*id))
                .collect();
            final_order.extend(
                existing
                    .iter()
                    .map(String::as_str)
                    .filter(|id| !seen.contains(id)),
            );
            for (i, id) in final_order.iter().enumerate() {
                let created_at = base.saturating_add(i64::try_from(i).unwrap_or(0));
                tx.execute(
                    "UPDATE sessions SET created_at = ?1 WHERE id = ?2",
                    params![created_at, id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
