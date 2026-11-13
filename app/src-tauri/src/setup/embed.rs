//! Embeds the daemon: builds the Tauri side of the in-process transport,
//! calls `Daemon::build`, hands every piece to `app.manage` and spawns the
//! two listener tasks. The construction itself is Tauri-free
//! (`skein-daemon`'s `daemon.rs`).

use std::sync::Arc;

use skein_daemon::daemon::{self, Daemon, Probe};
use skein_daemon::transport::InProcessTransport;
use tauri::Manager;

use crate::paths::Paths;
use crate::tauri_client::{TauriClientRequests, TauriWebviewCapture};
use crate::tauri_event_sink::TauriEventSink;

/// Resolves the paths, creates the app data dir, loads the spawn settings,
/// starts the login-shell probe and manages `SpawnEnvState`.
pub(super) fn spawn_env(app: &mut tauri::App) -> Result<Paths, Box<dyn std::error::Error>> {
    let paths = crate::tauri_paths::from_app(app)?;
    app.manage(Arc::new(daemon::spawn_env(&paths, Probe::Start)?));
    Ok(paths)
}

/// Builds the daemon over the Tauri transport, manages its pieces and
/// spawns its listeners. The database is managed last.
pub(super) fn daemon(
    app: &mut tauri::App,
    paths: &Paths,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let transport = InProcessTransport {
        events: Arc::new(TauriEventSink::new(handle.clone())),
        client: Arc::new(TauriClientRequests::new(handle.clone())),
        capture: Arc::new(TauriWebviewCapture::new(handle)),
    };
    let spawn_env = Arc::clone(&*app.state::<Arc<crate::SpawnEnvState>>());
    let d = Daemon::build(paths, spawn_env, app.config().identifier.clone(), transport)?;

    app.manage(d.harness_config);
    app.manage(d.rooms);
    app.manage(d.boot_resume);
    app.manage(d.harness_runner);
    app.manage(d.pty);
    app.manage(d.phase);
    // #698: the Windows updater exits without RunEvent::Exit; this guard's
    // Drop (via cleanup_before_exit) stops the sidecars on that path.
    app.resources_table()
        .add(crate::opencode_sidecar::SidecarExitGuard::new(
            d.sidecars.clone(),
        ));
    app.manage(d.sidecars);
    app.manage(d.watcher);
    app.manage(d.claude);
    app.manage(d.opencode);
    app.manage(d.hermes_actions);
    app.manage(d.events);
    app.manage(d.hermes_work);

    // Managed so `agent_request_complete` (#328) can reach the same
    // pending-request map the HTTP server's handlers register requests on.
    if let Some(api) = d.agent_api {
        app.manage(api);
    }
    if let Some(server) = d.agent_api_server {
        tauri::async_runtime::spawn(server);
    }
    app.manage(d.agent_endpoint);

    if let Some(server) = d.preview_server {
        tauri::async_runtime::spawn(server);
    }
    app.manage(d.preview);
    app.manage(d.preview_endpoint);

    app.manage(d.db);
    Ok(())
}
