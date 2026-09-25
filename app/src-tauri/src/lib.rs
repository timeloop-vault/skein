//! Skein — Tauri shell entrypoint: builder, managed state, and the
//! command registry.
//!
//! PTY commands let the frontend spawn a real interactive terminal
//! inside the harness pane. Each spawn produces an id; subsequent
//! writes/resizes/kills are keyed by it. Output streams back over a
//! per-spawn `tauri::ipc::Channel<PtyEvent>` (tagged data/exit).
mod agent_api;
mod agents;
mod db;
mod fs;
mod git;
mod harness_action_event;
mod harness_actions_claude;
mod harness_actions_opencode;
mod harness_config;
mod harness_events_claude;
mod harness_events_opencode;
mod harness_kind;
mod os_notify;
mod pty;
mod resume;
mod review;
mod review_surface;
mod spawn_env;
mod spawn_settings;
mod watcher;

use std::path::Path;
use std::sync::Arc;

use tauri::Manager;
use tauri::ipc::Channel;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::db::{Database, HarnessAction, HarnessEvent, LoadOutcome, Room};
use crate::harness_events_claude::{ClaudeEvent, ClaudeEventsManager};
use crate::harness_events_opencode::{OpencodeEvent, OpencodeEventsManager};
use crate::pty::{PtyEvent, PtyManager};
use crate::spawn_settings::SpawnSettings;
use crate::watcher::WatcherManager;

/// Hold the non-blocking tracing-appender guard for the lifetime of
/// the app. Dropping it stops the background flush thread; pending
/// log lines from just-before-quit can be lost. Kept in Tauri state
/// so it lives until process exit. The guard's value is never read —
/// only its `Drop` matters.
#[allow(dead_code)]
struct LogGuard(tracing_appender::non_blocking::WorkerGuard);

/// Everything the spawn path needs to know that isn't per-spawn: the
/// user's environment settings and where they live on disk.
///
/// Held as Tauri managed state rather than passed from the frontend
/// because the probe is kicked off during `setup()`, before a webview
/// exists to be asked — and because a frontend-push design would race
/// room hydration and lose *silently*, spawning harnesses with the
/// wrong environment. See `spawn_settings`.
pub(crate) struct SpawnEnvState {
    data_dir: std::path::PathBuf,
    settings: parking_lot::RwLock<SpawnSettings>,
    /// Set when the settings file existed but couldn't be used, so the
    /// UI can say "your edits aren't in effect" instead of quietly
    /// showing defaults.
    degraded: parking_lot::RwLock<Option<String>>,
}

impl SpawnEnvState {
    pub(crate) fn snapshot(&self) -> SpawnSettings {
        self.settings.read().clone()
    }
}

