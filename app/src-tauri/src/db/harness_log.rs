//! The `harness_events` and `harness_actions` append-only logs. Split out of db.rs (#454).

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Database;

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

/// One row in the `harness_actions` append-only log. Issue #80.
///
/// Sibling of [`HarnessEvent`] — that table tracks phase transitions
/// (a tight finite state machine); this one tracks everything else
/// (tool calls, plan changes, patches, …). They join naturally on
/// `(harness_id, timestamp_ms)` if a unified room timeline is ever
/// needed.
///
/// `kind` is a free-form string. The v1 vocabulary lives in
/// [`action_kind`] as constants — adapters write rows with those
/// values, consumers compare against them. Adding a new kind is zero
/// schema work: a new `pub const` here, populate it in the adapter.
///
/// `payload` is an opaque JSON string. Shape varies per kind; the
/// canonical shape per kind is documented in
/// `docs/live-context-design-brief.md` §3. The DB layer stores +
/// returns it verbatim — no parsing or validation here.
///
/// `source` carries the adapter event id that produced this row
/// (mirrors the L7a `source` column on `harness_events`). Reserved
/// for cross-room / cross-harness correlation in later issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessAction {
    pub id: i64,
    pub harness_id: String,
    pub room_id: String,
    /// Epoch milliseconds.
    pub timestamp_ms: i64,
    pub kind: String,
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    /// The `HarnessKind` string of the harness that wrote the row (#538),
    /// so a feed row keeps its chip after the harness is closed. `None`
    /// for rows written before the column existed.
    #[serde(default)]
    pub harness_kind: Option<String>,
}

/// One row to insert via the batch path, `record_harness_actions`
/// (#171e). `harness_id`/`room_id` aren't here — a batch is always for
/// one harness in one room, so the caller passes those once for the
/// whole slice instead of repeating them per row. Adapters build these
/// from their own (per-module, identically shaped) `ExtractedAction`.
pub struct NewHarnessAction {
    pub timestamp_ms: i64,
    pub kind: &'static str,
    pub payload: String,
    pub source: Option<String>,
}

/// The v1 `kind` vocabulary for [`HarnessAction`] rows. Adapters and
/// consumers refer to these constants instead of magic strings so a
/// rename is one place. Adding a new kind doesn't require changing
/// this list — it's a convenience, not a constraint.
///
/// `#[allow(dead_code)]` because this PR lands the schema + record/
/// read API ahead of the adapter PRs that consume the full kind set.
#[allow(dead_code)]
pub mod action_kind {
    pub const TOOL_CALL: &str = "tool_call";
    pub const PLAN_CHANGE: &str = "plan_change";
    pub const PATCH: &str = "patch";
    pub const PR_LINK: &str = "pr_link";
    pub const QUEUE_OP: &str = "queue_op";
    pub const EDITED_TEXT_FILE: &str = "edited_text_file";
    pub const SLASH_COMMAND: &str = "slash_command";
    pub const AWAY_SUMMARY: &str = "away_summary";
    pub const TURN_DURATION: &str = "turn_duration";
    pub const API_ERROR: &str = "api_error";
    pub const TURN_COST: &str = "turn_cost";
    pub const COST_STATE: &str = "cost_state";
    pub const PERMISSION_MODE: &str = "permission_mode";
    pub const AI_TITLE: &str = "ai_title";
    pub const BRIDGE_STATUS: &str = "bridge_status";
    pub const USER_PROMPT: &str = "user_prompt";
    pub const COMPACTION: &str = "compaction";
    pub const REASONING: &str = "reasoning";
    /// A subagent transcript reached its terminal row (epic #298). The
    /// delegation itself already renders via the main transcript's
    /// `Agent` `tool_call` row; this is the other end, for background
    /// subagents where nothing else ever marks completion.
    pub const SUBAGENT_END: &str = "subagent_end";
    /// A #327 mailbox message landed for this harness (#329) — the
    /// recipient's side of a `send_message`. Paired with
    /// [`MESSAGE_OUT`] on the sender, same timestamp, same payload; no
    /// message body in either, so the feed shows who talked to whom
    /// without duplicating what `read_messages` already guards.
    pub const MESSAGE_IN: &str = "message_in";
    /// The sender's side of a #327 mailbox `send_message` (#329). See
    /// [`MESSAGE_IN`].
    pub const MESSAGE_OUT: &str = "message_out";
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

fn row_to_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessAction> {
    Ok(HarnessAction {
        id: row.get(0)?,
        harness_id: row.get(1)?,
        room_id: row.get(2)?,
        timestamp_ms: row.get(3)?,
        kind: row.get(4)?,
        payload: row.get(5)?,
        source: row.get(6)?,
        harness_kind: row.get(7)?,
    })
}

impl Database {
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

