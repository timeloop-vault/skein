//! Chapter 5 — transparent harness resume.
//!
//! Tauri commands that probe the underlying tools' on-disk session
//! storage so Skein can capture and validate session ids. The
//! storage layouts and queries themselves live in `skein-harness`
//! (#209); this module is the command boundary, collapsing errors to
//! `String` and treating "no home dir" / "no db" as empty.

use std::path::PathBuf;

use skein_harness::{claude, hermes, opencode};

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

/// #624: does Hermes' `state.db` still hold a live session `id`? A
/// missing home or db file is `true` ("can't tell"): `HERMES_HOME` may
/// differ between Skein's environment and Hermes's own, and a `false`
/// makes hydrate strip the stored id for good. Only an existing db
/// without the row (or with it archived) is `false`.
///
/// Async: opens and queries sqlite, so off the main thread (#171).
#[tauri::command]
pub async fn hermes_session_exists(id: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(home) = hermes::hermes_home() else {
            return Ok(true);
        };
        hermes_exists_in(&hermes::state_db_path(&home), &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `hermes_session_exists` against the db at `db`; a missing file is
/// `true` (can't tell), see the command.
fn hermes_exists_in(db: &std::path::Path, id: &str) -> Result<bool, String> {
    if !db.exists() {
        return Ok(true);
    }
    hermes::session_exists(db, id).map_err(|e| format!("hermes exists: {e}"))
}

/// A Hermes harness still waiting for its session id, as the frontend
/// sends it.
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingDto {
    pub harness_id: String,
    pub spawned_at_ms: i64,
}

/// The claim for `harness_id` against the db at `db`: sessions in `cwd`
/// started since the earliest pending spawn (less clock skew), assigned
/// by `assign_sessions`. `None` when nothing is assigned to it.
fn claim_session_in(
    db: &std::path::Path,
    cwd: &str,
    harness_id: &str,
    pending: &[PendingDto],
    claimed: &[String],
    local_minus_utc_minutes: i32,
) -> Result<Option<String>, String> {
    let Some(earliest) = pending.iter().map(|p| p.spawned_at_ms).min() else {
        return Ok(None);
    };
    #[allow(clippy::cast_precision_loss)]
    let since = (earliest - hermes::CLOCK_SKEW_MS) as f64 / 1000.0;
    // The row stores `os.getcwd()`, which may be the canonical spelling
    // of what Skein holds; query both forms and merge by id.
    let mut candidates: Vec<hermes::SessionCandidate> = Vec::new();
    let mut forms = vec![cwd.to_string()];
    if let Ok(canon) = dunce::canonicalize(cwd) {
        let canon = canon.to_string_lossy().into_owned();
        if canon != cwd {
            forms.push(canon);
        }
    }
    for form in &forms {
        let found = hermes::tui_sessions_in_cwd(db, form, since)
            .map_err(|e| format!("hermes claim: {e}"))?;
        for c in found {
            if !candidates.iter().any(|x| x.id == c.id) {
                candidates.push(c);
            }
        }
    }
    candidates.sort_by(|a, b| {
        a.started_at
            .total_cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let pending: Vec<hermes::PendingHarness> = pending
        .iter()
        .map(|p| hermes::PendingHarness {
            harness_id: p.harness_id.clone(),
            spawned_at_ms: p.spawned_at_ms,
        })
        .collect();
    let claimed: std::collections::HashSet<String> = claimed.iter().cloned().collect();
    let mut assigned =
        hermes::assign_sessions(&candidates, &pending, &claimed, local_minus_utc_minutes);
    Ok(assigned.remove(harness_id))
}

/// #624: which Hermes session (if any) belongs to `harness_id`? See
/// `skein_harness::hermes::assign_sessions` for the rule.
///
/// Async: sqlite read, off the main thread (#171).
#[tauri::command]
pub async fn hermes_claim_session(
    cwd: String,
    harness_id: String,
    pending: Vec<PendingDto>,
    claimed: Vec<String>,
    local_minus_utc_minutes: i32,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(home) = hermes::hermes_home() else {
            return Ok(None);
        };
        let found = claim_session_in(
            &hermes::state_db_path(&home),
            &cwd,
            &harness_id,
            &pending,
            &claimed,
            local_minus_utc_minutes,
        )?;
        if let Some(session_id) = &found {
            tracing::info!(%harness_id, %session_id, "hermes session claimed");
        }
        Ok(found)
    })
    .await
    .map_err(|e| e.to_string())?
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

    fn hermes_db(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("state.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE sessions (id TEXT PRIMARY KEY, \
             source TEXT, parent_session_id TEXT, started_at REAL, cwd TEXT, \
             archived INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        // 2026-01-01 12:00:00 UTC, matching the id's stamp at offset 0.
        conn.execute(
            "INSERT INTO sessions (id, source, parent_session_id, started_at, cwd, archived) \
             VALUES ('20260101_120000_abcdef', 'tui', NULL, 1767268800.5, '/work/a', 0)",
            [],
        )
        .unwrap();
        path
    }

    #[test]
    fn hermes_claim_returns_this_harnesses_session() {
        let dir = tempfile::tempdir().unwrap();
        let db = hermes_db(dir.path());
        let pending = [PendingDto {
            harness_id: "h1".into(),
            spawned_at_ms: 1_767_268_798_000,
        }];
        let got = claim_session_in(&db, "/work/a", "h1", &pending, &[], 0).unwrap();
        assert_eq!(got.as_deref(), Some("20260101_120000_abcdef"));
        let claimed = ["20260101_120000_abcdef".to_string()];
        assert_eq!(
            claim_session_in(&db, "/work/a", "h1", &pending, &claimed, 0).unwrap(),
            None
        );
    }

    #[test]
    fn hermes_claim_matches_the_canonical_spelling_of_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("w")).unwrap();
        let canon = dunce::canonicalize(dir.path().join("w")).unwrap();
        let db = dir.path().join("state.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT,              parent_session_id TEXT, started_at REAL, cwd TEXT,              archived INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, source, parent_session_id, started_at, cwd, archived)              VALUES ('20260101_120000_abcdef', 'tui', NULL, 1767268800.5, ?1, 0)",
            [canon.to_string_lossy().as_ref()],
        )
        .unwrap();
        drop(conn);
        // A non-canonical spelling: a `.` segment.
        let spelled = canon.join(".").to_string_lossy().into_owned();
        let pending = [PendingDto {
            harness_id: "h1".into(),
            spawned_at_ms: 1_767_268_798_000,
        }];
        let got = claim_session_in(&db, &spelled, "h1", &pending, &[], 0).unwrap();
        assert_eq!(got.as_deref(), Some("20260101_120000_abcdef"));
    }

    #[test]
    fn hermes_exists_missing_db_is_unknown_but_missing_row_is_false() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(hermes_exists_in(&dir.path().join("none.db"), "x"), Ok(true));
        let db = hermes_db(dir.path());
        assert_eq!(hermes_exists_in(&db, "20260101_120000_abcdef"), Ok(true));
        assert_eq!(hermes_exists_in(&db, "nope"), Ok(false));
    }

    #[test]
    fn hermes_claim_on_missing_db_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let pending = [PendingDto {
            harness_id: "h1".into(),
            spawned_at_ms: 1_767_268_798_000,
        }];
        let got = claim_session_in(
            &dir.path().join("none.db"),
            "/work/a",
            "h1",
            &pending,
            &[],
            0,
        );
        assert_eq!(got, Ok(None));
    }

    #[test]
    fn stat_transcript_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(stat_transcript(&dir.path().join("nope.jsonl")), Ok(None));
    }
}
