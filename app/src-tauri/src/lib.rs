//! Skein — Tauri shell entrypoint.
//!
//! Phase 1 wires PTY commands so the frontend can spawn a real
//! interactive terminal inside the harness pane. Each spawn produces
//! an id; subsequent writes/resizes/kills are keyed by it. Output
//! streams back over a per-spawn `tauri::ipc::Channel<String>`.

mod db;
mod fs;
mod git;
mod pty;
mod resume;
mod watcher;

use std::path::Path;

use tauri::Manager;
use tauri::ipc::Channel;
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::db::{Database, Room};
use crate::pty::{PtyEvent, PtyManager};
use crate::watcher::WatcherManager;

/// Hold the non-blocking tracing-appender guard for the lifetime of
/// the app. Dropping it stops the background flush thread; pending
/// log lines from just-before-quit can be lost. Kept in Tauri state
/// so it lives until process exit. The guard's value is never read —
/// only its `Drop` matters.
#[allow(dead_code)]
struct LogGuard(tracing_appender::non_blocking::WorkerGuard);

/// Boots the Tauri runtime and blocks until the main window closes.
///
/// # Panics
///
/// Panics if the embedded Tauri context fails to build (missing config,
/// invalid capabilities, missing icon assets) — i.e. only on packaging
/// errors that would prevent the app from ever starting.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .on_menu_event(|app, event| {
            // The macOS app menu (built in setup) drives this. Phase 4
            // wires a frontend listener for skein://open-settings to open
            // the settings modal; for now the event is fire-and-forget.
            if event.id() == "preferences" {
                use tauri::Emitter;
                let _ = app.emit("skein://open-settings", ());
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

            // Persist Skein state under the OS-conventional app data dir
            // (e.g. %APPDATA%/com.timeloop-vault.skein on Windows). Create
            // the directory eagerly so first-launch users don't see a
            // misleading "DB open failed" before they've created anything.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("skein.db");
            let db = Database::open(&db_path).map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("opening {}: {e}", db_path.display()))
            })?;
            app.manage(db);
            app.manage(PtyManager::new());
            app.manage(WatcherManager::new());

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

                let app_menu = SubmenuBuilder::new(app, &product_name)
                    .about(Some(about))
                    .separator()
                    .item(&preferences)
                    .separator()
                    .hide()
                    .hide_others()
                    .show_all()
                    .separator()
                    .quit()
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
            fs::read_file_bytes,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            default_shell,
            default_cwd,
            db_load_rooms,
            db_save_rooms,
            git::git_is_repo,
            git::git_branches,
            git::git_head_branch,
            git::git_propose_worktree_path,
            git::git_add_worktree,
            git::git_status,
            git::git_watch_start,
            git::git_watch_stop,
            git::git_diff,
            resume::opencode_list_sessions,
            resume::opencode_session_exists,
            resume::claude_session_exists,
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

/// Spawn a child process attached to a fresh PTY and stream its output
/// over `on_output`. Returns an opaque id the frontend uses for follow-up
/// calls.
///
/// `cmd` is argv-style: the first element is the program, the rest are
/// arguments. Empty `cmd` is rejected. `cwd` must exist.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_spawn(
    cmd: Vec<String>,
    cwd: String,
    rows: u16,
    cols: u16,
    on_event: Channel<PtyEvent>,
    manager: tauri::State<'_, PtyManager>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    manager
        .spawn(
            id.clone(),
            &cmd,
            Path::new(&cwd),
            rows,
            cols,
            move |event| {
                // Channel send only fails if the frontend dropped the
                // channel; nothing useful we can do at that point.
                let _ = on_event.send(event);
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Forward stdin bytes to the child. `data` is the raw string xterm.js
/// gives us from `term.onData`.
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

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_kill(id: String, manager: tauri::State<'_, PtyManager>) {
    manager.kill(&id);
}

/// argv for the user's default interactive shell on this platform. Used
/// as the fallback when the new-harness picker doesn't have a more
/// specific binary in mind.
///
/// On Windows we prefer `pwsh.exe` (`PowerShell` 7) when it's on PATH —
/// it has better ANSI/UTF-8 handling — and fall back to `powershell.exe`
/// (`PowerShell` 5.1, which ships with every modern Windows install).
#[tauri::command]
fn default_shell() -> Vec<String> {
    if cfg!(windows) {
        let pwsh_on_path = std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|dir| dir.join("pwsh.exe").is_file()));
        if pwsh_on_path {
            vec!["pwsh.exe".into()]
        } else {
            vec!["powershell.exe".into()]
        }
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        vec![shell]
    }
}

/// User's home directory as a path string. Used as the default cwd
/// for newly-spawned harnesses until Phase 4 wires real worktrees.
#[tauri::command]
fn default_cwd() -> String {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(key)
        .ok()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned))
        })
        .unwrap_or_else(|| ".".into())
}

/// Returns every room currently in the DB. The frontend calls this
/// once at boot to hydrate state.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn db_load_rooms(db: tauri::State<'_, Database>) -> Result<Vec<Room>, String> {
    db.load_all()
}

/// Replaces the DB's room list wholesale. Called whenever the
/// frontend's rooms state changes — wipe-and-insert is fine at
/// prototype scale and avoids the bookkeeping of granular upserts.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn db_save_rooms(rooms: Vec<Room>, db: tauri::State<'_, Database>) -> Result<(), String> {
    db.save_all(&rooms)
}
