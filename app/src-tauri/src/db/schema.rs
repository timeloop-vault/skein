//! The idempotent schema setup (`Database::init_schema`). Split out of db.rs (#454).

use rusqlite::Connection;

use super::{Database, element_anchors, element_proposals};

impl Database {
    /// Idempotent schema setup. Each table uses `IF NOT EXISTS`; new
    /// tables just get added here without a separate migration step.
    /// At prototype scale this is sufficient — once columns need to
    /// be altered (vs added) we'll need a version table.
    ///
    /// A new table with a `room_id` column also needs adding to
    /// `ROOM_KEYED_TABLES` (#237), or `sweep_orphans` will never clean
    /// it up after its room is deleted forever.
    pub(super) fn init_schema(conn: &Connection) -> Result<(), String> {
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

        // Issue #409: the raw bytes behind a `Binary`/`TooLarge`
        // baseline on an image path. `review_baselines.content` only
        // ever holds text (a `Text` baseline — SVG included, since it's
        // UTF-8 — already carries its bytes there); a raster image's
        // baseline keeps nothing but a digest or a length, which is
        // enough to detect a change but not enough to render one. A
        // sibling table rather than widening `content` to a BLOB: every
        // other kind still has nothing to store, and this only ever
        // exists for the minority of baselines that are both non-text
        // and a known image extension.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS review_baseline_images (
                room_id TEXT NOT NULL,
                path TEXT NOT NULL,
                bytes BLOB NOT NULL,
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

        element_anchors::init_schema(conn)?;
        element_proposals::init_schema(conn)?;

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

        // Issue #327: the per-harness mailbox. `room_id`/`harness_id`
        // are the *recipient* — named plainly, not `to_*`, so this
        // table fits `ROOM_KEYED_TABLES`/`sweep_orphans` (#237) without
        // a special case: that sweep deletes by a column literally
        // called `room_id`. The sender travels as `from_room_id` /
        // `from_harness_id` for attribution only — a message survives
        // its sender room being deleted forever, the same way a review
        // comment survives the room that wrote it.
        //
        // `from_harness_id` is nullable: a caller can send with no
        // `X-Skein-Harness`, same as a review reply.
        //
        // No foreign keys, matching every other table here: closing a
        // room archives it rather than deleting it, so there is no
        // cascade to model.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS harness_messages (
                id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                harness_id TEXT NOT NULL,
                from_room_id TEXT NOT NULL,
                from_harness_id TEXT,
                body TEXT NOT NULL,
                created_ms INTEGER NOT NULL,
                read_ms INTEGER
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_messages_inbox \
             ON harness_messages(room_id, harness_id, read_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_harness_messages_sender \
             ON harness_messages(from_room_id, created_ms)",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests;
