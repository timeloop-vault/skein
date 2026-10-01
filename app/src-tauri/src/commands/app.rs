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
    } else if level == "warn" {
        tracing::warn!(source = "frontend", target = %target, truncated, "{message}");
    } else if level == "error" {
        tracing::error!(source = "frontend", target = %target, truncated, "{message}");
    } else {
        tracing::info!(source = "frontend", target = %target, truncated, "{message}");
    }
}
