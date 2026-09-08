//! Chapter 5 — transparent harness resume.
//!
//! Tauri commands that probe the underlying tools' on-disk session
//! storage so Skein can capture and validate session ids. The
//! storage layouts and queries themselves live in `skein-harness`
//! (#209); this module is the command boundary, collapsing errors to
//! `String` and treating "no home dir" / "no db" as empty.

use std::path::PathBuf;

use skein_harness::{claude, opencode};

/// Path to opencode's on-disk session db. Returns `None` when the home
/// dir can't be resolved — exotic environments (some CI / sandboxes) —
/// in which case the caller treats it the same as "db doesn't exist."
pub(crate) fn opencode_db_path() -> Option<PathBuf> {
    crate::home_dir().map(|home| opencode::db_path(&home))
}

/// Open opencode's db read-only, or `None` when it isn't there (the
/// user has never run opencode) or `HOME` isn't set.
fn open_opencode_db() -> Result<Option<rusqlite::Connection>, String> {
    let Some(db_path) = opencode_db_path() else {
        return Ok(None);
    };
    if !db_path.exists() {
        return Ok(None);
    }
    opencode::open_read_only(&db_path)
        .map(Some)
        .map_err(|e| format!("open opencode.db: {e}"))
}

/// IDs of all non-archived opencode sessions whose `directory` matches
/// `cwd`, newest first. Empty vec when the db doesn't exist or `HOME`
/// isn't set.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn opencode_list_sessions(cwd: String) -> Result<Vec<String>, String> {
    let Some(conn) = open_opencode_db()? else {
        return Ok(Vec::new());
    };
    opencode::session_ids_for_directory(&conn, &cwd).map_err(|e| format!("query: {e}"))
}

/// Phase 4: does the opencode session row for `id` still exist (and
/// is non-archived)? Used at boot to drop stale captured ids before
/// resume tries to use them.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn opencode_session_exists(id: String) -> Result<bool, String> {
    let Some(conn) = open_opencode_db()? else {
        return Ok(false);
    };
    opencode::session_exists(&conn, &id).map_err(|e| format!("exists: {e}"))
}

/// Phase 4: does Claude still have a session file for this id? Scans
/// every project dir rather than recomputing the lossy cwd encoding —
/// see `skein_harness::claude::session_exists`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn claude_session_exists(id: String) -> bool {
    crate::home_dir().is_some_and(|home| claude::session_exists(&home, &id))
}