/// Install rustls's default crypto provider so reqwest doesn't panic
/// with "No provider set" on the first `Client::builder().build()`.
/// Reqwest 0.13 + rustls 0.23 require an explicit `install_default`
/// call before any TLS context is constructed — and reqwest constructs
/// one eagerly even when we only ever use plain HTTP (the L2c-2
/// opencode adapter talks to 127.0.0.1). Idempotent: `install_default`
/// returns Err on the second call, which we ignore.
pub(crate) fn install_rustls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Boots the Tauri runtime and blocks until the main window closes.
///
/// # Panics
///
/// Panics if the embedded Tauri context fails to build (missing config,
/// invalid capabilities, missing icon assets) — i.e. only on packaging
/// errors that would prevent the app from ever starting.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_rustls_provider();
    // The notifications plugin on macOS uses a native Swift bridge
    // (`default-features = false`) which requires the binary to live
    // inside a real `.app` bundle — its init step calls
    // `require_bundle()` and panics otherwise. `npm run tauri:dev`
    // runs the binary directly, so we skip registering it in
    // macOS-debug builds. Linux / Windows builds use the notify-rust
    // backend, which has no such requirement, so they always register
    // (both dev and release). Epic #50 L5b. Windows clicks don't come
    // through this plugin at all (#155) — see `os_notify.rs`, wired up
    // separately below via `generate_handler!`, not `.plugin(...)`.
    // `mut` is unused only in the macOS-debug case where no plugin is
    // added below; quiet the warning for that one path.
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init());
    #[cfg(any(not(target_os = "macos"), not(debug_assertions)))]
    {
        builder = builder.plugin(tauri_plugin_notifications::init());
    }
    builder
        .on_menu_event(|app, event| {
            // The macOS app menu (built in setup) drives this. Phase 4
            // wires a frontend listener for skein://open-settings to open
            // the settings modal; for now the event is fire-and-forget.
            if event.id() == "preferences" {
                use tauri::Emitter;
                let _ = app.emit("skein://open-settings", ());
            }
            if event.id() == "quit" {
                use tauri::Emitter;
                let _ = app.emit("skein://quit-requested", ());
            }
        })
        .setup(|app| {
            // Daily-rotating file log in the OS-conventional app log dir,
            // plus stderr (visible when launched from a terminal). The
            // bundled .app on macOS doesn't surface stderr anywhere
            // user-visible, so the file is what release-build debugging
            // actually relies on.
            //
            // macOS:   ~/Library/Logs/com.timeloop-vault.skein/skein.log.YYYY-MM-DD
            // Linux:   ~/.local/state/com.timeloop-vault.skein/logs/...
            // Windows: %LOCALAPPDATA%\com.timeloop-vault.skein\logs\...
            //
            // RUST_LOG env var overrides the default `info` level for
            // anyone debugging a particular subsystem (e.g.
            // `RUST_LOG=skein_app::pty=debug`).
            let log_dir = app.path().app_log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            let appender = tracing_appender::rolling::daily(&log_dir, "skein.log");
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(std::io::stderr.and(non_blocking))
                .init();
            app.manage(LogGuard(guard));
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                log_dir = %log_dir.display(),
                "Skein starting"
            );

            // #155: registers Skein's unpackaged-build AUMID and COM
            // toast activator on Windows so a notification click
            // reaches it from Action Center or a cold `-Embedding`
            // launch, not only while the banner itself is on screen.
            // A no-op on every other OS. Needs only the app handle
            // (config, its own app-data dir for the icon) — placed as
            // early in `setup()` as that allows, right after logging
            // comes up, because a cold `-Embedding` launch blocks on
            // this registration with a timeout rather than waiting
            // behind the DB open and agent-API bind below.
            // Best-effort — see `os_notify::init` — never aborts
            // startup.
            crate::os_notify::init(app.handle());

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

            // The agent-facing review API (#213). Bound with the std
            // listener so a failure is known *here*, synchronously, and
            // can be recorded — an async bind inside the spawned task
            // would fail into a log line nobody reads and leave the
            // settings pane claiming a port that never existed.
            //
            // 127.0.0.1 only, and an ephemeral port: the URL reaches
            // the harnesses through SKEIN_REVIEW_URL at spawn time, so
            // nothing has to agree on a number in advance.
            let endpoint = match std::net::TcpListener::bind(("127.0.0.1", 0))
                .and_then(|l| {
                    let port = l.local_addr()?.port();
                    l.set_nonblocking(true)?;
                    Ok((l, port))
                }) {
                Ok((listener, port)) => {
                    let state = Arc::new(crate::agent_api::AgentApiState::new(
                        Arc::clone(&db),
                        app.handle().clone(),
                    ));
                    // Managed so `agent_request_complete` (#328) can
                    // reach the same pending-request map the HTTP
                    // server's handlers register requests on.
                    app.manage(Arc::clone(&state));
                    tauri::async_runtime::spawn(async move {
                        match tokio::net::TcpListener::from_std(listener) {
                            Ok(listener) => crate::agent_api::http::serve(listener, state).await,
                            Err(e) => {
                                tracing::error!(error = %e, "agent api: adopting listener failed");
                            }
                        }
                    });
                    tracing::info!(port, "agent api listening on 127.0.0.1");
                    crate::agent_api::state::AgentApiEndpoint::bound(port)
                }
                Err(e) => {
                    // Not fatal: Skein is perfectly usable without the
                    // agent API. But it must be *visible* — an agent
                    // whose tools silently do not exist is the worst of
                    // both worlds (#176).
                    tracing::error!(error = %e, "agent api: bind failed; agents cannot reach the review");
                    crate::agent_api::state::AgentApiEndpoint::failed(e.to_string())
                }
            };
            app.manage(endpoint);

            app.manage(db);

            // Resolve the product name from the merged tauri config —
            // base config gives "Skein"; the dev overlay
            // (tauri.dev.conf.json, issue #21) gives "Skein (dev)" so
            // the window/dock visibly reflect which build is running.
            // The fallback covers the (impossible-in-practice) case
            // where productName is missing from config entirely.
            let product_name = app
                .config()
                .product_name
                .clone()
                .unwrap_or_else(|| "Skein".to_owned());

            // Sync the window title to product_name so Windows/Linux
            // taskbar + alt-tab labels follow the dev/release split.
            // (macOS hides the title text via hiddenTitle, but does the
            // right thing in the dock/app-menu via product_name.) Also:
            // tauri.conf.json sets decorations: true so macOS draws its
            // standard traffic-light controls (titleBarStyle: Overlay
            // requires decorations to be true at window-creation time).
            // On Windows / Linux we still want the chrome-less custom
            // titlebar with our own min/max/close — strip the native
            // chrome here. macOS-only fields (titleBarStyle, hiddenTitle)
            // are quietly ignored on those platforms.
            {
                let window = app
                    .get_webview_window("main")
                    .ok_or("main window missing during setup")?;
                window.set_title(&product_name)?;
                #[cfg(not(target_os = "macos"))]
                window.set_decorations(false)?;
            }

            // macOS expects an app menu — without one ⌘Q doesn't work,
            // there's no Edit menu for cut/copy/paste/select-all in
            // text fields, and the app feels web-shimmed. Tauri's
            // predefined items wrap AppKit's standard responder-chain
            // selectors, so they target the focused element (xterm
            // selection, modal text input, etc.) without per-surface
            // wiring.
            //
            // "Preferences…" is custom — it carries id "preferences"
            // and the on_menu_event handler above emits a tauri event
            // that phase 4's settings modal will listen for.
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{
                    AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder,
                };

                let about = AboutMetadataBuilder::new()
                    .name(Some(product_name.clone()))
                    .version(Some(env!("CARGO_PKG_VERSION")))
                    .build();

                let preferences = MenuItemBuilder::new("Preferences…")
                    .id("preferences")
                    .accelerator("CmdOrCtrl+,")
                    .build(app)?;

                // #185: NOT the predefined quit item — that fires
                // NSApp terminate: directly, bypassing the webview's
                // close-requested hook and the unsaved-editor-buffer
                // prompt. This routes Cmd+Q through the frontend,
                // which confirms and then destroys the window.
                let quit = MenuItemBuilder::new(format!("Quit {product_name}"))
                    .id("quit")
                    .accelerator("CmdOrCtrl+Q")
                    .build(app)?;

                let app_menu = SubmenuBuilder::new(app, &product_name)
                    .about(Some(about))
                    .separator()
                    .item(&preferences)
                    .separator()
                    .hide()
                    .hide_others()
                    .show_all()
                    .separator()
                    .item(&quit)
                    .build()?;

                let edit_menu = SubmenuBuilder::new(app, "Edit")
                    .undo()
                    .redo()
                    .separator()
                    .cut()
                    .copy()
                    .paste()
                    .select_all()
                    .build()?;

                let menu = MenuBuilder::new(app)
                    .items(&[&app_menu, &edit_menu])
                    .build()?;
                app.set_menu(menu)?;
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            fs::list_dir,
            fs::read_file_text,
            fs::write_file_text,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            default_shell,
            spawn_settings_load,
            spawn_settings_save,
            spawn_env_preview,
            spawn_env_reprobe,
            harness_config_status,
            list_harness_agents,
            default_cwd,
            db_load_rooms,
            db_save_rooms,
            git::git_is_repo,
            git::git_inspect_folder,
            git::git_head_branch,
            git::git_propose_worktree_path,
            git::git_add_worktree,
            git::git_restore_worktree,
            git::git_status,
            git::git_watch_start,
            git::git_watch_stop,
            git::git_diff,
            resume::opencode_list_sessions,
            resume::opencode_session_exists,
            resume::claude_session_exists,
            claude_events_attach,
            claude_events_detach,
            opencode_events_attach,
            opencode_events_detach,
            pick_free_port,
            db_record_harness_event,
            db_recent_harness_events_by_harness,
            db_recent_harness_events_by_room,
            db_record_harness_action,
            db_recent_harness_actions_by_harness,
            db_recent_harness_actions_by_room,
            db_recent_harness_actions_by_room_and_kind,
            review::review_pending,
            review::review_accept,
            review::review_reject,
            review::review_discovery_start,
            review_surface::commands::review_scope,
            review_surface::commands::review_file,
            review_surface::commands::review_add_thread,
            review_surface::commands::review_reply,
            review_surface::commands::review_edit_comment,
            review_surface::commands::review_delete_comment,
            review_surface::commands::review_delete_thread,
            review_surface::commands::review_resolve_thread,
            review_surface::commands::review_mark_viewed,
            review_surface::commands::review_set_base,
            review_surface::commands::review_signoff_status,
            review_surface::commands::review_set_signoff,
            agent_api::commands::agent_api_status,
            agent_api::commands::mail_unread,
            agent_api::commands::agent_request_complete,
            os_notify::os_notify_show,
            os_notify::os_notify_take_pending,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Smoke-test command — useful while wiring up the front/back bridge.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn ping(message: String) -> String {
    format!("pong: {message}")
}

/// Wire shape for `pty_spawn`'s return value.
///
/// `injected` names whether #215's config injection actually happened
/// for this spawn (a non-empty `Injection`) — the one thing the
/// frontend cannot compute itself, since `injection_for` lives entirely
/// on the Rust side. #238's nudge gate refuses to send a prompt to a
/// harness whose CLI might not even have the review tools wired up.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtySpawnResult {
    id: String,
    injected: bool,
}

