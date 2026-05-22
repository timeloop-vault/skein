//! Room persistence — sqlite, one row per room, JSON blob.
//!
//! The schema is deliberately minimal: we don't query individual fields
//! today, and storing the Room as a JSON blob lets the TS shape evolve
//! without schema migrations. We pay for that with no SQL-level queries
//! over fields like `repo` or `branch` — when a phase needs that, we can
//! split the blob into proper columns.
//!
//! Save semantics: `save_all` is a wipe + re-insert inside one transaction.
//! Cheap at prototype scale (a dozen rows of <1 KB each) and frees the
//! frontend from tracking which rooms changed.
//!
//! The sqlite table is still called `sessions` for legacy reasons —
//! pre-chapter-6 the Skein concept was called "session" and renaming
//! the table would need a migration for cosmetic gain. JSON blobs
//! inside don't carry the table name.
//!
//! Epic #50 L6 adds a second table — `harness_events` — that keeps an
//! append-only log of every harness phase transition. Foundation for
//! L7 (cross-harness activity feed) and a longer-term "since last
//! visit" surface. The TS side writes per transition via
//! `db_record_harness_event`; reads come back via the `recent_*`
//! query commands.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// Mirrors the TS Harness interface. Field renames keep the wire format
/// camelCase to match what the frontend serializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Harness {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub status: String,
    pub model: String,
    pub tokens: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub live: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cmd: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    /// Conversation id from the underlying tool. See chapter-5-plan.md
    /// for how it gets populated; Skein only round-trips it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
    /// Count of attention-worthy transitions accumulated for this
    /// harness while the user wasn't viewing it. Cleared when the
    /// harness becomes the active harness in the active room.
    /// Persisted so the badge survives Skein restarts. Epic #50 L5a.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pending_notifications: Option<i64>,
}

/// One row in the `harness_events` append-only log. Epic #50 L6.
///
/// Stored fields are intentionally minimal — phase strings come from
/// the TS `ActivityPhase` union (`spawning` / `running` / `idle` /
/// `waiting` / `exited`) but we don't enforce a check constraint
/// here; future phases would just become new string values. The
/// `source` field is free-form text for v1 (e.g. `"l2c1-claude"`,
/// `"l2a-idle"`, `"pty-exit"`), reserved for L7 attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessEvent {
    pub id: i64,
    pub harness_id: String,
    pub room_id: String,
    pub from_phase: String,
    pub to_phase: String,
    /// Epoch milliseconds.
    pub timestamp_ms: i64,
    pub has_user_input: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
}

