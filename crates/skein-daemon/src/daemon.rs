//! The Tauri-free server, built from `Paths` and an in-process transport.
//! (#579, #582)
//!
//! [`spawn_env`] is the first step (the client runs its updater-proxy init
//! between it and the second, as it always has); [`Daemon::build`] is the
//! second: harness config, database, managers, then the two localhost
//! listeners. It spawns nothing itself, since the daemon has no runtime of
//! its own: each server comes back as a future for the embedder to spawn.
//! Imports nothing from `tauri`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::agent_api::AgentApiState;
use crate::agent_api::state::{AgentApiEndpoint, AgentApiStateArgs};
use crate::boot_resume::BootResume;
use crate::db::Database;
use crate::design::PreviewState;
use crate::design::entries::PreviewEndpoint;
use crate::event_sink::SharedEventSink;
use crate::harness_actions_hermes::HermesActionsManager;
use crate::harness_config::HarnessConfig;
use crate::harness_events_claude::ClaudeEventsManager;
use crate::harness_events_opencode::OpencodeEventsManager;
use crate::harness_runner::{HarnessRunner, RunnerDeps};
use crate::harness_work_hermes::HermesWorkManager;
use crate::opencode_sidecar::OpencodeSidecarManager;
use crate::paths::Paths;
use crate::pty::PtyManager;
use crate::room_store::RoomStore;
use crate::spawn_env::state::SpawnEnvState;
use crate::transport::InProcessTransport;
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

/// A listener's serve loop, ready to be spawned by the embedder.
pub type ServerFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Everything that depends only on `Paths` and an event sink.
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

/// The built daemon: the shared state, the two servers' state and endpoints,
/// and their serve loops. The embedder manages each piece and spawns the
/// futures; the database is best managed last.
pub struct Daemon {
    pub harness_config: HarnessConfig,
    pub db: Arc<Database>,
    /// The authoritative in-memory rooms (#729), filled by `rooms::load_rooms`.
    pub rooms: Arc<RoomStore>,
    /// Once-per-process boot preparation of the rooms (#731); load through it.
    pub boot_resume: Arc<BootResume>,
    /// Starts and stops harnesses from their records (#731).
    pub harness_runner: Arc<HarnessRunner>,
    pub pty: PtyManager,
    pub sidecars: OpencodeSidecarManager,
    pub watcher: WatcherManager,
    pub claude: ClaudeEventsManager,
    pub opencode: OpencodeEventsManager,
    pub hermes_actions: HermesActionsManager,
    pub hermes_work: HermesWorkManager,
    pub events: SharedEventSink,
    /// `None` when the agent API listener failed to bind.
    pub agent_api: Option<Arc<AgentApiState>>,
    pub agent_endpoint: AgentApiEndpoint,
    pub preview: Arc<PreviewState>,
    pub preview_endpoint: PreviewEndpoint,
    /// The agent API's serve loop; `None` when the bind failed.
    pub agent_api_server: Option<ServerFuture>,
    /// The design preview's serve loop; `None` when the bind failed.
    pub preview_server: Option<ServerFuture>,
}

/// Binds 127.0.0.1 on an ephemeral port, non-blocking, with the std
/// listener so a failure is known synchronously.
fn bind_local() -> std::io::Result<(std::net::TcpListener, u16)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    Ok((listener, port))
}

impl Daemon {
    /// Step two, in order: the shared state (harness config, database,
    /// stale-token revocation, managers), then the agent API listener and
    /// state, then the design preview listener and state.
    ///
    /// # Errors
    /// The database cannot be opened. A failed bind is not an error: it is
    /// recorded in the endpoint and logged.
    pub fn build(
        paths: &Paths,
        spawn_env: Arc<SpawnEnvState>,
        identifier: String,
        transport: InProcessTransport,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let InProcessTransport {
            events,
            client,
            capture,
        } = transport;
        let state = SharedState::build(paths, events.clone())?;
        let boot_env = Arc::clone(&spawn_env);

        // Bound with the std listener so a failure is known *here*,
        // synchronously, and can be recorded: an async bind inside the
        // spawned task would fail into a log line nobody reads and leave
        // the settings pane claiming a port that never existed.
        //
        // 127.0.0.1 only, and an ephemeral port: the URL reaches the
        // harnesses through SKEIN_REVIEW_URL at spawn time, so nothing has
        // to agree on a number in advance.
        let (agent_api, agent_api_server, agent_endpoint) = match bind_local() {
            Ok((listener, port)) => {
                let api = Arc::new(AgentApiState::new(AgentApiStateArgs {
                    db: Arc::clone(&state.db),
                    identifier,
                    events,
                    spawn_env,
                    client,
                    capture,
                }));
                let serving = Arc::clone(&api);
                let server: ServerFuture = Box::pin(async move {
                    match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => crate::agent_api::http::serve(listener, serving).await,
                        Err(e) => {
                            tracing::error!(error = %e, "agent api: adopting listener failed");
                        }
                    }
                });
                tracing::info!(port, "agent api listening on 127.0.0.1");
                (Some(api), Some(server), AgentApiEndpoint::bound(port))
            }
            Err(e) => {
                // Not fatal: Skein is perfectly usable without the agent
                // API. But it must be *visible* (#176).
                tracing::error!(error = %e, "agent api: bind failed; agents cannot reach the review");
                (None, None, AgentApiEndpoint::failed(e.to_string()))
            }
        };

        // The design preview (#433): a separate listener from the agent
        // API, and not fatal when the bind fails.
        let preview = Arc::new(PreviewState::new(Arc::clone(&state.db)));
        let (preview_server, preview_endpoint) = match bind_local() {
            Ok((listener, port)) => {
                let serving = Arc::clone(&preview);
                let server: ServerFuture = Box::pin(async move {
                    match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => crate::design::serve(listener, serving).await,
                        Err(e) => {
                            tracing::error!(error = %e, "design preview: adopting listener failed");
                        }
                    }
                });
                tracing::info!(port, "design preview listening on 127.0.0.1");
                (Some(server), PreviewEndpoint::bound(port))
            }
            Err(e) => {
                tracing::error!(error = %e, "design preview: bind failed");
                (None, PreviewEndpoint::failed(e.to_string()))
            }
        };

        let boot_resume = Arc::new(BootResume::new(Arc::clone(&boot_env)));
        let rooms = RoomStore::new(Arc::clone(&state.db), state.events.clone());
        // #731: the runner drives the same managers the app manages (clones
        // share their registries) and hears of every applied room batch.
        let harness_runner = Arc::new(HarnessRunner::new(RunnerDeps {
            store: Arc::clone(&rooms),
            db: Arc::clone(&state.db),
            pty: state.pty.clone(),
            sidecars: state.sidecars.clone(),
            claude: state.claude.clone(),
            opencode: state.opencode.clone(),
            hermes_actions: state.hermes_actions.clone(),
            spawn_env: boot_env,
            endpoint: agent_endpoint.clone(),
            harness_config: state.harness_config.clone(),
            events: state.events.clone(),
        }));
        harness_runner.observe_store();
        Ok(Self {
            harness_config: state.harness_config,
            rooms,
            harness_runner,
            boot_resume,
            db: state.db,
            pty: state.pty,
            sidecars: state.sidecars,
            watcher: state.watcher,
            claude: state.claude,
            opencode: state.opencode,
            hermes_actions: state.hermes_actions,
            hermes_work: state.hermes_work,
            events: state.events,
            agent_api,
            agent_endpoint,
            preview,
            preview_endpoint,
            agent_api_server,
            preview_server,
        })
    }
}

#[cfg(test)]
mod tests;
