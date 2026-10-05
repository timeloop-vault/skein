//! Skein — Tauri shell entrypoint: builder, managed state, and the
//! command registry.
//!
//! PTY commands let the frontend spawn a real interactive terminal
//! inside the harness pane. Each spawn produces an id; subsequent
//! writes/resizes/kills are keyed by it. Output streams back over a
//! per-spawn `tauri::ipc::Channel<PtyEvent>` (tagged data/exit).
mod agent_api;
mod agents;
mod build_info;
mod cli_shim;
mod commands;
mod db;
mod design;
mod fs;
mod git;
mod harness_action_event;
mod harness_actions_claude;
mod harness_actions_opencode;
mod harness_config;
mod harness_events_claude;
mod harness_events_opencode;
mod harness_kind;
mod open_request;
mod os_notify;
mod pty;
mod resume;
mod review;
mod review_surface;
mod room_paths;
mod setup;
mod spawn_env;
mod spawn_settings;
mod watcher;

pub(crate) use commands::spawn_env::{SpawnEnvState, home_dir};
pub(crate) use setup::install_rustls_provider;

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
        // Epic #255. First, as the plugin requires: a second launch must
        // be turned away before anything else starts — two Skeins on
        // one skein.db erase each other's rooms (see `open_request`).
        // It hands the second launch's argv + cwd to the running one.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            crate::open_request::from_second_instance(app, &argv, &cwd);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init());
    #[cfg(any(not(target_os = "macos"), not(debug_assertions)))]
    {
        builder = builder.plugin(tauri_plugin_notifications::init());
    }
    builder
        .on_menu_event(setup::on_menu_event)
        .on_window_event(setup::on_window_event)
        .setup(setup::setup)
        .invoke_handler(tauri::generate_handler![
            commands::app::ping,
            commands::app::window_raise_main,
            fs::list_dir,
            fs::read_file_text,
            fs::write_file_text,
            fs::read_image_bytes,
            commands::pty::pty_spawn,
            commands::pty::pty_write,
            commands::pty::pty_resize,
            commands::pty::pty_kill,
            commands::pty::pty_scan_opencode,
            commands::spawn_env::default_shell,
            commands::spawn_env::spawn_settings_load,
            commands::spawn_env::spawn_settings_save,
            commands::spawn_env::spawn_env_preview,
            commands::spawn_env::spawn_env_reprobe,
            commands::spawn_env::harness_config_status,
            commands::spawn_env::list_harness_agents,
            commands::spawn_env::claude_cli_version,
            commands::spawn_env::default_cwd,
            commands::rooms::db_load_rooms,
            commands::rooms::db_save_rooms,
            git::git_is_repo,
            git::git_inspect_folder,
            git::git_repo_identity,
            git::git_head_branch,
            git::git_propose_worktree_path,
            git::git_add_worktree,
            git::git_restore_worktree,
            git::git_status,
            git::git_watch_start,
            git::git_watch_stop,
            design::commands::design_preview_base,
            design::commands::design_list_entries,
            design::commands::design_watch_start,
            git::git_diff,
            resume::opencode_list_sessions,
            resume::opencode_session_exists,
            resume::claude_session_exists,
            resume::claude_transcript_stat,
            commands::harness_events::claude_events_attach,
            commands::harness_events::claude_events_detach,
            commands::harness_events::claude_events_reattach,
            commands::app::frontend_log,
            commands::harness_events::opencode_events_attach,
            commands::harness_events::opencode_events_detach,
            commands::pty::pick_free_port,
            commands::harness_log::db_record_harness_event,
            commands::harness_log::db_recent_harness_events_by_harness,
            commands::harness_log::db_recent_harness_events_by_room,
            commands::harness_log::db_record_harness_action,
            commands::harness_log::db_recent_harness_actions_by_harness,
            commands::harness_log::db_recent_harness_actions_by_room,
            commands::harness_log::db_recent_harness_actions_by_room_and_kind,
            review::review_pending,
            review::review_accept,
            review::review_reject,
            review::review_discovery_start,
            review_surface::commands::review_scope,
            review_surface::commands::review_file,
            review_surface::commands::review_image_bytes,
            review_surface::commands::review_add_thread,
            review_surface::commands::review_element_seen,
            review_surface::commands::review_element_threads,
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
            agent_api::commands::mail_last_status_by_room,
            agent_api::commands::agent_request_complete,
            os_notify::os_notify_show,
            os_notify::os_notify_take_pending,
            open_request::open_request_take,
            cli_shim::cli_shim_status,
            cli_shim::cli_shim_install,
            cli_shim::cli_shim_uninstall,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
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
}
