//! Chapter 5 — transparent harness resume.
//!
//! Tauri commands that probe the underlying tools' on-disk session
//! storage so Skein can capture and validate session ids. The probe
//! logic lives in `skein_daemon::resume` and the storage layouts and
//! queries in `skein-harness` (#209); this module is the command
//! boundary, collapsing errors to `String` and treating "no home dir" /
//! "no db" as empty.

pub(crate) use skein_daemon::resume::profile_db;
use skein_daemon::resume::{PendingDto, TranscriptStatDto};

/// IDs of all non-archived opencode sessions whose `directory` matches
/// `cwd`, newest first. Empty vec when the db doesn't exist or `HOME`
/// isn't set.
///
/// Async: opens the sqlite db and runs a query against it — real disk
/// I/O, and #171 wants that off the main thread.
#[tauri::command]
pub async fn opencode_list_sessions(cwd: String) -> Result<Vec<String>, String> {
    skein_daemon::resume::opencode_list_sessions(cwd).await
}

/// Phase 4: does the opencode session row for `id` still exist (and
/// is non-archived)? Used at boot to drop stale captured ids before
/// resume tries to use them.
///
/// Async: same opencode.db read as `opencode_list_sessions` (#171).
#[tauri::command]
pub async fn opencode_session_exists(id: String) -> Result<bool, String> {
    skein_daemon::resume::opencode_session_exists(id).await
}

/// Phase 4: does Claude still have a session file for this id? Scans
/// every project dir rather than recomputing the lossy cwd encoding —
/// see `skein_harness::claude::session_exists`.
///
/// Async: a directory scan across every Claude project dir — real
/// disk I/O run at boot for every restored harness at once (#171).
#[tauri::command]
pub async fn claude_session_exists(id: String) -> Result<bool, String> {
    skein_daemon::resume::claude_session_exists(id).await
}

/// #624: does Hermes' `state.db` still hold a live session `id`? A
/// missing home or db file is `true` ("can't tell"): `HERMES_HOME` may
/// differ between Skein's environment and Hermes's own, and a `false`
/// makes hydrate strip the stored id for good. Only an existing db
/// without the row (or with it archived) is `false`.
///
/// Async: opens and queries sqlite, so off the main thread (#171).
#[tauri::command]
pub async fn hermes_session_exists(
    id: String,
    profile: Option<String>,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<bool, String> {
    let settings = spawn_env.snapshot();
    skein_daemon::resume::hermes_session_exists(
        move || profile_db(&settings, profile.as_deref()),
        id,
    )
    .await
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
    profile: Option<String>,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<Option<String>, String> {
    let settings = spawn_env.snapshot();
    skein_daemon::resume::hermes_claim_session(
        move || profile_db(&settings, profile.as_deref()),
        cwd,
        harness_id,
        pending,
        claimed,
        local_minus_utc_minutes,
    )
    .await
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
    skein_daemon::resume::claude_transcript_stat(session_id, cwd)
}