/// Mirrors the TS Room interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub task: String,
    pub status: String,
    pub badge: i64,
    pub harnesses: Vec<Harness>,
    pub active_harness_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    /// `None` for non-git rooms (chapter 6 phase 3). Present together
    /// with `branch` when the room was created from a git repo.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<String>,
    /// Close timestamp (epoch ms). `None` = active; `Some` = archived
    /// (chapter 6 phase 2). Skein round-trips this; the frontend reads
    /// it for tab-strip filtering and the reopen modal.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archived: Option<i64>,
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Idempotent schema setup. Each table uses `IF NOT EXISTS`; new
    /// tables just get added here without a separate migration step.
    /// At prototype scale this is sufficient — once columns need to
    /// be altered (vs added) we'll need a version table.
    fn init_schema(conn: &Connection) -> Result<(), String> {
        // `created_at` preserves room order across save/load (frontend
        // appends new rooms, we want the same order back).
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Epic #50 L6: append-only harness activity log. INTEGER
        // PRIMARY KEY gives us a monotonic id (= insertion order)
        // for free, useful for paging without relying on
        // timestamp_ms which can collide on a fast machine.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS harness_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness_id TEXT NOT NULL,
                room_id TEXT NOT NULL,
                from_phase TEXT NOT NULL,
                to_phase TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                has_user_input INTEGER NOT NULL,
                source TEXT
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        // Both indices are time-ordered for the common
        // `WHERE ... AND timestamp_ms > ? ORDER BY timestamp_ms DESC`
        // query. sqlite uses the leading column for filter +
        // ordering simultaneously.
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_events_harness \
             ON harness_events(harness_id, timestamp_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_events_room \
             ON harness_events(room_id, timestamp_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<Room>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT data FROM sessions ORDER BY created_at, id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let data = row.map_err(|e| e.to_string())?;
            let r: Room = serde_json::from_str(&data).map_err(|e| e.to_string())?;
            out.push(r);
        }
        Ok(out)
    }

    pub fn save_all(&self, rooms: &[Room]) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM sessions", [])
            .map_err(|e| e.to_string())?;
        let base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX));
        for (i, r) in rooms.iter().enumerate() {
            let json = serde_json::to_string(r).map_err(|e| e.to_string())?;
            // base + i preserves insertion order on reload, even when
            // multiple saves happen within the same microsecond.
            let created_at = base.saturating_add(i64::try_from(i).unwrap_or(0));
            tx.execute(
                "INSERT INTO sessions (id, data, created_at) VALUES (?1, ?2, ?3)",
                params![r.id, json, created_at],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── harness event log (epic #50 L6) ──────────────────────────

    /// Append one row to `harness_events`. The TS side calls this
    /// per real phase transition. We don't dedupe or validate phase
    /// strings here — the activity store is the source of truth and
    /// will only emit real transitions.
    #[allow(clippy::too_many_arguments)]
    pub fn record_harness_event(
        &self,
        harness_id: &str,
        room_id: &str,
        from_phase: &str,
        to_phase: &str,
        timestamp_ms: i64,
        has_user_input: bool,
        source: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO harness_events \
             (harness_id, room_id, from_phase, to_phase, timestamp_ms, has_user_input, source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                harness_id,
                room_id,
                from_phase,
                to_phase,
                timestamp_ms,
                i64::from(has_user_input),
                source,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Most recent events for a single harness with `timestamp_ms > since_ms`.
    /// Ordered newest-first. `limit` caps the result; the caller picks a
    /// sensible bound (a hundred or two is plenty for a "what changed
    /// while I was away" surface).
    pub fn recent_harness_events_by_harness(
        &self,
        harness_id: &str,
        since_ms: i64,
        limit: i64,
    ) -> Result<Vec<HarnessEvent>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, harness_id, room_id, from_phase, to_phase, \
                        timestamp_ms, has_user_input, source \
                 FROM harness_events \
                 WHERE harness_id = ?1 AND timestamp_ms > ?2 \
                 ORDER BY timestamp_ms DESC, id DESC \
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![harness_id, since_ms, limit], row_to_event)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Most recent events across every harness in a room. Same shape
    /// as the per-harness query but useful for the L7 activity feed
    /// once it lands.
    pub fn recent_harness_events_by_room(
        &self,
        room_id: &str,
        since_ms: i64,
        limit: i64,
    ) -> Result<Vec<HarnessEvent>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, harness_id, room_id, from_phase, to_phase, \
                        timestamp_ms, has_user_input, source \
                 FROM harness_events \
                 WHERE room_id = ?1 AND timestamp_ms > ?2 \
                 ORDER BY timestamp_ms DESC, id DESC \
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id, since_ms, limit], row_to_event)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessEvent> {
    Ok(HarnessEvent {
        id: row.get(0)?,
        harness_id: row.get(1)?,
        room_id: row.get(2)?,
        from_phase: row.get(3)?,
        to_phase: row.get(4)?,
        timestamp_ms: row.get(5)?,
        has_user_input: row.get::<_, i64>(6)? != 0,
        source: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn record_then_query_by_harness_returns_event() {
        let (_dir, db) = fresh_db();
        db.record_harness_event("h1", "r1", "running", "waiting", 1_000, true, Some("l2c1"))
            .unwrap();
        let events = db.recent_harness_events_by_harness("h1", 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.harness_id, "h1");
        assert_eq!(e.room_id, "r1");
        assert_eq!(e.from_phase, "running");
        assert_eq!(e.to_phase, "waiting");
        assert_eq!(e.timestamp_ms, 1_000);
        assert!(e.has_user_input);
        assert_eq!(e.source.as_deref(), Some("l2c1"));
    }

    #[test]
    fn query_excludes_events_at_or_before_since_ms() {
        let (_dir, db) = fresh_db();
        for ts in [100, 200, 300, 400] {
            db.record_harness_event("h1", "r1", "running", "idle", ts, false, None)
                .unwrap();
        }
        let events = db.recent_harness_events_by_harness("h1", 200, 10).unwrap();
        // Strict > since_ms — caller passes the last-seen timestamp
        // and wants only events newer than that.
        let timestamps: Vec<i64> = events.iter().map(|e| e.timestamp_ms).collect();
        assert_eq!(timestamps, vec![400, 300]);
    }

    #[test]
    fn query_is_scoped_by_harness_id() {
        let (_dir, db) = fresh_db();
        db.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
            .unwrap();
        db.record_harness_event("h2", "r1", "running", "idle", 200, false, None)
            .unwrap();
        db.record_harness_event("h1", "r1", "idle", "running", 300, false, None)
            .unwrap();
        let h1 = db.recent_harness_events_by_harness("h1", 0, 10).unwrap();
        assert_eq!(h1.len(), 2);
        assert!(h1.iter().all(|e| e.harness_id == "h1"));
    }

    #[test]
    fn query_by_room_returns_all_harnesses_in_that_room() {
        let (_dir, db) = fresh_db();
        db.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
            .unwrap();
        db.record_harness_event("h2", "r1", "running", "idle", 200, false, None)
            .unwrap();
        db.record_harness_event("h3", "r2", "running", "idle", 300, false, None)
            .unwrap();
        let r1 = db.recent_harness_events_by_room("r1", 0, 10).unwrap();
        assert_eq!(r1.len(), 2);
        assert!(r1.iter().all(|e| e.room_id == "r1"));
    }

    #[test]
    fn query_respects_limit() {
        let (_dir, db) = fresh_db();
        for ts in 0..50 {
            db.record_harness_event("h1", "r1", "running", "idle", ts, false, None)
                .unwrap();
        }
        let events = db.recent_harness_events_by_harness("h1", -1, 5).unwrap();
        assert_eq!(events.len(), 5);
        // Newest first — last ts is the largest.
        assert_eq!(events[0].timestamp_ms, 49);
        assert_eq!(events[4].timestamp_ms, 45);
    }

    #[test]
    fn has_user_input_round_trips_correctly() {
        let (_dir, db) = fresh_db();
        db.record_harness_event("h1", "r1", "spawning", "running", 100, false, None)
            .unwrap();
        db.record_harness_event("h2", "r1", "running", "waiting", 200, true, None)
            .unwrap();
        let events = db.recent_harness_events_by_room("r1", 0, 10).unwrap();
        // Newest-first ordering means h2 comes back first.
        assert!(events[0].has_user_input);
        assert!(!events[1].has_user_input);
    }

    #[test]
    fn null_source_round_trips_as_none() {
        let (_dir, db) = fresh_db();
        db.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
            .unwrap();
        let events = db.recent_harness_events_by_harness("h1", 0, 10).unwrap();
        assert!(events[0].source.is_none());
    }

    #[test]
    fn schema_is_idempotent_across_open_calls() {
        // Open the same path twice — the second `Database::open`
        // must not fail on `CREATE TABLE IF NOT EXISTS`.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let _db1 = Database::open(&path).unwrap();
        let db2 = Database::open(&path).unwrap();
        db2.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
            .unwrap();
        assert_eq!(
            db2.recent_harness_events_by_harness("h1", 0, 10)
                .unwrap()
                .len(),
            1
        );
    }
}
