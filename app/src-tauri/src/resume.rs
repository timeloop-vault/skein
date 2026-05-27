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
pub(crate) fn opencode_db_path() -> Option<PathBuf> {
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

/// Phase 4: does the opencode session row for `id` still exist (and
/// is non-archived)? Used at boot to drop stale captured ids before
/// resume tries to use them.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn opencode_session_exists(id: String) -> Result<bool, String> {
    let Some(db_path) = opencode_db_path() else {
        return Ok(false);
    };
    if !db_path.exists() {
        return Ok(false);
    }
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open opencode.db: {e}"))?;
    let mut stmt = conn
        .prepare("SELECT 1 FROM session WHERE id = ?1 AND time_archived IS NULL LIMIT 1")
        .map_err(|e| format!("prepare: {e}"))?;
    stmt.exists([&id]).map_err(|e| format!("exists: {e}"))
}

/// Phase 4: does Claude still have a session file for this id?
///
/// Walks `~/.claude/projects/*/` looking for `<id>.jsonl`. We don't
/// recompute the `<encoded-cwd>` directory ourselves — Claude's
/// path-encoding scheme is lossy (recon §3) and a glob over the
/// project dirs is robust against future encoding changes. The 17 or
/// so project dirs on a typical machine make this a sub-millisecond
/// scan; even at 200+ dirs it's still trivial.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn claude_session_exists(id: String) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let projects_dir = PathBuf::from(home).join(".claude").join("projects");
    let filename = format!("{id}.jsonl");
    let Ok(entries) = std::fs::read_dir(&projects_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join(&filename).exists() {
            return true;
        }
    }
    false
}
