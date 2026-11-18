//! Which of opencode's session tables hold a session (#679).
//!
//! v1 keeps sessions in `session`, v2 in `session_v2`; both have `id` and a
//! nullable `time_archived`, and both live in the same `opencode.db`. A
//! database is in one of three states: v1 only (no `session_v2`), fresh v2
//! (no `session`), or migrated (both). v2 migrates v1 sessions once, so a
//! session a v1 binary creates afterwards is in `session` only. That is why
//! the answer is per table rather than one bool.

use rusqlite::Connection;

use super::Result;

/// Where a non-archived session row was found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionPresence {
    /// In the v1 `session` table.
    pub v1: bool,
    /// In the v2 `session_v2` table.
    pub v2: bool,
}

impl SessionPresence {
    /// Present in either table.
    #[must_use]
    pub fn any(self) -> bool {
        self.v1 || self.v2
    }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    Ok(stmt.exists([table])?)
}

fn live_in(conn: &Connection, table: &str, id: &str) -> Result<bool> {
    if !table_exists(conn, table)? {
        return Ok(false);
    }
    let sql = format!("SELECT 1 FROM {table} WHERE id = ?1 AND time_archived IS NULL LIMIT 1");
    let mut stmt = conn.prepare(&sql)?;
    Ok(stmt.exists([id])?)
}

/// Is there a non-archived session with this exact id, in each table? A
/// table that does not exist reads as absent, never as an error.
///
/// # Errors
/// A sqlite failure other than a missing table.
pub fn session_presence(conn: &Connection, id: &str) -> Result<SessionPresence> {
    Ok(SessionPresence {
        v1: live_in(conn, "session", id)?,
        v2: live_in(conn, "session_v2", id)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(v1: bool, v2: bool) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        if v1 {
            conn.execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, time_archived INTEGER);",
            )
            .unwrap();
        }
        if v2 {
            conn.execute_batch(
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_archived INTEGER);",
            )
            .unwrap();
        }
        conn
    }

    fn put(conn: &Connection, table: &str, id: &str, archived: Option<i64>) {
        conn.execute(
            &format!("INSERT INTO {table} (id, time_archived) VALUES (?1, ?2)"),
            rusqlite::params![id, archived],
        )
        .unwrap();
    }

    fn p(v1: bool, v2: bool) -> SessionPresence {
        SessionPresence { v1, v2 }
    }

    #[test]
    fn v1_only_database() {
        let conn = db(true, false);
        put(&conn, "session", "ses_a", None);
        assert_eq!(session_presence(&conn, "ses_a").unwrap(), p(true, false));
    }

    #[test]
    fn fresh_v2_database_has_no_session_table() {
        let conn = db(false, true);
        put(&conn, "session_v2", "ses_a", None);
        assert_eq!(session_presence(&conn, "ses_a").unwrap(), p(false, true));
        assert_eq!(session_presence(&conn, "ses_b").unwrap(), p(false, false));
    }

    #[test]
    fn migrated_database_with_the_id_in_both() {
        let conn = db(true, true);
        put(&conn, "session", "ses_a", None);
        put(&conn, "session_v2", "ses_a", None);
        let got = session_presence(&conn, "ses_a").unwrap();
        assert_eq!(got, p(true, true));
        assert!(got.any());
    }

    #[test]
    fn session_created_by_v1_after_migration_is_in_v1_only() {
        let conn = db(true, true);
        put(&conn, "session", "ses_late", None);
        assert_eq!(session_presence(&conn, "ses_late").unwrap(), p(true, false));
    }

    #[test]
    fn archived_rows_read_as_absent() {
        let conn = db(true, true);
        put(&conn, "session", "ses_a", Some(1));
        put(&conn, "session_v2", "ses_a", Some(2));
        let got = session_presence(&conn, "ses_a").unwrap();
        assert_eq!(got, p(false, false));
        assert!(!got.any());
    }

    #[test]
    fn unknown_id_and_empty_database() {
        let conn = db(true, true);
        put(&conn, "session", "ses_a", None);
        assert_eq!(session_presence(&conn, "nope").unwrap(), p(false, false));
        assert_eq!(
            session_presence(&db(false, false), "x").unwrap(),
            p(false, false)
        );
    }
}