/// Spawn a child process attached to a fresh PTY and stream its output
/// over `on_output`. Returns an opaque id the frontend uses for follow-up
/// calls, plus whether #215's config injection happened for this spawn.
///
/// `cmd` is argv-style: the first element is the program, the rest are
/// arguments. Empty `cmd` is rejected. `cwd` must exist.
///
/// `kind` is the harness kind, which decides what #215 injects so the
/// agent can reach the review API. It is passed separately from `cmd`
/// because the two can legitimately disagree — the user can swap a
/// harness's command without changing what the harness is. Tauri
/// deserializes it as `HarnessKind` (#116), so an unknown kind string
/// fails the invoke outright rather than silently injecting nothing.
///
/// Async: `PtyManager::spawn` calls into `apply_env`, which reads the
/// login-shell probe result and can block waiting on it for up to
/// `PROBE_WAIT` (6 s, #171/#177) — plus the actual child-process spawn.
/// None of the managed state here is `Arc`-wrapped, so the blocking
/// closure re-resolves each one off an owned `AppHandle` instead of
/// trying to move a borrowed `State` into a `'static` closure.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn pty_spawn(
    cmd: Vec<String>,
    cwd: String,
    rows: u16,
    cols: u16,
    room_id: String,
    harness_id: String,
    kind: crate::harness_kind::HarnessKind,
    on_event: Channel<PtyEvent>,
    app: tauri::AppHandle,
) -> Result<PtySpawnResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<PtyManager>();
        let spawn_env = app.state::<SpawnEnvState>();
        let db = app.state::<Arc<Database>>();
        let endpoint = app.state::<crate::agent_api::state::AgentApiEndpoint>();
        let harness_config = app.state::<crate::harness_config::HarnessConfig>();

        let id = uuid::Uuid::new_v4().to_string();
        let settings = spawn_env.snapshot();
        // #213: mint (or reuse) the room's review token and hand the
        // harness its endpoint. A failure here is not a reason to
        // refuse the spawn — the terminal still works, the agent
        // simply has no review tools — but it is logged rather than
        // swallowed (#176).
        let agent = endpoint.mcp_url().and_then(|url| {
            match db.ensure_room_token(&room_id, crate::review::now_ms()) {
                Ok(token) => Some(crate::agent_api::state::HarnessIdentity {
                    url,
                    token,
                    room_id: room_id.clone(),
                    harness_id: harness_id.clone(),
                }),
                Err(e) => {
                    tracing::error!(room_id, error = %e, "agent api: minting a room token failed");
                    None
                }
            }
        });
        let injected = manager
            .spawn(
                crate::pty::SpawnRequest {
                    id: id.clone(),
                    cmd: &cmd,
                    cwd: Path::new(&cwd),
                    rows,
                    cols,
                    settings: &settings,
                    agent: agent.as_ref(),
                    kind,
                    harness_config: Some(&harness_config),
                },
                move |event| {
                    // Channel send only fails if the frontend dropped
                    // the channel; nothing useful we can do at that
                    // point.
                    let _ = on_event.send(event);
                },
            )
            .map_err(|e| e.to_string())?;
        Ok(PtySpawnResult { id, injected })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Forward stdin bytes to the child. `data` is the raw string xterm.js
