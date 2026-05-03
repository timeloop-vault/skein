//! Chapter 5 — transparent harness resume.
//!
//! Query helpers against the underlying tools' on-disk session storage
//! so Skein can capture and validate session ids. See
//! `docs/chapter-5-recon.md` for the storage layouts.

use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};

/// Path to opencode's on-disk session db. Returns `None` when `HOME`
/// isn't set — exotic environments (some CI / sandboxes) — in which
/// case the caller treats it the same as "db doesn't exist."
fn opencode_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db")
    })
}

/// IDs of all non-archived opencode sessions whose `directory` matches
/// `cwd`, newest first. Empty vec when the db doesn't exist (user has
/// never run opencode) or `HOME` isn't set.
///
/// Opens the db read-only so this command never contends with opencode
/// itself for the writer lock.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn opencode_list_sessions(cwd: String) -> Result<Vec<String>, String> {
    let Some(db_path) = opencode_db_path() else {
        return Ok(Vec::new());
    };
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open opencode.db: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id FROM session \
             WHERE directory = ?1 AND time_archived IS NULL \
             ORDER BY time_created DESC",
        )
        .map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map([&cwd], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row: {e}"))
}
