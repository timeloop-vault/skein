//! Process-level commands: the bridge smoke test and the frontend's
//! narrow seam into `skein.log`.

/// Smoke-test command — useful while wiring up the front/back bridge.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn ping(message: String) -> String {
    format!("pong: {message}")
}

/// Forward one frontend log line into `skein.log` (#362). Exists so
/// `attachClaudeEvents`'s own attach/detach calls and their rejections
/// are visible on the Rust side — devtools console output is lost in
/// release builds, so "attach never called" was indistinguishable from
/// "invoke rejected before the Rust handler ran". This is NOT a general
/// logging framework: it's a narrow seam for that one adapter, with a
/// length cap so a runaway caller can't bloat the log file. Levels:
/// debug/info/warn/error (anything else logs as info).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn frontend_log(level: String, target: String, message: String) {
    const MAX_LEN: usize = 2000;
    let truncated = message.chars().count() > MAX_LEN;
    let message: String = message.chars().take(MAX_LEN).collect();
    if level == "debug" {
        tracing::debug!(source = "frontend", target = %target, truncated, "{message}");
    } else if level == "warn" && target == "perf" {
        let (cpu, disk, queue) = crate::perf_load::current_fields();
        tracing::warn!(
            source = "frontend",
            target = %target,
            truncated,
            cpu_pct = %cpu,
            disk_busy_pct = %disk,
            disk_queue = %queue,
            "{message}"
        );
    } else if level == "warn" {
        tracing::warn!(source = "frontend", target = %target, truncated, "{message}");
    } else if level == "error" {
        tracing::error!(source = "frontend", target = %target, truncated, "{message}");
    } else {
        tracing::info!(source = "frontend", target = %target, truncated, "{message}");
    }
}

/// Log the frontend's periodic `perf summary` line (#655) with the Rust
/// span aggregates appended, so one line covers both sides. Same length
/// cap as `frontend_log`; the cap applies to the frontend part only.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn perf_summary_log(line: String) {
    const MAX_LEN: usize = 2000;
    let truncated = line.chars().count() > MAX_LEN;
    let line: String = line.chars().take(MAX_LEN).collect();
    let mut line = crate::perf_spans::drain_and_append(&line);
    if let Some(seg) = crate::perf_load::window_segment() {
        line.push_str(&seg);
    }
    tracing::info!(source = "frontend", target = "perf", truncated, "{line}");
}

/// Raise and focus the main window (#493). Called from the Control
/// Center focus listener in the main window's JS: the pop-out only emits
/// the focus event, and main raises itself through this command.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn window_raise_main(app: tauri::AppHandle) {
    crate::setup::raise_main_window(&app);
}

/// Relaunch Skein into an update the updater installed in place (#692).
/// `request_restart` exits through `RunEvent::Exit` first, so the
/// sidecar cleanup in `lib.rs` and the single-instance socket release
/// both run before the new process starts. The frontend has already
/// landed the rooms save and confirmed unsaved Files buffers: the exit
/// tears the windows down without a `CloseRequested`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn app_restart(app: tauri::AppHandle) {
    tracing::info!("restart requested from the updater");
    app.request_restart();
}
