//! The room model (`Room`, `Harness`, …) and its load/save methods. Split out of db.rs (#454).

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::Database;

#[cfg(test)]
use skein_proto::rooms::{ClosedBy, HarnessCreatedBy, OpencodeVersion, RemoteSpec, ShellClaim};
pub use skein_proto::rooms::{CreatedBy, Harness, RepoIdentity, Room, SkippedRoom};

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

impl Database {
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
            if let Ok(parsed) = serde_json::from_str::<CwdOnly>(&data)
                && let Some(cwd) = parsed.cwd
            {
                out.push(cwd);
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

    /// Seq-aware wrapper around `save_all` (#171). `db_save_rooms` is
    /// async now, and the frontend fires it un-debounced on every
    /// `rooms` change without awaiting the previous call — so a save
    /// minted *before* another can still reach this lock *after* it,
    /// and a plain `save_all` would let the older, stale room list
    /// silently overwrite the newer one that already committed.
    ///
    /// `seq` is a ticket from `next_save_seq`, minted by the command
    /// before its blocking work starts. Holding `last_saved_seq` for
    /// the whole check-then-write serializes every seq-aware save
    /// against every other one, so a save whose ticket is older than
    /// the last one committed is dropped — the newer room list wins
    /// and the #167 empty-save refusal inside `save_all` still applies
    /// unchanged. This reliably fixes *completion*-order inversion —
    /// two overlapping saves where the older one's blocking work
    /// finishes last, which is the realistic case a work-stealing
    /// runtime produces. It does not guarantee *mint*-order matches
    /// dispatch order (see `next_save_seq`); that residual window is a
    /// few instructions wide and accepted.
    pub fn save_all_seq(&self, rooms: &[Room], seq: u64) -> Result<(), String> {
        let mut last_seq = self.last_saved_seq.lock();
        if seq < *last_seq {
            // A newer save already committed; applying this one would
            // silently revert it.
            return Ok(());
        }
        self.save_all(rooms)?;
        *last_seq = seq;
        Ok(())
    }

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

    /// Every parseable room, active and archived. Unlike `load_all`
    /// this never quarantines — a mailbox lookup that hit a corrupt row
    /// should simply not find that room, not mutate the table on the
    /// side of an unrelated `send_message` call.
    pub fn all_rooms(&self) -> Result<Vec<Room>, String> {
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
            if let Ok(room) = serde_json::from_str::<Room>(&data) {
                out.push(room);
            }
        }
        Ok(out)
    }

    /// Corrupt an already-saved room's blob in place, so it fails to
    /// parse as a `Room` — for tests, in this crate, that need a room
    /// Skein holds but cannot read (e.g. `agent_api`'s
    /// `find_rooms_for_path` tests). `conn` is private, so this is the
    /// only way another module can put a row into that state.
    #[cfg(any(test, feature = "test-support"))]
    pub fn corrupt_room_for_test(&self, id: &str) {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE sessions SET data = 'not json' WHERE id = ?1",
            params![id],
        )
        .unwrap();
    }

    /// Rooms Skein holds but cannot currently produce as a `Room`: a
    /// `sessions` row whose blob fails to parse (not yet quarantined —
    /// `all_rooms` never quarantines, see above) plus every row already
    /// parked in `sessions_quarantine`. A caller deciding whether a path
    /// is safe to remove must see this and refuse to trust a zero-match
    /// `all_rooms` result on its own.
    pub fn unreadable_room_count(&self) -> Result<u32, String> {
        let conn = self.conn.lock();
        let unparsed: usize = {
            let mut stmt = conn
                .prepare("SELECT data FROM sessions")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            let mut count = 0usize;
            for row in rows {
                let data = row.map_err(|e| e.to_string())?;
                if serde_json::from_str::<Room>(&data).is_err() {
                    count += 1;
                }
            }
            count
        };
        let quarantined: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions_quarantine", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        let quarantined = u32::try_from(quarantined).unwrap_or(u32::MAX);
        Ok(u32::try_from(unparsed)
            .unwrap_or(u32::MAX)
            .saturating_add(quarantined))
    }
}

#[cfg(test)]
mod tests;
