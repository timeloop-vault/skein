//! Review threads, comments, addressed marks, viewed marks, base ref and sign-off. Split out of db.rs (#454).

use rusqlite::{OptionalExtension, params};

use super::{Database, element_anchors};

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

impl Database {
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
            element_anchors::delete_thread(&conn, &thread_id)?;
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
        element_anchors::delete_thread(&conn, thread_id)?;
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

#[cfg(test)]
mod tests;