/// gives us from `term.onData`.
///
/// Deliberately sync (#171): keystroke ordering to a PTY must stay
/// FIFO, and async scheduling could let two writes to the same PTY
/// invert.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_write(
    id: String,
    data: String,
    manager: tauri::State<'_, PtyManager>,
) -> Result<(), String> {
    manager
        .write(&id, data.as_bytes())
        .map_err(|e| e.to_string())
}

// Deliberately sync (#171): same FIFO-ordering reasoning as `pty_write`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_resize(
    id: String,
    rows: u16,
    cols: u16,
    manager: tauri::State<'_, PtyManager>,
) -> Result<(), String> {
    manager.resize(&id, rows, cols).map_err(|e| e.to_string())
}

// Deliberately sync (#171): same FIFO-ordering reasoning as `pty_write`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_kill(id: String, manager: tauri::State<'_, PtyManager>) {
    manager.kill(&id);
}

/// Allocate a free TCP port on `127.0.0.1` for opencode's embedded
/// HTTP server. Epic #50 L2c-2.
///
/// Implementation: bind a `TcpListener` to port 0, read the OS-assigned
/// port, drop the listener. There's a small race window between drop
/// and the next process binding it (~microseconds typically), but it
/// only matters if another process simultaneously asks for a free
/// port and beats opencode to bind. In practice we've never observed
/// it; if it ever happens opencode reports the bind error in the PTY
/// and the user can restart the harness.
#[tauri::command]
fn pick_free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("pick_free_port: {e}"))
}