    /// Append one row to `harness_actions`. Adapters call this from
    /// the Rust side per extracted action; the canonical payload
    /// shape per `kind` is documented in the design brief. We don't
    /// validate `payload` here — it's stored verbatim.
    #[allow(clippy::too_many_arguments)]
    pub fn record_harness_action(
        &self,
        harness_id: &str,
        room_id: &str,
        timestamp_ms: i64,
        kind: &str,
        payload: &str,
        source: Option<&str>,
        harness_kind: Option<&str>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO harness_actions \
             (harness_id, room_id, timestamp_ms, kind, payload, source, harness_kind) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                harness_id,
                room_id,
                timestamp_ms,
                kind,
                payload,
                source,
                harness_kind
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    /// Batch counterpart to `record_harness_action` (#171e). The two
    /// backfill scans (Claude JSONL history, opencode `SQLite`
    /// history) walk a whole session's worth of rows in one shot at
    /// attach time; inserting each one individually meant one lock +
    /// one implicit transaction per row. This takes the lock once,
    /// opens a single transaction, reuses one prepared statement for
    /// every row, and commits — `harness_id`/`room_id` are stamped on
    /// every row (as is `harness_kind`) exactly like the per-call path.
    /// Empty `rows` is a no-op (no lock taken). Returns the number of rows inserted.
    pub fn record_harness_actions(
        &self,
        harness_id: &str,
        room_id: &str,
        harness_kind: Option<&str>,
        rows: &[NewHarnessAction],
    ) -> Result<usize, String> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO harness_actions \
                     (harness_id, room_id, timestamp_ms, kind, payload, source, harness_kind) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|e| e.to_string())?;
            for row in rows {
                stmt.execute(params![
                    harness_id,
                    room_id,
                    row.timestamp_ms,
                    row.kind,
                    row.payload,
                    row.source,
                    harness_kind
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(rows.len())
    }

    /// Most recent actions for a single harness with
    /// `timestamp_ms > since_ms`. Newest-first.
    pub fn recent_harness_actions_by_harness(
        &self,
        harness_id: &str,
        since_ms: i64,
        limit: i64,
    ) -> Result<Vec<HarnessAction>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, harness_id, room_id, timestamp_ms, kind, payload, source, harness_kind \
                 FROM harness_actions \
                 WHERE harness_id = ?1 AND timestamp_ms > ?2 \
                 ORDER BY timestamp_ms DESC, id DESC \
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![harness_id, since_ms, limit], row_to_action)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Most recent actions across every harness in a room. Newest-first.
    /// Backs the Activity card's unified per-room timeline.
    pub fn recent_harness_actions_by_room(
        &self,
        room_id: &str,
        since_ms: i64,
        limit: i64,
    ) -> Result<Vec<HarnessAction>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, harness_id, room_id, timestamp_ms, kind, payload, source, harness_kind \
                 FROM harness_actions \
                 WHERE room_id = ?1 AND timestamp_ms > ?2 \
                 ORDER BY timestamp_ms DESC, id DESC \
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id, since_ms, limit], row_to_action)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Most recent actions of a single `kind` in a room. Backs the
    /// Plan card (`kind = "plan_change"`) and other per-kind surfaces.
    /// Uses the `(room_id, kind, timestamp_ms)` index.
    pub fn recent_harness_actions_by_room_and_kind(
        &self,
        room_id: &str,
        kind: &str,
        since_ms: i64,
        limit: i64,
    ) -> Result<Vec<HarnessAction>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, harness_id, room_id, timestamp_ms, kind, payload, source, harness_kind \
                 FROM harness_actions \
                 WHERE room_id = ?1 AND kind = ?2 AND timestamp_ms > ?3 \
                 ORDER BY timestamp_ms DESC, id DESC \
                 LIMIT ?4",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id, kind, since_ms, limit], row_to_action)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
