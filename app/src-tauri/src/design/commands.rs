//! The Tauri boundary for the design surface (#433): the preview base
//! URL, the list of previewable entries, and the reload watcher. The
//! folder always comes from the database by room id, never from the
//! frontend. The logic itself is in `skein_daemon::design::entries`.

use std::sync::Arc;

use tauri::State;
use tauri::ipc::Channel;

use skein_daemon::design::entries::start_design_watch;
pub use skein_daemon::design::entries::{PreviewEndpoint, list_entries_for_room, room_root};

use super::PreviewState;
use crate::db::Database;
use crate::stream_sink::StreamSink;
use crate::tauri_stream_sink::TauriChannelSink;
use crate::watcher::WatcherManager;

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn design_preview_base(
    room_id: String,
    db: State<'_, Arc<Database>>,
    state: State<'_, Arc<PreviewState>>,
    endpoint: State<'_, PreviewEndpoint>,
) -> Result<String, String> {
    let port = endpoint.port()?;
    room_root(&db, &room_id)?;
    let token = state.mint(&room_id);
    Ok(format!("http://127.0.0.1:{port}/preview/{token}/"))
}

#[tauri::command]
pub async fn design_list_entries(
    room_id: String,
    db: State<'_, Arc<Database>>,
) -> Result<Vec<String>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || list_entries_for_room(&db, &room_id))
        .await
        .map_err(|e| e.to_string())?
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn design_watch_start(
    room_id: String,
    on_change: Channel<()>,
    db: State<'_, Arc<Database>>,
    manager: State<'_, WatcherManager>,
) -> Result<String, String> {
    let root = room_root(&db, &room_id)?;
    let canonical = dunce::canonicalize(&root).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    tracing::info!(room_id = %room_id, root = %canonical.display(), "design watch start");
    let sink = TauriChannelSink::new(on_change);
    start_design_watch(&manager, id.clone(), &canonical, move || {
        // Fails only if the frontend dropped its half.
        let _ = sink.send(());
    })
    .map_err(|e| e.to_string())?;
    Ok(id)
}