/// Start tailing the Claude JSONL session log for a harness. Emits
/// semantic `ClaudeEvent` values over `on_event` whenever the file
/// grows. Epic #50 L2c-1.
///
/// Purely additive: failing to attach (HOME unset, parent dir
/// unwriteable, watcher init failure) just means the harness falls
/// back to the L2a idle heuristic. We surface the error as a string
/// for the frontend to log, but the frontend treats it as soft —
/// notifications keep working from the chunk-based path.
///
/// Async: on first attach, `ClaudeEventsManager::attach` reads the
/// whole existing JSONL file to derive the current phase and backfill
/// actions (#80) before it starts tailing — for a long-running resumed
/// conversation that is real disk I/O, off the main thread (#171).
/// `ClaudeEventsManager` isn't `Arc`-wrapped, so the blocking closure
/// re-resolves it off an owned `AppHandle`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn claude_events_attach(
    harness_id: String,
    room_id: String,
    session_id: String,
    cwd: String,
    on_event: Channel<ClaudeEvent>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // #362: the frontend only `console.warn`s an Err from this command
    // (see harnessEvents.ts) and otherwise falls back silently to the
    // L2a idle heuristic — so an attach that fails, or never happens,
    // has been invisible on the Rust side. Every line below carries
    // `harness_id` so one harness's attach/detach/tick history can be
    // grepped out of a log with many rooms interleaved.
    tracing::info!(
        harness_id,
        room_id,
        session_id,
        "claude_events_attach: called"
    );
    let started = std::time::Instant::now();
    let log_harness_id = harness_id.clone();
    let send_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let send_failed_cb = Arc::clone(&send_failed);
    let send_failed_harness_id = harness_id.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<ClaudeEventsManager>();
        manager.attach(harness_id, room_id, &session_id, &cwd, move |event| {
            if on_event.send(event).is_err()
                && !send_failed_cb.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                // Only logged once per attach — a dead webview channel
                // (window closed, harness torn down) fails on every
                // subsequent event otherwise, and that's noise, not
                // new information.
                tracing::warn!(
                    harness_id = %send_failed_harness_id,
                    "claude_events_attach: channel send failed; webview likely gone"
                );
            }
        })
    })
    .await;

    match join_result {
        Ok(Ok(info)) => {
            tracing::info!(
                harness_id = %log_harness_id,
                path = %info.path.display(),
                already_existed = info.already_existed,
                subagents_armed = info.subagents_armed,
                duration_ms = started.elapsed().as_millis(),
                "claude_events_attach: attached"
            );
            Ok(())
        }
        Ok(Err(e)) => {
            tracing::warn!(
                harness_id = %log_harness_id,
                error = %e,
                "claude_events_attach: failed"
            );
            Err(e.to_string())
        }
        Err(join_error) => {
            // The blocking closure panicked — `attach`'s own error
            // path never runs, so without this the frontend just sees
            // its request hang, then error out with no explanation.
            tracing::warn!(
                harness_id = %log_harness_id,
                error = %join_error,
                "claude_events_attach: spawn_blocking join failed (panic inside attach?)"
            );
            Err(join_error.to_string())
        }
    }
}

