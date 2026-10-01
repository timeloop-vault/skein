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
    tauri::async_runtime::spawn_blocking(move || {
        let mut outcome = db.load_all()?;
        for s in &outcome.skipped {
            tracing::warn!(
                id = %s.id,
                error = %s.error,
                "quarantined unparseable room row (see sessions_quarantine)"
            );
        }
        if outcome.rooms.is_empty() && outcome.skipped.is_empty() {
            // A vanished/recreated skein.db parses as a clean fresh
            // install. If a backup with rooms sits next to it, tell the
            // frontend so the user isn't shown first-run onboarding over
            // recoverable rooms.
            outcome.backup_rooms = db.count_backup_rooms().filter(|n| *n > 0);
            if let Some(n) = outcome.backup_rooms {
                tracing::warn!("rooms table is empty but skein.db.bak holds {n} room(s)");
            }
        }
        // #237: sweep rows in the sibling tables whose room was deleted
        // forever (which only ever drops the `sessions` row). Gated on
        // `first_load && !rooms.is_empty()` — never on a failed load
        // (that must never read as "no rooms", #167), and an empty
        // `sessions` table after a successful load could itself be the
        // vanished-db case #167 guards against, so this stays
        // conservative: the next boot with a room sweeps. A sweep
        // failure must not fail the load.
        if outcome.first_load && !outcome.rooms.is_empty() {
            match db.sweep_orphans() {
                Ok(0) => {}
                Ok(n) => tracing::info!("swept {n} orphaned room-keyed row(s) (#237)"),
                Err(e) => tracing::warn!("orphan sweep failed: {e}"),
            }
        }
        // Refresh the last-known-good snapshot at most once per process
        // (first_load), and only for a clean, non-empty load — a marred
        // or empty load must leave the previous generations alone.
        if outcome.first_load && outcome.skipped.is_empty() && !outcome.rooms.is_empty() {
            let db = Arc::clone(&db);
            std::thread::spawn(move || match db.backup_last_known_good() {
                Ok(dest) => tracing::info!("refreshed room backup at {}", dest.display()),
                Err(e) => tracing::warn!("room backup failed: {e}"),
            });
        }
        Ok(outcome)
    })
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
