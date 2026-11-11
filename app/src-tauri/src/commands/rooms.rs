//! Room persistence: the boot-time load and the wholesale save.

use std::sync::Arc;

use crate::db::{Database, LoadOutcome, Room};

/// Returns every room currently in the DB, plus any rows that failed
/// to parse and were quarantined (#167). The frontend calls this once
/// at boot to hydrate state.
///
/// A clean, non-empty load also refreshes the `skein.db.bak`
/// last-known-good snapshot — on a helper thread so boot isn't taxed.
/// Empty or quarantine-marred loads leave the previous snapshot alone.
///
/// Async: `load_all` walks every row in `sessions` and parses each
/// blob — sqlite I/O plus JSON parsing, off the main thread (#171).
#[tauri::command]
pub(crate) async fn db_load_rooms(
    db: tauri::State<'_, Arc<Database>>,
) -> Result<LoadOutcome, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || skein_daemon::rooms::load_rooms(&db))
        .await
        .map_err(|e| e.to_string())?
}

/// Replaces the DB's room list wholesale. Called whenever the
/// frontend's rooms state changes — wipe-and-insert is fine at
/// prototype scale and avoids the bookkeeping of granular upserts.
///
/// Async: `save_all` is a wipe + re-insert in one transaction — sqlite
/// fsync, off the main thread (#171). The frontend fires this
/// un-debounced on every `rooms` change without awaiting the previous
/// call, so async scheduling can let two saves commit out of order;
/// the seq ticket, minted here before the blocking work starts, makes
/// `save_all_seq` drop a save that lands after a newer one already
/// committed instead of silently reverting it.
#[tauri::command]
pub(crate) async fn db_save_rooms(
    rooms: Vec<Room>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    let seq = db.next_save_seq();
    tauri::async_runtime::spawn_blocking(move || db.save_all_seq(&rooms, seq))
        .await
        .map_err(|e| e.to_string())?
}