/// Stop tailing the JSONL for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn claude_events_detach(harness_id: String, manager: tauri::State<'_, ClaudeEventsManager>) {
    manager.detach(&harness_id);
}

/// Start subscribing to opencode's `/event` SSE stream on `127.0.0.1:<port>`.
/// Epic #50 L2c-2.
///
/// Symmetrical with `claude_events_attach`: pass an `on_event` Channel
/// that receives semantic `OpencodeEvent` values. The adapter handles
/// initial-connect race (opencode hasn't bound the port yet) and
/// mid-session disconnects via exponential backoff — see
/// `harness_events_opencode::run_adapter`.
///
/// `async fn` so Tauri runs us on its tokio executor — the manager
/// calls `tokio::spawn` internally to launch the background SSE
/// reader, and that requires a runtime context. The function itself
/// returns immediately; the spawned task lives on until
/// `opencode_events_detach` is called or the manager is dropped.
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
#[tauri::command]
async fn opencode_events_attach(
    harness_id: String,
    room_id: String,
    cwd: String,
    port: u16,
    session_id: Option<String>,
    on_event: Channel<OpencodeEvent>,
    manager: tauri::State<'_, OpencodeEventsManager>,
) -> Result<(), String> {
    manager.attach(harness_id, room_id, cwd, port, session_id, move |event| {
        let _ = on_event.send(event);
    });
    Ok(())
}

/// Stop the SSE subscription for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn opencode_events_detach(harness_id: String, manager: tauri::State<'_, OpencodeEventsManager>) {
    manager.detach(&harness_id);
}

