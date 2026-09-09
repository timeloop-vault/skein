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
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
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

        // Issue #211 (epic #52 D3): the review baseline — the content
        // of each touched file as the user last reviewed it. A sibling
        // table rather than a field on the Room blob: #167's field
        // policy means a new Room field has to be `serde(default)` or
        // `Option`, and a per-file content snapshot has no business
        // being rewritten wholesale on every autosave.
        //
        // `content` is NULL unless `kind = 'text'` — the other kinds
        // (missing / binary / toolarge / symlink / unreadable) record
        // *why* there is nothing to diff, so the pane can say so
        // instead of silently dropping the file.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_baselines (
                room_id TEXT NOT NULL,
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT,
                harness_id TEXT NOT NULL,
                captured_ms INTEGER NOT NULL,
                touched_ms INTEGER NOT NULL,
                PRIMARY KEY (room_id, path)
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Issue #212 (epic #52 D5/D6/D7): review comments.
        //
        // A **thread** carries the anchor — where in the review it is
        // attached — and **comments** carry the words. Splitting them
        // is what makes D5's "flat comments with replies" a shape
        // rather than a convention: a reply is another row on the same
        // thread, and there is nowhere for a nested thread to go.
        //
        // `anchor_lines` is the text the comment was written against,
        // JSON-encoded. It is the anchor itself, not a cache of it
        // (#212 D6) — line numbers move, and a thread that cannot be
        // re-matched is rendered against these lines rather than
        // against whatever now occupies its old coordinates.
        //
        // Nothing here has a foreign key, matching `review_baselines`:
        // closing a room archives it rather than deleting it, so there
        // is no cascade to model, and a thread that outlives its file
        // is exactly the outdated case the model is built to show.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_threads (
                id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                scope TEXT NOT NULL,
                file_path TEXT,
                commit_sha TEXT,
                side TEXT,
                line_start INTEGER,
                line_end INTEGER,
                anchor_hash TEXT,
                anchor_lines TEXT,
                resolved_ms INTEGER,
                created_ms INTEGER NOT NULL,
                updated_ms INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_review_threads_room \
             ON review_threads(room_id, created_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;

        // `author_kind` / `author_id` exist from this first migration
        // even though v1 is human-only (D7). Sub-issue #213 lets the
        // agent reply; carrying the columns now makes that a row-level
        // change instead of a migration against live data.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_comments (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                room_id TEXT NOT NULL,
                author_kind TEXT NOT NULL,
                author_id TEXT,
                body TEXT NOT NULL,
                created_ms INTEGER NOT NULL,
                updated_ms INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_review_comments_thread \
             ON review_comments(thread_id, created_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Which files the user has marked as looked at, and what they
        // looked at. Storing the *content hash* rather than a flag is
        // what gives #212 its "changes since I last looked": the mark
        // survives a refresh and lapses by itself the moment the agent
        // touches the file again.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_viewed (
                room_id TEXT NOT NULL,
                path TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                viewed_ms INTEGER NOT NULL,
                PRIMARY KEY (room_id, path)
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Issue #213 (epic #52 D8): which review comments an agent has
        // said it handled, and with what commit.
        //
        // A sibling table rather than columns on `review_threads`,
        // because there is no migration machinery here — every table
        // above is `CREATE TABLE IF NOT EXISTS`, so a new *column* on a
        // table that already exists in a live db would simply never
        // appear. The same reasoning `review_baselines` and
        // `review_settings` followed.
        //
        // `commit_sha` is nullable on purpose: an agent that has
        // addressed a comment but not yet committed should still be
        // able to say so, and a claim with no sha is more useful than
        // no claim.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_addressed (
                thread_id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                commit_sha TEXT,
                harness_id TEXT NOT NULL,
                note TEXT,
                addressed_ms INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_review_addressed_room \
             ON review_addressed(room_id)",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Issue #214 (epic #52 D9, as corrected): the reviewer's
        // sign-off. One row per room, because a room is one review
        // (D1); no row means not approved.
        //
        // `head_sha` is the point of the table. A sign-off approves a
        // *state of the code*, not a room — so it records what HEAD was
        // when it was granted, and anything reading it compares that to
        // HEAD now. An agent that commits after being approved makes
        // the approval stale, which is visible, rather than silently
        // extending it over code nobody looked at. Same trick
        // `review_viewed` plays one level down with its content hash.
        //
        // A sibling table for the reason all the others are: there is
        // no migration machinery here, so a new *column* on an existing
        // table would never appear in a live database.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_signoff (
                room_id TEXT PRIMARY KEY,
                head_sha TEXT NOT NULL,
                base_ref TEXT,
                note TEXT,
                approved_ms INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        // Issue #213: the per-room bearer token the agent API
        // authenticates with. The token *is* the room scope — no
        // request carries a room id, so a token can only ever reach the
        // room it was minted for.
        //
        // Rotation revokes rather than deletes: a stale token has to be
        // distinguishable from one that never existed, so a harness
        // still holding an old one gets "revoked" instead of the
        // indistinguishable "unknown".
        conn.execute(
            "CREATE TABLE IF NOT EXISTS agent_tokens (
                token TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                created_ms INTEGER NOT NULL,
                revoked_ms INTEGER
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_agent_tokens_room \
             ON agent_tokens(room_id)",
            [],
        )
        .map_err(|e| e.to_string())?;

        // The per-room base ref. A sibling table rather than a Room
        // field for the same reason as the baselines: #167's field
        // policy, and no reason to rewrite it on every autosave.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_settings (
                room_id TEXT PRIMARY KEY,
                base_ref TEXT NOT NULL,
                updated_ms INTEGER NOT NULL
            )",
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

    // ── review baselines (issue #211) ─────────────────────────────

    /// Capture a baseline for `path` only if this room has never seen
    /// it. Returns `true` when a row was created.
    ///
    /// "Only if absent" is the whole episode boundary: the first time a
    /// harness touches a file we snapshot what the user had already
    /// accepted (or what was committed), and every subsequent edit
    /// diffs against that same snapshot until they review it. A second
    /// capture would move the baseline behind the user's back and make
    /// their pending change disappear.
    pub fn insert_review_baseline_if_absent(
        &self,
        room_id: &str,
        path: &str,
        kind: &str,
        content: Option<&str>,
        harness_id: &str,
        now_ms: i64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO review_baselines \
             (room_id, path, kind, content, harness_id, captured_ms, touched_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![room_id, path, kind, content, harness_id, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.changes() > 0)
    }

    /// Record that `harness_id` wrote `path` again. Attribution only —
    /// the baseline content is untouched (D4: harness is a chip, not a
    /// scope).
    pub fn touch_review_baseline(
        &self,
        room_id: &str,
        path: &str,
        harness_id: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE review_baselines SET harness_id = ?3, touched_ms = ?4 \
             WHERE room_id = ?1 AND path = ?2",
            params![room_id, path, harness_id, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Move the baseline forward — what "accept" does. The working
    /// tree is not this function's business and is never touched.
    ///
    /// An upsert rather than an update so a missing row can never make
    /// an accept a silent no-op; in practice accept always runs against
    /// a path that already has a baseline, and `harness_id` is
    /// deliberately absent from the conflict clause so advancing the
    /// baseline does not erase who last wrote the file.
    pub fn advance_review_baseline(
        &self,
        room_id: &str,
        path: &str,
        kind: &str,
        content: Option<&str>,
        now_ms: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_baselines \
             (room_id, path, kind, content, harness_id, captured_ms, touched_ms) \
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?5) \
             ON CONFLICT(room_id, path) DO UPDATE SET \
               kind = excluded.kind, content = excluded.content, captured_ms = excluded.captured_ms",
            params![room_id, path, kind, content, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Every baseline the room holds, path-ordered for stable tabs.
    pub fn review_baselines_for_room(&self, room_id: &str) -> Result<Vec<ReviewBaseline>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT path, kind, content, harness_id, captured_ms, touched_ms \
                 FROM review_baselines WHERE room_id = ?1 ORDER BY path",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], row_to_baseline)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// One baseline, or `None` when the room has never seen the path.
    pub fn review_baseline(
        &self,
        room_id: &str,
        path: &str,
    ) -> Result<Option<ReviewBaseline>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT path, kind, content, harness_id, captured_ms, touched_ms \
                 FROM review_baselines WHERE room_id = ?1 AND path = ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![room_id, path], row_to_baseline)
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    // ── review comments (issue #212) ──────────────────────────────

    /// Create a thread. The caller has already minted the id, so the
    /// same value can be used for the thread's first comment without a
    /// round trip.
    pub fn insert_review_thread(&self, t: &ReviewThreadRow) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_threads \
             (id, room_id, scope, file_path, commit_sha, side, line_start, line_end, \
              anchor_hash, anchor_lines, resolved_ms, created_ms, updated_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                t.id,
                t.room_id,
                t.scope,
                t.file_path,
                t.commit_sha,
                t.side,
                t.line_start,
                t.line_end,
                t.anchor_hash,
                t.anchor_lines,
                t.resolved_ms,
                t.created_ms,
                t.updated_ms,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Every thread in the room, oldest first.
    ///
    /// The whole room in one query on purpose: the pane re-anchors all
    /// of them on every refresh, and per-file queries would turn one
    /// refresh into a query per changed file.
    pub fn review_threads_for_room(&self, room_id: &str) -> Result<Vec<ReviewThreadRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, room_id, scope, file_path, commit_sha, side, line_start, line_end, \
                        anchor_hash, anchor_lines, resolved_ms, created_ms, updated_ms \
                 FROM review_threads WHERE room_id = ?1 ORDER BY created_ms, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], row_to_thread)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// One thread, or `None` when it has been deleted.
    pub fn review_thread(&self, thread_id: &str) -> Result<Option<ReviewThreadRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, room_id, scope, file_path, commit_sha, side, line_start, line_end, \
                        anchor_hash, anchor_lines, resolved_ms, created_ms, updated_ms \
                 FROM review_threads WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![thread_id], row_to_thread)
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(r) => r.map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    /// Every comment in the room, oldest first — the sibling bulk read
    /// to [`Database::review_threads_for_room`].
    pub fn review_comments_for_room(&self, room_id: &str) -> Result<Vec<ReviewCommentRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, thread_id, room_id, author_kind, author_id, body, \
                        created_ms, updated_ms \
                 FROM review_comments WHERE room_id = ?1 ORDER BY created_ms, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], row_to_comment)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn insert_review_comment(&self, c: &ReviewCommentRow) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_comments \
             (id, thread_id, room_id, author_kind, author_id, body, created_ms, updated_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                c.id,
                c.thread_id,
                c.room_id,
                c.author_kind,
                c.author_id,
                c.body,
                c.created_ms,
                c.updated_ms,
            ],
        )
        .map_err(|e| e.to_string())?;
        // Keep the thread's own timestamp meaningful — a reply is
        // activity on the thread, and the pane sorts unresolved threads
        // by it.
        conn.execute(
            "UPDATE review_threads SET updated_ms = ?2 WHERE id = ?1",
            params![c.thread_id, c.updated_ms],
        )
        .map_err(|e| e.to_string())?;
        // A human coming back to the thread withdraws any standing
        // "addressed" claim (#213). The agent said it was handled; the
        // user is still talking, so it evidently is not — and a stale
        // badge over live feedback is the sort of quiet lie that makes
        // the whole marker untrustworthy.
        if c.author_kind == "user" {
            conn.execute(
                "DELETE FROM review_addressed WHERE thread_id = ?1",
                params![c.thread_id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Edit a comment's text. Returns `false` when the comment is gone.
    pub fn update_review_comment(
        &self,
        comment_id: &str,
        body: &str,
        now_ms: i64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE review_comments SET body = ?2, updated_ms = ?3 WHERE id = ?1",
            params![comment_id, body, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.changes() > 0)
    }

    /// Delete one comment, and the thread with it when it was the last
    /// one. Returns the thread id if the whole thread went.
    ///
    /// An empty thread is not a thread — leaving one behind would put
    /// an anchor marker in the gutter with nothing to read under it.
    pub fn delete_review_comment(&self, comment_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        let thread_id: Option<String> = conn
            .query_row(
                "SELECT thread_id FROM review_comments WHERE id = ?1",
                params![comment_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(thread_id) = thread_id else {
            return Ok(None);
        };
        conn.execute(
            "DELETE FROM review_comments WHERE id = ?1",
            params![comment_id],
        )
        .map_err(|e| e.to_string())?;
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM review_comments WHERE thread_id = ?1",
                params![thread_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if remaining == 0 {
            conn.execute(
                "DELETE FROM review_threads WHERE id = ?1",
                params![thread_id],
            )
            .map_err(|e| e.to_string())?;
            return Ok(Some(thread_id));
        }
        Ok(None)
    }

    /// Delete a thread and every comment on it.
    pub fn delete_review_thread(&self, thread_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM review_comments WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM review_threads WHERE id = ?1",
            params![thread_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.changes() > 0)
    }

    /// Resolve (`Some(ts)`) or reopen (`None`) a thread.
    pub fn set_review_thread_resolved(
        &self,
        thread_id: &str,
        resolved_ms: Option<i64>,
        now_ms: i64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE review_threads SET resolved_ms = ?2, updated_ms = ?3 WHERE id = ?1",
            params![thread_id, resolved_ms, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.changes() > 0)
    }

    /// Re-record a thread's anchor after a successful re-match.
    ///
    /// Without this a thread would re-derive its position from
    /// ever-staler coordinates: each round of agent edits would search
    /// from where the comment was *originally* written rather than from
    /// where it was last seen, and the distance tie-break would decay
    /// into noise.
    pub fn update_review_thread_anchor(
        &self,
        thread_id: &str,
        line_start: i64,
        line_end: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE review_threads SET line_start = ?2, line_end = ?3 WHERE id = ?1",
            params![thread_id, line_start, line_end],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── the agent API (issue #213) ────────────────────────────────

    /// One room by id, or `None` when no such row exists.
    ///
    /// The agent API's only way into a room: a request carries a token
    /// and nothing else, so everything else it needs — the worktree,
    /// whether the room is archived, which harnesses are in it — comes
    /// from here.
    ///
    /// A blob that fails to parse is an error rather than a `None`.
    /// `load_all` owns quarantine; pretending the room is absent would
    /// turn a corrupt row into a plain 404 and hide it (#176).
    pub fn room_by_id(&self, room_id: &str) -> Result<Option<Room>, String> {
        let conn = self.conn.lock();
        let data: Option<String> = conn
            .query_row(
                "SELECT data FROM sessions WHERE id = ?1",
                params![room_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(data) = data else {
            return Ok(None);
        };
        serde_json::from_str::<Room>(&data)
            .map(Some)
            .map_err(|e| format!("room {room_id} is stored unparseably: {e}"))
    }

    /// The room's live bearer token, minting one the first time it is
    /// asked for. Idempotent: called on every harness spawn.
    pub fn ensure_room_token(&self, room_id: &str, now_ms: i64) -> Result<String, String> {
        let conn = self.conn.lock();
        let existing: Option<String> = conn
            .query_row(
                "SELECT token FROM agent_tokens \
                 WHERE room_id = ?1 AND revoked_ms IS NULL \
                 ORDER BY created_ms DESC LIMIT 1",
                params![room_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(token) = existing {
            return Ok(token);
        }
        // 256 bits from two v4 UUIDs rather than a `rand` dependency:
        // uuid already sources them from the OS CSPRNG, and this is the
        // only place in the tree that needs unguessable bytes.
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        conn.execute(
            "INSERT INTO agent_tokens (token, room_id, created_ms, revoked_ms) \
             VALUES (?1, ?2, ?3, NULL)",
            params![token, room_id, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(token)
    }

    /// Revoke live tokens — one room's, or every room's when `room_id`
    /// is `None`. Returns how many were revoked.
    ///
    /// Called with `None` on every boot. A token's lifetime is the
    /// lifetime of the Skein process that handed it out: PTYs die with
    /// the app, so nothing legitimate is still holding one, and a token
    /// that leaked into an old transcript or log stops working the next
    /// time Skein starts.
    pub fn revoke_agent_tokens(&self, room_id: Option<&str>, now_ms: i64) -> Result<usize, String> {
        let conn = self.conn.lock();
        match room_id {
            Some(id) => conn.execute(
                "UPDATE agent_tokens SET revoked_ms = ?2 \
                 WHERE room_id = ?1 AND revoked_ms IS NULL",
                params![id, now_ms],
            ),
            None => conn.execute(
                "UPDATE agent_tokens SET revoked_ms = ?1 WHERE revoked_ms IS NULL",
                params![now_ms],
            ),
        }
        .map_err(|e| e.to_string())
    }

    /// Resolve a bearer token. The three outcomes are deliberately
    /// distinct — "revoked" and "never existed" mean different things
    /// to whoever is holding it.
    pub fn room_for_token(&self, token: &str) -> Result<TokenLookup, String> {
        let conn = self.conn.lock();
        let row: Option<(String, Option<i64>)> = conn
            .query_row(
                "SELECT room_id, revoked_ms FROM agent_tokens WHERE token = ?1",
                params![token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(match row {
            None => TokenLookup::Unknown,
            Some((_, Some(_))) => TokenLookup::Revoked,
            Some((room_id, None)) => TokenLookup::Active { room_id },
        })
    }

    /// Record (or overwrite) an agent's claim that a thread is handled.
    pub fn set_thread_addressed(
        &self,
        room_id: &str,
        row: &ReviewAddressedRow,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_addressed \
             (thread_id, room_id, commit_sha, harness_id, note, addressed_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(thread_id) DO UPDATE SET \
               commit_sha = excluded.commit_sha, harness_id = excluded.harness_id, \
               note = excluded.note, addressed_ms = excluded.addressed_ms",
            params![
                row.thread_id,
                room_id,
                row.commit_sha,
                row.harness_id,
                row.note,
                row.addressed_ms,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn addressed_for_room(&self, room_id: &str) -> Result<Vec<ReviewAddressedRow>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, commit_sha, harness_id, note, addressed_ms \
                 FROM review_addressed WHERE room_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], row_to_addressed)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    // ── viewed markers and settings (issue #212) ──────────────────

    /// Mark `path` as looked at, at the content it currently holds.
    pub fn set_review_viewed(
        &self,
        room_id: &str,
        path: &str,
        content_hash: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_viewed (room_id, path, content_hash, viewed_ms) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(room_id, path) DO UPDATE SET \
               content_hash = excluded.content_hash, viewed_ms = excluded.viewed_ms",
            params![room_id, path, content_hash, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Un-mark `path`.
    pub fn clear_review_viewed(&self, room_id: &str, path: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM review_viewed WHERE room_id = ?1 AND path = ?2",
            params![room_id, path],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Every viewed marker in the room, as `(path, content_hash)`.
    pub fn review_viewed_for_room(&self, room_id: &str) -> Result<Vec<(String, String)>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT path, content_hash FROM review_viewed WHERE room_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![room_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// The room's chosen base ref, or `None` when it has never been set
    /// and the caller should fall back to the repo's own default.
    pub fn review_base_ref(&self, room_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT base_ref FROM review_settings WHERE room_id = ?1",
            params![room_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn set_review_base_ref(
        &self,
        room_id: &str,
        base_ref: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_settings (room_id, base_ref, updated_ms) VALUES (?1, ?2, ?3) \
             ON CONFLICT(room_id) DO UPDATE SET \
               base_ref = excluded.base_ref, updated_ms = excluded.updated_ms",
            params![room_id, base_ref, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── the reviewer's sign-off (#214) ────────────────────────────

    /// The room's sign-off, or `None` if the reviewer has not approved.
    ///
    /// Whether it is still *current* is not answered here: that needs
    /// HEAD, which is git's to say. `review_surface::signoff` compares
    /// the two.
    pub fn review_signoff(&self, room_id: &str) -> Result<Option<ReviewSignoffRow>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT room_id, head_sha, base_ref, note, approved_ms \
             FROM review_signoff WHERE room_id = ?1",
            params![room_id],
            |row| {
                Ok(ReviewSignoffRow {
                    room_id: row.get(0)?,
                    head_sha: row.get(1)?,
                    base_ref: row.get(2)?,
                    note: row.get(3)?,
                    approved_ms: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// Approve, or re-approve at a new HEAD. Replaces any prior row —
    /// a room has one sign-off, and the newest is the only one that
    /// means anything.
    pub fn set_review_signoff(
        &self,
        room_id: &str,
        head_sha: &str,
        base_ref: Option<&str>,
        note: Option<&str>,
        now_ms: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_signoff (room_id, head_sha, base_ref, note, approved_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(room_id) DO UPDATE SET \
               head_sha = excluded.head_sha, base_ref = excluded.base_ref, \
               note = excluded.note, approved_ms = excluded.approved_ms",
            params![room_id, head_sha, base_ref, note, now_ms],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Withdraw the sign-off. Deleting rather than flagging: "was
    /// approved once, at some sha, then withdrawn" is not a state
    /// anything acts on, and keeping it would invite something to.
    pub fn clear_review_signoff(&self, room_id: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM review_signoff WHERE room_id = ?1",
            params![room_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// A row of `review_signoff` — the reviewer said yes to `head_sha`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSignoffRow {
    pub room_id: String,
    pub head_sha: String,
    pub base_ref: Option<String>,
    pub note: Option<String>,
    pub approved_ms: i64,
}

/// One row of `review_threads` (issue #212).
///
/// The anchor fields are all `Option` because the scope decides which
/// apply: a review-level thread has none of them, a file thread has
/// `file_path`, and only a line thread carries a side and a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThreadRow {
    pub id: String,
    pub room_id: String,
    /// `review` | `commit` | `file` | `line` (D5).
    pub scope: String,
    pub file_path: Option<String>,
    pub commit_sha: Option<String>,
    /// `old` | `new` — which side of the diff a line thread hangs on.
    pub side: Option<String>,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub anchor_hash: Option<String>,
    /// JSON array of the lines the comment was written against.
    pub anchor_lines: Option<String>,
    pub resolved_ms: Option<i64>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

/// One row of `review_comments` (issue #212).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCommentRow {
    pub id: String,
    pub thread_id: String,
    pub room_id: String,
    /// `user` | `agent` (D7). v1 only ever writes `user`; #213 adds the
    /// other without touching the schema.
    pub author_kind: String,
    /// Harness id when `author_kind == "agent"`.
    pub author_id: Option<String>,
    pub body: String,
    pub created_ms: i64,
    pub updated_ms: i64,
}

fn row_to_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewThreadRow> {
    Ok(ReviewThreadRow {
        id: row.get(0)?,
        room_id: row.get(1)?,
        scope: row.get(2)?,
        file_path: row.get(3)?,
        commit_sha: row.get(4)?,
        side: row.get(5)?,
        line_start: row.get(6)?,
        line_end: row.get(7)?,
        anchor_hash: row.get(8)?,
        anchor_lines: row.get(9)?,
        resolved_ms: row.get(10)?,
        created_ms: row.get(11)?,
        updated_ms: row.get(12)?,
    })
}

fn row_to_comment(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewCommentRow> {
    Ok(ReviewCommentRow {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        room_id: row.get(2)?,
        author_kind: row.get(3)?,
        author_id: row.get(4)?,
        body: row.get(5)?,
        created_ms: row.get(6)?,
        updated_ms: row.get(7)?,
    })
}

/// One row of `review_addressed` (issue #213) — an agent's claim that
/// a comment has been dealt with, and what did it.
///
/// Not a resolution. Resolve stays human-only (D8), so this is the
/// agent's half of the handshake: it says "done, here is the sha", and
/// the user still decides whether the thread closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewAddressedRow {
    pub thread_id: String,
    pub commit_sha: Option<String>,
    /// Which harness made the claim.
    pub harness_id: String,
    pub note: Option<String>,
    pub addressed_ms: i64,
}

fn row_to_addressed(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewAddressedRow> {
    Ok(ReviewAddressedRow {
        thread_id: row.get(0)?,
        commit_sha: row.get(1)?,
        harness_id: row.get(2)?,
        note: row.get(3)?,
        addressed_ms: row.get(4)?,
    })
}

/// What a bearer token resolved to (issue #213).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenLookup {
    /// No such token was ever minted.
    Unknown,
    /// Minted, then rotated out from under the holder.
    Revoked,
    Active {
        room_id: String,
    },
}

/// One row of `review_baselines` (issue #211). `content` is `Some`
/// only when `kind == "text"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewBaseline {
    pub path: String,
    pub kind: String,
    pub content: Option<String>,
    /// Last harness to write this path — the Diff card's chip (D4).
    pub harness_id: String,
    pub captured_ms: i64,
    pub touched_ms: i64,
}

fn row_to_baseline(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewBaseline> {
    Ok(ReviewBaseline {
        path: row.get(0)?,
        kind: row.get(1)?,
        content: row.get(2)?,
        harness_id: row.get(3)?,
        captured_ms: row.get(4)?,
        touched_ms: row.get(5)?,
    })
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

#[cfg(test)]
mod review_baseline_tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn first_touch_captures_and_a_second_touch_does_not_move_the_baseline() {
        let (_d, db) = fresh();
        assert!(
            db.insert_review_baseline_if_absent("r1", "src/a.rs", "text", Some("v1\n"), "h1", 100)
                .unwrap()
        );
        // The agent writes again. Re-capturing here would silently
        // absorb the pending change — the exact failure #211 names.
        assert!(
            !db.insert_review_baseline_if_absent("r1", "src/a.rs", "text", Some("v2\n"), "h2", 200)
                .unwrap()
        );
        let b = db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
        assert_eq!(b.content.as_deref(), Some("v1\n"));
    }

    #[test]
    fn touch_updates_attribution_only() {
        let (_d, db) = fresh();
        db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("v1\n"), "h1", 100)
            .unwrap();
        db.touch_review_baseline("r1", "a.rs", "h2", 250).unwrap();
        let b = db.review_baseline("r1", "a.rs").unwrap().unwrap();
        assert_eq!(b.harness_id, "h2");
        assert_eq!(b.touched_ms, 250);
        assert_eq!(b.captured_ms, 100);
        assert_eq!(b.content.as_deref(), Some("v1\n"));
    }

    #[test]
    fn advance_moves_content_but_keeps_attribution() {
        let (_d, db) = fresh();
        db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("v1\n"), "h1", 100)
            .unwrap();
        db.advance_review_baseline("r1", "a.rs", "text", Some("v2\n"), 300)
            .unwrap();
        let b = db.review_baseline("r1", "a.rs").unwrap().unwrap();
        assert_eq!(b.content.as_deref(), Some("v2\n"));
        assert_eq!(b.harness_id, "h1", "accept must not erase who wrote it");
        assert_eq!(b.captured_ms, 300);
    }

    #[test]
    fn baselines_are_scoped_per_room_and_path_ordered() {
        let (_d, db) = fresh();
        db.insert_review_baseline_if_absent("r1", "z.rs", "text", Some("z"), "h1", 1)
            .unwrap();
        db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("a"), "h1", 1)
            .unwrap();
        db.insert_review_baseline_if_absent("r2", "other.rs", "text", Some("o"), "h9", 1)
            .unwrap();

        let r1 = db.review_baselines_for_room("r1").unwrap();
        assert_eq!(
            r1.iter().map(|b| b.path.as_str()).collect::<Vec<_>>(),
            ["a.rs", "z.rs"]
        );
        assert_eq!(db.review_baselines_for_room("r2").unwrap().len(), 1);
        assert!(db.review_baseline("r2", "a.rs").unwrap().is_none());
    }

    #[test]
    fn non_text_kinds_round_trip_with_a_null_content() {
        let (_d, db) = fresh();
        db.insert_review_baseline_if_absent("r1", "logo.png", "binary", None, "h1", 1)
            .unwrap();
        db.insert_review_baseline_if_absent("r1", "new.rs", "missing", None, "h1", 1)
            .unwrap();
        let b = db.review_baseline("r1", "logo.png").unwrap().unwrap();
        assert_eq!(b.kind, "binary");
        assert_eq!(b.content, None);
        assert_eq!(
            db.review_baseline("r1", "new.rs").unwrap().unwrap().kind,
            "missing"
        );
    }

    #[test]
    fn baselines_survive_a_restart() {
        // #211's mandatory property: an empty tracker after a reload
        // makes pending hunks "silently disappear — the user sees their
        // changes auto-applied".
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        {
            let db = Database::open(&path).unwrap();
            db.insert_review_baseline_if_absent("r1", "src/a.rs", "text", Some("v1\n"), "h1", 100)
                .unwrap();
            db.insert_review_baseline_if_absent(
                "r1",
                "src/b.rs",
                "text",
                Some("keep\n"),
                "h2",
                110,
            )
            .unwrap();
            db.advance_review_baseline("r1", "src/b.rs", "text", Some("accepted\n"), 200)
                .unwrap();
        }
        let db = Database::open(&path).unwrap();
        let rows = db.review_baselines_for_room("r1").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content.as_deref(), Some("v1\n"));
        assert_eq!(rows[0].harness_id, "h1");
        assert_eq!(rows[1].content.as_deref(), Some("accepted\n"));
    }

    #[test]
    fn crlf_and_unicode_content_survives_the_round_trip() {
        let (_d, db) = fresh();
        let content = "line\r\nnäst\r\n🧵 skein\r\n";
        db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some(content), "h1", 1)
            .unwrap();
        assert_eq!(
            db.review_baseline("r1", "a.rs")
                .unwrap()
                .unwrap()
                .content
                .as_deref(),
            Some(content)
        );
    }
}

#[cfg(test)]
mod review_comment_tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        (dir, db)
    }

    fn line_thread(id: &str, path: &str, start: i64, end: i64) -> ReviewThreadRow {
        ReviewThreadRow {
            id: id.into(),
            room_id: "r1".into(),
            scope: "line".into(),
            file_path: Some(path.into()),
            commit_sha: None,
            side: Some("new".into()),
            line_start: Some(start),
            line_end: Some(end),
            anchor_hash: Some("deadbeef".into()),
            anchor_lines: Some(r#"["let x = 1;"]"#.into()),
            resolved_ms: None,
            created_ms: 100,
            updated_ms: 100,
        }
    }

    fn comment(id: &str, thread_id: &str, body: &str, ms: i64) -> ReviewCommentRow {
        ReviewCommentRow {
            id: id.into(),
            thread_id: thread_id.into(),
            room_id: "r1".into(),
            author_kind: "user".into(),
            author_id: None,
            body: body.into(),
            created_ms: ms,
            updated_ms: ms,
        }
    }

    #[test]
    fn a_thread_round_trips_with_its_anchor_intact() {
        let (_d, db) = fresh();
        let t = line_thread("t1", "src/a.rs", 12, 14);
        db.insert_review_thread(&t).unwrap();
        assert_eq!(db.review_thread("t1").unwrap().as_ref(), Some(&t));
        assert_eq!(db.review_threads_for_room("r1").unwrap(), vec![t]);
        // Another room's review is not this one's.
        assert!(db.review_threads_for_room("r2").unwrap().is_empty());
    }

    #[test]
    fn replies_are_rows_on_the_same_thread_in_order() {
        // D5: flat comments with replies. A reply has nowhere to nest.
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        db.insert_review_comment(&comment("c1", "t1", "why this?", 100))
            .unwrap();
        db.insert_review_comment(&comment("c2", "t1", "because X", 200))
            .unwrap();
        let all = db.review_comments_for_room("r1").unwrap();
        let bodies: Vec<&str> = all.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(bodies, vec!["why this?", "because X"]);
    }

    #[test]
    fn a_comment_carries_its_author_from_the_first_migration() {
        // v1 only writes `user`, but #213's agent replies must be a row
        // change and not a migration (D7).
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        let mut agent = comment("c1", "t1", "addressed in 9531fc3", 100);
        agent.author_kind = "agent".into();
        agent.author_id = Some("h-claude-1".into());
        db.insert_review_comment(&agent).unwrap();
        let back = &db.review_comments_for_room("r1").unwrap()[0];
        assert_eq!(back.author_kind, "agent");
        assert_eq!(back.author_id.as_deref(), Some("h-claude-1"));
    }

    #[test]
    fn a_reply_bumps_the_thread_but_not_its_creation_time() {
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        db.insert_review_comment(&comment("c1", "t1", "first", 500))
            .unwrap();
        let t = db.review_thread("t1").unwrap().unwrap();
        assert_eq!(t.created_ms, 100);
        assert_eq!(t.updated_ms, 500);
    }

    #[test]
    fn deleting_the_last_comment_takes_the_thread_with_it() {
        // An empty thread would leave an anchor marker in the gutter
        // with nothing to read under it.
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        db.insert_review_comment(&comment("c1", "t1", "one", 100))
            .unwrap();
        db.insert_review_comment(&comment("c2", "t1", "two", 200))
            .unwrap();

        assert_eq!(db.delete_review_comment("c2").unwrap(), None);
        assert!(db.review_thread("t1").unwrap().is_some());

        assert_eq!(
            db.delete_review_comment("c1").unwrap().as_deref(),
            Some("t1")
        );
        assert!(db.review_thread("t1").unwrap().is_none());
    }

    #[test]
    fn deleting_a_comment_that_is_already_gone_is_not_an_error() {
        let (_d, db) = fresh();
        assert_eq!(db.delete_review_comment("nope").unwrap(), None);
    }

    #[test]
    fn deleting_a_thread_takes_every_comment_on_it() {
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        db.insert_review_comment(&comment("c1", "t1", "one", 100))
            .unwrap();
        db.insert_review_comment(&comment("c2", "t1", "two", 200))
            .unwrap();
        assert!(db.delete_review_thread("t1").unwrap());
        assert!(db.review_comments_for_room("r1").unwrap().is_empty());
    }

    #[test]
    fn resolve_and_reopen_are_both_reachable() {
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        assert!(db.set_review_thread_resolved("t1", Some(900), 900).unwrap());
        assert_eq!(
            db.review_thread("t1").unwrap().unwrap().resolved_ms,
            Some(900)
        );
        assert!(db.set_review_thread_resolved("t1", None, 950).unwrap());
        assert_eq!(db.review_thread("t1").unwrap().unwrap().resolved_ms, None);
        // A thread that no longer exists reports that rather than lying.
        assert!(!db.set_review_thread_resolved("gone", Some(1), 1).unwrap());
    }

    #[test]
    fn editing_a_comment_reports_whether_it_landed() {
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
            .unwrap();
        db.insert_review_comment(&comment("c1", "t1", "typo", 100))
            .unwrap();
        assert!(db.update_review_comment("c1", "fixed", 300).unwrap());
        let back = &db.review_comments_for_room("r1").unwrap()[0];
        assert_eq!(back.body, "fixed");
        assert_eq!(back.updated_ms, 300);
        assert_eq!(back.created_ms, 100, "creation time is not an edit time");
        assert!(!db.update_review_comment("gone", "x", 1).unwrap());
    }

    #[test]
    fn re_anchoring_stores_the_new_coordinates() {
        // A thread must search from where it was last seen, not from
        // where it was first written, or the distance tie-break decays
        // into noise over a multi-round review.
        let (_d, db) = fresh();
        db.insert_review_thread(&line_thread("t1", "a.rs", 12, 14))
            .unwrap();
        db.update_review_thread_anchor("t1", 40, 42).unwrap();
        let t = db.review_thread("t1").unwrap().unwrap();
        assert_eq!((t.line_start, t.line_end), (Some(40), Some(42)));
        assert_eq!(
            t.anchor_lines.as_deref(),
            Some(r#"["let x = 1;"]"#),
            "the anchor text itself never moves — only its coordinates"
        );
    }

    #[test]
    fn a_review_level_thread_carries_no_anchor_at_all() {
        let (_d, db) = fresh();
        let t = ReviewThreadRow {
            id: "t1".into(),
            room_id: "r1".into(),
            scope: "review".into(),
            file_path: None,
            commit_sha: None,
            side: None,
            line_start: None,
            line_end: None,
            anchor_hash: None,
            anchor_lines: None,
            resolved_ms: None,
            created_ms: 1,
            updated_ms: 1,
        };
        db.insert_review_thread(&t).unwrap();
        assert_eq!(db.review_thread("t1").unwrap(), Some(t));
    }

    #[test]
    fn threads_and_comments_survive_reopening_the_database() {
        // Persistence is the point: a review that evaporates on restart
        // is worse than none, because the user believes it is recorded.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        {
            let db = Database::open(&path).unwrap();
            db.insert_review_thread(&line_thread("t1", "a.rs", 3, 3))
                .unwrap();
            db.insert_review_comment(&comment("c1", "t1", "rename this", 100))
                .unwrap();
        }
        let db = Database::open(&path).unwrap();
        assert_eq!(db.review_threads_for_room("r1").unwrap().len(), 1);
        assert_eq!(
            db.review_comments_for_room("r1").unwrap()[0].body,
            "rename this"
        );
    }

    // ── viewed markers ────────────────────────────────────────────

    #[test]
    fn a_viewed_marker_records_what_was_looked_at_not_merely_that_it_was() {
        let (_d, db) = fresh();
        db.set_review_viewed("r1", "a.rs", "hash-v1", 100).unwrap();
        assert_eq!(
            db.review_viewed_for_room("r1").unwrap(),
            vec![("a.rs".to_string(), "hash-v1".to_string())]
        );
        // Looking again at newer content replaces the mark.
        db.set_review_viewed("r1", "a.rs", "hash-v2", 200).unwrap();
        assert_eq!(
            db.review_viewed_for_room("r1").unwrap(),
            vec![("a.rs".to_string(), "hash-v2".to_string())]
        );
        db.clear_review_viewed("r1", "a.rs").unwrap();
        assert!(db.review_viewed_for_room("r1").unwrap().is_empty());
    }

    // ── base ref ──────────────────────────────────────────────────

    #[test]
    fn the_base_ref_is_unset_until_chosen_and_then_sticks() {
        let (_d, db) = fresh();
        assert_eq!(
            db.review_base_ref("r1").unwrap(),
            None,
            "unset means fall back to the repo own guess"
        );
        db.set_review_base_ref("r1", "main", 100).unwrap();
        assert_eq!(db.review_base_ref("r1").unwrap().as_deref(), Some("main"));
        // A stacked branch points at the branch below it.
        db.set_review_base_ref("r1", "feat/211-review-baseline", 200)
            .unwrap();
        assert_eq!(
            db.review_base_ref("r1").unwrap().as_deref(),
            Some("feat/211-review-baseline")
        );
        assert_eq!(db.review_base_ref("r2").unwrap(), None);
    }
}
