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
//!
//! Issue #80 ("Live Context") adds a third table — `harness_actions` —
//! that keeps the richer per-tool-call / per-plan-change / per-patch
//! log feeding the right-pane card stack. Phase transitions stay in
//! `harness_events`; everything else lands here with a `kind`
//! discriminator and a JSON `payload`. Rationale and the v1 kind set
//! are in `docs/live-context-recon.md` §4 and the design brief.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, params};
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
}

/// Mirrors the TS Room interface.
///
/// Field policy (#167): every field added after v0.2.5 MUST carry
/// `#[serde(default)]` (or live inside `Option`). A required field
/// makes every previously-persisted blob unparseable, and an
/// unparseable blob gets quarantined out of the live table on the
/// next boot. Existing required fields stay required — a room
/// missing `name` or `id` is corrupt, not old.
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

/// A `sessions` row whose JSON blob failed to parse at load time.
/// The blob itself is preserved in `sessions_quarantine`; only the
/// id + parse error travel to the frontend (issue #167).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedRoom {
    pub id: String,
    pub error: String,
}

/// What `load_all` hands back: the rooms that parsed, plus the rows
/// that didn't (already moved to quarantine by the time this returns).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadOutcome {
    pub rooms: Vec<Room>,
    pub skipped: Vec<SkippedRoom>,
    /// True when this call flipped the process's `loaded_ok` latch —
    /// i.e. the first successful load. Internal signal for the backup
    /// policy (refresh at most once per process, so a dev `StrictMode`
    /// double-load can't sneak a backup in after a quarantine-marred
    /// sibling call). Never serialized to the frontend.
    #[serde(skip)]
    pub first_load: bool,
    /// Rooms sitting in the `.bak` snapshot. Populated by the command
    /// layer only when the live table came back empty, so the
    /// frontend can say "your db is empty but a backup exists"
    /// instead of showing first-run onboarding over lost rooms.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub backup_rooms: Option<i64>,
}

pub struct Database {
    conn: Mutex<Connection>,
    path: PathBuf,
    /// Set once `load_all` has completed successfully in this process.
    /// `save_all` refuses to commit an empty room list before that —
    /// the frontend only legitimately saves `[]` after a good load
    /// (issue #167: a failed boot load must never wipe the table).
    loaded_ok: AtomicBool,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        // WAL keeps readers and the (single) writer from blocking each
        // other and survives crash-mid-write without a hot journal;
        // busy_timeout papers over transient contention instead of
        // surfacing SQLITE_BUSY to the user. (#167 belt-and-braces;
        // #178 tunes the rest of the write path.)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        // Fold any WAL left by an unclean previous exit back into the
        // main file and truncate the sidecar. Keeps skein.db-wal
        // near-empty at rest, so hand-restoring `.bak` over skein.db
        // (or deleting skein.db alone) can't pair a stale hot WAL
        // with the wrong database file (#167 review).
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
            loaded_ok: AtomicBool::new(false),
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

        // Issue #167: rooms whose JSON blob no longer parses are moved
        // here instead of aborting the whole load (or worse, being
        // erased by the next save_all wipe). Plain `id` column — the
        // same room id can land here more than once across versions.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions_quarantine (
                id TEXT NOT NULL,
                data TEXT NOT NULL,
                error TEXT NOT NULL,
                quarantined_at INTEGER NOT NULL
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

