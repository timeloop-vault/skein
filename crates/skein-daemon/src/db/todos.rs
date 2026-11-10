//! The global manual todo list (#335). Same shape as rooms: one row per
//! todo, the whole todo as an opaque camelCase JSON blob, saved wholesale.
//!
//! Rust deliberately does not model a todo's fields — the TS shape owns
//! them, and storing `serde_json::Value` means keys this build does not
//! know about survive a load/save round trip.

use rusqlite::{Connection, params};
use serde_json::Value;

use super::Database;

pub(super) fn init_schema(conn: &Connection) -> Result<(), String> {
    // `ord` preserves list order across save/load.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS global_todos (
            id TEXT PRIMARY KEY,
            ord INTEGER NOT NULL,
            data TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

impl Database {
    /// Every stored todo, in saved order. A row whose blob no longer
    /// parses is skipped with a warning rather than failing the load.
    pub fn load_global_todos(&self) -> Result<Vec<Value>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, data FROM global_todos ORDER BY ord ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row.map_err(|e| e.to_string())?;
            match serde_json::from_str::<Value>(&data) {
                Ok(v) => out.push(v),
                Err(e) => tracing::warn!(id = %id, error = %e, "skipping unparseable todo row"),
            }
        }
        Ok(out)
    }

    /// Replace the list wholesale in one transaction. An empty list is a
    /// legitimate save (the user cleared their todos). Each todo needs a
    /// string `id`; one without is an error and nothing is written.
    pub fn save_global_todos(&self, todos: &[Value]) -> Result<(), String> {
        let mut rows = Vec::with_capacity(todos.len());
        for t in todos {
            let id = t
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "todo is missing a string id".to_string())?;
            rows.push((id, serde_json::to_string(t).map_err(|e| e.to_string())?));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM global_todos", [])
            .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare("INSERT OR REPLACE INTO global_todos (id, ord, data) VALUES (?1, ?2, ?3)")
                .map_err(|e| e.to_string())?;
            for (ord, (id, data)) in rows.iter().enumerate() {
                stmt.execute(params![id, i64::try_from(ord).unwrap_or(i64::MAX), data])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Seq-aware save, same reasoning as `save_all_seq` (#171): the
    /// frontend fires saves un-awaited, so a stale one that reaches the
    /// lock after a newer one committed is dropped.
    pub fn save_global_todos_seq(&self, todos: &[Value], seq: u64) -> Result<(), String> {
        let mut last = self.last_saved_todos_seq.lock();
        if seq < *last {
            return Ok(());
        }
        self.save_global_todos(todos)?;
        *last = seq;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::fresh_db;

    #[test]
    fn round_trip_preserves_order_and_unknown_keys() {
        let (_dir, db) = fresh_db();
        let todos = vec![
            json!({"id": "b", "text": "second-created", "futureKey": {"x": [1, 2]}}),
            json!({"id": "a", "text": "other", "done": true}),
        ];
        db.save_global_todos(&todos).unwrap();
        assert_eq!(db.load_global_todos().unwrap(), todos);
    }

    #[test]
    fn empty_save_clears() {
        let (_dir, db) = fresh_db();
        db.save_global_todos(&[json!({"id": "a"})]).unwrap();
        db.save_global_todos(&[]).unwrap();
        assert_eq!(
            db.load_global_todos().unwrap(),
            Vec::<serde_json::Value>::new()
        );
    }

    #[test]
    fn save_replaces_and_reorders() {
        let (_dir, db) = fresh_db();
        db.save_global_todos(&[json!({"id": "a"}), json!({"id": "b"})])
            .unwrap();
        db.save_global_todos(&[json!({"id": "b"}), json!({"id": "c"})])
            .unwrap();
        assert_eq!(
            db.load_global_todos().unwrap(),
            vec![json!({"id": "b"}), json!({"id": "c"})]
        );
    }

    #[test]
    fn missing_id_writes_nothing() {
        let (_dir, db) = fresh_db();
        db.save_global_todos(&[json!({"id": "a"})]).unwrap();
        assert!(db.save_global_todos(&[json!({"text": "no id"})]).is_err());
        assert_eq!(db.load_global_todos().unwrap(), vec![json!({"id": "a"})]);
    }

    #[test]
    fn unparseable_row_is_skipped() {
        let (_dir, db) = fresh_db();
        db.save_global_todos(&[json!({"id": "a"})]).unwrap();
        db.conn
            .lock()
            .execute(
                "INSERT INTO global_todos (id, ord, data) VALUES ('bad', 5, '{nope')",
                [],
            )
            .unwrap();
        assert_eq!(db.load_global_todos().unwrap(), vec![json!({"id": "a"})]);
    }

    #[test]
    fn stale_seq_is_dropped() {
        let (_dir, db) = fresh_db();
        db.save_global_todos_seq(&[json!({"id": "new"})], 2)
            .unwrap();
        db.save_global_todos_seq(&[json!({"id": "old"})], 1)
            .unwrap();
        assert_eq!(db.load_global_todos().unwrap(), vec![json!({"id": "new"})]);
    }
}
