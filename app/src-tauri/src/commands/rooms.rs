//! Room persistence: the boot-time load (`db_load_rooms`), the mutation
//! path (`rooms_apply`) and the shutdown flush (`rooms_flush`).

use std::sync::Arc;

use crate::db::{Database, LoadOutcome};
use skein_daemon::boot_resume::BootResume;
use skein_daemon::room_store::RoomStore;
use skein_proto::room_mutations::{RoomMutation, RoomsApplyOutcome};

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
///
/// #731/#753: the first load of the process that returns rooms also runs the
/// boot preparation (launch migration, shell claims, a fresh opencode port),
/// retried by later loads until it succeeds; see `BootResume`.
#[tauri::command]
pub(crate) async fn db_load_rooms(
    db: tauri::State<'_, Arc<Database>>,
    store: tauri::State<'_, Arc<RoomStore>>,
    boot: tauri::State<'_, Arc<BootResume>>,
) -> Result<LoadOutcome, String> {
    let db = Arc::clone(&db);
    let store = Arc::clone(&store);
    let boot = Arc::clone(&boot);
    tauri::async_runtime::spawn_blocking(move || boot.load_rooms(&db, &store))
        .await
        .map_err(|e| e.to_string())?
}

/// Applies a batch of room mutations through the daemon's `RoomStore`
/// (#729). Async: the store writes sqlite, so the work runs on the
/// blocking pool, off the runtime's core threads (#171).
///
/// Ordering: blocking-pool tasks can overtake each other, so two
/// overlapping calls could apply out of order. The frontend sends
/// serially (it awaits each call before the next), so each call is
/// finished before the next one is spawned; that is the guarantee.
#[tauri::command]
pub(crate) async fn rooms_apply(
    client_id: String,
    mutations: Vec<RoomMutation>,
    store: tauri::State<'_, Arc<RoomStore>>,
) -> Result<RoomsApplyOutcome, String> {
    let store = Arc::clone(&store);
    tauri::async_runtime::spawn_blocking(move || store.apply(&client_id, mutations))
        .await
        .map_err(|e| e.to_string())?
}

/// Writes out any coalesced room changes the store still holds (window
/// close, update install, page hide). Blocking pool, like `rooms_apply`.
#[tauri::command]
pub(crate) async fn rooms_flush(store: tauri::State<'_, Arc<RoomStore>>) -> Result<(), String> {
    let store = Arc::clone(&store);
    tauri::async_runtime::spawn_blocking(move || store.flush())
        .await
        .map_err(|e| e.to_string())?
}
