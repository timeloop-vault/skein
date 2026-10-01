//! The per-harness mailbox (`harness_messages`). Split out of db.rs (#454).

use rusqlite::{OptionalExtension, named_params, params};

use super::Database;

/// One row of `harness_messages` (issue #327).
///
/// `room_id`/`harness_id` name the *recipient* — plain names, not
/// `to_*`, so this table is keyed the same way every other room-scoped
/// table is (see `ROOM_KEYED_TABLES`). `from_room_id`/`from_harness_id`
/// are the sender, for attribution only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessMessageRow {
    pub id: String,
    pub room_id: String,
    pub harness_id: String,
    pub from_room_id: String,
    /// `None` when the sender had no `X-Skein-Harness` — attribution
    /// only, same as a review reply's `author_id`.
    pub from_harness_id: Option<String>,
    pub body: String,
    pub created_ms: i64,
    pub read_ms: Option<i64>,
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessMessageRow> {
    Ok(HarnessMessageRow {
        id: row.get(0)?,
        room_id: row.get(1)?,
        harness_id: row.get(2)?,
        from_room_id: row.get(3)?,
        from_harness_id: row.get(4)?,
        body: row.get(5)?,
        created_ms: row.get(6)?,
        read_ms: row.get(7)?,
    })
}

impl Database {
    pub fn insert_harness_message(&self, m: &HarnessMessageRow) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO harness_messages \
             (id, room_id, harness_id, from_room_id, from_harness_id, body, \
              created_ms, read_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                m.id,
                m.room_id,
                m.harness_id,
                m.from_room_id,
                m.from_harness_id,
                m.body,
                m.created_ms,
                m.read_ms,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// A harness's unread mail, oldest first.
    pub fn unread_harness_messages(
        &self,
        room_id: &str,
        harness_id: &str,
    ) -> Result<Vec<HarnessMessageRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, room_id, harness_id, from_room_id, from_harness_id, body, \
                        created_ms, read_ms \
                 FROM harness_messages \
                 WHERE room_id = ?1 AND harness_id = ?2 AND read_ms IS NULL \
                 ORDER BY created_ms, rowid",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id, harness_id], row_to_message)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// A harness's whole mail history, oldest first — `include_read`.
    pub fn all_harness_messages(
        &self,
        room_id: &str,
        harness_id: &str,
    ) -> Result<Vec<HarnessMessageRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, room_id, harness_id, from_room_id, from_harness_id, body, \
                        created_ms, read_ms \
                 FROM harness_messages \
                 WHERE room_id = ?1 AND harness_id = ?2 \
                 ORDER BY created_ms, rowid",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id, harness_id], row_to_message)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// One message by id, with no scope check at all — `message_history`
    /// (#364) uses this only to decide whether a `since` message id sits
    /// inside the caller's own visible mail before trusting it as a
    /// paging cursor; the caller does that check, not this method.
    pub fn harness_message_by_id(&self, id: &str) -> Result<Option<HarnessMessageRow>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, room_id, harness_id, from_room_id, from_harness_id, body, \
                    created_ms, read_ms \
             FROM harness_messages WHERE id = ?1",
            params![id],
            row_to_message,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// `message_history`'s (#364) rows: bounded, filtered by counterpart,
    /// in one or both directions, oldest first, never touching
    /// `read_ms`.
    ///
    /// Inbound is the caller harness's own inbox — the same
    /// `room_id`/`harness_id` scope `unread_harness_messages`/
    /// `all_harness_messages` read. Outbound is the caller ROOM's
    /// outbox (`from_room_id`), deliberately not narrowed to the
    /// caller's own harness — the brief is explicit that any harness in
    /// the room counts. That is also why a sibling harness's message
    /// *to* the caller matches both branches: the `NOT (room_id = :room
    /// AND harness_id = :harness)` on the outbound branch is what keeps
    /// such a row classified inbound instead of returned twice. The
    /// `:harness IS NOT NULL` guards on every `room_id = :room AND
    /// harness_id = :harness` comparison exist because SQL's `NOT NULL`
    /// is `NULL`, not `TRUE`: with no caller harness (a headless
    /// `direction: "out"` caller), `harness_id = :harness` alone would
    /// make the whole outbound branch's `NOT (...)` evaluate to `NULL`
    /// and silently drop every row rather than including all of them.
    ///
    /// `since_ms`/`since_message_id` are mutually exclusive and already
    /// resolved by the caller (`message_history` validates a message id
    /// sits in scope before it gets here) — this method just ANDs both
    /// conditions in, so passing both narrows to their intersection
    /// rather than picking one; the verb layer never does.
    #[allow(clippy::too_many_arguments)]
    pub fn harness_message_history(
        &self,
        caller_room_id: &str,
        caller_harness_id: Option<&str>,
        include_inbound: bool,
        include_outbound: bool,
        with: Option<&str>,
        since_ms: Option<i64>,
        since_message_id: Option<&str>,
        fetch_limit: u32,
    ) -> Result<Vec<HarnessMessageRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, room_id, harness_id, from_room_id, from_harness_id, body, \
                        created_ms, read_ms \
                 FROM harness_messages \
                 WHERE ( \
                     (:inbound AND :harness IS NOT NULL \
                      AND room_id = :room AND harness_id = :harness) \
                     OR (:outbound AND from_room_id = :room \
                         AND NOT (:harness IS NOT NULL \
                                  AND room_id = :room AND harness_id = :harness)) \
                 ) \
                 AND (:with IS NULL OR ( \
                     (:harness IS NOT NULL AND room_id = :room AND harness_id = :harness \
                      AND (from_room_id = :with OR from_harness_id = :with)) \
                     OR (from_room_id = :room \
                         AND NOT (:harness IS NOT NULL \
                                  AND room_id = :room AND harness_id = :harness) \
                         AND (room_id = :with OR harness_id = :with)) \
                 )) \
                 AND (:since_ms IS NULL OR created_ms > :since_ms) \
                 AND (:since_id IS NULL OR (created_ms, rowid) > ( \
                     SELECT created_ms, rowid FROM harness_messages WHERE id = :since_id \
                 )) \
                 ORDER BY created_ms, rowid \
                 LIMIT :limit",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                named_params! {
                    ":inbound": include_inbound,
                    ":outbound": include_outbound,
                    ":room": caller_room_id,
                    ":harness": caller_harness_id,
                    ":with": with,
                    ":since_ms": since_ms,
                    ":since_id": since_message_id,
                    ":limit": fetch_limit,
                },
                row_to_message,
            )
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Mark a batch of messages read. Only ever called with ids this
    /// same call just read as unread, so there is nothing to reconcile
    /// against a concurrent read.
    pub fn mark_harness_messages_read(&self, ids: &[String], now_ms: i64) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock();
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE harness_messages SET read_ms = ? WHERE id IN ({placeholders}) \
             AND read_ms IS NULL"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now_ms];
        for id in ids {
            params.push(id);
        }
        conn.execute(&sql, params.as_slice())
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// How many messages are sitting unread for a harness — the #327
    /// inbox cap.
    pub fn unread_harness_message_count(
        &self,
        room_id: &str,
        harness_id: &str,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM harness_messages \
             WHERE room_id = ?1 AND harness_id = ?2 AND read_ms IS NULL",
            params![room_id, harness_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    }

    /// How many messages a room has sent since `since_ms` — the #327
    /// rate cap, counted per sending room rather than per harness so a
    /// room cannot dodge it by spreading sends across its harnesses.
    pub fn harness_messages_sent_since(
        &self,
        from_room_id: &str,
        since_ms: i64,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM harness_messages \
             WHERE from_room_id = ?1 AND created_ms >= ?2",
            params![from_room_id, since_ms],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    }

    /// The newest message `from_room_id` sent to `to_room_id` — `None`
    /// if that room has never sent this one anything. Issue #356's
    /// `list_rooms`: a director rebuilding its room table from one call
    /// needs each child's last status without replaying its whole
    /// mailbox history.
    pub fn latest_message_from_room(
        &self,
        from_room_id: &str,
        to_room_id: &str,
    ) -> Result<Option<HarnessMessageRow>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, room_id, harness_id, from_room_id, from_harness_id, body, \
                    created_ms, read_ms \
             FROM harness_messages \
             WHERE from_room_id = ?1 AND room_id = ?2 \
             ORDER BY created_ms DESC, rowid DESC LIMIT 1",
            params![from_room_id, to_room_id],
            row_to_message,
        )
        .optional()
        .map_err(|e| e.to_string())
    }
}