/// Append one row to the `harness_events` log. Epic #50 L6.
///
/// Called by the frontend's transition listener — once per real
/// phase change. Fire-and-forget from the TS side; we still surface
/// errors as `String` so the caller can console.warn if something
/// goes wrong (most likely "disk full" or a corrupted DB; nothing
/// actionable from the user's perspective beyond seeing a log).
///
/// `source` is reserved for L7 attribution ("which adapter event
/// drove this transition"). For v1 the frontend passes `None`.
///
/// Async: an sqlite insert — off the main thread (#171).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn db_record_harness_event(
    harness_id: String,
    room_id: String,
    from_phase: String,
    to_phase: String,
    timestamp_ms: i64,
    has_user_input: bool,
    source: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.record_harness_event(
            &harness_id,
            &room_id,
            &from_phase,
            &to_phase,
            timestamp_ms,
            has_user_input,
            source.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent events for a single harness. Newest-first. Epic #50 L6.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
async fn db_recent_harness_events_by_harness(
    harness_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessEvent>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_events_by_harness(&harness_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent events across every harness in a room. Newest-first.
/// Epic #50 L6 — foundation for the L7 activity feed.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
async fn db_recent_harness_events_by_room(
    room_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessEvent>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_events_by_room(&room_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Append one row to the `harness_actions` log. Issue #80.
///
/// Adapters call this per extracted action. `kind` should be one of
/// the `action_kind` constants. `payload` is an opaque JSON string —
/// the canonical shape per kind is documented in the design brief.
/// `source` carries the adapter event id (mirrors the L7a `source`
/// column on `harness_events`).
///
/// Async: an sqlite insert, called once per extracted action — off
/// the main thread so an agent's tool-call storm can't queue behind
/// it (#171).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn db_record_harness_action(
    harness_id: String,
    room_id: String,
    timestamp_ms: i64,
    kind: String,
    payload: String,
    source: Option<String>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.record_harness_action(
            &harness_id,
            &room_id,
            timestamp_ms,
            &kind,
            &payload,
            source.as_deref(),
        )
        .map(drop)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent actions for a single harness. Newest-first.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
async fn db_recent_harness_actions_by_harness(
    harness_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessAction>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_actions_by_harness(&harness_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent actions across every harness in a room. Newest-first.
/// Backs the Activity card.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
async fn db_recent_harness_actions_by_room(
    room_id: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessAction>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_actions_by_room(&room_id, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read recent actions of a single `kind` in a room. Backs the Plan
/// card (`kind = "plan_change"`) and other per-kind surfaces.
///
/// Async: an sqlite query — off the main thread (#171).
#[tauri::command]
async fn db_recent_harness_actions_by_room_and_kind(
    room_id: String,
    kind: String,
    since_ms: i64,
    limit: i64,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<HarnessAction>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || {
        db.recent_harness_actions_by_room_and_kind(&room_id, &kind, since_ms, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Wire shape for the Settings "Shell & environment" section.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SpawnSettingsPayload {
    settings: SpawnSettings,
    /// Non-null when the settings file exists but couldn't be used.
    degraded: Option<String>,
    /// Shown in the UI so the file can be hand-edited or backed up.
    settings_path: String,
}

fn spawn_settings_payload(state: &SpawnEnvState) -> SpawnSettingsPayload {
    SpawnSettingsPayload {
        settings: state.snapshot(),
        degraded: state.degraded.read().clone(),
        settings_path: crate::spawn_settings::file_path(&state.data_dir)
            .display()
            .to_string(),
    }
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn spawn_settings_load(spawn_env: tauri::State<'_, SpawnEnvState>) -> SpawnSettingsPayload {
    spawn_settings_payload(&spawn_env)
}

/// Persist the environment settings and immediately re-probe.
///
/// Re-probing here rather than lazily matters: the shell the user just
/// picked is the shell whose `PATH` the next harness should get, and a
/// spawn racing the save would otherwise keep the old shell's answer for
/// the rest of the process's life.
///
/// Note the two different lifetimes, which the UI states explicitly:
/// `PATH` additions and the captured environment apply to the *next
/// spawn of any harness*, whereas the shell is baked into `Harness.cmd`
/// when a shell harness is created and persisted from there — so
/// existing harnesses are never rewritten behind the user's back.
///
/// Async: writes the settings file to disk (#171). `SpawnEnvState`
/// isn't `Arc`-wrapped, so the blocking closure re-resolves it off an
/// owned `AppHandle` rather than trying to move the `State` borrow
/// into a `'static` closure.
#[tauri::command]
async fn spawn_settings_save(
    settings: SpawnSettings,
    app: tauri::AppHandle,
) -> Result<SpawnSettingsPayload, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spawn_env = app.state::<SpawnEnvState>();
        crate::spawn_settings::save(&spawn_env.data_dir, &settings)?;
        *spawn_env.settings.write() = settings.clone();
        // A successful save replaces whatever was unreadable before.
        *spawn_env.degraded.write() = None;
        crate::pty::reprobe(spawn_env.data_dir.clone(), settings);
        Ok(spawn_settings_payload(&spawn_env))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The environment a harness would get if it spawned right now.
///
/// Built by the *same* code the spawn path runs, so the panel can't
/// drift from reality.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn spawn_env_preview(spawn_env: tauri::State<'_, SpawnEnvState>) -> crate::pty::EnvPreview {
    crate::pty::env_preview(&spawn_env.snapshot())
}

/// Which agents `kind` will accept at `--agent` for a harness spawned
/// in `cwd` (#246). Answers the picker in #247.
///
/// `async` deliberately: this shells out to the harness CLI, so a sync
/// command would run the subprocess on the main thread and stall the
/// event loop for as long as the CLI takes to start (#171). Never
/// `Err` for "found nothing" — the DTO carries `degraded` and
/// `unsupported` instead, because a picker needs a list plus a reason
/// rather than a failure (#176).
///
/// The body then goes to `spawn_blocking`, because `async` alone only
/// moves the block off the main thread and onto a tokio worker. #247
/// made this a per-spawn call rather than a per-picker-open one, so a
/// boot that restores several agent-bearing harnesses fires several at
/// once — and the axum agent API (#213) runs on those same workers.
///
/// `kind` deserializes as `HarnessKind` (#116): an unknown kind string
/// fails the invoke instead of falling through to `unsupported`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
async fn list_harness_agents(
    kind: crate::harness_kind::HarnessKind,
    cwd: String,
    spawn_env: tauri::State<'_, SpawnEnvState>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<crate::agents::AgentListDto, String> {
    let settings = spawn_env.snapshot();
    // Cloned rather than borrowed: the closure outlives this frame as
    // far as the compiler is concerned, and the config is a handful of
    // paths.
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::agents::list(kind, &cwd, &settings, Some(&config))
    })
    .await
    .map_err(|e| format!("agent discovery panicked: {e}"))
}

/// What Skein injects into each agent CLI so it can reach the review
/// API, and where the shipped bundle resolved to (#215 E3). Read by the
/// spawn-environment panel — the injection is additive, but the user
/// still gets to see it and switch it off.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn harness_config_status(
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> crate::harness_config::HarnessConfigStatus {
    harness_config.status()
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn spawn_env_reprobe(spawn_env: tauri::State<'_, SpawnEnvState>) {
    crate::pty::reprobe(spawn_env.data_dir.clone(), spawn_env.snapshot());
}

/// argv for the user's default interactive shell on this platform. Used
/// as the fallback when the new-harness picker doesn't have a more
/// specific binary in mind.
///
/// On Windows we prefer `pwsh.exe` (`PowerShell` 7) when it's on PATH —
/// it has better ANSI/UTF-8 handling — and fall back to `powershell.exe`
/// (`PowerShell` 5.1, which ships with every modern Windows install).
///
/// On Unix this delegates to the same resolution the `PATH` probe uses,
/// so a harness shell and the shell we asked for a `PATH` can never
/// disagree. It previously had its own copy that fell back to
/// `/bin/bash` when `$SHELL` was unset — the fix for that landed in the
/// probe in 2026-05 and was never applied here, leaving a bundled-app
/// shell harness reading `~/.bash_profile`. On a machine whose Homebrew
/// setup lives in `~/.zprofile` (the default `brew shellenv` install),
/// that yields a shell with no `/opt/homebrew/bin` at all.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn default_shell(spawn_env: tauri::State<'_, SpawnEnvState>) -> Vec<String> {
    #[cfg(windows)]
    {
        if let Some(shell) = spawn_env.snapshot().valid_shell() {
            return vec![shell.to_owned()];
        }
        let pwsh_on_path = std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|dir| dir.join("pwsh.exe").is_file()));
        if pwsh_on_path {
            vec!["pwsh.exe".into()]
        } else {
            vec!["powershell.exe".into()]
        }
    }
    #[cfg(not(windows))]
    {
        // `probe_shell` already prefers the configured shell, so a
        // harness shell and the shell we asked for a PATH can never
        // disagree.
        vec![crate::pty::probe_shell(&spawn_env.snapshot())]
    }
}

/// User's home directory, cross-platform. Windows keeps it in
/// `USERPROFILE`; Unix in `HOME`. `None` only in exotic environments
/// where neither is set (some CI images / sandboxes). The single
/// source of truth for "where does this tool keep its dotfiles" —
/// reading `HOME` directly is wrong on Windows, where it's usually
/// unset.
pub(crate) fn home_dir() -> Option<std::path::PathBuf> {
    skein_harness::home_dir()
}

/// User's home directory as a path string. Used as the default cwd
/// for newly-spawned harnesses until Phase 4 wires real worktrees.
#[tauri::command]
fn default_cwd() -> String {
    home_dir()
        .and_then(|p| p.to_str().map(str::to_owned))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned))
        })
        .unwrap_or_else(|| ".".into())
}

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
async fn db_load_rooms(db: tauri::State<'_, Arc<Database>>) -> Result<LoadOutcome, String> {
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
async fn db_save_rooms(
    rooms: Vec<Room>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    let seq = db.next_save_seq();
    tauri::async_runtime::spawn_blocking(move || db.save_all_seq(&rooms, seq))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::PtySpawnResult;

    /// #196 regression pin: with an onCloseRequested handler
    /// registered, Tauri's JS wrapper closes the window via
    /// `destroy()` — if the capability file stops granting it, every
    /// close path (close button, Cmd+Q) silently dies. 0.2.6 shipped
    /// that way; never again.
    #[test]
    fn close_paths_keep_their_window_capabilities() {
        let caps = include_str!("../capabilities/default.json");
        assert!(caps.contains("core:window:allow-close"));
        assert!(caps.contains("core:window:allow-destroy"));
    }

    /// #238: the frontend's `harnessInput` gate reads `injected` off
    /// `pty_spawn`'s resolved value — pin the wire shape so a field
    /// rename here doesn't silently turn into `undefined` there.
    #[test]
    fn pty_spawn_result_serializes_as_camel_case() {
        let json = serde_json::to_value(PtySpawnResult {
            id: "abc".to_owned(),
            injected: true,
        })
        .expect("serialize");
        assert_eq!(json, serde_json::json!({ "id": "abc", "injected": true }));
    }
}
