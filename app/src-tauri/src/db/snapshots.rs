//! The `.bak` last-known-good snapshots and the file replace they use. Split out of db.rs (#454).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};

use super::Database;

/// Tables the `.bak` snapshot copies (#167, #658). Each one is state a
/// user cannot get back if `skein.db` is lost or wiped. **Every new
/// table must be listed here or in [`SNAPSHOT_EXCLUDED`], never in
/// neither**: a forgotten table is silently unprotected.
pub(super) const SNAPSHOT_TABLES: &[&str] = &[
    "sessions",
    "sessions_quarantine",
    "review_threads",
    "review_comments",
    "review_addressed",
    "review_signoff",
    "review_settings",
    "review_viewed",
    "review_element_anchors",
    "review_element_proposals",
    "global_todos",
];

/// Tables the snapshot deliberately leaves empty (their schema is still
/// created, so the `.bak` opens). Every new table goes in exactly one of
/// this list and [`SNAPSHOT_TABLES`].
pub(super) const SNAPSHOT_EXCLUDED: &[&str] = &[
    // Logs, rebuilt from the harnesses' own transcripts.
    "harness_events",
    "harness_actions",
    // A missing baseline over-reports (reads as fully added), the safe
    // direction (#211).
    "review_baselines",
    // The image mirror is a rebuildable preview cache.
    "review_baseline_images",
    // Mail is history, not state.
    "harness_messages",
    // Tokens are revoked on every boot.
    "agent_tokens",
];

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

/// A read-only `file:` URI for `path`, for `ATTACH`. Backslashes become
/// forward slashes, a Windows drive letter gets a leading `/`, a
/// verbatim `\\?\` prefix is dropped, and everything but unreserved
/// characters, `/` and `:` is percent-encoded, so `%`, `?`, `#` and
/// spaces in the path can't be read as URI syntax.
fn readonly_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let raw = match raw.strip_prefix("//?/") {
        Some(rest) => match rest.strip_prefix("UNC/") {
            Some(unc) => format!("//{unc}"),
            None => rest.to_string(),
        },
        None => raw,
    };
    let bytes = raw.as_bytes();
    let mut uri = String::from("file:");
    if bytes.get(1) == Some(&b':') && bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        uri.push('/');
    } else if raw.starts_with("//") {
        // UNC: SQLite rejects a non-localhost authority, so keep the
        // server in the path (`file:////server/share/...`).
        uri.push_str("//");
    }
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            uri.push(char::from(b));
        } else {
            let _ = write!(uri, "%{b:02X}");
        }
    }
    uri.push_str("?mode=ro");
    uri
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// True when the attached `bak` already holds exactly the protected data
/// of the attached `src`. Any error (a table or column missing from an
/// old snapshot) counts as changed. `EXCEPT` has set semantics, so equal
/// row counts plus both differences empty is what rules out a changed
/// row; only a swap of duplicate-row multiplicities could slip through,
/// and every protected table except `sessions_quarantine` has a key.
fn snapshot_is_current(conn: &Connection, tables: &[String]) -> bool {
    let check = || -> rusqlite::Result<bool> {
        // A legacy full snapshot carries history we no longer keep;
        // replacing it once frees that space.
        for t in SNAPSHOT_EXCLUDED {
            let q = format!("SELECT EXISTS (SELECT 1 FROM bak.{})", quoted(t));
            if conn.query_row(&q, [], |r| r.get::<_, bool>(0))? {
                return Ok(false);
            }
        }
        for t in tables {
            let t = quoted(t);
            let count = |db: &str| -> rusqlite::Result<i64> {
                conn.query_row(&format!("SELECT COUNT(*) FROM {db}.{t}"), [], |r| r.get(0))
            };
            if count("src")? != count("bak")? {
                return Ok(false);
            }
            let diff: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM (SELECT * FROM src.{t} EXCEPT SELECT * FROM bak.{t})"
                ),
                [],
                |r| r.get(0),
            )?;
            if diff != 0 {
                return Ok(false);
            }
            let rdiff: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM (SELECT * FROM bak.{t} EXCEPT SELECT * FROM src.{t})"
                ),
                [],
                |r| r.get(0),
            )?;
            if rdiff != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    };
    check().unwrap_or(false)
}