        // Issue #80: append-only log of tool calls / plan changes /
        // patches / etc. Same shape concerns as `harness_events` —
        // monotonic id, time-ordered indexes — plus a (room, kind, ts)
        // index for the Plan card which queries one kind across a room.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS harness_actions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness_id TEXT NOT NULL,
                room_id TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                source TEXT
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_actions_harness \
             ON harness_actions(harness_id, timestamp_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_actions_room \
             ON harness_actions(room_id, timestamp_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_actions_room_kind \
             ON harness_actions(room_id, kind, timestamp_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Load every room. A row whose blob fails to parse (serde drift
    /// after a downgrade, corruption, a future required field) no
    /// longer fails the whole load — it is moved to
    /// `sessions_quarantine` and reported in `skipped`, and every
    /// other room comes back intact (issue #167).
    ///
    /// sqlite-level errors (open/read failures) still fail wholesale;
    /// the frontend parks its autosave on that path.
    pub fn load_all(&self) -> Result<LoadOutcome, String> {
        let conn = self.conn.lock();
        // Collect first, mutate after — deleting rows out from under
        // an open SELECT cursor on the same table is undefined-ish
        // in sqlite, and the table is a dozen rows.
        let raw: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, data FROM sessions ORDER BY created_at, id")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        let mut rooms = Vec::new();
        let mut skipped = Vec::new();
        for (id, data) in raw {
            match serde_json::from_str::<Room>(&data) {
                Ok(r) => rooms.push(r),
                Err(e) => {
                    let error = e.to_string();
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                    // Preserve the blob before dropping the live row —
                    // the next save_all wipe-and-reinsert would erase
                    // it otherwise. If the INSERT fails we abort the
                    // load rather than lose the row.
                    conn.execute(
                        "INSERT INTO sessions_quarantine (id, data, error, quarantined_at) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![id, data, error, now_ms],
                    )
                    .map_err(|e| e.to_string())?;
                    conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                        .map_err(|e| e.to_string())?;
                    skipped.push(SkippedRoom { id, error });
                }
            }
        }
        let first_load = self
            .loaded_ok
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        Ok(LoadOutcome {
            rooms,
            skipped,
            first_load,
            backup_rooms: None,
        })
    }

    /// Cwds of every persisted room, active and archived — the scope
    /// anchor for the fs commands (#49/#174: the webview may only
    /// read inside its rooms).
    /// Parses only the `cwd` field out of each blob; rows that fail
    /// even that are skipped here (`load_all` owns quarantine).
    pub fn room_cwds(&self) -> Result<Vec<String>, String> {
        #[derive(Deserialize)]
        struct CwdOnly {
            cwd: Option<String>,
        }
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT data FROM sessions")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let data = row.map_err(|e| e.to_string())?;
            if let Ok(parsed) = serde_json::from_str::<CwdOnly>(&data) {
                if let Some(cwd) = parsed.cwd {
                    out.push(cwd);
                }
            }
        }
        Ok(out)
    }

    pub fn save_all(&self, rooms: &[Room]) -> Result<(), String> {
        let mut conn = self.conn.lock();
        // #167: an empty save before any successful load in this
        // process is always a bug (the boot-wipe chain: load fails,
        // frontend state is still [], autosave fires). A legitimate
        // "user deleted the last room" save happens strictly after a
        // good load, so it passes the loaded_ok gate.
        if rooms.is_empty() && !self.loaded_ok.load(Ordering::Acquire) {
            let existing: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            if existing > 0 {
                return Err(format!(
                    "refusing to overwrite {existing} persisted room(s) with an empty list \
                     before a successful load (#167)"
                ));
            }
        }
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

    /// Snapshot the whole DB to `<db>.bak` (issue #167). `VACUUM INTO`
    /// gives a consistent single-file copy even under WAL, without the
    /// rusqlite backup feature. The caller decides *when* — the policy
    /// is "first clean, non-empty load of the process", so `.bak`
    /// always holds a last-known-good state and is never overwritten
    /// by a wipe or a quarantine-marred load.
    ///
    /// Write order matters (#167 review): the snapshot lands in a temp
    /// file first and only replaces `.bak` once complete, so a failed
    /// or interrupted VACUUM (disk full, crash) can't destroy the
    /// previous good snapshot. The displaced `.bak` is kept one more
    /// generation as `.bak.1` — the "rooms silently vanished, then one
    /// clean boot refreshed the backup" sequence stays recoverable.
    pub fn backup_last_known_good(&self) -> Result<PathBuf, String> {
        let dest = self.path.with_extension("db.bak");
        let prev = self.path.with_extension("db.bak.1");
        let tmp = self.path.with_extension("db.bak.tmp");
        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| format!("backup path is not valid UTF-8: {}", tmp.display()))?;
        if tmp.exists() {
            std::fs::remove_file(&tmp).map_err(|e| e.to_string())?;
        }
        {
            let conn = self.conn.lock();
            conn.execute("VACUUM INTO ?1", params![tmp_str])
                .map_err(|e| e.to_string())?;
        }
        if dest.exists() {
            replace_file(&dest, &prev)?;
        }
        replace_file(&tmp, &dest)?;
        Ok(dest)
    }

    /// Rooms stored in the `.bak` snapshot, or `None` when no readable
    /// backup exists. Opens read-only so probing can't touch either
    /// file. Used to warn when the live table is empty but a backup
    /// holds rooms — a vanished/recreated skein.db otherwise looks
    /// exactly like a fresh install (#167 review).
    pub fn count_backup_rooms(&self) -> Option<i64> {
        let bak = self.path.with_extension("db.bak");
        let conn = Connection::open_with_flags(&bak, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .ok()
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

    // ── harness action log (issue #80) ────────────────────────────

    /// Append one row to `harness_actions`. Adapters call this from
    /// the Rust side per extracted action; the canonical payload
    /// shape per `kind` is documented in the design brief. We don't
    /// validate `payload` here — it's stored verbatim.
    pub fn record_harness_action(
        &self,
        harness_id: &str,
        room_id: &str,
        timestamp_ms: i64,
        kind: &str,
        payload: &str,
        source: Option<&str>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO harness_actions \
             (harness_id, room_id, timestamp_ms, kind, payload, source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![harness_id, room_id, timestamp_ms, kind, payload, source],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
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
                "SELECT id, harness_id, room_id, timestamp_ms, kind, payload, source \
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
                "SELECT id, harness_id, room_id, timestamp_ms, kind, payload, source \
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
                "SELECT id, harness_id, room_id, timestamp_ms, kind, payload, source \
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

/// `rename` that also replaces an existing `to` on Windows, where
/// std's rename refuses to overwrite. Callers only pass a complete
/// file as `from`, so the remove-then-retry window never risks the
/// last good copy.
pub(crate) fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Only when the source still exists: if a concurrent caller
        // already consumed `from`, removing `to` here would delete the
        // freshly-written target (#185 review).
        Err(_) if to.exists() && from.exists() => {
            std::fs::remove_file(to).map_err(|e| e.to_string())?;
            std::fs::rename(from, to).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
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

fn row_to_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessAction> {
    Ok(HarnessAction {
        id: row.get(0)?,
        harness_id: row.get(1)?,
        room_id: row.get(2)?,
        timestamp_ms: row.get(3)?,
        kind: row.get(4)?,
        payload: row.get(5)?,
        source: row.get(6)?,
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

    fn room(id: &str) -> Room {
        Room {
            id: id.into(),
            name: format!("room {id}"),
            task: String::new(),
            status: "idle".into(),
            badge: 0,
            harnesses: Vec::new(),
            active_harness_id: String::new(),
            cwd: None,
            branch: None,
            repo: None,
            archived: None,
        }
    }

    // ── room persistence (#167) ──────────────────────────────────

    #[test]
    fn rooms_round_trip_through_save_and_load() {
        let (_dir, db) = fresh_db();
        let mut r1 = room("r1");
        r1.branch = Some("skein/r1".into());
        r1.archived = Some(1_000);
        db.save_all(&[r1, room("r2")]).unwrap();
        let outcome = db.load_all().unwrap();
        assert!(outcome.skipped.is_empty());
        assert_eq!(outcome.rooms.len(), 2);
        // created_at preserves insertion order across the round-trip.
        assert_eq!(outcome.rooms[0].id, "r1");
        assert_eq!(outcome.rooms[0].branch.as_deref(), Some("skein/r1"));
        assert_eq!(outcome.rooms[0].archived, Some(1_000));
        assert_eq!(outcome.rooms[1].id, "r2");
    }

    #[test]
    fn load_all_quarantines_unparseable_rows_and_keeps_good_ones() {
        let (_dir, db) = fresh_db();
        db.save_all(&[room("good"), room("bad")]).unwrap();
        db.conn
            .lock()
            .execute("UPDATE sessions SET data = 'not json' WHERE id = 'bad'", [])
            .unwrap();
        let outcome = db.load_all().unwrap();
        assert_eq!(outcome.rooms.len(), 1);
        assert_eq!(outcome.rooms[0].id, "good");
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].id, "bad");
        assert!(!outcome.skipped[0].error.is_empty());
        // The blob is preserved in quarantine and gone from the live
        // table, so the next save_all wipe can't destroy it.
        let conn = db.conn.lock();
        let blob: String = conn
            .query_row(
                "SELECT data FROM sessions_quarantine WHERE id = 'bad'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(blob, "not json");
        let live: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(live, 1);
    }

    #[test]
    fn save_all_empty_before_load_is_refused_when_rooms_exist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        {
            let db = Database::open(&path).unwrap();
            db.save_all(&[room("r1")]).unwrap();
        }
        // Fresh open = fresh process: loaded_ok is false. This call is
        // the exact #167 boot-wipe chain and must be refused.
        let db = Database::open(&path).unwrap();
        let err = db.save_all(&[]).unwrap_err();
        assert!(err.contains("#167"), "unexpected error: {err}");
        assert_eq!(db.load_all().unwrap().rooms.len(), 1);
    }

    #[test]
    fn save_all_empty_after_successful_load_is_allowed() {
        let (_dir, db) = fresh_db();
        db.save_all(&[room("r1")]).unwrap();
        let _ = db.load_all().unwrap();
        // "User deleted the last room" — legitimate empty save.
        db.save_all(&[]).unwrap();
        assert!(db.load_all().unwrap().rooms.is_empty());
    }

    #[test]
    fn first_load_is_flagged_only_once_per_process() {
        let (_dir, db) = fresh_db();
        assert!(db.load_all().unwrap().first_load);
        assert!(!db.load_all().unwrap().first_load);
    }

    #[test]
    fn backup_rotation_keeps_one_previous_generation() {
        let (_dir, db) = fresh_db();
        db.save_all(&[room("r1")]).unwrap();
        let _ = db.load_all().unwrap();
        db.backup_last_known_good().unwrap();
        db.save_all(&[room("r1"), room("r2")]).unwrap();
        let bak = db.backup_last_known_good().unwrap();
        let prev = bak.with_extension("bak.1");
        assert_eq!(
            Database::open(&bak)
                .unwrap()
                .load_all()
                .unwrap()
                .rooms
                .len(),
            2
        );
        // The displaced snapshot survives one generation back.
        assert_eq!(
            Database::open(&prev)
                .unwrap()
                .load_all()
                .unwrap()
                .rooms
                .len(),
            1
        );
    }

    #[test]
    fn count_backup_rooms_reads_snapshot_or_none() {
        let (_dir, db) = fresh_db();
        assert!(db.count_backup_rooms().is_none());
        db.save_all(&[room("r1")]).unwrap();
        db.backup_last_known_good().unwrap();
        assert_eq!(db.count_backup_rooms(), Some(1));
    }

    #[test]
    fn backup_snapshot_survives_a_later_wipe() {
        let (_dir, db) = fresh_db();
        db.save_all(&[room("r1")]).unwrap();
        let _ = db.backup_last_known_good().unwrap();
        // Second call must overwrite, not fail (VACUUM INTO refuses
        // to write over an existing file on its own).
        let bak = db.backup_last_known_good().unwrap();
        let _ = db.load_all().unwrap();
        db.save_all(&[]).unwrap();
        let restored = Database::open(&bak).unwrap();
        assert_eq!(restored.load_all().unwrap().rooms.len(), 1);
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

    // ── harness_actions (issue #80) ───────────────────────────────

    #[test]
    fn action_record_then_query_by_harness_returns_row() {
        let (_dir, db) = fresh_db();
        let payload = r#"{"tool":"bash","input":{"command":"ls"}}"#;
        db.record_harness_action(
            "h1",
            "r1",
            1_000,
            action_kind::TOOL_CALL,
            payload,
            Some("l2c1"),
        )
        .unwrap();
        let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.harness_id, "h1");
        assert_eq!(a.room_id, "r1");
        assert_eq!(a.timestamp_ms, 1_000);
        assert_eq!(a.kind, "tool_call");
        assert_eq!(a.payload, payload);
        assert_eq!(a.source.as_deref(), Some("l2c1"));
    }

    #[test]
    fn action_query_excludes_rows_at_or_before_since_ms() {
        let (_dir, db) = fresh_db();
        for ts in [100, 200, 300, 400] {
            db.record_harness_action("h1", "r1", ts, action_kind::PATCH, "{}", None)
                .unwrap();
        }
        let actions = db.recent_harness_actions_by_harness("h1", 200, 10).unwrap();
        let timestamps: Vec<i64> = actions.iter().map(|a| a.timestamp_ms).collect();
        assert_eq!(timestamps, vec![400, 300]);
    }

    #[test]
    fn action_query_is_scoped_by_harness_id() {
        let (_dir, db) = fresh_db();
        db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        db.record_harness_action("h2", "r1", 200, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        db.record_harness_action("h1", "r1", 300, action_kind::PATCH, "{}", None)
            .unwrap();
        let h1 = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
        assert_eq!(h1.len(), 2);
        assert!(h1.iter().all(|a| a.harness_id == "h1"));
    }

    #[test]
    fn action_query_by_room_returns_all_harnesses_in_that_room() {
        let (_dir, db) = fresh_db();
        db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        db.record_harness_action("h2", "r1", 200, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        db.record_harness_action("h3", "r2", 300, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        let r1 = db.recent_harness_actions_by_room("r1", 0, 10).unwrap();
        assert_eq!(r1.len(), 2);
        assert!(r1.iter().all(|a| a.room_id == "r1"));
    }

    #[test]
    fn action_query_by_room_and_kind_filters_other_kinds_out() {
        let (_dir, db) = fresh_db();
        db.record_harness_action(
            "h1",
            "r1",
            100,
            action_kind::PLAN_CHANGE,
            r#"{"n":1}"#,
            None,
        )
        .unwrap();
        db.record_harness_action("h1", "r1", 200, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        db.record_harness_action(
            "h2",
            "r1",
            300,
            action_kind::PLAN_CHANGE,
            r#"{"n":2}"#,
            None,
        )
        .unwrap();
        let plans = db
            .recent_harness_actions_by_room_and_kind("r1", action_kind::PLAN_CHANGE, 0, 10)
            .unwrap();
        assert_eq!(plans.len(), 2);
        assert!(plans.iter().all(|a| a.kind == "plan_change"));
        // Newest first.
        assert_eq!(plans[0].timestamp_ms, 300);
        assert_eq!(plans[1].timestamp_ms, 100);
    }

    #[test]
    fn action_query_respects_limit() {
        let (_dir, db) = fresh_db();
        for ts in 0..50 {
            db.record_harness_action("h1", "r1", ts, action_kind::TOOL_CALL, "{}", None)
                .unwrap();
        }
        let actions = db.recent_harness_actions_by_harness("h1", -1, 5).unwrap();
        assert_eq!(actions.len(), 5);
        assert_eq!(actions[0].timestamp_ms, 49);
        assert_eq!(actions[4].timestamp_ms, 45);
    }

    #[test]
    fn action_payload_is_stored_verbatim_including_unicode_and_quotes() {
        // The DB layer must not parse / re-serialize / escape payloads
        // beyond what sqlite needs — adapters write JSON, consumers
        // read the same bytes back.
        let (_dir, db) = fresh_db();
        let payload = r#"{"text":"hello \"world\" — café 🌮","nested":{"k":[1,2,3]}}"#;
        db.record_harness_action("h1", "r1", 100, action_kind::AWAY_SUMMARY, payload, None)
            .unwrap();
        let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
        assert_eq!(actions[0].payload, payload);
    }

    #[test]
    fn action_query_orders_same_ms_rows_by_id_desc() {
        // Two actions written in the same millisecond must come back
        // in insertion order (newest first), so the timeline doesn't
        // flicker between Skein restarts.
        let (_dir, db) = fresh_db();
        db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, r#"{"n":1}"#, None)
            .unwrap();
        db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, r#"{"n":2}"#, None)
            .unwrap();
        db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, r#"{"n":3}"#, None)
            .unwrap();
        let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].payload, r#"{"n":3}"#);
        assert_eq!(actions[2].payload, r#"{"n":1}"#);
    }

    #[test]
    fn action_null_source_round_trips_as_none() {
        let (_dir, db) = fresh_db();
        db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
        assert!(actions[0].source.is_none());
    }

    #[test]
    fn action_schema_is_idempotent_across_open_calls() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let _db1 = Database::open(&path).unwrap();
        let db2 = Database::open(&path).unwrap();
        db2.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
        assert_eq!(
            db2.recent_harness_actions_by_harness("h1", 0, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn action_kind_constants_match_persisted_strings() {
        // Lock the v1 vocabulary so accidental renames trigger a
        // failing test (consumers read these strings directly from
        // the DB; renaming would orphan historical rows).
        assert_eq!(action_kind::TOOL_CALL, "tool_call");
        assert_eq!(action_kind::PLAN_CHANGE, "plan_change");
        assert_eq!(action_kind::PATCH, "patch");
        assert_eq!(action_kind::PR_LINK, "pr_link");
        assert_eq!(action_kind::QUEUE_OP, "queue_op");
        assert_eq!(action_kind::EDITED_TEXT_FILE, "edited_text_file");
        assert_eq!(action_kind::SLASH_COMMAND, "slash_command");
        assert_eq!(action_kind::AWAY_SUMMARY, "away_summary");
        assert_eq!(action_kind::TURN_DURATION, "turn_duration");
        assert_eq!(action_kind::API_ERROR, "api_error");
        assert_eq!(action_kind::TURN_COST, "turn_cost");
        assert_eq!(action_kind::PERMISSION_MODE, "permission_mode");
        assert_eq!(action_kind::AI_TITLE, "ai_title");
        assert_eq!(action_kind::BRIDGE_STATUS, "bridge_status");
        assert_eq!(action_kind::USER_PROMPT, "user_prompt");
        assert_eq!(action_kind::COMPACTION, "compaction");
        assert_eq!(action_kind::REASONING, "reasoning");
    }
}
