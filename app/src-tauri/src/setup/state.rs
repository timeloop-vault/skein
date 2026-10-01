//! Managed state: spawn environment, harness config, the database and
//! the managers that hang off it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::Manager;

use crate::SpawnEnvState;
use crate::db::Database;
use crate::harness_events_claude::ClaudeEventsManager;
use crate::harness_events_opencode::OpencodeEventsManager;
use crate::pty::PtyManager;
use crate::watcher::WatcherManager;

/// Resolves (and creates) the app data dir, loads the spawn settings,
/// starts the login-shell probe and manages `SpawnEnvState`. Returns the
/// data dir for the database open that follows.
pub(super) fn spawn_env(app: &mut tauri::App) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Persist Skein state under the OS-conventional app data dir
    // (e.g. %APPDATA%/com.timeloop-vault.skein on Windows). Create
    // the directory eagerly so first-launch users don't see a
    // misleading "DB open failed" before they've created anything.
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;

    // Environment settings must be readable before the first
    // spawn, and the probe needs the configured shell, so both
    // happen here rather than being pushed in from the frontend.
    let (spawn_settings, spawn_degraded) = crate::spawn_settings::load(&data_dir);
    if let Some(ref problem) = spawn_degraded {
        tracing::warn!(problem, "spawn settings degraded");
    }

    // Ask the user's login shell for its PATH now, on a helper
    // thread, so the answer is already waiting when the first
    // harness spawns. This used to happen lazily inside the
    // first `pty_spawn` — a sync command, therefore the main
    // thread — with no timeout, so an rc file that blocked took
    // the whole app's event loop with it (#177).
    crate::pty::prewarm_probe(data_dir.clone(), spawn_settings.clone());

    app.manage(SpawnEnvState {
        data_dir: data_dir.clone(),
        settings: parking_lot::RwLock::new(spawn_settings),
        degraded: parking_lot::RwLock::new(spawn_degraded),
    });
    Ok(data_dir)
}

pub(super) fn harness_config(app: &mut tauri::App) {
    // #215: the shipped Claude Code plugin and opencode config
    // that teach the agent CLIs about the review API. Resolved
    // once — the resource directory does not move while the app
    // runs — and never fatal: a bundle that failed to package
    // costs the agent its review tools, which the Settings pane
    // reports rather than leaving as a mystery (#176).
    let harness_config = match app.path().resource_dir() {
        Ok(dir) => crate::harness_config::HarnessConfig::resolve(&dir),
        Err(e) => {
            tracing::error!(error = %e, "harness config: no resource dir");
            crate::harness_config::HarnessConfig::unavailable(&format!(
                "resource directory unavailable: {e}"
            ))
        }
    };
    app.manage(harness_config);
}

/// Opens the database, revokes stale agent tokens and manages the PTY,
/// watcher and harness-event managers. The database itself is managed
/// by the caller, after the servers that borrow it are up.
pub(super) fn open_database(
    app: &mut tauri::App,
    data_dir: &Path,
) -> Result<Arc<Database>, Box<dyn std::error::Error>> {
    let db_path = data_dir.join("skein.db");
    let db = Database::open(&db_path).map_err(|e| {
        Box::<dyn std::error::Error>::from(format!("opening {}: {e}", db_path.display()))
    })?;
    let db = Arc::new(db);

    // #213: a review token lives no longer than the process
    // that handed it out. Every PTY died with the last Skein,
    // so nothing legitimate still holds one — and a token that
    // ended up in a transcript or a log stops working here.
    match db.revoke_agent_tokens(None, crate::review::now_ms()) {
        Ok(n) if n > 0 => tracing::info!(count = n, "agent api: revoked stale room tokens"),
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "agent api: revoking stale tokens failed"),
    }
    app.manage(PtyManager::new());
    app.manage(WatcherManager::new());
    app.manage(ClaudeEventsManager::new(
        Arc::clone(&db),
        app.handle().clone(),
    ));
    app.manage(OpencodeEventsManager::new(
        Arc::clone(&db),
        app.handle().clone(),
    ));
    Ok(db)
}
