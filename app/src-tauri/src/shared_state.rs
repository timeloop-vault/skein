//! The server-side state that hangs off `Paths`, built without a
//! `tauri::App`. (#579)
//!
//! `setup/state.rs` calls this and `app.manage`s each field; a daemon will
//! call it with its own `Paths` and event sink. Imports nothing from
//! `tauri`.

use std::sync::Arc;

use crate::SpawnEnvState;
use crate::db::Database;
use crate::event_sink::SharedEventSink;
use crate::harness_actions_hermes::HermesActionsManager;
use crate::harness_config::HarnessConfig;
use crate::harness_events_claude::ClaudeEventsManager;
use crate::harness_events_opencode::OpencodeEventsManager;
use crate::harness_work_hermes::HermesWorkManager;
use crate::opencode_sidecar::OpencodeSidecarManager;
use crate::paths::Paths;
use crate::pty::PtyManager;
use crate::watcher::WatcherManager;

/// Whether to start the login-shell probe. It runs a real shell on
/// macOS/Linux, so a test must pass [`Probe::Skip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Start,
    /// Only tests (and a daemon, later) skip it.
    #[cfg_attr(not(test), allow(dead_code))]
    Skip,
}

/// Everything `setup()` manages that depends only on `Paths` and an event
/// sink. The database is managed by the caller, after the servers that
/// borrow it are up.
pub struct SharedState {
    pub harness_config: HarnessConfig,
    pub db: Arc<Database>,
    pub pty: PtyManager,
    pub sidecars: OpencodeSidecarManager,
    pub watcher: WatcherManager,
    pub claude: ClaudeEventsManager,
    pub opencode: OpencodeEventsManager,
    pub hermes_actions: HermesActionsManager,
    pub hermes_work: HermesWorkManager,
    pub events: SharedEventSink,
}

/// Step one: creates the data dir, loads the spawn settings and (unless
/// skipped) starts the login-shell probe. Split from [`SharedState::build`]
/// so the client can run its updater-proxy init between the two, as it
/// always has.
///
/// # Errors
/// The data dir cannot be created.
pub fn spawn_env(paths: &Paths, probe: Probe) -> Result<SpawnEnvState, Box<dyn std::error::Error>> {
    // Create the directory eagerly so first-launch users don't see a
    // misleading "DB open failed" before they've created anything.
    let data_dir = paths.data_dir.clone();
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
    if probe == Probe::Start {
        crate::pty::prewarm_probe(data_dir.clone(), spawn_settings.clone());
    }

    Ok(SpawnEnvState {
        data_dir,
        settings: parking_lot::RwLock::new(spawn_settings),
        degraded: parking_lot::RwLock::new(spawn_degraded),
    })
}

impl SharedState {
    /// Step two: the harness config, the database and the managers.
    ///
    /// # Errors
    /// The database cannot be opened.
    pub fn build(
        paths: &Paths,
        events: SharedEventSink,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let harness_config = HarnessConfig::from_paths(paths);

        let db_path = paths.db_path();
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
        // `new` sweeps orphaned sidecars of a crashed Skein before returning,
        // so no new record can be written ahead of it.
        let sidecars = OpencodeSidecarManager::new(Some(paths.data_dir.clone()));
        Ok(Self {
            harness_config,
            pty: PtyManager::new(),
            sidecars,
            watcher: WatcherManager::new(),
            claude: ClaudeEventsManager::new(Arc::clone(&db), events.clone()),
            opencode: OpencodeEventsManager::new(Arc::clone(&db), events.clone()),
            hermes_actions: HermesActionsManager::new(Arc::clone(&db), events.clone()),
            hermes_work: HermesWorkManager::default(),
            db,
            events,
        })
    }
}

#[cfg(test)]
mod tests;