impl Database {
    /// Snapshot the protected tables to `<db>.bak` (issue #167, #658).
    /// The caller decides *when* — the policy is "first clean,
    /// non-empty load of the process", so `.bak` always holds a
    /// last-known-good state and is never overwritten by a wipe or a
    /// quarantine-marred load.
    ///
    /// **An allowlist, not the whole file.** The DB is dominated by
    /// history #167 never meant to protect (logs, baselines, the image
    /// mirror), and copying it all cost seconds and gigabytes of I/O per
    /// boot. Only [`SNAPSHOT_TABLES`] are copied; [`SNAPSHOT_EXCLUDED`]
    /// names what is left out and why. The `.bak` still carries the full
    /// schema, so it opens and restores like any other skein.db.
    ///
    /// **Off the main lock.** This never takes `self.conn`. A fresh
    /// connection on the temp file attaches the live DB read-only and
    /// copies inside one transaction: under WAL a reader sees one
    /// consistent snapshot while writers carry on, and it pins the WAL
    /// only for the length of the copy.
    ///
    /// **Unchanged data skips rotation.** If the existing `.bak` already
    /// holds the same rows (see [`snapshot_is_current`]), nothing is
    /// touched, so `.bak.1` is not pushed out by an identical copy.
    ///
    /// Write order matters (#167 review): the snapshot lands in a temp
    /// file first and only replaces `.bak` once complete, so a failed
    /// or interrupted copy (disk full, crash) can't destroy the
    /// previous good snapshot. The displaced `.bak` is kept one more
    /// generation as `.bak.1` — the "rooms silently vanished, then one
    /// clean boot refreshed the backup" sequence stays recoverable.
    pub fn backup_last_known_good(&self) -> Result<PathBuf, String> {
        let dest = self.path.with_extension("db.bak");
        let prev = self.path.with_extension("db.bak.1");
        let tmp = self.path.with_extension("db.bak.tmp");
        if tmp.exists() {
            std::fs::remove_file(&tmp).map_err(|e| e.to_string())?;
        }
        let mut tmp_journal = tmp.clone().into_os_string();
        tmp_journal.push("-journal");
        let tmp_journal = PathBuf::from(tmp_journal);
        if tmp_journal.exists() {
            std::fs::remove_file(&tmp_journal).map_err(|e| e.to_string())?;
        }
        if !self.copy_snapshot(&tmp, &dest)? {
            let _ = std::fs::remove_file(&tmp);
            tracing::info!("db snapshot skipped: protected data unchanged since .bak");
            return Ok(dest);
        }
        if dest.exists() {
            replace_file(&dest, &prev)?;
        }
        replace_file(&tmp, &dest)?;
        Ok(dest)
    }

    /// Build the snapshot at `tmp`. `Ok(false)` means `bak` is already
    /// current and `tmp` holds nothing worth keeping.
    fn copy_snapshot(&self, tmp: &Path, bak: &Path) -> Result<bool, String> {
        let err = |e: rusqlite::Error| e.to_string();
        let conn = Connection::open_with_flags(
            tmp,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(err)?;
        // ATTACH is not allowed inside a transaction, so attach first;
        // `src` is only read once BEGIN has run.
        conn.execute(
            "ATTACH DATABASE ?1 AS src",
            params![readonly_uri(&self.path)],
        )
        .map_err(err)?;
        let mut have_bak = bak.exists();
        if have_bak
            && let Err(e) = conn.execute("ATTACH DATABASE ?1 AS bak", params![readonly_uri(bak)])
        {
            // An unreadable .bak must not stop backups on every launch:
            // treat it as absent so the rotation overwrites it.
            tracing::warn!("db snapshot: cannot attach existing .bak, replacing it: {e}");
            have_bak = false;
        }
        // One transaction: `src` is read from a single WAL snapshot.
        conn.execute_batch("BEGIN").map_err(err)?;
        let result = (|| -> Result<bool, String> {
            let mut ddl: Vec<String> = Vec::new();
            let mut live: Vec<String> = Vec::new();
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT type, name, sql FROM src.sqlite_master \
                         WHERE type IN ('table', 'index') AND sql IS NOT NULL \
                         AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
                         ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, rowid",
                    )
                    .map_err(err)?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(err)?;
                for row in rows {
                    let (kind, name, sql) = row.map_err(err)?;
                    if kind == "table" && SNAPSHOT_TABLES.contains(&name.as_str()) {
                        live.push(name);
                    }
                    ddl.push(sql);
                }
            }
            if have_bak && snapshot_is_current(&conn, &live) {
                return Ok(false);
            }
            for sql in &ddl {
                conn.execute_batch(sql).map_err(err)?;
            }
            for t in &live {
                let t = quoted(t);
                conn.execute_batch(&format!("INSERT INTO main.{t} SELECT * FROM src.{t}"))
                    .map_err(err)?;
            }
            Ok(true)
        })();
        let changed = result?;
        conn.execute_batch(if changed { "COMMIT" } else { "ROLLBACK" })
            .map_err(err)?;
        if have_bak {
            let _ = conn.execute_batch("DETACH DATABASE bak");
        }
        let _ = conn.execute_batch("DETACH DATABASE src");
        conn.close().map_err(|(_, e)| e.to_string())?;
        Ok(changed)
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
