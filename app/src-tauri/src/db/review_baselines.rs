//! The `review_baselines` rows and their image mirror. Split out of db.rs (#454).

use rusqlite::{OptionalExtension, params};

use super::{Database, MAX_ROOM_IMAGE_MIRROR_BYTES};

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

impl Database {
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

    /// Mirror (or clear) `review_baseline_images` for one baseline
    /// (#409). Every writer of a `review_baselines` row calls this
    /// alongside it: `Some(bytes)` upserts a mirror of the baseline's
    /// raw content for an image path Skein classified as `Binary` or
    /// `TooLarge`; `None` deletes any row that might be there — a
    /// `Text`/`Missing`/`Symlink`/`Unreadable` baseline, a non-image
    /// path, or bytes over the cap all have nothing worth keeping here.
    pub fn set_review_baseline_image(
        &self,
        room_id: &str,
        path: &str,
        bytes: Option<&[u8]>,
    ) -> Result<(), String> {
        self.set_review_baseline_image_capped(room_id, path, bytes, MAX_ROOM_IMAGE_MIRROR_BYTES)
    }

    /// [`Self::set_review_baseline_image`] with the per-room cap taken
    /// as a parameter, so a test can exercise the cap without writing
    /// 128 MiB (the `read_image_bytes_impl` pattern in `fs.rs`).
    pub(super) fn set_review_baseline_image_capped(
        &self,
        room_id: &str,
        path: &str,
        bytes: Option<&[u8]>,
        max_room_bytes: u64,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock();
        match bytes {
            Some(b) => {
                let tx = conn.transaction().map_err(|e| e.to_string())?;
                // Every OTHER mirror already stored for this room — the
                // path being written doesn't count against itself, so
                // replacing an existing mirror with a same-size blob at
                // the cap still succeeds.
                let other_bytes: i64 = tx
                    .query_row(
                        "SELECT COALESCE(SUM(length(bytes)), 0) FROM review_baseline_images \
                         WHERE room_id = ?1 AND path != ?2",
                        params![room_id, path],
                        |row| row.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                let other_bytes = u64::try_from(other_bytes).unwrap_or(0);
                if other_bytes.saturating_add(b.len() as u64) > max_room_bytes {
                    // Over the cap: drop whatever mirror this path had
                    // rather than keep a stale one — an old "before"
                    // image is worse than none, since it would show the
                    // wrong diff instead of falling back honestly.
                    tracing::info!(
                        room_id,
                        path,
                        other_bytes,
                        new_bytes = b.len(),
                        cap_bytes = max_room_bytes,
                        "review baseline image mirror over per-room cap; dropping mirror"
                    );
                    tx.execute(
                        "DELETE FROM review_baseline_images WHERE room_id = ?1 AND path = ?2",
                        params![room_id, path],
                    )
                } else {
                    tx.execute(
                        "INSERT INTO review_baseline_images (room_id, path, bytes) \
                         VALUES (?1, ?2, ?3) \
                         ON CONFLICT(room_id, path) DO UPDATE SET bytes = excluded.bytes",
                        params![room_id, path, b],
                    )
                }
                .map_err(|e| e.to_string())?;
                tx.commit().map_err(|e| e.to_string())?;
            }
            None => {
                conn.execute(
                    "DELETE FROM review_baseline_images WHERE room_id = ?1 AND path = ?2",
                    params![room_id, path],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// The mirrored bytes for a baseline, when
    /// [`Self::set_review_baseline_image`] kept one. `None` when there
    /// never was one — the pending image lookup
    /// (`review_surface::image`) falls back from there.
    pub fn review_baseline_image(
        &self,
        room_id: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT bytes FROM review_baseline_images WHERE room_id = ?1 AND path = ?2",
            params![room_id, path],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
