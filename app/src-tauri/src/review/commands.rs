//! The Tauri boundary for the review baseline (#211, #221): four thin
//! wrappers over `skein_daemon::review`.

use std::sync::Arc;

use skein_review::Hunk;

use crate::db::Database;
use crate::event_sink::SharedEventSink;
use crate::review::{PendingFileDto, accept_impl, catch_up_and_emit, pending_impl, reject_impl};
use crate::watcher::WatcherManager;

/// Every file in the room with unreviewed changes.
///
/// Runs on the blocking pool: it stats and reads one file per baseline
/// and the Diff card calls it on every debounced watcher tick, which is
/// exactly the shape of work #171/#172 want off the main thread.
#[tauri::command]
pub async fn review_pending(
    room_id: String,
    cwd: String,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<PendingFileDto>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || pending_impl(&db, &room_id, &cwd))
        .await
        .map_err(|e| e.to_string())?
}

/// Advance the baseline. **Never writes to the working tree.**
///
/// `path` `None` accepts every pending file in the room; `hunks` empty
/// accepts the whole file. Returns how many files moved.
#[tauri::command]
pub async fn review_accept(
    room_id: String,
    cwd: String,
    path: Option<String>,
    hunks: Vec<Hunk>,
    content_hash: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<usize, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        accept_impl(
            &db,
            &room_id,
            &cwd,
            path.as_deref(),
            &hunks,
            content_hash.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Put the working tree back to the baseline. **Never moves the
/// baseline.** `hunks` empty rejects the whole file.
///
/// There is deliberately no reject-all: it is a destructive verb and
/// wants to be chosen per file.
#[tauri::command]
pub async fn review_reject(
    room_id: String,
    cwd: String,
    path: String,
    hunks: Vec<Hunk>,
    content_hash: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        reject_impl(&db, &room_id, &cwd, &path, &hunks, content_hash.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start watching `cwd` for review discovery (#221); see
/// `skein_daemon::review::start_discovery`. Also runs one catch-up sweep
/// immediately, off the async runtime.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn review_discovery_start(
    room_id: String,
    cwd: String,
    events: tauri::State<'_, SharedEventSink>,
    db: tauri::State<'_, Arc<Database>>,
    manager: tauri::State<'_, WatcherManager>,
) -> Result<String, String> {
    let id = crate::review::start_discovery(&room_id, &cwd, &events, &db, &manager)?;
    let events = Arc::clone(&events);
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        catch_up_and_emit(&events, &db, &room_id, &cwd);
    });
    Ok(id)
}
