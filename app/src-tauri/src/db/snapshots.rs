//! The `.bak` last-known-good snapshots and the file replace they use. Split out of db.rs (#454).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};

use super::Database;

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

impl Database {
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
        // Strip the image mirror from the snapshot (#409 follow-up): a
        // mirror is a rebuildable preview cache, not state worth paying
        // for twice on every backup generation. A restored `.bak` falls
        // back to the HEAD-blob digest / "unavailable" path exactly like
        // a room whose baseline was never mirrored. A failure here must
        // fail the whole backup rather than promote a half-cleaned temp
        // file to `.bak` — returning early leaves `tmp` in place for the
        // next call's cleanup, same as a failed VACUUM INTO above.
        {
            let tmp_conn = Connection::open(&tmp).map_err(|e| e.to_string())?;
            tmp_conn
                .execute("DELETE FROM review_baseline_images", [])
                .map_err(|e| e.to_string())?;
            // VACUUM (not just DELETE) so the freed pages are actually
            // released — VACUUM INTO already produced a compact copy,
            // but the DELETE above reintroduces free pages of its own.
            tmp_conn.execute("VACUUM", []).map_err(|e| e.to_string())?;
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
}

#[cfg(test)]
mod tests;
