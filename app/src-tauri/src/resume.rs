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
///
/// Async: opens the sqlite db and runs a query against it — real disk
/// I/O, and #171 wants that off the main thread.
#[tauri::command]
pub async fn opencode_list_sessions(cwd: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(conn) = open_opencode_db()? else {
            return Ok(Vec::new());
        };
        opencode::session_ids_for_directory(&conn, &cwd).map_err(|e| format!("query: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 4: does the opencode session row for `id` still exist (and
/// is non-archived)? Used at boot to drop stale captured ids before
/// resume tries to use them.
///
/// Async: same opencode.db read as `opencode_list_sessions` (#171).
#[tauri::command]
pub async fn opencode_session_exists(id: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(conn) = open_opencode_db()? else {
            return Ok(false);
        };
        opencode::session_exists(&conn, &id).map_err(|e| format!("exists: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 4: does Claude still have a session file for this id? Scans
/// every project dir rather than recomputing the lossy cwd encoding —
/// see `skein_harness::claude::session_exists`.
///
/// Async: a directory scan across every Claude project dir — real
/// disk I/O run at boot for every restored harness at once (#171).
#[tauri::command]
pub async fn claude_session_exists(id: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::home_dir().is_some_and(|home| claude::session_exists(&home, &id))
    })
    .await
    .map_err(|e| e.to_string())
}

/// Size and mtime of a Claude transcript, for the #423 supervisor's
/// "adapter silent while the file keeps growing" check.
#[derive(serde::Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptStatDto {
    pub size: u64,
    pub mtime_ms: u64,
}

/// Stat `path`; `None` when it doesn't exist.
fn stat_transcript(path: &std::path::Path) -> Result<Option<TranscriptStatDto>, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("stat transcript: {e}")),
    };
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    Ok(Some(TranscriptStatDto {
        size: meta.len(),
        mtime_ms,
    }))
}

/// Stat the main transcript for `session_id` at the COMPUTED path only.
/// The tail also falls back to a directory scan when that path misses,
/// so `tail_silent_file_growing` is blind when the cwd encoding drifts
/// (#259). `None` when the file doesn't exist. One stat, so it
/// runs inline rather than on the blocking pool.
#[tauri::command]
pub fn claude_transcript_stat(
    session_id: &str,
    cwd: &str,
) -> Result<Option<TranscriptStatDto>, String> {
    if session_id.is_empty() || session_id.contains(['/', '\\']) {
        return Err("invalid session id".to_string());
    }
    let Some(home) = crate::home_dir() else {
        return Ok(None);
    };
    stat_transcript(&claude::session_jsonl_path(&home, cwd, session_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_transcript_reports_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, b"hello").unwrap();
        let stat = stat_transcript(&path).unwrap().unwrap();
        assert_eq!(stat.size, 5);
        assert!(stat.mtime_ms > 0);
    }

    #[test]
    fn stat_transcript_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(stat_transcript(&dir.path().join("nope.jsonl")), Ok(None));
    }
}
