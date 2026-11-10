//! Managed state: spawn environment, harness config, the database and
//! the managers that hang off it. The construction itself is Tauri-free
//! (`shared_state.rs`, from a `Paths`); this file only feeds it the Tauri
//! paths and event sink and hands each value to `app.manage`.

use std::sync::Arc;

use tauri::Manager;

use crate::db::Database;
use crate::event_sink::SharedEventSink;
use crate::paths::Paths;
use crate::shared_state::{self, Probe, SharedState};
use crate::tauri_event_sink::TauriEventSink;

/// Resolves the paths, creates the app data dir, loads the spawn settings,
/// starts the login-shell probe and manages `SpawnEnvState`.
pub(super) fn spawn_env(app: &mut tauri::App) -> Result<Paths, Box<dyn std::error::Error>> {
    let paths = crate::tauri_paths::from_app(app)?;
    app.manage(shared_state::spawn_env(&paths, Probe::Start)?);
    Ok(paths)
}

/// Builds the harness config, opens the database, revokes stale agent
/// tokens and manages the PTY, watcher and harness-event managers. The
/// database itself is returned for the caller to manage, after the
/// servers that borrow it are up.
pub(super) fn open_database(
    app: &mut tauri::App,
    paths: &Paths,
) -> Result<Arc<Database>, Box<dyn std::error::Error>> {
    let events: SharedEventSink = Arc::new(TauriEventSink::new(app.handle().clone()));
    let state = SharedState::build(paths, events)?;

    app.manage(state.harness_config);
    app.manage(state.pty);
    // #698: the Windows updater exits without RunEvent::Exit; this guard's
    // Drop (via cleanup_before_exit) stops the sidecars on that path.
    app.resources_table()
        .add(crate::opencode_sidecar::SidecarExitGuard::new(
            state.sidecars.clone(),
        ));
    app.manage(state.sidecars);
    app.manage(state.watcher);
    app.manage(state.claude);
    app.manage(state.opencode);
    app.manage(state.hermes_actions);
    app.manage(state.events);
    app.manage(state.hermes_work);
    Ok(state.db)
}
